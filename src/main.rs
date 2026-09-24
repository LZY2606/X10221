use flow_lineage::{api, db};

#[tokio::main]
async fn main() {
    let mut addr = "127.0.0.1:5341".to_string();
    let mut db_path = std::env::var("FLOW_DB").unwrap_or_else(|_| "flow.db".to_string());
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--addr" => {
                addr = args[i + 1].clone();
                i += 2;
            }
            "--db" => {
                db_path = args[i + 1].clone();
                i += 2;
            }
            _ => i += 1,
        }
    }
    let conn = db::open(&db_path).expect("打开数据库失败");
    let app = api::router(conn);
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("绑定端口失败");
    println!("流式边界室复核界面: http://{}", addr);
    axum::serve(listener, app).await.unwrap();
}
