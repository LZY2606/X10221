use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[
    r#"
    CREATE TABLE artifacts (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        version INTEGER NOT NULL,
        payload TEXT NOT NULL,
        unit TEXT,
        basis TEXT,
        created_seq INTEGER NOT NULL,
        UNIQUE(kind, name, version)
    );
    CREATE TABLE events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        id TEXT NOT NULL,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL
    );
    CREATE TABLE results (
        id TEXT PRIMARY KEY,
        branch TEXT NOT NULL,
        population TEXT NOT NULL,
        gate_name TEXT NOT NULL,
        gate_version INTEGER NOT NULL,
        parent_population TEXT,
        parent_gate_version INTEGER,
        parent_result TEXT,
        numerator INTEGER NOT NULL,
        denominator INTEGER NOT NULL,
        percent REAL NOT NULL,
        basis TEXT,
        provenance TEXT NOT NULL,
        created_seq INTEGER NOT NULL
    );
    CREATE TABLE drafts (
        id TEXT PRIMARY KEY,
        base_seq INTEGER NOT NULL,
        created_seq INTEGER NOT NULL
    );
    CREATE TABLE candidates (
        id TEXT PRIMARY KEY,
        subject TEXT NOT NULL,
        options TEXT NOT NULL,
        created_seq INTEGER NOT NULL
    );
    "#,
    r#"
    CREATE TABLE diagnostics (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL,
        subject TEXT NOT NULL,
        message TEXT NOT NULL,
        created_seq INTEGER NOT NULL
    );
    ALTER TABLE candidates ADD COLUMN resolved_seq INTEGER;
    "#,
];

pub fn open(path: &str) -> Connection {
    let conn = Connection::open(path).expect("open sqlite database");
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .ok();
    migrate(&conn);
    conn
}

pub fn open_memory() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    migrate(&conn);
    conn
}

pub fn user_version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0)
}

pub fn migrate_to(conn: &Connection, upto: usize) {
    let cur = user_version(conn);
    for (i, ddl) in MIGRATIONS.iter().enumerate() {
        let version = i as i64 + 1;
        if version > cur && version <= upto as i64 {
            conn.execute_batch(ddl).expect("apply migration");
            conn.execute_batch(&format!("PRAGMA user_version={version};"))
                .expect("set user_version");
        }
    }
}

pub fn migrate(conn: &Connection) {
    migrate_to(conn, MIGRATIONS.len());
}
