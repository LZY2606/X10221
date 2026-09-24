use serde::{Deserialize, Serialize};

/// 一条版本化记录：原始通道值、spillover 元数据、补偿矩阵、变换参数、
/// 门多边形、父子关系等各自成线（line），分开版本化。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Version {
    pub id: String,
    pub kind: String,
    pub line_id: String,
    pub branch: String,
    pub seq: i64,
    pub base_id: Option<String>,
    pub payload: serde_json::Value,
    pub has_raw: bool,
    pub created_ms: i64,
    /// fresh | stale —— 过期只标记，不覆盖。
    pub status: String,
}

/// 机器可读的判定事件。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub seq: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_ms: i64,
}

/// 审阅页/时间旅行视图：某一时刻可见的版本与结论。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateView {
    pub versions: Vec<Version>,
    pub events: Vec<Event>,
    /// 截至该时刻被标记过期的版本 id（由事件重放得出，不改写历史版本本身）。
    pub stale_ids: Vec<String>,
}
