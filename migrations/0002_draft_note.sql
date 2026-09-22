-- v2: drafts carry an optional reviewer note; existing drafts default to empty.
ALTER TABLE draft ADD COLUMN note TEXT NOT NULL DEFAULT '';
CREATE INDEX IF NOT EXISTS idx_draft_branch ON draft(case_id, branch_id);
