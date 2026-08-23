use std::collections::HashMap;
use std::{fs, path::PathBuf};

use anyhow::Context;
use uuid::Uuid;
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::ai::agent::ImageContext;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::blocklist::{
    BlocklistAIHistoryEvent, BlocklistAIHistoryModel, PendingAttachment, PendingFile,
};
use crate::persistence::local_prompt_queue::{
    LocalPromptQueueAttachment, LocalPromptQueueKind, LocalPromptQueueRepository,
    LocalPromptQueueRow, LocalPromptQueueSettings,
};

/// A globally unique identifier for a single queued prompt row.
/// Used by the queue panel to address rows across reorder, edit, and delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueuedQueryId(Uuid);

impl QueuedQueryId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }

    fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    fn as_uuid(self) -> Uuid {
        self.0
    }
}

/// Where a queued prompt came from.
/// The origin is informational for UI behavior; FIFO ordering and firing semantics are uniform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuedQueryOrigin {
    /// Filed while the initial Cloud Mode prompt waits to be handed off.
    InitialCloudMode,
    /// Filed via the `/queue <prompt>` slash command.
    QueueSlashCommand,
    /// Filed via the auto-queue toggle in the warping indicator.
    AutoQueueToggle,
}

impl QueuedQueryOrigin {
    fn as_str(self) -> &'static str {
        match self {
            Self::InitialCloudMode => "initial_cloud_mode",
            Self::QueueSlashCommand => "queue_slash_command",
            Self::AutoQueueToggle => "auto_queue_toggle",
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "initial_cloud_mode" => Ok(Self::InitialCloudMode),
            "queue_slash_command" => Ok(Self::QueueSlashCommand),
            "auto_queue_toggle" => Ok(Self::AutoQueueToggle),
            other => Err(anyhow::anyhow!("unknown queued prompt origin {other}")),
        }
    }
}

/// Whether a queued row is an agent prompt or a shell command. Commands cannot carry attachments.
#[derive(Debug, Clone)]
enum QueuedQueryKind {
    Prompt { attachments: Vec<PendingAttachment> },
    Command,
}

/// A single durable queued row: an agent prompt or a shell command.
#[derive(Debug, Clone)]
pub struct QueuedQuery {
    id: QueuedQueryId,
    text: String,
    origin: QueuedQueryOrigin,
    kind: QueuedQueryKind,
    file_fingerprints: Vec<Option<(u64, i64)>>,
    locked: bool,
    attempt_count: u32,
    created_at: i64,
    updated_at: i64,
    dispatched_at: Option<i64>,
    local_error: Option<String>,
}

impl QueuedQuery {
    pub fn new(text: String, origin: QueuedQueryOrigin) -> Self {
        Self::new_with_attachments(text, origin, Vec::new())
    }

    pub fn new_with_attachments(
        text: String,
        origin: QueuedQueryOrigin,
        attachments: Vec<PendingAttachment>,
    ) -> Self {
        let now = now_millis();
        let file_fingerprints = attachments
            .iter()
            .map(|attachment| match attachment {
                PendingAttachment::File(file) => file_fingerprint(&file.file_path).ok(),
                PendingAttachment::Image(_) => None,
            })
            .collect();
        Self {
            id: QueuedQueryId::new(),
            text,
            origin,
            kind: QueuedQueryKind::Prompt { attachments },
            file_fingerprints,
            locked: matches!(origin, QueuedQueryOrigin::InitialCloudMode),
            attempt_count: 0,
            created_at: now,
            updated_at: now,
            dispatched_at: None,
            local_error: None,
        }
    }

    pub fn new_command(text: String, origin: QueuedQueryOrigin) -> Self {
        let now = now_millis();
        Self {
            id: QueuedQueryId::new(),
            text,
            origin,
            kind: QueuedQueryKind::Command,
            file_fingerprints: Vec::new(),
            locked: matches!(origin, QueuedQueryOrigin::InitialCloudMode),
            attempt_count: 0,
            created_at: now,
            updated_at: now,
            dispatched_at: None,
            local_error: None,
        }
    }

    pub fn id(&self) -> QueuedQueryId {
        self.id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn origin(&self) -> QueuedQueryOrigin {
        self.origin
    }

    pub fn is_command(&self) -> bool {
        matches!(self.kind, QueuedQueryKind::Command)
    }

    pub fn attachments(&self) -> &[PendingAttachment] {
        match &self.kind {
            QueuedQueryKind::Prompt { attachments } => attachments,
            QueuedQueryKind::Command => &[],
        }
    }

    pub fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    pub fn has_local_error(&self) -> bool {
        self.local_error.is_some()
    }

    pub fn local_error(&self) -> Option<&str> {
        self.local_error.as_deref()
    }

    pub fn is_locked(&self) -> bool {
        self.locked || matches!(self.origin, QueuedQueryOrigin::InitialCloudMode)
    }

    fn to_repository_row(
        &self,
        conversation_id: AIConversationId,
        position: usize,
    ) -> LocalPromptQueueRow {
        let attachments = self
            .attachments()
            .iter()
            .enumerate()
            .map(|(index, attachment)| {
                local_attachment_with_fingerprint(
                    attachment,
                    self.file_fingerprints.get(index).copied().flatten(),
                )
            })
            .collect();
        LocalPromptQueueRow {
            id: self.id.as_uuid(),
            conversation_id,
            position: position as i64,
            kind: if self.is_command() {
                LocalPromptQueueKind::Command
            } else {
                LocalPromptQueueKind::Prompt
            },
            text: self.text.clone(),
            origin: self.origin.as_str().to_owned(),
            attachments,
            locked: self.is_locked(),
            attempt_count: self.attempt_count,
            created_at: self.created_at,
            updated_at: self.updated_at,
            dispatched_at: self.dispatched_at,
            local_error: self.local_error.clone(),
            auto_fireable: false,
        }
    }

    fn from_repository_row(row: LocalPromptQueueRow) -> anyhow::Result<Self> {
        let origin = QueuedQueryOrigin::parse(&row.origin)?;
        let file_fingerprints = row
            .attachments
            .iter()
            .map(|attachment| match attachment {
                LocalPromptQueueAttachment::FileWithFingerprint {
                    size, modified_at, ..
                } => Some((*size, *modified_at)),
                _ => None,
            })
            .collect();
        let kind = match row.kind {
            LocalPromptQueueKind::Prompt => QueuedQueryKind::Prompt {
                attachments: row
                    .attachments
                    .into_iter()
                    .map(pending_attachment)
                    .collect::<anyhow::Result<Vec<_>>>()?,
            },
            LocalPromptQueueKind::Command => QueuedQueryKind::Command,
        };
        Ok(Self {
            id: QueuedQueryId::from_uuid(row.id),
            text: row.text,
            origin,
            kind,
            file_fingerprints,
            locked: row.locked,
            attempt_count: row.attempt_count,
            created_at: row.created_at,
            updated_at: row.updated_at,
            dispatched_at: row.dispatched_at,
            local_error: row.local_error,
        })
    }
}

/// What the auto-fire drain should do with a popped row.
#[derive(Debug)]
pub enum AutofireAction {
    /// Submit this prompt as a normal queued user query.
    Submit { text: String },
    /// The popped row was in edit mode at the time of pop.
    /// The caller places `text` (the row's last committed text) in the input box.
    PopFromEditMode { text: String },
    /// Execute the head row as a local shell command.
    ExecuteCommand { command: String },
}

/// Per-conversation queue / edit / toggle state.
/// Lives inside [`QueuedQueryModel::queues`]; a missing key means empty queue, no edit in
/// progress, and toggle off.
#[derive(Clone, Default)]
struct ConversationQueueState {
    queue: Vec<QueuedQuery>,
    editing: Option<QueuedQueryId>,
    queue_next_prompt_enabled: bool,
    command_in_flight: bool,
}

/// App-wide singleton owning the queued prompts and auto-queue toggle for every conversation,
/// indexed by [`AIConversationId`]. Queues outlive the agent-view session that originated them;
/// cleanup is driven by [`BlocklistAIHistoryModel`] lifecycle events that this model subscribes
/// to in [`QueuedQueryModel::new`].
pub struct QueuedQueryModel {
    queues: HashMap<AIConversationId, ConversationQueueState>,
    repository: LocalPromptQueueRepository,
}

/// Events emitted by [`QueuedQueryModel`]. Every variant carries the `conversation_id` it applies
/// to so subscribers can filter to the conversation they care about.
#[derive(Debug, Clone)]
pub enum QueuedQueryEvent {
    Appended {
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
    },
    Removed {
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
    },
    Reordered {
        conversation_id: AIConversationId,
    },
    EditEntered {
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
    },
    EditCommitted {
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
    },
    EditCancelled {
        conversation_id: AIConversationId,
        #[allow(dead_code)]
        query_id: QueuedQueryId,
    },
    Cleared {
        conversation_id: AIConversationId,
    },
    QueueNextPromptToggled {
        conversation_id: AIConversationId,
    },
    LocalError {
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
    },
    PersistenceError {
        conversation_id: AIConversationId,
    },
}

impl Entity for QueuedQueryModel {
    type Event = QueuedQueryEvent;
}

impl SingletonEntity for QueuedQueryModel {}

impl QueuedQueryModel {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let repository = LocalPromptQueueRepository::in_memory()
            .expect("in-memory queue repository should be available");
        Self::new_with_repository(repository, ctx)
    }

    /// Builds the production model against the app-scoped SQLite database. Tests use [`Self::new`]
    /// so they never write queue fixtures into the user's database.
    pub fn new_persistent(ctx: &mut ModelContext<Self>) -> Self {
        let repository = crate::persistence::local_prompt_queue::LocalPromptQueueRepository::open(
            crate::persistence::database_file_path_for_current_scope(),
        )
        .unwrap_or_else(|error| {
            log::error!("failed to open durable local prompt queue: {error:#}");
            LocalPromptQueueRepository::in_memory()
                .expect("in-memory queue repository should be available")
        });
        Self::new_with_repository(repository, ctx)
    }

    pub fn new_with_repository(
        repository: LocalPromptQueueRepository,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        // Drop queue/toggle state for any conversation that is removed, deleted, or cleared
        // from its owning terminal view. Agent-view exit is intentionally NOT subscribed to:
        // conversations (cloud agents in particular) outlive their visible session.
        let history_handle = BlocklistAIHistoryModel::handle(ctx);
        ctx.subscribe_to_model(&history_handle, |this, _, event, ctx| {
            this.handle_history_event(event, ctx);
        });

        let mut queues = HashMap::new();
        match repository.load_all() {
            Ok(snapshots) => {
                for (conversation_id, snapshot) in snapshots {
                    let rows = snapshot
                        .rows
                        .into_iter()
                        .filter_map(|row| match QueuedQuery::from_repository_row(row) {
                            Ok(row) => Some(row),
                            Err(error) => {
                                log::error!(
                                    "skipping invalid local prompt queue row for {conversation_id}: {error:#}"
                                );
                                None
                            }
                        })
                        .collect();
                    queues.insert(
                        conversation_id,
                        ConversationQueueState {
                            queue: rows,
                            editing: None,
                            queue_next_prompt_enabled: snapshot.settings.queue_next_prompt_enabled,
                            command_in_flight: snapshot.settings.command_in_flight,
                        },
                    );
                }
            }
            Err(error) => log::error!("failed to load durable local prompt queue: {error:#}"),
        }

        Self { queues, repository }
    }

    fn handle_history_event(
        &mut self,
        event: &BlocklistAIHistoryEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        match event {
            BlocklistAIHistoryEvent::RemoveConversation {
                conversation_id, ..
            }
            | BlocklistAIHistoryEvent::DeletedConversation {
                conversation_id, ..
            } => {
                self.drop_conversation(*conversation_id, ctx);
            }
            BlocklistAIHistoryEvent::ClearedConversationsForTerminalSurface {
                cleared_conversation_ids,
                ..
            } => {
                for conversation_id in cleared_conversation_ids.clone() {
                    self.drop_conversation(conversation_id, ctx);
                }
            }
            _ => {}
        }
    }

    fn drop_conversation(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) {
        if let Err(error) = self.repository.delete_conversation(conversation_id) {
            log::error!("failed to delete local prompt queue for {conversation_id}: {error:#}");
            return;
        }
        if self.queues.remove(&conversation_id).is_some() {
            ctx.emit(QueuedQueryEvent::Cleared { conversation_id });
        }
    }

    fn persist_state(
        &self,
        conversation_id: AIConversationId,
        state: &ConversationQueueState,
    ) -> anyhow::Result<()> {
        let rows: Vec<_> = state
            .queue
            .iter()
            .enumerate()
            .map(|(position, row)| row.to_repository_row(conversation_id, position))
            .collect();
        self.repository.replace_conversation_with_settings(
            conversation_id,
            &rows,
            LocalPromptQueueSettings {
                queue_next_prompt_enabled: state.queue_next_prompt_enabled,
                command_in_flight: state.command_in_flight,
            },
        )
    }

    /// Returns the queue for `conversation_id`. Returns an empty slice when no entry exists.
    pub fn queue(&self, conversation_id: AIConversationId) -> &[QueuedQuery] {
        self.queues
            .get(&conversation_id)
            .map(|state| state.queue.as_slice())
            .unwrap_or(&[])
    }

    /// Returns true when `conversation_id` has at least one queued prompt.
    pub fn has_queue(&self, conversation_id: AIConversationId) -> bool {
        self.queues
            .get(&conversation_id)
            .is_some_and(|state| !state.queue.is_empty())
    }

    /// Returns true when a queued row would auto-fire for `conversation_id` the next time the
    /// conversation finishes successfully. Mirrors [`Self::peek_autofire`]'s gating: false for an
    /// empty queue or a locked head row (which never auto-fires).
    pub fn has_autofireable_prompt(&self, conversation_id: AIConversationId) -> bool {
        self.queues
            .get(&conversation_id)
            .filter(|state| !state.command_in_flight)
            .and_then(|state| state.queue.first())
            .is_some_and(|first| {
                !first.is_locked() && first.dispatched_at.is_none() && first.local_error.is_none()
            })
    }

    /// Returns the row currently in edit mode for `conversation_id`, if any.
    pub fn editing_row(&self, conversation_id: AIConversationId) -> Option<QueuedQueryId> {
        self.queues
            .get(&conversation_id)
            .and_then(|state| state.editing)
    }

    /// Returns true when the head row of `conversation_id`'s queue is currently being edited.
    pub fn first_row_is_in_edit_mode(&self, conversation_id: AIConversationId) -> bool {
        let Some(state) = self.queues.get(&conversation_id) else {
            return false;
        };
        let Some(editing_id) = state.editing else {
            return false;
        };
        state.queue.first().is_some_and(|q| q.id == editing_id)
    }

    /// Returns the per-conversation auto-queue toggle state. Defaults to false for conversations
    /// that have never been touched.
    pub fn is_queue_next_prompt_enabled(&self, conversation_id: AIConversationId) -> bool {
        self.queues
            .get(&conversation_id)
            .is_some_and(|state| state.queue_next_prompt_enabled)
    }

    /// Toggles the per-conversation auto-queue state.
    pub fn toggle_queue_next_prompt(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) {
        let mut next = self
            .queues
            .get(&conversation_id)
            .cloned()
            .unwrap_or_default();
        next.queue_next_prompt_enabled = !next.queue_next_prompt_enabled;
        if let Err(error) = self.persist_state(conversation_id, &next) {
            log::error!("failed to persist queued prompt toggle: {error:#}");
            ctx.emit(QueuedQueryEvent::PersistenceError { conversation_id });
            return;
        }
        self.queues.insert(conversation_id, next);
        ctx.emit(QueuedQueryEvent::QueueNextPromptToggled { conversation_id });
    }

    pub fn try_toggle_queue_next_prompt(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<()> {
        let mut next = self
            .queues
            .get(&conversation_id)
            .cloned()
            .unwrap_or_default();
        next.queue_next_prompt_enabled = !next.queue_next_prompt_enabled;
        self.persist_state(conversation_id, &next)?;
        self.queues.insert(conversation_id, next);
        ctx.emit(QueuedQueryEvent::QueueNextPromptToggled { conversation_id });
        Ok(())
    }

    /// Appends `query` to the tail of `conversation_id`'s queue.
    pub fn append(
        &mut self,
        conversation_id: AIConversationId,
        query: QueuedQuery,
        ctx: &mut ModelContext<Self>,
    ) -> QueuedQueryId {
        let query_id = query.id;
        if let Err(error) = self.try_append(conversation_id, query, ctx) {
            log::error!("failed to append local prompt queue row: {error:#}");
            ctx.emit(QueuedQueryEvent::PersistenceError { conversation_id });
        }
        query_id
    }

    pub fn try_append(
        &mut self,
        conversation_id: AIConversationId,
        query: QueuedQuery,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<QueuedQueryId> {
        let query_id = query.id;
        let mut next = self
            .queues
            .get(&conversation_id)
            .cloned()
            .unwrap_or_default();
        next.queue.push(query);
        self.persist_state(conversation_id, &next)?;
        self.queues.insert(conversation_id, next);
        ctx.emit(QueuedQueryEvent::Appended {
            conversation_id,
            query_id,
        });
        Ok(query_id)
    }

    /// Pops the first row in `conversation_id`'s queue and returns it. Used by the error/cancel
    /// drain path where the caller restores the popped text to the input editor.
    pub fn pop_front(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) -> Option<QueuedQuery> {
        let mut next = self.queues.get(&conversation_id)?.clone();
        let head = next.queue.first()?;
        if head.is_locked() {
            return None;
        }
        let popped = next.queue.remove(0);
        if next.editing == Some(popped.id) {
            next.editing = None;
        }
        if let Err(error) = self.persist_state(conversation_id, &next) {
            log::error!("failed to persist queued prompt removal: {error:#}");
            ctx.emit(QueuedQueryEvent::PersistenceError { conversation_id });
            return None;
        }
        self.queues.insert(conversation_id, next);
        ctx.emit(QueuedQueryEvent::Removed {
            conversation_id,
            query_id: popped.id,
        });
        Some(popped)
    }

    /// Auto-fire drain entry point for `conversation_id`. Pops the first row and tells the caller
    /// whether to submit it normally or treat it as a popped edit-mode row (per the spec, the
    /// row's last-committed text is restored to the input box).
    pub fn pop_for_autofire(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) -> Option<AutofireAction> {
        let action = self.peek_autofire(conversation_id)?;
        let query_id = self.queues.get(&conversation_id)?.queue.first()?.id;
        self.remove_fired_row(conversation_id, query_id, ctx)?;
        Some(action)
    }

    /// Returns an action for the head row without removing it. Runtime dispatch uses this method,
    /// followed by [`Self::begin_dispatch`], so a crash after dispatch leaves the durable row for
    /// explicit user recovery instead of silently replaying a side effect.
    pub fn peek_autofire(&self, conversation_id: AIConversationId) -> Option<AutofireAction> {
        let state = self.queues.get(&conversation_id)?;
        if state.command_in_flight {
            return None;
        }
        let first = state.queue.first()?;
        if first.is_locked() || first.dispatched_at.is_some() || first.local_error.is_some() {
            return None;
        }
        if state.editing == Some(first.id) {
            return Some(AutofireAction::PopFromEditMode {
                text: first.text.clone(),
            });
        }
        Some(if first.is_command() {
            AutofireAction::ExecuteCommand {
                command: first.text.clone(),
            }
        } else {
            AutofireAction::Submit {
                text: first.text.clone(),
            }
        })
    }

    /// Commits the attempt marker before an external prompt or command dispatch. The head remains
    /// in memory and on disk until [`Self::complete_dispatch`] confirms the side effect.
    pub fn begin_dispatch(
        &mut self,
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<()> {
        self.begin_dispatch_with_capability(conversation_id, query_id, true, ctx)
    }

    /// Begins a dispatch only when the local provider/capability is available. A failed
    /// capability or attachment check is durable local state and does not fall back to Warp.
    pub fn begin_dispatch_with_capability(
        &mut self,
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
        capability_available: bool,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<()> {
        let state = self
            .queues
            .get(&conversation_id)
            .ok_or_else(|| anyhow::anyhow!("conversation queue is missing"))?;
        let row = state
            .queue
            .iter()
            .find(|row| row.id == query_id)
            .ok_or_else(|| anyhow::anyhow!("queued row is missing"))?;
        if state.queue.first().map(QueuedQuery::id) != Some(query_id) {
            return Err(anyhow::anyhow!("only the queue head can be dispatched"));
        }
        if state.command_in_flight || row.is_locked() || row.dispatched_at.is_some() {
            return Err(anyhow::anyhow!("queued row is not dispatchable"));
        }
        let row = row.clone();
        if !capability_available {
            let error = anyhow::anyhow!("local queue provider or capability is unavailable");
            self.mark_local_error(conversation_id, query_id, &error.to_string(), ctx)?;
            return Err(error);
        }
        if let Err(error) = validate_attachments(row.attachments(), &row.file_fingerprints) {
            self.mark_local_error(conversation_id, query_id, &error.to_string(), ctx)?;
            return Err(error);
        }
        let command = row.is_command();
        self.repository
            .dispatch_row(conversation_id, query_id.as_uuid(), command)?;
        let next = self
            .queues
            .get_mut(&conversation_id)
            .expect("queue checked above");
        let row = next
            .queue
            .iter_mut()
            .find(|row| row.id == query_id)
            .expect("queue row checked above");
        row.attempt_count = row.attempt_count.saturating_add(1);
        row.dispatched_at = Some(now_millis());
        row.local_error = None;
        if command {
            next.command_in_flight = true;
        }
        ctx.notify();
        Ok(())
    }

    /// Explicitly records a local dispatch error while retaining the row for retry/edit/delete.
    pub fn mark_local_error(
        &mut self,
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
        message: &str,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<()> {
        let clear_command_in_flight = self
            .queues
            .get(&conversation_id)
            .and_then(|state| state.queue.iter().find(|row| row.id == query_id))
            .is_some_and(QueuedQuery::is_command);
        self.repository.set_local_error_with_command_state(
            conversation_id,
            query_id.as_uuid(),
            message,
            clear_command_in_flight,
        )?;
        let state = self
            .queues
            .get_mut(&conversation_id)
            .ok_or_else(|| anyhow::anyhow!("conversation queue is missing"))?;
        let row = state
            .queue
            .iter_mut()
            .find(|row| row.id == query_id)
            .ok_or_else(|| anyhow::anyhow!("queued row is missing"))?;
        row.local_error = Some(message.to_owned());
        if clear_command_in_flight {
            state.command_in_flight = false;
        }
        ctx.emit(QueuedQueryEvent::LocalError {
            conversation_id,
            query_id,
        });
        Ok(())
    }

    /// Clears a dispatch marker only after the user explicitly asks to retry a retained row.
    pub fn retry_row(
        &mut self,
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<()> {
        self.repository
            .retry_row(conversation_id, query_id.as_uuid())?;
        let state = self
            .queues
            .get_mut(&conversation_id)
            .ok_or_else(|| anyhow::anyhow!("conversation queue is missing"))?;
        let row = state
            .queue
            .iter_mut()
            .find(|row| row.id == query_id)
            .ok_or_else(|| anyhow::anyhow!("queued row is missing"))?;
        row.dispatched_at = None;
        row.local_error = None;
        state.command_in_flight = false;
        ctx.notify();
        Ok(())
    }

    /// Confirms a previously dispatched row and advances the durable queue. Command completion
    /// clears the command-in-flight gate atomically with row deletion.
    pub fn complete_dispatch(
        &mut self,
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<Option<QueuedQuery>> {
        let state = self
            .queues
            .get(&conversation_id)
            .ok_or_else(|| anyhow::anyhow!("conversation queue is missing"))?;
        let Some(index) = state.queue.iter().position(|row| row.id == query_id) else {
            return Ok(None);
        };
        let is_command = state.queue[index].is_command();
        self.repository
            .complete_row(conversation_id, query_id.as_uuid(), is_command)?;
        let state = self
            .queues
            .get_mut(&conversation_id)
            .expect("queue checked above");
        let removed = state.queue.remove(index);
        if state.editing == Some(query_id) {
            state.editing = None;
        }
        if is_command {
            state.command_in_flight = false;
        }
        ctx.emit(QueuedQueryEvent::Removed {
            conversation_id,
            query_id,
        });
        Ok(Some(removed))
    }

    pub fn has_command_in_flight(&self, conversation_id: AIConversationId) -> bool {
        self.queues
            .get(&conversation_id)
            .is_some_and(|state| state.command_in_flight)
    }

    /// Completes the command currently holding the per-conversation gate. This is called from
    /// the terminal's block-completed lifecycle event, not from the dispatch initiation path.
    pub fn complete_command_in_flight(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<Option<QueuedQuery>> {
        let Some(state) = self.queues.get(&conversation_id) else {
            return Ok(None);
        };
        if !state.command_in_flight {
            return Ok(None);
        }
        let Some(row_id) = state
            .queue
            .iter()
            .find(|row| row.is_command() && row.dispatched_at.is_some())
            .map(QueuedQuery::id)
        else {
            return Ok(None);
        };
        self.complete_dispatch(conversation_id, row_id, ctx)
    }

    /// Completes a prompt whose dispatch marker was committed on the previous terminal-state
    /// transition. Keeping the marker until the next terminal state avoids dropping a row during
    /// an uncertain provider failure.
    pub fn complete_prompt_in_flight(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<Option<QueuedQuery>> {
        let Some(state) = self.queues.get(&conversation_id) else {
            return Ok(None);
        };
        let Some(row_id) = state
            .queue
            .first()
            .filter(|row| !row.is_command() && row.dispatched_at.is_some())
            .map(QueuedQuery::id)
        else {
            return Ok(None);
        };
        self.complete_dispatch(conversation_id, row_id, ctx)
    }

    /// Retains a prompt after an error/cancel terminal state and records a local error instead of
    /// silently retrying a possibly delivered provider request.
    pub fn fail_prompt_in_flight(
        &mut self,
        conversation_id: AIConversationId,
        message: &str,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<bool> {
        let Some(row_id) = self
            .queues
            .get(&conversation_id)
            .and_then(|state| state.queue.first())
            .filter(|row| !row.is_command() && row.dispatched_at.is_some())
            .map(QueuedQuery::id)
        else {
            return Ok(false);
        };
        self.mark_local_error(conversation_id, row_id, message, ctx)?;
        Ok(true)
    }

    /// Removes a row after a confirmed dispatch. This is separate from [`Self::begin_dispatch`]
    /// so an uncertain network/provider result leaves the attempted row durable.
    pub fn remove_fired_row(
        &mut self,
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
        ctx: &mut ModelContext<Self>,
    ) -> Option<QueuedQuery> {
        let state = self.queues.get(&conversation_id)?.clone();
        let index = state.queue.iter().position(|row| row.id == query_id)?;
        let mut next = state.clone();
        let removed = next.queue.remove(index);
        if next.editing == Some(query_id) {
            next.editing = None;
        }
        if let Err(error) = self.persist_state(conversation_id, &next) {
            log::error!("failed to persist queued prompt removal: {error:#}");
            ctx.emit(QueuedQueryEvent::PersistenceError { conversation_id });
            return None;
        }
        self.queues.insert(conversation_id, next);
        ctx.emit(QueuedQueryEvent::Removed {
            conversation_id,
            query_id,
        });
        Some(removed)
    }

    /// Removes a specific row by id within `conversation_id`'s queue, if present.
    pub fn remove_by_id(
        &mut self,
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
        ctx: &mut ModelContext<Self>,
    ) -> Option<QueuedQuery> {
        let state = self.queues.get(&conversation_id)?.clone();
        let idx = state.queue.iter().position(|q| q.id == query_id)?;
        if state.queue[idx].is_locked() {
            return None;
        }
        let mut next = state;
        let removed = next.queue.remove(idx);
        if next.editing == Some(query_id) {
            next.editing = None;
        }
        if let Err(error) = self.persist_state(conversation_id, &next) {
            log::error!("failed to persist queued prompt removal: {error:#}");
            ctx.emit(QueuedQueryEvent::PersistenceError { conversation_id });
            return None;
        }
        self.queues.insert(conversation_id, next);
        ctx.emit(QueuedQueryEvent::Removed {
            conversation_id,
            query_id,
        });
        Some(removed)
    }

    /// Moves the row identified by `source_id` to position `target_index` within
    /// `conversation_id`'s queue. `target_index` is interpreted as the index in the post-removal
    /// list and is clamped to the queue length.
    pub fn reorder(
        &mut self,
        conversation_id: AIConversationId,
        source_id: QueuedQueryId,
        target_index: usize,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(state) = self.queues.get(&conversation_id).cloned() else {
            return;
        };
        let Some(source_idx) = state.queue.iter().position(|q| q.id == source_id) else {
            return;
        };
        if state.queue[source_idx].is_locked() {
            return;
        }
        let mut next = state;
        let row = next.queue.remove(source_idx);
        let clamped = target_index.min(next.queue.len());
        next.queue.insert(clamped, row);
        if let Err(error) = self.persist_state(conversation_id, &next) {
            log::error!("failed to persist queued prompt reorder: {error:#}");
            ctx.emit(QueuedQueryEvent::PersistenceError { conversation_id });
            return;
        }
        self.queues.insert(conversation_id, next);
        ctx.emit(QueuedQueryEvent::Reordered { conversation_id });
    }

    /// Enters edit mode for `query_id` in `conversation_id`'s queue. If another row was being
    /// edited, that edit is cancelled (its text is unchanged, per the spec).
    pub fn enter_edit_mode(
        &mut self,
        conversation_id: AIConversationId,
        query_id: QueuedQueryId,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(state) = self.queues.get_mut(&conversation_id) else {
            return;
        };
        if !state.queue.iter().any(|q| q.id == query_id) {
            return;
        }
        let prev_edit = state.editing.replace(query_id);
        if let Some(prev) = prev_edit
            && prev != query_id
        {
            ctx.emit(QueuedQueryEvent::EditCancelled {
                conversation_id,
                query_id: prev,
            });
        }
        ctx.emit(QueuedQueryEvent::EditEntered {
            conversation_id,
            query_id,
        });
    }

    /// Commits the in-progress edit in `conversation_id` by replacing the row's text with
    /// `new_text` and clearing edit state. An empty `new_text` cancels the edit and leaves the
    /// original row text untouched.
    pub fn commit_edit(
        &mut self,
        conversation_id: AIConversationId,
        new_text: String,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(state) = self.queues.get(&conversation_id).cloned() else {
            return;
        };
        let Some(query_id) = state.editing else {
            return;
        };
        if new_text.is_empty() {
            if let Some(state) = self.queues.get_mut(&conversation_id) {
                state.editing = None;
            }
            ctx.emit(QueuedQueryEvent::EditCancelled {
                conversation_id,
                query_id,
            });
            return;
        }
        let mut next = state;
        next.editing = None;
        if let Some(row) = next.queue.iter_mut().find(|q| q.id == query_id) {
            row.text = new_text;
            row.updated_at = now_millis();
        } else {
            return;
        }
        if let Err(error) = self.persist_state(conversation_id, &next) {
            log::error!("failed to persist queued prompt edit: {error:#}");
            ctx.emit(QueuedQueryEvent::PersistenceError { conversation_id });
            return;
        }
        self.queues.insert(conversation_id, next);
        ctx.emit(QueuedQueryEvent::EditCommitted {
            conversation_id,
            query_id,
        });
    }

    /// Cancels the in-progress edit in `conversation_id` without modifying the row's text.
    pub fn cancel_edit(&mut self, conversation_id: AIConversationId, ctx: &mut ModelContext<Self>) {
        let Some(state) = self.queues.get_mut(&conversation_id) else {
            return;
        };
        let Some(query_id) = state.editing.take() else {
            return;
        };
        ctx.emit(QueuedQueryEvent::EditCancelled {
            conversation_id,
            query_id,
        });
    }
}

fn local_attachment_with_fingerprint(
    attachment: &PendingAttachment,
    fingerprint: Option<(u64, i64)>,
) -> LocalPromptQueueAttachment {
    match attachment {
        PendingAttachment::Image(image) => LocalPromptQueueAttachment::Image {
            data: image.data.clone(),
            file_name: image.file_name.clone(),
            mime_type: image.mime_type.clone(),
        },
        PendingAttachment::File(file) => {
            let path = file.file_path.to_string_lossy().into_owned();
            match fingerprint.or_else(|| file_fingerprint(&file.file_path).ok()) {
                Some((size, modified_at)) => LocalPromptQueueAttachment::FileWithFingerprint {
                    path,
                    file_name: file.file_name.clone(),
                    mime_type: file.mime_type.clone(),
                    size,
                    modified_at,
                },
                None => LocalPromptQueueAttachment::File {
                    path,
                    file_name: file.file_name.clone(),
                    mime_type: file.mime_type.clone(),
                },
            }
        }
    }
}

fn pending_attachment(attachment: LocalPromptQueueAttachment) -> anyhow::Result<PendingAttachment> {
    Ok(match attachment {
        LocalPromptQueueAttachment::Image {
            data,
            file_name,
            mime_type,
        } => PendingAttachment::Image(ImageContext {
            data,
            mime_type,
            file_name,
            is_figma: false,
        }),
        LocalPromptQueueAttachment::File {
            path,
            file_name,
            mime_type,
        } => PendingAttachment::File(PendingFile {
            file_name,
            file_path: PathBuf::from(path),
            mime_type,
        }),
        LocalPromptQueueAttachment::FileWithFingerprint {
            path,
            file_name,
            mime_type,
            ..
        } => PendingAttachment::File(PendingFile {
            file_name,
            file_path: PathBuf::from(path),
            mime_type,
        }),
    })
}

fn validate_attachments(
    attachments: &[PendingAttachment],
    fingerprints: &[Option<(u64, i64)>],
) -> anyhow::Result<()> {
    for (index, attachment) in attachments.iter().enumerate() {
        match attachment {
            PendingAttachment::Image(image) => {
                // Image bytes are already held locally; retain a bounded payload and leave
                // provider-specific MIME/format validation to the direct local adapter.
                if image.data.len() > 32 * 1024 * 1024 {
                    return Err(anyhow::anyhow!(
                        "local queue image attachment exceeds the 32 MiB limit"
                    ));
                }
            }
            PendingAttachment::File(file) => {
                let metadata = fs::metadata(&file.file_path).with_context(|| {
                    format!(
                        "queued file attachment is unavailable: {}",
                        file.file_path.display()
                    )
                })?;
                if !metadata.is_file() {
                    return Err(anyhow::anyhow!(
                        "queued file attachment is not a regular file: {}",
                        file.file_path.display()
                    ));
                }
                if let Some((expected_size, expected_modified_at)) =
                    fingerprints.get(index).copied().flatten()
                {
                    let actual_modified_at = metadata
                        .modified()
                        .context("reading queued file modification time")?
                        .duration_since(std::time::UNIX_EPOCH)
                        .context("queued file modification time is before epoch")?
                        .as_millis()
                        .min(i64::MAX as u128) as i64;
                    if metadata.len() != expected_size || actual_modified_at != expected_modified_at
                    {
                        return Err(anyhow::anyhow!(
                            "queued file attachment changed since it was queued: {}",
                            file.file_path.display()
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn file_fingerprint(path: &std::path::Path) -> anyhow::Result<(u64, i64)> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("reading queued file metadata: {}", path.display()))?;
    if !metadata.is_file() {
        return Err(anyhow::anyhow!("queued attachment is not a regular file"));
    }
    let modified_at = metadata
        .modified()
        .context("reading queued file modification time")?
        .duration_since(std::time::UNIX_EPOCH)
        .context("queued file modification time is before epoch")?
        .as_millis()
        .min(i64::MAX as u128) as i64;
    Ok((metadata.len(), modified_at))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "queued_query_tests.rs"]
mod tests;
