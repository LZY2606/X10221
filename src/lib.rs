//! Flowline —— 流式边界室离线复核工具库。
//!
//! 来源（原始字节/元数据）与解释（补偿矩阵、变换、门、谱系、裁定）分层保存，
//! 所有派生物不可变、内容可校验，分支事件日志只追加。

pub mod bundle;
pub mod case;
pub mod comp;
pub mod db;
pub mod error;
pub mod fcs;
pub mod gating;
pub mod geom;
pub mod repo;
pub mod review;
pub mod server;
pub mod types;

pub use error::FlowError;
pub type Result<T> = std::result::Result<T, FlowError>;
