use flowline::app::Service;

#[test]
fn smoke_case() {
    let svc = Service::open_memory().unwrap();
    let sum = svc.load_builtin_case().unwrap();
    println!("summary={sum:?}");
    let st = svc.state("main").unwrap();
    for p in &st.populations {
        println!("pop {} rev={} count={} pct={:.2} denom={}@{} inside={:?} boundary={:?} unknown={:?}",
            p.gate_id, &p.gate_rev[..8], p.count, p.percent,
            p.denominator.parent_gate_id, &p.denominator.parent_gate_rev[..8],
            p.inside_event_ids, p.boundary_event_ids, p.unknown_event_ids);
    }
    println!("stale={:?}", st.stale.iter().map(|m| m.gate_id.clone()).collect::<Vec<_>>());
}

#[test]
fn smoke_coords() {
    let svc = Service::open_memory().unwrap();
    svc.load_builtin_case().unwrap();
    let v = svc.view_scatter("main", "gate#cd3").unwrap();
    for p in v.get("points").unwrap().as_array().unwrap() {
        println!("{} x={:?} y={:?} m={}", p["event_id"].as_str().unwrap(),
            p["x"].as_f64(), p["y"].as_f64(), p["membership"].as_str().unwrap());
    }
}
