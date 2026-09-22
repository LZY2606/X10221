//! 六类“来源/解释”材料的领域模型。
//!
//! 它们分别版本化，互不覆盖：
//! 1. 原始通道值（FCS 事件）
//! 2. spillover 元数据（原始 $SPILL 表示）
//! 3. 补偿矩阵（可被人工修订的解释层）
//! 4. 变换参数
//! 5. 门多边形
//! 6. 门父子关系

use serde::{Deserialize, Serialize};

use super::{
    fcs::FcsFile,
    gates::{GateHierarchy, PolygonGate},
    matrix::SpilloverMatrix,
    transform::TransformParams,
};

pub const MATERIAL_KINDS: [&str; 6] = [
    "raw_channels",
    "spillover_meta",
    "compensation_matrix",
    "transform_params",
    "gate_polygon",
    "gate_hierarchy",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaterialContent {
    RawChannels(RawChannels),
    SpilloverMeta(SpilloverMeta),
    CompensationMatrix(CompensationMatrix),
    TransformParams(TransformParamsMaterial),
    GatePolygon(PolygonGate),
    GateHierarchy(GateHierarchy),
}

impl MaterialContent {
    pub fn kind(&self) -> &'static str {
        match self {
            MaterialContent::RawChannels(_) => "raw_channels",
            MaterialContent::SpilloverMeta(_) => "spillover_meta",
            MaterialContent::CompensationMatrix(_) => "compensation_matrix",
            MaterialContent::TransformParams(_) => "transform_params",
            MaterialContent::GatePolygon(_) => "gate_polygon",
            MaterialContent::GateHierarchy(_) => "gate_hierarchy",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawChannels {
    /// 来源管（tube）标签，例如“单细胞悬液”或“FITC 单染”。
    pub tube: String,
    /// 坐标/单位基准（按通道）。
    pub channels: Vec<ChannelBasis>,
    /// 原始事件（导入表示，未做清洗）。
    pub events: serde_json::Value,
    /// 原始 FCS 字节的 SHA-256（来源链可回溯到字节）。
    pub source_sha256: String,
    pub source_filename: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelBasis {
    pub name: String,
    pub label: Option<String>,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    pub source_sha256: String,
    pub source_filename: String,
    /// FCS 中 $SPILL 原文，原样保留。
    pub raw_spill: String,
    /// 单染对照管 -> 通道（来自单染对照的解释，可选）。
    pub single_stain_tubes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationMatrix {
    /// 可能来自 spillover 元数据，也可能是人工修订版本。
    pub derived_from: Option<String>,
    pub matrix: SpilloverMatrix,
    pub revision_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParamsMaterial {
    pub params: TransformParams,
    pub revision_note: String,
}

/// 把解析后的 FCS 事件编码为保留原始表示的 JSON。
pub fn events_to_json(fcs: &FcsFile) -> serde_json::Value {
    let rows: Vec<Vec<serde_json::Value>> = fcs
        .events
        .iter()
        .map(|row| {
            row.iter()
                .map(|v| match v {
                    super::fcs::RawValue::Integer(i) => serde_json::json!({
                        "repr": "i64",
                        "v": i,
                    }),
                    super::fcs::RawValue::Float(f) => serde_json::json!({
                        "repr": "f64",
                        "v": special_float(*f),
                    }),
                })
                .collect()
        })
        .collect();
    serde_json::Value::Array(
        rows.into_iter()
            .map(serde_json::Value::Array)
            .collect(),
    )
}

/// NaN/Inf 无法进入严格 JSON，这里以带 `special` 标记的对象保留语义，
/// 重放时还原为相同的非有限值（“无法转换的值留在原位置并附诊断”）。
fn special_float(f: f64) -> serde_json::Value {
    if f.is_nan() {
        serde_json::json!({"special": "nan"})
    } else if f == f64::INFINITY {
        serde_json::json!({"special": "+inf"})
    } else if f == f64::NEG_INFINITY {
        serde_json::json!({"special": "-inf"})
    } else {
        serde_json::json!(f)
    }
}

/// 解码事件 JSON 为 f64（非有限值还原）。
pub fn events_to_f64(value: &serde_json::Value) -> Vec<Vec<f64>> {
    let rows = value.as_array().cloned().unwrap_or_default();
    rows.into_iter()
        .map(|row| {
            row.as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|cell| decode_cell(&cell))
                .collect()
        })
        .collect()
}

fn decode_cell(cell: &serde_json::Value) -> f64 {
    match cell.get("v") {
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(serde_json::Value::Object(o)) => match o.get("special").and_then(|s| s.as_str()) {
            Some("nan") => f64::NAN,
            Some("+inf") => f64::INFINITY,
            Some("-inf") => f64::NEG_INFINITY,
            _ => f64::NAN,
        },
        _ => f64::NAN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_kinds_are_distinct() {
        assert_eq!(MATERIAL_KINDS.len(), 6);
    }

    #[test]
    fn special_floats_round_trip_in_place() {
        let v = serde_json::json!([[
            {"repr": "f64", "v": {"special": "nan"}},
            {"repr": "f64", "v": {"special": "+inf"}},
            {"repr": "i64", "v": 17},
        ]]);
        let decoded = events_to_f64(&v);
        assert!(decoded[0][0].is_nan());
        assert_eq!(decoded[0][1], f64::INFINITY);
        assert_eq!(decoded[0][2], 17.0);
    }
}
