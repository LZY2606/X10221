use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

pub fn now_ts() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("2026-09-29T{:02}:{:02}:{:02}Z", millis / 3_600_000 % 24, millis / 60000 % 60, millis / 1000 % 60)
}

/// Deterministic id derived from the chain of stable inputs, so export ->
/// delete -> import preserves every identity.
pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\x1f");
    }
    format!("{prefix}_{}", hex::encode(h.finalize())[..20].to_string())
}
