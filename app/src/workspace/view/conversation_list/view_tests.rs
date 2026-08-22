use super::*;

#[test]
fn conversation_list_inline_rename_state_starts_finishes_and_cancels() {
    let conversation_id = AIConversationId::new();
    let mut state = InlineConversationRenameState::default();

    assert!(!state.is_renaming(conversation_id));
    state.start(conversation_id);
    assert!(state.is_renaming(conversation_id));
    assert_eq!(state.finish(), Some(conversation_id));
    assert!(!state.is_renaming(conversation_id));

    state.start(conversation_id);
    state.cancel();
    assert!(!state.is_renaming(conversation_id));
    assert_eq!(state.finish(), None);
}
