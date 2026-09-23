//! SQLite schema 迁移。
//!
//! 迁移测试会在 v1 数据库上执行 v2 迁移并验证回填结果。

pub const MIGRATION_001: &str = include_str!("../migrations/0001_init.sql");
pub const MIGRATION_002: &str = include_str!("../migrations/0002_provenance.sql");

pub fn migrations() -> Vec<(u32, &'static str)> {
    vec![(1, MIGRATION_001), (2, MIGRATION_002)]
}
