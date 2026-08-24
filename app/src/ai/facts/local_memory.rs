//! Durable, provider-independent storage and retrieval for user-managed memory.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use diesel::connection::SimpleConnection;
use diesel::sql_types::{BigInt, Text};
use diesel::{Connection, QueryableByName, RunQueryDsl, SqliteConnection, sql_query};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

const MAX_MEMORY_COUNT: i64 = 1_024;
const MAX_TITLE_CHARS: usize = 160;
const MAX_CONTENT_CHARS: usize = 16_000;
pub(crate) const MAX_CONTEXT_MEMORIES: usize = 8;
pub(crate) const MAX_CONTEXT_MEMORY_CHARS: usize = 6_000;
pub(crate) const MAX_CONTEXT_ITEM_CHARS: usize = 2_000;

const LOCAL_MEMORY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS local_memories (
    id TEXT PRIMARY KEY NOT NULL,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'project')),
    scope_key TEXT NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    revision INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS local_memories_scope_updated
    ON local_memories(scope_kind, scope_key, updated_at DESC, id);
CREATE TABLE IF NOT EXISTS local_memory_versions (
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('created', 'updated', 'deleted')),
    scope_kind TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    recorded_at INTEGER NOT NULL,
    PRIMARY KEY (memory_id, revision)
);
CREATE INDEX IF NOT EXISTS local_memory_versions_recorded
    ON local_memory_versions(memory_id, revision DESC);
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalMemoryScope {
    Global,
    Project { root: PathBuf },
}

impl LocalMemoryScope {
    pub fn display_name(&self) -> String {
        match self {
            Self::Global => "Global".to_string(),
            Self::Project { root } => format!("Project: {}", root.display()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalMemoryRecord {
    pub id: Uuid,
    pub scope: LocalMemoryScope,
    pub title: String,
    pub content: String,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalMemoryOperation {
    Created,
    Updated,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalMemoryVersion {
    pub memory_id: Uuid,
    pub revision: i64,
    pub operation: LocalMemoryOperation,
    pub scope: LocalMemoryScope,
    pub title: String,
    pub content: String,
    pub recorded_at: i64,
}

#[derive(Debug, Error)]
pub enum LocalMemoryError {
    #[error("memory title cannot be empty")]
    EmptyTitle,
    #[error("memory content cannot be empty")]
    EmptyContent,
    #[error("memory title exceeds {MAX_TITLE_CHARS} characters")]
    TitleTooLong,
    #[error("memory content exceeds {MAX_CONTENT_CHARS} characters")]
    ContentTooLong,
    #[error("project memory root does not exist or cannot be resolved: {0}")]
    InvalidProjectRoot(PathBuf),
    #[error("local memory limit of {MAX_MEMORY_COUNT} entries has been reached")]
    LimitReached,
    #[error("local memory {0} was not found")]
    NotFound(Uuid),
    #[error(
        "local memory {id} changed since it was opened (expected revision {expected}, current revision {actual})"
    )]
    Conflict {
        id: Uuid,
        expected: i64,
        actual: i64,
    },
    #[error("local memory storage error: {0}")]
    Storage(#[source] anyhow::Error),
    #[error("invalid local memory row: {0}")]
    Corrupt(String),
}

#[derive(Clone)]
pub struct LocalMemoryRepository {
    inner: Rc<RefCell<RepositoryInner>>,
}

enum RepositoryInner {
    Sqlite(SqliteConnection),
    Unavailable(String),
}

#[derive(QueryableByName)]
struct MemoryDbRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Text)]
    scope_kind: String,
    #[diesel(sql_type = Text)]
    scope_key: String,
    #[diesel(sql_type = Text)]
    title: String,
    #[diesel(sql_type = Text)]
    content: String,
    #[diesel(sql_type = BigInt)]
    revision: i64,
    #[diesel(sql_type = BigInt)]
    created_at: i64,
    #[diesel(sql_type = BigInt)]
    updated_at: i64,
}

#[derive(QueryableByName)]
struct VersionDbRow {
    #[diesel(sql_type = Text)]
    memory_id: String,
    #[diesel(sql_type = BigInt)]
    revision: i64,
    #[diesel(sql_type = Text)]
    operation: String,
    #[diesel(sql_type = Text)]
    scope_kind: String,
    #[diesel(sql_type = Text)]
    scope_key: String,
    #[diesel(sql_type = Text)]
    title: String,
    #[diesel(sql_type = Text)]
    content: String,
    #[diesel(sql_type = BigInt)]
    recorded_at: i64,
}

impl LocalMemoryRepository {
    pub fn open_current_scope() -> Result<Self, LocalMemoryError> {
        Self::open(crate::persistence::database_file_path_for_current_scope())
    }

    pub fn in_memory() -> Result<Self, LocalMemoryError> {
        let connection = SqliteConnection::establish(":memory:")
            .map_err(|error| storage_error(format!("opening in-memory database: {error}")))?;
        Self::from_connection(connection)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, LocalMemoryError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|error| {
                storage_error(format!("creating {}: {error}", parent.display()))
            })?;
        }
        let path_text = path.to_str().ok_or_else(|| {
            storage_error(format!(
                "database path is not valid UTF-8: {}",
                path.display()
            ))
        })?;
        let connection = SqliteConnection::establish(path_text)
            .map_err(|error| storage_error(format!("opening {}: {error}", path.display())))?;
        Self::from_connection(connection)
    }

    fn from_connection(mut connection: SqliteConnection) -> Result<Self, LocalMemoryError> {
        connection
            .batch_execute("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")
            .and_then(|_| connection.batch_execute(LOCAL_MEMORY_SCHEMA))
            .map_err(|error| storage_error(format!("initializing schema: {error}")))?;
        Ok(Self {
            inner: Rc::new(RefCell::new(RepositoryInner::Sqlite(connection))),
        })
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(RepositoryInner::Unavailable(message.into()))),
        }
    }

    pub fn startup_error(&self) -> Option<String> {
        match &*self.inner.borrow() {
            RepositoryInner::Sqlite(_) => None,
            RepositoryInner::Unavailable(message) => Some(message.clone()),
        }
    }

    pub fn create(
        &self,
        scope: LocalMemoryScope,
        title: &str,
        content: &str,
    ) -> Result<LocalMemoryRecord, LocalMemoryError> {
        let scope = normalize_scope(scope)?;
        let (title, content) = validate_text(title, content)?;
        self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    #[derive(QueryableByName)]
                    struct CountRow {
                        #[diesel(sql_type = BigInt)]
                        count: i64,
                    }
                    let count = sql_query("SELECT COUNT(*) AS count FROM local_memories")
                        .get_result::<CountRow>(connection)?
                        .count;
                    if count >= MAX_MEMORY_COUNT {
                        return Err(anyhow::Error::new(LocalMemoryError::LimitReached));
                    }
                    let now = now_millis();
                    let record = LocalMemoryRecord {
                        id: Uuid::new_v4(),
                        scope,
                        title,
                        content,
                        revision: 1,
                        created_at: now,
                        updated_at: now,
                    };
                    insert_current(connection, &record)?;
                    insert_version(connection, &record, LocalMemoryOperation::Created, now)?;
                    Ok(record)
                })
                .map_err(map_transaction_error)
        })
    }

    pub fn update(
        &self,
        id: Uuid,
        expected_revision: i64,
        scope: LocalMemoryScope,
        title: &str,
        content: &str,
    ) -> Result<LocalMemoryRecord, LocalMemoryError> {
        let scope = normalize_scope(scope)?;
        let (title, content) = validate_text(title, content)?;
        self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    let current = load_one(connection, id)?
                        .ok_or_else(|| anyhow::Error::new(LocalMemoryError::NotFound(id)))?;
                    if current.revision != expected_revision {
                        return Err(anyhow::Error::new(LocalMemoryError::Conflict {
                            id,
                            expected: expected_revision,
                            actual: current.revision,
                        }));
                    }
                    let now = now_millis();
                    let updated = LocalMemoryRecord {
                        id,
                        scope,
                        title,
                        content,
                        revision: expected_revision
                            .checked_add(1)
                            .ok_or_else(|| anyhow::anyhow!("local memory revision overflow"))?,
                        created_at: current.created_at,
                        updated_at: now,
                    };
                    let (scope_kind, scope_key) = scope_parts(&updated.scope);
                    let changed = sql_query(
                        "UPDATE local_memories SET scope_kind = ?, scope_key = ?, title = ?, \
                         content = ?, revision = ?, updated_at = ? WHERE id = ? AND revision = ?",
                    )
                    .bind::<Text, _>(scope_kind)
                    .bind::<Text, _>(scope_key)
                    .bind::<Text, _>(&updated.title)
                    .bind::<Text, _>(&updated.content)
                    .bind::<BigInt, _>(updated.revision)
                    .bind::<BigInt, _>(updated.updated_at)
                    .bind::<Text, _>(updated.id.to_string())
                    .bind::<BigInt, _>(expected_revision)
                    .execute(connection)?;
                    if changed != 1 {
                        return Err(anyhow::anyhow!("local memory compare-and-swap failed"));
                    }
                    insert_version(connection, &updated, LocalMemoryOperation::Updated, now)?;
                    Ok(updated)
                })
                .map_err(map_transaction_error)
        })
    }

    pub fn delete(&self, id: Uuid, expected_revision: i64) -> Result<(), LocalMemoryError> {
        self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    let current = load_one(connection, id)?
                        .ok_or_else(|| anyhow::Error::new(LocalMemoryError::NotFound(id)))?;
                    if current.revision != expected_revision {
                        return Err(anyhow::Error::new(LocalMemoryError::Conflict {
                            id,
                            expected: expected_revision,
                            actual: current.revision,
                        }));
                    }
                    let changed =
                        sql_query("DELETE FROM local_memories WHERE id = ? AND revision = ?")
                            .bind::<Text, _>(id.to_string())
                            .bind::<BigInt, _>(expected_revision)
                            .execute(connection)?;
                    if changed != 1 {
                        return Err(anyhow::anyhow!("local memory compare-and-swap failed"));
                    }
                    let mut tombstone = current;
                    tombstone.revision = expected_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("local memory revision overflow"))?;
                    tombstone.updated_at = now_millis();
                    insert_version(
                        connection,
                        &tombstone,
                        LocalMemoryOperation::Deleted,
                        tombstone.updated_at,
                    )?;
                    Ok(())
                })
                .map_err(map_transaction_error)
        })
    }

    pub fn get(&self, id: Uuid) -> Result<Option<LocalMemoryRecord>, LocalMemoryError> {
        self.with_connection(|connection| load_one(connection, id).map_err(map_transaction_error))
    }

    pub fn list(&self) -> Result<Vec<LocalMemoryRecord>, LocalMemoryError> {
        self.with_connection(|connection| {
            sql_query(
                "SELECT id, scope_kind, scope_key, title, content, revision, created_at, updated_at \
                 FROM local_memories ORDER BY updated_at DESC, id ASC",
            )
            .load::<MemoryDbRow>(connection)
            .map_err(|error| storage_error(format!("listing memories: {error}")))?
            .into_iter()
            .map(memory_from_row)
            .collect()
        })
    }

    pub fn history(&self, id: Uuid) -> Result<Vec<LocalMemoryVersion>, LocalMemoryError> {
        self.with_connection(|connection| {
            sql_query(
                "SELECT memory_id, revision, operation, scope_kind, scope_key, title, content, \
                 recorded_at FROM local_memory_versions WHERE memory_id = ? ORDER BY revision ASC",
            )
            .bind::<Text, _>(id.to_string())
            .load::<VersionDbRow>(connection)
            .map_err(|error| storage_error(format!("loading memory history: {error}")))?
            .into_iter()
            .map(version_from_row)
            .collect()
        })
    }

    /// Deterministic provider-free recall. Only global memories and project memories whose root
    /// contains the current local working directory are eligible.
    pub fn search(
        &self,
        query: &str,
        current_directory: Option<&Path>,
    ) -> Result<Vec<LocalMemoryRecord>, LocalMemoryError> {
        let query_tokens = lexical_tokens(query)
            .into_iter()
            .take(32)
            .collect::<HashSet<_>>();
        if query_tokens.is_empty() {
            return Ok(Vec::new());
        }
        let current_directory = current_directory.and_then(|path| fs::canonicalize(path).ok());
        let normalized_query = normalize_for_search(query);
        let mut scored = self
            .list()?
            .into_iter()
            .filter(|record| scope_applies(&record.scope, current_directory.as_deref()))
            .filter_map(|record| {
                let title = normalize_for_search(&record.title);
                let content = normalize_for_search(&record.content);
                let title_tokens = lexical_tokens(&title);
                let content_tokens = lexical_tokens(&content);
                let mut score = 0_u32;
                for token in &query_tokens {
                    if title_tokens.contains(token) {
                        score += 8;
                    } else if title.contains(token) {
                        score += 3;
                    }
                    if content_tokens.contains(token) {
                        score += 4;
                    } else if content.contains(token) {
                        score += 1;
                    }
                }
                if !normalized_query.is_empty() {
                    if title.contains(&normalized_query) {
                        score += 16;
                    }
                    if content.contains(&normalized_query) {
                        score += 6;
                    }
                }
                (score > 0).then_some((score, record))
            })
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.updated_at.cmp(&left.1.updated_at))
                .then_with(|| left.1.id.cmp(&right.1.id))
        });
        Ok(scored
            .into_iter()
            .take(MAX_CONTEXT_MEMORIES)
            .map(|(_, record)| record)
            .collect())
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut SqliteConnection) -> Result<T, LocalMemoryError>,
    ) -> Result<T, LocalMemoryError> {
        let mut inner = self
            .inner
            .try_borrow_mut()
            .map_err(|_| storage_error("repository is already in use"))?;
        match &mut *inner {
            RepositoryInner::Sqlite(connection) => operation(connection),
            RepositoryInner::Unavailable(message) => Err(storage_error(message.clone())),
        }
    }
}

fn insert_current(
    connection: &mut SqliteConnection,
    record: &LocalMemoryRecord,
) -> Result<(), anyhow::Error> {
    let (scope_kind, scope_key) = scope_parts(&record.scope);
    sql_query(
        "INSERT INTO local_memories \
         (id, scope_kind, scope_key, title, content, revision, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind::<Text, _>(record.id.to_string())
    .bind::<Text, _>(scope_kind)
    .bind::<Text, _>(scope_key)
    .bind::<Text, _>(&record.title)
    .bind::<Text, _>(&record.content)
    .bind::<BigInt, _>(record.revision)
    .bind::<BigInt, _>(record.created_at)
    .bind::<BigInt, _>(record.updated_at)
    .execute(connection)?;
    Ok(())
}

fn insert_version(
    connection: &mut SqliteConnection,
    record: &LocalMemoryRecord,
    operation: LocalMemoryOperation,
    recorded_at: i64,
) -> Result<(), anyhow::Error> {
    let (scope_kind, scope_key) = scope_parts(&record.scope);
    sql_query(
        "INSERT INTO local_memory_versions \
         (memory_id, revision, operation, scope_kind, scope_key, title, content, recorded_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind::<Text, _>(record.id.to_string())
    .bind::<BigInt, _>(record.revision)
    .bind::<Text, _>(operation_to_str(operation))
    .bind::<Text, _>(scope_kind)
    .bind::<Text, _>(scope_key)
    .bind::<Text, _>(&record.title)
    .bind::<Text, _>(&record.content)
    .bind::<BigInt, _>(recorded_at)
    .execute(connection)?;
    Ok(())
}

fn load_one(
    connection: &mut SqliteConnection,
    id: Uuid,
) -> Result<Option<LocalMemoryRecord>, anyhow::Error> {
    let row = sql_query(
        "SELECT id, scope_kind, scope_key, title, content, revision, created_at, updated_at \
         FROM local_memories WHERE id = ?",
    )
    .bind::<Text, _>(id.to_string())
    .load::<MemoryDbRow>(connection)?
    .into_iter()
    .next();
    row.map(memory_from_row)
        .transpose()
        .map_err(anyhow::Error::new)
}

fn memory_from_row(row: MemoryDbRow) -> Result<LocalMemoryRecord, LocalMemoryError> {
    let id = Uuid::parse_str(&row.id)
        .map_err(|error| LocalMemoryError::Corrupt(format!("invalid id: {error}")))?;
    if row.revision < 1 {
        return Err(LocalMemoryError::Corrupt(
            "revision must be positive".into(),
        ));
    }
    Ok(LocalMemoryRecord {
        id,
        scope: scope_from_parts(&row.scope_kind, &row.scope_key)?,
        title: row.title,
        content: row.content,
        revision: row.revision,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn version_from_row(row: VersionDbRow) -> Result<LocalMemoryVersion, LocalMemoryError> {
    let memory_id = Uuid::parse_str(&row.memory_id)
        .map_err(|error| LocalMemoryError::Corrupt(format!("invalid history id: {error}")))?;
    let operation = match row.operation.as_str() {
        "created" => LocalMemoryOperation::Created,
        "updated" => LocalMemoryOperation::Updated,
        "deleted" => LocalMemoryOperation::Deleted,
        value => {
            return Err(LocalMemoryError::Corrupt(format!(
                "unknown operation {value}"
            )));
        }
    };
    Ok(LocalMemoryVersion {
        memory_id,
        revision: row.revision,
        operation,
        scope: scope_from_parts(&row.scope_kind, &row.scope_key)?,
        title: row.title,
        content: row.content,
        recorded_at: row.recorded_at,
    })
}

fn normalize_scope(scope: LocalMemoryScope) -> Result<LocalMemoryScope, LocalMemoryError> {
    match scope {
        LocalMemoryScope::Global => Ok(LocalMemoryScope::Global),
        LocalMemoryScope::Project { root } => fs::canonicalize(&root)
            .map(|root| LocalMemoryScope::Project { root })
            .map_err(|_| LocalMemoryError::InvalidProjectRoot(root)),
    }
}

fn scope_parts(scope: &LocalMemoryScope) -> (&'static str, String) {
    match scope {
        LocalMemoryScope::Global => ("global", String::new()),
        LocalMemoryScope::Project { root } => ("project", root.to_string_lossy().into_owned()),
    }
}

fn scope_from_parts(kind: &str, key: &str) -> Result<LocalMemoryScope, LocalMemoryError> {
    match (kind, key) {
        ("global", "") => Ok(LocalMemoryScope::Global),
        ("project", key) if !key.is_empty() => Ok(LocalMemoryScope::Project {
            root: PathBuf::from(key),
        }),
        _ => Err(LocalMemoryError::Corrupt(format!(
            "invalid scope {kind}:{key}"
        ))),
    }
}

fn scope_applies(scope: &LocalMemoryScope, current_directory: Option<&Path>) -> bool {
    match scope {
        LocalMemoryScope::Global => true,
        LocalMemoryScope::Project { root } => {
            current_directory.is_some_and(|cwd| cwd.starts_with(root))
        }
    }
}

fn validate_text(title: &str, content: &str) -> Result<(String, String), LocalMemoryError> {
    let title = title.trim();
    let content = content.trim();
    if title.is_empty() {
        return Err(LocalMemoryError::EmptyTitle);
    }
    if content.is_empty() {
        return Err(LocalMemoryError::EmptyContent);
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(LocalMemoryError::TitleTooLong);
    }
    if content.chars().count() > MAX_CONTENT_CHARS {
        return Err(LocalMemoryError::ContentTooLong);
    }
    Ok((title.to_string(), content.to_string()))
}

fn lexical_tokens(value: &str) -> HashSet<String> {
    normalize_for_search(value)
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

fn normalize_for_search(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn operation_to_str(operation: LocalMemoryOperation) -> &'static str {
    match operation {
        LocalMemoryOperation::Created => "created",
        LocalMemoryOperation::Updated => "updated",
        LocalMemoryOperation::Deleted => "deleted",
    }
}

fn map_transaction_error(error: anyhow::Error) -> LocalMemoryError {
    match error.downcast::<LocalMemoryError>() {
        Ok(error) => error,
        Err(error) => LocalMemoryError::Storage(error),
    }
}

fn storage_error(message: impl Into<String>) -> LocalMemoryError {
    LocalMemoryError::Storage(anyhow::anyhow!(message.into()))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

pub(crate) fn truncate_context_content(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut truncated = content
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
#[path = "local_memory_tests.rs"]
mod tests;
