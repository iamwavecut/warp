//! Local ACP steering; queued corrections never replay the interrupted prompt.
use super::{
    connection::AcpConnection,
    protocol::{CancelParams, METHOD_SESSION_CANCEL},
};
use crate::ai::blocklist::ExternalHarnessPrompt;
use std::collections::VecDeque;

pub(super) const CANCEL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Default)]
pub(super) struct QueuedPrompts(VecDeque<ExternalHarnessPrompt>);
impl QueuedPrompts {
    pub(super) fn push(&mut self, prompt: ExternalHarnessPrompt) {
        self.0.push_back(prompt);
    }
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
    pub(super) fn take(&mut self) -> Option<ExternalHarnessPrompt> {
        if self.0.is_empty() {
            return None;
        }
        let mut text =
            vec!["[The user interrupted your previous turn with this message.]".to_owned()];
        let mut attachments = Vec::new();
        for prompt in self.0.drain(..) {
            text.push(prompt.text);
            attachments.extend(prompt.attachments);
        }
        Some(ExternalHarnessPrompt {
            text: text.join("\n\n"),
            attachments,
        })
    }
}

pub(super) async fn cancel_turn(
    connection: &AcpConnection,
    session_id: &str,
) -> anyhow::Result<()> {
    connection
        .notify(
            METHOD_SESSION_CANCEL,
            CancelParams {
                session_id: session_id.to_owned(),
            },
        )
        .await?;
    connection
        .cancel_pending_permission_requests(session_id)
        .await;
    Ok(())
}

#[cfg(test)]
#[path = "steering_tests.rs"]
mod tests;
