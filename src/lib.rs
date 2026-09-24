//! 流式边界室：补偿矩阵、门控系谱与细胞群不确定性。
//!
//! 分层约定：
//! - 原始层（blob / dataset / spillover 元数据）只保留原始表示；
//! - 派生层（补偿、变换、门、群体结果）才做规范化与计算；
//! - 一切状态变化都是追加事件，旧事件与旧结论永不被覆盖。

pub mod canonical;
pub mod domain;
pub mod events;
pub mod seed;
pub mod service;
pub mod store;
pub mod projection;
pub mod web;
