//! SQLite 持久层：六个独立版本化的存储、事件流、群体、依赖、候选与诊断。
//! 迁移按 user_version 逐级推进，迁移测试覆盖旧库升级。

use rusqlite::{params, Connection};

/// v1 基线结构（保留常量以便迁移测试从旧版本升级）。
pub const SCHEMA_V1: &str = r#"
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE seq(name TEXT PRIMARY KEY, value INTEGER NOT NULL);
CREATE TABLE store_versions(
  store TEXT NOT NULL,
  version INTEGER NOT NULL,
  id TEXT NOT NULL,
  supersedes TEXT,
  payload TEXT NOT NULL,
  branch TEXT NOT NULL DEFAULT 'main',
  created_seq INTEGER NOT NULL,
  PRIMARY KEY(store, version)
);
CREATE TABLE events(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,
  branch TEXT NOT NULL DEFAULT 'main',
  payload TEXT NOT NULL
);
CREATE TABLE populations(
  id TEXT NOT NULL,
  version INTEGER NOT NULL,
  gate_id TEXT NOT NULL,
  gate_version INTEGER NOT NULL,
  parent_pop_id TEXT,
  parent_pop_version INTEGER,
  event_count INTEGER NOT NULL,
  denominator_count INTEGER NOT NULL,
  percent REAL NOT NULL,
  status TEXT NOT NULL,
  basis TEXT NOT NULL,
  created_seq INTEGER NOT NULL,
  PRIMARY KEY(id, version)
);
CREATE TABLE deps(
  dependent_id TEXT NOT NULL,
  source_store TEXT NOT NULL,
  source_key TEXT NOT NULL,
  source_version INTEGER NOT NULL
);
CREATE TABLE candidates(
  id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  options TEXT NOT NULL,
  status TEXT NOT NULL,
  ruling TEXT,
  branch TEXT NOT NULL DEFAULT 'main',
  created_seq INTEGER NOT NULL
);
"#;

const MIGRATION_V2: &str = r#"
CREATE TABLE diagnostics(
  id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  raw_repr TEXT NOT NULL,
  message TEXT NOT NULL,
  created_seq INTEGER NOT NULL
);
"#;

const MIGRATION_V3: &str = r#"
ALTER TABLE candidates ADD COLUMN decided_seq INTEGER;
"#;

pub const STORES: [&str; 6] = [
    "raw_channels",
    "spillover",
    "comp_matrix",
    "transform_params",
    "gate_polygons",
    "gate_relations",
];

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v < 1 {
        conn.execute_batch(SCHEMA_V1)?;
        conn.execute_batch("PRAGMA user_version=1;")?;
    }
    if v < 2 {
        conn.execute_batch(MIGRATION_V2)?;
        conn.execute_batch("PRAGMA user_version=2;")?;
    }
    if v < 3 {
        conn.execute_batch(MIGRATION_V3)?;
        conn.execute_batch("PRAGMA user_version=3;")?;
    }
    Ok(())
}

/// 单调序号发生器（用于事件 id、候选 id 等稳定身份）。
pub fn next_seq(conn: &Connection, name: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO seq(name,value) VALUES(?1,1)
         ON CONFLICT(name) DO UPDATE SET value=value+1",
        params![name],
    )?;
    conn.query_row("SELECT value FROM seq WHERE name=?1", params![name], |r| {
        r.get(0)
    })
}

pub fn new_id(conn: &Connection, prefix: &str) -> rusqlite::Result<String> {
    Ok(format!("{}-{:06}", prefix, next_seq(conn, prefix)?))
}

/// 写入某存储的下一版本；版本号按存储各自独立递增（分开版本化）。
pub fn put_version(
    conn: &Connection,
    store: &str,
    id: &str,
    supersedes: Option<&str>,
    payload: &serde_json::Value,
    branch: &str,
) -> rusqlite::Result<i64> {
    let seq = next_seq(conn, "global")?;
    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version),0)+1 FROM store_versions WHERE store=?1",
        params![store],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO store_versions(store,version,id,supersedes,payload,branch,created_seq)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            store,
            version,
            id,
            supersedes,
            serde_json::to_string(payload).unwrap(),
            branch,
            seq
        ],
    )?;
    Ok(version)
}

pub fn latest(
    conn: &Connection,
    store: &str,
) -> rusqlite::Result<Option<(i64, String, serde_json::Value)>> {
    let mut stmt = conn.prepare(
        "SELECT version,id,payload FROM store_versions WHERE store=?1 ORDER BY version DESC LIMIT 1",
    )?;
    let mut rows = stmt.query_map(params![store], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })?;
    match rows.next() {
        Some(Ok((v, id, p))) => Ok(Some((v, id, serde_json::from_str(&p).unwrap()))),
        _ => Ok(None),
    }
}

/// 追加一条机器可读事件；事件是判断依据的唯一事实来源。
pub fn emit_event(
    conn: &Connection,
    kind: &str,
    branch: &str,
    payload: serde_json::Value,
) -> rusqlite::Result<(i64, String)> {
    let id = new_id(conn, "evt")?;
    conn.execute(
        "INSERT INTO events(id,kind,branch,payload) VALUES(?1,?2,?3,?4)",
        params![id, kind, branch, serde_json::to_string(&payload).unwrap()],
    )?;
    let seq: i64 = conn.query_row("SELECT seq FROM events WHERE id=?1", params![id], |r| r.get(0))?;
    Ok((seq, id))
}

pub fn add_diagnostic(
    conn: &Connection,
    subject: &str,
    raw_repr: &str,
    message: &str,
) -> rusqlite::Result<String> {
    let id = new_id(conn, "diag")?;
    let seq = next_seq(conn, "global")?;
    conn.execute(
        "INSERT INTO diagnostics(id,subject,raw_repr,message,created_seq) VALUES(?1,?2,?3,?4,?5)",
        params![id, subject, raw_repr, message, seq],
    )?;
    Ok(id)
}
