use futures::{future::BoxFuture, FutureExt};
use warpui::{Entity, EntityId, ModelContext, ModelHandle};

use crate::ai::agent::{
    AIAgentAction, AIAgentActionResultType, AIAgentActionType, UploadArtifactResult,
};
use crate::terminal::model::session::active_session::ActiveSession;

use super::{ActionExecution, AnyActionExecution, ExecuteActionInput, PreprocessActionInput};

pub struct UploadArtifactExecutor {
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    active_session: ModelHandle<ActiveSession>,
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    terminal_view_id: EntityId,
}

impl UploadArtifactExecutor {
    pub fn new(active_session: ModelHandle<ActiveSession>, terminal_view_id: EntityId) -> Self {
        Self {
            active_session,
            terminal_view_id,
        }
    }

    #[cfg_attr(target_family = "wasm", allow(unused_variables), allow(dead_code))]
    pub(super) fn should_autoexecute(
        &self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let _ = (input, ctx);
        false
    }

    #[cfg_attr(target_family = "wasm", allow(unused_variables), allow(dead_code))]
    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> AnyActionExecution {
        let ExecuteActionInput { action, .. } = input;
        let AIAgentAction {
            action: AIAgentActionType::UploadArtifact(_),
            ..
        } = action
        else {
            return ActionExecution::<()>::InvalidAction.into();
        };

        let _ = ctx;
        ActionExecution::<()>::Sync(AIAgentActionResultType::UploadArtifact(
            UploadArtifactResult::Error(
                "Uploading artifacts to Warp backend is disabled in this local-first build"
                    .to_string(),
            ),
        ))
        .into()
    }

    pub(super) fn preprocess_action(
        &mut self,
        _input: PreprocessActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }
}

impl Entity for UploadArtifactExecutor {
    type Event = ();
}
