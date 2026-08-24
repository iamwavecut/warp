use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MissedRunPolicyArg {
    Skip,
    RunOnce,
    CatchUp,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ScheduleCadenceArgs {
    /// Run repeatedly after this duration (for example: 15m, 2h, 1d).
    #[arg(long, value_name = "DURATION", conflicts_with = "daily")]
    pub every: Option<humantime::Duration>,

    /// Run once per day at local wall-clock HH:MM.
    #[arg(long, value_name = "HH:MM", conflicts_with = "every")]
    pub daily: Option<String>,
}

#[derive(Debug, Clone, Args)]
#[command(group(
    clap::ArgGroup::new("cadence")
        .required(true)
        .args(["every", "daily"])
))]
pub struct CreateScheduleArgs {
    /// Human-readable schedule name.
    #[arg(long)]
    pub name: String,

    /// UUID or unique exact name of a local named agent.
    #[arg(long, value_name = "ID_OR_NAME")]
    pub agent: String,

    /// Task supplied to the named agent on every run.
    #[arg(long, short = 'p')]
    pub prompt: String,

    #[command(flatten)]
    pub cadence: ScheduleCadenceArgs,

    /// Wall-clock timezone: local, UTC, or a fixed offset such as +02:00.
    #[arg(long, default_value = "local")]
    pub timezone: String,

    /// What to do when the app was not running at the scheduled time.
    #[arg(long, value_enum, default_value_t = MissedRunPolicyArg::RunOnce)]
    pub missed: MissedRunPolicyArg,

    /// Working directory for the local agent.
    #[arg(long = "cwd")]
    pub working_directory: Option<PathBuf>,

    /// Do not show an operating-system notification when a run finishes.
    #[arg(long)]
    pub no_notify: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ScheduleSelectorArgs {
    pub schedule_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct RevisionedScheduleArgs {
    pub schedule_id: String,

    /// Revision returned by `schedule show`.
    #[arg(long, required = true)]
    pub revision: i64,
}

#[derive(Debug, Clone, Args)]
pub struct UpdateScheduleArgs {
    pub schedule_id: String,

    /// Revision returned by `schedule show`.
    #[arg(long, required = true)]
    pub revision: i64,

    #[arg(long)]
    pub name: Option<String>,

    #[arg(long, value_name = "ID_OR_NAME")]
    pub agent: Option<String>,

    #[arg(long, short = 'p')]
    pub prompt: Option<String>,

    #[command(flatten)]
    pub cadence: ScheduleCadenceArgs,

    #[arg(long)]
    pub timezone: Option<String>,

    #[arg(long, value_enum)]
    pub missed: Option<MissedRunPolicyArg>,

    #[arg(long = "cwd", conflicts_with = "clear_working_directory")]
    pub working_directory: Option<PathBuf>,

    #[arg(long = "clear-cwd", conflicts_with = "working_directory")]
    pub clear_working_directory: bool,

    #[arg(long, conflicts_with = "no_notify")]
    pub notify: bool,

    #[arg(long, conflicts_with = "notify")]
    pub no_notify: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ScheduleEventsArgs {
    pub schedule_id: String,

    /// Return events strictly after this journal sequence.
    #[arg(long, default_value_t = 0)]
    pub after: i64,

    #[arg(long, default_value_t = 100)]
    pub limit: usize,

    /// Persist the returned sequence as this local consumer's durable cursor.
    #[arg(long)]
    pub consumer: Option<String>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ScheduleCommand {
    /// Create a durable local schedule.
    Create(CreateScheduleArgs),
    /// List durable local schedules.
    List,
    /// Show one durable local schedule.
    Show(ScheduleSelectorArgs),
    /// Update a schedule using compare-and-swap revision protection.
    Update(UpdateScheduleArgs),
    /// Pause future automatic runs.
    Pause(RevisionedScheduleArgs),
    /// Resume future automatic runs.
    Resume(RevisionedScheduleArgs),
    /// Delete a schedule and keep its audit journal.
    Delete(RevisionedScheduleArgs),
    /// Queue one immediate local run.
    Run(ScheduleSelectorArgs),
    /// Request cancellation of the active local run.
    Cancel(ScheduleSelectorArgs),
    /// Read the durable local event journal.
    Events(ScheduleEventsArgs),
}

impl ScheduleCommand {
    pub(crate) fn as_str_for_tracing(&self) -> &'static str {
        match self {
            Self::Create(_) => "schedule create",
            Self::List => "schedule list",
            Self::Show(_) => "schedule show",
            Self::Update(_) => "schedule update",
            Self::Pause(_) => "schedule pause",
            Self::Resume(_) => "schedule resume",
            Self::Delete(_) => "schedule delete",
            Self::Run(_) => "schedule run",
            Self::Cancel(_) => "schedule cancel",
            Self::Events(_) => "schedule events",
        }
    }
}
