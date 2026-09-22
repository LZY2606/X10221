//! 启动：cargo run -- --addr 127.0.0.1:5341
use clap::Parser;
use flowlineage::{router, Service, Store};
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "flowlineage", version, about = "流式补偿/门控谱系复核工具")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:5341")]
    addr: String,
    #[arg(long, default_value = "flowlineage.db")]
    db: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let store = Arc::new(Store::open(&args.db).expect("打开 SQLite 失败"));
    let svc = Arc::new(Service::new(store));
    let app = router(svc);
    let listener = tokio::net::TcpListener::bind(&args.addr)
        .await
        .expect("绑定地址失败");
    println!("flowlineage 监听 http://{}", args.addr);
    axum::serve(listener, app).await.expect("server error");
}
