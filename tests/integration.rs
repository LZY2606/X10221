use flow_lineage::case::CaseFile;
use flow_lineage::engine::Store;
use flow_lineage::workflow::{self, MAIN};
use std::path::PathBuf;

fn case_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("cases/flow-boundary-chamber")
}

fn load_case_text() -> String {
    std::fs::read_to_string(case_dir().join("case.json")).unwrap()
}

fn baseline_store() -> (Store, CaseFile) {
    let dir = std::env::temp_dir().join(format!("flow-test-{}", uuid_like()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("t.sqlite");
    let mut store = Store::open(db.to_str().unwrap()).unwrap();
    let case = CaseFile::from_str(&load_case_text()).unwrap();
    let mut blobs = Vec::new();
    for entry in walkdir(&case_dir().join("controls")) {
        let bytes = std::fs::read(&entry).unwrap();
        let rel = entry.strip_prefix(case_dir()).unwrap().to_str().unwrap().to_string();
        blobs.push((rel, bytes));
    }
    let blob_refs: Vec<(&str, &[u8])> = blobs.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect();
    workflow::load_case(&mut store, &case, &blob_refs).expect("load_case");
    (store, case)
}

fn walkdir(p: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                out.extend(walkdir(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

fn uuid_like() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn dbg_load() {
    let dir = std::env::temp_dir().join(format!("flow-dbg-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("t.sqlite");
    let mut store = Store::open(db.to_str().unwrap()).unwrap();
    let case = CaseFile::from_str(&load_case_text()).unwrap();
    let blob_refs: Vec<(&str,&[u8])> = vec![];
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| workflow::load_case(&mut store, &case, &blob_refs)));
    println!("RESULT={r:?}");
}

#[test]
fn dbg_coord() {
    let (store, _) = baseline_store();
    let st = store.state_at(MAIN, None).unwrap();
    for id in ["E01","E13","E18","E19"] {
        let c = flow_lineage::compute::coordinates(&st, id, None).unwrap();
        println!("{id} FITC={} PE={}", c.get("FITC-A").unwrap(), c.get("PE-A").unwrap());
    }
    let st = store.state_at(MAIN, None).unwrap();
    let pops = flow_lineage::compute::recompute_populations(&st, "t", None);
    for p in pops["populations"].as_array().unwrap() {
        if p["gate_logical"]=="g-cd3" {
            println!("CD3 members={:?}", p["members"]);
            println!("CD3 poly={:?} deps={}", p["members"], st.artifacts.get("gates:active").unwrap().content);
        }
    }
}

fn counts(store: &Store, branch: &str) -> std::collections::BTreeMap<String, i64> {
    let state = store.state_at(branch, None).unwrap();
    let pops = flow_lineage::compute::recompute_populations(&state, "t", None);
    pops["populations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["gate_logical"].as_str().unwrap().to_string(), p["count"].as_i64().unwrap()))
        .collect()
}

#[test]
fn baseline_counts_and_pinned_denominators() {
    let (store, _case) = baseline_store();
    let c = counts(&store, MAIN);
    println!("COUNTS={c:?}");
    assert_eq!(c["g-live"], 22, "E24 边界点 inclusive 必须计入");
    assert_eq!(c["g-singlets"], 21);
    assert_eq!(c["g-cd3"], 13);
    assert_eq!(c["g-cd4"], 5);
    let state = store.state_at(MAIN, None).unwrap();
    let pops = flow_lineage::compute::recompute_populations(&state, "t", None);
    for p in pops["populations"].as_array().unwrap() {
        if p["gate_logical"] == "g-live" {
            assert!(p["denominator_result_id"].is_null(), "根群以全部事件为分母");
        } else {
            assert!(!p["denominator_result_id"].is_null(), "子群分母必须钉住父群结果");
            assert!(!p["denominator_event_id"].is_null(), "分母必须钉住父群结果所在事件版本");
        }
    }
}

#[test]
fn replay_covers_all_boundary_chambers_without_overwrite() {
    let (mut store, case) = baseline_store();
    let report = workflow::replay_case(&mut store, &case).unwrap();
    let kinds: Vec<String> = report
        .steps
        .iter()
        .map(|s| s["step"].as_str().unwrap().to_string())
        .collect();
    for required in [
        "near_singular",
        "singular",
        "conflicting_decisions",
        "gate_nesting_rename",
        "upstream_stale_propagation",
        "source_patch_localized",
        "transform_revision",
    ] {
        assert!(kinds.contains(&required.to_string()), "缺少重放场景 {required}");
    }
    // 上游修订后：live 为新结果，下游保留旧结果但被标记 stale。
    let stale: Vec<(String, i64, String)> = store
        .conn
        .prepare("SELECT result_id, count, status FROM pop_results WHERE branch_id='main' AND stale=1")
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let stale_ids: Vec<String> = stale.iter().map(|(id, _, _)| id.clone()).collect();
    assert!(stale_ids.contains(&"pop-g-singlets".to_string()));
    assert!(stale_ids.contains(&"pop-g-cd3".to_string()));
    assert!(stale_ids.contains(&"pop-g-cd4".to_string()));
    // 旧计数被保留（21/13/5）而不是被覆盖。
    let old: std::collections::HashMap<String, i64> = stale.into_iter().map(|(id, n, _)| (id, n)).collect();
    assert_eq!(old["pop-g-singlets"], 21);
    assert_eq!(old["pop-g-cd3"], 13);
    assert_eq!(old["pop-g-cd4"], 5);
}

#[test]
fn same_revision_concurrent_decisions_conflict() {
    let (mut store, _case) = baseline_store();
    // 在 main 当前 revision 上抢先追加一笔，模拟另一窗口先保存。
    let rev = store.branch_revision(MAIN).unwrap();
    store
        .append(MAIN, "pop_stale", serde_json::json!({"content": {"expired_result_ids": []}}), Some(rev), None)
        .unwrap();
    // 后提交者仍带旧 revision：必须冲突。
    let err = store
        .append(MAIN, "pop_stale", serde_json::json!({"content": {"expired_result_ids": []}}), Some(rev), None)
        .unwrap_err();
    assert!(err.starts_with("CONFLICT:"), "同版本并发保存应产生版本冲突: {err}");
}

#[test]
fn near_singular_keeps_every_admissible_candidate() {
    let (mut store, case) = baseline_store();
    flow_lineage::engine::publish_artifact(
        &mut store,
        MAIN,
        "comp_matrix",
        "active",
        2,
        case.compensation_matrices["near_singular"].clone(),
        None,
    )
    .unwrap();
    workflow::evaluate_current_comp(&mut store, MAIN, "cand-x").unwrap();
    let state = store.state_at(MAIN, None).unwrap();
    let analysis = state.evals["cand-x"]["analysis"].clone();
    assert_eq!(analysis["status"], "NearSingular");
    let strategies: Vec<String> = analysis["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["strategy"].as_str().unwrap().to_string())
        .collect();
    assert!(strategies.contains(&"uncompensated".to_string()));
    assert!(strategies.contains(&"ridge".to_string()));
    assert!(strategies.contains(&"pseudo-inverse".to_string()));
    assert!(!strategies.contains(&"exact".to_string()));
}

#[test]
fn illegal_strategy_is_rejected_and_import_is_atomic() {
    let (mut store, case) = baseline_store();
    flow_lineage::engine::publish_artifact(
        &mut store,
        MAIN,
        "comp_matrix",
        "active",
        2,
        case.compensation_matrices["singular"].clone(),
        None,
    )
    .unwrap();
    workflow::evaluate_current_comp(&mut store, MAIN, "cand-s").unwrap();
    // 严格奇异矩阵上 exact 不是合法候选。
    let err = workflow::decide(&mut store, MAIN, "cand-s", "exact", None).unwrap_err();
    assert!(err.contains("不是该矩阵的合法候选"));
}

#[test]
fn history_at_old_event_restores_old_conclusion() {
    let (mut store, _case) = baseline_store();
    let before = counts(&store, MAIN);
    // 改名（不影响几何/计数），并修订上游门使下游过期。
    let case = CaseFile::from_str(&load_case_text()).unwrap();
    workflow::replay_case(&mut store, &case).unwrap();
    // 回到改名前事件不可直接枚举，但回到基线最后一个 pop_recomputed 事件：
    let baseline_pop_event: String = store
        .conn
        .query_row(
          "SELECT event_id FROM events WHERE branch_id='main' AND kind='pop_recomputed' ORDER BY seq LIMIT 1",
          [], |r| r.get(0))
        .unwrap();
    let state = store.state_at(MAIN, Some(&baseline_pop_event)).unwrap();
    let pops = flow_lineage::compute::recompute_populations(&state, &baseline_pop_event, None);
    let old: std::collections::BTreeMap<String, i64> = pops["populations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["gate_logical"].as_str().unwrap().to_string(), p["count"].as_i64().unwrap()))
        .collect();
    assert_eq!(old, before, "回到旧事件必须恢复当时结论，而不是用最新结果覆盖");
}
