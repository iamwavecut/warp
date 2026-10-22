//! Local ACP steering; queued corrections never replay the interrupted prompt.
use super::{
    connection::AcpConnection,
    protocol::{CancelParams, METHOD_SESSION_CANCEL},
};
use crate::ai::blocklist::ExternalHarnessPrompt;
use std::collections::VecDeque;

pub(super) const CANCEL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Default)]
pub(super) struct Cancellation {
    requested: bool,
    explicit: bool,
    retries: usize,
}

impl Cancellation {
    pub(super) fn follow_up(&mut self) -> bool {
        if self.requested {
            return false;
        }
        self.requested = true;
        true
    }
    pub(super) fn interrupt(&mut self) -> bool {
        if self.explicit {
            return false;
        }
        self.requested = true;
        self.explicit = true;
        true
    }
    pub(super) fn expire(&self, queue: &mut QueuedPrompts) -> bool {
        if self.explicit {
            queue.clear();
        }
        self.explicit
    }
    pub(super) fn next_retry_delay(&mut self) -> Option<std::time::Duration> {
        if !self.requested || self.explicit || self.retries >= 2 {
            return None;
        }
        let delay = std::time::Duration::from_secs(1 << self.retries);
        self.retries += 1;
        Some(delay)
    }
    pub(super) fn is_requested(&self) -> bool {
        self.requested
    }
}

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
    pub(super) fn collect_ready(
        &mut self,
        receiver: &async_channel::Receiver<ExternalHarnessPrompt>,
    ) {
        for _ in 0..receiver.len() {
            if let Ok(prompt) = receiver.try_recv() {
                self.push(prompt);
            } else {
                break;
            }
        }
    }
    pub(super) fn take(&mut self, interrupted: bool) -> Option<ExternalHarnessPrompt> {
        if self.0.is_empty() {
            return None;
        }
        let mut text = Vec::new();
        if interrupted {
            text.push("[The user interrupted your previous turn with this message.]".to_owned());
        }
        let mut attachments = Vec::new();
        for prompt in self.0.drain(..self.0.len().min(32)) {
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
