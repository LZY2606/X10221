fn main() {
    let m = vec![
        vec![1.0, 0.02, 0.01, 0.005],
        vec![0.10, 1.0, 0.03, 0.01],
        vec![0.02, 0.05, 1.0, 0.02],
        vec![0.01, 0.01, 0.04, 1.0],
    ];
    let v = flowlineage::domain::math::assess_matrix(&m);
    println!("cond={:?}", v.condition_number);
    if let Some(inv)=&v.inverse {
        // T CD4 row: FSC,SSC,FITC,PE,APC,PerCP -> ordered FITC,PE,APC,PerCP
        let raw = vec![18000.0,14000.0,600.0,500.0];
        let out = flowlineage::domain::math::apply_compensation(&raw, inv);
        for (ch,(u,c)) in ["FITC","PE","APC","PerCP"].iter().zip(raw.iter().zip(out.iter())) {
            println!("{ch}: raw={u} comp={c:.2} asinh150(raw)={:.3} asinh150(comp)={:.3}",
                (*u/150.0).asinh(), (*c/150.0).asinh());
        }
    }
}
