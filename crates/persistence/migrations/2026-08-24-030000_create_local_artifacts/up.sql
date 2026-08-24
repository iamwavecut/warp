CREATE TABLE IF NOT EXISTS local_artifacts (
    artifact_uid TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('file', 'screenshot')),
    local_path TEXT NOT NULL UNIQUE,
    source_path TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    checksum_sha256 TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    description TEXT,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS local_artifacts_created
    ON local_artifacts(created_at DESC, artifact_uid);

CREATE TABLE IF NOT EXISTS local_artifact_owners (
    artifact_uid TEXT NOT NULL REFERENCES local_artifacts(artifact_uid) ON DELETE CASCADE,
    owner_kind TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (artifact_uid, owner_kind, owner_id)
);

CREATE INDEX IF NOT EXISTS local_artifact_owners_owner
    ON local_artifact_owners(owner_kind, owner_id, artifact_uid);
