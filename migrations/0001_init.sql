-- v1: core provenance model for the flow cytometry boundary chamber.
-- Sources and interpretations are stored separately and versioned independently.
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE materials (
  id         TEXT NOT NULL,
  version    INTEGER NOT NULL,
  kind       TEXT NOT NULL CHECK (kind IN ('fcs','spillover','compensation','transform','gate','hierarchy')),
  name       TEXT NOT NULL,
  rel_path   TEXT NOT NULL,
  media_type TEXT NOT NULL,
  raw_bytes  BLOB NOT NULL,
  sha256     TEXT NOT NULL,
  created_event_id INTEGER NOT NULL,
  PRIMARY KEY (id, version)
);

CREATE TABLE branches (
  id                     TEXT PRIMARY KEY,
  name                   TEXT NOT NULL,
  parent_branch_id       TEXT,
  forked_from_event_id   INTEGER,
  head_event_id          INTEGER NOT NULL DEFAULT 0,
  created_at             TEXT NOT NULL
);

CREATE TABLE events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  event_uid   TEXT NOT NULL UNIQUE,
  branch_id   TEXT NOT NULL REFERENCES branches(id),
  seq         INTEGER NOT NULL,
  kind        TEXT NOT NULL,
  payload     TEXT NOT NULL,
  parent_uid  TEXT,
  snapshot    TEXT NOT NULL DEFAULT '{}',
  created_at  TEXT NOT NULL,
  UNIQUE (branch_id, seq)
);

CREATE TABLE conflict_log (
  id                  INTEGER PRIMARY KEY AUTOINCREMENT,
  branch_id           TEXT NOT NULL,
  attempted_by        TEXT NOT NULL,
  expected_parent_uid TEXT NOT NULL,
  actual_parent_uid   TEXT NOT NULL,
  payload             TEXT NOT NULL,
  created_at          TEXT NOT NULL
);
