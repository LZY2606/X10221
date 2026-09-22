//! 归档往返：导出 → 删除本地数据库 → 导入，身份/顺序/来源关系保持。

use flow_lineage::db;
use flow_lineage::domain::EventPayload;
use flow_lineage::{archive, scenarios, service, web};

fn temp_db(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("flow-lineage-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("case.db").to_string_lossy().to_string()
}

#[test]
fn export_delete_import_preserves_identity_order_and_lineage() {
    let path = temp_db("archive");
    let st = web::build_state(&path).unwrap();
    let (mut conn, events_before, view_before, branches_before);
    {
        let mut guard = st.conn.blocking_lock();
        // 追加两个场景，制造谱系与候选
        let head = db::head_seq(&guard, "main").unwrap();
        service::append(
            &mut guard, "main", head, "EV-singular", Some("tester"),
            EventPayload::ParameterRevised {
                bundle: scenarios::singular_comp_bundle(),
                reason: "近奇异检验".into(),
            },
        )
        .unwrap();
        let head = db::head_seq(&guard, "main").unwrap();
        service::append(
            &mut guard, "main", head, "EV-rename", Some("tester"),
            EventPayload::GateRenamed { bundle: scenarios::rename_bundle() },
        )
        .unwrap();
        db::create_branch(&mut guard, "draft-b", "main", "并排草案").unwrap();
        events_before = service::events_listing(&guard, "main");
        view_before = service::view(&guard, "main", None);
        branches_before = db::list_branches(&guard).unwrap();
        conn = db::open(&path).unwrap();
        drop(guard);
    }

    // 导出归档
    let archive = archive::export_all(&conn).unwrap();
    let raw = service::baseline_source_bytes();
    let archive_json = serde_json::to_string(&archive).unwrap();

    // 删除本地数据库（文件级删除），再导入
    drop(conn);
    std::fs::remove_file(&path).unwrap();
    let mut fresh = db::open(&path).unwrap();
    let parsed: archive::Archive = serde_json::from_str(&archive_json).unwrap();
    archive::import_all(&mut fresh, &parsed, &raw).unwrap();

    // 身份、顺序、父子来源关系保持
    let events_after = service::events_listing(&fresh, "main");
    assert_eq!(events_before, events_after, "事件身份与顺序必须保持");
    let branches_after = db::list_branches(&fresh).unwrap();
    assert_eq!(branches_before, branches_after, "分支谱系必须保持");
    let view_after = service::view(&fresh, "main", None);
    assert_eq!(view_before["heads"], view_after["heads"], "材料头部必须保持");
    assert_eq!(view_before["pops"], view_after["pops"], "派生群体必须保持");
    assert_eq!(
        view_before["candidate_sets"], view_after["candidate_sets"],
        "候选集（含未裁定）必须保持"
    );
    // draft-b 分支事件也在
    assert!(db::has_events(&fresh, "draft-b").unwrap());
}

#[test]
fn failed_import_rolls_back_without_partial_records() {
    let path = temp_db("rollback");
    let mut conn = db::open(&path).unwrap();
    // 构造损坏归档：分支引用不存在的事件负载
    let bad = archive::Archive {
        format: "flow-boundary-chamber/archive@1".into(),
        schema_version: 2,
        case_id: "x".into(),
        branches: vec![archive::ArchiveBranchRow {
            branch: "main".into(),
            forked_from: None,
            forked_seq: 0,
            head_seq: 5, // 声称 5 条事件但只给 1 条
            created_at: "unix:0".into(),
            note: String::new(),
        }],
        events: vec![(
            "main".into(),
            archive::ArchiveEventRow {
                seq: 1,
                event_id: "EV-bad".into(),
                at: "unix:0".into(),
                actor: None,
                payload_json: "{not-json".into(),
            },
        )],
        blobs: vec![],
        case_state: vec![],
    };
    let raw = service::baseline_source_bytes();
    let result = archive::import_all(&mut conn, &bad, &raw);
    assert!(result.is_err(), "损坏归档必须被拒绝");
    // 回滚：不留半份记录
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    let b: i64 = conn.query_row("SELECT COUNT(*) FROM branches", [], |r| r.get(0)).unwrap();
    assert_eq!(b, 0);
}
