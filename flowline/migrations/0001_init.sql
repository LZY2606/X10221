-- v1：事件溯源主干。所有状态变更只追加 events，视图由重放得到。
CREATE TABLE events (
    event_id     TEXT PRIMARY KEY,
    seq          INTEGER NOT NULL UNIQUE,
    branch_id    TEXT NOT NULL,
    parent_event TEXT,
    kind         TEXT NOT NULL,
    payload      TEXT NOT NULL,
    rule         TEXT,
    created_at   TEXT NOT NULL
);

CREATE TABLE branches (
    branch_id   TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    parent_branch TEXT,
    forked_from_event TEXT,
    fork_cutoff_seq INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
);

-- 原始字节 blob（来源材料）。events 通过 blob_id 引用，重解释不覆盖旧字节。
CREATE TABLE blobs (
    blob_id     TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    bytes       TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

-- 乐观并发控制：分支头版本（当前 head event seq）。
CREATE TABLE branch_heads (
    branch_id   TEXT PRIMARY KEY,
    head_seq    INTEGER NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE INDEX idx_events_branch ON events(branch_id, seq);
