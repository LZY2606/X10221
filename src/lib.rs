//! Flowline：流式细胞术补偿/门控谱系复核工具。
//!
//! 来源与解释分开保存：原始通道值、spillover 元数据、补偿矩阵、变换参数、
//! 门多边形与父子关系全部是内容寻址、不可变、版本化的制品；
//! 事件日志只增；上游修订按依赖关系计算失效范围，绝不全局替换旧结论。

pub mod app;
pub mod domain;
pub mod engine;
pub mod fcs;
pub mod replay;
pub mod store;
pub mod workspace;
