CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    case_id TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS branches (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    name TEXT NOT NULL,
    parent_branch_id TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS events_log (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    parent_event_id TEXT,
    seq INTEGER NOT NULL,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    note TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_project ON events_log(project_id, seq);

CREATE TABLE IF NOT EXISTS object_versions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    identity TEXT NOT NULL,
    version_no INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    superseded_by TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_objects ON object_versions(project_id, kind, identity, version_no);

CREATE TABLE IF NOT EXISTS comp_candidates (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    spillover_version TEXT NOT NULL,
    candidate_key TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL,
    current INTEGER NOT NULL,
    superseded_by TEXT,
    event_id TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cand ON comp_candidates(project_id, current);

CREATE TABLE IF NOT EXISTS populations (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    population_id TEXT NOT NULL,
    version_no INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL,
    current INTEGER NOT NULL,
    stale_reason TEXT,
    superseded_by TEXT,
    event_id TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pop ON populations(project_id, current);

CREATE TABLE IF NOT EXISTS decisions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    subject_kind TEXT NOT NULL,
    subject_ref TEXT NOT NULL,
    choice TEXT NOT NULL,
    rationale TEXT,
    reverted_by_event_id TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS conflicts (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    expected_head TEXT,
    actual_head TEXT,
    attempted_action TEXT NOT NULL,
    diff_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
