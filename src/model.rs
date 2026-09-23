//! 领域模型：原始材料、解释层（补偿/变换/门/谱系）、事件负载与投影。
//! 约定：所有领域量携带单位或坐标基准；原始层保留原始表示，派生层才规范化。

use serde::{Deserialize, Serialize};

// ---------- 材料 ----------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MaterialKind {
    FcsEvents,
    Spillover,
    Compensation,
    Transform,
    Gates,
    Lineage,
}

impl MaterialKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MaterialKind::FcsEvents => "fcs_events",
            MaterialKind::Spillover => "spillover",
            MaterialKind::Compensation => "compensation",
            MaterialKind::Transform => "transform",
            MaterialKind::Gates => "gates",
            MaterialKind::Lineage => "lineage",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "fcs_events" => MaterialKind::FcsEvents,
            "spillover" => MaterialKind::Spillover,
            "compensation" => MaterialKind::Compensation,
            "transform" => MaterialKind::Transform,
            "gates" => MaterialKind::Gates,
            "lineage" => MaterialKind::Lineage,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub location: String,
    #[serde(default = "default_severity")]
    pub severity: String,
}

fn default_severity() -> String {
    "warning".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventChannel {
    pub name: String,
    /// 坐标基准/单位说明，如 "FCS linear 10-bit ADC" 或 "asinh(compensated)"
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPoint {
    /// 事件序号（FCS 中的稳定序号）
    pub index: usize,
    /// 各通道值；无法转换的通道保留 null 并在 diagnostics 中记录
    pub values: Vec<Option<f64>>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsData {
    pub channels: Vec<EventChannel>,
    pub events: Vec<EventPoint>,
    /// 数据角色：sample / single_stain
    pub role: String,
    /// 单染对照时给出的染色通道
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stain_channel: Option<String>,
}

// ---------- spillover 元数据（原始来源，不做规范化清洗） ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverMeta {
    pub channels: Vec<String>,
    /// 行序与 channels 对齐的观测溢出（十进制/百分比字符串原样保留）
    pub matrix: Vec<Vec<String>>,
    pub source: String,
}

// ---------- 补偿矩阵（解释层） ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationMatrix {
    pub channels: Vec<String>,
    pub rows: Vec<Vec<f64>>,
    pub unit: String,
}

// ---------- 变换参数（解释层） ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransformSpec {
    Linear {
        channel: String,
        scale: f64,
        offset: f64,
        unit: String,
    },
    Log {
        channel: String,
        base: f64,
        offset: f64,
        unit: String,
    },
    Asinh {
        channel: String,
        t: f64,
        m: f64,
        a: f64,
        unit: String,
    },
}

impl TransformSpec {
    pub fn channel(&self) -> &str {
        match self {
            TransformSpec::Linear { channel, .. }
            | TransformSpec::Log { channel, .. }
            | TransformSpec::Asinh { channel, .. } => channel,
        }
    }
    pub fn apply(&self, x: f64) -> Option<f64> {
        if !x.is_finite() {
            return None;
        }
        Some(match self {
            TransformSpec::Linear { scale, offset, .. } => x * scale + offset,
            TransformSpec::Log { base, offset, .. } => {
                let v = x + offset;
                if v <= 0.0 {
                    return None;
                }
                v.log(*base)
            }
            TransformSpec::Asinh { t, m, a, .. } => {
                // Gating-ML 2.0 ASINH 参数化：T, M, A
                let b = (10_f64.powf(*m / 2.0) - 10_f64.powf(*a / 2.0)).ln() / (*t * (1.0 - 10_f64.powf(-(*m - *a) / 2.0)));
                if !b.is_finite() || b == 0.0 {
                    return None;
                }
                ((b * x).asinh() - (b * 0.0).asinh() + a * std::f64::consts::LN_2)
                    / ((b * t).asinh() + a * std::f64::consts::LN_2)
                    * m
            }
        })
    }
}

// ---------- 门多边形与谱系（解释层） ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatePolygon {
    /// 稳定门身份（改名不变）
    pub gate_id: String,
    pub label: String,
    pub x_channel: String,
    pub y_channel: String,
    /// 坐标基准（linear / asinh / log，与 transform 对应）
    pub coords_basis: String,
    /// 多边形顶点（含边界点）
    pub vertices: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSet {
    pub gates: Vec<GatePolygon>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageNode {
    /// 群稳定身份
    pub node_id: String,
    pub label: String,
    pub gate_id: Option<String>,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lineage {
    pub root_label: String,
    pub nodes: Vec<LineageNode>,
}

// ---------- 解释 ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InterpPayload {
    Spillover(SpilloverMeta),
    Compensation(CompensationMatrix),
    Transform {
        specs: Vec<TransformSpec>,
    },
    Gates(GateSet),
    Lineage(Lineage),
}

impl InterpPayload {
    pub fn kind_name(&self) -> &'static str {
        match self {
            InterpPayload::Spillover(_) => "spillover",
            InterpPayload::Compensation(_) => "compensation",
            InterpPayload::Transform { .. } => "transform",
            InterpPayload::Gates(_) => "gates",
            InterpPayload::Lineage(_) => "lineage",
        }
    }
}

// ---------- 事件负载（账本） ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedMaterial {
    pub material_id: String,
    pub kind: String,
    pub name: String,
    pub media_type: String,
    pub sha256: String,
    pub unit_or_basis: String,
    pub ordinal: i64,
    pub batch_id: String,
    #[serde(default)]
    pub supersedes: Option<String>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_id: String,
    pub seq: i64,
    pub branch_id: String,
    pub event_type: String,
    pub actor: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_seq: Option<i64>,
    pub payload: serde_json::Value,
}
