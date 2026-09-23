//! Stable content-addressed identity.
//!
//! Artifact ids are SHA-256 over `kind + canonical JSON`, so the same content
//! imported twice (or exported, deleted, re-imported) keeps the same identity,
//! order-independent inside maps but order-preserving for arrays.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::domain::{Artifact, ArtifactKind};

fn canonicalize(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut sorted: Vec<(&String, &Value)> = map.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            let mut out = serde_json::Map::new();
            for (k, val) in sorted {
                out.insert(k.clone(), canonicalize(val));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

pub fn canonical_json_bytes<T: serde::Serialize>(t: &T) -> Vec<u8> {
    let v = serde_json::to_value(t).expect("serializable");
    serde_json::to_vec(&canonicalize(&v)).expect("canonical serializable")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Stable id of an artifact version.
pub fn artifact_id(kind: ArtifactKind, payload: &impl serde::Serialize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(canonical_json_bytes(payload));
    hex::encode(hasher.finalize())
}

pub fn artifact_ref(a: &Artifact) -> crate::domain::ArtifactRef {
    crate::domain::ArtifactRef {
        kind: a.kind(),
        id: artifact_id(a.kind(), a),
    }
}

/// Deterministic id of a population result version.
pub fn result_id(gate_id: &str, inputs: &crate::domain::InputsRef) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"result\0");
    hasher.update(gate_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(canonical_json_bytes(inputs));
    hex::encode(hasher.finalize())
}

/// Stable event id: hash of branch + seq + type + canonical payload + inputs.
pub fn event_id(
    branch: &str,
    seq: u64,
    etype: &str,
    payload: &impl serde::Serialize,
    inputs: Option<&crate::domain::InputsRef>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"event\0");
    hasher.update(branch.as_bytes());
    hasher.update(b"\0");
    hasher.update(seq.to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(etype.as_bytes());
    hasher.update(b"\0");
    hasher.update(canonical_json_bytes(payload));
    if let Some(inp) = inputs {
        hasher.update(b"\0");
        hasher.update(canonical_json_bytes(inp));
    }
    hex::encode(hasher.finalize())
}
