use rusqlite::{Connection, Result};

/// Ordered schema migrations. `migrate_to` is exposed so migration tests can
/// build a database at an older schema version and then upgrade it.
pub const MIGRATIONS: &[&str] = &[
    // v1: core provenance tables
    r#"
    CREATE TABLE sources(
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      kind TEXT NOT NULL,
      label TEXT NOT NULL,
      raw BLOB NOT NULL,
      created_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE TABLE entity_versions(
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      kind TEXT NOT NULL,
      ekey TEXT NOT NULL,
      version INTEGER NOT NULL,
      branch TEXT NOT NULL,
      payload TEXT NOT NULL,
      created_at TEXT NOT NULL DEFAULT (datetime('now')),
      UNIQUE(kind, ekey, version)
    );
    CREATE TABLE drafts(
      name TEXT PRIMARY KEY,
      parent TEXT,
      head_rev INTEGER NOT NULL DEFAULT 0,
      pins TEXT NOT NULL DEFAULT '{}'
    );
    CREATE TABLE events(
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      kind TEXT NOT NULL,
      draft TEXT,
      summary TEXT NOT NULL,
      payload TEXT NOT NULL,
      created_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    "#,
    // v2: derived populations and multi-candidate records
    r#"
    CREATE TABLE populations(
      id TEXT PRIMARY KEY,
      draft TEXT NOT NULL,
      gate_id TEXT NOT NULL,
      count INTEGER NOT NULL,
      percent REAL,
      denominator TEXT,
      dep TEXT NOT NULL,
      stale INTEGER NOT NULL DEFAULT 0,
      created_event INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE candidates(
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      draft TEXT NOT NULL,
      subject TEXT NOT NULL,
      options TEXT NOT NULL,
      status TEXT NOT NULL DEFAULT 'open',
      resolution TEXT
    );
    "#,
    // v3: branch head history for undo / conflict diffs
    r#"
    CREATE TABLE head_history(
      draft TEXT NOT NULL,
      rev INTEGER NOT NULL,
      pins TEXT NOT NULL,
      event_id INTEGER NOT NULL DEFAULT 0,
      PRIMARY KEY(draft, rev)
    );
    "#,
];

pub fn migrate_to(conn: &Connection, target: usize) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(
           version INTEGER PRIMARY KEY,
           applied_at TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )?;
    let cur: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    for (i, m) in MIGRATIONS.iter().enumerate() {
        let v = (i + 1) as i64;
        if v > cur && v <= target as i64 {
            conn.execute_batch(m)?;
            conn.execute("INSERT INTO schema_migrations(version) VALUES (?1)", [v])?;
        }
    }
    Ok(())
}

pub fn migrate(conn: &Connection) -> Result<()> {
    migrate_to(conn, MIGRATIONS.len())
}

pub fn schema_version(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )
}
