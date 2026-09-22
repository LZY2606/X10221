//! 领域模型：来源材料与解释版本分离。
//!
//! 原始通道值（FCS 材料，内容寻址）永不修改；补偿矩阵、变换参数、门多边形、
//! 父子关系分别作为独立实体各自版本化。

use serde::{Deserialize, Serialize};

/// 分支（对同一批材料的不同解释/裁定链）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub head_event_id: Option<String>,
    pub created_at: i64,
}

/// 原始来源材料（FCS 字节、JSON 草案）。内容寻址，永不更新。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Material {
    pub id: String,
    pub filename: String,
    pub kind: String, // fcs | gates | transforms | lineage
    pub sha256: String,
    pub size: i64,
    pub imported_at: i64,
    pub label: String,
}

/// 可版本化实体类型（各自独立版本化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Spillover,
    Transform,
    Gate,
    Lineage,
}

impl EntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntityKind::Spillover => "spillover",
            EntityKind::Transform => "transform",
            EntityKind::Gate => "gate",
            EntityKind::Lineage => "lineage",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "spillover" => EntityKind::Spillover,
            "transform" => EntityKind::Transform,
            "gate" => EntityKind::Gate,
            "lineage" => EntityKind::Lineage,
            _ => return None,
        })
    }
}

/// 一个实体的不可变版本。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityVersion {
    pub id: String,
    pub kind: EntityKind,
    pub entity_id: String,
    pub version: i64,
    pub data: serde_json::Value,
    pub source_material_id: Option<String>,
    pub branch_id: String,
    pub parent_version_id: Option<String>,
    pub created_at: i64,
    pub note: String,
}

// ---------- 变换参数 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "transform", rename_all = "snake_case")]
pub enum TransformSpec {
    Linear {
        channel: String,
        unit: String,
    },
    Asinh {
        channel: String,
        unit: String,
        cofactor: f64,
        /// 输出坐标基准名称（无量纲显示单位）。
        output_unit: String,
    },
}

impl TransformSpec {
    pub fn channel(&self) -> &str {
        match self {
            TransformSpec::Linear { channel, .. } => channel,
            TransformSpec::Asinh { channel, .. } => channel,
        }
    }
    pub fn apply(&self, x: f64) -> f64 {
        match self {
            TransformSpec::Linear { .. } => x,
            TransformSpec::Asinh { cofactor, .. } => (x / cofactor).asinh(),
        }
    }
}

/// 一个变换版本包含按通道的多个参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformSet {
    pub params: Vec<TransformSpec>,
}

// ---------- 门 ----------

/// 坐标基准：门点所在的坐标空间及引用的变换版本。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateCoordBasis {
    pub x_channel: String,
    pub y_channel: String,
    /// 这些点是“线性原始值”还是“变换输出值”。
    pub space: String, // linear | transformed
    pub transform_version_id: Option<String>,
    pub x_unit: String,
    pub y_unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolygonGate {
    pub gate_id: String,
    pub name: String,
    pub basis: GateCoordBasis,
    /// 多边形顶点（变换空间坐标）。
    pub points: Vec<[f64; 2]>,
    /// 边界点判定规则（半开区间，保证无歧义归属）。
    pub boundary_rule: String,
}

// ---------- 父子关系（独立版本化） ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateLink {
    pub gate_id: String,
    /// 父群体的门 id；根为 None。
    pub parent_gate_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageSet {
    pub links: Vec<GateLink>,
}

// ---------- 补偿候选与裁定 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensationVerdict {
    pub spill_version_id: String,
    pub status: String, // well_conditioned | near_singular | singular
    pub det: f64,
    pub cond: Option<f64>,
    /// 全部合法候选（近奇异/奇异时 >0 且不自动选择）。
    pub candidates: Vec<serde_json::Value>,
    /// 被采纳的候选索引；None 表示尚未裁定。
    pub chosen_index: Option<usize>,
}

// ---------- 事件 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    /// 一次导入（可能含多份材料）。失败整体回滚，不留半份记录。
    ImportBegan { batch_id: String },
    ImportCommitted {
        batch_id: String,
        material_ids: Vec<String>,
    },
    ImportRolledBack {
        batch_id: String,
        reason: String,
    },
    /// 分支的主管材料切换（局部源数据改动），scope 为受影响的门实体。
    SourceMaterialChanged {
        old_material_id: Option<String>,
        new_material_id: String,
        affected_gate_ids: Vec<String>,
    },
    EntityVersioned {
        version_id: String,
        kind: String,
        entity_id: String,
        version: i64,
    },
    /// 补偿矩阵判定（机器可读规则）。
    CompensationEvaluated {
        spill_version_id: String,
        status: String,
        det: f64,
        cond: Option<f64>,
        candidate_count: usize,
        chosen_index: Option<usize>,
        rule: String,
    },
    /// 群体结果计算；分母钉住父群体版本。
    GateResultRecorded {
        result_id: String,
        gate_version_id: String,
        parent_gate_version_id: Option<String>,
        denominator_result_id: Option<String>,
        count: i64,
        denominator: i64,
        percent: f64,
    },
    /// 上游修订后，下游结果标记过期（不覆盖旧值）。
    MarkedStale {
        stale_result_ids: Vec<String>,
        reason: String,
        changed_version_id: String,
    },
    /// 人工接受/否定/参数选择。
    Adjudicated {
        target_version_id: String,
        decision: String, // accepted | rejected
        chosen_index: Option<usize>,
    },
    /// 撤销上一次裁定（恢复前一分支状态，不删历史）。
    AdjudicationUndone {
        undone_event_id: String,
    },
    /// 同版本并发修改冲突。
    ConflictDetected {
        kind: String,
        entity_id: String,
        expected_version: i64,
        actual_version: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: String,
    pub seq: i64,
    pub branch_id: String,
    pub timestamp: i64,
    pub actor: String,
    pub payload: EventPayload,
    pub prev_event_id: Option<String>,
}
