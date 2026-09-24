//! 应用服务：命令 → 事件（事务落库）+ 查询视图（事件重放）。

pub mod commands;
pub mod service;
pub mod views;
pub mod bundle;

pub use service::Service;

mod service_import;
mod service_commands;
