-- FlowLine initial schema: sources and interpretations are stored apart, and
-- each of the six interpretation layers is independently versioned.

CREATE TABLE IF NOT EXISTS cases (
    case_id        TEXT PRIMARY KEY,
    key_slug       TEXT NOT NULL UNIQUE,
    title          TEXT NOT NULL,
    manifest_json  TEXT NOT NULL,
    created_event  TEXT NOT NULL,
    created_at_ts  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS raw_blobs (
    case_id   TEXT NOT NULL REFERENCES cases(case_id),
    blob_id   TEXT NOT NULL,
    filename  TEXT NOT NULL,
    media_type TEXT NOT NULL,
    bytes     BLOB NOT NULL,
    sha256    TEXT NOT NULL,
    PRIMARY KEY (case_id, blob_id)
);

-- Generic versioned entities. stream_id identifies the "line" (e.g.
-- "comp_matrix:baseline"); each source revision creates a new (id, version).
CREATE TABLE IF NOT EXISTS entities (
    case_id     TEXT NOT NULL REFERENCES cases(case_id),
    id          TEXT NOT NULL,
    stream_id   TEXT NOT NULL,
    kind        TEXT NOT NULL,
    version     INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    created_event TEXT,
    PRIMARY KEY (case_id, id),
    UNIQUE (case_id, stream_id, version)
);

CREATE TABLE IF NOT EXISTS branches (
    case_id        TEXT NOT NULL REFERENCES cases(case_id),
    branch_id      TEXT NOT NULL,
    key_name       TEXT NOT NULL,
    parent_branch_id TEXT,
    base_event_id  TEXT,
    label          TEXT NOT NULL,
    revision       INTEGER NOT NULL DEFAULT 0,
    created_at_ts  INTEGER NOT NULL,
    PRIMARY KEY (case_id, branch_id),
    UNIQUE (case_id, key_name)
);

-- Current tip refs of every independently versioned stream for a branch.
CREATE TABLE IF NOT EXISTS branch_refs (
    case_id   TEXT NOT NULL REFERENCES cases(case_id),
    branch_id TEXT NOT NULL REFERENCES branches(branch_id),
    stream_id TEXT NOT NULL,
    entity_id TEXT NOT NULL REFERENCES entities(id),
    version   INTEGER NOT NULL,
    PRIMARY KEY (case_id, branch_id, stream_id)
);

-- Append-only, machine-readable event log. Events are content addressed and
-- immutable; going back to any event restores that point's dependencies.
CREATE TABLE IF NOT EXISTS events (
    case_id      TEXT NOT NULL REFERENCES cases(case_id),
    branch_id    TEXT NOT NULL REFERENCES branches(branch_id),
    event_id     TEXT NOT NULL,
    seq          INTEGER NOT NULL,
    event_type   TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at_ts INTEGER NOT NULL,
    PRIMARY KEY (case_id, event_id),
    UNIQUE (case_id, branch_id, seq)
);

-- Immutable computation results. A result is tied to the exact dependency
-- versions it was computed from and to a branch. Re-computation creates a new
-- result row; old results are marked stale, never overwritten.
CREATE TABLE IF NOT EXISTS results (
    result_id   TEXT NOT NULL,
    case_id     TEXT NOT NULL REFERENCES cases(case_id),
    branch_id   TEXT NOT NULL REFERENCES branches(branch_id),
    kind        TEXT NOT NULL,
    target_key  TEXT NOT NULL,
    status      TEXT NOT NULL,
    value_json  TEXT,
    denominator_result_id TEXT,
    computed_at_event TEXT NOT NULL,
    stale       INTEGER NOT NULL DEFAULT 0,
    stale_reason TEXT,
    ruling_id   TEXT,
    PRIMARY KEY (case_id, result_id)
);

CREATE TABLE IF NOT EXISTS result_candidates (
    case_id   TEXT NOT NULL REFERENCES cases(case_id),
    result_id TEXT NOT NULL REFERENCES results(result_id),
    candidate_id TEXT NOT NULL,
    label     TEXT NOT NULL,
    accepted  INTEGER,
    value_json TEXT NOT NULL,
    rationale TEXT NOT NULL,
    PRIMARY KEY (case_id, result_id, candidate_id)
);

CREATE TABLE IF NOT EXISTS result_deps (
    case_id   TEXT NOT NULL REFERENCES cases(case_id),
    result_id TEXT NOT NULL REFERENCES results(result_id),
    stream_id TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    version   INTEGER NOT NULL,
    PRIMARY KEY (case_id, result_id, stream_id)
);
