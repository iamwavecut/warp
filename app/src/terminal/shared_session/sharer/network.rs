//! Local-first boundary for hosted shared-session creation.
//!
//! The upstream implementation opened a websocket to Warp's shared-session
//! backend and mirrored terminal state to that service. This fork keeps the
//! surrounding local terminal state types compiling, but does not create hosted
//! sessions or send terminal data to a remote service.

use std::sync::Arc;

use async_channel::Receiver;
use parking_lot::FairMutex;
#[cfg(any(test, feature = "integration_tests"))]
use session_sharing_protocol::common::SessionId;
use session_sharing_protocol::common::{
    ActivePrompt, ParticipantId, Role, RoleRequestId, RoleRequestResponse, Selection,
    UniversalDeveloperInputContextUpdate,
};
use session_sharing_protocol::sharer::{
    FailedToInitializeSessionReason, RoleUpdateReason, SessionEndedReason,
};
#[cfg(not(any(test, feature = "integration_tests")))]
use session_sharing_protocol::{
    common::UniversalDeveloperInputContext,
    sharer::{Lifetime, SessionSourceType},
};
use warpui::{Entity, ModelContext};

use crate::auth::UserUid;
use crate::editor::{CrdtOperation, ReplicaId};
use crate::terminal::model::block::BlockId;
use crate::terminal::shared_session::SharedSessionScrollbackType;
use crate::terminal::TerminalModel;

const HOSTED_SHARED_SESSIONS_DISABLED: &str =
    "Hosted shared sessions are disabled in this local-first build.";

pub struct Network {
    active_prompt: ActivePrompt,
    selection: Selection,
}

impl Network {
    #[cfg(any(test, feature = "integration_tests"))]
    pub fn new_for_test(
        _model: Arc<FairMutex<TerminalModel>>,
        _ordered_events_rx: Receiver<session_sharing_protocol::common::OrderedTerminalEventType>,
        _scrollback_type: SharedSessionScrollbackType,
        active_prompt: ActivePrompt,
        selection: Selection,
        _input_replica_id: ReplicaId,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        ctx.emit(NetworkEvent::SharedSessionCreatedSuccessfully {
            session_id: SessionId::new(),
            sharer_id: ParticipantId::new(),
            sharer_firebase_uid: UserUid::new("local_shared_session_uid"),
        });

        Self {
            active_prompt,
            selection,
        }
    }

    #[cfg(not(any(test, feature = "integration_tests")))]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        _model: Arc<FairMutex<TerminalModel>>,
        _ordered_events_rx: Receiver<session_sharing_protocol::common::OrderedTerminalEventType>,
        _scrollback_type: SharedSessionScrollbackType,
        active_prompt: ActivePrompt,
        selection: Selection,
        _input_replica_id: ReplicaId,
        _terminal_view_id: warpui::EntityId,
        _universal_developer_input_context: UniversalDeveloperInputContext,
        _lifetime: Lifetime,
        _source_type: SessionSourceType,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        ctx.emit(NetworkEvent::FailedToCreateSharedSession {
            reason: FailedToInitializeSessionReason::InternalServerError {
                details: HOSTED_SHARED_SESSIONS_DISABLED.to_owned(),
            },
            cause: None,
        });

        Self {
            active_prompt,
            selection,
        }
    }

    pub fn is_connected(&self) -> bool {
        false
    }

    pub fn end_session(&mut self, _reason: SessionEndedReason) {}

    pub fn send_active_prompt_update_if_changed(&mut self, active_prompt: ActivePrompt) {
        self.active_prompt = active_prompt;
    }

    pub fn send_presence_selection_if_changed(&mut self, selection: Selection) {
        self.selection = selection;
    }

    pub fn send_role_update(&mut self, _participant_id: ParticipantId, _role: Role) {}

    pub fn send_user_role_update(&mut self, _user_uid: UserUid, _role: Role) {}

    pub fn send_pending_user_role_update(&mut self, _email: String, _role: Role) {}

    pub fn send_add_guests(&mut self, _emails: Vec<String>, _role: Role) {}

    pub fn send_remove_guest(&mut self, _user_uid: UserUid) {}

    pub fn send_remove_pending_guest(&mut self, _email: String) {}

    pub fn send_make_all_participants_readers(&mut self, _reason: RoleUpdateReason) {}

    pub fn send_role_request_response(
        &mut self,
        _participant_id: ParticipantId,
        _request_id: RoleRequestId,
        _response: RoleRequestResponse,
    ) {
    }

    pub fn send_input_update<'a>(
        &mut self,
        _block_id: &BlockId,
        _operations: impl Iterator<Item = &'a CrdtOperation>,
    ) {
    }

    pub fn send_link_permission_update(&mut self, _role: Option<Role>) {}

    pub fn send_team_permission_update(&mut self, _role: Option<Role>, _team_uid: String) {}

    pub fn send_universal_developer_input_context_update(
        &mut self,
        _update: UniversalDeveloperInputContextUpdate,
    ) {
    }
}

pub fn failed_to_initialize_session_user_error(
    _reason: &FailedToInitializeSessionReason,
) -> String {
    HOSTED_SHARED_SESSIONS_DISABLED.to_owned()
}

pub enum NetworkEvent {
    #[cfg(any(test, feature = "integration_tests"))]
    SharedSessionCreatedSuccessfully {
        session_id: SessionId,
        sharer_id: ParticipantId,
        sharer_firebase_uid: UserUid,
    },
    FailedToCreateSharedSession {
        reason: FailedToInitializeSessionReason,
        /// Internal error cause not suitable for displaying to the user,
        /// but useful for diagnostics.
        cause: Option<Arc<anyhow::Error>>,
    },
}

impl Entity for Network {
    type Event = NetworkEvent;
}
