//! Stable content-addressed identifiers (FNV-1a 64-bit, hex).
//!
//! IDs are derived from canonical content so that export -> delete -> import
//! preserves identity without surrogate keys.

const OFFSET: u64 = 0xcbf29ce484222325;
const PRIME: u64 = 0x100000001b3;

pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = OFFSET;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

pub fn content_id(prefix: &str, canonical: &str) -> String {
    format!("{}_{:016x}", prefix, fnv1a(canonical.as_bytes()))
}
