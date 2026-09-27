//! 领域对象：来源与解释分离，所有派生量携带坐标基准/单位。

use serde::{Deserialize, Serialize};

pub type Json = serde_json::Value;

/// 稳定身份：来源材料使用 `art_*` 逻辑身份 + 版本；派生结果使用
/// `res_*` 身份，跨导出/导入/重算保持不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// FCS 事件表（原始通道值，保留原始表示）
    Fcs,
    /// spillover 元数据（单染对照、荧光素-检测器映射）
    Spillover,
    /// 补偿矩阵（由单染对照推导或人工给出，是独立来源）
    Compensation,
    /// 变换参数（asinh 线性化参数、坐标基准）
    Transform,
    /// 门多边形集合 + 父子关系
    Gates,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Fcs => "fcs",
            ArtifactKind::Spillover => "spillover",
            ArtifactKind::Compensation => "compensation",
            ArtifactKind::Transform => "transform",
            ArtifactKind::Gates => "gates",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "fcs" => ArtifactKind::Fcs,
            "spillover" => ArtifactKind::Spillover,
            "compensation" => ArtifactKind::Compensation,
            "transform" => ArtifactKind::Transform,
            "gates" => ArtifactKind::Gates,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// 导入/安装一批来源版本（可能含拒绝）
    Import,
    /// 派生层重算完成
    Derived,
    /// 近奇异/奇异矩阵等机器判定（采纳与否都保留）
    Diagnostic,
    /// 人工裁定（接受候选 / 否定 / 参数调整）
    Adjudicate,
    /// 上游修订导致的下游失效传播（不覆盖旧结果）
    Stale,
    /// 同版本并发修改冲突
    Conflict,
    /// 门改名（身份不变、仅标签变化）
    GateRenamed,
    /// 回退到旧事件：当时的依赖版本与可见结论恢复
    Revert,
    /// 分支建立（对同一份材料的新解释）
    Fork,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            EventType::Import => "import",
            EventType::Derived => "derived",
            EventType::Diagnostic => "diagnostic",
            EventType::Adjudicate => "adjudicate",
            EventType::Stale => "stale",
            EventType::Conflict => "conflict",
            EventType::GateRenamed => "gate_renamed",
            EventType::Revert => "revert",
            EventType::Fork => "fork",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRow {
    pub id: String,
    pub seq: i64,
    pub branch: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub detail: Json,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactVersionRow {
    pub art_id: String,
    pub kind: String,
    pub version: i64,
    pub label: String,
    pub payload: Json,
    pub status: String,
    pub diagnostic: Option<Json>,
    pub created_event: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchRow {
    pub name: String,
    pub label: String,
    pub parent_branch: Option<String>,
    pub forked_from_event: Option<String>,
    pub head_event: Option<String>,
    pub base_version: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultRow {
    pub id: String,
    pub branch: String,
    pub scope: String,
    pub label: String,
    pub payload: Json,
    /// 该结论依赖的全部来源版本（钉住版本）
    pub deps: Json,
    pub parent_result: Option<String>,
    /// 父群结论身份 + 钉住的父结果版本（百分比分母来源）
    pub parent_result_version: Option<String>,
    pub status: String,
    pub produced_event: String,
    pub superseded_by: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateRow {
    pub id: String,
    pub branch: String,
    pub scope: String,
    pub label: String,
    pub payload: Json,
    pub rationale: String,
    pub status: String,
    pub produced_event: String,
    pub adjudicated_event: Option<String>,
    pub created_at: i64,
}

/// 机器可读判定依据（事件 detail 中的标准结构）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgmentRecord {
    pub code: String,
    pub severity: String,
    pub subject: String,
    pub message: String,
    pub evidence: Json,
    pub affected: Vec<String>,
    pub candidates: Vec<String>,
    pub outcome: String,
}
