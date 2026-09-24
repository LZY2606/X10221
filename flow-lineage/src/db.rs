//! SQLite storage with explicit schema migrations.
//!
//! Append-only design: entity revisions, branch head moves, decisions,
//! invalidations and conflicts are all ledger events. Old results are never
//! overwritten; they are marked stale via invalidation markers.

use rusqlite::{params, Connection};
use std::sync::Mutex;

pub struct Db(pub Mutex<Connection>);

pub const SCHEMA_VERSION: i64 = 2;

pub fn open(path: &str) -> anyhow::Result<crate::store::Store> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let mut s = crate::store::Store::new(conn);
    s.migrate()?;
    Ok(s)
}

pub fn open_memory() -> anyhow::Result<crate::store::Store> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let mut s = crate::store::Store::new(conn);
    s.migrate()?;
    Ok(s)
}

const MIGRATION_1: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_2: &str = include_str!("../migrations/0002_archive_labels.sql");

pub fn migrations() -> Vec<(i64, &'static str)> {
    vec![(1, MIGRATION_1), (2, MIGRATION_2)]
}
