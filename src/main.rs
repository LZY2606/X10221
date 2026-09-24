use std::net::SocketAddr;
use std::sync::Arc;

use flowline::app::Service;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut addr: SocketAddr = "127.0.0.1:5341".parse().unwrap();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--addr" {
            if let Some(v) = args.next() {
                addr = v.parse()?;
            }
        }
    }

    let db_path = std::env::var("FLOWLINE_DB").unwrap_or_else(|_| "flowline.db".to_string());
    let svc = Service::open_file(&db_path)?;
    println!("FlowLine 流式边界室已启动: http://{addr}  (db={db_path})");
    flowline::web::serve(addr, Arc::new(svc)).await?;
    Ok(())
}
