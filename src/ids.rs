use serde::Serialize;
use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

pub fn content_id(kind: &str, payload: &impl Serialize) -> String {
    let canonical = serde_json::to_string(payload).expect("content id payload serializable");
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(b":");
    h.update(canonical.as_bytes());
    format!("{}_{}", kind, hex::encode(h.finalize())[..16].to_string())
}

pub fn raw_value_id(draft_id: &str, event_index: u32, channel_id: &str, stage: &str) -> String {
    format!("val-{}-{}-{}-{}", draft_id, event_index, channel_id, stage)
}
