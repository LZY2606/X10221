use crate::db::{
    all_artifacts, append_event, artifact_exists, get_artifact, insert_artifact, list_events,
    list_heads, record_checkpoint, reset_branch_head, ConflictError, Database, DbError,
};
use crate::domain::{arcsinh, det2, fnv1a_64, invert2, multiply2, number, point_in_polygon};
use serde_json::{json, Map, Value};

pub const BRANCH_MAIN: &str = "main";
pub const BRANCH_DRAFT_A: &str = "draft-a";
pub const BRANCH_DRAFT_B: &str = "draft-b";
pub const CASE_ID: &str = "flow-boundary-suite";

pub fn source_fingerprint(source: &str) -> String {
    fnv1a_64(source.as_bytes())
}

fn canonical_json(value: &Value) -> String {
    let mut buffer = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"  ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    serde::Serialize::serialize(value, &mut serializer).expect("json serialization");
    String::from_utf8(buffer).expect("utf8 json")
}

fn payload_fingerprint(payload: &Value) -> String {
    fnv1a_64(canonical_json(payload).as_bytes())
}

fn matrix2(value: &Value) -> [[f64; 2]; 2] {
    let rows = value.as_array().expect("matrix rows");
    [
        [
            rows[0][0].as_f64().unwrap(),
            rows[0][1].as_f64().unwrap(),
        ],
        [
            rows[1][0].as_f64().unwrap(),
            rows[1][1].as_f64().unwrap(),
        ],
    ]
}

fn polygon(value: &Value) -> Vec<[f64; 2]> {
    value
        .as_array()
        .expect("polygon array")
        .iter()
        .map(|point| [point[0].as_f64().unwrap(), point[1].as_f64().unwrap()])
        .collect()
}

fn channel_index(channels: &[Value], channel_id: &str) -> Option<usize> {
    channels
        .iter()
        .position(|channel| channel["id"] == channel_id)
}

fn numeric_event_value(event: &Value, channel_id: &str) -> Option<f64> {
    let ordered = event["ordered_channels"].as_array()?;
    let index = ordered.iter().position(|value| value == channel_id)?;
    number(&event["values"][index])
}

fn event_raw_value(event: &Value, channel_id: &str) -> Value {
    let ordered = event["ordered_channels"].as_array().expect("ordered channels");
    let index = ordered
        .iter()
        .position(|value| value == channel_id)
        .expect("channel present");
    event["values"][index].clone()
}

fn artifact_id(kind: &str, version: &str) -> String {
    format!("{kind}/{version}")
}

fn insert_versioned(
    transaction: &rusqlite::Transaction<'_>,
    kind: &str,
    version: &str,
    mut payload: Value,
) -> Result<String, DbError> {
    let id = payload
        .get("id")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| artifact_id(kind, version));
    payload["artifact_id"] = Value::String(id.clone());
    payload["kind"] = Value::String(kind.to_string());
    payload["version"] = Value::String(version.to_string());
    let fingerprint = payload_fingerprint(&payload);
    payload["fingerprint"] = Value::String(fingerprint.clone());
    if !artifact_exists(transaction, &id)? {
        insert_artifact(transaction, &id, kind, version, &fingerprint, &payload)?;
    }
    Ok(id)
}

fn refs(
    raw: Option<&str>,
    spillover: Option<&str>,
    compensation: Option<&str>,
    transform: Option<&str>,
    gate: Option<&str>,
    parent_population: Option<&str>,
) -> Value {
    let mut map = Map::new();
    if let Some(value) = raw {
        map.insert("raw".into(), json!(value));
    }
    if let Some(value) = spillover {
        map.insert("spillover".into(), json!(value));
    }
    if let Some(value) = compensation {
        map.insert("compensation".into(), json!(value));
    }
    if let Some(value) = transform {
        map.insert("transform".into(), json!(value));
    }
    if let Some(value) = gate {
        map.insert("gate".into(), json!(value));
    }
    if let Some(value) = parent_population {
        map.insert("parent_population".into(), json!(value));
    }
    Value::Object(map)
}

pub fn reset_and_seed(db: &Database, source: &str, replay: bool) -> Result<Value, DbError> {
    let case: Value = serde_json::from_str(source).expect("bundled case parses");
    let fingerprint = source_fingerprint(source);
    db.transaction(|tx| {
        tx.execute_batch("DELETE FROM events; DELETE FROM branch_heads; DELETE FROM artifacts; DELETE FROM import_checkpoints;")?;
        let baseline_refs = seed_baseline(tx, &case, &fingerprint)?;
        append_event(
            tx,
            "evt-import-baseline",
            BRANCH_MAIN,
            "CASE_IMPORTED_ATOMICALLY",
            &json!({
                "case_id": case["case_id"],
                "source_fingerprint": fingerprint,
                "rollback_policy": "import_failure_rolls_back_whole_import",
                "raw_representation_policy": "original values retained; unconvertible values stay in place with diagnostics",
                "versioning_policy": "raw channels, spillover, compensation, transform, gates and parent-child edges are versioned independently",
                "machine_rule": "import is atomic and never performs irreversible cleaning"
            }),
            &baseline_refs,
            None,
        )?;
        record_checkpoint(tx, CASE_ID, &fingerprint, 1)?;
        if replay {
            replay_scenario_events(tx, &case)?;
        }
        Ok(json!({"status":"seeded", "replay": replay, "source_fingerprint": fingerprint}))
    })
}

fn seed_baseline(
    tx: &rusqlite::Transaction<'_>,
    case: &Value,
    fingerprint: &str,
) -> Result<Value, DbError> {
    let mut refs_map = Map::new();

    let raw_channels = insert_versioned(tx, "raw-channel-schema", "raw-v1", json!({
        "id": "channels/raw-v1",
        "case_id": case["case_id"],
        "channels": case["channels"],
        "coordinate_basis": case["coordinate_basis"]
    }))?;
    refs_map.insert("channels".into(), json!(raw_channels));

    let raw_events = insert_versioned(tx, "raw-events", "raw-v1", json!({
        "id": "events/raw-v1",
        "case_id": case["case_id"],
        "events": case["events"],
        "unit": "event rows preserve detector-native representations",
        "source_fingerprint": fingerprint,
        "diagnostics": unconvertible_diagnostics(case)
    }))?;
    refs_map.insert("raw_events".into(), json!(raw_events));

    for control in case["single_stain_controls"].as_array().unwrap() {
        let id = control["id"].as_str().unwrap();
        insert_versioned(tx, "single-stain-control", id, json!({
            "id": format!("control/{id}"),
            "control": control,
            "unit": control["basis"]
        }))?;
    }

    let spillover = insert_versioned(tx, "spillover", "spill-v1", json!({
        "case_id": case["case_id"],
        "spillover": case["spillover"],
        "unit": case["spillover"]["unit"],
        "coordinate_basis": case["spillover"]["coordinate_basis"]
    }))?;
    refs_map.insert("spillover".into(), json!(&spillover));

    let compensation = build_compensated_artifact(tx, case, "comp-v1", "spill-base", &case["spillover"], &case["compensation"]["near_singular_threshold"])?;
    refs_map.insert("compensation".into(), json!(&compensation));

    let transformed = build_transformed_artifact(tx, case, "transformed-v1", &raw_events, &compensation, &case["transform"])?;
    refs_map.insert("transform".into(), json!(&transformed));

    let mut gate_ids = Vec::new();
    for gate in case["gates"].as_array().unwrap() {
        let gate_version = gate["id"].as_str().unwrap().replace("gate-", "").to_string();
        let id = insert_versioned(tx, "gate", &gate_version, gate_payload(gate, "g-v1"))?;
        gate_ids.push(id);
    }
    refs_map.insert("gates".into(), json!(gate_ids));

    for (child_index, parent_index) in [(1, 0), (2, 0)] {
        let child = &case["gates"][child_index];
        let parent = &case["gates"][parent_index];
        insert_versioned(tx, "lineage-edge", &format!("edge-{}", child["id"].as_str().unwrap().replace("gate-", "")), json!({
            "id": format!("lineage/{}/v1", child["id"].as_str().unwrap()),
            "child_gate_id": child["id"],
            "parent_gate_id": parent["id"],
            "relationship": "gate_parent_child",
            "version": "edge-v1"
        }))?;
    }

    let root = build_population_artifact(
        tx,
        case,
        "pop-root-v1",
        &json!({"id":"gate-root-v1","name":"All imported events","gate_type":"root"}),
        "root/v1",
        None,
        &raw_events,
        Some(&compensation),
    )?;
    let cells = build_population_artifact(
        tx,
        case,
        "pop-cells-v1",
        &case["gates"][0],
        "cells/v1",
        Some(&root),
        &raw_events,
        Some(&compensation),
    )?;
    let fitc = build_population_artifact(
        tx,
        case,
        "pop-fitc-v1",
        &case["gates"][1],
        "fitc/v1",
        Some(&cells),
        &raw_events,
        Some(&compensation),
    )?;
    let pe = build_population_artifact(
        tx,
        case,
        "pop-pe-v1",
        &case["gates"][2],
        "pe/v1",
        Some(&cells),
        &raw_events,
        Some(&compensation),
    )?;
    refs_map.insert("populations".into(), json!([root, cells, fitc, pe]));
    Ok(Value::Object(refs_map))
}

fn unconvertible_diagnostics(case: &Value) -> Value {
    let mut diagnostics = Vec::new();
    for event in case["events"].as_array().unwrap() {
        for (index, channel) in event["ordered_channels"].as_array().unwrap().iter().enumerate() {
            if event["values"][index].is_string() {
                diagnostics.push(json!({
                    "event_id": event["id"],
                    "channel_id": channel,
                    "original_value": event["values"][index],
                    "status": "unconvertible_retained_at_source",
                    "derived_value": null,
                    "diagnostic": "non-numeric detector value cannot be normalized without losing original representation"
                }));
            }
        }
    }
    json!(diagnostics)
}
