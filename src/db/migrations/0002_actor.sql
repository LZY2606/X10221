-- 0002：新事件默认操作者 reviewer；显式登记并发版本冲突。
-- 旧事件的 actor 值原样保留（历史不被覆盖）。
CREATE TABLE IF NOT EXISTS conflicts (
    conflict_id  TEXT PRIMARY KEY,
    branch_id    TEXT NOT NULL,
    expected_seq INTEGER NOT NULL,
    incoming_json TEXT NOT NULL,
    current_json  TEXT NOT NULL,
    ts_ms        INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_conflicts_branch ON conflicts(branch_id, ts_ms);
