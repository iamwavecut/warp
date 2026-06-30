//! Unit tests for [`BlocklistAIContextModel`].
//!
//! These tests use [`BlocklistAIContextModel::new_for_test`] and a small conversation-selection
//! fake to avoid unrelated subscriptions while exercising context behavior.

use std::sync::Arc;

use parking_lot::FairMutex;
use warpui::r#async::executor::Background;
use warpui::{App, EntityId, ModelHandle, SingletonEntity};

use super::{BlocklistAIContextModel, PendingAttachment, PendingFile};
use crate::ai::agent::conversation::{AIConversationAutoexecuteMode, AIConversationId};
use crate::ai::agent::ImageContext;
use crate::ai::blocklist::agent_view::{AgentViewEntryOrigin, EnterAgentViewError};
use crate::ai::blocklist::conversation_selection::{
    ConversationSelection, ConversationSelectionEvent,
};
use crate::ai::blocklist::{BlocklistAIHistoryEvent, BlocklistAIHistoryModel};
use crate::terminal::color::{self, Colors};
use crate::terminal::event_listener::ChannelEventListener;
use crate::terminal::model::test_utils::block_size;
use crate::terminal::model::{BlockId, TerminalModel};
use crate::test_util::settings::initialize_history_persistence_for_tests;

impl BlocklistAIContextModel {
    pub(crate) fn append_pending_attachments_for_test(
        &mut self,
        attachments: Vec<PendingAttachment>,
    ) {
        self.pending_attachments.extend(attachments);
    }

    pub(crate) fn insert_pending_block_id_for_test(&mut self, block_id: BlockId) {
        self.pending_context_block_ids.insert(block_id);
    }

    pub(crate) fn set_pending_selected_text_for_test(&mut self, text: Option<String>) {
        self.pending_context_selected_text = text;
    }
}

struct TestConversationSelection {
    terminal_surface_id: EntityId,
    selected_conversation_id: Option<AIConversationId>,
}

impl TestConversationSelection {
    fn new(
        terminal_surface_id: EntityId,
        _: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) -> Self {
        Self {
            terminal_surface_id,
            selected_conversation_id: None,
        }
    }
}

impl ConversationSelection for TestConversationSelection {
    fn selected_conversation_id(&self, _: &warpui::AppContext) -> Option<AIConversationId> {
        self.selected_conversation_id
    }

    fn is_conversation_active(&self, _: &warpui::AppContext) -> bool {
        self.selected_conversation_id.is_some()
    }

    fn is_conversation_fullscreen(&self, _: &warpui::AppContext) -> bool {
        self.selected_conversation_id.is_some()
    }

    fn select_existing_conversation(
        &mut self,
        conversation_id: AIConversationId,
        _: AgentViewEntryOrigin,
        ctx: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) {
        if self.selected_conversation_id != Some(conversation_id) {
            self.selected_conversation_id = Some(conversation_id);
            ctx.emit(ConversationSelectionEvent::Changed);
        }
    }

    fn select_new_conversation(
        &mut self,
        _: AgentViewEntryOrigin,
        ctx: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) {
        if self.selected_conversation_id.take().is_some() {
            ctx.emit(ConversationSelectionEvent::Changed);
        }
    }

    fn try_start_new_conversation(
        &mut self,
        _: AgentViewEntryOrigin,
        ctx: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) -> Result<AIConversationId, EnterAgentViewError> {
        let conversation_id = BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
            history.start_new_conversation(self.terminal_surface_id, false, false, false, ctx)
        });
        self.select_existing_conversation(conversation_id, AgentViewEntryOrigin::Cli, ctx);
        Ok(conversation_id)
    }

    fn pending_query_autoexecute_override(
        &self,
        app: &warpui::AppContext,
    ) -> AIConversationAutoexecuteMode {
        self.selected_conversation_id
            .as_ref()
            .and_then(|conversation_id| {
                BlocklistAIHistoryModel::as_ref(app).conversation(conversation_id)
            })
            .map(|conversation| conversation.autoexecute_override())
            .unwrap_or_default()
    }

    fn toggle_pending_query_autoexecute(
        &mut self,
        ctx: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) {
        if let Some(conversation_id) = self.selected_conversation_id {
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.toggle_autoexecute_override(
                    &conversation_id,
                    self.terminal_surface_id,
                    ctx,
                );
            });
        }
    }

    fn handle_history_event(
        &mut self,
        _: &BlocklistAIHistoryEvent,
        _: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) {
    }
}

/// Builds a [`BlocklistAIContextModel`] with stub dependencies. None of the dependencies are
/// exercised by the methods under test; they only need to satisfy the struct's field types.
fn build_test_context_model(app: &mut App) -> ModelHandle<BlocklistAIContextModel> {
    initialize_history_persistence_for_tests(app);
    app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
    let terminal_model = Arc::new(FairMutex::new(TerminalModel::new_for_test(
        block_size(),
        color::List::from(&Colors::default()),
        ChannelEventListener::new_for_test(),
        Arc::new(Background::default()),
        false, /* should_show_bootstrap_block */
        None,  /* restored_blocks */
        false, /* honor_ps1 */
        false, /* is_inverted */
        None,  /* session_startup_path */
    )));
    let terminal_view_id = EntityId::new();

    let conversation_selection = app.add_model(|ctx| {
        Box::new(TestConversationSelection::new(terminal_view_id, ctx))
            as Box<dyn ConversationSelection>
    });

    app.add_model(|_| {
        BlocklistAIContextModel::new_for_test(
            terminal_model,
            terminal_view_id,
            conversation_selection,
        )
    })
}

/// Builds context state for a TUI conversation surface.
fn build_tui_context_model(app: &mut App) -> (ModelHandle<BlocklistAIContextModel>, EntityId) {
    initialize_history_persistence_for_tests(app);
    app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
    let terminal_model = Arc::new(FairMutex::new(TerminalModel::new_for_test(
        block_size(),
        color::List::from(&Colors::default()),
        ChannelEventListener::new_for_test(),
        Arc::new(Background::default()),
        false,
        None,
        false,
        false,
        None,
    )));
    let terminal_surface_id = EntityId::new();
    let conversation_selection = app.add_model(|ctx| {
        Box::new(TestConversationSelection::new(terminal_surface_id, ctx))
            as Box<dyn ConversationSelection>
    });
    let model = app.add_model(|_| {
        BlocklistAIContextModel::new_for_test(
            terminal_model,
            terminal_surface_id,
            conversation_selection,
        )
    });
    (model, terminal_surface_id)
}

#[test]
fn tui_context_tracks_selected_conversation() {
    App::test((), |mut app| async move {
        let (model, _) = build_tui_context_model(&mut app);
        let conversation_id = AIConversationId::new();

        model.update(&mut app, |model, ctx| {
            model.set_pending_query_state_for_existing_conversation(
                conversation_id,
                AgentViewEntryOrigin::Cli,
                ctx,
            );
        });
        model.read(&app, |model, ctx| {
            assert_eq!(model.selected_conversation_id(ctx), Some(conversation_id));
        });

        model.update(&mut app, |model, ctx| {
            model.set_pending_query_state_for_new_conversation(AgentViewEntryOrigin::Cli, ctx);
        });
        model.read(&app, |model, ctx| {
            assert_eq!(model.selected_conversation_id(ctx), None);
        });
    });
}

#[test]
fn tui_new_conversation_is_selected_and_terminal_surface_scoped() {
    App::test((), |mut app| async move {
        let (model, terminal_surface_id) = build_tui_context_model(&mut app);
        let history = BlocklistAIHistoryModel::handle(&app);

        let conversation_id = model
            .update(&mut app, |model, ctx| {
                model.try_start_new_conversation(AgentViewEntryOrigin::Cli, ctx)
            })
            .expect("TUI conversation creation should succeed");

        model.read(&app, |model, ctx| {
            assert_eq!(model.selected_conversation_id(ctx), Some(conversation_id));
        });
        history.read(&app, |history, _| {
            assert_eq!(
                history
                    .all_live_conversations_for_terminal_surface(terminal_surface_id)
                    .map(|conversation| conversation.id())
                    .collect::<Vec<_>>(),
                vec![conversation_id]
            );
        });
    });
}

fn make_image_attachment(file_name: &str) -> PendingAttachment {
    PendingAttachment::Image(ImageContext {
        data: String::new(),
        mime_type: "image/png".to_owned(),
        file_name: file_name.to_owned(),
        is_figma: false,
    })
}

fn make_file_attachment(file_name: &str) -> PendingAttachment {
    PendingAttachment::File(PendingFile {
        file_name: file_name.to_owned(),
        file_path: file_name.into(),
        mime_type: "text/plain".to_owned(),
    })
}

#[test]
fn has_locking_attachment_is_false_for_default_state() {
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.read(&app, |m, _| {
            assert!(!m.has_locking_attachment());
        });
    });
}

#[test]
fn has_locking_attachment_is_false_with_only_pending_block_id() {
    // A pending block alone is *not* a locking attachment: only image/file attachments
    // should force the input into AI mode (skipping NLD).
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.insert_pending_block_id_for_test(BlockId::new());
        });

        model.read(&app, |m, _| assert!(!m.has_locking_attachment()));
    });
}

#[test]
fn has_locking_attachment_is_false_with_only_pending_selected_text() {
    // Selected text alone is *not* a locking attachment: the user could be selecting shell
    // command text (e.g. to copy a previously-run command), and forcing the input into AI
    // mode in that case would be wrong. Only image or file attachments should force the lock.
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.set_pending_selected_text_for_test(Some("hello".to_owned()));
        });

        model.read(&app, |m, _| assert!(!m.has_locking_attachment()));
    });
}

#[test]
fn has_locking_attachment_is_true_with_pending_image_attachment() {
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.append_pending_attachments_for_test(vec![make_image_attachment("a.png")]);
        });

        model.read(&app, |m, _| assert!(m.has_locking_attachment()));
    });
}

#[test]
fn has_locking_attachment_is_true_with_only_file_attachments() {
    // File attachments are locking attachments — the user has explicitly attached a file as
    // context, which is unambiguously a signal that the next query is intended for the agent.
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.append_pending_attachments_for_test(vec![
                make_file_attachment("notes.txt"),
                make_file_attachment("readme.md"),
            ]);
        });

        model.read(&app, |m, _| assert!(m.has_locking_attachment()));
    });
}

#[test]
fn has_locking_attachment_is_true_with_mixed_image_and_file_attachments() {
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.append_pending_attachments_for_test(vec![
                make_file_attachment("notes.txt"),
                make_image_attachment("a.png"),
            ]);
        });

        model.read(&app, |m, _| assert!(m.has_locking_attachment()));
    });
}
