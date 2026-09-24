PRAGMA user_version = 2;

CREATE TABLE IF NOT EXISTS archive_labels (
  archive_id TEXT PRIMARY KEY,
  label      TEXT NOT NULL,
  imported_at TEXT NOT NULL
);
