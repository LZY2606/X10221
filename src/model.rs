//! 领域模型：来源材料（分开版本化）、事件、派生结果、裁定、分支、冲突。
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const UNIT_ADC: &str = "ADC_count_18bit";
pub const COORD_LINEAR: &str = "linear_adc";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceMaterial {
    pub id: String,
    pub kind: String, // raw_channels|spillover|comp_matrix|transform_params|gate_polygon|gate_hierarchy
    pub name: String,
    pub version: i64,
    pub unit: Option<String>,
    pub coord: Option<String>,
    pub payload: Value, // 原始表示，导入时不做规范化
    pub created_seq: i64,
    pub supersedes: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub id: String,
    pub ts_ms: i64,
    pub branch: String,
    pub kind: String,   // import|comp_check|gate_eval|decision|undo|stale_mark|conflict|branch|export|reset
    pub status: String, // ok|warning|rejected|stale|conflict|accepted|rejected_decision
    pub basis: Value,   // 机器可读的判定依据
    pub inputs: Value,  // 输入片段
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DerivedResult {
    pub id: String,
    pub branch: String,
    pub kind: String, // compensated|population
    pub population: Option<String>,
    pub version: i64,
    pub deps: Value,              // [{id,kind,name,version}]
    pub denominator: Option<Value>, // 钉住的父群版本 {population,result_id,version,count}
    pub value: Value,
    pub status: String, // current|stale
    pub created_seq: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub branch: String,
    pub target_event: Option<String>,
    pub action: String, // accept|reject|adjust
    pub candidate: Option<String>,
    pub params: Option<Value>,
    pub prev_head_seq: i64,
    pub event_seq: i64,
    pub undone_by: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub parent: Option<String>,
    pub base_seq: i64,
    pub head_seq: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conflict {
    pub id: String,
    pub branch: String,
    pub base_version: i64,
    pub ours: Value,
    pub theirs: Value,
    pub status: String, // open
    pub event_seq: i64,
}
