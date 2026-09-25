-- FlowLineage schema v1
-- 设计要点：
--  * 原始字节(blobs) 与解释层(materials) 分开；
--  * 六类源材料各自追加版本（append-only）；
--  * 派生结果不可变；上游修订新建结果，旧结果保留由视图判过期；
--  * 事件内容寻址、分支内单调序号；乐观并发用 base_event_id。

CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS blobs (
    id         TEXT PRIMARY KEY,   -- blob_<sha256:16>
    name       TEXT NOT NULL,
    bytes      BLOB NOT NULL,
    sha256     TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS materials (
    id             TEXT PRIMARY KEY,           -- mat_<kind>_v<seq>_<hash12>
    kind           TEXT NOT NULL,
    seq            INTEGER NOT NULL,
    body_json      TEXT NOT NULL,
    source_blob_id TEXT REFERENCES blobs(id),
    source_name    TEXT,
    note           TEXT NOT NULL DEFAULT '',
    created_at     TEXT NOT NULL,
    UNIQUE(kind, seq)
);

CREATE TABLE IF NOT EXISTS derived (
    id          TEXT PRIMARY KEY,               -- der_<kind>_<hash12>
    kind        TEXT NOT NULL,                  -- comp_analysis/compensated/pop
    node_id     TEXT,
    branch_id   TEXT NOT NULL,
    body_json   TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS branches (
    id                  TEXT PRIMARY KEY,
    label               TEXT NOT NULL,
    forked_from_event   TEXT,
    adopted             INTEGER NOT NULL DEFAULT 1,
    created_at          TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    id               TEXT PRIMARY KEY,
    branch_id        TEXT NOT NULL REFERENCES branches(id),
    seq              INTEGER NOT NULL,
    kind             TEXT NOT NULL,
    summary          TEXT NOT NULL,
    basis_json       TEXT NOT NULL,
    snapshot_json    TEXT NOT NULL,
    parent_event_id  TEXT,
    base_event_id    TEXT,
    created_at       TEXT NOT NULL,
    UNIQUE(branch_id, seq)
);

CREATE TABLE IF NOT EXISTS conflicts (
    id              TEXT PRIMARY KEY,
    branch_id       TEXT NOT NULL,
    base_event_id   TEXT NOT NULL,
    attempted_by    TEXT NOT NULL,
    attempted_json  TEXT NOT NULL,
    existing_json   TEXT NOT NULL,
    diff_json       TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_materials_kind ON materials(kind, seq);
CREATE INDEX IF NOT EXISTS idx_events_branch  ON events(branch_id, seq);
CREATE INDEX IF NOT EXISTS idx_derived_branch ON derived(branch_id);
