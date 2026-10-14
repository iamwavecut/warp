use super::*;
use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};
use serde_json::json;
use warp_util::path::EscapeChar;
use warpui::App;

#[test]
fn unbound_requests_and_protected_writes_fail_closed() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            let terminal_id = ctx.view_id();
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let request = PolicyRequest::Execute {
                    command: "cargo test".into(),
                };
                assert!(matches!(
                    request.evaluate(controller, terminal_id, EscapeChar::Backslash, ctx),
                    PolicyDecision::Deny { .. }
                ));
                controller.bind_native_prompt_conversation(None, ctx);
                assert!(matches!(
                    PolicyRequest::write("/repo/.warp/.mcp.json").evaluate(
                        controller,
                        terminal_id,
                        EscapeChar::Backslash,
                        ctx
                    ),
                    PolicyDecision::Deny { .. }
                ));
            });
        });
    });
}

#[test]
fn permission_selection_never_expands_to_allow_always_or_unknown_tools() {
    let params: protocol::RequestPermissionParams = serde_json::from_value(json!({
        "toolCall": {"toolCallId": "x"},
        "options": [
            {"optionId": "always", "kind": "allow_always"},
            {"optionId": "once", "kind": "allow_once"},
            {"optionId": "reject", "kind": "reject_once"}
        ]
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(select_permission(&PolicyDecision::Allow, &params.options)).unwrap()["optionId"],
        "once"
    );
    assert_eq!(
        serde_json::to_value(select_permission(
            &PolicyDecision::Deny {
                reason: "approval required".into()
            },
            &params.options
        ))
        .unwrap()["optionId"],
        "reject"
    );
    assert_eq!(
        serde_json::to_value(select_permission(
            &PolicyDecision::Allow,
            &params.options[..1]
        ))
        .unwrap()["outcome"],
        "cancelled"
    );
    assert!(PolicyRequest::for_tool_call(&params.tool_call).is_none());
}
