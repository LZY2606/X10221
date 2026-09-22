-- v1：事件溯源基线 schema
CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
    project_id TEXT PRIMARY KEY,
    head_seq   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS events (
    seq              INTEGER PRIMARY KEY,
    project_id       TEXT NOT NULL,
    branch           TEXT NOT NULL,
    parent_branch    TEXT,
    branch_start_seq INTEGER NOT NULL DEFAULT 0,
    event_id         TEXT NOT NULL,
    kind             TEXT NOT NULL,
    payload_json     TEXT NOT NULL,
    created_at       TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_events_project_branch ON events(project_id, branch, seq);

CREATE TABLE IF NOT EXISTS sources_blobs (
    project_id TEXT NOT NULL,
    source_id  TEXT NOT NULL,
    sha256     TEXT NOT NULL,
    bytes      BLOB NOT NULL,
    PRIMARY KEY (project_id, source_id)
);
