//! SQLite 存储与状态迁移。
//!
//! 设计要点：
//! - `materials`：六类来源材料只追加不覆盖，按 (id, version) 分开版本化；
//! - `events`：追加式事件日志，事件 id 形如 `evt-000012`，导出/导入后稳定；
//! - `populations`：派生结论每次变化新增一行（seq），旧结论不被覆盖；
//! - `drafts`：乐观锁版本号，同版本并发修改由后提交方看到冲突。

use rusqlite::{params, Connection, Result};

pub const MIGRATION_0001: &str = r#"
CREATE TABLE materials (
    id          TEXT NOT NULL,
    version     INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    payload     TEXT NOT NULL,
    created_seq INTEGER NOT NULL,
    PRIMARY KEY (id, version)
);
CREATE TABLE events (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    id         TEXT NOT NULL UNIQUE,
    kind       TEXT NOT NULL,
    draft      TEXT NOT NULL DEFAULT 'main',
    payload    TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE populations (
    seq                 INTEGER NOT NULL,
    draft               TEXT NOT NULL,
    pop_id              TEXT NOT NULL,
    gate_id             TEXT NOT NULL,
    gate_version        INTEGER NOT NULL,
    parent_pop_id       TEXT,
    parent_pop_version  INTEGER,
    denom_count         INTEGER NOT NULL,
    count               INTEGER NOT NULL,
    percent             REAL NOT NULL,
    status              TEXT NOT NULL,
    candidates          TEXT NOT NULL DEFAULT '[]',
    pins                TEXT NOT NULL,
    PRIMARY KEY (draft, pop_id, seq)
);
CREATE TABLE drafts (
    id       TEXT PRIMARY KEY,
    name     TEXT NOT NULL,
    version  INTEGER NOT NULL,
    head_seq INTEGER NOT NULL
);
"#;

/// 第二次迁移：诊断、冲突与候选裁定落表，证明迁移会保留旧数据。
pub const MIGRATION_0002: &str = r#"
CREATE TABLE diagnostics (
    seq              INTEGER PRIMARY KEY,
    draft            TEXT NOT NULL,
    material_ref     TEXT NOT NULL,
    severity         TEXT NOT NULL,
    code             TEXT NOT NULL,
    message          TEXT NOT NULL,
    subject          TEXT NOT NULL DEFAULT ''
);
CREATE TABLE conflicts (
    seq          INTEGER PRIMARY KEY,
    draft        TEXT NOT NULL,
    base_version INTEGER NOT NULL,
    incumbent    TEXT NOT NULL,
    challenger   TEXT NOT NULL,
    resolved     INTEGER NOT NULL DEFAULT 0
);
ALTER TABLE populations ADD COLUMN note TEXT NOT NULL DEFAULT '';
"#;

pub const MIGRATIONS: &[(&str, &str)] =
    &[("0001_init", MIGRATION_0001), ("0002_review_diagnostics", MIGRATION_0002)];

pub fn open(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
    Ok(conn)
}

/// 应用尚未执行的迁移；迁移只追加。失败时 rusqlite 自动回滚 execute_batch。
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at INTEGER NOT NULL
        );",
    )?;
    let applied: i64 = conn
        .query_row("SELECT COALESCE(MAX(version),0) FROM schema_migrations", [], |r| r.get(0))
        .unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for (i, (name, sql)) in MIGRATIONS.iter().enumerate() {
        let version = (i + 1) as i64;
        if version > applied {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO schema_migrations(version, name, applied_at) VALUES (?1,?2,?3)",
                params![version, name, now],
            )?;
        }
    }
    Ok(())
}

pub fn applied_migrations(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare("SELECT version, name FROM schema_migrations ORDER BY version")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 状态迁移测试：在 v1 库上写入数据，迁移到最新后旧数据必须保留，
    /// v2 新增结构可用。
    #[test]
    fn migration_v1_to_v2_preserves_data() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        conn.execute_batch(MIGRATION_0001).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations(version,name,applied_at) VALUES (1,'0001_init',0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO materials(id,version,kind,payload,created_seq)
             VALUES ('mat:comp:fl3',1,'comp_matrix','{\"note\":\"old\"}',1)",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let versions = applied_migrations(&conn).unwrap();
        assert_eq!(versions.len(), 2);
        let payload: String = conn
            .query_row(
                "SELECT payload FROM materials WHERE id='mat:comp:fl3' AND version=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(payload.contains("old"));
        conn.execute_batch("INSERT INTO diagnostics(seq,draft,material_ref,severity,code,message)
                            VALUES (1,'main','mat:x','info','ok','ok')").unwrap();
        conn.execute_batch(
            "INSERT INTO events(seq,id,kind,draft,payload,created_at)
             VALUES (99,'evt-000099','probe','main','{}',0)",
        )
        .unwrap();
    }
}
