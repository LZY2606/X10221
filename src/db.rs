use rusqlite::Connection;

/// 迁移脚本按顺序应用；PRAGMA user_version 记录已应用的迁移数。
/// 状态迁移测试会逐版本推进并校验数据保留。
pub const MIGRATIONS: [&str; 3] = [M1, M2, M3];

const M1: &str = "
CREATE TABLE materials(
  id TEXT NOT NULL,
  kind TEXT NOT NULL,
  version INTEGER NOT NULL,
  content_json TEXT NOT NULL,
  raw_text TEXT NOT NULL,
  supersedes TEXT,
  introduced_seq INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  PRIMARY KEY(id, version)
);
CREATE TABLE events(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,
  branch TEXT NOT NULL DEFAULT 'main',
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
";

const M2: &str = "
CREATE TABLE populations(
  id TEXT PRIMARY KEY,
  gate_id TEXT NOT NULL,
  gate_version INTEGER NOT NULL,
  parent_pop_id TEXT,
  parent_gate_version INTEGER,
  comp_version INTEGER NOT NULL,
  transform_version INTEGER NOT NULL,
  n_in INTEGER NOT NULL,
  n_total_parent INTEGER NOT NULL,
  percent REAL NOT NULL,
  status TEXT NOT NULL DEFAULT 'current',
  branch TEXT NOT NULL DEFAULT 'main',
  introduced_seq INTEGER NOT NULL DEFAULT 0,
  stale_since_seq INTEGER
);
CREATE TABLE candidates(
  id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  options_json TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open',
  accepted_option TEXT,
  branch TEXT NOT NULL DEFAULT 'main',
  created_at TEXT NOT NULL
);
";

const M3: &str = "
ALTER TABLE materials ADD COLUMN unit_json TEXT;
ALTER TABLE populations ADD COLUMN diagnostics_json TEXT;
CREATE TABLE drafts(
  id TEXT NOT NULL,
  version INTEGER NOT NULL,
  content_json TEXT NOT NULL,
  updated_by TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(id, version)
);
CREATE TABLE conflicts(
  id TEXT PRIMARY KEY,
  draft_id TEXT NOT NULL,
  base_version INTEGER NOT NULL,
  current_version INTEGER NOT NULL,
  ours_json TEXT NOT NULL,
  theirs_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
";

pub fn migrate(conn: &Connection) -> anyhow::Result<()> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = i as u32 + 1;
        if current < v {
            conn.execute_batch(sql)?;
            conn.pragma_update(None, "user_version", v)?;
        }
    }
    Ok(())
}

pub fn now() -> String {
    // 单调递增的逻辑时间戳，避免引入额外依赖；真实墙钟由调用方不关心。
    use std::sync::atomic::{AtomicU64, Ordering};
    static TICK: AtomicU64 = AtomicU64::new(1_700_000_000_000);
    format!("{}", TICK.fetch_add(1, Ordering::SeqCst))
}
