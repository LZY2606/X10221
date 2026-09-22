//! Deterministic stable identities (FNV-1a 64-bit over content bytes).

pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn doc_id(bytes: &[u8]) -> String {
    format!("doc-{:016x}", fnv1a_64(bytes))
}

pub fn members_hash(ids: &[String]) -> String {
    let mut joined = ids.join("\n");
    joined.push('\n');
    format!("members-{:016x}", fnv1a_64(joined.as_bytes()))
}

pub fn candidate_id(branch: &str, seq: i64, subject: &str) -> String {
    let raw = format!("{branch}|{seq}|{subject}");
    format!("cand-{:016x}", fnv1a_64(raw.as_bytes()))
}
