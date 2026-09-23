use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub type Id = String;

/// Content-addressed stable identity: sha256 of a domain tag + canonical JSON.
/// Identities depend only on content (no timestamps / randomness), so an
/// export -> wipe -> reimport cycle reproduces identical ids and ordering.
pub fn content_id(tag: &str, canonical: &str) -> Id {
    let mut h = Sha256::new();
    h.update(tag.as_bytes());
    h.update(b"\x00");
    h.update(canonical.as_bytes());
    hex::encode(h.finalize())
}

/// serde_json maps are BTreeMap-backed (sorted keys) and floats use ryu
/// shortest-roundtrip formatting, so this serialization is canonical.
pub fn canonical(v: &Value) -> String {
    serde_json::to_string(v).expect("canonical json serialization")
}

pub fn blob_id(bytes: &[u8]) -> Id {
    let mut h = Sha256::new();
    h.update(b"blob\x00");
    h.update(bytes);
    hex::encode(h.finalize())
}

/// A versioned domain object. Each logical entity line (raw channel values,
/// spillover metadata, compensation matrix, transform params, gate polygon,
/// gate tree, population, ...) is versioned independently under `logical_key`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Object {
    pub obj_id: Id,
    pub kind: String,
    pub logical_key: String,
    pub version_no: u32,
    pub prev: Option<Id>,
    pub payload: Value,
}

impl Object {
    pub fn new(kind: &str, logical_key: &str, version_no: u32, prev: Option<Id>, payload: Value) -> Object {
        let body = json!({
            "kind": kind,
            "logical_key": logical_key,
            "version_no": version_no,
            "prev": prev,
            "payload": payload,
        });
        let obj_id = content_id("object", &canonical(&body));
        Object { obj_id, kind: kind.into(), logical_key: logical_key.into(), version_no, prev, payload }
    }
}

/// Append-only event. `payload` carries machine-readable judgment evidence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub event_id: Id,
    pub kind: String,
    pub parents: Vec<Id>,
    pub payload: Value,
    pub seq: u64,
}

pub fn event_id(kind: &str, parents: &[Id], payload: &Value) -> Id {
    let body = json!({"kind": kind, "parents": parents, "payload": payload});
    content_id("event", &canonical(&body))
}

/// Fingerprint of the geometry-relevant content of a gate (channels+vertices),
/// so a pure rename does not invalidate downstream populations.
pub fn gate_fingerprint(channels: &[String], vertices: &[[f64; 2]]) -> Id {
    content_id("gate-geom", &canonical(&json!({"channels": channels, "vertices": vertices})))
}

pub fn transform_fingerprint(transform: &Value) -> Id {
    content_id("transform", &canonical(transform))
}
