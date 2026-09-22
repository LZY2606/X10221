//! SQLite 连接与迁移。迁移脚本全部显式版本化，用于状态迁移测试。

use rusqlite::Connection;

use crate::Result;

pub const MIGRATION_0001: &str = include_str!("db/migrations/0001_init.sql");
pub const MIGRATION_0002: &str = include_str!("db/migrations/0002_actor.sql");

pub fn migrate(conn: &mut Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
         CREATE TABLE IF NOT EXISTS schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    let version: i64 = conn
        .query_row(
            "SELECT CAST(COALESCE(value,'0') AS INTEGER) FROM schema_meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let steps: [(i64, &str); 2] = [(1, MIGRATION_0001), (2, MIGRATION_0002)];
    for (v, sql) in steps {
        if version < v {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_meta(key,value) VALUES('schema_version',?1)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [v.to_string()],
            )?;
            tx.commit()?;
        }
    }
    Ok(())
}

pub fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT CAST(COALESCE(value,'0') AS INTEGER) FROM schema_meta WHERE key='schema_version'",
        [],
        |r| r.get(0),
    )?)
}

/// 仅应用到 0001，供状态迁移测试使用。
pub fn migrate_to_v1(conn: &mut Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
         CREATE TABLE IF NOT EXISTS schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    let tx = conn.transaction()?;
    tx.execute_batch(MIGRATION_0001)?;
    tx.execute(
        "INSERT INTO schema_meta(key,value) VALUES('schema_version','1')
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_goes_to_latest() {
        let mut c = Connection::open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        assert_eq!(schema_version(&c).unwrap(), 2);
        migrate(&mut c).unwrap();
        assert_eq!(schema_version(&c).unwrap(), 2);
    }

    #[test]
    fn v1_to_v2_preserves_rows_and_adds_actor() {
        let mut c = Connection::open_in_memory().unwrap();
        migrate_to_v1(&mut c).unwrap();
        c.execute(
            "INSERT INTO blobs(blob_id,kind,payload,sha256,seq) VALUES('BL.x','note','{}','h',1)",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO branches(branch_id,name,base_branch_id,head_seq,head_event_id,created_seq)
             VALUES('BR.test','t',NULL,0,NULL,1)",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO events(branch_id,seq,event_id,kind,actor,payload_json,ts_ms)
             VALUES('BR.test',1,'EV.x','note','old','{}',0)",
            [],
        )
        .unwrap();
        // v1 的 events 表没有 actor NOT NULL 默认以外的新列；迁移补 0002。
        migrate(&mut c).unwrap();
        assert_eq!(schema_version(&c).unwrap(), 2);
        let n: i64 = c.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // 0002 后可以登记同版本并发冲突。
        c.execute(
            "INSERT INTO conflicts(conflict_id,branch_id,expected_seq,incoming_json,current_json,ts_ms)
             VALUES('CF.x','BR.test',1,'{}','{}',0)",
            [],
        )
        .unwrap();
        let cn: i64 = c.query_row("SELECT COUNT(*) FROM conflicts", [], |r| r.get(0)).unwrap();
        assert_eq!(cn, 1);
        // 追加事件仍需显式/调用方提供 actor，旧 actor 值保持不变。
        c.execute("INSERT INTO events(branch_id,seq,event_id,kind,actor,payload_json,ts_ms)
                   VALUES('BR.test',2,'EV.y','note','reviewer','{}',0)", []).unwrap();
        let actor: String = c
            .query_row("SELECT actor FROM events WHERE event_id='EV.y'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(actor, "reviewer");
        let actor1: String = c
            .query_row("SELECT actor FROM events WHERE event_id='EV.x'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(actor1, "old");
    }
}
