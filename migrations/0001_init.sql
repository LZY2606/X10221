-- v1: 追加事件、分支、原始字节。
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS blobs (
    blob_id     TEXT PRIMARY KEY,
    media_type  TEXT NOT NULL,
    bytes       BLOB NOT NULL,
    imported_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS branches (
    branch_id          TEXT PRIMARY KEY,
    title              TEXT NOT NULL,
    forked_from_branch TEXT,
    fork_event_id      TEXT,
    created_at_unix    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    id              TEXT PRIMARY KEY,
    branch_id       TEXT NOT NULL,
    seq             INTEGER NOT NULL,
    parent_id       TEXT,
    payload         TEXT NOT NULL,
    actor           TEXT NOT NULL,
    created_at_unix INTEGER NOT NULL,
    UNIQUE (branch_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_events_branch ON events (branch_id, seq);
