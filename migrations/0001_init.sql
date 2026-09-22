PRAGMA user_version = 1;

CREATE TABLE IF NOT EXISTS schema_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- 六类来源材料，类型/id/版本分开，payload 保留导入时的原始 JSON 表示
CREATE TABLE IF NOT EXISTS materials (
    material_type TEXT NOT NULL CHECK (
        material_type IN ('raw_events','spillover_meta','compensation','transform','gates','hierarchy')
    ),
    material_id   TEXT NOT NULL,
    version       INTEGER NOT NULL,
    payload_json  TEXT NOT NULL,
    sha256        TEXT NOT NULL,
    ingest_seq    INTEGER NOT NULL,
    PRIMARY KEY (material_type, material_id, version)
);

CREATE TABLE IF NOT EXISTS branches (
    branch_id       TEXT PRIMARY KEY,
    parent_branch   TEXT REFERENCES branches(branch_id),
    base_event_id   TEXT,
    tip_event_id    TEXT,
    label           TEXT NOT NULL,
    created_seq     INTEGER NOT NULL
);

-- append-only 事件流：每个事件带完整 refs 快照，回到旧事件即可恢复当时结论
CREATE TABLE IF NOT EXISTS events (
    event_id        TEXT PRIMARY KEY,
    seq             INTEGER NOT NULL,
    branch_id       TEXT NOT NULL REFERENCES branches(branch_id),
    parent_event_id TEXT REFERENCES events(event_id),
    kind            TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    refs_json       TEXT NOT NULL DEFAULT '{}',
    result_json     TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_events_branch ON events(branch_id, seq);

-- 多候选（近奇异补偿、过期重估等）与裁定分支显式保存
CREATE TABLE IF NOT EXISTS candidates (
    candidate_uid TEXT PRIMARY KEY,
    branch_id     TEXT NOT NULL REFERENCES branches(branch_id),
    group_id      TEXT NOT NULL,
    candidate_id  TEXT NOT NULL,
    kind          TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('proposed','adopted','rejected','stale')),
    detail_json   TEXT NOT NULL,
    event_id      TEXT NOT NULL REFERENCES events(event_id)
);

-- 物化的群体结论行：百分比分母钉住父群版本（denominator_ref）
CREATE TABLE IF NOT EXISTS appraisal_rows (
    row_uid          TEXT PRIMARY KEY,
    branch_id        TEXT NOT NULL REFERENCES branches(branch_id),
    event_id         TEXT NOT NULL REFERENCES events(event_id),
    gate_id          TEXT NOT NULL,
    gate_ref         TEXT NOT NULL,
    parent_ref       TEXT,
    denominator_ref  TEXT,
    numerator        INTEGER NOT NULL,
    denominator      INTEGER NOT NULL,
    fraction         REAL NOT NULL,
    status           TEXT NOT NULL,
    detail_json      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS seq_counters (
    scope TEXT PRIMARY KEY,
    next_seq INTEGER NOT NULL
);
