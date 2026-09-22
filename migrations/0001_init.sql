-- FlowLine schema v1: append-only event log + separately versioned domain resources.
CREATE TABLE IF NOT EXISTS schema_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS raw_blob (
  sha256 TEXT PRIMARY KEY,
  bytes BLOB NOT NULL,
  media_type TEXT NOT NULL,
  byte_count INTEGER NOT NULL,
  first_seen_seq INTEGER
);

CREATE TABLE IF NOT EXISTS event_log (
  case_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  branch_id TEXT NOT NULL,
  parent_seq INTEGER,
  kind TEXT NOT NULL,
  actor TEXT NOT NULL,
  created_at TEXT NOT NULL,
  payload TEXT NOT NULL,
  PRIMARY KEY (case_id, branch_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_event_case ON event_log(case_id, seq);

CREATE TABLE IF NOT EXISTS resource_version (
  case_id TEXT NOT NULL,
  resource_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  kind TEXT NOT NULL,
  seq INTEGER NOT NULL,
  branch_id TEXT NOT NULL,
  body TEXT NOT NULL,
  replaces_version INTEGER,
  PRIMARY KEY (case_id, resource_id, version)
);
CREATE INDEX IF NOT EXISTS idx_rv_kind ON resource_version(case_id, kind);

CREATE TABLE IF NOT EXISTS result_version (
  case_id TEXT NOT NULL,
  result_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  gate_id TEXT NOT NULL,
  gate_version INTEGER NOT NULL,
  parent_result_id TEXT,
  parent_result_version INTEGER,
  transform_id TEXT,
  transform_version INTEGER,
  comp_candidate_id TEXT,
  seq INTEGER NOT NULL,
  branch_id TEXT NOT NULL,
  body TEXT NOT NULL,
  PRIMARY KEY (case_id, result_id, version)
);

CREATE TABLE IF NOT EXISTS result_dependency (
  case_id TEXT NOT NULL,
  result_id TEXT NOT NULL,
  result_version INTEGER NOT NULL,
  dep_kind TEXT NOT NULL,
  dep_resource_id TEXT NOT NULL,
  dep_version INTEGER NOT NULL,
  PRIMARY KEY (case_id, result_id, version, dep_kind, dep_resource_id)
);

CREATE TABLE IF NOT EXISTS stale_marker (
  case_id TEXT NOT NULL,
  result_id TEXT NOT NULL,
  result_version INTEGER NOT NULL,
  seq INTEGER NOT NULL,
  branch_id TEXT NOT NULL,
  reason_kind TEXT NOT NULL,
  reason_resource_id TEXT NOT NULL,
  reason_version INTEGER NOT NULL,
  affected_scope TEXT NOT NULL,
  PRIMARY KEY (case_id, result_id, result_version, seq)
);

CREATE TABLE IF NOT EXISTS candidate (
  case_id TEXT NOT NULL,
  candidate_id TEXT NOT NULL,
  scope TEXT NOT NULL,
  created_seq INTEGER NOT NULL,
  decided_seq INTEGER,
  status TEXT NOT NULL,
  option_key TEXT NOT NULL,
  payload TEXT NOT NULL,
  rationale TEXT NOT NULL,
  PRIMARY KEY (case_id, candidate_id)
);

CREATE TABLE IF NOT EXISTS branch (
  case_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  parent_branch_id TEXT,
  fork_seq INTEGER,
  label TEXT NOT NULL,
  created_seq INTEGER NOT NULL,
  PRIMARY KEY (case_id, branch_id)
);

CREATE TABLE IF NOT EXISTS branch_event (
  case_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  visible INTEGER NOT NULL,
  PRIMARY KEY (case_id, branch_id, seq)
);

CREATE TABLE IF NOT EXISTS draft (
  case_id TEXT NOT NULL,
  draft_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  base_seq INTEGER NOT NULL,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  payload TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (case_id, draft_id)
);
