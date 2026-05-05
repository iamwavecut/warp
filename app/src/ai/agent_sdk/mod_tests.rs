use warp_cli::{
    agent::Harness,
    artifact::{ArtifactCommand, DownloadArtifactArgs, GetArtifactArgs, UploadArtifactArgs},
    task::{MessageCommand, MessageSendArgs, MessageWatchArgs, TaskCommand},
    CliCommand,
};

use super::command_requires_auth;

#[test]
fn logout_does_not_require_auth() {
    assert!(!command_requires_auth(&CliCommand::Logout));
}

#[test]
fn login_does_not_require_auth() {
    assert!(!command_requires_auth(&CliCommand::Login));
}

#[test]
fn artifact_download_requires_auth() {
    assert!(command_requires_auth(&CliCommand::Artifact(
        ArtifactCommand::Download(DownloadArtifactArgs {
            artifact_uid: "artifact-123".to_string(),
            out: None,
        },)
    )));
}

#[test]
fn run_message_send_requires_auth() {
    assert!(command_requires_auth(&CliCommand::Run(
        TaskCommand::Message(MessageCommand::Send(MessageSendArgs {
            to: vec!["run-456".to_string()],
            subject: "subject".to_string(),
            body: "body".to_string(),
            sender_run_id: "run-123".to_string(),
        }),)
    )));
}

#[test]
fn artifact_get_requires_auth() {
    assert!(command_requires_auth(&CliCommand::Artifact(
        ArtifactCommand::Get(GetArtifactArgs {
            artifact_uid: "artifact-123".to_string(),
        },)
    )));
}

#[test]
fn artifact_upload_requires_auth() {
    assert!(command_requires_auth(&CliCommand::Artifact(
        ArtifactCommand::Upload(UploadArtifactArgs {
            path: "artifact.txt".into(),
            run_id: Some("run-123".to_string()),
            conversation_id: None,
            description: None,
        },)
    )));
}
