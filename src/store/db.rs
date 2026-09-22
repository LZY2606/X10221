//! SQLite schema 与版本迁移。

use rusqlite::Connection;

pub const CURRENT_SCHEMA: i64 = 2;

/// 迁移按编号顺序执行；每条迁移在一个事务里，失败回滚。
/// 迁移 2 演示真实状态迁移：给 materials 增加 label 列并回填。
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<i64> {
    let current: i64 =
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?;
    if current >= CURRENT_SCHEMA {
        return Ok(current);
    }
    if current < 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(include_str!("migrations/001_init.sql"))?;
        tx.execute_batch("PRAGMA user_version = 1;")?;
        tx.commit()?;
    }
    if current < 2 {
        let tx = conn.transaction()?;
        tx.execute_batch(include_str!("migrations/002_material_label.sql"))?;
        tx.execute_batch("PRAGMA user_version = 2;")?;
        tx.commit()?;
    }
    Ok(CURRENT_SCHEMA)
}
