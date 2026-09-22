-- v2：同版本并发修改支持（草稿 + 合并状态）。
CREATE TABLE drafts (
    draft_id    TEXT PRIMARY KEY,
    branch_id   TEXT NOT NULL,
    base_seq    INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    payload     TEXT NOT NULL,
    merged_into_event TEXT,
    created_at  TEXT NOT NULL
);

ALTER TABLE branch_heads ADD COLUMN conflict_count INTEGER NOT NULL DEFAULT 0;
