use super::{ConversationTitleValidationError, validate_conversation_title};

#[test]
fn validate_conversation_title_trims_surrounding_whitespace() {
    assert_eq!(
        validate_conversation_title("  Local title\n".to_owned()),
        Ok("Local title".to_owned()),
    );
}

#[test]
fn validate_conversation_title_rejects_empty_trimmed_title() {
    assert_eq!(
        validate_conversation_title(" \n\t ".to_owned()),
        Err(ConversationTitleValidationError::Empty),
    );
}

#[test]
fn validate_conversation_title_accepts_500_unicode_scalar_values() {
    let title = "🦀".repeat(500);

    assert_eq!(validate_conversation_title(title.clone()), Ok(title));
}

#[test]
fn validate_conversation_title_rejects_501_unicode_scalar_values() {
    assert_eq!(
        validate_conversation_title("🦀".repeat(501)),
        Err(ConversationTitleValidationError::TooLong { max_chars: 500 }),
    );
}
