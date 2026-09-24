use crate::store::{ApiErr, Store};
use serde_json::{json, Value};

pub const MANIFEST: &str = include_str!("../casepack/manifest.json");
pub const STEPS: &str = include_str!("../casepack/steps.json");

const BASE_FCS: &str = include_str!("../casepack/baseline/fcs_events.json");
const BASE_SPILL: &str = include_str!("../casepack/baseline/spillover.json");
const BASE_COMP: &str = include_str!("../casepack/baseline/compensation_matrix.json");
const BASE_TRANS: &str = include_str!("../casepack/baseline/transform_params.json");
const BASE_GATES: &str = include_str!("../casepack/baseline/gate_polygons.json");
const BASE_TREE: &str = include_str!("../casepack/baseline/gate_tree.json");

const REV_TRANS_V2: &str = include_str!("../casepack/revisions/transform_params_v2.json");
const REV_FCS_V2: &str = include_str!("../casepack/revisions/fcs_events_v2.json");
const REV_COMP_V2: &str = include_str!("../casepack/revisions/compensation_matrix_v2.json");
const REV_COMP_V3: &str = include_str!("../casepack/revisions/compensation_matrix_v3.json");
const REV_GATES_V2: &str = include_str!("../casepack/revisions/gate_polygons_v2.json");
const REV_TREE_V2: &str = include_str!("../casepack/revisions/gate_tree_v2.json");

fn doc(s: &str) -> Result<Value, ApiErr> {
    Ok(serde_json::from_str(s)?)
}

fn revision_doc(r: &str) -> Result<Value, ApiErr> {
    match r {
        "revisions/transform_params_v2.json" => doc(REV_TRANS_V2),
        "revisions/fcs_events_v2.json" => doc(REV_FCS_V2),
        "revisions/compensation_matrix_v2.json" => doc(REV_COMP_V2),
        "revisions/compensation_matrix_v3.json" => doc(REV_COMP_V3),
        "revisions/gate_polygons_v2.json" => doc(REV_GATES_V2),
        "revisions/gate_tree_v2.json" => doc(REV_TREE_V2),
        other => Err(ApiErr::bad(format!("unknown revision ref: {other}"))),
    }
}

pub fn seed(store: &Store) -> Result<Value, ApiErr> {
    store.reset()?;
    let mut committed = Vec::new();
    for (kind, raw) in [
        ("fcs_events", BASE_FCS),
        ("spillover", BASE_SPILL),
        ("compensation_matrix", BASE_COMP),
        ("transform_params", BASE_TRANS),
        ("gate_polygons", BASE_GATES),
        ("gate_tree", BASE_TREE),
    ] {
        let content = doc(raw)?;
        let r = store.commit_material(kind, content, Some(raw.to_string()))?;
        committed.push(r["id"].clone());
    }
    store.create_draft("main", "草案A 主线")?;
    store.create_draft("alt", "草案B 对照")?;
    let (_, alt_head) = store.get_draft("alt")?;
    store.save_ruling("alt", alt_head, "boundary_policy", "exclude")?;
    let d1 = store.derive("main")?;
    let d2 = store.derive("alt")?;
    Ok(json!({
        "seeded": committed,
        "drafts": ["main", "alt"],
        "derive": [d1["seq"].clone(), d2["seq"].clone()],
    }))
}

pub fn replay(store: &Store) -> Result<Value, ApiErr> {
    let steps: Value = serde_json::from_str(STEPS)?;
    let mut log = Vec::new();
    for step in steps["steps"].as_array().cloned().unwrap_or_default() {
        let op = step["op"].as_str().unwrap_or("");
        let entry = match op {
            "commit" => {
                let kind = step["kind"].as_str().unwrap_or("");
                let content = revision_doc(step["ref"].as_str().unwrap_or(""))?;
                let r = store.commit_material(kind, content, None)?;
                json!({"step": step["id"], "op": op, "material": r["id"], "staled": r["staled"]})
            }
            "derive" => {
                let draft = step["draft"].as_str().unwrap_or("main");
                let r = store.derive(draft)?;
                json!({"step": step["id"], "op": op, "draft": draft, "seq": r["seq"], "comp_mode": r["comp_mode"]})
            }
            "ruling" => {
                let draft = step["draft"].as_str().unwrap_or("main");
                let (_, head) = store.get_draft(draft)?;
                let r = store.save_ruling(
                    draft,
                    head,
                    step["target"].as_str().unwrap_or(""),
                    step["decision"].as_str().unwrap_or(""),
                )?;
                json!({"step": step["id"], "op": op, "draft": draft, "seq": r["seq"]})
            }
            "conflict_demo" => {
                let draft = step["draft"].as_str().unwrap_or("main");
                let target = step["target"].as_str().unwrap_or("");
                let decisions: Vec<String> = step["decisions"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let (_, head) = store.get_draft(draft)?;
                let first = store.save_ruling(draft, head, target, &decisions[0])?;
                let conflict = match store.save_ruling(draft, head, target, &decisions[1]) {
                    Err(ApiErr::Conflict(c)) => c,
                    other => return Err(ApiErr::bad(format!("expected conflict, got {other:?}"))),
                };
                store.append_event(
                    Some(draft),
                    "conflict",
                    json!({
                        "rule": "same_version_concurrent_modification",
                        "draft": draft,
                        "base_seq": head,
                        "winner": {"target": target, "decision": decisions[0]},
                        "loser": {"target": target, "decision": decisions[1]},
                        "detail": conflict,
                    }),
                )?;
                json!({
                    "step": step["id"], "op": op, "draft": draft,
                    "accepted_seq": first["seq"], "conflict_at_seq": head,
                })
            }
            "note" => json!({"step": step["id"], "op": op}),
            other => return Err(ApiErr::bad(format!("unknown step op: {other}"))),
        };
        log.push(entry);
    }
    Ok(json!({"replayed": log}))
}
