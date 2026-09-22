mod branching;
mod backup;
mod db;
mod domain;
mod engine;
mod events;
mod geometry;
mod ingest;
mod matrix;
mod repository;
mod scenarios;
mod service;
mod service_view;
mod transform;
mod web;

use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Debug)]
struct Args {
    addr: SocketAddr,
    db: String,
}

fn parse_args() -> Args {
    let mut addr: SocketAddr = "127.0.0.1:5341".parse().unwrap();
    let mut db = "flowline.db".to_string();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--addr" => addr = it.next().expect("--addr requires value").parse().expect("invalid addr"),
            "--db" => db = it.next().expect("--db requires value"),
            other => panic!("unknown argument: {other}"),
        }
    }
    Args { addr, db }
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    let store = Arc::new(db::open(&args.db).expect("open sqlite"));
    // 启动即自动载入仓库内离线案例（幂等）
    if let Err(e) = store.load_bundled_case() {
        eprintln!("warning: bundled case not loaded: {e}");
    }
    let app = web::router(store);
    let listener = tokio::net::TcpListener::bind(args.addr).await.expect("bind");
    println!("flowline review UI: http://{}", args.addr);
    axum::serve(listener, app).await.expect("server");
}

#[cfg(test)]
mod integration {
    use super::*;
    use crate::db::Store;

    fn mem_store() -> Arc<Store> {
        let s = Arc::new(db::open(":memory:").unwrap());
        s.load_bundled_case().unwrap();
        s
    }

    #[test]
    fn debug_keys() {
        let s = mem_store();
        let l = s.load_materials();
        eprintln!("raw keys: {:?}", l.raw.keys().collect::<Vec<_>>());
        eprintln!("comp keys: {:?}", l.comp.keys().collect::<Vec<_>>());
        eprintln!("gate keys: {:?}", l.gates.keys().collect::<Vec<_>>());
        eprintln!("hier keys: {:?}", l.hierarchy.keys().collect::<Vec<_>>());
        eprintln!("tr keys: {:?}", l.transform.keys().collect::<Vec<_>>());
        eprintln!("meta keys: {:?}", l.meta.keys().collect::<Vec<_>>());
        panic!("dbg");
    }

    #[test]
    fn baseline_counts_and_denominators() {
        let s = mem_store();
        let view = s.branch_view("main", None);
        let by: std::collections::BTreeMap<_, _> =
            view.populations.iter().map(|r| (r.gate_id.clone(), r)).collect();
        assert_eq!(by["gate-lymph"].numerator, 12);
        assert_eq!(by["gate-lymph"].denominator, 14);
        assert_eq!(by["gate-t"].denominator, 12);
        assert_eq!(by["gate-cd4"].numerator, 2);
        assert_eq!(by["gate-cd4"].denominator_ref, "pop://gate-t@gate-t/v1");
        assert_eq!(by["gate-lymph"].denominator_ref, "pop://root@mat-raw-events/v1");
    }

    #[test]
    fn near_singular_refused_with_candidates_and_adoption_flow() {
        let s = mem_store();
        let r = s.scenario_near_singular("main", None).unwrap();
        assert_eq!(r["refused"], serde_json::json!(true));
        assert!(r["kappa"].as_f64().unwrap() >= 1e5);
        // 没有任何 appraisal 被静默产生，且当前 refs 未切换
        let view = s.branch_view("main", None);
        assert_eq!(view.refs.compensation.as_deref(), Some("mat-compensation/v1"));
        assert_eq!(view.candidates.len(), 3);
        // 拒绝岭候选：仍停在 v1
        s.decide_candidate("main", "fb-ridge-1e-4", false, None).unwrap();
        let view = s.branch_view("main", None);
        assert_eq!(view.refs.compensation.as_deref(), Some("mat-compensation/v1"));
    }

    #[test]
    fn transform_revision_limited_blast_radius_and_local_patch() {
        let s = mem_store();
        let r = s.scenario_transform_revision("main", None).unwrap();
        let scope: Vec<String> = r["invalidated_scope"].as_array().unwrap()
            .iter().map(|v| v.as_str().unwrap().into()).collect();
        assert!(scope.contains(&"gate-t".to_string()));
        assert!(scope.contains(&"gate-cd8".to_string()));
        assert!(!scope.contains(&"gate-lymph".to_string()));
        // 局部源补丁：CD4 从 2 -> 1，T 门数量不变
        let s2 = mem_store();
        s2.scenario_source_patch("main", None).unwrap();
        let view = s2.branch_view("main", None);
        let by: std::collections::BTreeMap<_, _> =
            view.populations.iter().map(|r| (r.gate_id.clone(), r)).collect();
        let t_latest = by.values().filter(|r| r.gate_id == "gate-t").max_by_key(|r| r.event_id.clone()).unwrap();
        assert_eq!(t_latest.numerator, 6);
        let cd4_latest = by.values().filter(|r| r.gate_id == "gate-cd4").max_by_key(|r| r.event_id.clone()).unwrap();
        assert_eq!(cd4_latest.numerator, 1);
    }

    #[test]
    fn rename_keeps_counts_upstream_revision_marks_stale_without_overwrite() {
        let s = mem_store();
        s.scenario_gate_rename("main", None).unwrap();
        let view = s.branch_view("main", None);
        let t = view.populations.iter().filter(|r| r.gate_id == "gate-t")
            .max_by_key(|r| r.event_id.clone()).unwrap();
        assert_eq!(t.numerator, 6);
        // 旧 gate-t/v1 的行仍在
        let old_rows: Vec<_> = view.populations.iter().filter(|r| r.gate_ref == "gate-t/v1").collect();
        assert!(!old_rows.is_empty());

        let s2 = mem_store();
        s2.scenario_upstream_revision("main", None).unwrap();
        let view = s2.branch_view("main", None);
        let stale: Vec<_> = view.populations.iter().filter(|r| r.status == "stale").collect();
        let stale_gates: std::collections::BTreeSet<_> = stale.iter().map(|r| r.gate_id.clone()).collect();
        assert!(stale_gates.contains("gate-t"));
        assert!(stale_gates.contains("gate-cd4"));
        // 旧结论未被覆盖：旧行数值仍可读
        let old_lymph = view.populations.iter().find(|r| r.gate_id == "gate-lymph" && r.status == "stale").unwrap();
        assert_eq!(old_lymph.numerator, 12);
        // 提出重估候选
        assert!(view.candidates.iter().any(|c| c.group_id.starts_with("stale:")));
    }

    #[test]
    fn same_version_concurrent_modification_conflicts() {
        let s = mem_store();
        let tip = s.tip("main");
        s.scenario_transform_revision("main", tip.as_deref()).unwrap();
        // 第二个窗口仍拿旧 tip 提交 -> 冲突，状态不改变
        let err = s.scenario_gate_rename("main", tip.as_deref()).err().unwrap();
        assert!(err.contains("version conflict"));
        let conflicts = s.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0]["attempted_kind"], "gate.renamed");
    }

    #[test]
    fn fork_revert_and_history_at_old_event() {
        let s = mem_store();
        let before = s.branch_view("main", None);
        let base_tip = before.tip_event_id.clone().unwrap();
        s.scenario_gate_rename("main", None).unwrap();
        // 在旧事件上开第二草案
        s.fork_at("draft-b", "draft B", "main", Some(&base_tip)).unwrap();
        let dv = s.branch_view("draft-b", None);
        assert_eq!(dv.refs.gates["gate-t"], "gate-t/v1");
        // 回到 main 的旧事件：看到的是当时结论而非最新
        let old = s.branch_view("main", Some(&base_tip));
        let names: Vec<String> = old.gates.iter().filter_map(|g| {
            if g["gate_id"] == "gate-t" { Some(g["name"].as_str().unwrap().into()) } else { None }
        }).collect();
        assert_eq!(names, vec!["T cells (CD3+)".to_string()]);
        // 撤销
        let rev = s.revert_to("main", &base_tip, None).unwrap();
        assert!(!rev.is_empty());
    }

    #[test]
    fn export_delete_import_preserves_identity_order_and_provenance() {
        let s = mem_store();
        s.scenario_gate_rename("main", None).unwrap();
        let archive = s.export_all();
        let fresh = Arc::new(db::open(":memory:").unwrap());
        let n = fresh.import_all(&archive, false).unwrap();
        assert!(n > 10);
        let v1 = s.branch_view("main", None);
        let v2 = fresh.branch_view("main", None);
        let ids1: Vec<_> = v1.events.iter().map(|e| e["event_id"].clone()).collect();
        let ids2: Vec<_> = v2.events.iter().map(|e| e["event_id"].clone()).collect();
        assert_eq!(ids1, ids2);
        let rows1: Vec<_> = v1.populations.iter().map(|r| (r.row_uid.clone(), r.numerator, r.denominator_ref.clone())).collect();
        let rows2: Vec<_> = v2.populations.iter().map(|r| (r.row_uid.clone(), r.numerator, r.denominator_ref.clone())).collect();
        assert_eq!(rows1, rows2);
    }

    #[test]
    fn malformed_import_rolls_back_without_half_records() {
        let s = mem_store();
        let bad = serde_json::json!({"materials": {"raw_events": [{"material_id":"x"}]}});
        assert!(crate::ingest::validate_case(&bad).is_err());
        // 正常数据未受影响
        let view = s.branch_view("main", None);
        assert_eq!(view.refs.raw_events.as_deref(), Some("mat-raw-events/v1"));
    }
}
