use std::time::Duration;

use instant::Instant;

use super::sampler::aggregate_state;
use super::{
    BlockActivity, LrcActivity, LrcActivityMonitor, LrcProcessActivity, LrcProcessState, PidSample,
    ProcessSample,
};
use crate::terminal::model::block::BlockId;

fn sample(pids: &[(u32, u64, u64)], state: LrcProcessState) -> ProcessSample {
    ProcessSample {
        per_pid: pids
            .iter()
            .map(|(pid, cpu_ms, io_write_bytes)| PidSample {
                pid: *pid,
                cpu_ms: *cpu_ms,
                io_write_bytes: *io_write_bytes,
            })
            .collect(),
        state,
    }
}

#[test]
fn no_report_is_fabricated_before_the_process_tree_is_sampled() {
    let start = Instant::now();
    let mut activity = BlockActivity::new(start);

    assert!(activity.take_report(start).is_none());
}

#[test]
fn cpu_io_and_process_churn_are_local_activity() {
    let start = Instant::now();
    let mut activity = BlockActivity::new(start);
    activity.apply_sample(
        sample(&[(100, 1_000, 0)], LrcProcessState::Running),
        start + Duration::from_secs(1),
    );
    activity.apply_sample(
        sample(
            &[(100, 1_750, 4_096), (101, 0, 0)],
            LrcProcessState::Running,
        ),
        start + Duration::from_secs(3),
    );

    let report = activity
        .take_report(start + Duration::from_secs(4))
        .expect("sampled activity");
    assert_eq!(report.process.cpu_time_delta, Duration::from_millis(750));
    assert_eq!(report.process.io_write_bytes_delta, 4_096);
    assert_eq!(report.process.live_process_count, 2);
    assert_eq!(report.since_last_activity, Duration::from_secs(1));
}

#[test]
fn per_report_deltas_reset_without_erasing_liveness() {
    let start = Instant::now();
    let mut activity = BlockActivity::new(start);
    activity.apply_sample(
        sample(&[(100, 0, 0)], LrcProcessState::Sleeping),
        start + Duration::from_secs(1),
    );
    activity.apply_sample(
        sample(&[(100, 500, 2_048)], LrcProcessState::Running),
        start + Duration::from_secs(2),
    );
    let first = activity
        .take_report(start + Duration::from_secs(2))
        .unwrap();
    assert_eq!(first.process.cpu_time_delta, Duration::from_millis(500));

    let second = activity
        .take_report(start + Duration::from_secs(5))
        .unwrap();
    assert_eq!(second.process.cpu_time_delta, Duration::ZERO);
    assert_eq!(second.process.io_write_bytes_delta, 0);
    assert_eq!(second.since_last_activity, Duration::from_secs(3));
}

#[test]
fn a_sampled_quiet_or_exited_tree_is_still_reported() {
    let start = Instant::now();
    let mut activity = BlockActivity::new(start);
    activity.apply_sample(
        sample(&[(100, 0, 0)], LrcProcessState::Sleeping),
        start + Duration::from_secs(1),
    );
    activity.apply_sample(
        sample(&[], LrcProcessState::Unknown),
        start + Duration::from_secs(2),
    );

    let report = activity
        .take_report(start + Duration::from_secs(4))
        .unwrap();
    assert_eq!(report.process.live_process_count, 0);
    assert_eq!(report.process.cpu_time_delta, Duration::ZERO);
}

#[test]
fn activity_annotation_is_readable_by_openai_compatible_models() {
    let activity = LrcActivity {
        since_last_activity: Duration::from_millis(1_500),
        process: LrcProcessActivity {
            cpu_time_delta: Duration::from_millis(2_750),
            state: LrcProcessState::Running,
            live_process_count: 3,
            io_write_bytes_delta: 4_096,
        },
    };

    assert_eq!(
        activity.annotation(),
        "[local process activity: running; 3 live processes; CPU +2750 ms; writes +4096 bytes; last activity 1.5 s ago]"
    );
}

#[test]
fn remote_commands_neither_report_activity_nor_start_the_sampler() {
    let monitor = LrcActivityMonitor::new();
    monitor.set_monitoring_enabled(false);

    assert!(!monitor.arm());
    assert!(monitor.report(&BlockId::new()).is_none());
}

#[test]
fn a_local_command_starts_sampling_without_fabricating_an_initial_report() {
    let monitor = LrcActivityMonitor::new();
    monitor.set_monitoring_enabled(true);

    assert!(monitor.arm());
    assert!(monitor.report(&BlockId::new()).is_none());
}

#[test]
fn aggregate_state_prefers_the_strongest_evidence_of_progress() {
    assert_eq!(
        aggregate_state(&[LrcProcessState::Sleeping, LrcProcessState::Running]),
        LrcProcessState::Running
    );
    assert_eq!(
        aggregate_state(&[LrcProcessState::Sleeping, LrcProcessState::DiskWait]),
        LrcProcessState::DiskWait
    );
    assert_eq!(aggregate_state(&[]), LrcProcessState::Unknown);
}
