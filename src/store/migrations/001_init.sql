PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS branches (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    head_event_id TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS materials (
    id TEXT PRIMARY KEY,
    filename TEXT NOT NULL,
    kind TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL,
    content BLOB NOT NULL,
    imported_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS material_diagnostics (
    material_id TEXT NOT NULL REFERENCES materials(id),
    seq INTEGER NOT NULL,
    code TEXT NOT NULL,
    detail TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    seq INTEGER NOT NULL,
    branch_id TEXT NOT NULL REFERENCES branches(id),
    timestamp INTEGER NOT NULL,
    actor TEXT NOT NULL,
    payload TEXT NOT NULL,
    prev_event_id TEXT,
    UNIQUE(branch_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_events_branch ON events(branch_id, seq);

CREATE TABLE IF NOT EXISTS entity_versions (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    data TEXT NOT NULL,
    source_material_id TEXT REFERENCES materials(id),
    branch_id TEXT NOT NULL REFERENCES branches(id),
    parent_version_id TEXT REFERENCES entity_versions(id),
    created_at INTEGER NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    UNIQUE(branch_id, kind, entity_id, version)
);
CREATE INDEX IF NOT EXISTS idx_ev_entity ON entity_versions(branch_id, kind, entity_id, version);

CREATE TABLE IF NOT EXISTS compensation_verdicts (
    id TEXT PRIMARY KEY,
    spill_version_id TEXT NOT NULL,
    branch_id TEXT NOT NULL REFERENCES branches(id),
    status TEXT NOT NULL,
    det REAL NOT NULL,
    cond REAL,
    candidates TEXT NOT NULL,
    chosen_index INTEGER,
    event_id TEXT NOT NULL,
    UNIQUE(branch_id, spill_version_id)
);

-- 门结果不可变：更新只允许改写 stale/stale_reason；计数、分母、百分比冻结。
CREATE TABLE IF NOT EXISTS gate_results (
    id TEXT PRIMARY KEY,
    gate_version_id TEXT NOT NULL,
    gate_entity_id TEXT NOT NULL,
    branch_id TEXT NOT NULL REFERENCES branches(id),
    lineage_version_id TEXT NOT NULL,
    transform_version_id TEXT NOT NULL,
    spill_version_id TEXT NOT NULL,
    parent_gate_version_id TEXT,
    denominator_result_id TEXT,
    count INTEGER NOT NULL,
    denominator INTEGER NOT NULL,
    percent REAL NOT NULL,
    included_indices TEXT NOT NULL,
    boundary_included TEXT NOT NULL,
    boundary_excluded TEXT NOT NULL,
    stale INTEGER NOT NULL DEFAULT 0,
    stale_reason TEXT NOT NULL DEFAULT '',
    blocked INTEGER NOT NULL DEFAULT 0,
    blocked_reason TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_results_gate ON gate_results(branch_id, gate_entity_id, created_at);

CREATE TABLE IF NOT EXISTS decisions (
    id TEXT PRIMARY KEY,
    branch_id TEXT NOT NULL REFERENCES branches(id),
    target_version_id TEXT NOT NULL,
    decision TEXT NOT NULL,
    chosen_index INTEGER,
    event_id TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);

-- 分支级配置（如当前主管材料），与材料本身分开。
CREATE TABLE IF NOT EXISTS branch_config (
    branch_id TEXT NOT NULL REFERENCES branches(id),
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY(branch_id, key)
);

-- 冻结结果列：任何试图改写 count/denominator/percent 的 UPDATE 都被拒绝，
-- 对应“修订上游门后下游结果标记过期而不覆盖”。
CREATE TRIGGER IF NOT EXISTS trg_results_freeze
BEFORE UPDATE ON gate_results
WHEN NEW.count != OLD.count
   OR NEW.denominator != OLD.denominator
   OR NEW.percent != OLD.percent
   OR NEW.included_indices != OLD.included_indices
   OR NEW.blocked != OLD.blocked
BEGIN
    SELECT RAISE(ABORT, 'gate result columns are immutable; create a new versioned result instead');
END;

-- 事件为追加式：禁止 UPDATE/DELETE。
CREATE TRIGGER IF NOT EXISTS trg_events_no_update
BEFORE UPDATE ON events
BEGIN
    SELECT RAISE(ABORT, 'events are append-only');
END;
CREATE TRIGGER IF NOT EXISTS trg_events_no_delete
BEFORE DELETE ON events
BEGIN
    SELECT RAISE(ABORT, 'events are append-only');
END;
