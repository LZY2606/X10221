use flowline::store::Store;

#[tokio::main]
async fn main() {
    let mut addr = "127.0.0.1:5341".to_string();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--addr" && i + 1 < args.len() {
            addr = args[i + 1].clone();
            i += 2;
        } else {
            i += 1;
        }
    }
    let db_path = std::env::var("FLOWLINE_DB").unwrap_or_else(|_| "flowline.db".to_string());
    let store = Store::open(&db_path).expect("open database");
    let app = flowline::server::router(store);
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!("flowline listening on http://{addr} (db: {db_path})");
    axum::serve(listener, app).await.expect("serve");
}
