CREATE TABLE IF NOT EXISTS file_evidence (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  branch_id TEXT NOT NULL,
  path TEXT NOT NULL,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  imported_at TEXT NOT NULL DEFAULT '2026-09-23T09:00:00+09:00',
  FOREIGN KEY (branch_id) REFERENCES branches(id)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_file_evidence_branch_path
  ON file_evidence(branch_id, path);

ALTER TABLE records ADD COLUMN stable_identity TEXT NOT NULL DEFAULT '';
ALTER TABLE records ADD COLUMN dependency_key TEXT NOT NULL DEFAULT '';
ALTER TABLE records ADD COLUMN parent_record_id TEXT;
ALTER TABLE records ADD COLUMN created_event_id TEXT NOT NULL DEFAULT '';
ALTER TABLE records ADD COLUMN file_path TEXT;
ALTER TABLE records ADD COLUMN source_sha256 TEXT;

UPDATE records
SET stable_identity = id,
    dependency_key = kind || ':' || id,
    created_event_id = (
      SELECT id FROM audit_events
      WHERE audit_events.branch_id = records.branch_id
      ORDER BY seq LIMIT 1
    )
WHERE stable_identity = '';

CREATE UNIQUE INDEX IF NOT EXISTS idx_records_branch_seq ON records(branch_id, seq);
CREATE INDEX IF NOT EXISTS idx_records_identity ON records(branch_id, stable_identity);
CREATE INDEX IF NOT EXISTS idx_records_dependency ON records(branch_id, dependency_key);

ALTER TABLE branches ADD COLUMN conflict_expected_version TEXT;
ALTER TABLE branches ADD COLUMN conflict_seen_at TEXT;

PRAGMA user_version = 2;
