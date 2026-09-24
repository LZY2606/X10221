//! SQLite 持久层：模式迁移 + 事件/工件/结果/诊断的存取。
//! 所有领域对象分开版本化：原始通道值、spillover 元数据、补偿矩阵、
//! 变换参数、门多边形、父子关系各自一张版本序列，互不覆盖。

use rusqlite::{params, Connection, Result};
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub type Db = Arc<Mutex<Connection>>;

/// 迁移序列。v1 为核心表，v2 增加 raw_blobs（导入原始字节留存）。
pub const MIGRATIONS: &[(u32, &str)] = &[
    (
        1,
        "CREATE TABLE events(
            event_id TEXT PRIMARY KEY,
            seq INTEGER NOT NULL UNIQUE,
            kind TEXT NOT NULL,
            payload TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE artifacts(
            artifact_id TEXT NOT NULL,
            version INTEGER NOT NULL,
            kind TEXT NOT NULL,
            payload TEXT NOT NULL,
            event_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            PRIMARY KEY(artifact_id, version)
        );
        CREATE TABLE results(
            result_id TEXT PRIMARY KEY,
            draft_id TEXT NOT NULL,
            population TEXT NOT NULL,
            payload TEXT NOT NULL,
            seq INTEGER NOT NULL
        );
        CREATE TABLE drafts(
            draft_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            pins TEXT NOT NULL,
            rulings TEXT NOT NULL,
            base_seq INTEGER NOT NULL
        );
        CREATE TABLE draft_history(
            draft_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            pins TEXT NOT NULL,
            rulings TEXT NOT NULL,
            PRIMARY KEY(draft_id, seq)
        );
        CREATE TABLE diagnostics(
            diag_id TEXT PRIMARY KEY,
            code TEXT NOT NULL,
            severity TEXT NOT NULL,
            payload TEXT NOT NULL,
            event_id TEXT NOT NULL,
            seq INTEGER NOT NULL
        );",
    ),
    (
        2,
        "CREATE TABLE raw_blobs(
            blob_id TEXT PRIMARY KEY,
            filename TEXT NOT NULL,
            bytes BLOB NOT NULL,
            event_id TEXT NOT NULL,
            seq INTEGER NOT NULL
        );",
    ),
];

pub fn migrate(conn: &Connection, upto: Option<u32>) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )?;
    let current: u32 = conn.query_row(
        "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    for (v, sql) in MIGRATIONS {
        if *v > current && upto.map_or(true, |u| *v <= u) {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, ?2)",
                params![v, format!("migration-{}", v)],
            )?;
        }
    }
    Ok(())
}

pub fn open(path: &str) -> Result<Db> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    migrate(&conn, None)?;
    Ok(Arc::new(Mutex::new(conn)))
}

pub fn open_memory() -> Result<Db> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn, None)?;
    Ok(Arc::new(Mutex::new(conn)))
}

pub fn next_seq(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq),0)+1 FROM events",
        [],
        |r| r.get(0),
    )
}

/// 追加一条机器可读取事件。event_id 稳定（evt-NNNNNN），顺序即 seq。
pub fn append_event(conn: &Connection, kind: &str, payload: &Value) -> Result<(String, i64)> {
    let seq = next_seq(conn)?;
    let event_id = format!("evt-{:06}", seq);
    conn.execute(
        "INSERT INTO events(event_id, seq, kind, payload, created_at) VALUES(?1,?2,?3,?4,?5)",
        params![
            event_id,
            seq,
            kind,
            serde_json::to_string(payload).unwrap(),
            format!("seq:{:06}", seq)
        ],
    )?;
    Ok((event_id, seq))
}

/// 写入工件新版本（版本号 = 当前最大+1），返回版本号。旧版本永不被改写。
pub fn put_artifact(
    conn: &Connection,
    artifact_id: &str,
    kind: &str,
    payload: &Value,
    event_id: &str,
    seq: i64,
) -> Result<i64> {
    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version),0)+1 FROM artifacts WHERE artifact_id=?1",
        params![artifact_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO artifacts(artifact_id, version, kind, payload, event_id, seq)
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            artifact_id,
            version,
            kind,
            serde_json::to_string(payload).unwrap(),
            event_id,
            seq
        ],
    )?;
    Ok(version)
}

pub fn get_artifact(conn: &Connection, artifact_id: &str, version: i64) -> Result<Value> {
    let s: String = conn.query_row(
        "SELECT payload FROM artifacts WHERE artifact_id=?1 AND version=?2",
        params![artifact_id, version],
        |r| r.get(0),
    )?;
    Ok(serde_json::from_str(&s).unwrap())
}

pub fn latest_version(conn: &Connection, artifact_id: &str) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT MAX(version) FROM artifacts WHERE artifact_id=?1",
        params![artifact_id],
        |r| r.get(0),
    )
}

pub struct DraftRow {
    pub draft_id: String,
    pub name: String,
    pub pins: Value,
    pub rulings: Value,
    pub base_seq: i64,
}

pub fn get_draft(conn: &Connection, draft_id: &str) -> Result<DraftRow> {
    conn.query_row(
        "SELECT draft_id, name, pins, rulings, base_seq FROM drafts WHERE draft_id=?1",
        params![draft_id],
        |r| {
            Ok(DraftRow {
                draft_id: r.get(0)?,
                name: r.get(1)?,
                pins: serde_json::from_str(&r.get::<_, String>(2)?).unwrap(),
                rulings: serde_json::from_str(&r.get::<_, String>(3)?).unwrap(),
                base_seq: r.get(4)?,
            })
        },
    )
}

pub fn list_draft_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut st = conn.prepare("SELECT draft_id FROM drafts ORDER BY draft_id")?;
    let rows = st.query_map([], |r| r.get(0))?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

/// 创建或更新草案；每次变更都写 draft_history，供“回到任意旧事件”重放。
pub fn save_draft(
    conn: &Connection,
    draft_id: &str,
    name: &str,
    pins: &Value,
    rulings: &Value,
    seq: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO drafts(draft_id, name, pins, rulings, base_seq) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(draft_id) DO UPDATE SET pins=excluded.pins, rulings=excluded.rulings, base_seq=excluded.base_seq",
        params![draft_id, name, serde_json::to_string(pins).unwrap(), serde_json::to_string(rulings).unwrap(), seq],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO draft_history(draft_id, seq, pins, rulings) VALUES(?1,?2,?3,?4)",
        params![draft_id, seq, serde_json::to_string(pins).unwrap(), serde_json::to_string(rulings).unwrap()],
    )?;
    Ok(())
}

pub fn add_diag(
    conn: &Connection,
    diag_id: &str,
    code: &str,
    severity: &str,
    payload: &Value,
    event_id: &str,
    seq: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO diagnostics(diag_id, code, severity, payload, event_id, seq)
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![diag_id, code, severity, serde_json::to_string(payload).unwrap(), event_id, seq],
    )?;
    Ok(())
}

pub fn put_blob(
    conn: &Connection,
    blob_id: &str,
    filename: &str,
    bytes: &[u8],
    event_id: &str,
    seq: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO raw_blobs(blob_id, filename, bytes, event_id, seq) VALUES(?1,?2,?3,?4,?5)",
        params![blob_id, filename, bytes, event_id, seq],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_is_stepwise_and_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        // 只迁移到 v1：核心表存在，raw_blobs 不存在
        migrate(&conn, Some(1)).unwrap();
        let v: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 1);
        let has_blobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='raw_blobs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_blobs, 0);
        // 继续迁移到最新：raw_blobs 出现
        migrate(&conn, None).unwrap();
        let v: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.last().unwrap().0);
        let has_blobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='raw_blobs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_blobs, 1);
        // 幂等：再次迁移不出错、版本不变
        migrate(&conn, None).unwrap();
        let v2: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v2, v);
    }

    #[test]
    fn events_have_stable_ids_and_order() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn, None).unwrap();
        let (a, sa) = append_event(&conn, "import", &serde_json::json!({"n":1})).unwrap();
        let (b, sb) = append_event(&conn, "ruling", &serde_json::json!({"n":2})).unwrap();
        assert_eq!(a, "evt-000001");
        assert_eq!(b, "evt-000002");
        assert!(sb > sa);
    }
}
