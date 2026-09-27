//! SQLite 存储层：来源与解释分开保存，所有领域对象分开版本化。
use rusqlite::{params, Connection, Result};

/// 迁移脚本：按顺序应用，老库可逐步升级（状态迁移测试覆盖）。
pub const MIGRATIONS: &[(&str, &str)] = &[
    (
        "m1_core",
        r#"
        CREATE TABLE meta(
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE artifacts(
            id TEXT NOT NULL,
            version INTEGER NOT NULL,
            kind TEXT NOT NULL,
            raw TEXT NOT NULL,
            supersedes TEXT,
            branch TEXT NOT NULL DEFAULT 'main',
            created_seq INTEGER NOT NULL,
            PRIMARY KEY(id, version, branch)
        );
        CREATE TABLE events(
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL,
            branch TEXT NOT NULL DEFAULT 'main',
            payload TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE populations(
            id TEXT NOT NULL,
            version INTEGER NOT NULL,
            gate_id TEXT NOT NULL,
            gate_version INTEGER NOT NULL,
            parent_ref TEXT,
            count INTEGER NOT NULL,
            percent REAL,
            denominator_source TEXT,
            branch TEXT NOT NULL DEFAULT 'main',
            basis_seq INTEGER NOT NULL,
            detail TEXT NOT NULL,
            PRIMARY KEY(id, version, branch)
        );
        CREATE TABLE invalidations(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            population_id TEXT NOT NULL,
            population_version INTEGER NOT NULL,
            branch TEXT NOT NULL,
            seq INTEGER NOT NULL,
            cause TEXT NOT NULL
        );
        CREATE TABLE adjudications(
            id TEXT PRIMARY KEY,
            target TEXT NOT NULL,
            decision TEXT NOT NULL,
            branch TEXT NOT NULL,
            seq INTEGER NOT NULL,
            payload TEXT NOT NULL,
            undone INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE drafts(
            id TEXT NOT NULL,
            branch TEXT NOT NULL,
            version INTEGER NOT NULL,
            content TEXT NOT NULL,
            seq INTEGER NOT NULL,
            PRIMARY KEY(id, branch, version)
        );
        "#,
    ),
    (
        "m2_events_actor",
        r#"
        ALTER TABLE events ADD COLUMN actor TEXT NOT NULL DEFAULT 'system';
        CREATE INDEX idx_events_kind ON events(kind);
        CREATE INDEX idx_populations_gate ON populations(gate_id, branch);
        "#,
    ),
];

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, name TEXT NOT NULL);",
    )?;
    let current: i64 =
        conn.query_row("SELECT COALESCE(MAX(version),0) FROM schema_migrations", [], |r| r.get(0))?;
    for (idx, (name, sql)) in MIGRATIONS.iter().enumerate() {
        let version = idx as i64 + 1;
        if version > current {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations(version,name) VALUES(?1,?2)",
                params![version, name],
            )?;
            tx.commit()?;
        }
    }
    Ok(())
}

pub fn open(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
    Ok(conn)
}

/// 库是否为空（尚未载入任何案例包）。
pub fn is_empty(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM meta", [], |r| r.get(0))?;
    Ok(n == 0)
}

/// 清空全部领域表（导入档案/强制重载前调用），保留迁移版本。
pub fn wipe_domain(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM meta; DELETE FROM artifacts; DELETE FROM events;
         DELETE FROM populations; DELETE FROM invalidations;
         DELETE FROM adjudications; DELETE FROM drafts;
         DELETE FROM sqlite_sequence;",
    )?;
    Ok(())
}
