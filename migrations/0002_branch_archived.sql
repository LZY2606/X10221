-- Migration 2: branches can be archived; human adjudication decisions get an
-- explicit ledger. Both are backfilled/created without touching existing data.
ALTER TABLE branches ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;

-- Rulings are an append-only ledger keyed by a stable *decision slot* (e.g.
-- "comp:ridge" or "gate_edge:gate_key"). The chosen candidate is carried
-- forward into re-computation; going back in time replays the ledger.
CREATE TABLE IF NOT EXISTS branch_decisions (
    case_id       TEXT NOT NULL REFERENCES cases(case_id),
    branch_id     TEXT NOT NULL REFERENCES branches(branch_id),
    decision_slot TEXT NOT NULL,
    candidate_id  TEXT NOT NULL,
    rationale     TEXT NOT NULL DEFAULT '',
    event_id      TEXT NOT NULL,
    seq           INTEGER NOT NULL,
    PRIMARY KEY (case_id, branch_id, decision_slot, seq)
);

CREATE TABLE IF NOT EXISTS result_timeline (
    case_id   TEXT NOT NULL REFERENCES cases(case_id),
    result_id TEXT NOT NULL REFERENCES results(result_id),
    event_id  TEXT NOT NULL REFERENCES events(event_id),
    state     TEXT NOT NULL,
    PRIMARY KEY (case_id, result_id, event_id)
);
