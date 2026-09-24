use flowline::app::Service;
use flowline::app::commands::*;

#[test]
fn flow() {
    let svc = Service::open_memory().unwrap();
    let sum = svc.load_builtin_case().unwrap();
    let h1 = svc.state("main").unwrap().head_hash;
    println!("loaded status={} candidates={:?}", sum.matrix_status, sum.matrix_candidates);

    // 1) 参数修订：近奇异矩阵
    let r = svc.revise_compensation("main", Some(&h1), "matrix#panel",
        flowline::casepack::near_singular_spillover(), "近奇异测试", "reviewer").unwrap();
    println!("near: {r}");
    let st = svc.state("main").unwrap();
    println!("stale after near: {:?}", st.stale.iter().map(|m| (&m.gate_id, &m.because_kind)).collect::<Vec<_>>());
    println!("active matrix chosen: {:?}", st.active_matrix().map(|m| m.chosen_candidate.clone()));
    let h2 = st.head_hash.clone();

    // 默默求逆派生必须失败
    let err = svc.derive_now("main", Some(&h2), "reviewer").unwrap_err();
    println!("derive blocked: {} {}", err.code, err.message);

    // 错误候选必须拒绝
    let bad = svc.adjudicate_compensation(&AdjudicateComp {
        branch: "main".into(), base_head_hash: Some(h2.clone()), matrix_id: "matrix#panel".into(),
        decision: "accept".into(), candidate: Some("nonsense".into()), note: None, actor: None,
    }).unwrap_err();
    println!("bad candidate: {}", bad.code);

    // 显式接受 ridge_regularized
    let ok = svc.adjudicate_compensation(&AdjudicateComp {
        branch: "main".into(), base_head_hash: Some(h2.clone()), matrix_id: "matrix#panel".into(),
        decision: "accept".into(), candidate: Some("ridge_regularized".into()), note: Some("正则化候选".into()), actor: None,
    }).unwrap();
    println!("accepted: {ok}");
    let st = svc.state("main").unwrap();
    println!("chosen after adj: {:?}", st.active_matrix().unwrap().chosen_candidate);
    let h3 = st.head_hash.clone();
    let d = svc.derive_now("main", Some(&h3), "reviewer").unwrap();
    println!("derive: {d}");

    // 2) 同版本并发：用过期 h3 再提交一次裁定，必须 VERSION_CONFLICT
    let conflict = svc.adjudicate_compensation(&AdjudicateComp {
        branch: "main".into(), base_head_hash: Some(h3.clone()), matrix_id: "matrix#panel".into(),
        decision: "reject".into(), candidate: None, note: None, actor: Some("late-window".into()),
    }).unwrap_err();
    println!("conflict: {}", conflict.code);
}
