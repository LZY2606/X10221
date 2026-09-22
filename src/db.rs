//! SQLite 持久层与显式、可测的版本化 schema 迁移。

use rusqlite::Connection;
use std::path::Path;

pub const CURRENT_SCHEMA: u32 = 3;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<u32> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    let version: u32 = conn
        .query_row("SELECT value FROM schema_meta WHERE key='version'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if version >= CURRENT_SCHEMA {
        return Ok(version);
    }
    if version == 0 {
        apply_v1(conn)?;
        set_version(conn, 1)?;
    }
    if version <= 1 {
        apply_v2(conn)?;
        set_version(conn, 2)?;
    }
    if version <= 2 {
        apply_v3(conn)?;
        set_version(conn, 3)?;
    }
    Ok(CURRENT_SCHEMA)
}

fn set_version(conn: &Connection, v: u32) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO schema_meta(key,value) VALUES('version',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [v.to_string()],
    )?;
    Ok(())
}

fn apply_v1(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE cases(
            case_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            fixture_ref TEXT,
            created_seq INTEGER NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE objects(
            case_id TEXT NOT NULL,
            obj_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            current_seq INTEGER NOT NULL,
            PRIMARY KEY(case_id, obj_id)
        );
        CREATE TABLE versions(
            case_id TEXT NOT NULL,
            obj_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            version_id TEXT NOT NULL UNIQUE,
            payload TEXT NOT NULL,
            source_sha TEXT NOT NULL,
            created_event TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY(case_id, obj_id, seq)
        );
        CREATE TABLE assets(
            case_id TEXT NOT NULL,
            asset_id TEXT NOT NULL PRIMARY KEY,
            filename TEXT NOT NULL,
            media_type TEXT NOT NULL,
            bytes BLOB NOT NULL,
            sha256 TEXT NOT NULL,
            created_event TEXT NOT NULL
        );
        CREATE TABLE events(
            case_id TEXT NOT NULL,
            branch_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            event_id TEXT NOT NULL UNIQUE,
            parent_event_id TEXT,
            kind TEXT NOT NULL,
            payload TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY(case_id, branch_id, seq)
        );
        CREATE TABLE branches(
            case_id TEXT NOT NULL,
            branch_id TEXT NOT NULL,
            name TEXT NOT NULL,
            parent_branch_id TEXT,
            forked_from_event TEXT,
            tip_event_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY(case_id, branch_id)
        );
        CREATE TABLE results(
            case_id TEXT NOT NULL,
            result_id TEXT NOT NULL,
            branch_id TEXT NOT NULL,
            node_id TEXT NOT NULL,
            gate_version_id TEXT NOT NULL,
            parent_result_id TEXT,
            denominator_result_id TEXT,
            denominator_count INTEGER NOT NULL,
            numerator_count INTEGER NOT NULL,
            percent REAL NOT NULL,
            memberships TEXT NOT NULL,
            basis TEXT NOT NULL,
            candidate_id TEXT,
            adopted INTEGER NOT NULL DEFAULT 1,
            stale INTEGER NOT NULL DEFAULT 0,
            stale_event TEXT,
            created_event TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY(case_id, result_id)
        );
        CREATE TABLE candidates(
            case_id TEXT NOT NULL,
            candidate_id TEXT NOT NULL PRIMARY KEY,
            branch_id TEXT NOT NULL,
            target_kind TEXT NOT NULL,
            target_ref TEXT NOT NULL,
            label TEXT NOT NULL,
            payload TEXT NOT NULL,
            rationale TEXT NOT NULL,
            adopted INTEGER NOT NULL DEFAULT 0,
            decided_event TEXT,
            created_event TEXT NOT NULL
        );
        CREATE TABLE decisions(
            case_id TEXT NOT NULL,
            decision_id TEXT NOT NULL PRIMARY KEY,
            branch_id TEXT NOT NULL,
            action TEXT NOT NULL,
            subject_ref TEXT NOT NULL,
            candidate_id TEXT,
            reverted_decision_id TEXT,
            note TEXT,
            event_id TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE diagnostics(
            case_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            code TEXT NOT NULL,
            severity TEXT NOT NULL,
            subject_ref TEXT NOT NULL,
            message TEXT NOT NULL,
            detail TEXT NOT NULL,
            seq INTEGER NOT NULL
        );
        CREATE TABLE conflicts(
            case_id TEXT NOT NULL,
            conflict_id TEXT NOT NULL PRIMARY KEY,
            branch_id TEXT NOT NULL,
            expected_tip TEXT NOT NULL,
            actual_tip TEXT NOT NULL,
            incoming TEXT NOT NULL,
            existing TEXT NOT NULL,
            diff TEXT NOT NULL,
            resolved_event TEXT,
            created_event TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        "#,
    )
}

fn apply_v2(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        ALTER TABLE objects ADD COLUMN note TEXT NOT NULL DEFAULT '';
        CREATE TABLE source_annotations(
            case_id TEXT NOT NULL,
            annotation_id TEXT NOT NULL PRIMARY KEY,
            obj_id TEXT NOT NULL,
            version_id TEXT,
            note TEXT NOT NULL,
            created_event TEXT NOT NULL
        );
        "#,
    )
}

fn apply_v3(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("ALTER TABLE events ADD COLUMN actor TEXT NOT NULL DEFAULT 'reviewer';")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_through_all_versions_and_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_v1(&conn).unwrap();
        set_version(&conn, 1).unwrap();
        let v = migrate(&mut conn).unwrap();
        assert_eq!(v, CURRENT_SCHEMA);
        // 再跑一次不报错且版本不变。
        let v2 = migrate(&mut conn).unwrap();
        assert_eq!(v2, CURRENT_SCHEMA);
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('events') WHERE name='actor'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(n, 1);
        conn.execute("INSERT INTO objects(case_id,obj_id,kind,current_seq) VALUES('c','o','fcs',1)", []).unwrap();
    }

    #[test]
    fn v1_schema_usable_before_later_columns_exist() {
        let conn = Connection::open_in_memory().unwrap();
        apply_v1(&conn).unwrap();
        conn.execute("INSERT INTO cases VALUES('c','n',NULL,1,'t')", []).unwrap();
        // v1 的 events 还没有 actor 列。
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('events') WHERE name='actor'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(n, 0);
    }
}
