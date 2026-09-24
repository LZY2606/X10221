//! SQLite 存储：分步迁移，事件日志 + 分版本工件 + 群体结果 + 候选 + 草案。

use rusqlite::Connection;

pub const MIGRATIONS: [&str; 3] = [
    // v1：事件日志、分版本工件、候选裁定
    "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY);
     CREATE TABLE events(
       seq INTEGER PRIMARY KEY AUTOINCREMENT,
       kind TEXT NOT NULL,
       payload TEXT NOT NULL,
       created_at TEXT NOT NULL DEFAULT (datetime('now'))
     );
     CREATE TABLE artifacts(
       id TEXT NOT NULL,
       kind TEXT NOT NULL,
       version INTEGER NOT NULL,
       content TEXT NOT NULL,
       created_seq INTEGER NOT NULL,
       PRIMARY KEY(id, version)
     );
     CREATE TABLE candidates(
       candidate_id TEXT PRIMARY KEY,
       subject TEXT NOT NULL,
       options TEXT NOT NULL,
       decision TEXT,
       created_seq INTEGER NOT NULL,
       decided_seq INTEGER,
       undone_seq INTEGER
     );",
    // v2：群体结果（钉住分母版本）与审阅草案
    "CREATE TABLE populations(
       pop_id TEXT NOT NULL,
       version INTEGER NOT NULL,
       gate_id TEXT NOT NULL,
       gate_version INTEGER NOT NULL,
       parent_pop_id TEXT,
       parent_pop_version INTEGER,
       count INTEGER NOT NULL,
       denom_pop_id TEXT,
       denom_pop_version INTEGER,
       denom_count INTEGER,
       pct REAL,
       member_event_ids TEXT NOT NULL,
       used_candidates TEXT NOT NULL DEFAULT '[]',
       pending_candidates TEXT NOT NULL DEFAULT '[]',
       valid_from_seq INTEGER NOT NULL,
       stale_since_seq INTEGER,
       PRIMARY KEY(pop_id, version)
     );
     CREATE TABLE drafts(
       draft_id TEXT PRIMARY KEY,
       base_seq INTEGER NOT NULL,
       edits TEXT NOT NULL,
       status TEXT NOT NULL,
       created_seq INTEGER NOT NULL
     );",
    // v3：原始表示列与失效原因列
    "ALTER TABLE artifacts ADD COLUMN raw_payload TEXT;
     ALTER TABLE populations ADD COLUMN stale_reason TEXT;
     CREATE INDEX idx_events_kind ON events(kind);",
];

pub fn current_version(conn: &Connection) -> rusqlite::Result<usize> {
    let has_table: bool = conn.query_row(
        "SELECT count(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_migrations'",
        [],
        |r| r.get(0),
    )?;
    if !has_table {
        return Ok(0);
    }
    let v: Option<i64> = conn.query_row("SELECT max(version) FROM schema_migrations", [], |r| r.get(0))?;
    Ok(v.unwrap_or(0) as usize)
}

pub fn migrate_to(conn: &Connection, target: usize) -> rusqlite::Result<()> {
    let mut v = current_version(conn)?;
    while v < target {
        conn.execute_batch(MIGRATIONS[v])?;
        conn.execute(
            "INSERT INTO schema_migrations(version) VALUES (?1)",
            [v as i64 + 1],
        )?;
        v += 1;
    }
    Ok(())
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    migrate_to(conn, MIGRATIONS.len())
}
