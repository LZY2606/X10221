-- v3：分支标签，用于审阅页并排展示稳定身份。
ALTER TABLE branches ADD COLUMN color TEXT NOT NULL DEFAULT '#7aa2f7';
