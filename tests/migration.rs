use flowline::db;
use rusqlite::Connection;

#[test]
fn fresh_db_migrates_to_latest() {
    let conn = Connection::open_in_memory().unwrap();
    db::migrate(&conn).unwrap();
    let v: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, db::MIGRATIONS.len() as i64);
    for t in ["materials", "drafts", "events", "results", "diagnostics"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [t],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "table {t} should exist");
    }
}

#[test]
fn stepwise_migration_preserves_existing_rows() {
    let conn = Connection::open_in_memory().unwrap();
    db::migrate_to(&conn, 1).unwrap();
    conn.execute(
        "INSERT INTO materials(id, kind, version, content, raw, committed_seq, created_at)
         VALUES ('mat:fcs_events:1', 'fcs_events', 1, '{\"events\":[]}', NULL, 1, '0')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO results(id, version, draft_id, node_id, content, inputs, stale, created_seq)
         VALUES ('pop:main:n1', 1, 'main', 'n1', '{}', '{}', 0, 1)",
        [],
    )
    .unwrap();
    db::migrate(&conn).unwrap();
    let kept: String = conn
        .query_row("SELECT id FROM materials WHERE kind='fcs_events'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept, "mat:fcs_events:1");
    let denom: Option<String> = conn
        .query_row("SELECT denominator FROM results WHERE id='pop:main:n1'", [], |r| r.get(0))
        .unwrap();
    assert!(denom.is_none(), "new column added by v2 defaults to NULL");
    let d: i64 = conn
        .query_row("SELECT COUNT(*) FROM diagnostics", [], |r| r.get(0))
        .unwrap();
    assert_eq!(d, 0);
}

#[test]
fn migration_is_idempotent() {
    let conn = Connection::open_in_memory().unwrap();
    db::migrate(&conn).unwrap();
    db::migrate(&conn).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, db::MIGRATIONS.len() as i64);
}
