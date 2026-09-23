-- v1: 初始谱系账本：原始材料、分支、事件（仅最小集，后续迁移扩展）
CREATE TABLE _meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE materials (
  material_id TEXT PRIMARY KEY,
  kind        TEXT NOT NULL,
  name        TEXT NOT NULL,
  media_type  TEXT,
  raw_bytes   BLOB NOT NULL,
  sha256      TEXT NOT NULL,
  imported_at TEXT NOT NULL
);

CREATE TABLE branches (
  branch_id     TEXT PRIMARY KEY,
  parent_branch TEXT,
  fork_seq      INTEGER,
  name          TEXT NOT NULL,
  created_at    TEXT NOT NULL
);

CREATE TABLE events (
  event_id   TEXT PRIMARY KEY,
  seq        INTEGER NOT NULL,
  branch_id  TEXT NOT NULL,
  event_type TEXT NOT NULL,
  payload    TEXT NOT NULL,
  created_at TEXT NOT NULL,
  actor      TEXT NOT NULL,
  UNIQUE (branch_id, seq)
);
