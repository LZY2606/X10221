-- v3: 分支头指针缓存，支撑同版本并发修改检测
CREATE TABLE branch_heads (
  branch_id  TEXT PRIMARY KEY,
  head_seq   INTEGER NOT NULL,
  updated_at TEXT NOT NULL
);

INSERT INTO branch_heads (branch_id, head_seq, updated_at)
SELECT branch_id, MAX(seq), MAX(created_at) FROM events GROUP BY branch_id;

-- 同版本并发修改审计（不改变目标分支状态）
CREATE TABLE conflict_audits (
  conflict_id          TEXT PRIMARY KEY,
  branch_id            TEXT NOT NULL,
  attempted_by         TEXT NOT NULL,
  base_seq             INTEGER NOT NULL,
  server_head          INTEGER NOT NULL,
  attempted_event_type TEXT NOT NULL,
  attempted_payload    TEXT NOT NULL,
  created_at           TEXT NOT NULL
);
