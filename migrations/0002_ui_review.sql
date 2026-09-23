ALTER TABLE candidates ADD COLUMN display_label TEXT NOT NULL DEFAULT '';
ALTER TABLE candidate_statuses ADD COLUMN ui_visible INTEGER NOT NULL DEFAULT 1;
CREATE TABLE review_drafts (
  draft_id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL,
  expected_head_event_id TEXT NOT NULL,
  created_at TEXT NOT NULL
);
