//! Process-local ownership and delivery state for local child agents.
//!
//! The registry is deliberately independent from the hosted orchestration
//! service.  A run is identified by the local UUID assigned when its
//! conversation is created, and all controller/message operations resolve
//! through that UUID.  Conversation persistence remains the source of truth
//! for historical topology; this registry only owns state that is live in the
//! current process.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warp_cli::agent::Harness;
use warpui::{Entity, EntityId, SingletonEntity};

use crate::ai::agent::conversation::AIConversationId;

/// Maximum number of children accepted by one local `RunAgents` call.
pub const MAX_LOCAL_CHILD_FANOUT: usize = 8;
/// Maximum local parent -> child nesting depth.
pub const MAX_LOCAL_AGENT_DEPTH: usize = 4;
/// Maximum number of live local children in one process.
pub const MAX_LIVE_LOCAL_CHILDREN: usize = 16;
/// Maximum number of messages retained for one busy controller.
pub const MAX_PENDING_LOCAL_MESSAGES: usize = 16;

/// Explicit local limits.  Keeping these in one value makes preflight
/// deterministic and lets focused tests exercise smaller limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalAgentLimits {
    pub max_fanout: usize,
    pub max_depth: usize,
    pub max_live_children: usize,
    pub max_pending_messages: usize,
}

impl Default for LocalAgentLimits {
    fn default() -> Self {
        Self {
            max_fanout: MAX_LOCAL_CHILD_FANOUT,
            max_depth: MAX_LOCAL_AGENT_DEPTH,
            max_live_children: MAX_LIVE_LOCAL_CHILDREN,
            max_pending_messages: MAX_PENDING_LOCAL_MESSAGES,
        }
    }
}

/// Lifecycle visible to local topology/status consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalAgentStatus {
    Starting,
    Idle,
    Running,
    Blocked,
    Succeeded,
    Failed,
    Cancelled,
    /// A persisted run which is known locally but has no live controller after
    /// restart.  It must not be treated as an executable process.
    Stopped,
}

impl LocalAgentStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Stopped
        )
    }

    pub fn is_live(self) -> bool {
        !self.is_terminal()
    }
}

/// A process-local cancellation signal owned by the run's controller.
#[derive(Clone, Default)]
pub struct LocalAgentCancellationHandle(Arc<AtomicBool>);

impl fmt::Debug for LocalAgentCancellationHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalAgentCancellationHandle")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl LocalAgentCancellationHandle {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// The immutable request envelope used while registering a local child.
#[derive(Debug, Clone)]
pub struct LocalAgentRegistration {
    pub run_id: Option<String>,
    pub conversation_id: AIConversationId,
    pub terminal_surface_id: Option<EntityId>,
    pub pane_id: Option<EntityId>,
    pub parent_run_id: Option<String>,
    pub name: String,
    pub prompt: String,
    pub harness: Harness,
    pub model_id: Option<String>,
    pub model_available: bool,
    pub tools_available: bool,
    pub working_directory: Option<PathBuf>,
    pub action_id: Option<String>,
    pub request_id: Option<String>,
    /// Number of siblings requested by a fan-out operation.  A single child
    /// registration uses the default value of one.
    pub requested_fanout: usize,
    pub controller_owner: Option<String>,
}

impl LocalAgentRegistration {
    pub fn new(
        conversation_id: AIConversationId,
        parent_run_id: Option<String>,
        name: impl Into<String>,
        prompt: impl Into<String>,
        harness: Harness,
    ) -> Self {
        Self {
            run_id: None,
            conversation_id,
            terminal_surface_id: None,
            pane_id: None,
            parent_run_id,
            name: name.into(),
            prompt: prompt.into(),
            harness,
            model_id: None,
            model_available: true,
            tools_available: true,
            working_directory: None,
            action_id: None,
            request_id: None,
            requested_fanout: 1,
            controller_owner: None,
        }
    }
}

/// A historical run used to rebuild topology after restart.  Restored runs
/// are always inserted as [`LocalAgentStatus::Stopped`].
#[derive(Debug, Clone)]
pub struct RestoredLocalAgent {
    pub run_id: String,
    pub conversation_id: AIConversationId,
    pub terminal_surface_id: Option<EntityId>,
    pub pane_id: Option<EntityId>,
    pub parent_run_id: Option<String>,
    pub name: String,
    pub harness: Harness,
}

/// A snapshot returned to callers after registration or lookup.
#[derive(Debug, Clone)]
pub struct LocalAgentRun {
    pub run_id: String,
    pub conversation_id: AIConversationId,
    pub terminal_surface_id: Option<EntityId>,
    pub pane_id: Option<EntityId>,
    pub parent_run_id: Option<String>,
    pub child_run_ids: Vec<String>,
    pub name: String,
    pub harness: Harness,
    pub status: LocalAgentStatus,
    pub depth: usize,
    pub controller_owner: Option<String>,
    pub cancellation: LocalAgentCancellationHandle,
}

impl LocalAgentRun {
    pub fn is_controller_ready(&self) -> bool {
        self.controller_owner.is_some() && self.status.is_live()
    }
}

/// Result of an idempotent registration.  A duplicate action/request key
/// returns the original run and `created == false`; it never starts a second
/// process or conversation.
#[derive(Debug, Clone)]
pub struct LocalAgentRegistrationOutcome {
    pub run: LocalAgentRun,
    pub created: bool,
}

/// Preflight input shared by StartAgent and RunAgents.
#[derive(Debug, Clone)]
pub struct LocalAgentPreflight {
    pub parent_run_id: Option<String>,
    pub name: String,
    pub prompt: String,
    pub harness: Harness,
    pub model_available: bool,
    pub tools_available: bool,
    pub working_directory: Option<PathBuf>,
    pub requested_fanout: usize,
}

impl Default for LocalAgentPreflight {
    fn default() -> Self {
        Self {
            parent_run_id: None,
            name: String::new(),
            prompt: String::new(),
            harness: Harness::Oz,
            model_available: true,
            tools_available: true,
            working_directory: None,
            requested_fanout: 1,
        }
    }
}

/// A process-local immutable message envelope.  The envelope is never reused
/// for retries: acceptance is acknowledged only when it enters the recipient
/// controller's bounded queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAgentMessageEnvelope {
    pub message_id: String,
    pub sender_run_id: String,
    pub recipient_run_id: String,
    pub subject: String,
    pub body: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAgentMessageAck {
    pub message_id: String,
    pub sequence: u64,
    pub recipient_run_id: String,
    pub wake_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalAgentRegistryError {
    EmptyName,
    EmptyPrompt,
    EmptyMessage,
    EmptySubject,
    UnknownRun(String),
    HistoricalRun(String),
    ControllerRequired(String),
    ControllerOwnedByAnotherRun(String),
    DuplicateSiblingName(String),
    FanoutLimit { requested: usize, limit: usize },
    DepthLimit { requested: usize, limit: usize },
    ConcurrentLimit { limit: usize },
    QueueFull { run_id: String, limit: usize },
    ModelUnavailable,
    ToolsUnavailable,
    WorkingDirectoryUnavailable(PathBuf),
    UnsupportedHarness(Harness),
    IdempotencyConflict(String),
    DuplicateConversation(String),
    InvalidRunId,
    InvalidTopology(String),
}

impl fmt::Display for LocalAgentRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "local child agent name is required"),
            Self::EmptyPrompt => write!(f, "local child agent prompt is required"),
            Self::EmptyMessage => write!(f, "local agent message body is required"),
            Self::EmptySubject => write!(f, "local agent message subject is required"),
            Self::UnknownRun(run_id) => write!(f, "unknown local run id `{run_id}`"),
            Self::HistoricalRun(run_id) => write!(f, "local run `{run_id}` is stopped"),
            Self::ControllerRequired(run_id) => {
                write!(f, "local run `{run_id}` has no live controller")
            }
            Self::ControllerOwnedByAnotherRun(run_id) => {
                write!(f, "local run `{run_id}` is owned by another controller")
            }
            Self::DuplicateSiblingName(name) => {
                write!(f, "local child sibling name `{name}` is already in use")
            }
            Self::FanoutLimit { requested, limit } => {
                write!(f, "local child fan-out {requested} exceeds limit {limit}")
            }
            Self::DepthLimit { requested, limit } => {
                write!(f, "local child nesting depth {requested} exceeds limit {limit}")
            }
            Self::ConcurrentLimit { limit } => {
                write!(f, "local live child limit {limit} has been reached")
            }
            Self::QueueFull { run_id, limit } => {
                write!(f, "local run `{run_id}` message queue is full (limit {limit})")
            }
            Self::ModelUnavailable => write!(f, "selected local child model is unavailable"),
            Self::ToolsUnavailable => write!(f, "selected local child model has no tools"),
            Self::WorkingDirectoryUnavailable(path) => {
                write!(f, "local child working directory is unavailable: {}", path.display())
            }
            Self::UnsupportedHarness(harness) => {
                write!(f, "local child harness {} is unavailable", harness.display_name())
            }
            Self::IdempotencyConflict(key) => {
                write!(f, "local action/request id `{key}` was already used for another run")
            }
            Self::DuplicateConversation(conversation_id) => {
                write!(f, "conversation `{conversation_id}` is already registered")
            }
            Self::InvalidRunId => write!(f, "local run id must be non-empty"),
            Self::InvalidTopology(message) => write!(f, "invalid local agent topology: {message}"),
        }
    }
}

impl std::error::Error for LocalAgentRegistryError {}

/// A process-local registry for live child controllers and their bounded
/// message queues.
#[derive(Debug)]
pub struct LocalAgentRegistry {
    runs: HashMap<String, LocalAgentRun>,
    conversation_to_run: HashMap<AIConversationId, String>,
    idempotency: HashMap<String, String>,
    pending_messages: HashMap<String, VecDeque<LocalAgentMessageEnvelope>>,
    next_message_sequence: u64,
    limits: LocalAgentLimits,
}

/// The registry currently emits no network-facing events.  The event type is
/// kept explicit so controllers can subscribe to local wakeups as the
/// process-local input path is wired into the agent controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalAgentRegistryEvent {
    MessageAccepted,
    StatusChanged,
}

impl Entity for LocalAgentRegistry {
    type Event = LocalAgentRegistryEvent;
}

impl SingletonEntity for LocalAgentRegistry {}

impl Default for LocalAgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalAgentRegistry {
    pub fn new() -> Self {
        Self::with_limits(LocalAgentLimits::default())
    }

    pub fn with_limits(limits: LocalAgentLimits) -> Self {
        Self {
            runs: HashMap::new(),
            conversation_to_run: HashMap::new(),
            idempotency: HashMap::new(),
            pending_messages: HashMap::new(),
            next_message_sequence: 0,
            limits,
        }
    }

    pub fn limits(&self) -> LocalAgentLimits {
        self.limits
    }

    /// Allocates the stable local run identifier before a child process is
    /// launched.  UUIDs keep persisted IDs opaque and collision-resistant.
    pub fn allocate_run_id() -> String {
        Uuid::new_v4().to_string()
    }

    pub fn len(&self) -> usize {
        self.runs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    pub fn get(&self, run_id: &str) -> Option<&LocalAgentRun> {
        self.runs.get(run_id)
    }

    pub fn run_id_for_conversation(&self, conversation_id: AIConversationId) -> Option<&str> {
        self.conversation_to_run
            .get(&conversation_id)
            .map(String::as_str)
    }

    pub fn child_run_ids(&self, parent_run_id: &str) -> &[String] {
        self.runs
            .get(parent_run_id)
            .map(|run| run.child_run_ids.as_slice())
            .unwrap_or_default()
    }

    pub fn preflight(
        &self,
        request: &LocalAgentPreflight,
    ) -> Result<usize, LocalAgentRegistryError> {
        if request.name.trim().is_empty() {
            return Err(LocalAgentRegistryError::EmptyName);
        }
        if request.prompt.trim().is_empty() {
            return Err(LocalAgentRegistryError::EmptyPrompt);
        }
        if request.requested_fanout == 0 || request.requested_fanout > self.limits.max_fanout {
            return Err(LocalAgentRegistryError::FanoutLimit {
                requested: request.requested_fanout,
                limit: self.limits.max_fanout,
            });
        }
        if !request.model_available {
            return Err(LocalAgentRegistryError::ModelUnavailable);
        }
        if !request.tools_available {
            return Err(LocalAgentRegistryError::ToolsUnavailable);
        }
        if matches!(request.harness, Harness::Unknown | Harness::Gemini) {
            return Err(LocalAgentRegistryError::UnsupportedHarness(request.harness));
        }
        if let Some(path) = request.working_directory.as_ref()
            && !path.is_dir()
        {
            return Err(LocalAgentRegistryError::WorkingDirectoryUnavailable(path.clone()));
        }

        let depth = if let Some(parent_run_id) = request.parent_run_id.as_deref() {
            let parent = self
                .runs
                .get(parent_run_id)
                .ok_or_else(|| LocalAgentRegistryError::UnknownRun(parent_run_id.to_string()))?;
            if !parent.status.is_live() || parent.controller_owner.is_none() {
                return Err(LocalAgentRegistryError::HistoricalRun(parent_run_id.to_string()));
            }
            let sibling_name_in_use = parent.child_run_ids.iter().any(|child_run_id| {
                self.runs
                    .get(child_run_id)
                    .is_some_and(|child| child.name == request.name)
            });
            if sibling_name_in_use {
                return Err(LocalAgentRegistryError::DuplicateSiblingName(
                    request.name.clone(),
                ));
            }
            let depth = parent.depth.saturating_add(1);
            if depth > self.limits.max_depth {
                return Err(LocalAgentRegistryError::DepthLimit {
                    requested: depth,
                    limit: self.limits.max_depth,
                });
            }
            if parent.child_run_ids.len().saturating_add(request.requested_fanout)
                > self.limits.max_fanout
            {
                return Err(LocalAgentRegistryError::FanoutLimit {
                    requested: parent.child_run_ids.len() + request.requested_fanout,
                    limit: self.limits.max_fanout,
                });
            }
            depth
        } else {
            0
        };

        let live_children = self
            .runs
            .values()
            .filter(|run| run.parent_run_id.is_some() && run.status.is_live())
            .count();
        if live_children.saturating_add(request.requested_fanout) > self.limits.max_live_children {
            return Err(LocalAgentRegistryError::ConcurrentLimit {
                limit: self.limits.max_live_children,
            });
        }
        Ok(depth)
    }

    /// Registers a local child synchronously.  Callers should persist the
    /// returned `run_id` together with the conversation topology before
    /// launching the executor.
    pub fn register_child(
        &mut self,
        request: LocalAgentRegistration,
    ) -> Result<LocalAgentRegistrationOutcome, LocalAgentRegistryError> {
        let idempotency_keys = idempotency_keys(&request);
        for key in &idempotency_keys {
            if let Some(existing_run_id) = self.idempotency.get(key) {
                let existing = self
                    .runs
                    .get(existing_run_id)
                    .expect("idempotency index must point to a run");
                if existing.conversation_id == request.conversation_id
                    && existing.name == request.name
                    && existing.parent_run_id == request.parent_run_id
                {
                    return Ok(LocalAgentRegistrationOutcome {
                        run: existing.clone(),
                        created: false,
                    });
                }
                return Err(LocalAgentRegistryError::IdempotencyConflict(key.clone()));
            }
        }

        if self
            .conversation_to_run
            .contains_key(&request.conversation_id)
        {
            return Err(LocalAgentRegistryError::DuplicateConversation(
                request.conversation_id.to_string(),
            ));
        }

        let preflight = LocalAgentPreflight {
            parent_run_id: request.parent_run_id.clone(),
            name: request.name.clone(),
            prompt: request.prompt.clone(),
            harness: request.harness,
            model_available: request.model_available,
            tools_available: request.tools_available,
            working_directory: request.working_directory.clone(),
            requested_fanout: request.requested_fanout,
        };
        let depth = self.preflight(&preflight)?;
        let run_id = request
            .run_id
            .filter(|run_id| !run_id.trim().is_empty())
            .unwrap_or_else(Self::allocate_run_id);
        if self.runs.contains_key(&run_id) {
            return Err(LocalAgentRegistryError::DuplicateConversation(run_id));
        }

        let run = LocalAgentRun {
            run_id: run_id.clone(),
            conversation_id: request.conversation_id,
            terminal_surface_id: request.terminal_surface_id,
            pane_id: request.pane_id,
            parent_run_id: request.parent_run_id.clone(),
            child_run_ids: Vec::new(),
            name: request.name,
            harness: request.harness,
            status: LocalAgentStatus::Starting,
            depth,
            controller_owner: request.controller_owner,
            cancellation: LocalAgentCancellationHandle::default(),
        };
        if let Some(parent_run_id) = request.parent_run_id.as_deref() {
            self.runs
                .get_mut(parent_run_id)
                .expect("preflight verified parent")
                .child_run_ids
                .push(run_id.clone());
        }
        self.conversation_to_run
            .insert(request.conversation_id, run_id.clone());
        for key in idempotency_keys {
            self.idempotency.insert(key, run_id.clone());
        }
        self.pending_messages.entry(run_id.clone()).or_default();
        self.runs.insert(run_id, run.clone());
        Ok(LocalAgentRegistrationOutcome { run, created: true })
    }

    /// Registers a conversation whose local run ID was already persisted by
    /// the history model.  This is used at pane/controller attachment time;
    /// the synchronous allocation and validation boundary remains
    /// [`Self::register_child`].  Re-attaching the same conversation is
    /// idempotent and does not create a second controller record.
    pub fn register_existing(
        &mut self,
        run_id: String,
        conversation_id: AIConversationId,
        terminal_surface_id: Option<EntityId>,
        pane_id: Option<EntityId>,
        parent_run_id: Option<String>,
        name: String,
        harness: Harness,
        controller_owner: Option<String>,
        status: LocalAgentStatus,
    ) -> Result<LocalAgentRun, LocalAgentRegistryError> {
        if run_id.trim().is_empty() {
            return Err(LocalAgentRegistryError::InvalidRunId);
        }
        if let Some(existing_run_id) = self.conversation_to_run.get(&conversation_id) {
            let existing = self
                .runs
                .get(existing_run_id)
                .expect("conversation index must point to a run");
            if existing.run_id == run_id {
                return Ok(existing.clone());
            }
            return Err(LocalAgentRegistryError::DuplicateConversation(
                conversation_id.to_string(),
            ));
        }
        if self.runs.contains_key(&run_id) {
            return Err(LocalAgentRegistryError::DuplicateConversation(run_id));
        }
        let depth = parent_run_id
            .as_deref()
            .and_then(|parent| self.runs.get(parent).map(|run| run.depth + 1))
            .unwrap_or(0);
        let run = LocalAgentRun {
            run_id: run_id.clone(),
            conversation_id,
            terminal_surface_id,
            pane_id,
            parent_run_id: parent_run_id.clone(),
            child_run_ids: Vec::new(),
            name,
            harness,
            status,
            depth,
            controller_owner,
            cancellation: LocalAgentCancellationHandle::default(),
        };
        if let Some(parent_run_id) = parent_run_id.as_deref()
            && let Some(parent) = self.runs.get_mut(parent_run_id)
            && !parent.child_run_ids.contains(&run_id)
        {
            parent.child_run_ids.push(run_id.clone());
        }
        self.conversation_to_run.insert(conversation_id, run_id.clone());
        self.pending_messages.entry(run_id.clone()).or_default();
        self.runs.insert(run_id, run.clone());
        Ok(run)
    }

    /// Rehydrates persisted topology without claiming that a process survived
    /// restart.  Parent links are kept when available; malformed cycles are
    /// rejected and the caller can still show the conversation historically.
    pub fn restore_stopped(
        &mut self,
        restored: RestoredLocalAgent,
    ) -> Result<LocalAgentRun, LocalAgentRegistryError> {
        if restored.run_id.trim().is_empty() {
            return Err(LocalAgentRegistryError::InvalidRunId);
        }
        if self.runs.contains_key(&restored.run_id) {
            return Err(LocalAgentRegistryError::DuplicateConversation(
                restored.run_id,
            ));
        }
        if self
            .conversation_to_run
            .contains_key(&restored.conversation_id)
        {
            return Err(LocalAgentRegistryError::DuplicateConversation(
                restored.conversation_id.to_string(),
            ));
        }
        if let Some(parent_run_id) = restored.parent_run_id.as_deref()
            && parent_run_id == restored.run_id
        {
            return Err(LocalAgentRegistryError::InvalidTopology(
                "a run cannot be its own parent".to_string(),
            ));
        }
        let depth = restored
            .parent_run_id
            .as_deref()
            .and_then(|parent| self.runs.get(parent).map(|run| run.depth + 1))
            .unwrap_or(0);
        let run = LocalAgentRun {
            run_id: restored.run_id.clone(),
            conversation_id: restored.conversation_id,
            terminal_surface_id: restored.terminal_surface_id,
            pane_id: restored.pane_id,
            parent_run_id: restored.parent_run_id.clone(),
            child_run_ids: Vec::new(),
            name: restored.name,
            harness: restored.harness,
            status: LocalAgentStatus::Stopped,
            depth,
            controller_owner: None,
            cancellation: LocalAgentCancellationHandle::default(),
        };
        if let Some(parent_run_id) = restored.parent_run_id.as_deref()
            && let Some(parent) = self.runs.get_mut(parent_run_id)
        {
            parent.child_run_ids.push(restored.run_id.clone());
        }
        self.conversation_to_run
            .insert(restored.conversation_id, restored.run_id.clone());
        self.pending_messages
            .entry(restored.run_id.clone())
            .or_default();
        self.runs.insert(restored.run_id, run.clone());
        Ok(run)
    }

    /// Reclaims a stopped run only for a new live same-process controller.
    pub fn claim_controller(
        &mut self,
        run_id: &str,
        controller_owner: impl Into<String>,
    ) -> Result<LocalAgentCancellationHandle, LocalAgentRegistryError> {
        let controller_owner = controller_owner.into();
        if controller_owner.trim().is_empty() {
            return Err(LocalAgentRegistryError::ControllerRequired(run_id.to_string()));
        }
        let run = self
            .runs
            .get_mut(run_id)
            .ok_or_else(|| LocalAgentRegistryError::UnknownRun(run_id.to_string()))?;
        if let Some(existing_owner) = run.controller_owner.as_deref()
            && existing_owner != controller_owner
        {
            return Err(LocalAgentRegistryError::ControllerOwnedByAnotherRun(
                run_id.to_string(),
            ));
        }
        run.controller_owner = Some(controller_owner);
        if run.status == LocalAgentStatus::Stopped {
            run.status = LocalAgentStatus::Idle;
        }
        Ok(run.cancellation.clone())
    }

    pub fn register_controller(
        &mut self,
        run_id: &str,
        controller_owner: impl Into<String>,
    ) -> Result<LocalAgentCancellationHandle, LocalAgentRegistryError> {
        self.claim_controller(run_id, controller_owner)
    }

    pub fn release_controller(
        &mut self,
        run_id: &str,
        controller_owner: &str,
    ) -> Result<(), LocalAgentRegistryError> {
        let run = self
            .runs
            .get_mut(run_id)
            .ok_or_else(|| LocalAgentRegistryError::UnknownRun(run_id.to_string()))?;
        ensure_owner(run, controller_owner)?;
        run.controller_owner = None;
        if !run.status.is_terminal() {
            run.status = LocalAgentStatus::Stopped;
        }
        Ok(())
    }

    pub fn set_status(
        &mut self,
        run_id: &str,
        controller_owner: &str,
        status: LocalAgentStatus,
    ) -> Result<(), LocalAgentRegistryError> {
        let run = self
            .runs
            .get_mut(run_id)
            .ok_or_else(|| LocalAgentRegistryError::UnknownRun(run_id.to_string()))?;
        ensure_owner(run, controller_owner)?;
        run.status = status;
        Ok(())
    }

    pub fn cancel(
        &mut self,
        run_id: &str,
        controller_owner: &str,
    ) -> Result<LocalAgentCancellationHandle, LocalAgentRegistryError> {
        let run = self
            .runs
            .get_mut(run_id)
            .ok_or_else(|| LocalAgentRegistryError::UnknownRun(run_id.to_string()))?;
        ensure_owner(run, controller_owner)?;
        run.cancellation.cancel();
        run.status = LocalAgentStatus::Cancelled;
        if let Some(queue) = self.pending_messages.get_mut(run_id) {
            queue.clear();
        }
        Ok(run.cancellation.clone())
    }

    pub fn is_ready(&self, run_id: &str) -> bool {
        self.runs
            .get(run_id)
            .is_some_and(LocalAgentRun::is_controller_ready)
    }

    /// Enqueues one message and acknowledges only after the target's local
    /// controller accepts it.  There is no server-token or model-echo path.
    pub fn send_message(
        &mut self,
        sender_run_id: &str,
        recipient_run_id: &str,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> Result<LocalAgentMessageAck, LocalAgentRegistryError> {
        self.send_message_owned(sender_run_id, None, recipient_run_id, subject, body)
    }

    pub fn send_message_owned(
        &mut self,
        sender_run_id: &str,
        sender_owner: Option<&str>,
        recipient_run_id: &str,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> Result<LocalAgentMessageAck, LocalAgentRegistryError> {
        let subject = subject.into();
        let body = body.into();
        if subject.trim().is_empty() {
            return Err(LocalAgentRegistryError::EmptySubject);
        }
        if body.trim().is_empty() {
            return Err(LocalAgentRegistryError::EmptyMessage);
        }
        let sender = self
            .runs
            .get(sender_run_id)
            .ok_or_else(|| LocalAgentRegistryError::UnknownRun(sender_run_id.to_string()))?;
        if !sender.status.is_live() {
            return Err(LocalAgentRegistryError::HistoricalRun(
                sender_run_id.to_string(),
            ));
        }
        if sender.controller_owner.is_none() {
            return Err(LocalAgentRegistryError::ControllerRequired(
                sender_run_id.to_string(),
            ));
        }
        if let Some(owner) = sender_owner {
            ensure_owner(sender, owner)?;
        }
        let recipient = self
            .runs
            .get(recipient_run_id)
            .ok_or_else(|| LocalAgentRegistryError::UnknownRun(recipient_run_id.to_string()))?;
        if !recipient.status.is_live() {
            return Err(LocalAgentRegistryError::HistoricalRun(
                recipient_run_id.to_string(),
            ));
        }
        if recipient.controller_owner.is_none() {
            return Err(LocalAgentRegistryError::ControllerRequired(
                recipient_run_id.to_string(),
            ));
        }
        let queue = self
            .pending_messages
            .entry(recipient_run_id.to_string())
            .or_default();
        if queue.len() >= self.limits.max_pending_messages {
            return Err(LocalAgentRegistryError::QueueFull {
                run_id: recipient_run_id.to_string(),
                limit: self.limits.max_pending_messages,
            });
        }
        let sequence = self.next_message_sequence;
        self.next_message_sequence = self.next_message_sequence.wrapping_add(1);
        let message_id = Uuid::new_v4().to_string();
        queue.push_back(LocalAgentMessageEnvelope {
            message_id: message_id.clone(),
            sender_run_id: sender_run_id.to_string(),
            recipient_run_id: recipient_run_id.to_string(),
            subject,
            body,
            sequence,
        });
        Ok(LocalAgentMessageAck {
            message_id,
            sequence,
            recipient_run_id: recipient_run_id.to_string(),
            wake_requested: recipient.status == LocalAgentStatus::Idle,
        })
    }

    pub fn take_pending_messages(
        &mut self,
        run_id: &str,
        controller_owner: &str,
    ) -> Result<Vec<LocalAgentMessageEnvelope>, LocalAgentRegistryError> {
        let run = self
            .runs
            .get_mut(run_id)
            .ok_or_else(|| LocalAgentRegistryError::UnknownRun(run_id.to_string()))?;
        ensure_owner(run, controller_owner)?;
        if !run.status.is_live() {
            return Err(LocalAgentRegistryError::HistoricalRun(run_id.to_string()));
        }
        Ok(self
            .pending_messages
            .get_mut(run_id)
            .map(|queue| queue.drain(..).collect())
            .unwrap_or_default())
    }

    pub fn pending_message_count(&self, run_id: &str) -> usize {
        self.pending_messages
            .get(run_id)
            .map(VecDeque::len)
            .unwrap_or_default()
    }
}

fn idempotency_keys(request: &LocalAgentRegistration) -> Vec<String> {
    request
        .action_id
        .iter()
        .chain(request.request_id.iter())
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn ensure_owner(
    run: &LocalAgentRun,
    controller_owner: &str,
) -> Result<(), LocalAgentRegistryError> {
    match run.controller_owner.as_deref() {
        Some(owner) if owner == controller_owner => Ok(()),
        Some(_) => Err(LocalAgentRegistryError::ControllerOwnedByAnotherRun(
            run.run_id.clone(),
        )),
        None => Err(LocalAgentRegistryError::ControllerRequired(run.run_id.clone())),
    }
}

#[cfg(test)]
#[path = "local_agent_registry_tests.rs"]
mod tests;
