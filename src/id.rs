//! Deterministic, export-stable identity helpers.
//!
//! Every persistent identity is derived from content (SHA-256) so that an
//! export -> wipe -> import round trip reproduces exactly the same ids, the
//! same ordering and the same provenance links. Runtime-only sequence ids
//! (audit events) use a monotonic generator seeded deterministically per
//! import batch.
use sha2::{Digest, Sha256};

pub fn digest_hex(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Short (16 char) stable id with a readable prefix.
pub fn sid(prefix: &str, material: &str) -> String {
    let hex = digest_hex(material.as_bytes());
    format!("{prefix}_{}", &hex[..16])
}

/// Deterministic id for an artifact version.
pub fn version_id(artifact_id: &str, ordinal: u32) -> String {
    sid("ver", &format!("{artifact_id}#v{ordinal}"))
}

/// Deterministic id for a derived result row.
pub fn derived_id(kind: &str, key: &str) -> String {
    sid("der", &format!("{kind}:{key}"))
}
