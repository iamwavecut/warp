use std::path::PathBuf;

use futures::FutureExt;
use futures::future::BoxFuture;
use warpui::{Entity, EntityId, ModelContext, ModelHandle, SingletonEntity};

use super::{ActionExecution, AnyActionExecution, ExecuteActionInput, PreprocessActionInput};
use crate::ai::agent::{
    AIAgentAction, AIAgentActionResultType, AIAgentActionType, UploadArtifactResult,
};
use crate::ai::artifacts::Artifact;
use crate::ai::blocklist::{BlocklistAIHistoryModel, BlocklistAIPermissions};
use crate::ai::local_artifacts::{LocalArtifactKind, LocalArtifactOwner, LocalArtifactRepository};
use crate::ai::paths::host_native_absolute_path;
use crate::terminal::model::session::active_session::ActiveSession;

pub struct UploadArtifactExecutor {
    active_session: ModelHandle<ActiveSession>,
    terminal_view_id: EntityId,
}

impl UploadArtifactExecutor {
    pub fn new(active_session: ModelHandle<ActiveSession>, terminal_view_id: EntityId) -> Self {
        Self {
            active_session,
            terminal_view_id,
        }
    }

    pub(super) fn should_autoexecute(
        &self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let ExecuteActionInput {
            action:
                AIAgentAction {
                    action: AIAgentActionType::UploadArtifact(request),
                    ..
                },
            conversation_id,
        } = input
        else {
            return false;
        };

        BlocklistAIPermissions::as_ref(ctx)
            .can_read_files_with_conversation(
                &conversation_id,
                vec![self.resolve_path(&request.file_path, ctx)],
                Some(self.terminal_view_id),
                ctx,
            )
            .is_allowed()
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> AnyActionExecution {
        let ExecuteActionInput {
            action,
            conversation_id,
        } = input;
        let AIAgentAction {
            action: AIAgentActionType::UploadArtifact(request),
            ..
        } = action
        else {
            return ActionExecution::<()>::InvalidAction.into();
        };

        let resolved_path = self.resolve_path(&request.file_path, ctx);
        BlocklistAIPermissions::handle(ctx).update(ctx, |model, _ctx| {
            model.add_temporary_file_read_permissions(conversation_id, [resolved_path.clone()]);
        });

        let owner = LocalArtifactOwner::conversation(conversation_id);
        let description = request.description.clone();
        let terminal_view_id = self.terminal_view_id;
        ActionExecution::new_async(
            blocking::unblock(move || {
                LocalArtifactRepository::open_current_scope()?.import_path(
                    resolved_path,
                    owner,
                    description,
                )
            }),
            move |result, ctx| match result {
                Ok(record) => {
                    let artifact_uid = record.artifact_uid.to_string();
                    let artifact = match record.kind {
                        LocalArtifactKind::Screenshot => Artifact::Screenshot {
                            artifact_uid: artifact_uid.clone(),
                            mime_type: record.mime_type.clone(),
                            description: record.description.clone(),
                        },
                        LocalArtifactKind::File => Artifact::File {
                            artifact_uid: artifact_uid.clone(),
                            filepath: record.local_path.to_string_lossy().into_owned(),
                            filename: record.filename.clone(),
                            mime_type: record.mime_type.clone(),
                            description: record.description.clone(),
                            size_bytes: i32::try_from(record.size_bytes).ok(),
                        },
                    };
                    BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                        if let Some(conversation) = history.conversation_mut(&conversation_id) {
                            conversation.add_artifact(artifact, terminal_view_id, ctx);
                        }
                    });
                    AIAgentActionResultType::UploadArtifact(UploadArtifactResult::Success {
                        artifact_uid,
                        filepath: Some(record.local_path.to_string_lossy().into_owned()),
                        mime_type: record.mime_type,
                        description: record.description,
                        size_bytes: record.size_bytes,
                    })
                }
                Err(error) => AIAgentActionResultType::UploadArtifact(UploadArtifactResult::Error(
                    format!("Failed to preserve local artifact: {error}"),
                )),
            },
        )
        .into()
    }

    pub(super) fn preprocess_action(
        &mut self,
        _input: PreprocessActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }

    fn resolve_path(&self, file_path: &str, ctx: &ModelContext<Self>) -> PathBuf {
        let current_working_directory = self
            .active_session
            .as_ref(ctx)
            .current_working_directory()
            .cloned();
        let shell = self.active_session.as_ref(ctx).shell_launch_data(ctx);

        let path = PathBuf::from(host_native_absolute_path(
            file_path,
            &shell,
            &current_working_directory,
        ));
        std::fs::canonicalize(&path).unwrap_or(path)
    }
}

impl Entity for UploadArtifactExecutor {
    type Event = ();
}
