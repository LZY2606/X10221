//! SQLite connection management and versioned migrations.

use rusqlite::Connection;
use std::path::Path;

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("../migrations/0001_init.sql")),
    ("0002_review_note", include_str!("../migrations/0002_review_note.sql")),
];

pub fn migrate(conn: &mut Connection) -> crate::error::AppResult<i64> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0);
    let mut v = version;
    for (name, sql) in MIGRATIONS {
        let idx = name.split('_').next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        if idx <= v { continue; }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", idx)?;
        tx.commit()?;
        v = idx;
    }
    Ok(v)
}

pub fn open(path: &Path) -> crate::error::AppResult<Connection> {
    let mut conn = Connection::open(path)?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn open_memory() -> crate::error::AppResult<Connection> {
    let mut conn = Connection::open_in_memory()?;
    migrate(&mut conn)?;
    Ok(conn)
}
