use super::*;
use crate::ai::agent::LocalMemoryContextItem;

fn memory_item(content: impl Into<String>) -> LocalMemoryContextItem {
    LocalMemoryContextItem {
        title: "Editor preference".to_string(),
        content: content.into(),
        scope: "Global".to_string(),
        revision: 3,
    }
}

#[test]
fn local_memory_context_round_trips_through_local_task_storage() {
    let original = vec![AIAgentContext::LocalMemory {
        items: vec![memory_item("Use Helix for quick edits")],
    }];

    let stored = api_input_context_from_agent_context(&original)
        .unwrap()
        .expect("memory should produce stored input context");
    let restored = super::super::convert_conversation::convert_input_context(Some(&stored));

    assert_eq!(restored.as_ref(), original.as_slice());
    assert!(
        context_item_text(&restored[0])
            .unwrap()
            .contains("Use Helix for quick edits")
    );
}

#[test]
fn local_memory_context_rejects_unbounded_or_invalid_items() {
    let too_many = vec![AIAgentContext::LocalMemory {
        items: (0..=MAX_CONTEXT_MEMORIES)
            .map(|_| memory_item("bounded"))
            .collect(),
    }];
    let invalid_revision = vec![AIAgentContext::LocalMemory {
        items: vec![LocalMemoryContextItem {
            revision: 0,
            ..memory_item("bounded")
        }],
    }];
    let oversized = vec![AIAgentContext::LocalMemory {
        items: vec![memory_item("x".repeat(MAX_CONTEXT_ITEM_CHARS + 1))],
    }];

    for context in [too_many, invalid_revision, oversized] {
        let mut image_count = 0;
        assert!(validate_openai_context(&context, false, &mut image_count).is_err());
    }
}

#[test]
fn restored_local_memory_is_revalidated_before_provider_projection() {
    let items = (0..=MAX_CONTEXT_MEMORIES)
        .map(|_| memory_item("bounded"))
        .collect::<Vec<_>>();
    let context = api::InputContext {
        selected_text: vec![api::input_context::SelectedText {
            text: encode_local_memory_context(&items),
        }],
        ..Default::default()
    };
    let message = api_message_with_context(
        "task",
        "request",
        api::message::Message::UserQuery(api::message::UserQuery {
            query: "use memory".to_string(),
            context: Some(context),
            referenced_attachments: Default::default(),
            mode: None,
            intended_agent: 0,
        }),
        &[],
    );

    assert!(direct_openai_context_from_api_message(&message, false).is_err());
}
