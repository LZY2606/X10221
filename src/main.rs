//! 启动：`cargo run -- --addr 127.0.0.1:5341`

use flow_lineage::web;

#[tokio::main]
async fn main() {
    let mut addr = "127.0.0.1:5341".to_string();
    let mut db_path = "flow-boundary-chamber.db".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--addr" => addr = args.next().expect("--addr 需要地址参数"),
            "--db" => db_path = args.next().expect("--db 需要路径参数"),
            other => {
                eprintln!("未知参数: {other}");
                std::process::exit(2);
            }
        }
    }

    let state = web::build_state(&db_path).expect("打开/迁移数据库失败");
    let (local, _handle) = web::serve(state, &addr)
        .await
        .expect("绑定监听地址失败");
    println!("流式边界室已启动: http://{local}");
    println!("（指定地址: http://{addr}，数据库: {db_path}）");

    tokio::signal::ctrl_c().await.ok();
}
