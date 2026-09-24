//! Stable, content-addressed identifiers.
//!
//! Every artifact, event and derived result gets a deterministic id derived
//! from its branch/inputs and canonical payload. Replaying the same inputs on an
//! empty database (e.g. after export -> delete -> import) therefore reproduces
//! every id, order and provenance link byte-for-byte.

use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

fn digest16(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        h.update((i as u64).to_le_bytes());
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    hex::encode(h.finalize())[..16].to_string()
}

pub fn artifact_id(kind: &str, canonical_json: &str) -> String {
    format!("art_{}_{}", kind_prefix(kind), digest16(&[kind, canonical_json]))
}

pub fn event_id(branch: &str, seq: u64, kind: &str, payload_json: &str, parents: &[String]) -> String {
    let parents_joined = parents.join(",");
    format!(
        "evt_{}",
        digest16(&[branch, &seq.to_string(), kind, payload_json, &parents_joined])
    )
}

pub fn derived_id(prefix: &str, parts: &[&str]) -> String {
    format!("der_{}_{}", prefix, digest16(parts))
}

fn kind_prefix(kind: &str) -> &str {
    match kind {
        "raw" => "raw",
        "spillover" => "spl",
        "comp_matrix" => "mat",
        "transform" => "xfo",
        "gate" => "gat",
        "lineage" => "lin",
        "pop" => "pop",
        "comp_run" => "run",
        "judgment" => "jdg",
        _ => "obj",
    }
}
