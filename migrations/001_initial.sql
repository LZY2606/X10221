CREATE TABLE IF NOT EXISTS branches (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL,
  root_event_id TEXT NOT NULL,
  latest_event_id TEXT NOT NULL,
  base_case_version TEXT NOT NULL,
  created_from_branch TEXT,
  created_at TEXT NOT NULL DEFAULT '2026-09-23T09:00:00+09:00'
);

CREATE TABLE IF NOT EXISTS records (
  id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  kind TEXT NOT NULL,
  version TEXT NOT NULL,
  status TEXT NOT NULL,
  payload TEXT NOT NULL,
  PRIMARY KEY (id, branch_id),
  FOREIGN KEY (branch_id) REFERENCES branches(id)
);

CREATE TABLE IF NOT EXISTS audit_events (
  id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  parent_event_id TEXT,
  event_type TEXT NOT NULL,
  status TEXT NOT NULL,
  actor TEXT NOT NULL,
  occurred_at TEXT NOT NULL,
  payload TEXT NOT NULL,
  PRIMARY KEY (id, branch_id),
  FOREIGN KEY (branch_id) REFERENCES branches(id)
);

CREATE INDEX IF NOT EXISTS idx_records_branch ON records(branch_id, seq);
CREATE INDEX IF NOT EXISTS idx_events_branch ON audit_events(branch_id, seq);
PRAGMA user_version = 1;
