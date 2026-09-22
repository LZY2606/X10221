-- 迁移 2：材料增加人工可读标签（验收包含状态迁移测试）。
ALTER TABLE materials ADD COLUMN label TEXT NOT NULL DEFAULT '';
UPDATE materials SET label = filename WHERE label = '';
