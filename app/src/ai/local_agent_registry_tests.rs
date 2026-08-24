use super::*;
use crate::ai::agent::conversation::AIConversationId;
use warp_cli::agent::Harness;

fn registration(
    parent_run_id: Option<String>,
    name: &str,
    action_id: &str,
) -> LocalAgentRegistration {
    let mut request = LocalAgentRegistration::new(
        AIConversationId::new(),
        parent_run_id,
        name,
        "do the local work",
        Harness::Oz,
    );
    request.action_id = Some(action_id.to_string());
    request.controller_owner = Some("controller".to_string());
    request
}

#[test]
fn registering_a_child_assigns_a_stable_run_id_and_topology() {
    let mut registry = LocalAgentRegistry::new();
    let parent = registry
        .register_child(registration(None, "parent", "parent-action"))
        .unwrap()
        .run;
    let child = registry
        .register_child(registration(
            Some(parent.run_id.clone()),
            "child",
            "child-action",
        ))
        .unwrap()
        .run;

    assert!(!parent.run_id.is_empty());
    assert_eq!(child.parent_run_id.as_deref(), Some(parent.run_id.as_str()));
    assert_eq!(
        registry.child_run_ids(&parent.run_id),
        std::slice::from_ref(&child.run_id)
    );
    assert!(registry.is_ready(&child.run_id));
}

#[test]
fn duplicate_action_id_returns_original_run_without_duplicate_registration() {
    let mut registry = LocalAgentRegistry::new();
    let mut first = LocalAgentRegistration::new(
        AIConversationId::new(),
        None,
        "child",
        "prompt",
        Harness::Oz,
    );
    first.action_id = Some("same-action".to_string());
    first.controller_owner = Some("controller".to_string());
    let original = registry.register_child(first.clone()).unwrap().run;

    let duplicate = registry
        .register_child(LocalAgentRegistration {
            conversation_id: original.conversation_id,
            run_id: Some("different-run-id".to_string()),
            ..first
        })
        .unwrap();

    assert!(!duplicate.created);
    assert_eq!(duplicate.run.run_id, original.run_id);
    assert_eq!(registry.len(), 1);
}

#[test]
fn fanout_depth_and_live_limits_fail_before_child_creation() {
    let mut registry = LocalAgentRegistry::with_limits(LocalAgentLimits {
        max_fanout: 2,
        max_depth: 1,
        max_live_children: 1,
        max_pending_messages: 1,
    });
    let parent = registry
        .register_child(registration(None, "parent", "parent-action"))
        .unwrap()
        .run;

    let mut too_many = registration(
        Some(parent.run_id.clone()),
        "first",
        "first-action",
    );
    too_many.requested_fanout = 3;
    assert!(matches!(
        registry.register_child(too_many),
        Err(LocalAgentRegistryError::FanoutLimit { .. })
    ));
    assert_eq!(registry.len(), 1);

    let child = registry
        .register_child(registration(
            Some(parent.run_id.clone()),
            "first",
            "first-action",
        ))
        .unwrap()
        .run;
    assert!(matches!(
        registry.register_child(registration(
            Some(child.run_id.clone()),
            "nested",
            "nested-action",
        )),
        Err(LocalAgentRegistryError::DepthLimit { .. })
    ));
    assert_eq!(registry.len(), 2);

    assert!(matches!(
        registry.register_child(registration(
            Some(parent.run_id),
            "sibling",
            "sibling-action",
        )),
        Err(LocalAgentRegistryError::ConcurrentLimit { .. })
    ));
    assert_eq!(registry.len(), 2);
}

#[test]
fn restored_runs_are_stopped_until_controller_reclaims_them() {
    let mut registry = LocalAgentRegistry::new();
    let restored = registry
        .restore_stopped(RestoredLocalAgent {
            run_id: "persisted-run".to_string(),
            conversation_id: AIConversationId::new(),
            terminal_surface_id: None,
            pane_id: None,
            parent_run_id: None,
            name: "historical child".to_string(),
            harness: Harness::Claude,
        })
        .unwrap();
    assert_eq!(restored.status, LocalAgentStatus::Stopped);
    assert!(!registry.is_ready("persisted-run"));
    assert!(matches!(
        registry.send_message("persisted-run", "persisted-run", "subject", "body"),
        Err(LocalAgentRegistryError::HistoricalRun(_))
            | Err(LocalAgentRegistryError::ControllerRequired(_))
    ));

    registry
        .claim_controller("persisted-run", "new-controller")
        .unwrap();
    assert!(registry.is_ready("persisted-run"));
    assert_eq!(
        registry.get("persisted-run").map(|run| run.status),
        Some(LocalAgentStatus::Idle)
    );
}

#[test]
fn message_ack_is_local_intake_with_bounded_queue_and_no_retry() {
    let mut registry = LocalAgentRegistry::with_limits(LocalAgentLimits {
        max_pending_messages: 1,
        ..LocalAgentLimits::default()
    });
    let sender = registry
        .register_child(registration(None, "sender", "sender-action"))
        .unwrap()
        .run;
    let receiver = registry
        .register_child(registration(None, "receiver", "receiver-action"))
        .unwrap()
        .run;

    let ack = registry
        .send_message(
            &sender.run_id,
            &receiver.run_id,
            "subject",
            "one message",
        )
        .unwrap();
    assert_eq!(ack.recipient_run_id, receiver.run_id);
    assert!(ack.wake_requested);
    assert_eq!(registry.pending_message_count(&receiver.run_id), 1);

    assert!(matches!(
        registry.send_message(&sender.run_id, &receiver.run_id, "subject", "second message"),
        Err(LocalAgentRegistryError::QueueFull { .. })
    ));
    let messages = registry
        .take_pending_messages(&receiver.run_id, "controller")
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message_id, ack.message_id);
    assert_eq!(registry.pending_message_count(&receiver.run_id), 0);
}
