//! 领域层：流式事件、补偿、变换、门控谱系与事件溯源。
//!
//! 约定：
//! - 所有领域量携带单位或坐标基准（见 [`Unit`] / [`CoordBasis`]）。
//! - 原始表示导入时原样保留，派生层才做规范化；无法转换的值留在原位置并附诊断。
//! - 每个版本化对象有稳定身份（`stable_id`，导出/删除/再导入后不变）和内容指纹（`rev`）。

pub mod ids;
pub mod fcs;
pub mod matrix;
pub mod transform;
pub mod gate;
pub mod model;
pub mod derive;
pub mod events;
pub mod project;

pub use ids::*;
pub use model::*;
