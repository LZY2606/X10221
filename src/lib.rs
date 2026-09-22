pub fn parse_addr(args: Vec<String>) -> String {
    let mut addr = String::from("127.0.0.1:5341");
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        if a == "--addr" {
            if let Some(v) = it.next() { addr = v.clone(); }
        }
    }
    addr
}

pub fn serve(addr: String, db_path: std::path::PathBuf) {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async move {
        let app = flowline::http::app(db_path).await;
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        println!("flowline listening on http://{addr}");
        axum::serve(listener, app).await.unwrap();
    });
}
