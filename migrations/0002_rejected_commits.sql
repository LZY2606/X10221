-- v2: 同版本并发修改留下冲突档案（被拒绝的提交），不进入分支事件链。
CREATE TABLE IF NOT EXISTS rejected_commits (
    rejected_id    TEXT PRIMARY KEY,
    branch_id      TEXT NOT NULL,
    base_event_id  TEXT,
    payload        TEXT NOT NULL,
    actor          TEXT NOT NULL,
    reason         TEXT NOT NULL,
    diff_summary   TEXT NOT NULL,
    created_at_unix INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_rejected_branch ON rejected_commits (branch_id, created_at_unix);
