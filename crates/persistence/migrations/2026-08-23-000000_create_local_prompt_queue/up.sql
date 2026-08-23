CREATE TABLE local_prompt_queue_rows (
    id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    kind TEXT NOT NULL,
    text TEXT NOT NULL,
    origin TEXT NOT NULL,
    attachments_json TEXT NOT NULL DEFAULT '[]',
    locked INTEGER NOT NULL DEFAULT 0,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    dispatched_at INTEGER,
    local_error TEXT
);

CREATE INDEX local_prompt_queue_rows_conversation_position
    ON local_prompt_queue_rows(conversation_id, position, id);

CREATE TABLE local_prompt_queue_settings (
    conversation_id TEXT PRIMARY KEY NOT NULL,
    queue_next_prompt_enabled INTEGER NOT NULL DEFAULT 0,
    command_in_flight INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);

CREATE TABLE local_prompt_queue_quarantine (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    row_id TEXT,
    conversation_id TEXT NOT NULL,
    raw_row TEXT NOT NULL,
    reason TEXT NOT NULL,
    quarantined_at INTEGER NOT NULL
);
