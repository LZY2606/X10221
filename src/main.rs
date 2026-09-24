fn main() {
    let mut addr = "127.0.0.1:5341".to_string();
    let mut db = "flowline.db".to_string();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--addr" if i + 1 < args.len() => {
                addr = args[i + 1].clone();
                i += 2;
            }
            "--db" if i + 1 < args.len() => {
                db = args[i + 1].clone();
                i += 2;
            }
            _ => i += 1,
        }
    }
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(flowline::api::run(&addr, &db)).expect("server");
}
