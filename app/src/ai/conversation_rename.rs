use warpui::{SingletonEntity, View, ViewContext};

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::blocklist::BlocklistAIHistoryModel;
use crate::ai::blocklist::history_model::LocalConversationRenameOutcome;
use crate::view_components::DismissibleToast;
use crate::workspace::ToastStack;

pub(crate) const CONVERSATION_TITLE_MAX_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ConversationTitleValidationError {
    #[error("Please provide a conversation title")]
    Empty,
    #[error("Conversation title must be {max_chars} characters or fewer")]
    TooLong { max_chars: usize },
}

/// Trims and validates a requested conversation title by Unicode scalar count.
pub(crate) fn validate_conversation_title(
    title: String,
) -> Result<String, ConversationTitleValidationError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ConversationTitleValidationError::Empty);
    }
    if title.chars().count() > CONVERSATION_TITLE_MAX_CHARS {
        return Err(ConversationTitleValidationError::TooLong {
            max_chars: CONVERSATION_TITLE_MAX_CHARS,
        });
    }
    Ok(title.to_owned())
}

pub(crate) fn rename_conversation<T: View>(
    conversation_id: AIConversationId,
    title: String,
    ctx: &mut ViewContext<T>,
) -> bool {
    let result = BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
        history.rename_conversation_locally(conversation_id, title, ctx)
    });
    let succeeded = result.is_ok();

    let toast = match result {
        Ok(LocalConversationRenameOutcome::Renamed { title }) => Some(DismissibleToast::success(
            format!("Conversation renamed to {title}"),
        )),
        Ok(LocalConversationRenameOutcome::Unchanged) => None,
        Err(error) => Some(DismissibleToast::error(error.to_string())),
    };

    if let Some(toast) = toast {
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(toast, window_id, ctx);
        });
    }

    succeeded
}

#[cfg(test)]
#[path = "conversation_rename_tests.rs"]
mod tests;
