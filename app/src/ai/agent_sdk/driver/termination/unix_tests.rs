use std::{
    fs, io::Write as _, os::unix::process::ExitStatusExt as _, process::Command, time::Duration,
};

use futures::executor::block_on;
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use tempfile::TempDir;

use super::InterruptWatch;
use crate::ai::agent_sdk::driver::termination::Interrupt;

/// Set on a re-executed child so it runs only the signal lifecycle body.
const SIGNAL_CHILD_ENV: &str = "WARP_AGENT_DRIVER_SIGNAL_CHILD";
const READY_MARKER: &str = "warp-signal-test: ready";
const SHUTDOWN_MARKER: &str = "warp-signal-test: shutdown_started";
const STUCK_SHUTDOWN_BAILOUT: Duration = Duration::from_secs(3);

fn signal_lifecycle_child() -> ! {
    let kind = std::env::var(SIGNAL_CHILD_ENV).expect("child kind");
    let expected = match kind.as_str() {
        "term" | "disarm" => Interrupt::Terminate,
        "int" | "int-hang" => Interrupt::Interrupt,
        other => panic!("unknown child kind {other}"),
    };

    let mut watch = InterruptWatch::register().expect("signal watch");
    if kind == "disarm" {
        watch.disarm();
        println!("{READY_MARKER}");
        std::io::stdout().flush().expect("flush ready marker");
        std::thread::sleep(STUCK_SHUTDOWN_BAILOUT);
        std::process::exit(1);
    }
    println!("{READY_MARKER}");
    std::io::stdout().flush().expect("flush ready marker");

    let interrupt = block_on(watch.wait());
    assert_eq!(interrupt, expected);

    if kind == "int-hang" {
        println!("{SHUTDOWN_MARKER}");
        std::io::stdout().flush().expect("flush shutdown marker");
        std::thread::sleep(STUCK_SHUTDOWN_BAILOUT);
        std::process::exit(1);
    }
    watch.terminate(interrupt);
}

/// Run signal handling in an isolated child: signal dispositions are process-wide and must never
/// be changed in the test runner itself.
fn spawn_signal_lifecycle_child(
    kind: &str,
    test_name: &str,
    first_signal: Signal,
    second_signal: Option<Signal>,
    expected_signal: Signal,
    expect_stuck_shutdown: bool,
) {
    if std::env::var_os(SIGNAL_CHILD_ENV).is_some() {
        signal_lifecycle_child();
    }

    let temp_dir = TempDir::new().expect("temporary signal-test directory");
    let stdout_path = temp_dir.path().join("stdout");
    let stderr_path = temp_dir.path().join("stderr");
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .arg(
            test_name
                .strip_prefix("warp::")
                .expect("crate-qualified test name"),
        )
        .arg("--exact")
        .arg("--nocapture")
        .env(SIGNAL_CHILD_ENV, kind)
        .env("RUST_TEST_THREADS", "1")
        .stdout(fs::File::create(&stdout_path).expect("child stdout"))
        .stderr(fs::File::create(&stderr_path).expect("child stderr"));
    for (key, _) in std::env::vars() {
        if key.starts_with("NEXTEST") {
            command.env_remove(key);
        }
    }
    let mut child = command.spawn().expect("spawn signal child");

    let wait_for_marker = |child: &mut std::process::Child, marker: &str| {
        for _ in 0..500 {
            if fs::read_to_string(&stdout_path)
                .unwrap_or_default()
                .contains(marker)
            {
                return;
            }
            if let Some(status) = child.try_wait().expect("poll signal child") {
                panic!(
                    "signal child exited before {marker:?}: status={status:?} stdout={} stderr={}",
                    fs::read_to_string(&stdout_path).unwrap_or_default(),
                    fs::read_to_string(&stderr_path).unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!(
            "signal child never printed {marker:?}; stdout={} stderr={}",
            fs::read_to_string(&stdout_path).unwrap_or_default(),
            fs::read_to_string(&stderr_path).unwrap_or_default()
        );
    };

    wait_for_marker(&mut child, READY_MARKER);
    let child_pid = Pid::from_raw(i32::try_from(child.id()).expect("child pid"));
    kill(child_pid, first_signal).expect("send first signal");

    if second_signal.is_some() {
        wait_for_marker(&mut child, SHUTDOWN_MARKER);
    }
    if let Some(signal) = second_signal {
        kill(child_pid, signal).expect("send second signal");
    }

    let status = (0..500)
        .find_map(|_| {
            let status = child.try_wait().expect("poll signal child");
            if status.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
            status
        })
        .unwrap_or_else(|| {
            let _ = child.kill();
            let _ = child.wait();
            panic!("signal child did not terminate within five seconds");
        });
    let stdout = fs::read_to_string(&stdout_path).unwrap_or_default();
    assert_eq!(
        status.signal(),
        Some(expected_signal as i32),
        "status={status:?} stdout={stdout} stderr={}",
        fs::read_to_string(&stderr_path).unwrap_or_default()
    );
    assert_eq!(
        stdout.contains(SHUTDOWN_MARKER),
        expect_stuck_shutdown,
        "stdout={stdout}"
    );
}

#[test]
fn sigterm_subprocess_exits_signaled() {
    spawn_signal_lifecycle_child(
        "term",
        concat!(module_path!(), "::sigterm_subprocess_exits_signaled"),
        Signal::SIGTERM,
        None,
        Signal::SIGTERM,
        false,
    );
}

#[test]
fn sigint_subprocess_exits_signaled() {
    spawn_signal_lifecycle_child(
        "int",
        concat!(module_path!(), "::sigint_subprocess_exits_signaled"),
        Signal::SIGINT,
        None,
        Signal::SIGINT,
        false,
    );
}

#[test]
fn second_sigint_kills_during_stuck_shutdown() {
    spawn_signal_lifecycle_child(
        "int-hang",
        concat!(
            module_path!(),
            "::second_sigint_kills_during_stuck_shutdown"
        ),
        Signal::SIGINT,
        Some(Signal::SIGINT),
        Signal::SIGINT,
        true,
    );
}

#[test]
fn different_second_signal_kills_during_stuck_shutdown() {
    spawn_signal_lifecycle_child(
        "int-hang",
        concat!(
            module_path!(),
            "::different_second_signal_kills_during_stuck_shutdown"
        ),
        Signal::SIGINT,
        Some(Signal::SIGTERM),
        Signal::SIGTERM,
        true,
    );
}

#[test]
fn disarmed_watch_does_not_swallow_shutdown_signals() {
    spawn_signal_lifecycle_child(
        "disarm",
        concat!(
            module_path!(),
            "::disarmed_watch_does_not_swallow_shutdown_signals"
        ),
        Signal::SIGTERM,
        None,
        Signal::SIGTERM,
        false,
    );
}
