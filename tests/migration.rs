use flowlineage::{Service, Store};
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn migration_from_v1_preserves_events_and_backfills_title() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("m1.db");
    let path = db.to_str().unwrap();

    // 1) 手工建 v1 schema 并写入一条事件
    {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(include_str!("../migrations/0001_init.sql")).unwrap();
        conn.execute(
            "INSERT INTO schema_meta(key,value) VALUES ('schema_version','1')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO projects(project_id, head_seq) VALUES ('p1', 1)", [])
            .unwrap();
        let payload = serde_json::json!({
            "type": "project_created",
            "project_id": "p1",
            "title": "迁移前项目"
        });
        conn.execute(
            "INSERT INTO events(seq, project_id, branch, branch_start_seq, event_id, kind, payload_json)
             VALUES (1,'p1','main',0,'evt-1','project_created',?1)",
            rusqlite::params![payload.to_string()],
        )
        .unwrap();
    }

    // 2) 打开即在 v1 之上迁移到 v2，旧事件必须存活，标题从事件回填
    let store = Store::open(path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 2);
    let events = store.load_events("p1").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, "evt-1");
    let projects = store.list_projects().unwrap();
    let p = projects.iter().find(|(id, _, _, _)| id == "p1").unwrap();
    assert_eq!(p.1, "迁移前项目", "v2 必须回填标题");
    assert_eq!(p.2, 1, "head_seq 不丢");

    // 3) 迁移后服务仍可追加事件
    let svc = Service::new(Arc::new(store));
    svc.create_project("p2", "新项目").unwrap();
    assert!(svc.view("p2", "main", None).is_ok());
}
