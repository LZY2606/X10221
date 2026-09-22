//! SQLite 存储：事件账本、版本化材料、分支、裁定与派生物快照。
use rusqlite::{params, Connection};
use serde_json::Value;
use std::sync::Mutex;

pub const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
pub const MIGRATION_0002: &str = include_str!("../migrations/0002_sessions.sql");

#[derive(Clone, Debug)]
pub struct RowEvent {
    pub seq: i64,
    pub kind: String,
    pub subject: String,
    pub payload: Value,
    pub basis: Value,
    pub basis_rules: String,
    pub mat_kind: Option<String>,
    pub mat_id: Option<String>,
    pub mat_version: Option<i64>,
    pub origin_seq: Option<i64>,
    pub origin_branch: Option<String>,
}

#[derive(Clone)]
pub struct Store {
    conn: Mutex<Connection>,
}

fn current_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='schema_version'",
        [],
        |_| Ok(()),
    )
    .optional()?;
    let v: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap_or(0);
    Ok(v)
}

trait OptionalExt<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}
impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl Store {
    /// 在全新数据库上按顺序执行全部迁移。
    pub fn open(path: &str) -> anyhow_lite::Result<Store> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        Self::migrate(&conn, 2)?;
        Ok(Store { conn: Mutex::new(conn) })
    }

    /// 从指定历史版本开始迁移（状态迁移测试用）：先建 v1，再升级到 v2。
    pub fn open_at(path: &str, version: i64) -> anyhow_lite::Result<Store> {
        let conn = Connection::open(path)?;
        Self::migrate(&conn, version)?;
        Ok(Store { conn: Mutex::new(conn) })
    }

    pub fn in_memory() -> anyhow_lite::Result<Store> {
        let conn = Connection::open_in_memory()?;
        Self::migrate(&conn, 2)?;
        Ok(Store { conn: Mutex::new(conn) })
    }

    pub fn migrate(conn: &Connection, target: i64) -> anyhow_lite::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL, applied_at TEXT NOT NULL DEFAULT '')",
        )?;
        let v = current_version(conn)?;
        if v < 1 && target >= 1 {
            conn.execute_batch(MIGRATION_0001)?;
            conn.execute("DELETE FROM schema_version", [])?;
            conn.execute("INSERT INTO schema_version(version) VALUES(1)", [])?;
        }
        if v < 2 && target >= 2 {
            conn.execute_batch(MIGRATION_0002)?;
            conn.execute("UPDATE schema_version SET version=2", [])?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> i64 {
        let c = self.conn.lock().unwrap();
        c.query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap_or(0)
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }
}

/// 事务闭包；失败自动回滚，导入绝不允许留下半份记录。
pub fn tx<F, T>(conn: &Connection, f: F) -> anyhow_lite::Result<T>
where
    F: FnOnce(&rusqlite::Transaction) -> anyhow_lite::Result<T>,
{
    let t = conn.unchecked_transaction()?;
    match f(&t) {
        Ok(v) => {
            t.commit()?;
            Ok(v)
        }
        Err(e) => {
            t.rollback().ok();
            Err(e)
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let d = h.finalize();
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 轻量 anyhow 替身：保持依赖最小。
pub mod anyhow_lite {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
}
