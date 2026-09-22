CREATE TABLE IF NOT EXISTS blobs (
    blob_id   TEXT PRIMARY KEY,
    kind      TEXT NOT NULL,
    payload   TEXT NOT NULL,
    sha256    TEXT NOT NULL,
    seq       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS branches (
    branch_id      TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    base_branch_id TEXT,
    head_seq       INTEGER NOT NULL DEFAULT 0,
    head_event_id  TEXT,
    created_seq    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    branch_id  TEXT NOT NULL,
    seq        INTEGER NOT NULL,
    event_id   TEXT NOT NULL,
    kind       TEXT NOT NULL,
    actor      TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    snapshot_blob_id TEXT,
    ts_ms      INTEGER NOT NULL,
    PRIMARY KEY (branch_id, seq),
    UNIQUE (branch_id, event_id)
);
CREATE INDEX IF NOT EXISTS idx_events_branch ON events(branch_id, seq);
