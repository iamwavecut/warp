CREATE TABLE local_codebase_index_nodes (
    space_id TEXT NOT NULL,
    node_hash TEXT NOT NULL,
    children_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (space_id, node_hash)
);

CREATE TABLE local_codebase_index_chunks (
    space_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    content TEXT NOT NULL,
    vector BLOB NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (space_id, content_hash)
);

CREATE TABLE local_codebase_index_roots (
    space_id TEXT NOT NULL,
    root_hash TEXT NOT NULL,
    repo_path TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (space_id, root_hash)
);

CREATE INDEX local_codebase_index_roots_repo
    ON local_codebase_index_roots(repo_path, space_id);
