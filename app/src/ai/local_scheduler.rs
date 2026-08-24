//! Durable, local-only scheduling for named agents.
//!
//! The repository, journal, process supervisor, and OS notifications in this
//! module are deliberately independent from Warp's hosted ambient-agent APIs.

#![cfg(not(target_family = "wasm"))]

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use command::r#async::Command as AsyncCommand;
use diesel::connection::SimpleConnection;
use diesel::sql_types::{BigInt, Integer, Nullable, Text};
use diesel::{Connection, QueryableByName, RunQueryDsl, SqliteConnection, sql_query};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::notification::UserNotification;
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::workspace::Workspace;

const MAX_SCHEDULE_COUNT: i64 = 512;
const MAX_NAME_CHARS: usize = 160;
const MAX_PROMPT_CHARS: usize = 32_000;
const MAX_EVENT_DETAIL_CHARS: usize = 16_000;
const MIN_INTERVAL_SECONDS: i64 = 60;
const MAX_INTERVAL_SECONDS: i64 = 366 * 24 * 60 * 60;
const MISSED_GRACE_MS: i64 = 60_000;
const MAX_CLAIMS_PER_TICK: usize = 4;
const SUPERVISOR_TICK: Duration = Duration::from_secs(1);

const LOCAL_SCHEDULER_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS local_schedules (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    prompt TEXT NOT NULL,
    working_directory TEXT,
    cadence_kind TEXT NOT NULL CHECK (cadence_kind IN ('every', 'daily')),
    cadence_value TEXT NOT NULL,
    timezone TEXT NOT NULL,
    missed_policy TEXT NOT NULL CHECK (missed_policy IN ('skip', 'run_once', 'catch_up')),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    notify INTEGER NOT NULL CHECK (notify IN (0, 1)),
    next_run_at INTEGER NOT NULL,
    last_scheduled_at INTEGER,
    manual_requested INTEGER NOT NULL DEFAULT 0 CHECK (manual_requested IN (0, 1)),
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0, 1)),
    active_run_id TEXT,
    active_started_at INTEGER,
    revision INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS local_schedules_due
    ON local_schedules(enabled, next_run_at, id);
CREATE TABLE IF NOT EXISTS local_schedule_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    schedule_id TEXT NOT NULL,
    run_id TEXT,
    kind TEXT NOT NULL CHECK (
        kind IN ('started', 'completed', 'failed', 'cancelled', 'missed', 'interrupted')
    ),
    scheduled_at INTEGER,
    occurred_at INTEGER NOT NULL,
    detail TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS local_schedule_events_schedule_sequence
    ON local_schedule_events(schedule_id, sequence);
CREATE TABLE IF NOT EXISTS local_schedule_cursors (
    consumer_id TEXT PRIMARY KEY NOT NULL,
    sequence INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MissedRunPolicy {
    Skip,
    RunOnce,
    CatchUp,
}

impl MissedRunPolicy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::RunOnce => "run_once",
            Self::CatchUp => "catch_up",
        }
    }

    fn parse(value: &str) -> Result<Self, LocalSchedulerError> {
        match value {
            "skip" => Ok(Self::Skip),
            "run_once" => Ok(Self::RunOnce),
            "catch_up" => Ok(Self::CatchUp),
            value => Err(LocalSchedulerError::Corrupt(format!(
                "unknown missed-run policy `{value}`"
            ))),
        }
    }
}

impl From<warp_cli::schedule::MissedRunPolicyArg> for MissedRunPolicy {
    fn from(value: warp_cli::schedule::MissedRunPolicyArg) -> Self {
        match value {
            warp_cli::schedule::MissedRunPolicyArg::Skip => Self::Skip,
            warp_cli::schedule::MissedRunPolicyArg::RunOnce => Self::RunOnce,
            warp_cli::schedule::MissedRunPolicyArg::CatchUp => Self::CatchUp,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalScheduleCadence {
    Every { seconds: i64 },
    Daily { hour: u32, minute: u32 },
}

impl LocalScheduleCadence {
    pub fn every(duration: Duration) -> Result<Self, LocalSchedulerError> {
        let seconds = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
        if !(MIN_INTERVAL_SECONDS..=MAX_INTERVAL_SECONDS).contains(&seconds) {
            return Err(LocalSchedulerError::InvalidCadence(format!(
                "interval must be between 1 minute and 366 days"
            )));
        }
        Ok(Self::Every { seconds })
    }

    pub fn daily(value: &str) -> Result<Self, LocalSchedulerError> {
        let (hour_text, minute_text) = value.split_once(':').ok_or_else(|| {
            LocalSchedulerError::InvalidCadence("daily time must be HH:MM".into())
        })?;
        if hour_text.len() != 2 || minute_text.len() != 2 {
            return Err(LocalSchedulerError::InvalidCadence(
                "daily time must be zero-padded HH:MM".into(),
            ));
        }
        let hour = hour_text
            .parse::<u32>()
            .map_err(|_| LocalSchedulerError::InvalidCadence("daily hour is invalid".into()))?;
        let minute = minute_text
            .parse::<u32>()
            .map_err(|_| LocalSchedulerError::InvalidCadence("daily minute is invalid".into()))?;
        if hour > 23 || minute > 59 {
            return Err(LocalSchedulerError::InvalidCadence(
                "daily time must be zero-padded HH:MM".into(),
            ));
        }
        Ok(Self::Daily { hour, minute })
    }

    fn parts(&self) -> (&'static str, String) {
        match self {
            Self::Every { seconds } => ("every", seconds.to_string()),
            Self::Daily { hour, minute } => ("daily", format!("{hour:02}:{minute:02}")),
        }
    }

    fn from_parts(kind: &str, value: &str) -> Result<Self, LocalSchedulerError> {
        match kind {
            "every" => {
                let seconds = value
                    .parse::<i64>()
                    .map_err(|_| LocalSchedulerError::Corrupt("invalid interval cadence".into()))?;
                Self::every(Duration::from_secs(seconds.try_into().map_err(|_| {
                    LocalSchedulerError::Corrupt("negative interval cadence".into())
                })?))
            }
            "daily" => Self::daily(value),
            value => Err(LocalSchedulerError::Corrupt(format!(
                "unknown cadence kind `{value}`"
            ))),
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Every { seconds } => format!(
                "every {}",
                humantime::format_duration(Duration::from_secs(*seconds as u64))
            ),
            Self::Daily { hour, minute } => format!("daily at {hour:02}:{minute:02}"),
        }
    }

    pub fn next_after(
        &self,
        after_millis: i64,
        timezone: &LocalScheduleTimezone,
    ) -> Result<i64, LocalSchedulerError> {
        match self {
            Self::Every { seconds } => after_millis
                .checked_add(seconds.saturating_mul(1_000))
                .ok_or_else(|| LocalSchedulerError::InvalidCadence("next run overflow".into())),
            Self::Daily { hour, minute } => timezone.next_daily_after(after_millis, *hour, *minute),
        }
    }

    fn first_after(
        &self,
        reference_millis: i64,
        timezone: &LocalScheduleTimezone,
    ) -> Result<i64, LocalSchedulerError> {
        self.next_after(reference_millis, timezone)
    }

    fn advance_past(
        &self,
        mut scheduled_at: i64,
        reference_millis: i64,
        timezone: &LocalScheduleTimezone,
    ) -> Result<i64, LocalSchedulerError> {
        let mut iterations = 0usize;
        while scheduled_at <= reference_millis {
            scheduled_at = self.next_after(scheduled_at, timezone)?;
            iterations += 1;
            if iterations > 10_000 {
                return Err(LocalSchedulerError::InvalidCadence(
                    "schedule catch-up exceeds safety bound".into(),
                ));
            }
        }
        Ok(scheduled_at)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalScheduleTimezone {
    Local,
    Utc,
    FixedOffset { seconds_east: i32 },
}

impl LocalScheduleTimezone {
    pub fn parse(value: &str) -> Result<Self, LocalSchedulerError> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("local") {
            return Ok(Self::Local);
        }
        if value.eq_ignore_ascii_case("utc") || value == "Z" {
            return Ok(Self::Utc);
        }
        let sign = match value.as_bytes().first().copied() {
            Some(b'+') => 1,
            Some(b'-') => -1,
            _ => return Err(LocalSchedulerError::InvalidTimezone(value.to_owned())),
        };
        let (hours, minutes) = value[1..]
            .split_once(':')
            .ok_or_else(|| LocalSchedulerError::InvalidTimezone(value.to_owned()))?;
        if hours.len() != 2 || minutes.len() != 2 {
            return Err(LocalSchedulerError::InvalidTimezone(value.to_owned()));
        }
        let hours = hours
            .parse::<i32>()
            .map_err(|_| LocalSchedulerError::InvalidTimezone(value.to_owned()))?;
        let minutes = minutes
            .parse::<i32>()
            .map_err(|_| LocalSchedulerError::InvalidTimezone(value.to_owned()))?;
        if hours > 23 || minutes > 59 {
            return Err(LocalSchedulerError::InvalidTimezone(value.to_owned()));
        }
        Ok(Self::FixedOffset {
            seconds_east: sign * (hours * 3_600 + minutes * 60),
        })
    }

    pub fn display(&self) -> String {
        match self {
            Self::Local => "local".to_owned(),
            Self::Utc => "UTC".to_owned(),
            Self::FixedOffset { seconds_east } => {
                let sign = if *seconds_east < 0 { '-' } else { '+' };
                let absolute = seconds_east.abs();
                format!(
                    "{sign}{:02}:{:02}",
                    absolute / 3_600,
                    (absolute % 3_600) / 60
                )
            }
        }
    }

    fn next_daily_after(
        &self,
        after_millis: i64,
        hour: u32,
        minute: u32,
    ) -> Result<i64, LocalSchedulerError> {
        match self {
            Self::Local => next_daily_in_timezone(&Local, after_millis, hour, minute),
            Self::Utc => next_daily_in_timezone(&Utc, after_millis, hour, minute),
            Self::FixedOffset { seconds_east } => {
                let offset = chrono::FixedOffset::east_opt(*seconds_east)
                    .ok_or_else(|| LocalSchedulerError::InvalidTimezone(self.display()))?;
                next_daily_in_timezone(&offset, after_millis, hour, minute)
            }
        }
    }
}

fn next_daily_in_timezone<T: TimeZone>(
    timezone: &T,
    after_millis: i64,
    hour: u32,
    minute: u32,
) -> Result<i64, LocalSchedulerError> {
    let after = DateTime::<Utc>::from_timestamp_millis(after_millis)
        .ok_or_else(|| LocalSchedulerError::InvalidCadence("invalid reference time".into()))?;
    let local = after.with_timezone(timezone);
    let start_date = NaiveDate::from_ymd_opt(local.year(), local.month(), local.day())
        .ok_or_else(|| LocalSchedulerError::InvalidCadence("invalid local date".into()))?;
    let requested = NaiveTime::from_hms_opt(hour, minute, 0)
        .ok_or_else(|| LocalSchedulerError::InvalidCadence("invalid daily time".into()))?;

    for day_offset in 0..=3 {
        let Some(date) = start_date.checked_add_days(chrono::Days::new(day_offset)) else {
            break;
        };
        let mut candidate = date.and_time(requested);
        // DST gaps are advanced minute-by-minute to the first valid local instant.
        for _ in 0..=180 {
            let resolved = match timezone.from_local_datetime(&candidate) {
                LocalResult::Single(value) => Some(value),
                LocalResult::Ambiguous(first, second) => {
                    Some(if first.timestamp_millis() <= second.timestamp_millis() {
                        first
                    } else {
                        second
                    })
                }
                LocalResult::None => None,
            };
            if let Some(resolved) = resolved {
                let millis = resolved.timestamp_millis();
                if millis > after_millis {
                    return Ok(millis);
                }
                break;
            }
            candidate = candidate
                .checked_add_signed(chrono::Duration::minutes(1))
                .ok_or_else(|| LocalSchedulerError::InvalidCadence("daily time overflow".into()))?;
        }
    }
    Err(LocalSchedulerError::InvalidCadence(
        "could not resolve the next daily run".into(),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSchedule {
    pub id: Uuid,
    pub name: String,
    pub agent_id: Uuid,
    pub prompt: String,
    pub working_directory: Option<PathBuf>,
    pub cadence: LocalScheduleCadence,
    pub timezone: LocalScheduleTimezone,
    pub missed_policy: MissedRunPolicy,
    pub enabled: bool,
    pub notify: bool,
    pub next_run_at: i64,
    pub last_scheduled_at: Option<i64>,
    pub active_run_id: Option<Uuid>,
    pub active_started_at: Option<i64>,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewLocalSchedule {
    pub name: String,
    pub agent_id: Uuid,
    pub prompt: String,
    pub working_directory: Option<PathBuf>,
    pub cadence: LocalScheduleCadence,
    pub timezone: LocalScheduleTimezone,
    pub missed_policy: MissedRunPolicy,
    pub notify: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalScheduleEventKind {
    Started,
    Completed,
    Failed,
    Cancelled,
    Missed,
    Interrupted,
}

impl LocalScheduleEventKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Missed => "missed",
            Self::Interrupted => "interrupted",
        }
    }

    fn parse(value: &str) -> Result<Self, LocalSchedulerError> {
        match value {
            "started" => Ok(Self::Started),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "missed" => Ok(Self::Missed),
            "interrupted" => Ok(Self::Interrupted),
            value => Err(LocalSchedulerError::Corrupt(format!(
                "unknown schedule event `{value}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalScheduleEvent {
    pub sequence: i64,
    pub event_id: Uuid,
    pub schedule_id: Uuid,
    pub run_id: Option<Uuid>,
    pub kind: LocalScheduleEventKind,
    pub scheduled_at: Option<i64>,
    pub occurred_at: i64,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct ClaimedLocalScheduleRun {
    pub schedule: LocalSchedule,
    pub run_id: Uuid,
    pub scheduled_at: i64,
    pub manual: bool,
}

#[derive(Debug, Error)]
pub enum LocalSchedulerError {
    #[error("schedule name cannot be empty")]
    EmptyName,
    #[error("schedule prompt cannot be empty")]
    EmptyPrompt,
    #[error("schedule name exceeds {MAX_NAME_CHARS} characters")]
    NameTooLong,
    #[error("schedule prompt exceeds {MAX_PROMPT_CHARS} characters")]
    PromptTooLong,
    #[error("invalid schedule cadence: {0}")]
    InvalidCadence(String),
    #[error("invalid schedule timezone `{0}`; use local, UTC, or a fixed offset such as +02:00")]
    InvalidTimezone(String),
    #[error("schedule working directory does not exist: {0}")]
    InvalidWorkingDirectory(PathBuf),
    #[error("local schedule limit of {MAX_SCHEDULE_COUNT} has been reached")]
    LimitReached,
    #[error("local schedule {0} was not found")]
    NotFound(Uuid),
    #[error("local schedule {0} already has an active run")]
    AlreadyRunning(Uuid),
    #[error("local schedule {0} has no active run")]
    NotRunning(Uuid),
    #[error(
        "local schedule {id} changed since it was opened (expected revision {expected}, current revision {actual})"
    )]
    Conflict {
        id: Uuid,
        expected: i64,
        actual: i64,
    },
    #[error("local schedule storage error: {0}")]
    Storage(#[source] anyhow::Error),
    #[error("invalid local schedule row: {0}")]
    Corrupt(String),
}

#[derive(Clone)]
pub struct LocalScheduleRepository {
    inner: Rc<RefCell<SqliteConnection>>,
}

#[derive(QueryableByName)]
struct ScheduleDbRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Text)]
    name: String,
    #[diesel(sql_type = Text)]
    agent_id: String,
    #[diesel(sql_type = Text)]
    prompt: String,
    #[diesel(sql_type = Nullable<Text>)]
    working_directory: Option<String>,
    #[diesel(sql_type = Text)]
    cadence_kind: String,
    #[diesel(sql_type = Text)]
    cadence_value: String,
    #[diesel(sql_type = Text)]
    timezone: String,
    #[diesel(sql_type = Text)]
    missed_policy: String,
    #[diesel(sql_type = Integer)]
    enabled: i32,
    #[diesel(sql_type = Integer)]
    notify: i32,
    #[diesel(sql_type = BigInt)]
    next_run_at: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    last_scheduled_at: Option<i64>,
    #[diesel(sql_type = Nullable<Text>)]
    active_run_id: Option<String>,
    #[diesel(sql_type = Nullable<BigInt>)]
    active_started_at: Option<i64>,
    #[diesel(sql_type = BigInt)]
    revision: i64,
    #[diesel(sql_type = BigInt)]
    created_at: i64,
    #[diesel(sql_type = BigInt)]
    updated_at: i64,
}

#[derive(QueryableByName)]
struct EventDbRow {
    #[diesel(sql_type = BigInt)]
    sequence: i64,
    #[diesel(sql_type = Text)]
    event_id: String,
    #[diesel(sql_type = Text)]
    schedule_id: String,
    #[diesel(sql_type = Nullable<Text>)]
    run_id: Option<String>,
    #[diesel(sql_type = Text)]
    kind: String,
    #[diesel(sql_type = Nullable<BigInt>)]
    scheduled_at: Option<i64>,
    #[diesel(sql_type = BigInt)]
    occurred_at: i64,
    #[diesel(sql_type = Text)]
    detail: String,
}

const SCHEDULE_COLUMNS: &str = "id, name, agent_id, prompt, working_directory, cadence_kind, \
cadence_value, timezone, missed_policy, enabled, notify, next_run_at, last_scheduled_at, \
active_run_id, active_started_at, revision, created_at, updated_at";

impl LocalScheduleRepository {
    pub fn open_current_scope() -> Result<Self, LocalSchedulerError> {
        Self::open(crate::persistence::database_file_path_for_current_scope())
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, LocalSchedulerError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|error| {
                storage_error(format!("creating {}: {error}", parent.display()))
            })?;
        }
        let path = path.to_str().ok_or_else(|| {
            storage_error(format!(
                "database path is not valid UTF-8: {}",
                path.display()
            ))
        })?;
        let connection = SqliteConnection::establish(path)
            .map_err(|error| storage_error(format!("opening schedule database: {error}")))?;
        Self::from_connection(connection)
    }

    pub fn in_memory() -> Result<Self, LocalSchedulerError> {
        let connection = SqliteConnection::establish(":memory:")
            .map_err(|error| storage_error(format!("opening in-memory database: {error}")))?;
        Self::from_connection(connection)
    }

    fn from_connection(mut connection: SqliteConnection) -> Result<Self, LocalSchedulerError> {
        connection
            .batch_execute("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")
            .and_then(|_| connection.batch_execute(LOCAL_SCHEDULER_SCHEMA))
            .map_err(|error| storage_error(format!("initializing scheduler schema: {error}")))?;
        Ok(Self {
            inner: Rc::new(RefCell::new(connection)),
        })
    }

    pub fn create(&self, input: NewLocalSchedule) -> Result<LocalSchedule, LocalSchedulerError> {
        let input = validate_new_schedule(input)?;
        self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    #[derive(QueryableByName)]
                    struct CountRow {
                        #[diesel(sql_type = BigInt)]
                        count: i64,
                    }
                    let count = sql_query("SELECT COUNT(*) AS count FROM local_schedules")
                        .get_result::<CountRow>(connection)?
                        .count;
                    if count >= MAX_SCHEDULE_COUNT {
                        return Err(anyhow::Error::new(LocalSchedulerError::LimitReached));
                    }
                    let now = now_millis();
                    let next_run_at = input.cadence.first_after(now, &input.timezone)?;
                    let schedule = LocalSchedule {
                        id: Uuid::new_v4(),
                        name: input.name,
                        agent_id: input.agent_id,
                        prompt: input.prompt,
                        working_directory: input.working_directory,
                        cadence: input.cadence,
                        timezone: input.timezone,
                        missed_policy: input.missed_policy,
                        enabled: true,
                        notify: input.notify,
                        next_run_at,
                        last_scheduled_at: None,
                        active_run_id: None,
                        active_started_at: None,
                        revision: 1,
                        created_at: now,
                        updated_at: now,
                    };
                    insert_schedule(connection, &schedule)?;
                    Ok(schedule)
                })
                .map_err(map_transaction_error)
        })
    }

    pub fn get(&self, id: Uuid) -> Result<Option<LocalSchedule>, LocalSchedulerError> {
        self.with_connection(|connection| load_schedule(connection, id))
    }

    pub fn list(&self) -> Result<Vec<LocalSchedule>, LocalSchedulerError> {
        self.with_connection(|connection| {
            sql_query(format!(
                "SELECT {SCHEDULE_COLUMNS} FROM local_schedules ORDER BY name COLLATE NOCASE, id"
            ))
            .load::<ScheduleDbRow>(connection)
            .map_err(|error| storage_error(format!("listing schedules: {error}")))?
            .into_iter()
            .map(schedule_from_row)
            .collect()
        })
    }

    pub fn update(
        &self,
        expected_revision: i64,
        replacement: NewLocalSchedule,
        id: Uuid,
    ) -> Result<LocalSchedule, LocalSchedulerError> {
        let replacement = validate_new_schedule(replacement)?;
        self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    let current = load_schedule(connection, id)?
                        .ok_or_else(|| anyhow::Error::new(LocalSchedulerError::NotFound(id)))?;
                    ensure_revision(&current, expected_revision)?;
                    let now = now_millis();
                    let next_run_at = if current.cadence == replacement.cadence
                        && current.timezone == replacement.timezone
                    {
                        current.next_run_at
                    } else {
                        replacement.cadence.first_after(now, &replacement.timezone)?
                    };
                    let revision = expected_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("schedule revision overflow"))?;
                    let (cadence_kind, cadence_value) = replacement.cadence.parts();
                    let changed = sql_query(
                        "UPDATE local_schedules SET name = ?, agent_id = ?, prompt = ?, \
                         working_directory = ?, cadence_kind = ?, cadence_value = ?, timezone = ?, \
                         missed_policy = ?, notify = ?, next_run_at = ?, revision = ?, updated_at = ? \
                         WHERE id = ? AND revision = ?",
                    )
                    .bind::<Text, _>(&replacement.name)
                    .bind::<Text, _>(replacement.agent_id.to_string())
                    .bind::<Text, _>(&replacement.prompt)
                    .bind::<Nullable<Text>, _>(path_to_string(replacement.working_directory.as_deref()))
                    .bind::<Text, _>(cadence_kind)
                    .bind::<Text, _>(cadence_value)
                    .bind::<Text, _>(replacement.timezone.display())
                    .bind::<Text, _>(replacement.missed_policy.as_str())
                    .bind::<Integer, _>(i32::from(replacement.notify))
                    .bind::<BigInt, _>(next_run_at)
                    .bind::<BigInt, _>(revision)
                    .bind::<BigInt, _>(now)
                    .bind::<Text, _>(id.to_string())
                    .bind::<BigInt, _>(expected_revision)
                    .execute(connection)?;
                    if changed != 1 {
                        return Err(anyhow::anyhow!("schedule compare-and-swap failed"));
                    }
                    load_schedule(connection, id)?
                        .ok_or_else(|| anyhow::Error::new(LocalSchedulerError::NotFound(id)))
                })
                .map_err(map_transaction_error)
        })
    }

    pub fn set_enabled(
        &self,
        id: Uuid,
        expected_revision: i64,
        enabled: bool,
    ) -> Result<LocalSchedule, LocalSchedulerError> {
        self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    let current = load_schedule(connection, id)?
                        .ok_or_else(|| anyhow::Error::new(LocalSchedulerError::NotFound(id)))?;
                    ensure_revision(&current, expected_revision)?;
                    let now = now_millis();
                    let next_run_at = if enabled && !current.enabled {
                        current.cadence.first_after(now, &current.timezone)?
                    } else {
                        current.next_run_at
                    };
                    let revision = expected_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("schedule revision overflow"))?;
                    let changed = sql_query(
                        "UPDATE local_schedules SET enabled = ?, next_run_at = ?, revision = ?, \
                         updated_at = ? WHERE id = ? AND revision = ?",
                    )
                    .bind::<Integer, _>(i32::from(enabled))
                    .bind::<BigInt, _>(next_run_at)
                    .bind::<BigInt, _>(revision)
                    .bind::<BigInt, _>(now)
                    .bind::<Text, _>(id.to_string())
                    .bind::<BigInt, _>(expected_revision)
                    .execute(connection)?;
                    if changed != 1 {
                        return Err(anyhow::anyhow!("schedule compare-and-swap failed"));
                    }
                    load_schedule(connection, id)?
                        .ok_or_else(|| anyhow::Error::new(LocalSchedulerError::NotFound(id)))
                })
                .map_err(map_transaction_error)
        })
    }

    pub fn delete(&self, id: Uuid, expected_revision: i64) -> Result<(), LocalSchedulerError> {
        self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    let current = load_schedule(connection, id)?
                        .ok_or_else(|| anyhow::Error::new(LocalSchedulerError::NotFound(id)))?;
                    ensure_revision(&current, expected_revision)?;
                    if current.active_run_id.is_some() {
                        return Err(anyhow::Error::new(LocalSchedulerError::AlreadyRunning(id)));
                    }
                    let changed =
                        sql_query("DELETE FROM local_schedules WHERE id = ? AND revision = ?")
                            .bind::<Text, _>(id.to_string())
                            .bind::<BigInt, _>(expected_revision)
                            .execute(connection)?;
                    if changed != 1 {
                        return Err(anyhow::anyhow!("schedule compare-and-swap failed"));
                    }
                    Ok(())
                })
                .map_err(map_transaction_error)
        })
    }

    pub fn request_run(&self, id: Uuid) -> Result<(), LocalSchedulerError> {
        self.with_connection(|connection| {
            let schedule =
                load_schedule(connection, id)?.ok_or(LocalSchedulerError::NotFound(id))?;
            if schedule.active_run_id.is_some() {
                return Err(LocalSchedulerError::AlreadyRunning(id));
            }
            sql_query(
                "UPDATE local_schedules SET manual_requested = 1, updated_at = ? WHERE id = ?",
            )
            .bind::<BigInt, _>(now_millis())
            .bind::<Text, _>(id.to_string())
            .execute(connection)
            .map_err(|error| storage_error(format!("requesting immediate run: {error}")))?;
            Ok(())
        })
    }

    pub fn request_cancel(&self, id: Uuid) -> Result<(), LocalSchedulerError> {
        self.with_connection(|connection| {
            let schedule =
                load_schedule(connection, id)?.ok_or(LocalSchedulerError::NotFound(id))?;
            if schedule.active_run_id.is_none() {
                return Err(LocalSchedulerError::NotRunning(id));
            }
            sql_query(
                "UPDATE local_schedules SET cancel_requested = 1, updated_at = ? WHERE id = ?",
            )
            .bind::<BigInt, _>(now_millis())
            .bind::<Text, _>(id.to_string())
            .execute(connection)
            .map_err(|error| storage_error(format!("requesting cancellation: {error}")))?;
            Ok(())
        })
    }

    pub fn cancellation_requests(&self) -> Result<Vec<(Uuid, Uuid)>, LocalSchedulerError> {
        #[derive(QueryableByName)]
        struct CancellationRow {
            #[diesel(sql_type = Text)]
            id: String,
            #[diesel(sql_type = Text)]
            active_run_id: String,
        }
        self.with_connection(|connection| {
            sql_query(
                "SELECT id, active_run_id FROM local_schedules \
                 WHERE cancel_requested = 1 AND active_run_id IS NOT NULL",
            )
            .load::<CancellationRow>(connection)
            .map_err(|error| storage_error(format!("loading cancellation requests: {error}")))?
            .into_iter()
            .map(|row| {
                Ok((
                    parse_uuid(&row.id, "schedule id")?,
                    parse_uuid(&row.active_run_id, "run id")?,
                ))
            })
            .collect()
        })
    }

    pub fn claim_due(&self, now: i64) -> Result<Vec<ClaimedLocalScheduleRun>, LocalSchedulerError> {
        self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    #[derive(QueryableByName)]
                    struct DueRow {
                        #[diesel(sql_type = Text)]
                        id: String,
                        #[diesel(sql_type = Integer)]
                        manual_requested: i32,
                    }
                    let rows = sql_query(
                        "SELECT id, manual_requested FROM local_schedules \
                         WHERE active_run_id IS NULL AND \
                         (manual_requested = 1 OR (enabled = 1 AND next_run_at <= ?)) \
                         ORDER BY manual_requested DESC, next_run_at ASC, id ASC LIMIT ?",
                    )
                    .bind::<BigInt, _>(now)
                    .bind::<BigInt, _>(MAX_CLAIMS_PER_TICK as i64)
                    .load::<DueRow>(connection)?;

                    let mut claims = Vec::new();
                    for row in rows {
                        let id = parse_uuid(&row.id, "schedule id")?;
                        let schedule = load_schedule(connection, id)?
                            .ok_or_else(|| anyhow::Error::new(LocalSchedulerError::NotFound(id)))?;
                        let manual = row.manual_requested != 0;
                        let scheduled_at = if manual { now } else { schedule.next_run_at };
                        let missed = !manual && now.saturating_sub(scheduled_at) > MISSED_GRACE_MS;

                        if missed && schedule.missed_policy == MissedRunPolicy::Skip {
                            let next = schedule.cadence.advance_past(
                                schedule.next_run_at,
                                now,
                                &schedule.timezone,
                            )?;
                            sql_query(
                                "UPDATE local_schedules SET next_run_at = ?, last_scheduled_at = ?, \
                                 updated_at = ?, revision = revision + 1 WHERE id = ? AND active_run_id IS NULL",
                            )
                            .bind::<BigInt, _>(next)
                            .bind::<Nullable<BigInt>, _>(Some(scheduled_at))
                            .bind::<BigInt, _>(now)
                            .bind::<Text, _>(id.to_string())
                            .execute(connection)?;
                            insert_event(
                                connection,
                                id,
                                None,
                                LocalScheduleEventKind::Missed,
                                Some(scheduled_at),
                                now,
                                "missed run skipped by local policy",
                            )?;
                            continue;
                        }

                        let next = if manual {
                            schedule.next_run_at
                        } else if missed && schedule.missed_policy == MissedRunPolicy::RunOnce {
                            schedule.cadence.first_after(now, &schedule.timezone)?
                        } else {
                            schedule
                                .cadence
                                .next_after(scheduled_at, &schedule.timezone)?
                        };
                        let run_id = Uuid::new_v4();
                        let changed = sql_query(
                            "UPDATE local_schedules SET active_run_id = ?, active_started_at = ?, \
                             manual_requested = 0, cancel_requested = 0, next_run_at = ?, \
                             last_scheduled_at = ?, updated_at = ?, revision = revision + 1 \
                             WHERE id = ? AND active_run_id IS NULL",
                        )
                        .bind::<Text, _>(run_id.to_string())
                        .bind::<BigInt, _>(now)
                        .bind::<BigInt, _>(next)
                        .bind::<Nullable<BigInt>, _>(Some(scheduled_at))
                        .bind::<BigInt, _>(now)
                        .bind::<Text, _>(id.to_string())
                        .execute(connection)?;
                        if changed != 1 {
                            continue;
                        }
                        insert_event(
                            connection,
                            id,
                            Some(run_id),
                            LocalScheduleEventKind::Started,
                            Some(scheduled_at),
                            now,
                            if manual { "manual local run" } else { "scheduled local run" },
                        )?;
                        let claimed = load_schedule(connection, id)?.ok_or_else(|| {
                            anyhow::Error::new(LocalSchedulerError::NotFound(id))
                        })?;
                        claims.push(ClaimedLocalScheduleRun {
                            schedule: claimed,
                            run_id,
                            scheduled_at,
                            manual,
                        });
                    }
                    Ok(claims)
                })
                .map_err(map_transaction_error)
        })
    }

    pub fn finish_run(
        &self,
        schedule_id: Uuid,
        run_id: Uuid,
        kind: LocalScheduleEventKind,
        detail: &str,
    ) -> Result<bool, LocalSchedulerError> {
        if !matches!(
            kind,
            LocalScheduleEventKind::Completed
                | LocalScheduleEventKind::Failed
                | LocalScheduleEventKind::Cancelled
                | LocalScheduleEventKind::Interrupted
        ) {
            return Err(LocalSchedulerError::Corrupt(
                "finish_run requires a terminal event".into(),
            ));
        }
        self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    let now = now_millis();
                    let schedule = load_schedule(connection, schedule_id)?.ok_or_else(|| {
                        anyhow::Error::new(LocalSchedulerError::NotFound(schedule_id))
                    })?;
                    if schedule.active_run_id != Some(run_id) {
                        return Ok(false);
                    }
                    sql_query(
                        "UPDATE local_schedules SET active_run_id = NULL, active_started_at = NULL, \
                         cancel_requested = 0, updated_at = ?, revision = revision + 1 \
                         WHERE id = ? AND active_run_id = ?",
                    )
                    .bind::<BigInt, _>(now)
                    .bind::<Text, _>(schedule_id.to_string())
                    .bind::<Text, _>(run_id.to_string())
                    .execute(connection)?;
                    insert_event(
                        connection,
                        schedule_id,
                        Some(run_id),
                        kind,
                        schedule.last_scheduled_at,
                        now,
                        detail,
                    )?;
                    Ok(true)
                })
                .map_err(map_transaction_error)
        })
    }

    pub fn recover_interrupted_runs(&self) -> Result<usize, LocalSchedulerError> {
        self.with_connection(|connection| {
            connection
                .transaction::<_, anyhow::Error, _>(|connection| {
                    let active = sql_query(format!(
                        "SELECT {SCHEDULE_COLUMNS} FROM local_schedules WHERE active_run_id IS NOT NULL"
                    ))
                    .load::<ScheduleDbRow>(connection)?
                    .into_iter()
                    .map(schedule_from_row)
                    .collect::<Result<Vec<_>, _>>()?;
                    let now = now_millis();
                    for schedule in &active {
                        let Some(run_id) = schedule.active_run_id else {
                            continue;
                        };
                        sql_query(
                            "UPDATE local_schedules SET active_run_id = NULL, active_started_at = NULL, \
                             cancel_requested = 0, updated_at = ?, revision = revision + 1 WHERE id = ?",
                        )
                        .bind::<BigInt, _>(now)
                        .bind::<Text, _>(schedule.id.to_string())
                        .execute(connection)?;
                        insert_event(
                            connection,
                            schedule.id,
                            Some(run_id),
                            LocalScheduleEventKind::Interrupted,
                            schedule.last_scheduled_at,
                            now,
                            "local supervisor restarted before the child process completed",
                        )?;
                    }
                    Ok(active.len())
                })
                .map_err(map_transaction_error)
        })
    }

    pub fn events_after(
        &self,
        schedule_id: Uuid,
        after: i64,
        limit: usize,
    ) -> Result<Vec<LocalScheduleEvent>, LocalSchedulerError> {
        let limit = limit.clamp(1, 1_000) as i64;
        self.with_connection(|connection| {
            sql_query(
                "SELECT sequence, event_id, schedule_id, run_id, kind, scheduled_at, occurred_at, detail \
                 FROM local_schedule_events WHERE schedule_id = ? AND sequence > ? \
                 ORDER BY sequence ASC LIMIT ?",
            )
            .bind::<Text, _>(schedule_id.to_string())
            .bind::<BigInt, _>(after.max(0))
            .bind::<BigInt, _>(limit)
            .load::<EventDbRow>(connection)
            .map_err(|error| storage_error(format!("reading schedule events: {error}")))?
            .into_iter()
            .map(event_from_row)
            .collect()
        })
    }

    pub fn cursor(&self, consumer_id: &str) -> Result<i64, LocalSchedulerError> {
        validate_consumer_id(consumer_id)?;
        #[derive(QueryableByName)]
        struct CursorRow {
            #[diesel(sql_type = BigInt)]
            sequence: i64,
        }
        self.with_connection(|connection| {
            Ok(
                sql_query("SELECT sequence FROM local_schedule_cursors WHERE consumer_id = ?")
                    .bind::<Text, _>(consumer_id)
                    .get_result::<CursorRow>(connection)
                    .optional()
                    .map_err(|error| storage_error(format!("reading event cursor: {error}")))?
                    .map(|row| row.sequence)
                    .unwrap_or(0),
            )
        })
    }

    pub fn advance_cursor(
        &self,
        consumer_id: &str,
        sequence: i64,
    ) -> Result<i64, LocalSchedulerError> {
        validate_consumer_id(consumer_id)?;
        self.with_connection(|connection| {
            sql_query(
                "INSERT INTO local_schedule_cursors (consumer_id, sequence, updated_at) VALUES (?, ?, ?) \
                 ON CONFLICT(consumer_id) DO UPDATE SET \
                 sequence = MAX(local_schedule_cursors.sequence, excluded.sequence), \
                 updated_at = excluded.updated_at",
            )
            .bind::<Text, _>(consumer_id)
            .bind::<BigInt, _>(sequence.max(0))
            .bind::<BigInt, _>(now_millis())
            .execute(connection)
            .map_err(|error| storage_error(format!("advancing event cursor: {error}")))?;
            #[derive(QueryableByName)]
            struct CursorRow {
                #[diesel(sql_type = BigInt)]
                sequence: i64,
            }
            Ok(sql_query("SELECT sequence FROM local_schedule_cursors WHERE consumer_id = ?")
                .bind::<Text, _>(consumer_id)
                .get_result::<CursorRow>(connection)
                .map_err(|error| storage_error(format!("reading advanced event cursor: {error}")))?
                .sequence)
        })
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut SqliteConnection) -> Result<T, LocalSchedulerError>,
    ) -> Result<T, LocalSchedulerError> {
        operation(&mut self.inner.borrow_mut())
    }
}

use diesel::OptionalExtension as _;

fn validate_new_schedule(
    mut input: NewLocalSchedule,
) -> Result<NewLocalSchedule, LocalSchedulerError> {
    input.name = input.name.trim().to_owned();
    input.prompt = input.prompt.trim().to_owned();
    if input.name.is_empty() {
        return Err(LocalSchedulerError::EmptyName);
    }
    if input.prompt.is_empty() {
        return Err(LocalSchedulerError::EmptyPrompt);
    }
    if input.name.chars().count() > MAX_NAME_CHARS {
        return Err(LocalSchedulerError::NameTooLong);
    }
    if input.prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(LocalSchedulerError::PromptTooLong);
    }
    if let Some(path) = input.working_directory.take() {
        input.working_directory = Some(
            dunce::canonicalize(&path)
                .map_err(|_| LocalSchedulerError::InvalidWorkingDirectory(path))?,
        );
    }
    Ok(input)
}

fn validate_consumer_id(value: &str) -> Result<(), LocalSchedulerError> {
    if value.trim().is_empty()
        || value.len() > 160
        || value.chars().any(|character| character.is_control())
    {
        return Err(LocalSchedulerError::Corrupt(
            "event consumer id must be a non-empty local label".into(),
        ));
    }
    Ok(())
}

fn ensure_revision(schedule: &LocalSchedule, expected: i64) -> Result<(), anyhow::Error> {
    if schedule.revision != expected {
        return Err(anyhow::Error::new(LocalSchedulerError::Conflict {
            id: schedule.id,
            expected,
            actual: schedule.revision,
        }));
    }
    Ok(())
}

fn insert_schedule(
    connection: &mut SqliteConnection,
    schedule: &LocalSchedule,
) -> Result<(), anyhow::Error> {
    let (cadence_kind, cadence_value) = schedule.cadence.parts();
    sql_query(
        "INSERT INTO local_schedules (id, name, agent_id, prompt, working_directory, cadence_kind, \
         cadence_value, timezone, missed_policy, enabled, notify, next_run_at, last_scheduled_at, \
         manual_requested, cancel_requested, active_run_id, active_started_at, revision, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?, ?, ?, ?, ?)",
    )
    .bind::<Text, _>(schedule.id.to_string())
    .bind::<Text, _>(&schedule.name)
    .bind::<Text, _>(schedule.agent_id.to_string())
    .bind::<Text, _>(&schedule.prompt)
    .bind::<Nullable<Text>, _>(path_to_string(schedule.working_directory.as_deref()))
    .bind::<Text, _>(cadence_kind)
    .bind::<Text, _>(cadence_value)
    .bind::<Text, _>(schedule.timezone.display())
    .bind::<Text, _>(schedule.missed_policy.as_str())
    .bind::<Integer, _>(i32::from(schedule.enabled))
    .bind::<Integer, _>(i32::from(schedule.notify))
    .bind::<BigInt, _>(schedule.next_run_at)
    .bind::<Nullable<BigInt>, _>(schedule.last_scheduled_at)
    .bind::<Nullable<Text>, _>(schedule.active_run_id.map(|id| id.to_string()))
    .bind::<Nullable<BigInt>, _>(schedule.active_started_at)
    .bind::<BigInt, _>(schedule.revision)
    .bind::<BigInt, _>(schedule.created_at)
    .bind::<BigInt, _>(schedule.updated_at)
    .execute(connection)?;
    Ok(())
}

fn load_schedule(
    connection: &mut SqliteConnection,
    id: Uuid,
) -> Result<Option<LocalSchedule>, LocalSchedulerError> {
    sql_query(format!(
        "SELECT {SCHEDULE_COLUMNS} FROM local_schedules WHERE id = ?"
    ))
    .bind::<Text, _>(id.to_string())
    .get_result::<ScheduleDbRow>(connection)
    .optional()
    .map_err(|error| storage_error(format!("loading schedule: {error}")))?
    .map(schedule_from_row)
    .transpose()
}

fn schedule_from_row(row: ScheduleDbRow) -> Result<LocalSchedule, LocalSchedulerError> {
    Ok(LocalSchedule {
        id: parse_uuid(&row.id, "schedule id")?,
        name: row.name,
        agent_id: parse_uuid(&row.agent_id, "agent id")?,
        prompt: row.prompt,
        working_directory: row.working_directory.map(PathBuf::from),
        cadence: LocalScheduleCadence::from_parts(&row.cadence_kind, &row.cadence_value)?,
        timezone: LocalScheduleTimezone::parse(&row.timezone)?,
        missed_policy: MissedRunPolicy::parse(&row.missed_policy)?,
        enabled: parse_bool(row.enabled, "enabled")?,
        notify: parse_bool(row.notify, "notify")?,
        next_run_at: row.next_run_at,
        last_scheduled_at: row.last_scheduled_at,
        active_run_id: row
            .active_run_id
            .map(|value| parse_uuid(&value, "active run id"))
            .transpose()?,
        active_started_at: row.active_started_at,
        revision: row.revision,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn insert_event(
    connection: &mut SqliteConnection,
    schedule_id: Uuid,
    run_id: Option<Uuid>,
    kind: LocalScheduleEventKind,
    scheduled_at: Option<i64>,
    occurred_at: i64,
    detail: &str,
) -> Result<(), anyhow::Error> {
    let detail = truncate(detail, MAX_EVENT_DETAIL_CHARS);
    sql_query(
        "INSERT INTO local_schedule_events \
         (event_id, schedule_id, run_id, kind, scheduled_at, occurred_at, detail) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind::<Text, _>(Uuid::new_v4().to_string())
    .bind::<Text, _>(schedule_id.to_string())
    .bind::<Nullable<Text>, _>(run_id.map(|id| id.to_string()))
    .bind::<Text, _>(kind.as_str())
    .bind::<Nullable<BigInt>, _>(scheduled_at)
    .bind::<BigInt, _>(occurred_at)
    .bind::<Text, _>(detail)
    .execute(connection)?;
    Ok(())
}

fn event_from_row(row: EventDbRow) -> Result<LocalScheduleEvent, LocalSchedulerError> {
    Ok(LocalScheduleEvent {
        sequence: row.sequence,
        event_id: parse_uuid(&row.event_id, "event id")?,
        schedule_id: parse_uuid(&row.schedule_id, "event schedule id")?,
        run_id: row
            .run_id
            .map(|value| parse_uuid(&value, "event run id"))
            .transpose()?,
        kind: LocalScheduleEventKind::parse(&row.kind)?,
        scheduled_at: row.scheduled_at,
        occurred_at: row.occurred_at,
        detail: row.detail,
    })
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, LocalSchedulerError> {
    Uuid::parse_str(value)
        .map_err(|error| LocalSchedulerError::Corrupt(format!("invalid {field}: {error}")))
}

fn parse_bool(value: i32, field: &str) -> Result<bool, LocalSchedulerError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(LocalSchedulerError::Corrupt(format!(
            "invalid {field} boolean {value}"
        ))),
    }
}

fn path_to_string(path: Option<&Path>) -> Option<String> {
    path.map(|path| path.to_string_lossy().into_owned())
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut output = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}

fn map_transaction_error(error: anyhow::Error) -> LocalSchedulerError {
    match error.downcast::<LocalSchedulerError>() {
        Ok(error) => error,
        Err(error) => LocalSchedulerError::Storage(error),
    }
}

fn storage_error(message: impl Into<String>) -> LocalSchedulerError {
    LocalSchedulerError::Storage(anyhow::anyhow!(message.into()))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

struct ActiveRun {
    run_id: Uuid,
    handle: SpawnedFutureHandle,
}

#[derive(Debug, Clone)]
pub enum LocalSchedulerEvent {
    JournalAdvanced { schedule_id: Uuid, sequence: i64 },
}

pub struct LocalScheduler {
    repository: Option<LocalScheduleRepository>,
    active: HashMap<Uuid, ActiveRun>,
    tick_handle: Option<SpawnedFutureHandle>,
}

impl Entity for LocalScheduler {
    type Event = LocalSchedulerEvent;
}

impl SingletonEntity for LocalScheduler {}

impl LocalScheduler {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let repository = match LocalScheduleRepository::open_current_scope() {
            Ok(repository) => {
                if let Err(error) = repository.recover_interrupted_runs() {
                    log::error!("Failed to recover interrupted local schedules: {error}");
                }
                Some(repository)
            }
            Err(error) => {
                log::error!(
                    "Local scheduler is unavailable because its SQLite repository could not be opened: {error}"
                );
                None
            }
        };
        let mut scheduler = Self {
            repository,
            active: HashMap::new(),
            tick_handle: None,
        };
        if scheduler.repository.is_some() {
            scheduler.arm_tick(Duration::from_millis(250), ctx);
        }
        scheduler
    }

    pub fn repository(&self) -> Option<LocalScheduleRepository> {
        self.repository.clone()
    }

    fn arm_tick(&mut self, delay: Duration, ctx: &mut ModelContext<Self>) {
        if let Some(handle) = self.tick_handle.take() {
            handle.abort();
        }
        self.tick_handle = Some(ctx.spawn(
            async move {
                Timer::after(delay).await;
            },
            |scheduler, (), ctx| {
                scheduler.tick(ctx);
                scheduler.arm_tick(SUPERVISOR_TICK, ctx);
            },
        ));
    }

    fn tick(&mut self, ctx: &mut ModelContext<Self>) {
        let Some(repository) = self.repository.clone() else {
            return;
        };
        match repository.cancellation_requests() {
            Ok(requests) => {
                for (schedule_id, run_id) in requests {
                    let Some(active) = self.active.remove(&schedule_id) else {
                        continue;
                    };
                    if active.run_id != run_id {
                        self.active.insert(schedule_id, active);
                        continue;
                    }
                    active.handle.abort();
                    if let Err(error) = repository.finish_run(
                        schedule_id,
                        run_id,
                        LocalScheduleEventKind::Cancelled,
                        "cancelled by local user request",
                    ) {
                        log::error!("Failed to journal local schedule cancellation: {error}");
                    }
                    self.emit_latest(schedule_id, ctx);
                }
            }
            Err(error) => log::error!("Failed to poll local schedule cancellations: {error}"),
        }

        match repository.claim_due(now_millis()) {
            Ok(claims) => {
                for claim in claims {
                    self.start_claim(claim, ctx);
                }
            }
            Err(error) => log::error!("Failed to claim local schedules: {error}"),
        }
    }

    fn start_claim(&mut self, claim: ClaimedLocalScheduleRun, ctx: &mut ModelContext<Self>) {
        let schedule_id = claim.schedule.id;
        let run_id = claim.run_id;
        let schedule = claim.schedule.clone();
        let handle = ctx.spawn(
            run_schedule_process(schedule),
            move |scheduler, result, ctx| {
                let Some(active) = scheduler.active.remove(&schedule_id) else {
                    return;
                };
                if active.run_id != run_id {
                    return;
                }
                let (kind, detail) = match result {
                    Ok(output) if output.success => {
                        (LocalScheduleEventKind::Completed, output.detail)
                    }
                    Ok(output) => (LocalScheduleEventKind::Failed, output.detail),
                    Err(error) => (LocalScheduleEventKind::Failed, error.to_string()),
                };
                let Some(repository) = scheduler.repository.as_ref() else {
                    return;
                };
                match repository.finish_run(schedule_id, run_id, kind, &detail) {
                    Ok(true) => {
                        scheduler.emit_latest(schedule_id, ctx);
                        if claim.schedule.notify {
                            send_schedule_notification(&claim.schedule.name, kind, &detail, ctx);
                        }
                    }
                    Ok(false) => {}
                    Err(error) => log::error!("Failed to finish local schedule: {error}"),
                }
            },
        );
        self.active
            .insert(schedule_id, ActiveRun { run_id, handle });
        self.emit_latest(schedule_id, ctx);
    }

    fn emit_latest(&self, schedule_id: Uuid, ctx: &mut ModelContext<Self>) {
        if let Some(repository) = self.repository.as_ref()
            && let Ok(events) = repository.events_after(schedule_id, 0, 1_000)
            && let Some(event) = events.last()
        {
            ctx.emit(LocalSchedulerEvent::JournalAdvanced {
                schedule_id,
                sequence: event.sequence,
            });
        }
    }
}

struct ScheduleProcessOutput {
    success: bool,
    detail: String,
}

async fn run_schedule_process(
    schedule: LocalSchedule,
) -> Result<ScheduleProcessOutput, anyhow::Error> {
    let executable = std::env::current_exe()
        .map_err(|error| anyhow::anyhow!("cannot locate the local WarpOss executable: {error}"))?;
    let mut command = AsyncCommand::new_with_process_group(executable);
    command
        .arg("agent")
        .arg("run")
        .arg("--agent")
        .arg(schedule.agent_id.to_string())
        .arg("--prompt")
        .arg(&schedule.prompt)
        .arg("--output-format")
        .arg("text")
        .env("WARP_LOCAL_SCHEDULE_ID", schedule.id.to_string())
        .env(
            "WARP_LOCAL_SCHEDULE_RUN_ID",
            schedule
                .active_run_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(directory) = &schedule.working_directory {
        command.arg("--cwd").arg(directory);
    }
    let output = command.output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = if output.status.success() {
        if stdout.trim().is_empty() {
            "local agent completed successfully".to_owned()
        } else {
            truncate(stdout.trim(), MAX_EVENT_DETAIL_CHARS)
        }
    } else {
        let status = output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_owned());
        let message = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        truncate(
            &format!("local agent exited with {status}: {message}"),
            MAX_EVENT_DETAIL_CHARS,
        )
    };
    Ok(ScheduleProcessOutput {
        success: output.status.success(),
        detail,
    })
}

fn send_schedule_notification(
    name: &str,
    kind: LocalScheduleEventKind,
    detail: &str,
    ctx: &mut ModelContext<LocalScheduler>,
) {
    let title = match kind {
        LocalScheduleEventKind::Completed => format!("{name} completed"),
        LocalScheduleEventKind::Failed => format!("{name} failed"),
        LocalScheduleEventKind::Cancelled => format!("{name} cancelled"),
        _ => return,
    };
    let body = truncate(detail, UserNotification::MAX_BODY_LENGTH);
    let workspace = ctx.window_ids().find_map(|window_id| {
        ctx.views_of_type::<Workspace>(window_id)
            .and_then(|views| views.into_iter().next())
    });
    if let Some(workspace) = workspace {
        workspace.update(ctx, move |_, ctx| {
            ctx.send_desktop_notification(
                UserNotification::new(
                    truncate(&title, UserNotification::MAX_TITLE_LENGTH),
                    body,
                    None,
                ),
                |_, error, _| {
                    log::warn!("Failed to show local schedule notification: {error:?}");
                },
            );
        });
    }
}

#[cfg(test)]
#[path = "local_scheduler_tests.rs"]
mod tests;
