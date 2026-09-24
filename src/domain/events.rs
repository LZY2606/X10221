//! 机器可读事件。
//!
//! 事件是唯一事实来源：接受、否定、参数调整都产生新事件；旧事件永不覆盖。
//! 事件通过 `prev_hash` 串成哈希链；分支在某个事件处分叉，回到旧事件即可恢复当时的
//! 依赖版本与可见结论（投影重建）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    /// 导入提交（FCS 字节、通道、spillover 元数据、补偿矩阵、变换、门草案在同一原子事件）。
    ImportCommitted,
    /// spillover/补偿矩阵被修订（引入新的近奇异/奇异矩阵）。
    CompensationRevised,
    /// spillover/补偿矩阵候选被裁定（接受正则化 / 拒绝）。
    CompensationAdjudicated,
    /// 变换参数修订（参数修订）。
    TransformRevised,
    /// 门多边形修订 / 改名（身份不变，ordinal 增加）。
    GateRevised,
    /// 边界点显式裁定（计入/排除），形成裁定分支。
    BoundaryAdjudicated,
    /// 派生结果记录（群成员与分母钉版）。
    PopulationsDerived,
    /// 下游结果被标记过期（不覆盖旧结果）。
    MarkedStale,
    /// 分支创建（两个草案并排）。
    DraftForked,
    /// 撤销某裁定（恢复前一分支结论）。
    DecisionUndone,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::ImportCommitted => "import_committed",
            EventKind::CompensationRevised => "compensation_revised",
            EventKind::CompensationAdjudicated => "compensation_adjudicated",
            EventKind::TransformRevised => "transform_revised",
            EventKind::GateRevised => "gate_revised",
            EventKind::BoundaryAdjudicated => "boundary_adjudicated",
            EventKind::PopulationsDerived => "populations_derived",
            EventKind::MarkedStale => "marked_stale",
            EventKind::DraftForked => "draft_forked",
            EventKind::DecisionUndone => "decision_undone",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "import_committed" => EventKind::ImportCommitted,
            "compensation_revised" => EventKind::CompensationRevised,
            "compensation_adjudicated" => EventKind::CompensationAdjudicated,
            "transform_revised" => EventKind::TransformRevised,
            "gate_revised" => EventKind::GateRevised,
            "boundary_adjudicated" => EventKind::BoundaryAdjudicated,
            "populations_derived" => EventKind::PopulationsDerived,
            "marked_stale" => EventKind::MarkedStale,
            "draft_forked" => EventKind::DraftForked,
            "decision_undone" => EventKind::DecisionUndone,
            _ => return None,
        })
    }
}

/// 机器可读的判定依据（judgement）：每个事件可携带多条。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Judgement {
    /// 规则稳定身份，例如 `rule.no-silent-inverse`、`rule.denominator-pinned-parent`、
    /// `rule.stale-not-overwrite`。
    pub rule_id: String,
    pub status: String,
    pub message: String,
    /// 受影响对象稳定身份。
    pub affected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    /// 链上序号（全局递增）。
    pub seq: i64,
    /// 事件稳定身份：`e{seq}`，跨导出保持。
    pub event_id: String,
    pub branch: String,
    pub kind: String,
    /// 分支前一事件 hash（创世事件为空串）。
    pub prev_hash: String,
    /// 本事件 hash（对规范内容计算）。
    pub hash: String,
    /// 固定案例时间戳（ISO8601 字符串，保证可重放）。
    pub at: String,
    pub payload: Value,
    pub judgements: Vec<Judgement>,
}

impl EventRecord {
    /// 被本事件引用的上游对象版本（依赖边，用于失效范围计算）。
    pub fn references(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let p = &self.payload;
        if let Some(arr) = p.get("depends_on").and_then(|v| v.as_array()) {
            for item in arr {
                if let (Some(k), Some(r)) = (item.get("kind").and_then(|v| v.as_str()), item.get("rev").and_then(|v| v.as_str())) {
                    out.push((k.to_string(), r.to_string()));
                }
            }
        }
        out
    }
}
