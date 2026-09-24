PRAGMA user_version = 1;

CREATE TABLE IF NOT EXISTS entity_versions (
  id              TEXT PRIMARY KEY,
  entity_id       TEXT NOT NULL,
  kind            TEXT NOT NULL,
  version_seq     INTEGER NOT NULL,
  payload         TEXT NOT NULL,
  base_version_id TEXT,
  created_seq     INTEGER NOT NULL,
  UNIQUE(entity_id, kind, version_seq)
);

CREATE TABLE IF NOT EXISTS flow_events (
  dataset_version_id TEXT NOT NULL,
  event_index        INTEGER NOT NULL,
  channel_name       TEXT NOT NULL,
  raw_json           TEXT NOT NULL,
  numeric_value      REAL,
  diagnostic         TEXT,
  PRIMARY KEY (dataset_version_id, event_index, channel_name)
);

CREATE TABLE IF NOT EXISTS branches (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  parent_id   TEXT,
  fork_seq    INTEGER,
  created_seq INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS branch_head_events (
  seq          INTEGER PRIMARY KEY AUTOINCREMENT,
  branch_id    TEXT NOT NULL,
  entity_kind  TEXT NOT NULL,
  entity_id    TEXT NOT NULL,
  version_id   TEXT NOT NULL,
  reason       TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS results (
  id                   TEXT PRIMARY KEY,
  branch_id            TEXT NOT NULL,
  gate_id              TEXT,
  gate_version_id      TEXT,
  option_key           TEXT NOT NULL,
  parent_result_id     TEXT,
  numerator            INTEGER,
  denominator          INTEGER,
  percent              REAL,
  members              TEXT NOT NULL,
  boundary_count       INTEGER NOT NULL DEFAULT 0,
  blocked_reason       TEXT,
  created_seq          INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS result_deps (
  result_id      TEXT NOT NULL,
  dep_kind       TEXT NOT NULL,
  dep_version_id TEXT NOT NULL,
  PRIMARY KEY (result_id, dep_kind, dep_version_id)
);

CREATE TABLE IF NOT EXISTS invalidations (
  seq              INTEGER PRIMARY KEY AUTOINCREMENT,
  ledger_seq       INTEGER NOT NULL,
  branch_id        TEXT NOT NULL,
  cause_kind       TEXT NOT NULL,
  cause_version_id TEXT NOT NULL,
  scope            TEXT NOT NULL,
  reason           TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS decisions (
  id              TEXT PRIMARY KEY,
  branch_id       TEXT NOT NULL,
  gate_id         TEXT NOT NULL,
  gate_version_id TEXT NOT NULL,
  option_key      TEXT NOT NULL,
  verdict         TEXT NOT NULL,
  result_id       TEXT NOT NULL,
  reversed        INTEGER NOT NULL DEFAULT 0,
  note            TEXT,
  created_seq     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ledger (
  seq       INTEGER PRIMARY KEY AUTOINCREMENT,
  branch_id TEXT NOT NULL,
  at        TEXT NOT NULL,
  type      TEXT NOT NULL,
  payload   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS conflicts (
  id                TEXT PRIMARY KEY,
  ledger_seq        INTEGER NOT NULL,
  branch_id         TEXT NOT NULL,
  at                TEXT NOT NULL,
  entity_id         TEXT NOT NULL,
  expected_base_seq INTEGER NOT NULL,
  actual_seq        INTEGER NOT NULL,
  detail            TEXT NOT NULL,
  payload           TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ev_kind ON entity_versions(kind);
CREATE INDEX IF NOT EXISTS idx_results_branch ON results(branch_id);
CREATE INDEX IF NOT EXISTS idx_ledger_branch ON ledger(branch_id, seq);
CREATE INDEX IF NOT EXISTS idx_head_branch ON branch_head_events(branch_id, seq);
