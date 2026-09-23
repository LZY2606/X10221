//! SQLite 结构迁移：每次结构变化都是一份独立、可重放的 SQL。

pub const MIGRATIONS: &[(&str, &str)] = &[
    // v1：基线五表。事件表保存推导快照，保证历史不被最新计算覆盖。
    (
        "0001_baseline",
        r#"
        CREATE TABLE meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE branches (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            parent      TEXT,
            fork_event  TEXT,
            head        TEXT NOT NULL,
            status      TEXT NOT NULL DEFAULT 'open',
            created_seq INTEGER NOT NULL
        );
        CREATE TABLE source_versions (
            id         TEXT PRIMARY KEY,
            kind       TEXT NOT NULL,
            seq        INTEGER NOT NULL,
            payload    TEXT NOT NULL,
            created_by TEXT NOT NULL
        );
        CREATE TABLE events (
            id         TEXT PRIMARY KEY,
            seq        INTEGER NOT NULL UNIQUE,
            branch     TEXT NOT NULL,
            kind       TEXT NOT NULL,
            summary    TEXT NOT NULL,
            payload    TEXT NOT NULL,
            view       TEXT,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE decisions (
            id             TEXT PRIMARY KEY,
            event_id       TEXT NOT NULL,
            branch         TEXT NOT NULL,
            action         TEXT NOT NULL,
            subject        TEXT NOT NULL,
            rationale      TEXT NOT NULL DEFAULT '',
            reviewer       TEXT NOT NULL DEFAULT 'reviewer',
            status         TEXT NOT NULL,
            revert_of      TEXT
        );
        "#,
    ),
    // v2：同版本并发修改的被拒提交留档 + 裁定冲突分组。
    (
        "0002_conflict_audit",
        r#"
        CREATE TABLE rejected_commits (
            id           TEXT PRIMARY KEY,
            branch       TEXT NOT NULL,
            base_event   TEXT NOT NULL,
            current_head TEXT NOT NULL,
            action       TEXT NOT NULL,
            payload      TEXT NOT NULL,
            diff         TEXT NOT NULL,
            created_at   INTEGER NOT NULL
        );
        ALTER TABLE decisions ADD COLUMN conflict_group TEXT;
        "#,
    ),
];
