CREATE TABLE schema_migrations (
  version TEXT PRIMARY KEY,
  applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE sources (
  stable_id TEXT NOT NULL,
  version TEXT NOT NULL,
  kind TEXT NOT NULL,
  path TEXT NOT NULL,
  content_json TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  created_event_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  PRIMARY KEY (stable_id, version)
);

CREATE TABLE branches (
  branch_id TEXT PRIMARY KEY,
  parent_branch_id TEXT,
  parent_event_id TEXT,
  base_refs_json TEXT NOT NULL,
  head_event_id TEXT,
  head_seq INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE events (
  event_id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  parent_event_id TEXT,
  kind TEXT NOT NULL,
  code TEXT NOT NULL,
  refs_json TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  snapshot_json TEXT NOT NULL,
  changed_head INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  UNIQUE (branch_id, seq)
);

CREATE TABLE candidates (
  candidate_id TEXT PRIMARY KEY,
  candidate_kind TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  object_stable_id TEXT NOT NULL,
  refs_json TEXT NOT NULL,
  value_json TEXT NOT NULL,
  ordinal INTEGER NOT NULL
);

CREATE TABLE candidate_statuses (
  candidate_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  status TEXT NOT NULL,
  reason_code TEXT NOT NULL,
  affected_scope_json TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  PRIMARY KEY (candidate_id, event_id)
);

CREATE TABLE decisions (
  decision_id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  candidate_id TEXT NOT NULL,
  action TEXT NOT NULL,
  rationale TEXT NOT NULL,
  expected_head_event_id TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE archive_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE INDEX idx_sources_kind ON sources(kind, version);
CREATE INDEX idx_events_branch ON events(branch_id, seq);
CREATE INDEX idx_candidates_object ON candidates(object_stable_id);
CREATE INDEX idx_candidates_event ON candidates(event_id);
CREATE INDEX idx_statuses_event ON candidate_statuses(event_id);
CREATE INDEX idx_decisions_event ON decisions(event_id);
