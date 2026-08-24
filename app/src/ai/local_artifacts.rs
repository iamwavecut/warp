//! Durable local storage for file and screenshot artifacts.

use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use diesel::connection::SimpleConnection as _;
use diesel::sql_types::{BigInt, Nullable, Text};
use diesel::{Connection as _, OptionalExtension as _, QueryableByName, RunQueryDsl as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::util::image::{MIME_SNIFF_BYTES, infer_mime_type, is_supported_image_mime_type};

const LOCAL_ARTIFACT_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS local_artifacts (
    artifact_uid TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('file', 'screenshot')),
    local_path TEXT NOT NULL UNIQUE,
    source_path TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    checksum_sha256 TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    description TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS local_artifacts_created
    ON local_artifacts(created_at DESC, artifact_uid);
CREATE TABLE IF NOT EXISTS local_artifact_owners (
    artifact_uid TEXT NOT NULL REFERENCES local_artifacts(artifact_uid) ON DELETE CASCADE,
    owner_kind TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (artifact_uid, owner_kind, owner_id)
);
CREATE INDEX IF NOT EXISTS local_artifact_owners_owner
    ON local_artifact_owners(owner_kind, owner_id, artifact_uid);
"#;

const LOCAL_ARTIFACT_DIRECTORY: &str = "local-artifacts";
const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DESCRIPTION_CHARS: usize = 2_000;
const MAX_OWNER_FIELD_CHARS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalArtifactKind {
    File,
    Screenshot,
}

impl LocalArtifactKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Screenshot => "screenshot",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalArtifactOwner {
    pub kind: String,
    pub id: String,
}

impl LocalArtifactOwner {
    pub fn conversation(id: impl ToString) -> Self {
        Self {
            kind: "conversation".to_string(),
            id: id.to_string(),
        }
    }

    pub fn manual(id: impl Into<String>) -> Self {
        Self {
            kind: "manual".to_string(),
            id: id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalArtifactRecord {
    pub artifact_uid: Uuid,
    pub kind: LocalArtifactKind,
    pub local_path: PathBuf,
    pub source_path: PathBuf,
    pub filename: String,
    pub mime_type: String,
    pub checksum_sha256: String,
    pub size_bytes: i64,
    pub description: Option<String>,
    pub created_at: i64,
    pub owners: Vec<LocalArtifactOwner>,
}

#[derive(Debug, Error)]
pub enum LocalArtifactError {
    #[error("artifact source does not exist or is not a regular file: {0}")]
    InvalidSource(PathBuf),
    #[error("artifact source is empty")]
    EmptySource,
    #[error("artifact exceeds the local {MAX_ARTIFACT_BYTES} byte limit")]
    TooLarge,
    #[error("artifact description exceeds {MAX_DESCRIPTION_CHARS} characters")]
    DescriptionTooLong,
    #[error(
        "artifact owner kind and id must be non-empty and at most {MAX_OWNER_FIELD_CHARS} characters"
    )]
    InvalidOwner,
    #[error("local artifact {0} was not found")]
    NotFound(Uuid),
    #[error("local artifact path escaped the managed artifact directory")]
    UnsafeManagedPath,
    #[error("local artifact {artifact_uid} failed checksum verification")]
    ChecksumMismatch { artifact_uid: Uuid },
    #[error("invalid local artifact row: {0}")]
    Corrupt(String),
    #[error("local artifact storage error: {0}")]
    Storage(#[source] anyhow::Error),
}

#[derive(Clone, Debug)]
pub struct LocalArtifactRepository {
    database_path: PathBuf,
    root: PathBuf,
}

#[derive(QueryableByName)]
struct ArtifactDbRow {
    #[diesel(sql_type = Text)]
    artifact_uid: String,
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = Text)]
    local_path: String,
    #[diesel(sql_type = Text)]
    source_path: String,
    #[diesel(sql_type = Text)]
    filename: String,
    #[diesel(sql_type = Text)]
    mime_type: String,
    #[diesel(sql_type = Text)]
    checksum_sha256: String,
    #[diesel(sql_type = BigInt)]
    size_bytes: i64,
    #[diesel(sql_type = Nullable<Text>)]
    description: Option<String>,
    #[diesel(sql_type = BigInt)]
    created_at: i64,
}

#[derive(QueryableByName)]
struct OwnerDbRow {
    #[diesel(sql_type = Text)]
    owner_kind: String,
    #[diesel(sql_type = Text)]
    owner_id: String,
}

#[derive(QueryableByName)]
struct PathDbRow {
    #[diesel(sql_type = Text)]
    local_path: String,
}

impl LocalArtifactRepository {
    pub fn open_current_scope() -> Result<Self, LocalArtifactError> {
        let database_path = crate::persistence::database_file_path_for_current_scope();
        let parent = database_path.parent().ok_or_else(|| {
            storage_error(format!(
                "local persistence path has no parent: {}",
                database_path.display()
            ))
        })?;
        Self::open(database_path.clone(), parent.join(LOCAL_ARTIFACT_DIRECTORY))
    }

    pub fn in_directory(directory: impl AsRef<Path>) -> Result<Self, LocalArtifactError> {
        let directory = directory.as_ref();
        Self::open(
            directory.join("local-artifacts.sqlite"),
            directory.join(LOCAL_ARTIFACT_DIRECTORY),
        )
    }

    pub fn open(
        database_path: impl Into<PathBuf>,
        root: impl Into<PathBuf>,
    ) -> Result<Self, LocalArtifactError> {
        let mut repository = Self {
            database_path: database_path.into(),
            root: root.into(),
        };
        repository.initialize()?;
        repository.root = fs::canonicalize(&repository.root)
            .map_err(|error| storage_error(format!("resolving artifact root: {error}")))?;
        Ok(repository)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn import_path(
        &self,
        source: impl AsRef<Path>,
        owner: LocalArtifactOwner,
        description: Option<String>,
    ) -> Result<LocalArtifactRecord, LocalArtifactError> {
        validate_owner(&owner)?;
        let description = normalize_description(description)?;
        let source_input = source.as_ref();
        let source = fs::canonicalize(source_input)
            .map_err(|_| LocalArtifactError::InvalidSource(source_input.to_path_buf()))?;
        let metadata =
            fs::metadata(&source).map_err(|_| LocalArtifactError::InvalidSource(source.clone()))?;
        if !metadata.is_file() {
            return Err(LocalArtifactError::InvalidSource(source));
        }
        if metadata.len() == 0 {
            return Err(LocalArtifactError::EmptySource);
        }
        if metadata.len() > MAX_ARTIFACT_BYTES {
            return Err(LocalArtifactError::TooLarge);
        }

        let artifact_uid = Uuid::new_v4();
        let filename = source
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("artifact")
            .to_string();
        let uid_text = artifact_uid.simple().to_string();
        let destination_dir = self.objects_root().join(&uid_text[..2]);
        create_private_directory(&destination_dir)?;
        let destination_name = safe_extension(&source)
            .map(|extension| format!("{artifact_uid}.{extension}"))
            .unwrap_or_else(|| artifact_uid.to_string());
        let destination = destination_dir.join(destination_name);
        let temporary = destination_dir.join(format!(".{artifact_uid}.partial"));

        let (checksum_sha256, prefix, copied_bytes) = match copy_and_hash(&source, &temporary) {
            Ok(result) => result,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(storage_error(error));
            }
        };
        if copied_bytes != metadata.len() {
            let _ = fs::remove_file(&temporary);
            return Err(storage_error(format!(
                "artifact changed while being copied: expected {} bytes, copied {copied_bytes}",
                metadata.len()
            )));
        }
        if let Err(error) = set_owner_only_file_permissions(&temporary) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        fs::rename(&temporary, &destination).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            storage_error(format!(
                "moving artifact into {}: {error}",
                destination.display()
            ))
        })?;

        let mime_type = infer_mime_type(&source, &prefix);
        let kind = if is_supported_image_mime_type(&mime_type) {
            LocalArtifactKind::Screenshot
        } else {
            LocalArtifactKind::File
        };
        let created_at = chrono::Local::now().timestamp_millis();
        let record = LocalArtifactRecord {
            artifact_uid,
            kind,
            local_path: destination.clone(),
            source_path: source,
            filename,
            mime_type,
            checksum_sha256,
            size_bytes: i64::try_from(copied_bytes)
                .map_err(|error| storage_error(format!("artifact size overflow: {error}")))?,
            description,
            created_at,
            owners: vec![owner.clone()],
        };

        let insert_result = self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    diesel::sql_query(
                        "INSERT INTO local_artifacts (artifact_uid, kind, local_path, source_path, \
                         filename, mime_type, checksum_sha256, size_bytes, description, created_at) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind::<Text, _>(record.artifact_uid.to_string())
                    .bind::<Text, _>(record.kind.as_str())
                    .bind::<Text, _>(path_text(&record.local_path)?)
                    .bind::<Text, _>(path_text(&record.source_path)?)
                    .bind::<Text, _>(&record.filename)
                    .bind::<Text, _>(&record.mime_type)
                    .bind::<Text, _>(&record.checksum_sha256)
                    .bind::<BigInt, _>(record.size_bytes)
                    .bind::<Nullable<Text>, _>(record.description.as_deref())
                    .bind::<BigInt, _>(record.created_at)
                    .execute(connection)?;
                    insert_owner(connection, record.artifact_uid, &owner, created_at)?;
                    Ok(())
                })
                .map_err(|error| storage_error(format!("recording local artifact: {error}")))
        });
        if let Err(error) = insert_result {
            let _ = fs::remove_file(&destination);
            return Err(error);
        }
        Ok(record)
    }

    pub fn get(
        &self,
        artifact_uid: Uuid,
    ) -> Result<Option<LocalArtifactRecord>, LocalArtifactError> {
        self.with_connection(|connection| load_artifact(connection, artifact_uid))
    }

    pub fn list(
        &self,
        owner: Option<&LocalArtifactOwner>,
    ) -> Result<Vec<LocalArtifactRecord>, LocalArtifactError> {
        if let Some(owner) = owner {
            validate_owner(owner)?;
        }
        self.with_connection(|connection| {
            let rows = if let Some(owner) = owner {
                diesel::sql_query(
                    "SELECT a.artifact_uid, a.kind, a.local_path, a.source_path, a.filename, \
                     a.mime_type, a.checksum_sha256, a.size_bytes, a.description, a.created_at \
                     FROM local_artifacts a JOIN local_artifact_owners o \
                     ON o.artifact_uid = a.artifact_uid WHERE o.owner_kind = ? AND o.owner_id = ? \
                     ORDER BY a.created_at DESC, a.artifact_uid",
                )
                .bind::<Text, _>(&owner.kind)
                .bind::<Text, _>(&owner.id)
                .load::<ArtifactDbRow>(connection)
            } else {
                diesel::sql_query(
                    "SELECT artifact_uid, kind, local_path, source_path, filename, mime_type, \
                     checksum_sha256, size_bytes, description, created_at FROM local_artifacts \
                     ORDER BY created_at DESC, artifact_uid",
                )
                .load::<ArtifactDbRow>(connection)
            }
            .map_err(|error| storage_error(format!("listing local artifacts: {error}")))?;
            rows.into_iter()
                .map(|row| artifact_from_row(connection, row))
                .collect()
        })
    }

    pub fn attach_owner_if_present(
        &self,
        artifact_uid: Uuid,
        owner: &LocalArtifactOwner,
    ) -> Result<bool, LocalArtifactError> {
        validate_owner(owner)?;
        self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    #[derive(QueryableByName)]
                    struct CountRow {
                        #[diesel(sql_type = BigInt)]
                        count: i64,
                    }
                    let exists = diesel::sql_query(
                        "SELECT COUNT(*) AS count FROM local_artifacts WHERE artifact_uid = ?",
                    )
                    .bind::<Text, _>(artifact_uid.to_string())
                    .get_result::<CountRow>(connection)?
                    .count
                        == 1;
                    if exists {
                        insert_owner(
                            connection,
                            artifact_uid,
                            owner,
                            chrono::Local::now().timestamp_millis(),
                        )?;
                    }
                    Ok(exists)
                })
                .map_err(|error| storage_error(format!("attaching artifact owner: {error}")))
        })
    }

    pub fn release_owner(
        &self,
        owner: &LocalArtifactOwner,
    ) -> Result<Vec<Uuid>, LocalArtifactError> {
        validate_owner(owner)?;
        let removed = self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    #[derive(QueryableByName)]
                    struct OwnedRow {
                        #[diesel(sql_type = Text)]
                        artifact_uid: String,
                    }
                    let owned = diesel::sql_query(
                        "SELECT artifact_uid FROM local_artifact_owners \
                         WHERE owner_kind = ? AND owner_id = ?",
                    )
                    .bind::<Text, _>(&owner.kind)
                    .bind::<Text, _>(&owner.id)
                    .load::<OwnedRow>(connection)?;
                    diesel::sql_query(
                        "DELETE FROM local_artifact_owners WHERE owner_kind = ? AND owner_id = ?",
                    )
                    .bind::<Text, _>(&owner.kind)
                    .bind::<Text, _>(&owner.id)
                    .execute(connection)?;

                    let mut removed = Vec::new();
                    for owned in owned {
                        let uid = Uuid::parse_str(&owned.artifact_uid)?;
                        if owner_count(connection, uid)? == 0 {
                            let path = diesel::sql_query(
                                "SELECT local_path FROM local_artifacts WHERE artifact_uid = ?",
                            )
                            .bind::<Text, _>(uid.to_string())
                            .get_result::<PathDbRow>(connection)
                            .optional()?
                            .map(|row| PathBuf::from(row.local_path));
                            diesel::sql_query("DELETE FROM local_artifacts WHERE artifact_uid = ?")
                                .bind::<Text, _>(uid.to_string())
                                .execute(connection)?;
                            if let Some(path) = path {
                                removed.push((uid, path));
                            }
                        }
                    }
                    Ok(removed)
                })
                .map_err(|error| storage_error(format!("releasing artifact owner: {error}")))
        })?;

        let mut removed_uids = Vec::with_capacity(removed.len());
        for (uid, path) in removed {
            self.remove_managed_file(&path)?;
            removed_uids.push(uid);
        }
        Ok(removed_uids)
    }

    pub fn resolve_verified_path(
        &self,
        artifact_uid: Uuid,
    ) -> Result<LocalArtifactRecord, LocalArtifactError> {
        let mut record = self
            .get(artifact_uid)?
            .ok_or(LocalArtifactError::NotFound(artifact_uid))?;
        let path = self.validate_managed_file(&record.local_path)?;
        let checksum = checksum_file(&path).map_err(storage_error)?;
        if checksum != record.checksum_sha256 {
            return Err(LocalArtifactError::ChecksumMismatch { artifact_uid });
        }
        record.local_path = path;
        Ok(record)
    }

    pub fn cleanup_unowned(&self) -> Result<Vec<Uuid>, LocalArtifactError> {
        let removed = self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    #[derive(QueryableByName)]
                    struct UnownedRow {
                        #[diesel(sql_type = Text)]
                        artifact_uid: String,
                        #[diesel(sql_type = Text)]
                        local_path: String,
                    }
                    let rows = diesel::sql_query(
                        "SELECT a.artifact_uid, a.local_path FROM local_artifacts a \
                         LEFT JOIN local_artifact_owners o ON o.artifact_uid = a.artifact_uid \
                         WHERE o.artifact_uid IS NULL",
                    )
                    .load::<UnownedRow>(connection)?;
                    for row in &rows {
                        diesel::sql_query("DELETE FROM local_artifacts WHERE artifact_uid = ?")
                            .bind::<Text, _>(&row.artifact_uid)
                            .execute(connection)?;
                    }
                    Ok(rows)
                })
                .map_err(|error| storage_error(format!("cleaning local artifacts: {error}")))
        })?;
        let mut removed_uids = Vec::with_capacity(removed.len());
        for row in removed {
            let uid = Uuid::parse_str(&row.artifact_uid)
                .map_err(|error| LocalArtifactError::Corrupt(error.to_string()))?;
            self.remove_managed_file(Path::new(&row.local_path))?;
            removed_uids.push(uid);
        }
        Ok(removed_uids)
    }

    fn initialize(&self) -> Result<(), LocalArtifactError> {
        if let Some(parent) = self
            .database_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            create_private_directory(parent)?;
        }
        create_private_directory(&self.objects_root())?;
        self.with_connection(|_| Ok(()))
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut diesel::SqliteConnection) -> Result<T, LocalArtifactError>,
    ) -> Result<T, LocalArtifactError> {
        let database_path = self.database_path.to_str().ok_or_else(|| {
            storage_error(format!(
                "artifact database path is not valid UTF-8: {}",
                self.database_path.display()
            ))
        })?;
        let mut connection = diesel::SqliteConnection::establish(database_path)
            .map_err(|error| storage_error(format!("opening artifact database: {error}")))?;
        connection
            .batch_execute("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")
            .and_then(|_| connection.batch_execute(LOCAL_ARTIFACT_SCHEMA))
            .map_err(|error| storage_error(format!("initializing artifact schema: {error}")))?;
        operation(&mut connection)
    }

    fn objects_root(&self) -> PathBuf {
        self.root.join("objects")
    }

    fn validate_managed_file(&self, path: &Path) -> Result<PathBuf, LocalArtifactError> {
        let root = fs::canonicalize(self.objects_root())
            .map_err(|error| storage_error(format!("resolving artifact root: {error}")))?;
        let path = fs::canonicalize(path)
            .map_err(|error| storage_error(format!("resolving artifact path: {error}")))?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(LocalArtifactError::UnsafeManagedPath);
        }
        Ok(path)
    }

    fn remove_managed_file(&self, path: &Path) -> Result<(), LocalArtifactError> {
        if !path.exists() {
            return Ok(());
        }
        let path = self.validate_managed_file(path)?;
        fs::remove_file(&path).map_err(|error| {
            storage_error(format!(
                "removing local artifact {}: {error}",
                path.display()
            ))
        })?;
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir(parent);
        }
        Ok(())
    }
}

fn load_artifact(
    connection: &mut diesel::SqliteConnection,
    artifact_uid: Uuid,
) -> Result<Option<LocalArtifactRecord>, LocalArtifactError> {
    let row = diesel::sql_query(
        "SELECT artifact_uid, kind, local_path, source_path, filename, mime_type, \
         checksum_sha256, size_bytes, description, created_at FROM local_artifacts \
         WHERE artifact_uid = ?",
    )
    .bind::<Text, _>(artifact_uid.to_string())
    .get_result::<ArtifactDbRow>(connection)
    .optional()
    .map_err(|error| storage_error(format!("loading local artifact: {error}")))?;
    row.map(|row| artifact_from_row(connection, row))
        .transpose()
}

fn artifact_from_row(
    connection: &mut diesel::SqliteConnection,
    row: ArtifactDbRow,
) -> Result<LocalArtifactRecord, LocalArtifactError> {
    let artifact_uid = Uuid::parse_str(&row.artifact_uid)
        .map_err(|error| LocalArtifactError::Corrupt(format!("invalid uid: {error}")))?;
    let kind = match row.kind.as_str() {
        "file" => LocalArtifactKind::File,
        "screenshot" => LocalArtifactKind::Screenshot,
        value => {
            return Err(LocalArtifactError::Corrupt(format!(
                "invalid artifact kind {value}"
            )));
        }
    };
    if row.size_bytes < 1
        || row.checksum_sha256.len() != 64
        || !row
            .checksum_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(LocalArtifactError::Corrupt(
            "invalid size or checksum".to_string(),
        ));
    }
    let owners = diesel::sql_query(
        "SELECT owner_kind, owner_id FROM local_artifact_owners WHERE artifact_uid = ? \
         ORDER BY owner_kind, owner_id",
    )
    .bind::<Text, _>(artifact_uid.to_string())
    .load::<OwnerDbRow>(connection)
    .map_err(|error| storage_error(format!("loading artifact owners: {error}")))?
    .into_iter()
    .map(|row| LocalArtifactOwner {
        kind: row.owner_kind,
        id: row.owner_id,
    })
    .collect();
    Ok(LocalArtifactRecord {
        artifact_uid,
        kind,
        local_path: PathBuf::from(row.local_path),
        source_path: PathBuf::from(row.source_path),
        filename: row.filename,
        mime_type: row.mime_type,
        checksum_sha256: row.checksum_sha256,
        size_bytes: row.size_bytes,
        description: row.description,
        created_at: row.created_at,
        owners,
    })
}

fn insert_owner(
    connection: &mut diesel::SqliteConnection,
    artifact_uid: Uuid,
    owner: &LocalArtifactOwner,
    created_at: i64,
) -> Result<(), anyhow::Error> {
    diesel::sql_query(
        "INSERT OR IGNORE INTO local_artifact_owners \
         (artifact_uid, owner_kind, owner_id, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind::<Text, _>(artifact_uid.to_string())
    .bind::<Text, _>(&owner.kind)
    .bind::<Text, _>(&owner.id)
    .bind::<BigInt, _>(created_at)
    .execute(connection)?;
    Ok(())
}

fn owner_count(
    connection: &mut diesel::SqliteConnection,
    artifact_uid: Uuid,
) -> Result<i64, anyhow::Error> {
    #[derive(QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = BigInt)]
        count: i64,
    }
    Ok(diesel::sql_query(
        "SELECT COUNT(*) AS count FROM local_artifact_owners WHERE artifact_uid = ?",
    )
    .bind::<Text, _>(artifact_uid.to_string())
    .get_result::<CountRow>(connection)?
    .count)
}

fn validate_owner(owner: &LocalArtifactOwner) -> Result<(), LocalArtifactError> {
    let valid = [&owner.kind, &owner.id].into_iter().all(|value| {
        let value = value.trim();
        !value.is_empty()
            && value.chars().count() <= MAX_OWNER_FIELD_CHARS
            && !value.chars().any(char::is_control)
    });
    valid.then_some(()).ok_or(LocalArtifactError::InvalidOwner)
}

fn normalize_description(
    description: Option<String>,
) -> Result<Option<String>, LocalArtifactError> {
    let description = description
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if description
        .as_ref()
        .is_some_and(|value| value.chars().count() > MAX_DESCRIPTION_CHARS)
    {
        return Err(LocalArtifactError::DescriptionTooLong);
    }
    Ok(description)
}

fn safe_extension(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    (!extension.is_empty()
        && extension.len() <= 16
        && extension.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    .then_some(extension)
}

fn copy_and_hash(source: &Path, destination: &Path) -> anyhow::Result<(String, Vec<u8>, u64)> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut hasher = Sha256::new();
    let mut prefix = Vec::with_capacity(MIME_SNIFF_BYTES);
    let mut buffer = [0_u8; 64 * 1024];
    let mut copied = 0_u64;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .ok_or_else(|| anyhow::anyhow!("artifact size overflow"))?;
        if copied > MAX_ARTIFACT_BYTES {
            return Err(anyhow::anyhow!(
                "artifact exceeded size limit while copying"
            ));
        }
        if prefix.len() < MIME_SNIFF_BYTES {
            let remaining = MIME_SNIFF_BYTES - prefix.len();
            prefix.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        hasher.update(&buffer[..read]);
        output.write_all(&buffer[..read])?;
    }
    output.sync_all()?;
    Ok((hex::encode(hasher.finalize()), prefix, copied))
}

fn checksum_file(path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn path_text(path: &Path) -> Result<String, anyhow::Error> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("path is not valid UTF-8: {}", path.display()))
}

fn create_private_directory(path: &Path) -> Result<(), LocalArtifactError> {
    fs::create_dir_all(path).map_err(|error| {
        storage_error(format!(
            "creating artifact directory {}: {error}",
            path.display()
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
            storage_error(format!(
                "setting artifact directory permissions {}: {error}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

fn set_owner_only_file_permissions(path: &Path) -> Result<(), LocalArtifactError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            storage_error(format!(
                "setting artifact file permissions {}: {error}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

fn storage_error(error: impl std::fmt::Display) -> LocalArtifactError {
    LocalArtifactError::Storage(anyhow::anyhow!(error.to_string()))
}

#[cfg(test)]
#[path = "local_artifacts_tests.rs"]
mod tests;
