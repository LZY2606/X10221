//! SQLite 模式与迁移。迁移按 PRAGMA user_version 逐步推进，便于状态迁移测试。

use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[
    // v1：对象版本表、事件日志、细胞群结果、草案。
    "
    CREATE TABLE objects (
        id TEXT NOT NULL,
        version INTEGER NOT NULL,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_seq INTEGER NOT NULL,
        PRIMARY KEY (id, version)
    );
    CREATE TABLE events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        id TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        rule TEXT,
        status TEXT,
        payload TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE TABLE populations (
        id TEXT NOT NULL,
        draft_id TEXT NOT NULL,
        gate_id TEXT NOT NULL,
        gate_version INTEGER NOT NULL,
        parent_pop_id TEXT,
        parent_pop_version INTEGER,
        version INTEGER NOT NULL,
        count INTEGER,
        percent REAL,
        denominator_count INTEGER,
        status TEXT NOT NULL,
        candidates TEXT NOT NULL DEFAULT '[]',
        chosen TEXT,
        diag TEXT NOT NULL DEFAULT '{}',
        created_seq INTEGER NOT NULL,
        retired_seq INTEGER,
        PRIMARY KEY (id, version)
    );
    CREATE TABLE drafts (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        payload TEXT NOT NULL DEFAULT '{}',
        created_seq INTEGER NOT NULL
    );
    ",
    // v2：人工裁定分支表（可撤销）与事件索引。
    "
    CREATE TABLE adjudications (
        id TEXT PRIMARY KEY,
        draft_id TEXT NOT NULL,
        target_id TEXT NOT NULL,
        decision TEXT NOT NULL,
        payload TEXT NOT NULL,
        base_seq INTEGER NOT NULL,
        created_seq INTEGER NOT NULL,
        retired_seq INTEGER
    );
    CREATE INDEX idx_events_kind ON events(kind);
    CREATE INDEX idx_pop_visible ON populations(draft_id, retired_seq);
    ",
];

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = (i + 1) as i64;
        if current < v {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", v)?;
            tx.commit()?;
        }
    }
    Ok(())
}

/// 测试辅助：只迁移到指定版本。
pub fn migrate_to(conn: &Connection, target: i64) -> rusqlite::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = (i + 1) as i64;
        if current < v && v <= target {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", v)?;
            tx.commit()?;
        }
    }
    Ok(())
}

pub fn open(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    Ok(conn)
}
