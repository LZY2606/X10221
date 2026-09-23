-- v2：来源链强化字段。
ALTER TABLE object_versions ADD COLUMN input_fragment TEXT NOT NULL DEFAULT '';
ALTER TABLE object_versions ADD COLUMN raw_sha256 TEXT NOT NULL DEFAULT '';

CREATE TABLE IF NOT EXISTS cases_recorded (
    case_id TEXT PRIMARY KEY,
    manifest_json TEXT NOT NULL,
    recorded_at INTEGER NOT NULL
);

-- 回填：v1 事件即事件日志的来源片段；sha256 无法在纯 SQL 中计算，留空
-- （迁移代码会对可解析的事件 payload 补算 sha256）。
UPDATE object_versions
SET input_fragment = COALESCE(event_id, '')
WHERE input_fragment = '';
