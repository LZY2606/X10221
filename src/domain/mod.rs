//! 流式领域核心：不依赖数据库与 HTTP 的纯逻辑。

pub mod fcs;
pub mod gates;
pub mod geometry;
pub mod ids;
pub mod materials;
pub mod matrix;
pub mod stats;
pub mod transform;

/// 规则集合版本。任何规则改动都应推进此版本，派生层据此整体失效，
/// 但旧结论保留（不覆盖历史）。
pub const RULE_VERSION: &str = "flowline.rules.v1";

/// 派生结果类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedKind {
    /// 补偿后事件：输入 raw_channels + compensation_matrix。
    Compensated,
    /// 变换后事件：输入 compensated + transform_params。
    Transformed,
    /// 群体计数：输入 transformed（或对应 basis 事件）+ gate_polygon +
    /// 上游群体（分母）+ gate_hierarchy。
    Population,
}

impl DerivedKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            DerivedKind::Compensated => "compensated",
            DerivedKind::Transformed => "transformed",
            DerivedKind::Population => "population",
        }
    }
}
