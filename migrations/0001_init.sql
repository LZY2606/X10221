CREATE TABLE IF NOT EXISTS branches (
    name        TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    parent      TEXT,
    fork_seq    INTEGER,
    created_seq INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS materials (
    kind      TEXT NOT NULL,
    id        TEXT NOT NULL,
    version   INTEGER NOT NULL,
    title     TEXT NOT NULL,
    raw_json  TEXT NOT NULL,
    sha256    TEXT NOT NULL,
    source_file TEXT,
    PRIMARY KEY (kind, id, version)
);

CREATE TABLE IF NOT EXISTS events (
    seq       INTEGER NOT NULL,
    branch    TEXT NOT NULL,
    kind      TEXT NOT NULL,
    subject   TEXT NOT NULL,
    payload   TEXT NOT NULL,
    basis     TEXT NOT NULL,
    basis_rules TEXT NOT NULL,
    mat_kind  TEXT,
    mat_id    TEXT,
    mat_version INTEGER,
    origin_seq INTEGER,
    origin_branch TEXT,
    PRIMARY KEY (branch, seq)
);

CREATE TABLE IF NOT EXISTS decisions (
    branch      TEXT NOT NULL,
    subject     TEXT NOT NULL,
    active      INTEGER NOT NULL,
    candidate   TEXT,
    decided_seq INTEGER NOT NULL,
    PRIMARY KEY (branch, subject)
);

CREATE TABLE IF NOT EXISTS derived_results (
    branch    TEXT NOT NULL,
    node_id   TEXT NOT NULL,
    cfg_hash  TEXT NOT NULL,
    seq       INTEGER NOT NULL,
    status    TEXT NOT NULL,
    data_json TEXT NOT NULL,
    basis_json TEXT NOT NULL,
    PRIMARY KEY (branch, node_id, cfg_hash)
);

CREATE TABLE IF NOT EXISTS conflict_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    branch      TEXT NOT NULL,
    observed_seq INTEGER NOT NULL,
    expected_rev INTEGER NOT NULL,
    attempted_rev INTEGER NOT NULL,
    diff_json   TEXT NOT NULL,
    at_unix     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS bundles (
    bundle_id TEXT PRIMARY KEY,
    exported_seq INTEGER NOT NULL,
    imported_at_unix INTEGER
);
