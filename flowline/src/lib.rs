//! Flowline —— 流式细胞术 / 补偿 / 门控谱系复核工具核心库。
//!
//! 架构要点：
//! - 来源（FCS 字节）与解释（spillover、矩阵、变换、门、边）分开版本化；
//! - 事件溯源：所有裁定/修订都是只追加事件，任何旧事件都可折叠出当时的结论；
//! - 派生结果不可变；上游修订只追加“过期”标记，不覆盖旧结果；
//! - 近奇异矩阵不默默求逆，而是显式登记多候选等待人工裁定；
//! - 门的百分比分母钉住具体版本的父群结果（pop ref）。

pub mod ids;
pub mod domain;
pub mod fcs;
pub mod transform;
pub mod compensation;
pub mod gates;
pub mod casepack;
pub mod store;
pub mod engine;
pub mod web;

pub use ids::{Oid, Stid, StableId, ts};
pub use store::Store;
pub use engine::Engine;
