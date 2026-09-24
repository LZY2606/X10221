use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[MIGRATION_V1, MIGRATION_V2];

const MIGRATION_V1: &str = "
CREATE TABLE materials(
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    version INTEGER NOT NULL,
    content TEXT NOT NULL,
    raw TEXT,
    committed_seq INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(kind, version)
);
CREATE TABLE drafts(
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    state TEXT NOT NULL,
    head_seq INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE events(
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL,
    draft_id TEXT,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE results(
    id TEXT NOT NULL,
    version INTEGER NOT NULL,
    draft_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    content TEXT NOT NULL,
    inputs TEXT NOT NULL,
    stale INTEGER NOT NULL DEFAULT 0,
    created_seq INTEGER NOT NULL,
    PRIMARY KEY(id, version)
);
";

const MIGRATION_V2: &str = "
CREATE TABLE diagnostics(
    id TEXT PRIMARY KEY,
    code TEXT NOT NULL,
    subject TEXT NOT NULL,
    severity TEXT NOT NULL,
    detail TEXT NOT NULL,
    candidates TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    created_seq INTEGER NOT NULL,
    resolved_seq INTEGER
);
ALTER TABLE results ADD COLUMN denominator TEXT;
CREATE INDEX idx_events_draft ON events(draft_id, seq);
CREATE INDEX idx_results_draft ON results(draft_id, id, version);
";

pub fn now_millis() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    ms.to_string()
}

pub fn migrate_to(conn: &Connection, target: usize) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = (i + 1) as i64;
        if v > current && v <= target as i64 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                rusqlite::params![v, now_millis()],
            )?;
            tx.commit()?;
        }
    }
    Ok(())
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    migrate_to(conn, MIGRATIONS.len())
}
