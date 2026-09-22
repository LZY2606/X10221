use crate::domain;
use crate::store::{Artifact, Store};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("cases/flow_boundary_chamber")
}

fn read(dir: &Path, file: &str) -> Result<(String, Value), String> {
    let text = std::fs::read_to_string(dir.join(file)).map_err(|e| format!("{}: {}", file, e))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {}", file, e))?;
    Ok((text, v))
}

fn put(store: &Store, kind: &str, name: &str, payload: Value, raw: Option<String>, unit: Option<String>, source: &str, branch: &str, seq: i64) -> Artifact {
    let version = store.latest(kind, name, branch).map(|a| a.version + 1).unwrap_or(1);
    let a = Artifact::<'static>placeholder;
    a
}
