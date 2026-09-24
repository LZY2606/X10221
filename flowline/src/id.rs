use sha2::{Digest, Sha256};

/// Deterministic, content-addressed identity. Survives export/delete/reimport
/// because the id is a pure function of canonical artifact content.
pub fn content_id(prefix: &str, canonical_bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    h.update(b":");
    h.update(canonical_bytes);
    format!("{}_{}", prefix, hex::encode(&h.finalize())[..24].to_string())
}
