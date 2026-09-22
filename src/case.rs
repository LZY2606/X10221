//! 仓库内置离线案例包的结构与装载/重放。

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct CaseFile {
    pub id: String,
    pub title: String,
    #[allow(dead_code)]
    pub unit_system: serde_json::Value,
    pub channels: Vec<serde_json::Value>,
    pub events: Vec<serde_json::Value>,
    pub patch_events: Vec<serde_json::Value>,
    pub spillover_meta: serde_json::Value,
    pub compensation_matrices: serde_json::Value,
    pub transforms: serde_json::Value,
    pub gates: serde_json::Value,
    pub links: serde_json::Value,
    pub scenarios: serde_json::Value,
    pub expectations: serde_json::Value,
}

impl CaseFile {
    pub fn from_str(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| format!("案例包解析失败: {e}"))
    }
}
