use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[
    // v1: core provenance schema
    "CREATE TABLE sources(
        source_id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        current_version INTEGER NOT NULL
     );
     CREATE TABLE source_versions(
        source_id TEXT NOT NULL,
        version INTEGER NOT NULL,
        payload TEXT NOT NULL,
        created_event INTEGER NOT NULL,
        PRIMARY KEY(source_id, version)
     );
     CREATE TABLE events(
        event_id INTEGER PRIMARY KEY AUTOINCREMENT,
        branch_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        status TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_seq INTEGER NOT NULL
     );
     CREATE TABLE branches(
        branch_id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        parent_branch TEXT,
        base_event INTEGER NOT NULL
     );
     CREATE TABLE derived(
        artifact_id TEXT NOT NULL,
        branch_id TEXT NOT NULL,
        version INTEGER NOT NULL,
        kind TEXT NOT NULL,
        status TEXT NOT NULL,
        deps TEXT NOT NULL,
        result TEXT,
        candidates TEXT,
        denominator TEXT,
        created_event INTEGER NOT NULL,
        PRIMARY KEY(artifact_id, branch_id, version)
     );
     CREATE TABLE decisions(
        decision_id INTEGER PRIMARY KEY AUTOINCREMENT,
        branch_id TEXT NOT NULL,
        target_id TEXT NOT NULL,
        action TEXT NOT NULL,
        candidate_id TEXT,
        payload TEXT NOT NULL,
        event_id INTEGER NOT NULL
     );",
    // v2: undo support
    "ALTER TABLE events ADD COLUMN undone INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE decisions ADD COLUMN prior_state TEXT;
     ALTER TABLE decisions ADD COLUMN undone INTEGER NOT NULL DEFAULT 0;",
];

pub fn migrate_to(conn: &Connection, target: usize) -> rusqlite::Result<()> {
    let current: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (idx, sql) in MIGRATIONS.iter().enumerate() {
        let version = idx + 1;
        if version > current && version <= target {
            conn.execute_batch(sql)?;
        }
    }
    if target > current {
        conn.pragma_update(None, "user_version", target as i64)?;
    }
    Ok(())
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    migrate_to(conn, MIGRATIONS.len())
}
