//! 领域模型：分开版本化的材料、派生结果、事件、分支、候选与裁定。
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// 分开版本化的材料类别：原始通道值 / spillover 元数据 / 补偿矩阵 /
/// 变换参数 / 门多边形 / 父子关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    RawEvents,
    Spillover,
    CompMatrix,
    Transform,
    Gate,
    Relation,
}

impl EntityKind {
    pub fn as_str(&self) -> &'static str {
        use EntityKind::*;
        match self {
            RawEvents => "raw_events",
            Spillover => "spillover",
            CompMatrix => "comp_matrix",
            Transform => "transform",
            Gate => "gate",
            Relation => "relation",
        }
    }

    pub fn parse(s: &str) -> Option<EntityKind> {
        use EntityKind::*;
        Some(match s {
            "raw_events" => RawEvents,
            "spillover" => Spillover,
            "comp_matrix" => CompMatrix,
            "transform" => Transform,
            "gate" => Gate,
            "relation" => Relation,
            _ => return None,
        })
    }
}

/// 一条版本化材料。raw 永远保留导入时的原始表示；
/// 单位/坐标基准随版本一起保存，派生层才做规范化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub entity_id: String,
    pub version: i64,
    pub kind: EntityKind,
    pub supersedes: Option<i64>,
    pub raw: Value,
    pub unit_basis: Value,
    pub branch_id: String,
    pub created_seq: i64,
}

/// 来源引用：指向某个实体版本或某个派生结果版本。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvRef {
    pub ref_kind: String,
    pub id: String,
    pub version: i64,
}

impl ProvRef {
    pub fn entity(e: &Entity) -> ProvRef {
        ProvRef { ref_kind: "entity".into(), id: e.entity_id.clone(), version: e.version }
    }
    pub fn derived(d: &Derived) -> ProvRef {
        ProvRef { ref_kind: "derived".into(), id: d.result_id.clone(), version: d.version }
    }
}

/// 派生结果（补偿后事件、细胞群体）。内容不可变；
/// “过期/撤回”由事件推导，不改写历史行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Derived {
    pub result_id: String,
    pub version: i64,
    pub kind: String,
    pub inputs: Vec<ProvRef>,
    pub payload: Value,
    pub branch_id: String,
    pub created_seq: i64,
}

impl Derived {
    pub fn to_json(&self, status: &str) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        v["status"] = json!(status);
        v
    }
}

/// 机器可读取的事件：所有判定、诊断、过期传播、冲突都落在这里。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub event_id: String,
    pub kind: String,
    pub branch_id: Option<String>,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub branch_id: String,
    pub parent_branch: Option<String>,
    pub parent_seq: Option<i64>,
    pub name: String,
    pub created_seq: i64,
}

/// 多候选：无法唯一决定时全部保留，等待裁定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub cand_id: String,
    pub group_id: String,
    pub payload: Value,
    pub branch_id: String,
    pub created_seq: i64,
}

impl Candidate {
    pub fn to_json(&self, status: &str) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        v["status"] = json!(status);
        v
    }
}

/// 人工裁定（接受/否定/参数调整）。undone 由 undo 事件推导。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Adjudication {
    pub adj_id: String,
    pub target_kind: String,
    pub target_id: String,
    pub decision: String,
    pub params: Value,
    pub branch_id: String,
    pub created_seq: i64,
}
