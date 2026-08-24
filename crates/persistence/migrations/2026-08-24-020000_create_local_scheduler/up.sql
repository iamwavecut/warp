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
