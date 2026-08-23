//! Local, synchronous persistence for the queued-prompt surface.
//!
//! Queue writes intentionally use a small repository boundary rather than the general event
//! writer. Queue UI state is observable immediately after a mutation, so the row and its
//! per-conversation settings must be committed before the model emits an event.

use std::{cell::RefCell, fs, path::Path, rc::Rc};

use anyhow::{Context, Result, anyhow};
use diesel::sqlite::SqliteConnection;
use diesel::{
    Connection, QueryableByName, RunQueryDsl, connection::SimpleConnection, sql_query, sql_types,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ai::agent::conversation::AIConversationId;

const QUEUE_TABLE: &str = "local_prompt_queue_rows";
const SETTINGS_TABLE: &str = "local_prompt_queue_settings";
const QUARANTINE_TABLE: &str = "local_prompt_queue_quarantine";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalPromptQueueKind {
    Prompt,
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalPromptQueueAttachment {
    Image {
        data: String,
        file_name: String,
        mime_type: String,
    },
    File {
        path: String,
        file_name: String,
        mime_type: String,
    },
    /// File metadata captured when the row is queued. Keeping this as a separate variant
    /// preserves compatibility with older rows that only recorded path/name/MIME while allowing
    /// restart-time dispatch to reject a file that changed in place.
    FileWithFingerprint {
        path: String,
        file_name: String,
        mime_type: String,
        size: u64,
        modified_at: i64,
    },
}

#[derive(Debug, Clone)]
pub struct LocalPromptQueueRow {
    pub id: Uuid,
    pub conversation_id: AIConversationId,
    pub position: i64,
    pub kind: LocalPromptQueueKind,
    pub text: String,
    pub origin: String,
    pub attachments: Vec<LocalPromptQueueAttachment>,
    pub locked: bool,
    pub attempt_count: u32,
    pub created_at: i64,
    pub updated_at: i64,
    pub dispatched_at: Option<i64>,
    pub local_error: Option<String>,
    /// Whether this row is eligible for a terminal-state auto-fire after loading.
    pub auto_fireable: bool,
}

impl LocalPromptQueueRow {
    pub fn prompt(
        id: Uuid,
        conversation_id: AIConversationId,
        position: i64,
        text: impl Into<String>,
        origin: impl Into<String>,
        attachments: Vec<LocalPromptQueueAttachment>,
    ) -> Self {
        Self::new(
            id,
            conversation_id,
            position,
            LocalPromptQueueKind::Prompt,
            text,
            origin,
            attachments,
        )
    }

    pub fn command(
        id: Uuid,
        conversation_id: AIConversationId,
        position: i64,
        text: impl Into<String>,
        origin: impl Into<String>,
    ) -> Self {
        Self::new(
            id,
            conversation_id,
            position,
            LocalPromptQueueKind::Command,
            text,
            origin,
            Vec::new(),
        )
    }

    fn new(
        id: Uuid,
        conversation_id: AIConversationId,
        position: i64,
        kind: LocalPromptQueueKind,
        text: impl Into<String>,
        origin: impl Into<String>,
        attachments: Vec<LocalPromptQueueAttachment>,
    ) -> Self {
        let now = now_millis();
        Self {
            id,
            conversation_id,
            position,
            kind,
            text: text.into(),
            origin: origin.into(),
            attachments,
            locked: false,
            attempt_count: 0,
            created_at: now,
            updated_at: now,
            dispatched_at: None,
            local_error: None,
            auto_fireable: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalPromptQueueSettings {
    pub queue_next_prompt_enabled: bool,
    pub command_in_flight: bool,
}

#[derive(Debug, Clone)]
pub struct LocalPromptQueueSnapshot {
    pub rows: Vec<LocalPromptQueueRow>,
    pub settings: LocalPromptQueueSettings,
}

#[derive(Debug, QueryableByName)]
struct QueueDbRow {
    #[diesel(sql_type = sql_types::Text)]
    id: String,
    #[diesel(sql_type = sql_types::Text)]
    conversation_id: String,
    #[diesel(sql_type = sql_types::BigInt)]
    position: i64,
    #[diesel(sql_type = sql_types::Text)]
    kind: String,
    #[diesel(sql_type = sql_types::Text)]
    text: String,
    #[diesel(sql_type = sql_types::Text)]
    origin: String,
    #[diesel(sql_type = sql_types::Text)]
    attachments_json: String,
    #[diesel(sql_type = sql_types::Integer)]
    locked: i32,
    #[diesel(sql_type = sql_types::Integer)]
    attempt_count: i32,
    #[diesel(sql_type = sql_types::BigInt)]
    created_at: i64,
    #[diesel(sql_type = sql_types::BigInt)]
    updated_at: i64,
    #[diesel(sql_type = sql_types::Nullable<sql_types::BigInt>)]
    dispatched_at: Option<i64>,
    #[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
    local_error: Option<String>,
}

#[derive(Debug, QueryableByName)]
struct SettingsDbRow {
    #[diesel(sql_type = sql_types::Integer)]
    queue_next_prompt_enabled: i32,
    #[diesel(sql_type = sql_types::Integer)]
    command_in_flight: i32,
}

#[derive(Clone)]
pub struct LocalPromptQueueRepository {
    inner: Rc<RefCell<RepositoryInner>>,
}

enum RepositoryInner {
    Sqlite(SqliteConnection),
    Unavailable(String),
}

impl LocalPromptQueueRepository {
    pub fn in_memory() -> Result<Self> {
        let mut connection = SqliteConnection::establish(":memory:")
            .context("opening in-memory local prompt queue database")?;
        create_tables(&mut connection)?;
        Ok(Self {
            inner: Rc::new(RefCell::new(RepositoryInner::Sqlite(connection))),
        })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).with_context(|| {
                format!("creating queue database directory {}", parent.display())
            })?;
        }
        let mut connection = SqliteConnection::establish(path.to_string_lossy().as_ref())
            .with_context(|| format!("opening local prompt queue database {}", path.display()))?;
        create_tables(&mut connection)?;
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
        let inner = self.inner.borrow();
        match &*inner {
            RepositoryInner::Unavailable(message) => Some(message.clone()),
            RepositoryInner::Sqlite(_) => None,
        }
    }

    /// A deterministic failure repository used by model tests to prove persistence-before-state.
    #[cfg(test)]
    pub fn failing_for_test() -> Self {
        Self {
            inner: Rc::new(RefCell::new(RepositoryInner::Unavailable(
                "injected local prompt queue write failure".to_owned(),
            ))),
        }
    }

    pub fn load_conversation(
        &self,
        conversation_id: AIConversationId,
    ) -> Result<LocalPromptQueueSnapshot> {
        self.with_connection(|connection| load_conversation(connection, conversation_id))
    }

    pub fn load_all(&self) -> Result<Vec<(AIConversationId, LocalPromptQueueSnapshot)>> {
        self.with_connection(|connection| {
            #[derive(QueryableByName)]
            struct ConversationRow {
                #[diesel(sql_type = sql_types::Text)]
                conversation_id: String,
            }
            let conversation_ids = sql_query(format!(
                "SELECT DISTINCT conversation_id FROM {QUEUE_TABLE}
                 UNION SELECT conversation_id FROM {SETTINGS_TABLE}
                 ORDER BY conversation_id"
            ))
            .load::<ConversationRow>(connection)
            .context("loading local prompt queue conversations")?;
            let mut snapshots = Vec::with_capacity(conversation_ids.len());
            for conversation_row in conversation_ids {
                let conversation_id =
                    match AIConversationId::try_from(conversation_row.conversation_id.clone()) {
                        Ok(conversation_id) => conversation_id,
                        Err(error) => {
                            quarantine_invalid_conversation_rows(
                                connection,
                                &conversation_row.conversation_id,
                                &format!("invalid conversation id: {error}"),
                            )?;
                            continue;
                        }
                    };
                snapshots.push((
                    conversation_id,
                    load_conversation(connection, conversation_id)?,
                ));
            }
            Ok(snapshots)
        })
    }

    pub fn replace_conversation(
        &self,
        conversation_id: AIConversationId,
        rows: &[LocalPromptQueueRow],
        queue_next_prompt_enabled: bool,
    ) -> Result<()> {
        self.replace_conversation_with_settings(
            conversation_id,
            rows,
            LocalPromptQueueSettings {
                queue_next_prompt_enabled,
                command_in_flight: false,
            },
        )
    }

    pub fn replace_conversation_with_settings(
        &self,
        conversation_id: AIConversationId,
        rows: &[LocalPromptQueueRow],
        settings: LocalPromptQueueSettings,
    ) -> Result<()> {
        self.with_connection(|connection| {
            connection.transaction::<_, anyhow::Error, _>(|connection| {
                diesel::sql_query(format!(
                    "DELETE FROM {QUEUE_TABLE} WHERE conversation_id = ?"
                ))
                .bind::<sql_types::Text, _>(conversation_id.to_string())
                .execute(connection)
                .context("deleting previous local prompt queue rows")?;

                for row in rows {
                    if row.conversation_id != conversation_id {
                        return Err(anyhow!("queue row belongs to another conversation"));
                    }
                    insert_row(connection, row)?;
                }

                diesel::sql_query(format!(
                    "INSERT INTO {SETTINGS_TABLE}
                     (conversation_id, queue_next_prompt_enabled, command_in_flight, updated_at)
                     VALUES (?, ?, ?, ?)
                     ON CONFLICT(conversation_id) DO UPDATE SET
                       queue_next_prompt_enabled = excluded.queue_next_prompt_enabled,
                       command_in_flight = excluded.command_in_flight,
                       updated_at = excluded.updated_at"
                ))
                .bind::<sql_types::Text, _>(conversation_id.to_string())
                .bind::<sql_types::Integer, _>(settings.queue_next_prompt_enabled as i32)
                .bind::<sql_types::Integer, _>(settings.command_in_flight as i32)
                .bind::<sql_types::BigInt, _>(now_millis())
                .execute(connection)
                .context("saving local prompt queue settings")?;
                Ok(())
            })
        })
    }

    pub fn mark_dispatched(&self, conversation_id: AIConversationId, row_id: Uuid) -> Result<()> {
        self.with_connection(|connection| {
            let now = now_millis();
            let changed = diesel::sql_query(format!(
                "UPDATE {QUEUE_TABLE}
                    SET attempt_count = attempt_count + 1,
                        dispatched_at = ?,
                        updated_at = ?,
                        local_error = NULL
                  WHERE conversation_id = ? AND id = ? AND dispatched_at IS NULL"
            ))
            .bind::<sql_types::BigInt, _>(now)
            .bind::<sql_types::BigInt, _>(now)
            .bind::<sql_types::Text, _>(conversation_id.to_string())
            .bind::<sql_types::Text, _>(row_id.to_string())
            .execute(connection)
            .context("marking local prompt queue row dispatched")?;
            if changed == 0 {
                return Err(anyhow!("queue row is missing or already dispatched"));
            }
            Ok(())
        })
    }

    /// Marks a row dispatched and, for command rows, gates the next queue row in one SQLite
    /// transaction. The caller must perform the external side effect only after this returns.
    pub fn dispatch_row(
        &self,
        conversation_id: AIConversationId,
        row_id: Uuid,
        command_in_flight: bool,
    ) -> Result<()> {
        self.with_connection(|connection| {
            connection.transaction::<_, anyhow::Error, _>(|connection| {
                let now = now_millis();
                let changed = diesel::sql_query(format!(
                    "UPDATE {QUEUE_TABLE}
                        SET attempt_count = attempt_count + 1,
                            dispatched_at = ?,
                            updated_at = ?,
                            local_error = NULL
                      WHERE conversation_id = ? AND id = ? AND dispatched_at IS NULL"
                ))
                .bind::<sql_types::BigInt, _>(now)
                .bind::<sql_types::BigInt, _>(now)
                .bind::<sql_types::Text, _>(conversation_id.to_string())
                .bind::<sql_types::Text, _>(row_id.to_string())
                .execute(connection)
                .context("marking local prompt queue row dispatched")?;
                if changed == 0 {
                    return Err(anyhow!("queue row is missing or already dispatched"));
                }
                if command_in_flight {
                    diesel::sql_query(format!(
                        "INSERT INTO {SETTINGS_TABLE}
                         (conversation_id, queue_next_prompt_enabled, command_in_flight, updated_at)
                         VALUES (?, 0, 1, ?)
                         ON CONFLICT(conversation_id) DO UPDATE SET
                           command_in_flight = 1,
                           updated_at = excluded.updated_at"
                    ))
                    .bind::<sql_types::Text, _>(conversation_id.to_string())
                    .bind::<sql_types::BigInt, _>(now)
                    .execute(connection)
                    .context("saving local prompt command-in-flight state")?;
                }
                Ok(())
            })
        })
    }

    pub fn complete_row(
        &self,
        conversation_id: AIConversationId,
        row_id: Uuid,
        clear_command_in_flight: bool,
    ) -> Result<()> {
        self.with_connection(|connection| {
            connection.transaction::<_, anyhow::Error, _>(|connection| {
                diesel::sql_query(format!(
                    "DELETE FROM {QUEUE_TABLE} WHERE conversation_id = ? AND id = ?"
                ))
                .bind::<sql_types::Text, _>(conversation_id.to_string())
                .bind::<sql_types::Text, _>(row_id.to_string())
                .execute(connection)
                .context("deleting completed local prompt queue row")?;
                if clear_command_in_flight {
                    diesel::sql_query(format!(
                        "UPDATE {SETTINGS_TABLE} SET command_in_flight = 0, updated_at = ?
                         WHERE conversation_id = ?"
                    ))
                    .bind::<sql_types::BigInt, _>(now_millis())
                    .bind::<sql_types::Text, _>(conversation_id.to_string())
                    .execute(connection)
                    .context("clearing local prompt command-in-flight state")?;
                }
                Ok(())
            })
        })
    }

    pub fn clear_dispatched(&self, conversation_id: AIConversationId, row_id: Uuid) -> Result<()> {
        self.with_connection(|connection| {
            diesel::sql_query(format!(
                "UPDATE {QUEUE_TABLE} SET dispatched_at = NULL, updated_at = ?
                  WHERE conversation_id = ? AND id = ?"
            ))
            .bind::<sql_types::BigInt, _>(now_millis())
            .bind::<sql_types::Text, _>(conversation_id.to_string())
            .bind::<sql_types::Text, _>(row_id.to_string())
            .execute(connection)
            .context("clearing local prompt queue dispatch marker")?;
            Ok(())
        })
    }

    pub fn set_local_error(
        &self,
        conversation_id: AIConversationId,
        row_id: Uuid,
        message: Option<&str>,
    ) -> Result<()> {
        self.with_connection(|connection| {
            diesel::sql_query(format!(
                "UPDATE {QUEUE_TABLE} SET local_error = ?, updated_at = ?
                  WHERE conversation_id = ? AND id = ?"
            ))
            .bind::<sql_types::Nullable<sql_types::Text>, _>(message)
            .bind::<sql_types::BigInt, _>(now_millis())
            .bind::<sql_types::Text, _>(conversation_id.to_string())
            .bind::<sql_types::Text, _>(row_id.to_string())
            .execute(connection)
            .context("saving local prompt queue error")?;
            Ok(())
        })
    }

    /// Records a local dispatch error and clears the command gate in one transaction. The
    /// dispatched marker intentionally remains, so recovery is always an explicit retry.
    pub fn set_local_error_with_command_state(
        &self,
        conversation_id: AIConversationId,
        row_id: Uuid,
        message: &str,
        clear_command_in_flight: bool,
    ) -> Result<()> {
        self.with_connection(|connection| {
            connection.transaction::<_, anyhow::Error, _>(|connection| {
                diesel::sql_query(format!(
                    "UPDATE {QUEUE_TABLE} SET local_error = ?, updated_at = ?
                      WHERE conversation_id = ? AND id = ?"
                ))
                .bind::<sql_types::Nullable<sql_types::Text>, _>(Some(message))
                .bind::<sql_types::BigInt, _>(now_millis())
                .bind::<sql_types::Text, _>(conversation_id.to_string())
                .bind::<sql_types::Text, _>(row_id.to_string())
                .execute(connection)
                .context("saving local prompt queue dispatch error")?;
                if clear_command_in_flight {
                    diesel::sql_query(format!(
                        "UPDATE {SETTINGS_TABLE} SET command_in_flight = 0, updated_at = ?
                         WHERE conversation_id = ?"
                    ))
                    .bind::<sql_types::BigInt, _>(now_millis())
                    .bind::<sql_types::Text, _>(conversation_id.to_string())
                    .execute(connection)
                    .context("clearing local prompt command-in-flight state")?;
                }
                Ok(())
            })
        })
    }

    /// Clears the durable dispatch/error marker only after an explicit user retry. This never
    /// runs during startup, so an uncertain side effect is not silently repeated.
    pub fn retry_row(&self, conversation_id: AIConversationId, row_id: Uuid) -> Result<()> {
        self.with_connection(|connection| {
            connection.transaction::<_, anyhow::Error, _>(|connection| {
                let changed = diesel::sql_query(format!(
                    "UPDATE {QUEUE_TABLE}
                        SET dispatched_at = NULL, local_error = NULL, updated_at = ?
                      WHERE conversation_id = ? AND id = ?"
                ))
                .bind::<sql_types::BigInt, _>(now_millis())
                .bind::<sql_types::Text, _>(conversation_id.to_string())
                .bind::<sql_types::Text, _>(row_id.to_string())
                .execute(connection)
                .context("resetting local prompt queue row for explicit retry")?;
                if changed == 0 {
                    return Err(anyhow!("queue row is missing"));
                }
                diesel::sql_query(format!(
                    "UPDATE {SETTINGS_TABLE} SET command_in_flight = 0, updated_at = ?
                     WHERE conversation_id = ?"
                ))
                .bind::<sql_types::BigInt, _>(now_millis())
                .bind::<sql_types::Text, _>(conversation_id.to_string())
                .execute(connection)
                .context("clearing local prompt retry command gate")?;
                Ok(())
            })
        })
    }

    pub fn delete_conversation(&self, conversation_id: AIConversationId) -> Result<()> {
        self.with_connection(|connection| {
            connection.transaction::<_, anyhow::Error, _>(|connection| {
                diesel::sql_query(format!(
                    "DELETE FROM {QUEUE_TABLE} WHERE conversation_id = ?"
                ))
                .bind::<sql_types::Text, _>(conversation_id.to_string())
                .execute(connection)
                .context("deleting local prompt queue rows")?;
                diesel::sql_query(format!(
                    "DELETE FROM {SETTINGS_TABLE} WHERE conversation_id = ?"
                ))
                .bind::<sql_types::Text, _>(conversation_id.to_string())
                .execute(connection)
                .context("deleting local prompt queue settings")?;
                diesel::sql_query(format!(
                    "DELETE FROM {QUARANTINE_TABLE} WHERE conversation_id = ?"
                ))
                .bind::<sql_types::Text, _>(conversation_id.to_string())
                .execute(connection)
                .context("deleting local prompt queue quarantine diagnostics")?;
                Ok(())
            })
        })
    }

    pub fn set_command_in_flight(
        &self,
        conversation_id: AIConversationId,
        in_flight: bool,
    ) -> Result<()> {
        self.with_connection(|connection| {
            diesel::sql_query(format!(
                "INSERT INTO {SETTINGS_TABLE}
                 (conversation_id, queue_next_prompt_enabled, command_in_flight, updated_at)
                 VALUES (?, 0, ?, ?)
                 ON CONFLICT(conversation_id) DO UPDATE SET
                   command_in_flight = excluded.command_in_flight,
                   updated_at = excluded.updated_at"
            ))
            .bind::<sql_types::Text, _>(conversation_id.to_string())
            .bind::<sql_types::Integer, _>(in_flight as i32)
            .bind::<sql_types::BigInt, _>(now_millis())
            .execute(connection)
            .context("saving local prompt command-in-flight state")?;
            Ok(())
        })
    }

    pub fn quarantined_count(&self) -> Result<i64> {
        self.with_connection(|connection| {
            #[derive(QueryableByName)]
            struct Count {
                #[diesel(sql_type = sql_types::BigInt)]
                count: i64,
            }
            let count = sql_query(format!("SELECT COUNT(*) AS count FROM {QUARANTINE_TABLE}"))
                .load::<Count>(connection)
                .context("counting quarantined local prompt queue rows")?
                .into_iter()
                .next()
                .map(|row| row.count)
                .unwrap_or_default();
            Ok(count)
        })
    }

    #[cfg(test)]
    pub fn insert_raw_for_test(
        &self,
        id: Uuid,
        conversation_id: AIConversationId,
        position: i64,
        kind: &str,
        text: &str,
        origin: &str,
        attachments_json: &str,
    ) -> Result<()> {
        self.with_connection(|connection| {
            diesel::sql_query(format!(
                "INSERT INTO {QUEUE_TABLE}
                 (id, conversation_id, position, kind, text, origin, attachments_json,
                  locked, attempt_count, created_at, updated_at, dispatched_at, local_error)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 0, 0, ?, ?, NULL, NULL)"
            ))
            .bind::<sql_types::Text, _>(id.to_string())
            .bind::<sql_types::Text, _>(conversation_id.to_string())
            .bind::<sql_types::BigInt, _>(position)
            .bind::<sql_types::Text, _>(kind)
            .bind::<sql_types::Text, _>(text)
            .bind::<sql_types::Text, _>(origin)
            .bind::<sql_types::Text, _>(attachments_json)
            .bind::<sql_types::BigInt, _>(now_millis())
            .bind::<sql_types::BigInt, _>(now_millis())
            .execute(connection)
            .context("inserting raw local prompt queue row")?;
            Ok(())
        })
    }

    #[cfg(test)]
    pub fn insert_corrupt_raw_for_test(
        &self,
        conversation_id: AIConversationId,
        position: i64,
        kind: &str,
        text: &str,
    ) -> Result<()> {
        self.insert_raw_for_test(
            Uuid::new_v4(),
            conversation_id,
            position,
            kind,
            text,
            "bad-origin",
            "not-json",
        )
    }

    fn with_connection<T>(&self, f: impl FnOnce(&mut SqliteConnection) -> Result<T>) -> Result<T> {
        let mut inner = self
            .inner
            .try_borrow_mut()
            .map_err(|_| anyhow!("queue repository is already in use"))?;
        match &mut *inner {
            RepositoryInner::Sqlite(connection) => f(connection),
            RepositoryInner::Unavailable(message) => Err(anyhow!(message.clone())),
        }
    }
}

fn quarantine_invalid_conversation_rows(
    connection: &mut SqliteConnection,
    conversation_id: &str,
    reason: &str,
) -> Result<()> {
    connection.transaction::<_, anyhow::Error, _>(|connection| {
        diesel::sql_query(format!(
            "INSERT INTO {QUARANTINE_TABLE} (row_id, conversation_id, raw_row, reason, quarantined_at)
             SELECT id, conversation_id, kind || ':' || position || ':' || text, ?, ?
               FROM {QUEUE_TABLE} WHERE conversation_id = ?"
        ))
        .bind::<sql_types::Text, _>(reason)
        .bind::<sql_types::BigInt, _>(now_millis())
        .bind::<sql_types::Text, _>(conversation_id)
        .execute(connection)
        .context("quarantining rows with an invalid conversation id")?;
        diesel::sql_query(format!(
            "DELETE FROM {QUEUE_TABLE} WHERE conversation_id = ?"
        ))
        .bind::<sql_types::Text, _>(conversation_id)
        .execute(connection)
        .context("removing rows with an invalid conversation id")?;
        diesel::sql_query(format!(
            "DELETE FROM {SETTINGS_TABLE} WHERE conversation_id = ?"
        ))
        .bind::<sql_types::Text, _>(conversation_id)
        .execute(connection)
        .context("removing settings with an invalid conversation id")?;
        Ok(())
    })
}

fn create_tables(connection: &mut SqliteConnection) -> Result<()> {
    connection
        .batch_execute(
            &format!(
                "PRAGMA busy_timeout = 1000;
                CREATE TABLE IF NOT EXISTS {QUEUE_TABLE} (
                    id TEXT PRIMARY KEY NOT NULL,
                    conversation_id TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    text TEXT NOT NULL,
                    origin TEXT NOT NULL,
                    attachments_json TEXT NOT NULL DEFAULT '[]',
                    locked INTEGER NOT NULL DEFAULT 0,
                    attempt_count INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    dispatched_at INTEGER,
                    local_error TEXT
                );
                CREATE INDEX IF NOT EXISTS {QUEUE_TABLE}_conversation_position
                    ON {QUEUE_TABLE}(conversation_id, position, id);
                CREATE TABLE IF NOT EXISTS {SETTINGS_TABLE} (
                    conversation_id TEXT PRIMARY KEY NOT NULL,
                    queue_next_prompt_enabled INTEGER NOT NULL DEFAULT 0,
                    command_in_flight INTEGER NOT NULL DEFAULT 0,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS {QUARANTINE_TABLE} (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    row_id TEXT,
                    conversation_id TEXT NOT NULL,
                    raw_row TEXT NOT NULL,
                    reason TEXT NOT NULL,
                    quarantined_at INTEGER NOT NULL
                );"
            )
            .as_str(),
        )
        .context("creating local prompt queue tables")?;
    Ok(())
}

fn insert_row(connection: &mut SqliteConnection, row: &LocalPromptQueueRow) -> Result<()> {
    let attachments_json = serde_json::to_string(&row.attachments)
        .context("serializing local prompt queue attachments")?;
    diesel::sql_query(format!(
        "INSERT INTO {QUEUE_TABLE}
         (id, conversation_id, position, kind, text, origin, attachments_json,
          locked, attempt_count, created_at, updated_at, dispatched_at, local_error)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    ))
    .bind::<sql_types::Text, _>(row.id.to_string())
    .bind::<sql_types::Text, _>(row.conversation_id.to_string())
    .bind::<sql_types::BigInt, _>(row.position)
    .bind::<sql_types::Text, _>(kind_to_str(row.kind))
    .bind::<sql_types::Text, _>(&row.text)
    .bind::<sql_types::Text, _>(&row.origin)
    .bind::<sql_types::Text, _>(attachments_json)
    .bind::<sql_types::Integer, _>(row.locked as i32)
    .bind::<sql_types::Integer, _>(row.attempt_count as i32)
    .bind::<sql_types::BigInt, _>(row.created_at)
    .bind::<sql_types::BigInt, _>(row.updated_at)
    .bind::<sql_types::Nullable<sql_types::BigInt>, _>(row.dispatched_at)
    .bind::<sql_types::Nullable<sql_types::Text>, _>(row.local_error.as_deref())
    .execute(connection)
    .context("inserting local prompt queue row")?;
    Ok(())
}

fn load_conversation(
    connection: &mut SqliteConnection,
    conversation_id: AIConversationId,
) -> Result<LocalPromptQueueSnapshot> {
    let conversation_text = conversation_id.to_string();
    let db_rows = sql_query(format!(
        "SELECT id, conversation_id, position, kind, text, origin, attachments_json,
                locked, attempt_count, created_at, updated_at, dispatched_at, local_error
           FROM {QUEUE_TABLE}
          WHERE conversation_id = ?
          ORDER BY position ASC, id ASC"
    ))
    .bind::<sql_types::Text, _>(&conversation_text)
    .load::<QueueDbRow>(connection)
    .context("loading local prompt queue rows")?;

    let mut settings = sql_query(format!(
        "SELECT queue_next_prompt_enabled, command_in_flight
           FROM {SETTINGS_TABLE}
          WHERE conversation_id = ?"
    ))
    .bind::<sql_types::Text, _>(&conversation_text)
    .load::<SettingsDbRow>(connection)
    .context("loading local prompt queue settings")?
    .into_iter()
    .next()
    .map(|row| LocalPromptQueueSettings {
        queue_next_prompt_enabled: row.queue_next_prompt_enabled != 0,
        command_in_flight: row.command_in_flight != 0,
    })
    .unwrap_or_default();

    // A process restart cannot know whether a command side effect completed. Clear only the
    // transient gate; retain the row's dispatched marker and attempt count so it is visible for
    // explicit recovery and is never auto-replayed.
    if settings.command_in_flight {
        connection.transaction::<_, anyhow::Error, _>(|connection| {
            diesel::sql_query(format!(
                "UPDATE {SETTINGS_TABLE} SET command_in_flight = 0, updated_at = ?
                     WHERE conversation_id = ?"
            ))
            .bind::<sql_types::BigInt, _>(now_millis())
            .bind::<sql_types::Text, _>(&conversation_text)
            .execute(connection)
            .context("resetting local prompt command gate after restart")?;
            Ok(())
        })?;
        settings.command_in_flight = false;
    }

    let mut valid = Vec::with_capacity(db_rows.len());
    let mut corrupt = Vec::new();
    for db_row in db_rows {
        match convert_db_row(&db_row, conversation_id) {
            Ok(row) => valid.push(row),
            Err(reason) => corrupt.push((db_row, reason)),
        }
    }

    // Repair and quarantine are one transaction. A malformed row can never prevent valid rows
    // from loading, and positions are deterministic even when the database was hand-edited.
    connection.transaction::<_, anyhow::Error, _>(|connection| {
        for (row, reason) in &corrupt {
            diesel::sql_query(format!(
                "INSERT INTO {QUARANTINE_TABLE}
                 (row_id, conversation_id, raw_row, reason, quarantined_at)
                 VALUES (?, ?, ?, ?, ?)"
            ))
            .bind::<sql_types::Nullable<sql_types::Text>, _>(Some(row.id.as_str()))
            .bind::<sql_types::Text, _>(&conversation_text)
            .bind::<sql_types::Text, _>(format!("{}:{}:{}", row.kind, row.position, row.text))
            .bind::<sql_types::Text, _>(reason)
            .bind::<sql_types::BigInt, _>(now_millis())
            .execute(connection)
            .context("quarantining corrupt local prompt queue row")?;
            diesel::sql_query(format!(
                "DELETE FROM {QUEUE_TABLE} WHERE id = ? AND conversation_id = ?"
            ))
            .bind::<sql_types::Text, _>(&row.id)
            .bind::<sql_types::Text, _>(&conversation_text)
            .execute(connection)
            .context("removing corrupt local prompt queue row")?;
        }

        for (position, row) in valid.iter_mut().enumerate() {
            if row.position != position as i64 {
                row.position = position as i64;
                diesel::sql_query(format!(
                    "UPDATE {QUEUE_TABLE} SET position = ?, updated_at = ? WHERE id = ? AND conversation_id = ?"
                ))
                .bind::<sql_types::BigInt, _>(row.position)
                .bind::<sql_types::BigInt, _>(now_millis())
                .bind::<sql_types::Text, _>(row.id.to_string())
                .bind::<sql_types::Text, _>(&conversation_text)
                .execute(connection)
                .context("repairing local prompt queue position")?;
            }
            row.auto_fireable = !row.locked && row.dispatched_at.is_none() && row.local_error.is_none();
        }
        Ok(())
    })?;

    Ok(LocalPromptQueueSnapshot {
        rows: valid,
        settings,
    })
}

fn convert_db_row(
    db_row: &QueueDbRow,
    conversation_id: AIConversationId,
) -> Result<LocalPromptQueueRow, String> {
    let id = Uuid::parse_str(&db_row.id).map_err(|error| format!("invalid row id: {error}"))?;
    let stored_conversation_id = AIConversationId::try_from(db_row.conversation_id.clone())
        .map_err(|error| format!("invalid conversation id: {error}"))?;
    if stored_conversation_id != conversation_id {
        return Err("conversation id mismatch".into());
    }
    if db_row.position < 0 {
        return Err("negative queue position".into());
    }
    let kind = match db_row.kind.as_str() {
        "prompt" => LocalPromptQueueKind::Prompt,
        "command" => {
            if db_row.attachments_json != "[]" {
                return Err("command row carries attachments".into());
            }
            LocalPromptQueueKind::Command
        }
        other => return Err(format!("unknown queue row kind {other}")),
    };
    if !(db_row.locked == 0 || db_row.locked == 1) {
        return Err("invalid lock state".into());
    }
    if db_row.attempt_count < 0 {
        return Err("negative attempt count".into());
    }
    if !matches!(
        db_row.origin.as_str(),
        "queue_slash_command" | "auto_queue_toggle"
    ) {
        return Err(format!("unknown queue row origin {}", db_row.origin));
    }
    let attachments = serde_json::from_str(&db_row.attachments_json)
        .map_err(|error| format!("invalid attachment metadata: {error}"))?;
    Ok(LocalPromptQueueRow {
        id,
        conversation_id: stored_conversation_id,
        position: db_row.position,
        kind,
        text: db_row.text.clone(),
        origin: db_row.origin.clone(),
        attachments,
        locked: db_row.locked != 0,
        attempt_count: db_row.attempt_count as u32,
        created_at: db_row.created_at,
        updated_at: db_row.updated_at,
        dispatched_at: db_row.dispatched_at,
        local_error: db_row.local_error.clone(),
        auto_fireable: false,
    })
}

fn kind_to_str(kind: LocalPromptQueueKind) -> &'static str {
    match kind {
        LocalPromptQueueKind::Prompt => "prompt",
        LocalPromptQueueKind::Command => "command",
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}
