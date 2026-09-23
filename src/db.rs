//! SQLite persistence primitives and migrations.
//!
//! Import rule: a load runs in a single transaction; any failure rolls the
//! whole change set back so no half-loaded record can remain.

use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Mutex;

pub struct Db {
    pub conn: Mutex<Connection>,
}

pub fn current_schema_version(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT user_version FROM pragma_user_version",
        [],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

pub fn set_schema_version(conn: &mut Connection, v: i64) {
    conn.execute_batch(&format!("PRAGMA user_version = {};", v))
        .expect("set user_version");
}

pub fn run_migrations(conn: &mut Connection) {
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let ver = current_schema_version(conn);
    if ver < 1 {
        conn.execute_batch(include_str!("../migrations/0001_init.sql"))
            .expect("migration 1");
        set_schema_version(conn, 1);
    }
    if ver < 2 {
        conn.execute_batch(include_str!("../migrations/0002_branch_archived.sql"))
            .expect("migration 2");
        set_schema_version(conn, 2);
    }
}

impl Db {
    pub fn open(path: &str) -> Self {
        let mut conn = Connection::open(path).expect("open sqlite");
        run_migrations(&mut conn);
        Db {
            conn: Mutex::new(conn),
        }
    }

    pub fn in_memory() -> Self {
        let mut conn = Connection::open_in_memory().expect("in-memory sqlite");
        run_migrations(&mut conn);
        Db {
            conn: Mutex::new(conn),
        }
    }

    pub fn case_exists(&self, case_id: &str) -> bool {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT 1 FROM cases WHERE case_id = ?1", params![case_id], |_| Ok(()))
            .optional()
            .unwrap()
            .is_some()
    }
}

/// A clock that can be frozen for deterministic replay.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

pub struct FixedClock(pub i64);
impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}
