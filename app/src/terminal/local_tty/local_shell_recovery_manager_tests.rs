use super::*;

#[test]
fn local_shell_recovery_discards_old_command_bytes_and_honors_shutdown() {
    let (sender, receiver) = mio_channel::channel();
    sender
        .send(Message::Input(b"touch should-not-replay\n".to_vec().into()))
        .unwrap();
    sender.send(Message::ChildExited).unwrap();
    assert!(discard_interrupted_shell_input(&receiver));
    assert!(receiver.try_recv().is_err());
    sender.send(Message::Shutdown).unwrap();
    assert!(!discard_interrupted_shell_input(&receiver));
}
