//! SQLite 持久化：追加式事件账本 + 构件版本化 + 分支头（乐观并发）。

use crate::Result;
use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 2;

pub fn open(path: &str) -> Result<Connection> {
    let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.pragma_update(None, "foreign_keys", "ON").ok();
    conn.pragma_update(None, "journal_mode", "WAL").ok();
    migrate(&mut conn)?;
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> Result<()> {
    conn.pragma_update(None, "user_version", SCHEMA_VERSION).ok();
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS studies(
          study_id TEXT PRIMARY KEY,
          title TEXT NOT NULL,
          created_at TEXT NOT NULL,
          schema_version INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS raw_blobs(
          blob_id TEXT PRIMARY KEY,
          rel_path TEXT NOT NULL,
          sha256 TEXT NOT NULL,
          bytes BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS events(
          event_id TEXT PRIMARY KEY,
          seq INTEGER NOT NULL,
          branch_id TEXT NOT NULL,
          parent_event_id TEXT,
          kind TEXT NOT NULL,
          payload_json TEXT NOT NULL,
          basis_revision INTEGER NOT NULL,
          created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS branches(
          branch_id TEXT PRIMARY KEY,
          study_id TEXT NOT NULL,
          name TEXT NOT NULL,
          fork_event_id TEXT,
          head_event_id TEXT,
          revision INTEGER NOT NULL DEFAULT 0,
          created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS artifacts(
          artifact_id TEXT NOT NULL,
          version INTEGER NOT NULL,
          kind TEXT NOT NULL,
          logical_id TEXT NOT NULL,
          content_json TEXT NOT NULL,
          sha256 TEXT NOT NULL,
          introduced_event_id TEXT NOT NULL,
          superseded_event_id TEXT,
          PRIMARY KEY(artifact_id, version)
        );
        CREATE TABLE IF NOT EXISTS comp_evaluations(
          eval_id TEXT PRIMARY KEY,
          matrix_artifact TEXT NOT NULL,
          matrix_version INTEGER NOT NULL,
          status TEXT NOT NULL,
          rcond REAL NOT NULL,
          analysis_json TEXT NOT NULL,
          event_id TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS comp_decisions(
          branch_id TEXT NOT NULL,
          eval_id TEXT NOT NULL,
          strategy TEXT,
          decided_event_id TEXT,
          undone_event_id TEXT,
          PRIMARY KEY(branch_id, eval_id)
        );
        CREATE TABLE IF NOT EXISTS pop_results(
          result_id TEXT NOT NULL,
          gate_logical TEXT NOT NULL,
          branch_id TEXT NOT NULL,
          event_id TEXT NOT NULL,
          count INTEGER NOT NULL,
          percent REAL NOT NULL,
          denominator_result_id TEXT,
          denominator_event_id TEXT,
          deps_json TEXT NOT NULL,
          status TEXT NOT NULL,
          stale INTEGER NOT NULL DEFAULT 0,
          superseded_by_event TEXT,
          PRIMARY KEY(result_id, event_id)
        );
        "#,
    )
    .map_err(|e| e.to_string())?;
    // v2 列：来源片段身份与候选标记；用 ADD COLUMN 做可验证的状态迁移。
    let have_source_frag = table_has_column(conn, "events", "source_fragment_id")?;
    if !have_source_frag {
      conn.execute_batch(
        "ALTER TABLE events ADD COLUMN source_fragment_id TEXT;
         ALTER TABLE comp_evaluations ADD COLUMN undone INTEGER NOT NULL DEFAULT 0;",
      )
      .map_err(|e| e.to_string())?;
    }
    conn.execute(
      "CREATE TABLE IF NOT EXISTS idx_events_branch ON events(branch_id, seq)",
      [],
    )
    .ok();
    Ok(())
}

pub fn user_version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0)
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| e.to_string())?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| e.to_string())?;
    for name in names {
        if name.map_err(|e| e.to_string())? == column {
            return Ok(true);
        }
    }
    Ok(false)
}
