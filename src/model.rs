use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 六类分开版本化的工件种类。
pub const ARTIFACT_KINDS: [&str; 6] = [
    "raw_channel_values",
    "spillover_metadata",
    "compensation_matrix",
    "transform_params",
    "gate_polygons",
    "parent_child_relations",
];

pub const STATUS_CURRENT: &str = "current";
pub const STATUS_STALE: &str = "stale";
pub const STATUS_BLOCKED: &str = "blocked";
pub const STATUS_CANDIDATE: &str = "candidate";
pub const STATUS_ACCEPTED: &str = "accepted";
pub const STATUS_REJECTED: &str = "rejected";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub base_seq: i64,
    pub head_seq: i64,
    pub status: String,
    pub locked: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub branch_id: String,
    pub kind: String,
    pub version: i64,
    pub payload: Value,
    pub event_seq: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Derived {
    pub id: String,
    pub branch_id: String,
    pub kind: String,
    /// {"artifacts": {kind: version}, "derived": [derived_id, ...]}
    pub deps: Value,
    pub payload: Value,
    pub status: String,
    pub event_seq: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub id: String,
    pub branch_id: String,
    pub kind: String,
    pub payload: Value,
    pub supersedes: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub branch_id: String,
    pub target_id: String,
    pub ruling: String, // accept | reject | adjust
    pub payload: Value, // 含 prev_status 以支持撤销
    pub event_seq: i64,
    pub undone: bool,
    pub created_at: String,
}

pub fn now_iso() -> String {
    // 单调时间戳，足够测试与展示使用（毫秒级，来自系统时钟）。
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let millis = dur.subsec_millis();
    // 简化的 UTC 换算（不引入 chrono）。
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y,
        m,
        d,
        tod / 3600,
        (tod / 60) % 60,
        tod % 60,
        millis
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
