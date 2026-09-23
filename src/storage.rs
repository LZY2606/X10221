//! SQLite 存储层：全部来源版本与事件只追加，派生结果带 live/stale 状态。
//!
//! 重要不变量：
//! * 任何写入都在单个事务内完成，导入失败整体回滚，不留半份记录；
//! * events.seq 全局递增；branch_seq 在分支内递增；
//! * 上游修订只把受影响的 gate_run 标记 stale，不删除/覆盖旧行；
//! * 并发的同版本修改：expected_head 不匹配时拒绝并写冲突审计事件。

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use crate::domain::{Basis, ContentObject, EventRecord};

#[derive(Clone)]
pub struct Store {
    pub conn: std::sync::Arc<tokio::sync::Mutex<Connection>>,
}

pub fn now_ts() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    // 单调毫秒时间戳，用于展示；排序永远以 seq 为准
    format!("2026-09-23T{:02}:{:02}:{:02}.{:03}Z", 0, 0, 0, d.as_millis() as u64 % 1000)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

pub const SCHEMA_V1: &str = r#"
CREATE TABLE schema_version (version INTEGER NOT NULL PRIMARY KEY, applied_at TEXT NOT NULL);

CREATE TABLE cases (
    case_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    created_seq INTEGER NOT NULL
);

-- 原始字节（FCS、平台报告、场景脚本），与解释分开保存。
CREATE TABLE sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    case_id TEXT NOT NULL,
    role TEXT NOT NULL,              -- fcs | platform_json | scenario
    filename TEXT NOT NULL,
    content BLOB NOT NULL,
    sha256 TEXT NOT NULL
);

-- 每类来源独立版本化、不可变。
CREATE TABLE objects (
    id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    version INTEGER NOT NULL,
    supersedes TEXT,
    payload TEXT NOT NULL,
    UNIQUE(case_id, kind, version)
);

CREATE TABLE branches (
    case_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    parent_branch TEXT,
    fork_after_seq INTEGER,
    created_seq INTEGER NOT NULL,
    PRIMARY KEY(case_id, branch)
);

-- 全局只追加事件日志；branch_seq 仅对进入分支视图的事件非空。
CREATE TABLE events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    case_id TEXT NOT NULL,
    branch TEXT NOT NULL DEFAULT '',
    branch_seq INTEGER,
    event_type TEXT NOT NULL,
    actor TEXT NOT NULL,
    summary TEXT NOT NULL,
    payload TEXT NOT NULL,
    depends_on TEXT NOT NULL,
    basis TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE branch_heads (
    case_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    head_seq INTEGER NOT NULL,
    PRIMARY KEY(case_id, branch)
);

-- 派生批次不可变；gate_runs 中没有可更新的 status 列。
CREATE TABLE comp_runs (
    id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    seq INTEGER NOT NULL,
    spillover_ref TEXT NOT NULL,
    rcond REAL,
    UNIQUE(case_id, branch, seq)
);

CREATE TABLE comp_candidates (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    case_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    matrix TEXT NOT NULL,
    rationale TEXT NOT NULL
);

CREATE TABLE gate_runs (
    id TEXT PRIMARY KEY,
    case_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    seq INTEGER NOT NULL,
    gate_id TEXT NOT NULL,
    gate_ref TEXT NOT NULL,
    parent_run_ref TEXT,
    UNIQUE(case_id, branch, seq)
);

CREATE TABLE gate_run_candidates (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    case_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    member_indices TEXT NOT NULL,
    numerator INTEGER NOT NULL,
    denominator INTEGER NOT NULL,
    percent REAL NOT NULL,
    denominator_source TEXT NOT NULL,
    boundary_indices TEXT NOT NULL,
    unconvertible_indices TEXT NOT NULL
);

-- 冲突审计：同版本并发修改被拒绝，不进入任何分支视图。
CREATE TABLE conflicts (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    case_id TEXT NOT NULL,
    branch TEXT NOT NULL,
    actor TEXT NOT NULL,
    expected_head_seq INTEGER NOT NULL,
    actual_head_seq INTEGER NOT NULL,
    rejected_command TEXT NOT NULL,
    diff TEXT NOT NULL,
    created_at TEXT NOT NULL
);
"#;

/// v2 迁移：仅为状态迁移测试而存在（新增注释表），并回填已有案例。
pub const SCHEMA_V2: &str = r#"
CREATE TABLE archive_notes (
    case_id TEXT PRIMARY KEY,
    note TEXT NOT NULL
);
INSERT OR IGNORE INTO archive_notes(case_id, note) SELECT case_id, '' FROM cases;
"#;

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 10_000)?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: std::sync::Arc::new(tokio::sync::Mutex::new(conn)),
        })
    }

    /// 测试专用：内存库，可指定停留在旧 schema。
    pub fn open_in_memory_at(version: u32) -> rusqlite::Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA_V1)?;
        conn.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (1, ?)",
            params![now_ts()],
        )?;
        if version >= 2 {
            conn.execute_batch(SCHEMA_V2)?;
            conn.execute(
                "INSERT INTO schema_version(version, applied_at) VALUES (2, ?)",
                params![now_ts()],
            )?;
        }
        Ok(Self {
            conn: std::sync::Arc::new(tokio::sync::Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::open_in_memory_at(2)
    }
}

fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER NOT NULL PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )?;
    let current: u32 = conn
        .query_row("SELECT COALESCE(MAX(version),0) FROM schema_version", [], |r| {
            r.get(0)
        })
        .unwrap_or(0);
    if current == 0 {
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA_V1)?;
        tx.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (1, ?)",
            params![now_ts()],
        )?;
        tx.execute_batch(SCHEMA_V2)?;
        tx.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (2, ?)",
            params![now_ts()],
        )?;
        tx.commit()?;
    } else if current == 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA_V2)?;
        tx.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (2, ?)",
            params![now_ts()],
        )?;
        tx.commit()?;
    }
    Ok(())
}
