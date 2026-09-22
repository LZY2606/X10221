//! flow-lineage 启动入口：cargo run -- --addr 127.0.0.1:5341

use flow_lineage::server::serve_addr;

#[tokio::main]
async fn main() {
    let mut addr = "127.0.0.1:5341".to_string();
    let mut db = "flow-lineage.sqlite".to_string();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--addr" => {
                addr = args[i + 1].clone();
                i += 2;
            }
            "--db" => {
                db = args[i + 1].clone();
                i += 2;
            }
            _ => i += 1,
        }
    }
    println!("flow-lineage 监听 http://{addr} （数据库 {db}）");
    if let Err(e) = serve_addr(&addr, db).await {
        eprintln!("启动失败: {e}");
        std::process::exit(1);
    }
}
