//! OS process-table sampling for local long-running commands.

use crate::terminal::model::terminal_model::ShellProcessInfo;

cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        use super::ProcessSample;

        #[derive(Default)]
        pub(super) struct Sampler;

        impl Sampler {
            pub(super) fn collect(&self, _shell: Option<&ShellProcessInfo>) -> Option<ProcessSample> {
                None
            }

            pub(super) fn reset(&self) {}
        }
    } else {
        use std::collections::HashSet;

        use parking_lot::Mutex;
        use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};

        use super::{LrcProcessState, PidSample, ProcessSample};

        #[derive(Default)]
        pub(super) struct Sampler {
            system: Mutex<System>,
        }

        impl Sampler {
            pub(super) fn collect(&self, shell: Option<&ShellProcessInfo>) -> Option<ProcessSample> {
                let shell = shell?;
                let shell_pid = Pid::from_u32(shell.pid);
                let mut system = self.system.lock();
                system.refresh_processes_specifics(
                    ProcessesToUpdate::All,
                    true,
                    ProcessRefreshKind::nothing(),
                );
                let pids = command_process_tree(&system, shell_pid, foreground_pgid(shell));
                system.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&pids),
                    true,
                    ProcessRefreshKind::nothing().with_cpu().with_disk_usage(),
                );

                let mut per_pid = Vec::with_capacity(pids.len());
                let mut states = Vec::with_capacity(pids.len());
                for pid in pids {
                    let Some(process) = system.process(pid) else {
                        continue;
                    };
                    per_pid.push(PidSample {
                        pid: pid.as_u32(),
                        cpu_ms: process.accumulated_cpu_time(),
                        io_write_bytes: process.disk_usage().total_written_bytes,
                    });
                    states.push(process_state(process.status()));
                }
                Some(ProcessSample {
                    state: aggregate_state(&states),
                    per_pid,
                })
            }

            pub(super) fn reset(&self) {
                *self.system.lock() = System::new();
            }
        }

        pub(super) fn aggregate_state(states: &[LrcProcessState]) -> LrcProcessState {
            for candidate in [
                LrcProcessState::Running,
                LrcProcessState::DiskWait,
                LrcProcessState::Sleeping,
                LrcProcessState::Stopped,
                LrcProcessState::Zombie,
            ] {
                if states.contains(&candidate) {
                    return candidate;
                }
            }
            LrcProcessState::Unknown
        }

        fn process_state(status: ProcessStatus) -> LrcProcessState {
            match status {
                ProcessStatus::Run | ProcessStatus::Waking => LrcProcessState::Running,
                ProcessStatus::UninterruptibleDiskSleep => LrcProcessState::DiskWait,
                ProcessStatus::Sleep | ProcessStatus::Idle | ProcessStatus::Parked => {
                    LrcProcessState::Sleeping
                }
                ProcessStatus::Stop | ProcessStatus::Tracing | ProcessStatus::LockBlocked => {
                    LrcProcessState::Stopped
                }
                ProcessStatus::Zombie | ProcessStatus::Dead | ProcessStatus::Wakekill => {
                    LrcProcessState::Zombie
                }
                ProcessStatus::Unknown(_) => LrcProcessState::Unknown,
            }
        }

        pub(super) fn command_process_tree(
            system: &System,
            shell_pid: Pid,
            foreground_pgid: Option<u32>,
        ) -> Vec<Pid> {
            let descendants = descendants_of(system, shell_pid);
            let Some(pgid) = foreground_pgid else {
                return descendants.into_iter().collect();
            };
            let mut foreground: Vec<Pid> = descendants
                .iter()
                .filter(|pid| process_group_of(**pid) == Some(pgid))
                .copied()
                .collect();
            if process_group_of(shell_pid) == Some(pgid) {
                foreground.push(shell_pid);
            }
            if foreground.is_empty() {
                return descendants.into_iter().collect();
            }
            foreground
        }

        fn descendants_of(system: &System, pid: Pid) -> HashSet<Pid> {
            let mut descendants = HashSet::new();
            loop {
                let mut added = false;
                for (candidate, process) in system.processes() {
                    if descendants.contains(candidate) {
                        continue;
                    }
                    let Some(parent) = process.parent() else {
                        continue;
                    };
                    if parent == pid || descendants.contains(&parent) {
                        descendants.insert(*candidate);
                        added = true;
                    }
                }
                if !added {
                    return descendants;
                }
            }
        }

        #[cfg(unix)]
        pub(super) fn process_group_of(pid: Pid) -> Option<u32> {
            // SAFETY: getpgid only reads process scheduling metadata.
            let pgid = unsafe { libc::getpgid(pid.as_u32() as libc::pid_t) };
            (pgid > 0).then_some(pgid as u32)
        }

        #[cfg(not(unix))]
        pub(super) fn process_group_of(_pid: Pid) -> Option<u32> {
            None
        }

        #[cfg(unix)]
        fn foreground_pgid(shell: &ShellProcessInfo) -> Option<u32> {
            let fd = shell.pty_leader_fd?;
            // SAFETY: tcgetpgrp only reads terminal state for the live pty descriptor.
            let pgid = unsafe { libc::tcgetpgrp(fd) };
            (pgid > 0).then_some(pgid as u32)
        }

        #[cfg(not(unix))]
        fn foreground_pgid(_shell: &ShellProcessInfo) -> Option<u32> {
            None
        }
    }
}
