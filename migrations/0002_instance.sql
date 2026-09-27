-- v2：增加实例槽位与门稳定身份登记表（改名不改身份），验证状态迁移保留历史。
CREATE TABLE IF NOT EXISTS instances (
    name        TEXT PRIMARY KEY,
    case_id     TEXT,
    created_at  INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

CREATE TABLE IF NOT EXISTS gate_identities (
    stable_id   TEXT NOT NULL,
    branch      TEXT NOT NULL REFERENCES branches(name),
    label       TEXT NOT NULL,
    renamed_at  INTEGER NOT NULL,
    PRIMARY KEY (stable_id, branch, renamed_at)
);
CREATE INDEX IF NOT EXISTS idx_gate_id ON gate_identities(stable_id, branch);

-- 来源版本间的局部来源血缘（哪一版修补自哪一版），便于反查差异来源。
CREATE TABLE IF NOT EXISTS artifact_lineage (
    art_id        TEXT NOT NULL,
    version       INTEGER NOT NULL,
    derived_from  TEXT NOT NULL,          -- "art_id@version"
    note          TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (art_id, version, derived_from)
);
