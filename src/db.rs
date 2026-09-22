use rusqlite::{params, Connection};
use serde_json::Value;

pub const M1: &str = r#"
CREATE TABLE IF NOT EXISTS artifacts(
  id TEXT NOT NULL,
  version INTEGER NOT NULL,
  kind TEXT NOT NULL,
  branch TEXT NOT NULL DEFAULT 'main',
  content TEXT NOT NULL,
  raw TEXT,
  media_type TEXT,
  created_seq INTEGER NOT NULL,
  PRIMARY KEY(id, version, branch)
);
CREATE TABLE IF NOT EXISTS events(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL,
  branch TEXT NOT NULL DEFAULT 'main',
  payload TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS results(
  result_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  branch TEXT NOT NULL DEFAULT 'main',
  gate_id TEXT NOT NULL,
  gate_version INTEGER NOT NULL,
  count INTEGER NOT NULL DEFAULT 0,
  percent REAL,
  denom_gate_id TEXT,
  denom_gate_version INTEGER,
  denom_count INTEGER,
  blocked INTEGER NOT NULL DEFAULT 0,
  candidates TEXT NOT NULL DEFAULT '[]',
  members TEXT NOT NULL DEFAULT '[]',
  provenance TEXT NOT NULL DEFAULT '{}',
  created_seq INTEGER NOT NULL,
  PRIMARY KEY(result_id, version, branch)
);
CREATE TABLE IF NOT EXISTS result_inputs(
  result_id TEXT NOT NULL,
  result_version INTEGER NOT NULL,
  branch TEXT NOT NULL DEFAULT 'main',
  kind TEXT NOT NULL,
  ref_id TEXT NOT NULL,
  ref_version INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS drafts(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  head_version INTEGER NOT NULL,
  content TEXT NOT NULL,
  updated_seq INTEGER NOT NULL
);
"#;

pub const M2: &str = r#"
ALTER TABLE results ADD COLUMN diagnostics TEXT NOT NULL DEFAULT '[]';
"#;

pub const MIGRATIONS: &[&str] = &[M1, M2];

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn migrate_to(conn: &Connection, target: usize) -> rusqlite::Result<usize> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(
           version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);",
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    let mut applied = current as usize;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = i + 1;
        if v > current as usize && v <= target {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (?1,?2)",
                params![v as i64, now_secs()],
            )?;
            applied = v;
        }
    }
    Ok(applied)
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<usize> {
    migrate_to(conn, MIGRATIONS.len())
}

pub fn max_seq(conn: &Connection) -> i64 {
    conn.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))
        .unwrap_or(0)
}

pub fn record_event(conn: &Connection, kind: &str, branch: &str, payload: &Value) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO events(kind, branch, payload, created_at) VALUES (?1,?2,?3,?4)",
        params![kind, branch, payload.to_string(), now_secs()],
    )?;
    let seq = conn.last_insert_rowid();
    conn.execute(
        "UPDATE events SET id = 'evt-' || printf('%06d', seq) WHERE seq = ?1",
        params![seq],
    )?;
    Ok(seq)
}

pub fn latest_artifact(conn: &Connection, kind: &str, id: &str, branch: &str) -> Option<(i64, Value)> {
    conn.query_row(
        "SELECT version, content FROM artifacts
         WHERE kind=?1 AND id=?2 AND branch=?3 ORDER BY version DESC LIMIT 1",
        params![kind, id, branch],
        |r| {
            let v: i64 = r.get(0)?;
            let c: String = r.get(1)?;
            Ok((v, c))
        },
    )
    .ok()
    .and_then(|(v, c)| serde_json::from_str(&c).ok().map(|j| (v, j)))
}

pub fn insert_artifact(
    conn: &Connection,
    kind: &str,
    id: &str,
    branch: &str,
    content: &Value,
    raw: Option<&str>,
    media_type: &str,
) -> rusqlite::Result<i64> {
    let prev: Option<i64> = conn
        .query_row(
            "SELECT MAX(version) FROM artifacts WHERE kind=?1 AND id=?2 AND branch=?3",
            params![kind, id, branch],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    let version = prev.unwrap_or(0) + 1;
    let seq = max_seq(conn);
    conn.execute(
        "INSERT INTO artifacts(id, version, kind, branch, content, raw, media_type, created_seq)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![id, version, kind, branch, content.to_string(), raw, media_type, seq],
    )?;
    Ok(version)
}
