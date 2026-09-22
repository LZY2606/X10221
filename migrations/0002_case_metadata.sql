-- v2：案例元数据 + 标签（迁移测试验证旧数据存活）
ALTER TABLE projects ADD COLUMN title TEXT NOT NULL DEFAULT '';
ALTER TABLE projects ADD COLUMN case_id TEXT;

CREATE TABLE IF NOT EXISTS case_runs (
    project_id TEXT PRIMARY KEY,
    case_id    TEXT NOT NULL,
    loaded_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 回填：从首个 project_created 事件恢复标题
UPDATE projects
SET title = COALESCE(
    (SELECT json_extract(payload_json, '$.title')
     FROM events e
     WHERE e.project_id = projects.project_id AND e.kind = 'project_created'
     LIMIT 1),
    title);
