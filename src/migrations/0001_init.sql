CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE blobs (
  blob_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  bytes BLOB NOT NULL,
  sha256 TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE drafts (
  draft_id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  case_id TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE branches (
  branch_id TEXT PRIMARY KEY,
  draft_id TEXT NOT NULL REFERENCES drafts(draft_id),
  name TEXT NOT NULL,
  parent_branch_id TEXT REFERENCES branches(branch_id),
  fork_event_id TEXT,
  fork_branch_seq INTEGER,
  created_at TEXT NOT NULL
);

CREATE TABLE events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL,
  draft_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  branch_seq INTEGER NOT NULL,
  parent_event_id TEXT,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(branch_id, branch_seq),
  UNIQUE(branch_id, event_id)
);
CREATE INDEX idx_events_draft ON events(draft_id);
CREATE INDEX idx_events_kind ON events(kind);

CREATE TABLE versions (
  version_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  supersedes_version_id TEXT,
  payload TEXT NOT NULL,
  event_id TEXT NOT NULL,
  draft_id TEXT NOT NULL
);
CREATE INDEX idx_versions_entity ON versions(kind, entity_id);

CREATE TABLE pop_results (
  pop_result_id TEXT PRIMARY KEY,
  gate_id TEXT NOT NULL,
  gate_geom_version_id TEXT NOT NULL,
  parent_pop_result_id TEXT,
  denominator_kind TEXT NOT NULL,
  denominator_ref TEXT NOT NULL,
  denominator_count INTEGER NOT NULL,
  numerator_count INTEGER NOT NULL,
  excluded_unconvertible INTEGER NOT NULL,
  percent REAL,
  status TEXT NOT NULL,
  detail TEXT NOT NULL,
  deps TEXT NOT NULL,
  created_event_id TEXT NOT NULL
);
CREATE INDEX idx_pop_results_gate ON pop_results(gate_id);

CREATE TABLE pop_status_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  pop_result_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  branch_seq INTEGER NOT NULL,
  status TEXT NOT NULL,
  reason_event_id TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_pop_status_result ON pop_status_log(pop_result_id, branch_id);

CREATE TABLE candidates (
  candidate_id TEXT PRIMARY KEY,
  candidate_set_id TEXT NOT NULL,
  label TEXT NOT NULL,
  legal INTEGER NOT NULL,
  payload TEXT NOT NULL,
  created_event_id TEXT NOT NULL
);
CREATE INDEX idx_candidates_set ON candidates(candidate_set_id);

CREATE TABLE decisions (
  decision_id TEXT PRIMARY KEY,
  candidate_set_id TEXT NOT NULL,
  candidate_id TEXT,
  action TEXT NOT NULL,
  rationale TEXT,
  event_id TEXT NOT NULL
);
CREATE INDEX idx_decisions_set ON decisions(candidate_set_id);
CREATE INDEX idx_decisions_event ON decisions(event_id);
