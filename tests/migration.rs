//! 状态迁移测试：v1 → v2 迁移保留既有数据并建立新表。

use flow_lineage::db;

#[test]
fn migrates_v1_to_v2_preserving_data() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // 模拟 v1 库：只有 meta 表与一条业务标记。
    conn.execute_batch(
        "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
         INSERT INTO meta(key,value) VALUES ('schema_version','1');
         INSERT INTO meta(key,value) VALUES ('legacy_note','v1-era');",
    )
    .unwrap();

    db::migrate(&conn).unwrap();

    let v: String = conn
        .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, "2");
    // 旧数据保留
    let legacy: String = conn
        .query_row("SELECT value FROM meta WHERE key='legacy_note'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(legacy, "v1-era");
    // v2 表已建立且 main 分支存在
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM branches WHERE branch='main'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    // 迁移幂等
    db::migrate(&conn).unwrap();
}

#[test]
fn fresh_database_starts_at_v2() {
    let conn = db::open(":memory:").unwrap();
    let v: String = conn
        .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, "2");
}
