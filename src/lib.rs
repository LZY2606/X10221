//! flowline —— 流式细胞术 / 补偿 / 门控谱系复核工具。
//!
//! 分层：
//! - `domain`   纯函数领域核心（矩阵分析、变换、门控、谱系视图推导）
//! - `store`    SQLite 持久化与状态迁移
//! - `service`  动作编排（分支、版本、裁定、冲突、过期传播）
//! - `casepack` 离线案例包「流式边界室」
//! - `server`   Axum HTTP API 与静态复核页

pub mod casepack;
pub mod domain;
pub mod server;
pub mod service;
pub mod store;
