use super::ClosedItem;
use crate::workspace::view::tests::{initialize_app, mock_workspace};
use warpui::App;

#[test]
fn undo_close_cleanup_skips_unavailable_group_in_open_window() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);
        let group = workspace.read(&app, |workspace, _| {
            workspace.tab_views().next().unwrap().clone()
        });
        // A view is temporarily unavailable while its update callback owns it.
        // Undo-close cleanup must not attempt a nested update in an open window.
        group.update(&mut app, |_, ctx| {
            assert!(ctx.is_window_open(group.window_id(ctx)));
            ClosedItem::clean_up_pane_group(&group, ctx);
        });
    });
}
