//! SQLite schema with explicit, testable migrations.
//! Sources (raw bytes, parsed views) and interpretations (versions,
//! adjudications) are stored separately; nothing is overwritten in place.

use anyhow::Result;
use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 3;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v < 1 {
        conn.execute_batch(MIGRATION_1)?;
    }
    if v < 2 {
        conn.execute_batch(MIGRATION_2)?;
    }
    if v < 3 {
        conn.execute_batch(MIGRATION_3)?;
    }
    conn.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION};"))?;
    Ok(())
}

const MIGRATION_1: &str = r#"
-- v1: meta clock, raw blobs, separately-versioned artifacts, event log
CREATE TABLE meta(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
INSERT INTO meta(key,value) VALUES('id_seq','0'),('event_seq','0');

CREATE TABLE raw_blobs(
  id TEXT PRIMARY KEY,          -- blob-<n>
  filename TEXT NOT NULL,
  sha TEXT NOT NULL,
  bytes BLOB NOT NULL,          -- original representation, never normalized
  created_seq INTEGER NOT NULL
);

CREATE TABLE artifacts(
  id TEXT PRIMARY KEY,          -- <kind>-<n>
  logical_id TEXT NOT NULL,     -- groups versions of one logical object
  kind TEXT NOT NULL,           -- raw_events|spillover|comp_matrix|transform|gate|hierarchy
  version INTEGER NOT NULL,
  name TEXT NOT NULL,
  status TEXT NOT NULL,         -- active|superseded|rejected
  parent_id TEXT,               -- previous version artifact id
  branch_id TEXT NOT NULL,
  payload TEXT NOT NULL,        -- JSON domain payload (parsed view / params)
  source_ref TEXT,              -- blob id or casepack file
  created_seq INTEGER NOT NULL,
  UNIQUE(logical_id, version, branch_id)
);
CREATE INDEX idx_art_kind ON artifacts(kind, status);
"#;

const MIGRATION_2: &str = r#"
-- v2: derived results with pinned dependencies, machine-readable events
CREATE TABLE results(
  id TEXT PRIMARY KEY,          -- res-<n>
  kind TEXT NOT NULL,           -- population|comp_preview
  logical_id TEXT NOT NULL,     -- e.g. population of gate logical id
  version INTEGER NOT NULL,
  status TEXT NOT NULL,         -- current|stale|rejected
  deps TEXT NOT NULL,           -- JSON {gate,comp,transform,hierarchy,parent_result}
  branch_id TEXT NOT NULL,
  payload TEXT NOT NULL,        -- counts, percent, denominator pin, diagnostics
  created_seq INTEGER NOT NULL
);
CREATE INDEX idx_res_deps ON results(status);

CREATE TABLE events(
  seq INTEGER PRIMARY KEY,      -- logical clock, defines replay order
  id TEXT NOT NULL,             -- evt-<seq>
  branch_id TEXT NOT NULL,
  kind TEXT NOT NULL,           -- import|comp_near_singular|extreme_signal|boundary_point|
                                -- gate_renamed|upstream_stale|conflict|adjudicate|undo|export|import
  subject TEXT,                 -- artifact/result/candidate id
  payload TEXT NOT NULL,        -- machine-readable judgment basis
  reverted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_events_branch ON events(branch_id, seq);
"#;

const MIGRATION_3: &str = r#"
-- v3: adjudication branches, multi-candidates, optimistic-concurrency drafts
CREATE TABLE branches(
  id TEXT PRIMARY KEY,          -- br-<n> ; 'main' always exists
  name TEXT NOT NULL,
  parent_id TEXT,
  head_seq INTEGER NOT NULL DEFAULT 0,
  created_seq INTEGER NOT NULL
);
INSERT INTO branches(id,name,parent_id,head_seq,created_seq) VALUES('main','main',NULL,0,0);

CREATE TABLE candidates(
  id TEXT PRIMARY KEY,          -- cand-<n>
  subject TEXT NOT NULL,        -- what is undecidable (e.g. comp matrix logical id)
  branch_id TEXT NOT NULL,
  payload TEXT NOT NULL,        -- JSON: the legal option
  status TEXT NOT NULL,         -- pending|accepted|rejected
  created_seq INTEGER NOT NULL
);

CREATE TABLE drafts(
  resource TEXT NOT NULL,       -- logical id being edited
  branch_id TEXT NOT NULL,
  version INTEGER NOT NULL,     -- optimistic lock counter
  payload TEXT NOT NULL,
  PRIMARY KEY(resource, branch_id)
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_from_empty_and_is_idempotent() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        migrate(&c).unwrap(); // idempotent
        for t in ["meta","raw_blobs","artifacts","results","events","branches","candidates","drafts"] {
            let n: i64 = c.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [t], |r| r.get(0)).unwrap();
            assert_eq!(n, 1, "missing table {t}");
        }
        let main: String = c.query_row("SELECT id FROM branches WHERE id='main'", [], |r| r.get(0)).unwrap();
        assert_eq!(main, "main");
    }

    #[test]
    fn migrates_stepwise_from_v1() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(MIGRATION_1).unwrap();
        c.execute_batch("PRAGMA user_version=1").unwrap();
        migrate(&c).unwrap();
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        // v1 data survives
        c.execute("INSERT INTO meta(key,value) VALUES('probe','ok')", []).unwrap();
        let s: String = c.query_row("SELECT value FROM meta WHERE key='probe'", [], |r| r.get(0)).unwrap();
        assert_eq!(s, "ok");
    }
}
