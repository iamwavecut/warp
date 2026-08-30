//! Local process liveness for long-running shell commands.

mod sampler;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use instant::Instant;
use parking_lot::{FairMutex, Mutex};

use self::sampler::Sampler;
use crate::terminal::TerminalModel;
use crate::terminal::model::block::BlockId;

pub(super) const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum LrcProcessState {
    Running,
    DiskWait,
    Sleeping,
    Stopped,
    Zombie,
    #[default]
    Unknown,
}

impl fmt::Display for LrcProcessState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Running => "running",
            Self::DiskWait => "waiting for disk",
            Self::Sleeping => "sleeping",
            Self::Stopped => "stopped",
            Self::Zombie => "exited",
            Self::Unknown => "unknown",
        };
        formatter.write_str(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LrcActivity {
    pub since_last_activity: Duration,
    pub process: LrcProcessActivity,
}

impl LrcActivity {
    pub(super) fn annotation(&self) -> String {
        format!(
            "[local process activity: {}; {} live processes; CPU +{} ms; writes +{} bytes; last activity {:.1} s ago]",
            self.process.state,
            self.process.live_process_count,
            self.process.cpu_time_delta.as_millis(),
            self.process.io_write_bytes_delta,
            self.since_last_activity.as_secs_f64(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LrcProcessActivity {
    pub cpu_time_delta: Duration,
    pub state: LrcProcessState,
    pub live_process_count: u32,
    pub io_write_bytes_delta: u64,
}

struct BlockActivity {
    process: ProcessTier,
    last_activity: Instant,
}

#[derive(Default)]
struct ProcessTier {
    cpu_ms_by_pid: HashMap<u32, u64>,
    io_write_bytes_by_pid: HashMap<u32, u64>,
    cpu_ms_since_report: u64,
    io_write_bytes_since_report: u64,
    state: LrcProcessState,
    live_process_count: u32,
    sampled: bool,
}

#[derive(Clone)]
struct ProcessSample {
    per_pid: Vec<PidSample>,
    state: LrcProcessState,
}

#[derive(Clone)]
struct PidSample {
    pid: u32,
    cpu_ms: u64,
    io_write_bytes: u64,
}

#[derive(Default)]
pub(super) struct LrcActivityMonitor {
    state: Mutex<MonitorState>,
    sampler: Sampler,
}

#[derive(Default)]
struct MonitorState {
    blocks: HashMap<BlockId, BlockActivity>,
    armed_actions: usize,
    sampler_running: bool,
    monitoring_enabled: bool,
}

impl LrcActivityMonitor {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn set_monitoring_enabled(&self, enabled: bool) {
        self.state.lock().monitoring_enabled = enabled;
    }

    pub(super) fn arm(&self) -> bool {
        let mut state = self.state.lock();
        state.armed_actions += 1;
        if !state.monitoring_enabled || state.sampler_running {
            return false;
        }
        state.sampler_running = true;
        true
    }

    pub(super) fn disarm(&self) {
        let mut state = self.state.lock();
        state.armed_actions = state.armed_actions.saturating_sub(1);
    }

    pub(super) fn report(&self, block_id: &BlockId) -> Option<LrcActivity> {
        let now = Instant::now();
        let mut state = self.state.lock();
        if !state.monitoring_enabled {
            return None;
        }
        state
            .blocks
            .entry(block_id.clone())
            .or_insert_with(|| BlockActivity::new(now))
            .take_report(now)
    }

    pub(super) fn forget(&self, block_id: &BlockId) {
        self.state.lock().blocks.remove(block_id);
    }

    pub(super) fn sample(&self, terminal_model: &Arc<FairMutex<TerminalModel>>) -> bool {
        let tracked: Vec<BlockId> = self.state.lock().blocks.keys().cloned().collect();
        let (live, finished, shell_process) = {
            let model = terminal_model.lock();
            let mut live = Vec::new();
            let mut finished = Vec::new();
            for block_id in tracked {
                match model.block_list().block_with_id(&block_id) {
                    Some(block) if !block.finished() => live.push(block_id),
                    Some(_) | None => finished.push(block_id),
                }
            }
            (live, finished, model.shell_process_info().copied())
        };

        let process_sample = self.sampler.collect(shell_process.as_ref());
        let now = Instant::now();
        let mut state = self.state.lock();
        for block_id in finished {
            state.blocks.remove(&block_id);
        }
        if let Some(sample) = process_sample {
            for block_id in live {
                if let Some(activity) = state.blocks.get_mut(&block_id) {
                    activity.apply_sample(sample.clone(), now);
                }
            }
        }

        let keep_sampling = !state.blocks.is_empty() || state.armed_actions > 0;
        state.sampler_running = keep_sampling;
        drop(state);
        if !keep_sampling {
            self.sampler.reset();
        }
        keep_sampling
    }
}

impl BlockActivity {
    fn new(now: Instant) -> Self {
        Self {
            process: ProcessTier::default(),
            last_activity: now,
        }
    }

    fn apply_sample(&mut self, sample: ProcessSample, now: Instant) {
        let mut cpu_ms_by_pid = HashMap::with_capacity(sample.per_pid.len());
        let mut io_write_bytes_by_pid = HashMap::with_capacity(sample.per_pid.len());
        let mut cpu_delta = 0u64;
        let mut io_delta = 0u64;

        for pid_sample in &sample.per_pid {
            if let Some(previous) = self.process.cpu_ms_by_pid.get(&pid_sample.pid) {
                cpu_delta = cpu_delta.saturating_add(pid_sample.cpu_ms.saturating_sub(*previous));
            }
            if let Some(previous) = self.process.io_write_bytes_by_pid.get(&pid_sample.pid) {
                io_delta =
                    io_delta.saturating_add(pid_sample.io_write_bytes.saturating_sub(*previous));
            }
            cpu_ms_by_pid.insert(pid_sample.pid, pid_sample.cpu_ms);
            io_write_bytes_by_pid.insert(pid_sample.pid, pid_sample.io_write_bytes);
        }
        let pid_set_changed = cpu_ms_by_pid.len() != self.process.cpu_ms_by_pid.len()
            || cpu_ms_by_pid
                .keys()
                .any(|pid| !self.process.cpu_ms_by_pid.contains_key(pid));

        self.process.cpu_ms_by_pid = cpu_ms_by_pid;
        self.process.io_write_bytes_by_pid = io_write_bytes_by_pid;
        self.process.cpu_ms_since_report =
            self.process.cpu_ms_since_report.saturating_add(cpu_delta);
        self.process.io_write_bytes_since_report = self
            .process
            .io_write_bytes_since_report
            .saturating_add(io_delta);
        self.process.state = sample.state;
        self.process.live_process_count = sample.per_pid.len().try_into().unwrap_or(u32::MAX);
        self.process.sampled = true;

        if cpu_delta > 0 || io_delta > 0 || pid_set_changed {
            self.last_activity = now;
        }
    }

    fn take_report(&mut self, now: Instant) -> Option<LrcActivity> {
        if !self.process.sampled {
            return None;
        }

        let report = LrcActivity {
            since_last_activity: now.saturating_duration_since(self.last_activity),
            process: LrcProcessActivity {
                cpu_time_delta: Duration::from_millis(self.process.cpu_ms_since_report),
                state: self.process.state,
                live_process_count: self.process.live_process_count,
                io_write_bytes_delta: self.process.io_write_bytes_since_report,
            },
        };
        self.process.cpu_ms_since_report = 0;
        self.process.io_write_bytes_since_report = 0;
        Some(report)
    }
}

#[cfg(test)]
#[path = "lrc_activity_tests.rs"]
mod tests;
