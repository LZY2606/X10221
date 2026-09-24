//! SQLite schema 与状态迁移。每个迁移都是幂等执行的独立步骤，迁移过程保留旧数据。

use rusqlite::Connection;

/// v1：对象版本表、追加式事件日志、分支头。
/// v2：并发冲突登记表 + 分支标签列。
pub const MIGRATIONS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS objects (
        id TEXT NOT NULL,
        version INTEGER NOT NULL,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_event INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (id, version)
    );
    CREATE TABLE IF NOT EXISTS events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        branch TEXT NOT NULL,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL,
        snapshot TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE IF NOT EXISTS branches (
        id TEXT PRIMARY KEY,
        parent TEXT,
        head_event INTEGER NOT NULL DEFAULT 0
    );",
    "CREATE TABLE IF NOT EXISTS conflicts (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        branch TEXT NOT NULL,
        expected_head INTEGER NOT NULL,
        actual_head INTEGER NOT NULL,
        diff TEXT NOT NULL,
        incoming TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT ''
    );
    ALTER TABLE branches ADD COLUMN label TEXT NOT NULL DEFAULT '';",
];

/// 应用到指定版本（测试用：可先停在 v1 再整体迁移）。
pub fn migrate_to(conn: &Connection, target: u32) -> rusqlite::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let mut current = current as u32;
    while current < target {
        let next = current + 1;
        let sql = MIGRATIONS[(next - 1) as usize];
        conn.execute_batch(sql)?;
        conn.pragma_update(None, "user_version", next as i64)?;
        current = next;
    }
    Ok(())
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    migrate_to(conn, MIGRATIONS.len() as u32)
}

pub fn current_version(conn: &Connection) -> rusqlite::Result<u32> {
    let v: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(v as u32)
}
