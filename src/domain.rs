//! 领域类型：所有领域量携带单位或坐标基准。
//! 原始表示保存在 raw 层，派生层（补偿、变换、门判定）只引用版本，不回写原数据。

use serde::{Deserialize, Serialize};

/// 数值带单位/基准。导入时保留原始表示，无法转换的值留在原位置并附诊断。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Measured {
    /// 原始字符串表示（例如 FCS 文本段或 CSV 原值），永不丢失。
    pub raw: String,
    /// 解析后的数值；解析失败为 None，诊断写入 `diagnostic`。
    pub value: Option<f64>,
    /// 单位或坐标基准，例如 "linear:scale" / "asinh:arcsinh-scale"。
    pub unit: String,
    /// 解析/转换诊断；None 表示正常。
    pub diagnostic: Option<String>,
}

impl Measured {
    pub fn new(raw: impl Into<String>, value: Option<f64>, unit: impl Into<String>) -> Self {
        let raw = raw.into();
        let diagnostic = if value.is_none() {
            Some(format!("unconvertible value kept in place: {:?}", raw))
        } else {
            None
        };
        Measured { raw, value, unit: unit.into(), diagnostic }
    }
    pub fn numeric(raw: f64, unit: &str) -> Self {
        Measured { raw: format_num(raw), value: Some(raw), unit: unit.to_string(), diagnostic: None }
    }
}

pub fn format_num(x: f64) -> String {
    if x == x.trunc() && x.abs() < 1e15 {
        format!("{:.1}", x)
    } else {
        let s = format!("{:.6}", x);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// 一个 FCS 事件：各检测通道的原始通道值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvent {
    pub event_id: String,
    /// channel name -> 带单位原始测量（线性脉冲面积尺度）
    pub channels: Vec<(String, Measured)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
pub struct RawEvents {
    pub material_id: String,
    pub version: u32,
    pub channels: Vec<String>,
    pub events: Vec<RawEvent>,
}

/// Spillover 元数据：矩阵的“解释来源”，与补偿矩阵本体分开版本化。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
pub struct SpilloverMeta {
    pub material_id: String,
    pub version: u32,
    pub detectors: Vec<String>,
    pub fluorochromes: Vec<String>,
    /// 单染对照：荧光染料 -> 用于生成该列的对照事件范围
    pub single_stain_controls: Vec<SingleStainControl>,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SingleStainControl {
    pub fluorochrome: String,
    pub control_event_ids: Vec<String>,
    pub note: String,
}

/// 补偿矩阵（spillover S，dimensionless 比例）。补偿信号 = S^{-1} * 原始信号。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
pub struct CompensationMatrix {
    pub material_id: String,
    pub version: u32,
    pub labels: Vec<String>,
    /// 行优先方阵；每个元素携带单位 "dimensionless:spillover-fraction"
    pub rows: Vec<Vec<f64>>,
}

/// 变换参数（通道级 asinh：x' = asinh((x - offset) / cofactor) / ln10，基准为 asinh 尺度）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
pub struct TransformParams {
    pub material_id: String,
    pub version: u32,
    pub transforms: Vec<ChannelTransform>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelTransform {
    pub channel: String,
    pub method: String,
    pub cofactor: f64,
    pub offset: f64,
    pub output_unit: String,
}

/// 二维门多边形。坐标基准钉住变换版本（transformed:asinh-scale）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gate {
    pub gate_id: String,
    pub version: u32,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub coordinate_basis: String,
    /// 多边形顶点（闭合环，首尾不必重复）
    pub polygon: Vec<[f64; 2]>,
    /// 边界点约定："raycast-include" 表示射线法 + 落在边上的点计入
    pub boundary_convention: String,
}

/// 门的父子关系（谱系），与门几何分开版本化：改名/重挂不改变多边形。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
pub struct GateHierarchy {
    pub material_id: String,
    pub version: u32,
    /// gate_id -> 父 gate_id；根群为 None
    pub parent_of: Vec<(String, Option<String>)>,
}

/// 材料包：六类来源分开版本化。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Materials {
    pub raw_events: RawEvents,
    pub spillover_meta: SpilloverMeta,
    pub compensation: CompensationMatrix,
    pub transform: TransformParams,
    pub gates: Vec<Gate>,
    pub hierarchy: GateHierarchy,
}
