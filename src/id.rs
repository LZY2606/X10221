//! Deterministic, content-derived stable identifiers.
//!
//! Every object that can appear in the review UI gets an id derived from its
//! semantic bytes (never from a rowid or an insertion timestamp). That is what
//! makes export -> delete -> import preserve identity, order and provenance.

use sha2::{Digest, Sha256};

pub fn digest_hex(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0x1f]);
        }
        h.update(p.len().to_le_bytes());
        h.update(p);
    }
    hex::encode(h.finalize())
}

/// `"<kind>_<12 hex>"`. Kind carries the type so ids never collide across
/// raw events, matrices, gates, events, results ...
pub fn obj_id(kind: &str, bytes: &[u8]) -> String {
    format!("{}_{}", kind, &digest_hex(&[kind.as_bytes(), bytes])[..12])
}

pub fn stream_id(kind: &str, key: &str) -> String {
    format!("{}:{}", kind, key)
}
