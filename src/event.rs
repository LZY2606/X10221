//! 事件溯源：所有导入、计算、裁定、视图操作都产生不可变事件。
//! 回到任意旧事件时恢复当时依赖版本与可见结论，而不是用最新结果覆盖历史。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// 源材料版本写入（六类之一）。
    MaterialImported,
    /// 派生层计算出补偿/群体结果（或矩阵奇异阻断）。
    Derivation,
    /// 近奇异矩阵/分母/过期等判定依据（机器可读）。
    Judgement,
    /// 接受/否定/参数调整等人工裁定。
    Decision,
    /// 分支创建。
    BranchForked,
    /// 视图选择/候选切换等审阅动作（不改原数据）。
    ViewAction,
    /// 同版本并发修改冲突。
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// 稳定事件身份（内容寻址，见 ids::event_id）。
    pub id: String,
    /// 分支 id。
    pub branch: String,
    /// 分支内单调序号（稳定顺序）。
    pub seq: u32,
    pub kind: EventKind,
    /// 人读标题。
    pub summary: String,
    /// 机器可读判定依据：规则代码、受影响对象、输入片段引用、状态。
    pub basis: serde_json::Value,
    /// 该事件发生时分支可见的依赖版本快照（node/material -> version id）。
    pub dependency_snapshot: serde_json::Value,
    /// 父事件 id（分支链）。
    pub parent_event_id: Option<String>,
    /// 乐观并发：客户端保存时基于的最后事件 id；同版本并发冲突时比对。
    pub base_event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub label: String,
    /// 分叉点事件 id（主分支为 null）。
    pub forked_from_event: Option<String>,
    /// 该分支是否保留（否定结果也保留）。
    pub adopted: bool,
}
