//! In-memory counterpart for targets without native filesystem persistence.
//!
//! The queue model still has the same repository contract on WASM and other non-native targets;
//! this implementation keeps the UI usable without pretending that rows are durable across a
//! process restart.

use std::{cell::RefCell, collections::HashMap, path::Path, rc::Rc};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ai::agent::conversation::AIConversationId;

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

#[derive(Clone)]
pub struct LocalPromptQueueRepository {
    inner: Rc<RefCell<StubState>>,
}

struct StubState {
    rows: HashMap<AIConversationId, Vec<LocalPromptQueueRow>>,
    settings: HashMap<AIConversationId, LocalPromptQueueSettings>,
    unavailable: Option<String>,
}

impl LocalPromptQueueRepository {
    pub fn in_memory() -> Result<Self> {
        Ok(Self {
            inner: Rc::new(RefCell::new(StubState {
                rows: HashMap::new(),
                settings: HashMap::new(),
                unavailable: None,
            })),
        })
    }

    pub fn open(_path: impl AsRef<Path>) -> Result<Self> {
        Err(anyhow!(
            "native local prompt queue persistence is unavailable on this target"
        ))
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(StubState {
                rows: HashMap::new(),
                settings: HashMap::new(),
                unavailable: Some(message.into()),
            })),
        }
    }

    pub fn startup_error(&self) -> Option<String> {
        self.inner.borrow().unavailable.clone()
    }

    #[cfg(test)]
    pub fn failing_for_test() -> Self {
        Self::unavailable("injected local prompt queue write failure")
    }

    pub fn load_conversation(
        &self,
        conversation_id: AIConversationId,
    ) -> Result<LocalPromptQueueSnapshot> {
        let mut state = self.inner.borrow_mut();
        ensure_available(&state)?;
        let mut rows = state
            .rows
            .get(&conversation_id)
            .cloned()
            .unwrap_or_default();
        rows.sort_by_key(|row| (row.position, row.id));
        for (position, row) in rows.iter_mut().enumerate() {
            row.position = position as i64;
            row.auto_fireable =
                !row.locked && row.dispatched_at.is_none() && row.local_error.is_none();
        }
        let mut settings = state
            .settings
            .get(&conversation_id)
            .copied()
            .unwrap_or_default();
        settings.command_in_flight = false;
        Ok(LocalPromptQueueSnapshot { rows, settings })
    }

    pub fn load_all(&self) -> Result<Vec<(AIConversationId, LocalPromptQueueSnapshot)>> {
        let ids: Vec<_> = {
            let state = self.inner.borrow();
            ensure_available(&state)?;
            state
                .rows
                .keys()
                .chain(state.settings.keys())
                .copied()
                .collect()
        };
        ids.into_iter()
            .map(|id| self.load_conversation(id).map(|snapshot| (id, snapshot)))
            .collect()
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
        let mut state = self.inner.borrow_mut();
        ensure_available(&state)?;
        if rows
            .iter()
            .any(|row| row.conversation_id != conversation_id)
        {
            return Err(anyhow!("queue row belongs to another conversation"));
        }
        state.rows.insert(conversation_id, rows.to_vec());
        state.settings.insert(conversation_id, settings);
        Ok(())
    }

    pub fn mark_dispatched(&self, conversation_id: AIConversationId, row_id: Uuid) -> Result<()> {
        self.dispatch_row(conversation_id, row_id, false)
    }

    pub fn dispatch_row(
        &self,
        conversation_id: AIConversationId,
        row_id: Uuid,
        command: bool,
    ) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        ensure_available(&state)?;
        let row = state
            .rows
            .get_mut(&conversation_id)
            .and_then(|rows| rows.iter_mut().find(|row| row.id == row_id))
            .ok_or_else(|| anyhow!("queue row is missing"))?;
        if row.dispatched_at.is_some() {
            return Err(anyhow!("queue row is already dispatched"));
        }
        row.attempt_count = row.attempt_count.saturating_add(1);
        row.dispatched_at = Some(now_millis());
        row.local_error = None;
        if command {
            state
                .settings
                .entry(conversation_id)
                .or_default()
                .command_in_flight = true;
        }
        Ok(())
    }

    pub fn complete_row(
        &self,
        conversation_id: AIConversationId,
        row_id: Uuid,
        command: bool,
    ) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        ensure_available(&state)?;
        if let Some(rows) = state.rows.get_mut(&conversation_id) {
            rows.retain(|row| row.id != row_id);
        }
        if command {
            state
                .settings
                .entry(conversation_id)
                .or_default()
                .command_in_flight = false;
        }
        Ok(())
    }

    pub fn clear_dispatched(&self, conversation_id: AIConversationId, row_id: Uuid) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        ensure_available(&state)?;
        if let Some(row) = state
            .rows
            .get_mut(&conversation_id)
            .and_then(|rows| rows.iter_mut().find(|row| row.id == row_id))
        {
            row.dispatched_at = None;
        }
        Ok(())
    }

    pub fn set_local_error(
        &self,
        conversation_id: AIConversationId,
        row_id: Uuid,
        message: Option<&str>,
    ) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        ensure_available(&state)?;
        if let Some(row) = state
            .rows
            .get_mut(&conversation_id)
            .and_then(|rows| rows.iter_mut().find(|row| row.id == row_id))
        {
            row.local_error = message.map(str::to_owned);
        }
        Ok(())
    }

    pub fn set_local_error_with_command_state(
        &self,
        conversation_id: AIConversationId,
        row_id: Uuid,
        message: &str,
        clear_command: bool,
    ) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        ensure_available(&state)?;
        if let Some(row) = state
            .rows
            .get_mut(&conversation_id)
            .and_then(|rows| rows.iter_mut().find(|row| row.id == row_id))
        {
            row.local_error = Some(message.to_owned());
        }
        if clear_command {
            state
                .settings
                .entry(conversation_id)
                .or_default()
                .command_in_flight = false;
        }
        Ok(())
    }

    pub fn retry_row(&self, conversation_id: AIConversationId, row_id: Uuid) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        ensure_available(&state)?;
        let row = state
            .rows
            .get_mut(&conversation_id)
            .and_then(|rows| rows.iter_mut().find(|row| row.id == row_id))
            .ok_or_else(|| anyhow!("queue row is missing"))?;
        row.dispatched_at = None;
        row.local_error = None;
        state
            .settings
            .entry(conversation_id)
            .or_default()
            .command_in_flight = false;
        Ok(())
    }

    pub fn delete_conversation(&self, conversation_id: AIConversationId) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        ensure_available(&state)?;
        state.rows.remove(&conversation_id);
        state.settings.remove(&conversation_id);
        Ok(())
    }

    pub fn set_command_in_flight(
        &self,
        conversation_id: AIConversationId,
        in_flight: bool,
    ) -> Result<()> {
        let mut state = self.inner.borrow_mut();
        ensure_available(&state)?;
        state
            .settings
            .entry(conversation_id)
            .or_default()
            .command_in_flight = in_flight;
        Ok(())
    }

    pub fn quarantined_count(&self) -> Result<i64> {
        Ok(0)
    }

    #[cfg(test)]
    pub fn insert_raw_for_test(
        &self,
        _id: Uuid,
        _conversation_id: AIConversationId,
        _position: i64,
        _kind: &str,
        _text: &str,
        _origin: &str,
        _attachments_json: &str,
    ) -> Result<()> {
        Err(anyhow!(
            "raw SQLite fixtures are unavailable on this target"
        ))
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
}

fn ensure_available(state: &StubState) -> Result<()> {
    if let Some(message) = &state.unavailable {
        Err(anyhow!(message.clone()))
    } else {
        Ok(())
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}
