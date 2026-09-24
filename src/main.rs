mod actions;
mod casepack;
mod comp;
mod engine;
mod fcs;
mod gate;
mod model;
mod server;
mod store;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let mut addr = "127.0.0.1:5341".to_string();
    let mut db_path = "flowchamber.db".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--addr" => {
                addr = args.get(i + 1).cloned().ok_or("缺少 --addr 参数")?;
                i += 2;
            }
            "--db" => {
                db_path = args.get(i + 1).cloned().ok_or("缺少 --db 参数")?;
                i += 2;
            }
            other => return Err(format!("未知参数 {other}").into()),
        }
    }
    println!("FlowChamber 流式边界室复核界面: http://{addr}");
    println!("数据库: {db_path}");
    server::serve(&addr, &db_path).await?;
    Ok(())
}
