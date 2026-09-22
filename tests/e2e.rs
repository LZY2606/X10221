//! 端到端：重放仓库案例包，断言结论 + 来源链 + 失效范围 + 时间旅行 + 导出/删除/导入一致。
use flowlineage::case::{discover_case_dir, load_case, replay};
use flowlineage::{Service, Store};
use std::sync::Arc;
use tempfile::TempDir;

fn fresh_service() -> (TempDir, Arc<Service>) {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("e2e.db");
    let store = Arc::new(Store::open(db.to_str().unwrap()).unwrap());
    (dir, Arc::new(Service::new(store)))
}

#[test]
fn full_case_replay_assertions() {
    let cwd = std::env::current_dir().unwrap();
    let case_dir = discover_case_dir(&cwd).expect("找到 cases/flow-boundary-chamber");
    let (def, root) = load_case(&case_dir).unwrap();

    let (_dir, svc) = fresh_service();
    let pid = "proj-boundary";
    let reports = replay(&svc, pid, &def, &root).expect("案例重放应全部通过");
    assert!(reports.len() >= 20, "至少 20 个重放步骤，实际 {}", reports.len());

    // 基线群数（main 头状态：cd3 已修订为 r2）
    let main = svc.view(pid, "main", None).unwrap();
    assert_eq!(main.populations["gate-lymph"].count, 23, "23 淋巴（含边界），碎屑与 3 极端事件被排除");
    assert!(main.populations["gate-lymph"].member_indices.contains(&22), "边界点仍是淋巴");
    assert!(main.compensation.is_some());

    // 近奇异后 main 的补偿被阻断（没有自动逆）
    let comp = main.compensation.clone().unwrap();
    assert!(comp.verdict.near_singular);
    assert!(comp.blocked, "main 上近奇异矩阵未裁定必须阻断");
    assert_eq!(comp.verdict.candidates.len(), 3);

    // 上游 CD3 修订后：CD8 标记过期且分母仍钉住 gate-cd3@1
    let cd8 = main.populations.get("gate-cd8").expect("cd8 群保留");
    assert_eq!(cd8.status, "stale_pending_revision");
    assert!(
        cd8.denominator.parent_revision_sig.contains("gate-cd3@1"),
        "分母必须钉住旧父群版本: {}",
        cd8.denominator.parent_revision_sig
    );
    // 旧结论本身仍可读
    assert!(cd8.computed_at_seq >= 11);

    // 改名：身份不变，显示名变化
    let cd3 = main.gates.get("gate-cd3").unwrap();
    assert_eq!(cd3.name, "T cells (CD3)");
    assert!(cd3.revisions.contains(&1) && cd3.revisions.contains(&2), "门修订历史保留");

    // 冲突留档
    assert!(main.conflicts.iter().any(|c| c.branch == "main" && c.actual_head > c.expected_head),
        "同版本并发冲突必须留档");

    // 分支：接受候选 vs 否定
    let ridge = svc.view(pid, "review-ridge", None).unwrap();
    // 该分支随后撤销了接受 → 恢复阻断
    assert!(ridge.compensation.clone().unwrap().blocked, "撤销后恢复阻断");
    let reject = svc.view(pid, "review-reject", None).unwrap();
    let rc = reject.compensation.unwrap();
    assert!(rc.blocked);
    assert!(matches!(
        rc.active_decision.as_ref().map(|d| d.outcome),
        Some(flowlineage::domain::model::DecisionOutcome::Rejected)
    ));

    // 精确奇异探针
    let probe = svc.view(pid, "singular-probe", None).unwrap();
    let probe_sp = probe.spillovers.get("sp-singular-probe").unwrap();
    let verdict = probe_sp.verdict.as_ref().unwrap();
    assert!(verdict.singular);
    assert!(verdict.inverse.is_none());

    // 时间旅行：seq=12 时 cd3 仍是 r1，cd8 fresh
    let hist = svc.view(pid, "main", Some(12)).unwrap();
    assert_eq!(hist.gates["gate-cd3"].revision, 1);
    assert_eq!(hist.populations["gate-cd8"].status, "fresh");
    assert_eq!(hist.populations["gate-cd3"].count, 13, "基线下 13 个 CD3（含边界）");
    assert_eq!(hist.populations["gate-cd8"].count, 5);
    assert_eq!(hist.populations["gate-cd8"].denominator.parent_count, 13);

    // 来源链：样本原始字节仍可读取，诊断保留
    let bytes = svc.store.load_blob(pid, "raw-sample-1").unwrap();
    assert!(!bytes.is_empty());
    let fcs = svc.fcs_of(pid, "raw-sample-1").unwrap();
    assert!(fcs.diagnostics.iter().any(|d| d.code == "NAN_VALUE"));
}

#[test]
fn optimistic_concurrency_rejects_second_writer() {
    let (_d, svc) = fresh_service();
    svc.create_project("p", "并发").unwrap(); // head=1
    // 两个窗口都基于 head=1 提交变换
    use flowlineage::domain::math::TransformSpec;
    let first = svc.set_transform("p", "main", Some(1), "FITC-A", 1, TransformSpec::Asinh { cofactor: 150.0 });
    assert!(first.is_ok());
    let second = svc.set_transform("p", "main", Some(1), "FITC-A", 2, TransformSpec::Asinh { cofactor: 250.0 });
    let err = second.expect_err("第二个同版本提交必须失败");
    assert_eq!(err.status, 409);
}

#[test]
fn export_delete_import_preserves_identity_order_and_provenance() {
    let cwd = std::env::current_dir().unwrap();
    let case_dir = discover_case_dir(&cwd).unwrap();
    let (def, root) = load_case(&case_dir).unwrap();
    let dir1 = TempDir::new().unwrap();
    let db1 = dir1.path().join("a.db");
    {
        let store = Arc::new(Store::open(db1.to_str().unwrap()).unwrap());
        let svc = Service::new(store.clone());
        replay(&svc, "proj-x", &def, &root).unwrap();
    }
    let bundle = {
        let store = Store::open(db1.to_str().unwrap()).unwrap();
        store.export_bundle("test").unwrap()
    };
    // 删除“本地数据库”，导入到全新库
    let dir2 = TempDir::new().unwrap();
    let db2 = dir2.path().join("b.db");
    Store::import_bundle(db2.to_str().unwrap(), &bundle).unwrap();
    let store2 = Arc::new(Store::open(db2.to_str().unwrap()).unwrap());
    let svc2 = Service::new(store2);
    let events_before = Store::open(db1.to_str().unwrap()).unwrap().load_events("proj-x").unwrap();
    let events_after = svc2.store.load_events("proj-x").unwrap();
    assert_eq!(events_before.len(), events_after.len());
    for (a, b) in events_before.iter().zip(events_after.iter()) {
        assert_eq!(a.seq, b.seq, "顺序保持");
        assert_eq!(a.event_id, b.event_id, "稳定身份保持");
        assert_eq!(a.branch, b.branch, "分支保持");
    }
    let view = svc2.view("proj-x", "main", None).unwrap();
    assert_eq!(view.populations["gate-lymph"].count, 23);
    assert!(view.conflicts.iter().any(|c| c.branch == "main"));
    // 原始字节恢复
    let blob = svc2.store.load_blob("proj-x", "raw-sample-1").unwrap();
    assert!(blob.len() > 100);
}
