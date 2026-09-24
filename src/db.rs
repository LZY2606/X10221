use rusqlite::Connection;

/// Step-wise migrations. Index+1 is the schema version.
pub const MIGRATIONS: &[&str] = &[
    // v1: base tables. Artifacts are immutable, separately versioned domain objects.
    "CREATE TABLE artifacts(
       id TEXT NOT NULL,
       version INTEGER NOT NULL,
       kind TEXT NOT NULL,
       payload TEXT NOT NULL,
       seq INTEGER NOT NULL,
       PRIMARY KEY(id, version)
     );
     CREATE TABLE deps(
       child_id TEXT NOT NULL,
       child_version INTEGER NOT NULL,
       parent_id TEXT NOT NULL,
       parent_version INTEGER NOT NULL,
       PRIMARY KEY(child_id, child_version, parent_id, parent_version)
     );
     CREATE TABLE events(
       id TEXT PRIMARY KEY,
       seq INTEGER NOT NULL UNIQUE,
       branch TEXT NOT NULL,
       kind TEXT NOT NULL,
       payload TEXT NOT NULL
     );
     CREATE TABLE branches(
       id TEXT PRIMARY KEY,
       name TEXT NOT NULL,
       head_seq INTEGER NOT NULL,
       version INTEGER NOT NULL
     );
     CREATE TABLE counters(
       name TEXT PRIMARY KEY,
       value INTEGER NOT NULL
     );",
    // v2: append-only derived state (stale/blocked/adjudicated). History kept via seq.
    "CREATE TABLE artifact_state(
       id TEXT NOT NULL,
       version INTEGER NOT NULL,
       seq INTEGER NOT NULL,
       status TEXT NOT NULL,
       PRIMARY KEY(id, version, seq)
     );",
    // v3: index for branch-visible event scans.
    "CREATE INDEX idx_events_branch_seq ON events(branch, seq);",
];

pub fn migrate_to(conn: &mut Connection, target: usize) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(
           version INTEGER PRIMARY KEY,
           applied_at TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    let target = target.min(MIGRATIONS.len()) as i64;
    for v in (current + 1)..=target {
        let tx = conn.transaction()?;
        tx.execute_batch(MIGRATIONS[(v - 1) as usize])?;
        tx.execute("INSERT INTO schema_migrations(version) VALUES (?1)", [v])?;
        tx.commit()?;
    }
    Ok(())
}

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    migrate_to(conn, MIGRATIONS.len())
}

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let mut conn = Connection::open(path)?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn open_memory() -> rusqlite::Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    migrate(&mut conn)?;
    Ok(conn)
}
