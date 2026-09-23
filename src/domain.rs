//! 流式领域模型：通道、原始事件、spillover、补偿、变换、门、谱系。
//!
//! 设计约束（对应需求）：
//! * 原始表示与解释分离：原始值以字符串 repr 保存，派生层才解析/规范化。
//! * 所有领域量携带单位或坐标基准。
//! * 无法转换的值留在原位置并附诊断，绝不丢弃。

use serde::{Deserialize, Serialize};

pub const BASIS_RAW: &str = "raw[channel_counts]";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub domain: String,
    pub units_notes: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Scatter,
    Fluorescence,
    Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelDef {
    pub id: String,
    pub label: String,
    pub kind: ChannelKind,
    /// 该通道检测到的荧光素（散射/时间通道为 None）。
    pub fluorophore: Option<String>,
    /// 原始测量单位 / ADC 基准，例如 "18-bit ADC channel_counts, 0..262144"。
    pub unit: String,
    pub range_min: f64,
    pub range_max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvent {
    pub event_id: String,
    /// 通道 id -> 原始表示（导入时原样保留）。
    pub channels: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventsData {
    pub basis: String,
    pub acquired_unit: String,
    pub events: Vec<RawEvent>,
}

/// 单染对照测量得到的 spillover（溢出）信息。S * 真实荧光 = 观测信号。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SingleStainControl {
    pub control_id: String,
    pub fluorophore: String,
    pub detector: String,
    pub reagent_lot: String,
    pub gate_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverData {
    /// 探测器（=荧光通道）顺序，矩阵行列均按此顺序。
    pub detector_order: Vec<String>,
    /// S[i][j]：荧光素 j 漏到探测器 i 的比例，无量纲。
    pub matrix: Vec<Vec<f64>>,
    pub fraction_unit: String,
    pub controls: Vec<SingleStainControl>,
    pub basis: String,
}

/// 被采纳用于补偿的矩阵 M（数值上等于所选 S 求逆候选）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationData {
    pub detector_order: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub fraction_unit: String,
    pub derived_from_spillover: String,
    pub method: String,
    pub ridge_epsilon: Option<f64>,
    pub basis: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransformKind {
    Linear { factor: f64, offset: f64 },
    Asinh { cofactor: f64, offset: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelTransform {
    pub input_unit: String,
    pub output_basis: String,
    #[serde(flatten)]
    pub kind: TransformKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformData {
    /// channel id -> 变换参数；output_basis 即门坐标基准。
    pub channels: std::collections::BTreeMap<String, ChannelTransform>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateAxis {
    pub channel: String,
    /// 与 TransformData 中该通道 output_basis 一致。
    pub basis: String,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateGeometry {
    pub gate_id: String,
    pub x: GateAxis,
    pub y: GateAxis,
    /// 顶点坐标基准：变换后通道坐标（非原始 ADC）。
    pub coordinate_basis: String,
    /// 边界点规则：落在边上的事件计入门内，并单独标注 on_boundary。
    pub boundary_rule: String,
    pub polygon: Vec<Point>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LineageNode {
    pub node_id: String,
    pub label: String,
    pub gate_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LineageEdge {
    pub parent: String,
    pub child: String,
    pub relation: String,
}

/// 门父子关系与显示名；与多边形几何分开版本化，改名不改变几何依赖。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LineageData {
    pub root_id: String,
    pub nodes: Vec<LineageNode>,
    pub edges: Vec<LineageEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourcePayload {
    Events(EventsData),
    Spillover(SpilloverData),
    Compensation(CompensationData),
    Transform(TransformData),
    GatePolygon(GateGeometry),
    Lineage(LineageData),
}

impl SourcePayload {
    pub fn kind_name(&self) -> &'static str {
        match self {
            SourcePayload::Events(_) => "events",
            SourcePayload::Spillover(_) => "spillover",
            SourcePayload::Compensation(_) => "compensation",
            SourcePayload::Transform(_) => "transform",
            SourcePayload::GatePolygon(_) => "gate_polygon",
            SourcePayload::Lineage(_) => "lineage",
        }
    }
    pub fn identity(&self) -> String {
        match self {
            SourcePayload::GatePolygon(g) => format!("polygon:{}", g.gate_id),
            other => other.kind_name().to_string(),
        }
    }
}

/// 原始值解析结果：保留 repr，f64 转换失败时留在原位置。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedValue {
    pub repr: String,
    pub value: Option<f64>,
}

pub fn parse_raw(repr: &str) -> ParsedValue {
    let repr = repr.trim().to_string();
    match repr.parse::<f64>() {
        Ok(v) if v.is_finite() => ParsedValue { repr, value: Some(v) },
        _ => ParsedValue { repr, value: None },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub code: String,
    pub severity: String,
    pub event_id: Option<String>,
    pub channel: Option<String>,
    pub message: String,
    pub value_repr: Option<String>,
}

/// 边界容差按坐标尺度缩放，单位与门坐标一致。
pub fn on_segment(p: Point, a: Point, b: Point) -> bool {
    const EPS: f64 = 1e-9;
    let vx = b.x - a.x;
    let vy = b.y - a.y;
    let wx = p.x - a.x;
    let wy = p.y - a.y;
    let cross = (wx * vy - wy * vx).abs();
    let scale = (1.0_f64).max((vx * vx + vy * vy).sqrt());
    if cross > EPS * scale {
        return false;
    }
    let within = {
        let minx = a.x.min(b.x) - EPS * scale;
        let maxx = a.x.max(b.x) + EPS * scale;
        let miny = a.y.min(b.y) - EPS * scale;
        let maxy = a.y.max(b.y) + EPS * scale;
        p.x >= minx && p.x <= maxx && p.y >= miny && p.y <= maxy
    };
    within
}

/// 射线法判定，返回 (inside, on_boundary)。
pub fn point_in_polygon(p: Point, poly: &[Point]) -> (bool, bool) {
    let n = poly.len();
    if n < 3 {
        return (false, false);
    }
    let mut boundary = false;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let a = poly[j];
        let b = poly[i];
        if on_segment(p, a, b) {
            boundary = true;
        }
        if (a.y > p.y) != (b.y > p.y) {
            let x_inter = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x_inter {
                inside = !inside;
            }
        }
        j = i;
    }
    // 落在边上时按 boundary_rule 计入。
    if boundary {
        inside = true;
    }
    (inside, true && boundary)
}

pub fn transform_value(t: &ChannelTransform, v: f64) -> f64 {
    match t.kind {
        TransformKind::Linear { factor, offset } => factor * v + offset,
        TransformKind::Asinh { cofactor, offset } => (v / cofactor).asinh() + offset,
    }
}
