//! Stable, content-derived identities.
//!
//! Material versions and derived results are addressed by SHA-256 of their
//! canonical JSON content. Re-importing byte-identical material yields the same
//! version id, and a round trip through export/import keeps every identity,
//! ordering and lineage relation.

use serde::Serialize;
use sha2::{Digest, Sha256};

pub fn content_hash<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("serialize for hashing");
    let mut hasher = Sha256::new();
    hasher.update(b"flowline/v1\n");
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}
