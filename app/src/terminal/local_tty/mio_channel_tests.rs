use super::*;
use mio::{Events, Interest, Poll, Token};
use std::time::Duration;

#[test]
fn reused_channel_wakes_the_replacement_event_loop() {
    let (tx, mut rx) = channel();
    let first = Poll::new().unwrap();
    first
        .registry()
        .register(&mut rx, Token(1), Interest::READABLE)
        .unwrap();
    first.registry().deregister(&mut rx).unwrap();
    drop(first);
    tx.send("pending while the shell restarts").unwrap();
    let mut replacement = Poll::new().unwrap();
    replacement
        .registry()
        .register(&mut rx, Token(2), Interest::READABLE)
        .unwrap();
    let mut events = Events::with_capacity(4);
    replacement
        .poll(&mut events, Some(Duration::from_secs(1)))
        .unwrap();
    assert!(events.iter().any(|event| event.token() == Token(2)));
    assert_eq!(rx.try_recv().unwrap(), "pending while the shell restarts");
    tx.send("next command").unwrap();
    replacement
        .poll(&mut events, Some(Duration::from_secs(1)))
        .unwrap();
    assert!(events.iter().any(|event| event.token() == Token(2)));
    assert_eq!(rx.try_recv().unwrap(), "next command");
}
