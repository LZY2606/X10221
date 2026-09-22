use flowlineage::case::{discover_case_dir, load_case, replay};
use flowlineage::{Service, Store};
use std::sync::Arc;

fn main() {
    let dir = discover_case_dir(&std::env::current_dir().unwrap()).unwrap();
    let (def, root) = load_case(&dir).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("d.db");
    let svc = Service::new(Arc::new(Store::open(db.to_str().unwrap()).unwrap()));
    let _ = replay(&svc, "p", &def, &root);
    let v = svc.view("p", "main", Some(12)).unwrap();
    let fcs = svc.fcs_of("p", "raw-sample-1").unwrap();
    let sc = flowlineage::domain::engine::build_scatter(&v, &fcs, "FITC-A", "PE-A");
    for p in sc.points.iter().take(26) {
        println!("{} raw=({:.1},{:.1}) t=({:.3},{:.3}) ok={}", p.event_index, p.x_raw, p.y_raw, p.x, p.y, p.x_ok&&p.y_ok);
    }
    println!("transforms: {:?}", v.transforms.keys().collect::<Vec<_>>());
}
