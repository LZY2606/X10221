PRAGMA user_version = 2;

-- v2：记录同版本并发修改冲突；旧库平滑升级，v1 数据不丢
CREATE TABLE IF NOT EXISTS conflict_log (
    conflict_uid   TEXT PRIMARY KEY,
    branch_id      TEXT NOT NULL,
    attempted_kind TEXT NOT NULL,
    expected_parent TEXT,
    actual_parent   TEXT,
    diff_json      TEXT NOT NULL,
    created_seq     INTEGER NOT NULL
);

ALTER TABLE appraisal_rows ADD COLUMN stale_reason TEXT;
