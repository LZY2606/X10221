use flowline::app::commands::*;
use flowline::app::Service;

#[test]
fn flow2() {
    let svc = Service::open_memory().unwrap();
    svc.load_builtin_case().unwrap();
    let h1 = svc.state("main").unwrap().head_hash;

    // 门改名：身份不变
    let r = svc.revise_gate(&ReviseGate {
        branch: "main".into(), base_head_hash: Some(h1.clone()),
        gate_id: "gate#cd3".into(), new_name: Some("CD3 positive".into()),
        polygon: None, new_parent: None, note: Some("改名测试".into()), actor: None,
    }).unwrap();
    println!("rename: {r}");
    let st = svc.state("main").unwrap();
    let g = st.gate("gate#cd3").unwrap();
    assert_eq!(g.name, "CD3 positive");
    assert_eq!(g.ordinal, 2);
    // cd4/cd8 的父引用仍然有效
    assert_eq!(st.gate("gate#cd4").unwrap().parent_gate_id.as_deref(), Some("gate#cd3"));

    // 修订 cells 多边形（只影响局部依赖：所有后代失效）
    let h2 = st.head_hash.clone();
    let r = svc.revise_gate(&ReviseGate {
        branch: "main".into(), base_head_hash: Some(h2),
        gate_id: "gate#cells".into(), new_name: None,
        polygon: Some(vec![PointIn{x:100.0,y:100.0},PointIn{x:900.0,y:100.0},
                          PointIn{x:900.0,y:800.0},PointIn{x:100.0,y:800.0}]),
        new_parent: None, note: Some("收紧 cells".into()), actor: None,
    }).unwrap();
    println!("revise cells: {r}");
    let st = svc.state("main").unwrap();
    let scope: Vec<_> = st.stale.iter().map(|m| m.gate_id.clone()).collect();
    println!("stale scope: {scope:?}");
    assert!(scope.contains(&"gate#cd3".to_string()));
    assert!(scope.contains(&"gate#cd4".to_string()));
    assert!(scope.contains(&"gate#cd8".to_string()));

    // 旧结果仍在（未被覆盖）
    let old_pops = svc.state_at("main", 1).unwrap();
    assert_eq!(old_pops.populations.iter().find(|p| p.gate_id=="gate#cd3").unwrap().count(), 6);

    // 重新派生
    let h3 = st.head_hash.clone();
    let d = svc.derive_now("main", Some(&h3), "reviewer").unwrap();
    println!("derive: {d}");
    let st = svc.state("main").unwrap();
    let cells = st.population_for_latest("gate#cells").unwrap();
    println!("new cells count={} (was 13)", cells.count);
    assert!(cells.count < 13);

    // 边界点裁定（在新 cells 里事件11仍在吗？事件11 FSC550 SSC700 在内）
    let h4 = st.head_hash.clone();
    let b = svc.adjudicate_boundary(&BoundaryCall {
        branch: "main".into(), base_head_hash: Some(h4),
        gate_id: "gate#cd3".into(), event_id: "event#11".into(),
        call: "include".into(), note: Some("边界计入".into()), actor: None,
    }).unwrap();
    println!("boundary: {b}");

    // 分叉草案
    let st = svc.state("main").unwrap();
    let h5 = st.head_hash.clone();
    let f = svc.fork_draft(&ForkDraft {
        branch: "main".into(), base_head_hash: Some(h5.clone()),
        new_branch: "draft/exclude11".into(), label: "边界排除分支".into(), actor: None,
    }).unwrap();
    println!("fork: {f}");

    // 并排差异
    let diff = svc.view_diff("main", "draft/exclude11").unwrap();
    println!("diff rows={}", diff.get("rows").unwrap().as_array().unwrap().len());

    // 在草案上把边界改成排除（需要先撤销 main 的包含裁定？草案复制了包含裁定；
    // 这里直接测撤销）
    let u = svc.undo_decision(&UndoCall {
        branch: "draft/exclude11".into(), base_head_hash: None,
        adjudication_id: "adj#boundary#gate_cd3#event_11".into(), actor: None,
    }).unwrap();
    println!("undo: {u}");
    let dst = svc.state("draft/exclude11").unwrap();
    assert!(dst.undone.contains("adj#boundary#gate_cd3#event_11"));
    let cd3 = dst.population_for_latest("gate#cd3").unwrap();
    println!("draft cd3 count after undo={}", cd3.count);
    assert!(!cd3.inside_event_ids.contains(&"event#11".to_string()));

    // main 上的包含裁定不受影响
    let mst = svc.state("main").unwrap();
    let cd3m = mst.population_for_latest("gate#cd3").unwrap();
    assert!(cd3m.inside_event_ids.contains(&"event#11".to_string()));

    // 导出 → 新库导入 → 身份/顺序保持
    let bundle = svc.export_bundle().unwrap();
    let svc2 = Service::open_memory().unwrap();
    let n = svc2.import_bundle(&bundle).unwrap();
    println!("reimported events={n}");
    let st1 = svc.state("main").unwrap();
    let st2 = svc2.state("main").unwrap();
    assert_eq!(st1.event_order, st2.event_order);
    assert_eq!(st1.head_hash, st2.head_hash);
    for (a,b) in st1.events.iter().zip(&st2.events) {
        assert_eq!(a.event_id, b.event_id);
        assert_eq!(a.seq, b.seq);
        assert_eq!(a.hash, b.hash);
    }
    let d1 = svc.view_diff("main","draft/exclude11").unwrap();
    let d2 = svc2.view_diff("main","draft/exclude11").unwrap();
    assert_eq!(d1, d2);
    println!("ALL OK");
}
