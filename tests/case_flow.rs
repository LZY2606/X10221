//! 固定案例的端到端推导断言：结论 + 来源链 + 失效范围。

use flow_lineage::domain::*;
use flow_lineage::{case, scenarios, state};

fn ev(seq: i64, payload: EventPayload) -> BranchEvent {
    BranchEvent {
        event_id: format!("EV-{seq}"),
        seq,
        branch: "main".into(),
        at: format!("unix:{seq}"),
        actor: None,
        payload,
    }
}

fn baseline_events() -> Vec<BranchEvent> {
    vec![ev(1, EventPayload::Imported {
        bundle: case::baseline_bundle(),
        source_blob: "blob:baseline-fcs".into(),
        source_sha256: "test".into(),
    })]
}

fn live_pops<'a>(replay: &'a state::Replay, gate_id: &str) -> Vec<&'a state::DerivedPop> {
    replay.pops.iter().filter(|p| p.gate_id == gate_id && !p.stale).collect()
}

#[test]
fn baseline_six_materials_separately_versioned_and_denominator_pinned() {
    let r = state::replay(&baseline_events(), None);
    assert_eq!(r.state.dataset_head.as_deref(), Some("DS-v1"));
    assert_eq!(r.state.spillover_head.as_deref(), Some("SM-v1"));
    assert_eq!(r.state.compensation_head.as_deref(), Some("COMP-v1"));
    assert_eq!(r.state.transform_head.as_deref(), Some("TF-v1"));
    assert_eq!(r.state.relations_head.as_deref(), Some("REL-v1"));
    assert_eq!(r.state.gate_heads.len(), 4);

    let root = live_pops(&r, "G-ROOT").pop().unwrap();
    assert_eq!(root.numerator, Some(12));
    assert_eq!(root.denominator, Some(12));

    // 细胞门：E07 在门外，E10/E11 极端与不可解析值不影响 FSC/SSC 坐标 → 11 个
    let cells = live_pops(&r, "G-CELLS").pop().unwrap();
    assert_eq!(cells.numerator, Some(11), "E07 碎片排除，其余在细胞门内");
    // 分母钉住根群版本（事件总数快照）
    assert_eq!(cells.denominator, Some(12));
    assert!(cells.parent_pop_id.is_some());

    // FITC 门：E12 恰在 x=200 边界 → 歧义候选，不作为唯一结论
    let fitc = live_pops(&r, "G-FITC");
    let ambig: Vec<_> = fitc.iter().filter(|p| matches!(p.status, state::PopStatus::Ambiguous { .. })).collect();
    assert!(!ambig.is_empty(), "边界点必须保留 in/out 两个候选");
    let ambig = ambig[0];
    assert!(ambig.ambiguous_event_ids.contains(&"E12".to_string()));
    assert!(r.state.candidate_sets.values().any(|s| s.kind == "boundary-membership"));
}

#[test]
fn near_singular_compensation_blocks_without_silent_inverse_and_keeps_candidates() {
    let mut events = baseline_events();
    events.push(ev(2, EventPayload::ParameterRevised {
        bundle: scenarios::singular_comp_bundle(),
        reason: "singular".into(),
    }));
    let r = state::replay(&events, None);
    // 荧光门当前版本阻塞，旧 v1 结果保留但标过期
    let blocked: Vec<_> = r.pops.iter().filter(|p| matches!(p.status, state::PopStatus::Blocked { .. })).collect();
    assert!(!blocked.is_empty());
    let cs = r.state.candidate_sets.values().find(|s| s.kind == "near-singular-compensation").unwrap();
    assert_eq!(cs.candidates.len(), 2);
    assert!(cs.candidates.iter().any(|c| c.candidate_id.ends_with(":reject")));
    assert!(cs.candidates.iter().any(|c| c.candidate_id.ends_with(":regularize")));
    // 判断依据机器可读
    assert!(r.judgments.iter().any(|j| j.rule_id == "comp.near_singular_no_silent_inverse"));
}

#[test]
fn reject_singular_repins_gates_to_v1_and_regularize_is_contested() {
    let mut events = baseline_events();
    events.push(ev(2, EventPayload::ParameterRevised {
        bundle: scenarios::singular_comp_bundle(),
        reason: "singular".into(),
    }));
    let cs_id = flow_lineage::util::stable_id("CS-COMP", "COMP-v2");
    events.push(ev(3, scenarios::decision(&cs_id, Some("COMP-v2:reject"),
        vec!["COMP-v2:regularize".into()], "拒绝近奇异")));
    events.push(ev(4, EventPayload::Recomputed {
        bundle: scenarios::reject_singular_bundle(),
        reason: "回钉 v1".into(),
    }));
    let r = state::replay(&events, None);
    let fitc_live: Vec<_> = r.pops.iter().filter(|p| p.gate_id == "G-FITC" && !p.stale).collect();
    assert!(!fitc_live.is_empty());
    // 回钉后不再有阻塞状态的当前 FITC 群
    assert!(fitc_live.iter().all(|p| !matches!(p.status, state::PopStatus::Blocked { .. })));
}

#[test]
fn rename_keeps_gate_id_and_marks_lineage_stale_without_overwrite() {
    let mut events = baseline_events();
    events.push(ev(2, EventPayload::GateRenamed { bundle: scenarios::rename_bundle() }));
    let r = state::replay(&events, None);
    let head = r.state.gate_heads.get("G-FITC").cloned().unwrap();
    assert_eq!(head, "GV-FITC-v2-rename");
    // 旧派生仍在（不覆盖），且标过期
    let old = r.pops.iter().find(|p| p.gate_version_id == "GV-FITC-v1").unwrap();
    assert!(old.stale);
    assert!(old.stale_reasons.iter().any(|x| x.contains("gate-version-superseded")));
}

#[test]
fn upstream_gate_revision_marks_downstream_stale_but_keeps_results() {
    let mut events = baseline_events();
    // 先裁定边界，使 FITC/PE 有确定结论
    let cs = flow_lineage::util::stable_id("CS-BND", "G-FITC|GV-FITC-v1");
    events.push(ev(2, scenarios::decision(&cs, Some("GV-FITC-v1:include"),
        vec!["GV-FITC-v1:exclude".into()], "计入边界")));
    // 重新派生 FITC/PE（decision 事件 requested gate）
    events.push(ev(3, EventPayload::GateRevised {
        bundle: scenarios::upstream_gate_bundle(),
        reason: "细胞门扩大".into(),
    }));
    let r = state::replay(&events, None);
    let stale_downstream: Vec<_> = r.pops.iter()
        .filter(|p| (p.gate_id == "G-FITC" || p.gate_id == "G-PE") && p.stale)
        .collect();
    assert!(!stale_downstream.is_empty(), "上游修订后下游必须标记过期");
    // 旧结果可继续读取（已解决的派生保留分子与分母；阻塞记录保留原因）
    assert!(stale_downstream.iter().any(|p| p.denominator.is_some() && p.numerator.is_some()));
    // 显式重算后产生新结果，旧结果仍保留
    let mut events2 = events.clone();
    events2.push(ev(4, EventPayload::Recomputed {
        bundle: scenarios::recompute_downstream_after_cells(),
        reason: "跟随新细胞门".into(),
    }));
    let r2 = state::replay(&events2, None);
    let versions: std::collections::BTreeSet<_> =
        r2.pops.iter().filter(|p| p.gate_id == "G-FITC").map(|p| p.gate_version_id.clone()).collect();
    assert!(versions.len() >= 2, "旧版与重算版必须并存");
}

#[test]
fn local_source_patch_invalidates_only_pinned_dataset_results() {
    let mut events = baseline_events();
    events.push(ev(2, EventPayload::SourcePatched {
        bundle: scenarios::local_source_patch_bundle(),
        reason: "E05 重对齐".into(),
    }));
    let r = state::replay(&events, None);
    // 门仍钉 DS-v1：其派生因 dataset 版本链被取代而标过期
    assert!(r.pops.iter().any(|p| p.stale_reasons.iter().any(|x| x.starts_with("dataset-superseded-by:DS-v2"))));
    // 重钉 DS-v2 后生成新结果，旧 DS-v1 结果保留
    let mut e2 = events.clone();
    e2.push(ev(3, EventPayload::Recomputed {
        bundle: scenarios::repin_dataset_bundle(),
        reason: "跟随 DS-v2".into(),
    }));
    let r2 = state::replay(&e2, None);
    assert!(r2.pops.iter().any(|p| p.dataset_version == "DS-v2" && !p.stale));
    assert!(r2.pops.iter().any(|p| p.dataset_version == "DS-v1" && p.stale));
}

#[test]
fn retraction_restores_prior_dependencies_and_conclusion() {
    let mut events = baseline_events();
    events.push(ev(2, EventPayload::GateRevised {
        bundle: scenarios::upstream_gate_bundle(),
        reason: "细胞门扩大".into(),
    }));
    events.push(ev(3, scenarios::retraction("EV-2", "撤销上游修订")));
    let r = state::replay(&events, None);
    // 回到撤销后视图：头部仍是 v1 细胞门，且无过期
    assert_eq!(r.state.gate_heads.get("G-CELLS"), Some(&"GV-CELLS-v1".to_string()));
    assert!(r.pops.iter().all(|p| !p.stale));
}

#[test]
fn history_replay_does_not_overwrite_with_latest() {
    let mut events = baseline_events();
    events.push(ev(2, EventPayload::GateRenamed { bundle: scenarios::rename_bundle() }));
    let r1 = state::replay(&events, Some(1));
    assert_eq!(r1.state.gate_heads.get("G-FITC"), Some(&"GV-FITC-v1".to_string()));
    let r2 = state::replay(&events, None);
    assert_eq!(r2.state.gate_heads.get("G-FITC"), Some(&"GV-FITC-v2-rename".to_string()));
}

#[test]
fn extreme_and_unconvertible_values_are_kept_with_diagnostics() {
    let r = state::replay(&baseline_events(), None);
    let ds = r.state.datasets.get("DS-v1").unwrap();
    let e10 = ds.events.iter().find(|e| e.event_id == "E10").unwrap();
    let fitc = e10.values.iter().find(|v| v.channel == "FITC").unwrap();
    assert_eq!(fitc.raw, "300000");
    assert!(fitc.diagnostics.iter().any(|d| d.contains("overflow")));
    let e11 = ds.events.iter().find(|e| e.event_id == "E11").unwrap();
    let pe = e11.values.iter().find(|v| v.channel == "PE").unwrap();
    assert_eq!(pe.raw, "NA");
    assert!(pe.numeric.is_none());
    assert!(pe.diagnostics.iter().any(|d| d.contains("unconvertible")));
}
