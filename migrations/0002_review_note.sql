-- Migration 0002: add an optional review note to decisions (state migration).
ALTER TABLE decision ADD COLUMN note TEXT;
