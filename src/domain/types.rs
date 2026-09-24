use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 带单位/坐标基准的领域量。原始表示导入时原样保留，派生层才规范化。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scaled {
    /// 原始表示（例如 "4823.0"、"NaN"、"0.37"），导入时不做不可逆清洗。
    pub raw: String,
    /// 能转换时给出规范数值；不能转换时为 null 并附诊断，原值保留在 raw。
    pub value: Option<f64>,
    /// 单位或坐标基准，例如 "counts"、"counts_compensated"、"asinh(cofactor=150)"、"percent:parent"。
    pub unit: String,
    /// 转换诊断（无法转换/极端信号时非空）。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub diagnostics: Vec<String>,
}

impl Scaled {
    pub fn raw_counts(v: f64) -> Self {
        Self { raw: format_f64(v), value: Some(v), unit: "counts".to_string(), diagnostics: vec![] }
    }
    pub fn unconvertible(raw: impl Into<String>, why: impl Into<String>) -> Self {
        Self { raw: raw.into(), value: None, unit: "unknown".to_string(), diagnostics: vec![why.into()] }
    }
    pub fn with_unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = unit.into();
        self
    }
}

pub fn format_f64(v: f64) -> String {
    if v.is_nan() {
        "NaN".to_string()
    } else if v.is_infinite() {
        if v > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() }
    } else if v == v.trunc() && v.abs() < 1e15 {
        format!("{:.1}", v)
    } else {
        let s = format!("{:.6}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelDef {
    /// 稳定通道短名（如 FITC），作为矩阵/门/事件的引用键。
    pub name: String,
    /// 检测器长名（$PnN/$PnS），仅展示。
    pub detector: String,
    /// 荧光/散射标记，例如 "CD3-FITC"。
    pub label: String,
    /// 原始单位（FCS $PnE/$PnG 描述保留在 raw_repr）。
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub raw_repr: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventPoint {
    /// 原始通道值，逐通道保留原始表示。
    pub channels: Vec<Scaled>,
    /// 极端信号/越界诊断，保留在原位、不删点。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FcsEvents {
    pub fcs_version: String,
    pub channels: Vec<ChannelDef>,
    pub points: Vec<EventPoint>,
    /// 无法解析/转换的通道定位（event_index, channel_index, 原因），值仍留在 points 中。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub unconvertible: Vec<(usize, usize, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    pub matrix_id: String,
    pub channel_names: Vec<String>,
    /// spillover 矩阵 S：列 j 单染时在各检测器 i 的分配（列和≈1，对角=1）。
    pub spillover: Vec<Vec<f64>>,
    pub source_blob_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub raw_repr: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    pub transform_id: String,
    /// 通道 -> cofactor（asinh(x/cofactor)）。
    pub cofactors: std::collections::BTreeMap<String, f64>,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolygonGate {
    pub gate_id: String,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    /// 坐标基准（transform 版本引用），顶点在变换后坐标中。
    pub basis: String,
    pub vertices: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateRelation {
    /// 子门 id。
    pub child: String,
    /// 父门 id；根门为 None（分母钉全事件）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateDraft {
    pub draft_id: String,
    pub gates: Vec<PolygonGate>,
    pub relations: Vec<GateRelation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateStats {
    /// 父群版本钉住键，None 表示全事件基线。
    pub pinned_parent_pop: Option<String>,
    pub numerator: usize,
    pub denominator: usize,
    pub percent: Option<f64>,
    pub denominator_unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PopulationResult {
    /// 群体稳定身份：门@门版本(父@父群版本)，修订门后产生新 id 而非覆盖。
    pub pop_id: String,
    pub gate_id: String,
    pub gate_version: u64,
    pub branch: String,
    /// 所属门版本链。
    pub gate_ref: String,
    /// 分母来源（钉住的父群体版本 pop_id；None=全事件基线 blob 版本）。
    pub denominator_ref: String,
    pub stats: GateStats,
    /// true 表示依赖已过期：保留旧结论，不覆盖。
    pub stale: bool,
    /// 过期原因（上游对象@被钉版本 vs 当前版本）。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stale_reasons: Vec<String>,
    /// 命中的原始事件序号（以钉住输入为准）。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub members: Vec<usize>,
    pub adopted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationCandidate {
    pub candidate_id: String,
    pub label: String,
    /// 候选逆矩阵；奇异阻断时为空。
    pub inverse: Option<Vec<Vec<f64>>>,
    pub reciprocal_condition: Option<f64>,
    pub condition_number: Option<f64>,
    pub singular: bool,
    pub near_singular: bool,
    pub diagnostics: Vec<String>,
    /// 每个原始点一行补偿值（逐通道 Scaled，保留诊断）。
    pub compensated: Option<Vec<Vec<Scaled>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationResult {
    pub matrix_id: String,
    pub matrix_version: u64,
    pub branch: String,
    pub input_events_ref: String,
    pub candidates: Vec<CompensationCandidate>,
    /// 裁定选中的候选；未裁定前为 None（近奇异不能被默默求逆）。
    pub selected_candidate: Option<String>,
    pub adopted: bool,
    pub stale: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stale_reasons: Vec<String>,
}

/// 机器可读取的判定事件（rule_decided）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleRationale {
    pub rule: String,
    pub status: String,
    pub affected: Vec<String>,
    pub evidence: Value,
}

/// 分支上的领域事件。所有写入只追加，不原地修改。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DomainEvent {
    CaseImported {
        case_id: String,
        name: String,
        events_blob_id: String,
        controls: Vec<String>,
        manifest: Value,
    },
    SpilloverImported {
        matrix_id: String,
        meta: SpilloverMeta,
    },
    CompensationMatrixRevised {
        matrix_id: String,
        based_on_version: u64,
        spillover: Vec<Vec<f64>>,
        reason: String,
    },
    TransformParamsRevised {
        transform_id: String,
        based_on_version: u64,
        cofactors: std::collections::BTreeMap<String, f64>,
        reason: String,
    },
    GateRenamed {
        gate_id: String,
        based_on_version: u64,
        new_name: String,
    },
    GateGeometryRevised {
        gate_id: String,
        based_on_version: u64,
        vertices: Vec<[f64; 2]>,
        basis: String,
    },
    GateRelationRevised {
        child: String,
        based_on_version: u64,
        new_parent: Option<String>,
    },
    RawEventsRevised {
        new_blob_id: String,
        based_on_blob_id: String,
        scope_note: String,
    },
    CompensationComputed {
        result: CompensationResult,
    },
    PopulationComputed {
        result: PopulationResult,
    },
    DecisionRecorded {
        subject: String,
        decision: String,
        accepted: bool,
        rationale: RuleRationale,
    },
    RuleDecided {
        rationale: RuleRationale,
    },
    StaleFlagged {
        subject: String,
        reasons: Vec<String>,
        affected_pops: Vec<String>,
    },
    BranchCreated {
        new_branch: String,
        from_branch: String,
        reason: String,
    },
}

impl DomainEvent {
    pub fn kind_name(&self) -> &'static str {
        match self {
            DomainEvent::CaseImported { .. } => "case_imported",
            DomainEvent::SpilloverImported { .. } => "spillover_imported",
            DomainEvent::CompensationMatrixRevised { .. } => "comp_matrix_revised",
            DomainEvent::TransformParamsRevised { .. } => "transform_revised",
            DomainEvent::GateRenamed { .. } => "gate_renamed",
            DomainEvent::GateGeometryRevised { .. } => "gate_geometry_revised",
            DomainEvent::GateRelationRevised { .. } => "gate_relation_revised",
            DomainEvent::RawEventsRevised { .. } => "raw_events_revised",
            DomainEvent::CompensationComputed { .. } => "comp_computed",
            DomainEvent::PopulationComputed { .. } => "pop_computed",
            DomainEvent::DecisionRecorded { .. } => "decision_recorded",
            DomainEvent::RuleDecided { .. } => "rule_decided",
            DomainEvent::StaleFlagged { .. } => "stale_flagged",
            DomainEvent::BranchCreated { .. } => "branch_created",
        }
    }
}
