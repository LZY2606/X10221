-- 第二次迁移：事件需要自带机器可读诊断；产物需要来源细节。
-- 对旧库做可验证的回填，而不是要求重建。

ALTER TABLE events ADD COLUMN diagnostics_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE artifacts ADD COLUMN origin_detail TEXT NOT NULL DEFAULT '';

UPDATE events SET diagnostics_json = '[]' WHERE diagnostics_json IS NULL;
UPDATE artifacts SET origin_detail = '' WHERE origin_detail IS NULL;
