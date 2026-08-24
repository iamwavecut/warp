use std::time::Duration;

use super::{
    DEFAULT_IDLE_TIMEOUT_SECONDS, WATCHDOG_FLOOR, WATCHDOG_SAFETY_MARGIN, watchdog_timeout,
};

#[test]
fn p2_4_wait_for_events_watchdog_is_bounded_and_local() {
    assert_eq!(DEFAULT_IDLE_TIMEOUT_SECONDS, 30 * 60);
    assert_eq!(WATCHDOG_SAFETY_MARGIN, Duration::from_secs(30));
    assert_eq!(WATCHDOG_FLOOR, Duration::from_secs(5));
    assert_eq!(watchdog_timeout(60), Duration::from_secs(30));
    assert_eq!(watchdog_timeout(10), WATCHDOG_FLOOR);
    assert_eq!(
        watchdog_timeout(0),
        Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECONDS as u64) - WATCHDOG_SAFETY_MARGIN
    );
    assert_eq!(watchdog_timeout(-1), watchdog_timeout(0));
}
