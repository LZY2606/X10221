use crate::model::*;
use rusqlite::{params, Connection};
use serde_json::Value;

pub const SCHEMA_VERSION: i64 = 2;

const MIGRATION_1: &str = r#"
CREATE TABLE events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,
    stream TEXT NOT NULL,
    kind TEXT NOT NULL,
    version INTEGER NOT NULL,
    payload TEXT NOT NULL,
    raw_repr TEXT NOT NULL,
    created_event TEXT NOT NULL,
    created_seq INTEGER NOT NULL,
    UNIQUE(stream, version)
);
CREATE INDEX idx_artifacts_stream ON artifacts(stream, version);
CREATE TABLE derived (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    deps TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_event TEXT NOT NULL,
    created_seq INTEGER NOT NULL
);
CREATE TABLE branches (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    parent TEXT,
    created_seq INTEGER NOT NULL
);
CREATE adjudications_placeholder
"#;

const MIGRATION_2: &str = r#"
ALTER TABLE artifacts ADD COLUMN units TEXT;
ALTER TABLE artifacts ADD COLUMN basis TEXT;
CREATE TABLE diagnostics (
    id TEXT PRIMARY KEY,
    subject TEXT NOT NULL,
    severity TEXT NOT NULL,
    message TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_seq INTEGER NOT NULL
);
"#;
