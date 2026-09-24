//! Flowline：流式补偿 / 门控系谱 / 细胞群不确定性的事件溯源复核内核。
//!
//! 所有派生结论都以只追加事件表达；原始字节与解释分层保存。
//! 详见 `cases/flow_boundary_case.json` 与 README。

pub mod domain;
pub mod linalg;
pub mod polygon;
pub mod transform;
pub mod projection;
pub mod store;
pub mod cases;
pub mod service;
pub mod web;
