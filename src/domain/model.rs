//! 持久化领域模型（分开版本化的六大来源 + 派生群结果 + 裁定）。

use crate::domain::gate::PolygonGate;
use crate::domain::matrix::LegalCandidate;
use crate::domain::transform::TransformSpec;
use serde::{Deserialize, Serialize};

/// 通道描述：名称 + 单位/坐标基准（导入时保留原始表示）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub name: String,
    /// 原始表示，例如 "FITC fluorescence" / "FSC, linear units"。
    pub raw_label: String,
    pub unit: String,
    pub range_max: f64,
}

/// spillover 元数据（单染对照导出的荧光串扰描述），与补偿矩阵分开版本化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpilloverMeta {
    pub stable_id: String,
    pub ordinal: u64,
    pub rev: String,
    /// 荧光通道顺序（列/行）。
    pub channels: Vec<String>,
    /// 单染对照事件来源 blob（稳定引用）。
    pub single_stain_blobs: Vec<String>,
    /// 仪器/面板注释。
    pub source_note: String,
}

/// 补偿矩阵版本。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompensationMatrix {
    pub stable_id: String,
    pub ordinal: u64,
    pub rev: String,
    pub channels: Vec<String>,
    /// spillover 矩阵 S（测量串扰）。
    pub spillover: Vec<Vec<f64>>,
    /// 来源 spillover 元数据版本 rev。
    pub source_spillover_rev: String,
    pub rcond: f64,
    pub status: String,
    /// 矩阵来自哪个候选（`exact_inverse` / `ridge_regularized`），近奇异时必须有人工裁定。
    pub chosen_candidate: Option<String>,
    pub candidates: Vec<LegalCandidate>,
    pub diagnostics: Vec<String>,
}

impl CompensationMatrix {
    /// 选定的补偿（求逆）矩阵；Rejected 或未裁定时为 None。
    pub fn comp_matrix(&self) -> Option<&Vec<Vec<f64>>> {
        let key = self.chosen_candidate.as_deref().unwrap_or("exact_inverse");
        self.candidates.iter().find(|c| c.kind == key).map(|c| &c.matrix)
    }
}

/// 某通道的变换参数版本。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformParam {
    pub stable_id: String,
    pub ordinal: u64,
    pub rev: String,
    pub channel: String,
    pub spec: TransformSpec,
}

/// 门版本（多边形 + 父子关系在此版本化）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateVersion {
    pub rev: String,
    pub gate: PolygonGate,
}

/// 分母来源：群百分比的分母必须钉住父群的特定版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenominatorRef {
    /// 父群门 stable_id（根群指向特殊 `gate#root`）。
    pub parent_gate_id: String,
    /// 父门版本 rev。
    pub parent_gate_rev: String,
    /// 父群使用的补偿矩阵 rev（根群亦记录）。
    pub compensation_rev: String,
    /// 父群结果事件集合快照身份（内容指纹）。
    pub parent_membership_rev: String,
    /// 父群基数。
    pub parent_count: usize,
}

/// 单个门群的一次派生结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PopulationResult {
    pub gate_id: String,
    pub gate_rev: String,
    pub gate_name: String,
    pub parent_gate_id: Option<String>,
    pub compensation_rev: String,
    /// 成员事件稳定 ID（顺序固定）。
    pub inside_event_ids: Vec<String>,
    /// 边界上的事件（歧义，未计入 inside）。
    pub boundary_event_ids: Vec<String>,
    /// 无法判定（坐标缺失/NaN）事件。
    pub unknown_event_ids: Vec<String>,
    pub count: usize,
    pub percent: f64,
    pub denominator: DenominatorRef,
    pub result_rev: String,
}

/// 派生失效标记：上游版本变化后，下游结果标记过期而不覆盖。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaleMark {
    pub gate_id: String,
    pub stale_gate_rev: String,
    /// 触发失效的上游对象（门/矩阵）rev。
    pub because_source_rev: String,
    pub because_kind: String,
    /// 影响范围链（从上游到该门）。
    pub affected_path: Vec<String>,
}

/// 人工裁定记录（接受 / 否定 / 参数调整）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Adjudication {
    pub id: String,
    pub kind: String,
    pub decision: String,
    pub target_id: String,
    pub base_rev: String,
    pub note: String,
}
