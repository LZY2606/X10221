-- v1：来源（原始材料）、解释（门谱系/事件）、派生结果分开存放。
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS branches (
    name               TEXT PRIMARY KEY,
    label              TEXT NOT NULL,
    parent_branch      TEXT REFERENCES branches(name),
    forked_from_event  TEXT,
    head_event         TEXT,
    base_version       INTEGER NOT NULL DEFAULT 1,
    created_at         INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    id          TEXT PRIMARY KEY,
    seq         INTEGER NOT NULL,
    branch      TEXT NOT NULL REFERENCES branches(name),
    type        TEXT NOT NULL,
    title       TEXT NOT NULL,
    detail      TEXT NOT NULL,           -- 机器可读 JSON（判定依据/影响范围/候选）
    created_at  INTEGER NOT NULL,
    UNIQUE(branch, seq)
);
CREATE INDEX IF NOT EXISTS idx_events_branch ON events(branch, seq);

-- 同一逻辑材料 art_id 的多个版本：原始通道值/spillover/补偿矩阵/变换/门 各自独立版本化。
CREATE TABLE IF NOT EXISTS artifact_versions (
    art_id        TEXT NOT NULL,
    kind          TEXT NOT NULL,         -- fcs | spillover | compensation | transform | gates
    version       INTEGER NOT NULL,
    label         TEXT NOT NULL,
    payload       TEXT NOT NULL,         -- 保留原始表示
    status        TEXT NOT NULL DEFAULT 'accepted', -- accepted | rejected
    diagnostic    TEXT,
    created_event TEXT NOT NULL REFERENCES events(id),
    created_at    INTEGER NOT NULL,
    PRIMARY KEY (art_id, version)
);
CREATE INDEX IF NOT EXISTS idx_art_event ON artifact_versions(created_event);

-- 派生层结果：钉住依赖版本与父群版本；上游变化时标 stale，不覆盖旧行。
CREATE TABLE IF NOT EXISTS results (
    id                    TEXT PRIMARY KEY,
    branch                TEXT NOT NULL REFERENCES branches(name),
    scope                 TEXT NOT NULL,  -- gate | compensation | transform
    label                 TEXT NOT NULL,
    payload               TEXT NOT NULL,
    deps                  TEXT NOT NULL,  -- [{art_id,version,channels?}]
    parent_result         TEXT,
    parent_result_version TEXT,
    status                TEXT NOT NULL DEFAULT 'current', -- current | stale | revalidated
    produced_event        TEXT NOT NULL REFERENCES events(id),
    superseded_by         TEXT,
    created_at            INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_results_branch ON results(branch, status);

-- 无法唯一决定时保留的全部合法候选，等待人工裁定。
CREATE TABLE IF NOT EXISTS candidates (
    id                 TEXT PRIMARY KEY,
    branch             TEXT NOT NULL REFERENCES branches(name),
    scope              TEXT NOT NULL,
    label              TEXT NOT NULL,
    payload            TEXT NOT NULL,
    rationale          TEXT NOT NULL,
    status             TEXT NOT NULL DEFAULT 'open', -- open | accepted | rejected
    produced_event     TEXT NOT NULL REFERENCES events(id),
    adjudicated_event  TEXT,
    created_at         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cand_branch ON candidates(branch, status);
