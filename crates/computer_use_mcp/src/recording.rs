//! Screen recordings of a computer-use session, published when they stop.

use std::path::PathBuf;

use computer_use::{Recorder, RecordingCompletionStatus, RecordingHandle};
use serde_json::{Value, json};

use crate::{ToolError, publish};

/// Where finished recordings are published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingDestination {
    /// Moved into a local directory.
    Directory(PathBuf),
}

pub(crate) enum RecordingFinalizeReason {
    StoppedByAgent,
    RunEnded,
}

pub(crate) struct ActiveRecording {
    id: String,
    handle: RecordingHandle,
    title: Option<String>,
    description: Option<String>,
}

impl ActiveRecording {
    pub fn new(
        id: String,
        handle: RecordingHandle,
        title: Option<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            id,
            handle,
            title,
            description,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Stops `recording` and publishes its video to `destination`, returning a summary for the model.
pub(crate) async fn finish(
    recorder: &dyn Recorder,
    recording: ActiveRecording,
    destination: &RecordingDestination,
    reason: RecordingFinalizeReason,
) -> Result<Value, ToolError> {
    let ActiveRecording {
        handle,
        title,
        description,
        ..
    } = recording;
    let output = recorder
        .stop(handle)
        .await
        .map_err(|error| format!("Failed to stop the recording: {error}"))?;
    let RecordingDestination::Directory(directory) = destination;
    let saved = publish::move_into(&output.path, directory)
        .await
        .map_err(|error| {
            format!(
                "{error} The recording was kept at {}.",
                output.path.display()
            )
        });
    let mut summary =
        json!({"status": "saved", "path": saved?, "title": title, "description": description});
    summary["duration_secs"] = json!(output.duration.as_secs_f64());
    summary["width_px"] = json!(output.width);
    summary["height_px"] = json!(output.height);
    summary["termination_reason"] = json!(match (reason, output.completion_status) {
        (_, RecordingCompletionStatus::StoppedEarly) => "Recording stopped early",
        (RecordingFinalizeReason::StoppedByAgent, _) => "Stopped by agent",
        (RecordingFinalizeReason::RunEnded, _) => "Session ended",
    });
    Ok(summary)
}
