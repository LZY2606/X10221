-- v2: 来源/诊断/批次与乐观并发基序号
ALTER TABLE materials ADD COLUMN unit_or_basis TEXT;
ALTER TABLE materials ADD COLUMN import_batch TEXT;
ALTER TABLE materials ADD COLUMN ordinal INTEGER NOT NULL DEFAULT 0;
ALTER TABLE materials ADD COLUMN supersedes TEXT;

CREATE TABLE material_diagnostics (
  material_id TEXT NOT NULL,
  code        TEXT NOT NULL,
  message     TEXT NOT NULL,
  location    TEXT NOT NULL DEFAULT '',
  severity    TEXT NOT NULL DEFAULT 'warning',
  PRIMARY KEY (material_id, code, location)
);

CREATE TABLE import_batches (
  batch_id   TEXT PRIMARY KEY,
  created_at TEXT NOT NULL,
  status     TEXT NOT NULL,
  detail     TEXT NOT NULL DEFAULT ''
);

ALTER TABLE events ADD COLUMN base_seq INTEGER;
