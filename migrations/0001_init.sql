-- FlowLine initial schema: content-addressed blobs, separately versioned
-- artifacts, append-only event log (provenance), immutable derived results,
-- review drafts and decision records.

CREATE TABLE blob_store (
    id       TEXT PRIMARY KEY,
    bytes    BLOB NOT NULL,
    media_type TEXT NOT NULL,
    kind     TEXT NOT NULL
);

CREATE TABLE artifact (
    logical_id   TEXT NOT NULL,
    version      INTEGER NOT NULL,
    kind         TEXT NOT NULL,
    id           TEXT NOT NULL UNIQUE,
    payload_json TEXT NOT NULL,
    source_blob_id TEXT,
    registered_at TEXT NOT NULL,
    PRIMARY KEY (logical_id, version)
);
CREATE INDEX idx_artifact_kind ON artifact(kind);

CREATE TABLE branch (
    id          TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    forked_from TEXT,
    fork_event_id TEXT,
    created_at  TEXT NOT NULL
);

CREATE TABLE event (
    id          TEXT PRIMARY KEY,
    seq         INTEGER NOT NULL,
    branch_id   TEXT NOT NULL,
    kind        TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    parent_event_id TEXT,
    created_at  TEXT NOT NULL,
    UNIQUE(branch_id, seq)
);
CREATE INDEX idx_event_branch ON event(branch_id, seq);

CREATE TABLE derived (
    id           TEXT PRIMARY KEY,
    group_id     TEXT NOT NULL,
    kind         TEXT NOT NULL,              -- comp_candidates | gate_result | root_population
    candidate_id INTEGER,                    -- NULL for gate/root
    payload_json TEXT NOT NULL,
    status       TEXT NOT NULL,              -- candidate | adopted | rejected
    created_by_event TEXT NOT NULL,
    created_at   TEXT NOT NULL
);
CREATE INDEX idx_derived_group ON derived(group_id, kind);

CREATE TABLE stale_marker (
    derived_id   TEXT NOT NULL,
    as_of_event  TEXT NOT NULL,
    reason_code  TEXT NOT NULL,
    detail_json  TEXT NOT NULL,
    PRIMARY KEY (derived_id, as_of_event)
);

CREATE TABLE decision (
    group_id     TEXT PRIMARY KEY,
    chosen_derived_id TEXT,
    event_id     TEXT NOT NULL,
    decided_at   TEXT NOT NULL
);

CREATE TABLE draft (
    id          TEXT PRIMARY KEY,
    branch_id   TEXT NOT NULL,
    label       TEXT NOT NULL,
    basis_event_id TEXT NOT NULL,
    revision    INTEGER NOT NULL,
    content_json TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE scenario_state (
    scenario_id TEXT PRIMARY KEY,
    next_step   INTEGER NOT NULL DEFAULT 0,
    updated_at  TEXT NOT NULL
);
