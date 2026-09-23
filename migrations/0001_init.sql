-- 流式边界室：初始 schema。
-- 来源与解释分开：原始字节(raw_blobs)、版本化产物(artifacts)、
-- 只追加的事件日志(events)、裁定(decisions)、草案(drafts)、分支(branches)。

CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE schema_migrations (
  version    INTEGER PRIMARY KEY,
  name       TEXT NOT NULL,
  applied_ms INTEGER NOT NULL
);

CREATE TABLE raw_blobs (
  id            TEXT PRIMARY KEY,
  filename      TEXT NOT NULL,
  media_type    TEXT NOT NULL,
  bytes_b64     TEXT NOT NULL,
  sha256        TEXT NOT NULL,
  length_bytes  INTEGER NOT NULL,
  imported_seq  INTEGER
);

CREATE TABLE branches (
  name          TEXT PRIMARY KEY,
  label         TEXT NOT NULL,
  parent_branch TEXT,
  fork_seq      INTEGER,
  created_seq   INTEGER NOT NULL
);

CREATE TABLE artifacts (
  kind         TEXT NOT NULL,           -- events|spillover|comp_matrix|transform|gate|lineage
                                        -- |comp_events|xform_events|population
  id           TEXT NOT NULL,
  version      INTEGER NOT NULL,
  branch       TEXT NOT NULL DEFAULT 'main',
  name         TEXT NOT NULL DEFAULT '',
  payload_json TEXT NOT NULL,
  origin       TEXT NOT NULL,           -- imported|derived
  source_blob  TEXT,
  content_hash TEXT NOT NULL,
  created_seq  INTEGER NOT NULL,
  PRIMARY KEY (branch, kind, id, version)
);

CREATE INDEX idx_artifacts_created ON artifacts (created_seq);
CREATE INDEX idx_artifacts_kind ON artifacts (branch, kind, id);

CREATE TABLE events (
  seq              INTEGER PRIMARY KEY AUTOINCREMENT,
  at_ms            INTEGER NOT NULL,
  branch           TEXT NOT NULL,
  type             TEXT NOT NULL,       -- import|revise|rename|derive|branch|adjudicate
                                        -- |reject|undo|conflict|import_failed
  target_kind      TEXT,
  target_id        TEXT,
  target_version   INTEGER,
  actor            TEXT NOT NULL DEFAULT 'reviewer',
  payload_json     TEXT NOT NULL,
  invalidates_json TEXT NOT NULL DEFAULT '[]',
  prev_seq         INTEGER
);

CREATE INDEX idx_events_branch ON events (branch, seq);

CREATE TABLE decisions (
  id              TEXT PRIMARY KEY,
  branch          TEXT NOT NULL,
  ref_kind        TEXT NOT NULL,
  ref_id          TEXT NOT NULL,
  ref_version     INTEGER NOT NULL,
  candidate_id    TEXT,
  action          TEXT NOT NULL,        -- accept|reject
  note            TEXT NOT NULL DEFAULT '',
  created_seq     INTEGER NOT NULL,
  retracted_seq   INTEGER
);

CREATE INDEX idx_decisions_ref ON decisions (branch, ref_kind, ref_id, ref_version);

CREATE TABLE drafts (
  id          TEXT PRIMARY KEY,
  label       TEXT NOT NULL,
  branch      TEXT NOT NULL,
  base_seq    INTEGER NOT NULL,
  created_seq INTEGER NOT NULL
);
