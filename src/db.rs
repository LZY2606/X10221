//! SQLite 存储层：schema 与状态迁移。
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 2;

/// 迁移脚本：按版本顺序执行。v1 建立基线表，v2 增加分支头指针表。
pub const MIGRATIONS: &[(i64, &str)] = &[
    (
        1,
        r#"
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
-- 分开版本化的六类材料 + 派生群体共用的版本实体表。
-- id 为稳定身份（如 gate:lymphocytes），同一 id 多版本共存，互不覆盖。
CREATE TABLE entities (
    id TEXT NOT NULL,
    kind TEXT NOT NULL,
    version INTEGER NOT NULL,
    branch TEXT NOT NULL DEFAULT 'main',
    payload TEXT NOT NULL,
    supersedes TEXT,
    event_seq INTEGER NOT NULL REFERENCES events(seq),
    PRIMARY KEY (id, version, branch)
);
CREATE TABLE populations (
    pop_id TEXT NOT NULL,
    gate_ref TEXT NOT NULL,          -- 形如 gate:lymphocytes@2，钉住门版本
    parent_pop_ref TEXT,             -- 父群体版本引用，百分比分母钉住它
    percent REAL,
    denominator_ref TEXT,            -- 与 parent_pop_ref 一致，显式记录分母来源
    status TEXT NOT NULL DEFAULT 'current',  -- current | stale
    branch TEXT NOT NULL DEFAULT 'main',
    event_seq INTEGER NOT NULL REFERENCES events(seq),
    PRIMARY KEY (pop_id, branch)
);
CREATE TABLE candidates (
    cand_id TEXT NOT NULL,
    subject TEXT NOT NULL,           -- 例如 comp:FL2-FL1 或 gate:boundary:evt42
    payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',  -- pending | adopted | rejected
    branch TEXT NOT NULL DEFAULT 'main',
    event_seq INTEGER NOT NULL REFERENCES events(seq),
    PRIMARY KEY (cand_id, branch)
);
CREATE TABLE decisions (
    decision_id TEXT PRIMARY KEY,
    subject TEXT NOT NULL,
    verdict TEXT NOT NULL,           -- accept | reject | adjust
    payload TEXT NOT NULL,
    prev_head TEXT,                  -- 裁定前分支头，用于撤销
    undone INTEGER NOT NULL DEFAULT 0,
    event_seq INTEGER NOT NULL REFERENCES events(seq)
);
CREATE TABLE diagnostics (
    diag_id TEXT PRIMARY KEY,
    subject TEXT NOT NULL,
    severity TEXT NOT NULL,
    message TEXT NOT NULL,
    raw_value TEXT,                  -- 无法转换的值留在原位置
    event_seq INTEGER NOT NULL REFERENCES events(seq)
);
"#,
    ),
    (
        2,
        r#"
-- 每个 subject 的当前分支头指针；撤销裁定即回拨指针，不改写历史。
CREATE TABLE heads (
    subject TEXT PRIMARY KEY,
    head TEXT NOT NULL
);
"#,
    ),
];

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    Ok(conn)
}

pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory_with_flags(OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

pub fn current_version(conn: &Connection) -> rusqlite::Result<i64> {
    let v: Result<String, _> = conn.query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0));
    Ok(v.ok().and_then(|s| s.parse().ok()).unwrap_or(0))
}

/// 执行所有未应用的迁移，返回应用到的版本号列表。
pub fn migrate(conn: &Connection) -> rusqlite::Result<Vec<i64>> {
    let mut applied = Vec::new();
    let mut v = current_version(conn);
    for (ver, sql) in MIGRATIONS {
        if *ver > v {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT OR REPLACE INTO meta(key,value) VALUES('schema_version', ?1)",
                [ver.to_string()],
            )?;
            applied.push(*ver);
            v = *ver;
        }
    }
    Ok(applied)
}
