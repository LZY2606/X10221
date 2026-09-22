use sha2::{Digest, Sha256};

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let d = h.finalize();
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn short(bytes: &[u8]) -> String {
    sha256_hex(bytes)[..12].to_string()
}

/// 材料 id：内容寻址（mat_<sha12>），跨导出/导入保持稳定。
pub fn material_id(sha: &str) -> String {
    format!("mat_{}", &sha[..12])
}

/// 实体版本 id：由分支、实体、版本与内容决定。
pub fn version_id(branch: &str, kind: &str, entity: &str, version: i64, data: &str) -> String {
    let key = format!("{branch}|{kind}|{entity}|v{version}|{data}");
    format!("ver_{kind}_{}_{}", entity.replace('.', "_"), short(key.as_bytes()))
}

pub fn result_id(branch: &str, gate_version_id: &str, lineage_version_id: &str, seq: i64) -> String {
    let key = format!("result|{branch}|{gate_version_id}|{lineage_version_id}|{seq}");
    format!("res_{}", short(key.as_bytes()))
}

pub fn event_id(branch: &str, seq: i64, payload: &str, prev: Option<&str>) -> String {
    let key = format!("event|{branch}|{seq}|{payload}|{}", prev.unwrap_or(""));
    format!("evt_{}", short(key.as_bytes()))
}

pub fn branch_id(name: &str, created_at: i64) -> String {
    let key = format!("branch|{name}|{created_at}");
    format!("br_{}", short(key.as_bytes()))
}

pub fn decision_id(branch: &str, target: &str, created_at: i64) -> String {
    let key = format!("decision|{branch}|{target}|{created_at}");
    format!("dec_{}", short(key.as_bytes()))
}
