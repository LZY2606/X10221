use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 分开版本化的来源材料：原始通道值 / spillover 元数据 / 补偿矩阵 /
/// 变换参数 / 门多边形 / 父子关系，各自独立递增版本。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemVersion {
    pub item_id: String,
    pub kind: String,
    pub version: i64,
    pub branch: String,
    pub payload: Value,
    /// 导入时的原始表示（原样保留，派生层才做规范化）。
    pub raw_repr: String,
    pub created_seq: i64,
}

/// 机器可读取的事件：每一次导入、判定、裁定、冲突都落库。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub id: String,
    pub kind: String,
    pub branch: String,
    pub actor: String,
    pub payload: Value,
}

/// 派生群体结果：百分比分母钉住父群版本。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Population {
    pub pop_id: String,
    pub result_version: i64,
    pub branch: String,
    pub gate_id: String,
    pub gate_version: i64,
    pub comp_version: Option<i64>,
    pub transform_version: i64,
    pub parent_pop_id: Option<String>,
    pub parent_result_version: Option<i64>,
    pub count_in: i64,
    pub count_boundary: i64,
    pub denom_count: Option<i64>,
    pub pct: Option<f64>,
    /// fresh | stale | blocked
    pub status: String,
    pub created_seq: i64,
}

/// 人工裁定（接受 / 否定 / 参数调整），可撤销。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ruling {
    pub id: String,
    pub target: String,
    pub decision: String,
    pub branch: String,
    pub actor: String,
    pub seq: i64,
    /// 撤销恢复所需的前一状态（分支头指针）。
    pub prev_state: Value,
    pub reverted: bool,
}

/// 同版本并发修改冲突：后提交者看到双方差异。
#[derive(Clone, Debug, Serialize)]
pub struct Conflict {
    pub item_id: String,
    pub branch: String,
    pub head_version: i64,
    pub base_version: i64,
    pub head_payload: Value,
    pub offered_payload: Value,
}

pub const KINDS: [&str; 6] = [
    "raw_channels",
    "spillover_meta",
    "comp_matrix",
    "transform_params",
    "gate_polygon",
    "gate_tree",
];
