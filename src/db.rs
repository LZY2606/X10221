use rusqlite::Connection;

/// 迁移脚本：v1 建核心表，v2 增加分母钉住列与诊断表（用于状态迁移测试）。
pub const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE artifacts(
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        created_event INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE artifact_versions(
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        seq INTEGER NOT NULL,
        payload TEXT NOT NULL,
        raw_repr TEXT NOT NULL,
        source TEXT NOT NULL,
        event_seq INTEGER NOT NULL DEFAULT 0,
        UNIQUE(artifact_id, seq)
    );
    CREATE TABLE drafts(
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        parent_id TEXT,
        head_event INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE candidates(
        id TEXT PRIMARY KEY,
        draft_id TEXT NOT NULL REFERENCES drafts(id),
        population TEXT NOT NULL,
        label TEXT NOT NULL,
        status TEXT NOT NULL,
        value_json TEXT NOT NULL,
        provenance_json TEXT NOT NULL,
        event_seq INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE adjudications(
        id TEXT PRIMARY KEY,
        draft_id TEXT NOT NULL REFERENCES drafts(id),
        candidate_id TEXT NOT NULL,
        action TEXT NOT NULL,
        note TEXT NOT NULL,
        prev_status TEXT NOT NULL,
        prev_head INTEGER NOT NULL,
        undone INTEGER NOT NULL DEFAULT 0,
        event_seq INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE events(
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        id TEXT NOT NULL,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    "#,
    // v2
    r#"
    ALTER TABLE candidates ADD COLUMN denominator_json TEXT;
    CREATE TABLE diagnostics(
        id TEXT PRIMARY KEY,
        object_id TEXT NOT NULL,
        severity TEXT NOT NULL,
        code TEXT NOT NULL,
        message TEXT NOT NULL,
        event_seq INTEGER NOT NULL DEFAULT 0
    );
    "#,
];

pub fn current_version(conn: &Connection) -> rusqlite::Result<usize> {
    let ok = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_migrations'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0);
    if ok == 0 {
        return Ok(0);
    }
    let v: i64 = conn.query_row("SELECT COALESCE(MAX(version),0) FROM schema_migrations", [], |r| r.get(0))?;
    Ok(v as usize)
}

/// 迁移到指定版本（含），供迁移测试逐版本推进。
pub fn migrate_to(conn: &mut Connection, target: usize) -> rusqlite::Result<()> {
    if current_version(conn)? == 0 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )?;
    }
    let mut v = current_version(conn)?;
    while v < target && v < MIGRATIONS.len() {
        let tx = conn.transaction()?;
        tx.execute_batch(MIGRATIONS[v])?;
        tx.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, ?2)",
            rusqlite::params![v + 1, now()],
        )?;
        tx.commit()?;
        v += 1;
    }
    Ok(())
}

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    migrate_to(conn, MIGRATIONS.len())
}

pub fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_preserves_data_across_versions() {
        let mut conn = Connection::open_in_memory().unwrap();
        // 只推进到 v1，写入一行候选记录
        migrate_to(&mut conn, 1).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 1);
        conn.execute(
            "INSERT INTO drafts(id,name,head_event) VALUES('dft-0001','main',0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO candidates(id,draft_id,population,label,status,value_json,provenance_json,event_seq)
             VALUES('cand-0001','dft-0001','G1','base','pending','{}','[]',1)",
            [],
        )
        .unwrap();
        // 推进到 v2：新增 denominator_json 列与 diagnostics 表，旧数据必须保留
        migrate(&mut conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), MIGRATIONS.len());
        let status: String = conn
            .query_row("SELECT status FROM candidates WHERE id='cand-0001'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "pending");
        let denom: Option<String> = conn
            .query_row("SELECT denominator_json FROM candidates WHERE id='cand-0001'", [], |r| r.get(0))
            .unwrap();
        assert!(denom.is_none());
        conn.execute(
            "INSERT INTO diagnostics(id,object_id,severity,code,message,event_seq) VALUES('dgn-0001','e01','warn','extreme-signal','x',2)",
            [],
        )
        .unwrap();
        // 重复迁移是幂等的
        migrate(&mut conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), MIGRATIONS.len());
    }
}
