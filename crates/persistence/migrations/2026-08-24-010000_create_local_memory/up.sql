CREATE TABLE local_memories (
    id TEXT PRIMARY KEY NOT NULL,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'project')),
    scope_key TEXT NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    revision INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX local_memories_scope_updated
    ON local_memories(scope_kind, scope_key, updated_at DESC, id);

CREATE TABLE local_memory_versions (
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('created', 'updated', 'deleted')),
    scope_kind TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    recorded_at INTEGER NOT NULL,
    PRIMARY KEY (memory_id, revision)
);

CREATE INDEX local_memory_versions_recorded
    ON local_memory_versions(memory_id, revision DESC);
