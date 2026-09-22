//! 流式领域对象：所有领域量都显式携带单位或坐标基准。
//! 导入层保留原始表示，派生层才做规范化；无法转换的值留在原位置并附诊断。

use serde::{Deserialize, Serialize};

pub type Id = String;

/// 通道描述：单位是仪器原始采集量（通道号/检测器名）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelSpec {
    pub name: String,
    pub detector: String,
    pub fluorophore: Option<String>,
    /// 原始动态范围上界（来自 `$PnR`），无量纲通道计数。
    pub range: f64,
    /// 线性/对数通道标记，保留 FCS 原值。
    pub decade: String,
}

/// 单条事件的原始通道值。`raw_tokens` 保留 FCS 文本原始表示，
/// `values` 是解析后的浮点；无法解析的通道 value 为 None，原始 token 仍在。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvent {
    /// 稳定身份：FCS 文件内行序（从 0 起），导入后永不改变。
    pub ordinal: usize,
    pub values: Vec<Option<f64>>,
    pub raw_tokens: Vec<String>,
}

impl RawEvent {
    pub fn extreme(&self, channels: &[ChannelSpec]) -> Vec<usize> {
        let mut out = Vec::new();
        for (i, v) in self.values.iter().enumerate() {
            if let Some(x) = v {
                let r = channels.get(i).map(|c| c.range).filter(|r| *r > 0.0).unwrap_or(f64::MAX);
                if !x.is_finite() || x < 0.0 || x >= r {
                    out.push(i);
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FcsDataSet {
    pub version: String,
    pub text_delimiter: String,
    pub keywords: Vec<(String, String)>,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<RawEvent>,
}

/// spillover 元数据：单独版本化，描述单染对照与检测器串扰，不持有数值矩阵的“已求逆”状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    pub source_keyword: String,
    pub detectors: Vec<String>,
    pub fluorophores: Vec<String>,
    pub single_stain_files: Vec<String>,
    pub basis: String,
}

/// 补偿矩阵 S：测得信号 y = S x（x 为真实染色）。补偿即求 S^{-1} y。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationMatrix {
    pub labels: Vec<String>,
    pub rows: Vec<Vec<f64>>,
    /// 坐标基准：`spillover_linear_counts`（线性化通道计数上的串扰系数）。
    pub basis: String,
}

/// 反双正弦变换参数（FlowJo/Cytobank 风格），仅作用于派生显示层。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AsinhParams {
    pub channel: String,
    pub cofactor: f64,
    pub scale: f64,
    /// 输入坐标基准：`linearized_counts`；输出坐标基准：`asinh_dimensionless`。
    pub output_basis: String,
}

impl AsinhParams {
    pub fn apply(&self, v: f64) -> f64 {
        (v / self.cofactor).asinh() * self.scale
    }
}

/// 门顶点坐标基准：始终是变换后坐标（`asinh_dimensionless`），
/// 导入草案时原样保留，门对象记录所钉住的变换版本。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatePolygon {
    pub node_id: Id,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub vertices: Vec<[f64; 2]>,
    pub coordinate_basis: String,
    /// 边界点处理策略（无法唯一决定时的合法候选来源）。
    pub boundary_policy: BoundaryPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryPolicy {
    /// 严格：恰在边上的点算门内。
    IncludeBoundary,
    /// 严格：恰在边上的点算门外。
    ExcludeBoundary,
}

/// 门树父子关系，独立版本化：改名不改 `node_id`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateLink {
    pub child_node_id: Id,
    pub parent_node_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateDraft {
    pub gates: Vec<GatePolygon>,
    pub links: Vec<GateLink>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Membership {
    Inside,
    Boundary,
    Outside,
    /// 输入含无法转换的值（极端/非有限/缺失），保留原位置不出结论。
    Invalid,
}

/// 射线法判定点是否在多边形内；同时显式识别“恰在边界上”的点。
pub fn point_in_polygon(x: f64, y: f64, poly: &[[f64; 2]], eps: f64) -> Membership {
    if !x.is_finite() || !y.is_finite() {
        return Membership::Invalid;
    }
    let n = poly.len();
    if n < 3 {
        return Membership::Invalid;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if point_on_segment(x, y, xi, yi, xj, yj, eps) {
            return Membership::Boundary;
        }
        if (yi > y) != (yj > y) {
            let cross = (xj - xi) * (y - yi) / (yj - yi) + xi;
            if cross > x {
                inside = !inside;
            }
        }
        j = i;
    }
    if inside { Membership::Inside } else { Membership::Outside }
}

fn point_on_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64, eps: f64) -> bool {
    let cross = (px - ax) * (by - ay) - (py - ay) * (bx - ax);
    if cross.abs() > eps {
        return false;
    }
    let dot = (px - ax) * (px - bx) + (py - ay) * (py - by);
    dot <= eps
}

/// 门评估产生的细胞群结果（不可变快照）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PopulationResult {
    pub result_id: Id,
    pub node_id: Id,
    pub gate_version_id: Id,
    pub parent_result_id: Option<Id>,
    /// 分母钉住的父群结果版本（None 表示以全部有效事件为分母）。
    pub denominator_result_id: Option<Id>,
    /// 母群在该版本下的成员计数（机器可读事件会重复该值）。
    pub denominator_count: usize,
    pub numerator_count: usize,
    pub percent: f64,
    /// 逐事件成员身份（顺序与原始事件 ordinal 对齐）。
    pub memberships: Vec<Membership>,
    /// 评估时钉住的依赖版本。
    pub basis: PopulationBasis,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PopulationBasis {
    pub fcs_version_id: Id,
    pub spillover_version_id: Id,
    pub compensation_version_id: Id,
    pub transform_version_id: Id,
    pub gate_version_id: Id,
    pub lineage_version_id: Id,
    pub coordinate_basis: String,
}

/// 无法唯一决定时保留的全部合法候选。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub candidate_id: Id,
    pub target_kind: String,
    pub target_ref: Id,
    pub label: String,
    pub payload: serde_json::Value,
    pub rationale: String,
    pub adopted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub code: String,
    pub severity: String,
    pub subject_ref: Id,
    pub message: String,
    pub detail: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn inside_outside_and_boundary() {
        let p = square();
        assert_eq!(point_in_polygon(5.0, 5.0, &p, 1e-9), Membership::Inside);
        assert_eq!(point_in_polygon(20.0, 5.0, &p, 1e-9), Membership::Outside);
        assert_eq!(point_in_polygon(0.0, 5.0, &p, 1e-9), Membership::Boundary);
        assert_eq!(point_in_polygon(5.0, 0.0, &p, 1e-9), Membership::Boundary);
        assert_eq!(point_in_polygon(f64::NAN, 1.0, &p, 1e-9), Membership::Invalid);
    }

    #[test]
    fn extreme_events_flagged_without_dropping() {
        let chans = vec![ChannelSpec {
            name: "FL1".into(), detector: "D1".into(), fluorophore: None, range: 1024.0, decade: "LIN".into(),
        }];
        let ev = RawEvent { ordinal: 0, values: vec![Some(2000.0)], raw_tokens: vec!["2000".into()] };
        assert_eq!(ev.extreme(&chans), vec![0]);
    }
}
