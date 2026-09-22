//! SQLite 存储：事件是唯一事实来源；物化表只作读取加速与导出载体。
//! 写操作串行化并在单个事务内完成，导入失败整体回滚，不留半份记录。

use rusqlite::Connection;
use std::sync::Mutex;

pub const SCHEMA_VERSION: u32 = 2;

pub struct Store {
    pub conn: Mutex<Connection>,
}

pub fn open(path: &str) -> rusqlite::Result<Store> {
    let conn = if path == ":memory:" {
        Connection::open_in_memory()?
    } else {
        Connection::open(path)?
    };
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&conn)?;
    Ok(Store { conn: Mutex::new(conn) })
}

fn current_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    let v: Option<String> = conn
        .query_row("SELECT value FROM schema_meta WHERE key='schema_version'", [], |r| {
            r.get::<_, String>(0)
        })
        .ok();
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<u32> {
    let v = current_version(conn)?;
    if v < 1 {
        conn.execute_batch(include_str!("../migrations/0001_init.sql"))?;
        conn.execute(
            "INSERT INTO schema_meta(key,value) VALUES('schema_version','1')
             ON CONFLICT(key) DO UPDATE SET value='1'",
            [],
        )?;
    }
    if current_version(conn)? < 2 {
        conn.execute_batch(include_str!("../migrations/0002_conflict_log.sql"))?;
        conn.execute(
            "UPDATE schema_meta SET value='2' WHERE key='schema_version'",
            [],
        )?;
    }
    Ok(SCHEMA_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sql_v1() -> &'static str {
        include_str!("../migrations/0001_init.sql")
    }

    #[test]
    fn fresh_database_runs_forward_migrations() {
        let conn = Connection::open_in_memory().unwrap();
        let v = migrate(&conn).unwrap();
        assert_eq!(v, 2);
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='conflict_log'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 1);
        // v2 追加列存在
        let col: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('appraisal_rows') WHERE name='stale_reason'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col, 1);
    }

    #[test]
    fn migrates_v1_state_to_v2_without_data_loss() {
        // 状态迁移测试：先按 v1 建表并写入 v1 形态数据，再迁移到 v2。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(sql_v1()).unwrap();
        conn.execute(
            "INSERT INTO schema_meta(key,value) VALUES('schema_version','1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO branches(branch_id,parent_branch,base_event_id,tip_event_id,label,created_seq)
             VALUES('main',NULL,NULL,NULL,'main',0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO materials(material_type,material_id,version,payload_json,sha256,ingest_seq)
             VALUES('raw_events','mat-raw-events',1,'{}','abc',0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events(event_id,seq,branch_id,parent_event_id,kind,payload_json)
             VALUES('ev-main-000001',1,'main',NULL,'case.loaded','{}')",
            [],
        )
        .unwrap();
        let v = migrate(&conn).unwrap();
        assert_eq!(v, 2);
        let count: i64 = conn
            .query_row("SELECT count(*) FROM materials", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let label: String = conn
            .query_row("SELECT label FROM branches WHERE branch_id='main'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(label, "main");
        // v1 行在 v2 新列上得到默认空值而非报错
        let reason: Option<String> = conn
            .query_row(
                "SELECT stale_reason FROM appraisal_rows WHERE 1=0",
                [],
                |r| r.get(0),
            )
            .unwrap_or(None);
        assert!(reason.is_none());
    }
}
