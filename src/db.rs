use rusqlite::Connection;

/// 迁移按顺序应用；`PRAGMA user_version` 记录已应用的迁移数。
/// v1: 分开版本化的实体（原始通道值 / spillover / 补偿矩阵 / 变换参数 / 门 / 父子关系）
///     与追加式事件日志。
/// v2: 派生层 —— 群体结果、候选与裁定分支、诊断。
pub const MIGRATIONS: &[&str] = &[
    "CREATE TABLE entities (
        entity_id TEXT NOT NULL,
        version INTEGER NOT NULL,
        branch TEXT NOT NULL,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_event TEXT NOT NULL,
        created_seq INTEGER NOT NULL,
        PRIMARY KEY (entity_id, version, branch)
    );
    CREATE TABLE events (
        event_id TEXT PRIMARY KEY,
        seq INTEGER NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );",
    "CREATE TABLE populations (
        pop_id TEXT PRIMARY KEY,
        gate_entity TEXT NOT NULL,
        gate_version INTEGER NOT NULL,
        branch TEXT NOT NULL,
        parent_gate TEXT,
        parent_version INTEGER,
        denominator INTEGER NOT NULL,
        count INTEGER NOT NULL,
        percent REAL NOT NULL,
        status TEXT NOT NULL,
        dependencies TEXT NOT NULL,
        basis_event TEXT NOT NULL
    );
    CREATE TABLE candidates (
        candidate_id TEXT PRIMARY KEY,
        subject TEXT NOT NULL,
        detail TEXT NOT NULL,
        decision TEXT,
        decided_event TEXT,
        branch TEXT NOT NULL
    );
    CREATE TABLE diagnostics (
        diag_id TEXT PRIMARY KEY,
        event_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        subject TEXT NOT NULL,
        detail TEXT NOT NULL
    );",
];

pub fn migrate_to(conn: &Connection, target: usize) -> rusqlite::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let current = current as usize;
    for (idx, sql) in MIGRATIONS.iter().enumerate() {
        if idx >= current && idx < target {
            conn.execute_batch(sql)?;
        }
    }
    conn.pragma_update(None, "user_version", target as i64)?;
    Ok(())
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    migrate_to(conn, MIGRATIONS.len())
}
