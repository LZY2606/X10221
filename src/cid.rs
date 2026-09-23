use serde::Serialize;
use sha2::{Digest, Sha256};

/// Content identity: a stable, deterministic, human-recognisable id derived
/// from the *canonical* JSON representation of a piece of content. The same
/// content on any branch or database yields the same id, which is what makes
/// export -> delete -> import preserve identities, order and provenance.
pub fn cid_of<T: Serialize>(kind: &str, value: &T) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"flowline/v1/");
    hasher.update(kind.as_bytes());
    hasher.update(b"/");
    let canonical = serde_json::to_vec(value).expect("cid serialization");
    hasher.update(canonical);
    format!("cid_{}_{}", kind, hex::encode(hasher.finalize())[..16])
}

/// Hash an ordered list of identity strings (e.g. membership vector ids).
pub fn cid_of_ids(kind: &str, ids: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"flowline/v1/list/");
    hasher.update(kind.as_bytes());
    hasher.update(b"/");
    for id in ids {
        hasher.update(id.as_bytes());
        hasher.update(b"\n");
    }
    format!("cid_{}_{}", kind, hex::encode(hasher.finalize())[..16])
}
