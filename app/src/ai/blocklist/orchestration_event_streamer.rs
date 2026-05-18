use super::history_model::{BlocklistAIHistoryEvent, BlocklistAIHistoryModel};
#[cfg(test)]
use super::orchestration_events::{
    build_lifecycle_event, LifecycleEventDetailPayload, LifecycleEventDetailStage,
    OrchestrationEventService, PendingEvent, PendingEventDetail,
};
#[cfg(test)]
use crate::ai::agent::ReceivedMessageInput;
use crate::ai::agent::{
    conversation::{AIAgentHarness, AIConversationId, ConversationStatus},
    AIAgentExchangeId, AIAgentOutputMessageType,
};
#[cfg(test)]
use crate::server::server_api::ai::AIClient;
#[cfg(test)]
use crate::server::server_api::ai::AgentRunEvent;
#[cfg(test)]
use crate::server::server_api::ServerApi;
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use uuid::Uuid;
use warp_cli::agent::Harness;
use warp_core::features::FeatureFlag;
#[cfg(test)]
use warp_multi_agent_api as api;
use warpui::{
    Entity, EntityId, GetSingletonModelHandle, ModelContext, SingletonEntity, UpdateModel,
};

/// Cap killed-run tombstones while keeping normal sessions well below the limit.
const MAX_KILLED_RUN_IDS: usize = 1024;

/// All per-conversation streaming state. Created lazily on first access
/// (via `entry().or_default()`) and dropped when the conversation is
/// removed from the history model.
#[derive(Default)]
struct ConversationStreamState {
    /// Run IDs the SSE filter watches for this conversation. When the
    /// conversation has any orchestration role, this contains its own
    /// `self_run_id` (its inbox — used both for parent→child traffic on
    /// children and child→parent traffic on parents); when it acts as a
    /// parent it additionally contains each registered child run_id.
    watched_run_ids: HashSet<String>,
    /// Last fully handled event sequence number. 0 means "no events
    /// processed yet".
    event_cursor: i64,
    /// Message IDs awaiting server-side `mark_delivered` confirmation,
    /// triggered when the recipient streams a `MessagesReceivedFromAgents`
    /// chunk through `BlocklistAIHistoryEvent::UpdatedStreamingExchange`.
    pending_message_ids: Vec<String>,
    /// Local consumers (terminal pane id for an open agent view, driver
    /// model id for `agent_sdk`) that need events delivered to this
    /// conversation.
    consumers: HashSet<EntityId>,
    /// Execution harness from the task row, when available. Local harness
    /// child conversations are created before they have server conversation
    /// metadata, so this lets us recognize dormant local Claude children
    /// without relying on `ServerAIConversationMetadata`.
    harness: Option<Harness>,
}

/// Async network coordinator for v2 orchestration event delivery via SSE.
///
/// Holds at most one long-lived SSE connection per conversation. The
/// streamer opens a connection only when a conversation has both an
/// active local consumer (an open agent view, or an `agent_sdk` driver
/// in CLI / cloud worker processes) and at least one orchestration role
/// in this process — being a child, or having registered child run_ids.
/// Without a local consumer the events would have nowhere to go, so the
/// connection stays closed and the cursor is used to backfill once a
/// consumer registers.
pub struct OrchestrationEventStreamer {
    /// Per-conversation streaming state.
    streams: HashMap<AIConversationId, ConversationStreamState>,
    /// Run IDs killed locally; kept briefly to drop late server events.
    killed_run_ids: HashSet<String>,
    killed_run_id_order: VecDeque<String>,
}

impl OrchestrationEventStreamer {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let history_model = BlocklistAIHistoryModel::handle(ctx);
        ctx.subscribe_to_model(&history_model, |me, event, ctx| {
            me.handle_history_event(event, ctx);
        });
        Self {
            streams: HashMap::new(),
            killed_run_ids: HashSet::new(),
            killed_run_id_order: VecDeque::new(),
        }
    }

    /// Constructs a streamer wired to the supplied (mock) clients instead of
    /// looking them up via the runtime provider. Lets unit tests inject a
    /// `MockAIClient` while still subscribing to `BlocklistAIHistoryModel`.
    #[cfg(test)]
    pub(super) fn new_with_clients_for_test(
        ai_client: Arc<dyn AIClient>,
        server_api: Arc<ServerApi>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let history_model = BlocklistAIHistoryModel::handle(ctx);
        ctx.subscribe_to_model(&history_model, |me, event, ctx| {
            me.handle_history_event(event, ctx);
        });
        let _ = (ai_client, server_api);
        Self {
            streams: HashMap::new(),
            killed_run_ids: HashSet::new(),
            killed_run_id_order: VecDeque::new(),
        }
    }

    // ---- Public consumer registry API ---------------------------------

    /// Tombstone a killed run so late SSE events cannot resurrect it.
    pub fn mark_conversation_killed(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(run_id) = self.self_run_id(conversation_id, ctx) else {
            log::info!("mark_conversation_killed: conversation {conversation_id:?} has no run_id");
            return;
        };
        log::info!(
            "Marking orchestration run as killed: conversation_id={conversation_id:?} run_id={run_id}"
        );
        self.remember_killed_run_id(run_id);
    }

    fn remember_killed_run_id(&mut self, run_id: String) {
        if self.killed_run_ids.insert(run_id.clone()) {
            self.killed_run_id_order.push_back(run_id);
        }
        while self.killed_run_ids.len() > MAX_KILLED_RUN_IDS {
            let Some(evicted_run_id) = self.killed_run_id_order.pop_front() else {
                break;
            };
            self.killed_run_ids.remove(&evicted_run_id);
        }
    }

    /// Register a consumer for a conversation. Re-evaluates eligibility
    /// and opens the SSE connection if the conversation is newly
    /// eligible. Idempotent: re-registering an existing consumer is a
    /// no-op for the registry, but still triggers eligibility
    /// re-evaluation (which is itself idempotent).
    pub fn register_consumer(
        &mut self,
        conversation_id: AIConversationId,
        consumer_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        let stream = self.streams.entry(conversation_id).or_default();
        let inserted = stream.consumers.insert(consumer_id);
        if inserted {
            log::info!(
                "register_consumer for {conversation_id:?}: {consumer_id:?} \
                 (total={})",
                stream.consumers.len()
            );
        }
        // If the server-token event fired before this registration, pick
        // up the now-available child role here.
        self.ensure_self_run_id_watched(conversation_id, ctx);
        self.reevaluate_eligibility(conversation_id, ctx);
    }

    /// Unregister a consumer for a conversation. Re-evaluates eligibility
    /// and tears down the SSE connection if the conversation is no longer
    /// eligible (and the conversation is not also in the child role).
    pub fn unregister_consumer(
        &mut self,
        conversation_id: AIConversationId,
        consumer_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        let removed = self
            .streams
            .get_mut(&conversation_id)
            .map(|s| s.consumers.remove(&consumer_id))
            .unwrap_or(false);
        if removed {
            let remaining = self
                .streams
                .get(&conversation_id)
                .map(|s| s.consumers.len())
                .unwrap_or(0);
            log::info!(
                "unregister_consumer for {conversation_id:?}: {consumer_id:?} \
                 (remaining={remaining})"
            );
        }
        self.reevaluate_eligibility(conversation_id, ctx);
    }

    /// Registers a run_id to watch for events on a conversation. Called
    /// by the start_agent executor for child run_ids and by the
    /// streamer's own helpers for self_run_id (child / parent inbox).
    pub fn register_watched_run_id(
        &mut self,
        conversation_id: AIConversationId,
        run_id: String,
        ctx: &mut ModelContext<Self>,
    ) {
        let inserted = self
            .streams
            .entry(conversation_id)
            .or_default()
            .watched_run_ids
            .insert(run_id);
        // Adding the first child flips the conversation into the parent
        // role; ensure self_run_id is also watched so child→parent
        // messages match the SSE filter (without it the parent only sees
        // child lifecycle events).
        let self_inserted = self.ensure_self_run_id_watched(conversation_id, ctx);
        if inserted || self_inserted {
            self.reevaluate_eligibility(conversation_id, ctx);
        }
    }

    // ---- Event subscriptions from BlocklistAIHistoryModel -------------

    fn handle_history_event(
        &mut self,
        event: &BlocklistAIHistoryEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        match event {
            BlocklistAIHistoryEvent::ConversationServerTokenAssigned {
                conversation_id, ..
            } => self.on_server_token_assigned(*conversation_id, ctx),
            BlocklistAIHistoryEvent::UpdatedStreamingExchange {
                conversation_id,
                exchange_id,
                ..
            } => self.on_streaming_exchange_updated(*conversation_id, *exchange_id, ctx),
            BlocklistAIHistoryEvent::RemoveConversation {
                conversation_id,
                run_id,
                ..
            }
            | BlocklistAIHistoryEvent::DeletedConversation {
                conversation_id,
                run_id,
                ..
            } => {
                self.on_conversation_removed(*conversation_id, run_id.clone(), ctx);
            }
            BlocklistAIHistoryEvent::RestoredConversations {
                conversation_ids, ..
            } => {
                self.on_restored_conversations(conversation_ids.clone(), ctx);
            }
            BlocklistAIHistoryEvent::UpdatedConversationStatus {
                conversation_id, ..
            }
            | BlocklistAIHistoryEvent::UpdatedConversationMetadata {
                conversation_id, ..
            } => self.reevaluate_eligibility(*conversation_id, ctx),
            BlocklistAIHistoryEvent::StartedNewConversation { .. }
            | BlocklistAIHistoryEvent::CreatedSubtask { .. }
            | BlocklistAIHistoryEvent::UpgradedTask { .. }
            | BlocklistAIHistoryEvent::AppendedExchange { .. }
            | BlocklistAIHistoryEvent::ReassignedExchange { .. }
            | BlocklistAIHistoryEvent::SetActiveConversation { .. }
            | BlocklistAIHistoryEvent::ClearedActiveConversation { .. }
            | BlocklistAIHistoryEvent::ClearedConversationsInTerminalView { .. }
            | BlocklistAIHistoryEvent::UpdatedTodoList { .. }
            | BlocklistAIHistoryEvent::UpdatedAutoexecuteOverride { .. }
            | BlocklistAIHistoryEvent::SplitConversation { .. }
            | BlocklistAIHistoryEvent::UpdatedConversationArtifacts { .. }
            | BlocklistAIHistoryEvent::ConversationOwnershipTransferred { .. }
            | BlocklistAIHistoryEvent::NewConversationRequestComplete { .. }
            | BlocklistAIHistoryEvent::OrchestrationConfigUpdated { .. } => {}
        }
    }

    fn on_server_token_assigned(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.ensure_self_run_id_watched(conversation_id, ctx) {
            self.reevaluate_eligibility(conversation_id, ctx);
        }
    }

    /// Inserts `self_run_id` into the conversation's watched set if the
    /// conversation has any orchestration role (child or parent) and is
    /// not a passive remote-run view. Returns whether anything was
    /// inserted; callers reevaluate eligibility on `true`. Idempotent.
    fn ensure_self_run_id_watched(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &warpui::AppContext,
    ) -> bool {
        let (run_id, is_child) = {
            let history = BlocklistAIHistoryModel::as_ref(ctx);
            let Some(conversation) = history.conversation(&conversation_id) else {
                return false;
            };
            // Passive views of agent runs hosted elsewhere (shared-session
            // viewers and remote-child placeholders) must not subscribe —
            // the actual agent (in another process) is the inbox.
            if conversation.is_viewing_shared_session() || conversation.is_remote_child() {
                return false;
            }
            let Some(run_id) = conversation.run_id() else {
                return false;
            };
            (run_id, conversation.has_parent_agent())
        };

        // Parent role: any watched run_id that isn't this conversation's
        // own self_run_id (i.e. a registered child).
        let is_parent = self
            .streams
            .get(&conversation_id)
            .is_some_and(|s| s.watched_run_ids.iter().any(|id| id != &run_id));

        if !is_child && !is_parent {
            return false;
        }

        self.streams
            .entry(conversation_id)
            .or_default()
            .watched_run_ids
            .insert(run_id)
    }

    fn on_streaming_exchange_updated(
        &mut self,
        conversation_id: AIConversationId,
        exchange_id: AIAgentExchangeId,
        ctx: &mut ModelContext<Self>,
    ) {
        // Snapshot pending IDs so the immutable borrow on `self.streams`
        // doesn't collide with the history model lookup below.
        let pending_ids: HashSet<String> = match self.streams.get(&conversation_id) {
            Some(s) if !s.pending_message_ids.is_empty() => {
                s.pending_message_ids.iter().cloned().collect()
            }
            _ => return,
        };

        let Some(conversation) =
            BlocklistAIHistoryModel::as_ref(ctx).conversation(&conversation_id)
        else {
            return;
        };
        let Some(exchange) = conversation.exchange_with_id(exchange_id) else {
            return;
        };

        // Check if the exchange output contains any of the messages we're
        // waiting to confirm.
        let mut confirmed_ids = Vec::new();
        if let Some(output) = exchange.output_status.output() {
            for msg in &output.get().messages {
                if let AIAgentOutputMessageType::MessagesReceivedFromAgents { messages } =
                    &msg.message
                {
                    for received in messages {
                        if pending_ids.contains(received.message_id.as_str()) {
                            confirmed_ids.push(received.message_id.clone());
                        }
                    }
                }
            }
        }

        if confirmed_ids.is_empty() {
            return;
        }

        // Remove confirmed messages from pending.
        if let Some(stream) = self.streams.get_mut(&conversation_id) {
            stream
                .pending_message_ids
                .retain(|id| !confirmed_ids.contains(id));
        }

        let _ = confirmed_ids;
    }

    /// Cleans up local state for a removed/deleted conversation, then
    /// prunes the removed conversation's run_id from any *other*
    /// tracked conversation's watched set (in case it was a child of
    /// another parent we're still tracking) and re-evaluates eligibility
    /// for those parents.
    ///
    /// `removed_run_id` is the run_id of the conversation as captured by
    /// the history model just before it dropped its in-memory record.
    /// Looking it up here would return `None` because the history model
    /// emits the removal event after removing the record.
    fn on_conversation_removed(
        &mut self,
        conversation_id: AIConversationId,
        removed_run_id: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        // Drop all per-conversation streamer state in one go.
        self.streams.remove(&conversation_id);

        if let Some(run_id) = removed_run_id.as_deref() {
            let mut affected = Vec::new();
            for (other_id, stream) in self.streams.iter_mut() {
                if stream.watched_run_ids.remove(run_id) {
                    affected.push(*other_id);
                }
            }
            for other_id in affected {
                self.reevaluate_eligibility(other_id, ctx);
            }
        }
    }

    // ---- Restore-on-startup ------------------------------------------

    /// Re-establishes orchestration event delivery state for conversations
    /// loaded from disk on startup. Initializes the in-memory cursor from
    /// the SQLite-persisted `last_event_sequence`, registers each
    /// conversation's own run_id as watched, and re-evaluates local
    /// eligibility against the restored conversation tree.
    fn on_restored_conversations(
        &mut self,
        conversation_ids: Vec<AIConversationId>,
        ctx: &mut ModelContext<Self>,
    ) {
        // Orchestration v2 owns the events endpoints and the cursor model.
        // V1 conversations may carry a run_id but the v2-only event APIs
        // would return spurious 4xx responses, so skip restore entirely
        // when V2 is disabled.
        if !FeatureFlag::OrchestrationV2.is_enabled() {
            return;
        }

        for conv_id in conversation_ids {
            let (run_id, cursor, is_remote_view) = {
                let history = BlocklistAIHistoryModel::as_ref(ctx);
                let Some(conversation) = history.conversation(&conv_id) else {
                    continue;
                };
                let is_remote_view =
                    conversation.is_viewing_shared_session() || conversation.is_remote_child();
                let run_id = conversation.run_id();
                let cursor = conversation.last_event_sequence().unwrap_or(0);
                (run_id, cursor, is_remote_view)
            };

            // Passive views of remote runs (shared-session viewers,
            // remote-child placeholders) must not subscribe — the actual
            // agent in another process owns the inbox.
            if is_remote_view {
                continue;
            }

            // Initialize the in-memory cursor from the persisted SQLite
            // value and register the conversation's own run_id so lifecycle
            // events for self are correctly filtered.
            let stream = self.streams.entry(conv_id).or_default();
            stream.event_cursor = cursor;
            if let Some(ref own) = run_id {
                stream.watched_run_ids.insert(own.clone());
            }

            self.reevaluate_eligibility(conv_id, ctx);
        }
    }

    // ---- Eligibility predicate ---------------------------------------

    fn self_run_id(
        &self,
        conversation_id: AIConversationId,
        ctx: &warpui::AppContext,
    ) -> Option<String> {
        BlocklistAIHistoryModel::as_ref(ctx)
            .conversation(&conversation_id)
            .and_then(|c| c.run_id())
    }

    /// Parent role: the conversation has at least one watched child
    /// run_id (i.e. a watched run_id that is not its own self_run_id).
    fn is_parent_agent_conversation(
        &self,
        conversation_id: AIConversationId,
        ctx: &warpui::AppContext,
    ) -> bool {
        let Some(stream) = self.streams.get(&conversation_id) else {
            return false;
        };
        let self_run_id = self.self_run_id(conversation_id, ctx);
        stream
            .watched_run_ids
            .iter()
            .any(|id| Some(id.as_str()) != self_run_id.as_deref())
    }

    fn has_active_consumer(&self, conversation_id: AIConversationId) -> bool {
        self.streams
            .get(&conversation_id)
            .is_some_and(|s| !s.consumers.is_empty())
    }

    /// True iff this conversation is a passive view of an agent run that
    /// is actually executing in another process — either a shared-session
    /// viewer or a placeholder for a remote child run spawned via
    /// `start_agent` with cloud `execution_mode`. Either way the actual
    /// run lives elsewhere (and that process owns the inbox), so this
    /// process should not open its own SSE for the conversation.
    fn is_remote_run_view(
        &self,
        conversation_id: AIConversationId,
        ctx: &warpui::AppContext,
    ) -> bool {
        BlocklistAIHistoryModel::as_ref(ctx)
            .conversation(&conversation_id)
            .is_some_and(|c| c.is_viewing_shared_session() || c.is_remote_child())
    }

    fn should_skip_sse_for_dormant_local_claude_child(
        &self,
        conversation_id: AIConversationId,
        ctx: &warpui::AppContext,
    ) -> bool {
        let Some(conversation) =
            BlocklistAIHistoryModel::as_ref(ctx).conversation(&conversation_id)
        else {
            return false;
        };
        conversation.is_child_agent_conversation()
            && !conversation.is_remote_child()
            && matches!(conversation.status(), ConversationStatus::Success)
            && (conversation
                .server_metadata()
                .is_some_and(|metadata| metadata.harness == AIAgentHarness::ClaudeCode)
                || self
                    .streams
                    .get(&conversation_id)
                    .and_then(|stream| stream.harness)
                    .is_some_and(|harness| harness == Harness::Claude))
    }

    /// True iff this conversation should currently hold an SSE connection.
    /// A subscription is needed only when there is an active consumer in
    /// this process (an open agent view or an agent_sdk driver) AND the
    /// conversation has a real role to consume events for. Passive views
    /// of agent runs hosted elsewhere are excluded regardless of state.
    fn is_eligible(&self, conversation_id: AIConversationId, ctx: &warpui::AppContext) -> bool {
        if !self.has_active_consumer(conversation_id) {
            return false;
        }
        if self.is_remote_run_view(conversation_id, ctx) {
            return false;
        }
        if self.should_skip_sse_for_dormant_local_claude_child(conversation_id, ctx) {
            log::info!(
                "Skipping generic SSE delivery for dormant local Claude child {conversation_id:?}; parent bridge will deliver wake events"
            );
            return false;
        }
        let has_parent = BlocklistAIHistoryModel::as_ref(ctx)
            .conversation(&conversation_id)
            .is_some_and(|c| c.has_parent_agent());
        has_parent || self.is_parent_agent_conversation(conversation_id, ctx)
    }

    /// Re-evaluates eligibility and either opens / reconnects or tears
    /// down the SSE connection for the given conversation.
    fn reevaluate_eligibility(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.is_eligible(conversation_id, ctx) {
            self.start_sse_connection(conversation_id, ctx);
        }
    }

    /// Opens a long-lived SSE connection for `conversation_id`. Events
    /// are sent through an mpsc channel and drained by a periodic timer.
    fn start_sse_connection(
        &mut self,
        conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) {
        let _ = (conversation_id, ctx);
        log::debug!("Hosted orchestration SSE is disabled in this local-first build");
    }

    /// Feeds a batch of fetched events through the OrchestrationEventService,
    /// updating the in-memory and persisted cursors and tracking message
    /// IDs awaiting delivery confirmation.
    #[cfg(test)]
    fn handle_event_batch(
        &mut self,
        conversation_id: AIConversationId,
        self_run_id: &str,
        previous_cursor: i64,
        mut events: Vec<AgentRunEvent>,
        mut messages: Vec<ReceivedMessageInput>,
        ctx: &mut ModelContext<Self>,
    ) {
        let max_seq = events
            .iter()
            .map(|e| e.sequence)
            .max()
            .unwrap_or(previous_cursor);
        // Advance the cursor before filtering so dropped killed-run events
        // are not replayed later.
        BlocklistAIHistoryModel::handle(ctx).update(ctx, |model, ctx| {
            model.update_event_sequence(conversation_id, max_seq, ctx);
        });

        if !self.killed_run_ids.is_empty() {
            let dropped_message_ids: HashSet<String> = events
                .iter()
                .filter(|event| self.killed_run_ids.contains(&event.run_id))
                .filter_map(|event| event.ref_id.clone())
                .collect();
            let event_count_before = events.len();
            events.retain(|event| !self.killed_run_ids.contains(&event.run_id));
            messages.retain(|message| {
                !dropped_message_ids.contains(&message.message_id)
                    && !self.killed_run_ids.contains(&message.sender_agent_id)
            });
            let dropped_event_count = event_count_before - events.len();
            if dropped_event_count > 0 {
                log::info!(
                    "Dropped {dropped_event_count} orchestration events for killed run IDs while handling {conversation_id:?}"
                );
            }
        }
        // Track message IDs for server-side mark_delivered calls.
        let message_ids: Vec<String> = messages
            .iter()
            .map(|message| message.message_id.clone())
            .collect();
        if !message_ids.is_empty() {
            self.streams
                .entry(conversation_id)
                .or_default()
                .pending_message_ids
                .extend(message_ids);
        }

        let lifecycle_events = convert_lifecycle_events(&events, self_run_id);
        if messages.is_empty() && lifecycle_events.is_empty() {
            return;
        }

        let pending = build_pending_events(&events, messages, lifecycle_events);
        OrchestrationEventService::handle(ctx).update(ctx, |svc, ctx| {
            svc.enqueue_event_batch(conversation_id, pending, ctx);
        });
    }
}

impl Entity for OrchestrationEventStreamer {
    type Event = ();
}

impl SingletonEntity for OrchestrationEventStreamer {}

#[cfg(test)]
fn parse_occurred_at(s: &str) -> prost_types::Timestamp {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| prost_types::Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
        })
        .unwrap_or_else(|_| {
            let now = chrono::Utc::now();
            prost_types::Timestamp {
                seconds: now.timestamp(),
                nanos: now.timestamp_subsec_nanos() as i32,
            }
        })
}

#[cfg(test)]
fn convert_lifecycle_events(events: &[AgentRunEvent], self_run_id: &str) -> Vec<api::AgentEvent> {
    events
        .iter()
        .filter(|e| e.event_type != "new_message" && e.run_id != self_run_id)
        .filter_map(|event| {
            let lifecycle_type = match event.event_type.as_str() {
                // New canonical event types aligned with task states.
                "run_in_progress" => api::LifecycleEventType::InProgress,
                "run_succeeded" => api::LifecycleEventType::Succeeded,
                "run_failed" => api::LifecycleEventType::Failed,
                // Legacy event types mapped to new variants for backward compat.
                #[allow(deprecated)]
                "run_started" => api::LifecycleEventType::InProgress,
                #[allow(deprecated)]
                "run_idle" => api::LifecycleEventType::Succeeded,
                #[allow(deprecated)]
                "run_restarted" => api::LifecycleEventType::InProgress,
                "run_errored" => api::LifecycleEventType::Errored,
                "run_cancelled" => api::LifecycleEventType::Cancelled,
                "run_blocked" => api::LifecycleEventType::Blocked,
                _ => return None,
            };
            let timestamp = parse_occurred_at(&event.occurred_at);
            // TODO: Parse richer detail payloads (reason, error_message) from
            // the server event log once the schema supports them.
            let detail = match lifecycle_type {
                api::LifecycleEventType::Errored => LifecycleEventDetailPayload {
                    stage: Some(LifecycleEventDetailStage::Runtime),
                    reason: event.ref_id.clone(),
                    ..Default::default()
                },
                _ => LifecycleEventDetailPayload::default(),
            };
            let event_id = Uuid::new_v4().to_string();
            Some(build_lifecycle_event(
                event_id,
                event.run_id.clone(),
                lifecycle_type,
                timestamp,
                &detail,
            ))
        })
        .collect()
}

#[cfg(test)]
fn build_pending_events(
    events: &[AgentRunEvent],
    messages: Vec<ReceivedMessageInput>,
    lifecycle_events: Vec<api::AgentEvent>,
) -> Vec<PendingEvent> {
    let mut pending = Vec::with_capacity(messages.len() + lifecycle_events.len());
    for msg in &messages {
        let metadata = events
            .iter()
            .find(|event| {
                event.event_type == "new_message"
                    && event.ref_id.as_deref() == Some(msg.message_id.as_str())
            })
            .map(|event| (event.sequence, event.occurred_at.clone()));
        let (sequence, occurred_at) =
            metadata.unwrap_or_else(|| (0, chrono::Utc::now().to_rfc3339()));
        pending.push(PendingEvent {
            event_id: msg.message_id.clone(),
            source_agent_id: msg.sender_agent_id.clone(),
            attempt_count: 0,
            detail: PendingEventDetail::Message {
                sequence,
                message_id: msg.message_id.clone(),
                addresses: msg.addresses.clone(),
                subject: msg.subject.clone(),
                message_body: msg.message_body.clone(),
                occurred_at,
            },
        });
    }
    for event in lifecycle_events {
        pending.push(PendingEvent {
            event_id: event.event_id.clone(),
            source_agent_id: String::new(),
            attempt_count: 0,
            detail: PendingEventDetail::Lifecycle { event },
        });
    }
    pending
}

// ---- Free-function consumer registration helpers ---------------------
//
// Wrap the feature-flag check + singleton handle update so call sites
// in `ActiveAgentViewsModel` and the agent_sdk driver don't have to
// repeat the boilerplate. The generic bound covers both
// `&mut AppContext` and `&mut ModelContext<T>` / `&mut ViewContext<T>`.
//
// Consumers are identified by an `EntityId` — the terminal pane's id
// for an agent view, the driver model's id for `agent_sdk`. The
// streamer never branches on consumer kind, so a single pair of helpers
// covers both call sites.

/// Registers a consumer of orchestration agent events for
/// `conversation_id`. No-op when `OrchestrationV2` is disabled.
pub fn register_agent_event_consumer<C>(
    conversation_id: AIConversationId,
    consumer_id: EntityId,
    ctx: &mut C,
) where
    C: GetSingletonModelHandle + UpdateModel,
{
    if !FeatureFlag::OrchestrationV2.is_enabled() {
        return;
    }
    OrchestrationEventStreamer::handle(ctx).update(ctx, |streamer, ctx| {
        streamer.register_consumer(conversation_id, consumer_id, ctx);
    });
}

/// Pair to [`register_agent_event_consumer`].
pub fn unregister_agent_event_consumer<C>(
    conversation_id: AIConversationId,
    consumer_id: EntityId,
    ctx: &mut C,
) where
    C: GetSingletonModelHandle + UpdateModel,
{
    if !FeatureFlag::OrchestrationV2.is_enabled() {
        return;
    }
    OrchestrationEventStreamer::handle(ctx).update(ctx, |streamer, ctx| {
        streamer.unregister_consumer(conversation_id, consumer_id, ctx);
    });
}

#[cfg(test)]
#[path = "orchestration_event_streamer_tests.rs"]
mod tests;
