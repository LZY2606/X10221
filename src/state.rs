use crate::domain::*;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone)]
pub struct AppError {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl AppError {
    pub fn conflict(message: impl Into<String>, details: Value) -> Self {
        Self {
            status: 409,
            code: "VERSION_CONFLICT".into(),
            message: message.into(),
            details,
        }
    }

    pub fn unprocessable(message: impl Into<String>, details: Value) -> Self {
        Self {
            status: 422,
            code: "RULE_VIOLATION".into(),
            message: message.into(),
            details,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: 404,
            code: "NOT_FOUND".into(),
            message: message.into(),
            details: json!({}),
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for AppError {}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppData {
    pub case_id: String,
    pub initialized: bool,
    pub source_files: Vec<Value>,
    pub materials: Vec<Value>,
    pub raw_channels: Vec<Value>,
    pub events: Vec<Value>,
    pub conflicts: Vec<Value>,
    pub derived: Vec<Value>,
    pub invalidations: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DecisionRequest {
    pub candidate_group: String,
    pub option: String,
    pub base_event_id: String,
    pub client_window: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UndoRequest {
    pub decision_event_id: String,
    pub base_event_id: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ReplayRequest {
    pub step: String,
    pub base_event_id: Option<String>,
}

fn now_label(seq: usize) -> String {
    format!("case-sequence-{:03}", seq)
}

fn hash_bytes(input: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("sha-fnv1a-64:{:016x}", hash)
}

pub fn case_files() -> Vec<(&'static str, &'static str, &'static [u8])> {
    vec![
        ("f-events", "cases/flow-boundary-chamber/events.fcs.json", include_bytes!("../cases/flow-boundary-chamber/events.fcs.json")),
        ("f-spillover", "cases/flow-boundary-chamber/spillover.json", include_bytes!("../cases/flow-boundary-chamber/spillover.json")),
        ("f-comp-matrix", "cases/flow-boundary-chamber/compensation_matrix.json", include_bytes!("../cases/flow-boundary-chamber/compensation_matrix.json")),
        ("f-near-singular-matrix", "cases/flow-boundary-chamber/near_singular_matrix.json", include_bytes!("../cases/flow-boundary-chamber/near_singular_matrix.json")),
        ("f-transform", "cases/flow-boundary-chamber/transform.json", include_bytes!("../cases/flow-boundary-chamber/transform.json")),
        ("f-gates", "cases/flow-boundary-chamber/gates.json", include_bytes!("../cases/flow-boundary-chamber/gates.json")),
        ("f-lineage", "cases/flow-boundary-chamber/lineage.json", include_bytes!("../cases/flow-boundary-chamber/lineage.json")),
    ]
}

pub fn manifest_bytes() -> &'static [u8] {
    include_bytes!("../cases/flow-boundary-chamber/manifest.json")
}

fn parse(bytes: &[u8]) -> Result<Value, AppError> {
    serde_json::from_slice(bytes).map_err(|error| AppError::unprocessable(
        "case file is not valid JSON",
        json!({ "diagnostic": error.to_string() }),
    ))
}

fn object_provenance(parts: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in parts {
        map.insert((*key).to_string(), Value::String((*value).to_string()));
    }
    Value::Object(map)
}

pub fn head_event_id(data: &AppData) -> String {
    data.events.last().and_then(|event| event["event_id"].as_str()).unwrap_or("event-000").to_string()
}

pub fn is_initialized(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row("select exists(select 1 from material_versions limit 1)", [], |row| row.get(0))
}

fn append_event(data: &mut AppData, event_type: &str, branch: &str, parent_event_id: &str, payload: Value, judgment: Option<Value>, affected: Vec<Value>) {
    let seq = data.events.len() + 1;
    let event_id = format!("event-{:03}", seq);
    data.events.push(json!({
        "event_id": event_id,
        "sequence": seq,
        "timestamp": now_label(seq),
        "event_type": event_type,
        "branch": branch,
        "parent_event_id": parent_event_id,
        "payload": payload,
        "judgment": judgment,
        "affected_objects": affected,
        "machine_readable": true
    }));
}

fn latest_material(data: &AppData, kind: &str) -> Option<Value> {
    data.materials
        .iter()
        .filter(|item| item["material_kind"] == kind)
        .filter_map(|item| item["raw_json"].as_str().and_then(|raw| serde_json::from_str::<Value>(raw).ok()).map(|doc| (item, doc)))
        .last()
        .map(|(item, doc)| json!({ "record": item, "doc": doc }))
}

fn material_by_version(data: &AppData, kind: &str, version: &str) -> Value {
    data.materials
        .iter()
        .filter(|item| item["material_kind"] == kind && item["version"] == version)
        .find_map(|item| item["raw_json"].as_str().and_then(|raw| serde_json::from_str::<Value>(raw).ok()))
        .unwrap_or_else(|| json!({}))
}

fn active_ref(kind: &str, version: &str) -> Value {
    json!({ "material_kind": kind, "version": version })
}

fn derived_push(data: &mut AppData, mut object: Value, introduced_by: &str, status: &str) {
    let id = object["stable_id"].as_str().unwrap_or("derived").to_string();
    object["introduced_by_event"] = json!(introduced_by);
    object["status"] = json!(status);
    object["ordinal"] = json!(data.derived.len() + 1);
    object["stable_id"] = json!(id);
    data.derived.push(object);
}

fn invalidate(data: &mut AppData, stable_id: &str, from_event: &str, reason: &str, scope: &str, affected_dependencies: Vec<Value>) {
    let already = data.invalidations.iter().any(|item| {
        item["derived_stable_id"] == stable_id && item["from_event"] == from_event
    });
    if already {
        return;
    }
    for object in &mut data.derived {
        if object["stable_id"] == stable_id && object["status"] != "invalidated" {
            object["status"] = json!("invalidated");
            object["invalidated_by_event"] = json!(from_event);
        }
    }
    data.invalidations.push(json!({
        "invalidation_id": format!("inv::{}::{}", from_event, stable_id),
        "derived_stable_id": stable_id,
        "from_event": from_event,
        "reason_code": reason,
        "scope": scope,
        "affected_dependencies": affected_dependencies
    }));
}

fn add_source_file(data: &mut AppData, file_id: &str, path: &str, bytes: &[u8], material_kind: &str, version: &str, media_type: &str) {
    let raw_text = String::from_utf8_lossy(bytes).to_string();
    let hash = hash_bytes(bytes);
    data.source_files.push(json!({
        "file_id": file_id,
        "path": path,
        "media_type": media_type,
        "material_kind": material_kind,
        "version": version,
        "sha256_like_content_hash": hash,
        "original_representation": raw_text
    }));
        data.materials.push(json!({
            "material_kind": material_kind,
            "version": version,
            "file_id": file_id,
            "raw_json": raw_text,
            "content_hash": hash,
            "immutable": true,
            "introduced_by_event": "event-001",
            "branch": "baseline"
        }));
}

pub fn initialize_case(conn: &mut Connection) -> Result<(), AppError> {
    let tx = conn.transaction().map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    let mut data = load_data(&tx)?;
    if data.initialized {
        return Ok(());
    }
    data.case_id = CASE_ID.into();
    data.initialized = true;

    let manifest: Value = parse(manifest_bytes())?;
    let files = case_files();
    for (file_id, path, bytes) in files {
        let descriptor = manifest["files"]
            .as_array()
            .and_then(|entries| entries.iter().find(|entry| entry["file_id"] == file_id))
            .ok_or_else(|| AppError::unprocessable("manifest is missing a case file", json!({ "file_id": file_id })))?;
        parse(bytes)?;
        add_source_file(
            &mut data,
            file_id,
            path,
            bytes,
            descriptor["material_kind"].as_str().unwrap(),
            descriptor["version"].as_str().unwrap(),
            descriptor["media_type"].as_str().unwrap(),
        );
    }
    build_baseline(&mut data);
    persist_all(&tx, &data)?;
    tx.commit().map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    Ok(())
}

fn matrix_evaluation_object(matrix: &Value, source_event: &str) -> Value {
    let version = matrix["version"].as_str().unwrap_or("unknown");
    let check = check_compensation_matrix(&numeric_cells(matrix));
    json!({
        "stable_id": format!("matrix-evaluation::{}", version),
        "derived_kind": "matrix_evaluation",
        "matrix_version": version,
        "spillover_version": matrix["source_spillover_version"],
        "determinant": check.determinant,
        "reciprocal_condition": check.reciprocal_condition,
        "threshold": NEAR_SINGULAR_LIMIT,
        "state": format!("{:?}", check.status),
        "inverse_allowed": check.status == MatrixStatus::Invertible,
        "inverse": check.inverse.map(|value| json!(value)),
        "diagnostics": check.diagnostics,
        "provenance": object_provenance(&[
            ("source_event", source_event),
            ("material_kind", "compensation_matrix"),
            ("version", version),
            ("spillover_version", matrix["source_spillover_version"].as_str().unwrap_or("spo-v1"))
        ])
    })
}

fn build_baseline(data: &mut AppData) {
    let events_doc = material_by_version(data, "raw_channel_values", "evt-v1");
    let spillover_doc = material_by_version(data, "spillover_metadata", "spo-v1");
    let matrix_doc = material_by_version(data, "compensation_matrix", "mat-v1");
    let near_doc = material_by_version(data, "compensation_matrix", "mat-v2-near-singular");
    let transform_doc = material_by_version(data, "transform_parameters", "trn-v1");
    let gates_doc = material_by_version(data, "gate_polygon", "gate-v1");
    let edges_doc = material_by_version(data, "parent_child_relation", "lin-v1");

    for event in events_doc["events"].as_array().cloned().unwrap_or_default() {
        for (channel, channel_value) in event["channels"].as_object().cloned().unwrap_or_default() {
            let event_id = event["event_id"].as_str().unwrap_or("unknown");
            data.raw_channels.push(json!({
                "stable_id": format!("raw::{}::{}::evt-v1", event_id, channel),
                "event_id": event_id,
                "channel": channel,
                "value": channel_value["value"],
                "unit": channel_value["unit"],
                "material_version": "evt-v1",
                "representation": "original acquisition representation"
            }));
        }
    }

    let event_id = "event-001";
    append_event(
        data,
        "CASE_IMPORT",
        "baseline",
        "event-000",
        json!({
            "case_id": CASE_ID,
            "manifest": manifest_bytes().escape_ascii().to_string(),
            "material_versions": [
                active_ref("raw_channel_values", "evt-v1"),
                active_ref("spillover_metadata", "spo-v1"),
                active_ref("compensation_matrix", "mat-v1"),
                active_ref("compensation_matrix", "mat-v2-near-singular"),
                active_ref("transform_parameters", "trn-v1"),
                active_ref("gate_polygon", "gate-v1"),
                active_ref("parent_child_relation", "lin-v1")
            ]
        }),
        Some(json!({
            "rule_id": "IMPORT_ATOMIC_SEPARATED_VERSIONING",
            "state": "accepted",
            "basis": "all source classes were stored with their original representation and no irreversible cleaning occurred"
        })),
        vec![
            json!({"kind":"material", "id":"raw_channel_values/evt-v1"}),
            json!({"kind":"material", "id":"spillover_metadata/spo-v1"}),
            json!({"kind":"material", "id":"compensation_matrix/mat-v1"}),
            json!({"kind":"material", "id":"transform_parameters/trn-v1"}),
            json!({"kind":"material", "id":"gate_polygon/gate-v1"}),
            json!({"kind":"material", "id":"parent_child_relation/lin-v1"})
        ]
    );

    let matrix_eval = matrix_evaluation_object(&matrix_doc, event_id);
    let near_eval = matrix_evaluation_object(&near_doc, event_id);
    derived_push(data, matrix_eval, event_id, "active");
    derived_push(data, near_eval, event_id, "rejected_candidate");

    append_event(
        data,
        "RULE_JUDGMENT",
        "baseline",
        event_id,
        json!({
            "rule_id": "NEAR_SINGULAR_COMPENSATION_MATRIX_NOT_INVERTED",
            "matrix_version": "mat-v2-near-singular",
            "reciprocal_condition": 0.000000999999000001,
            "threshold": NEAR_SINGULAR_LIMIT
        }),
        Some(json!({
            "rule_id": "NEAR_SINGULAR_COMPENSATION_MATRIX_NOT_INVERTED",
            "state": "rejected",
            "basis": "reciprocal condition is below threshold; inverse is not emitted or substituted silently"
        })),
        vec![
            json!({"kind":"derived", "id":"matrix-evaluation::mat-v2-near-singular"}),
            json!({"kind":"scope", "id":"compensated::mat-v2-near-singular::*"})
        ]
    );

    let check = check_compensation_matrix(&numeric_cells(&matrix_doc));
    let inverse = check.inverse.unwrap();
    let cofactor = transform_doc["cofactor"].as_f64().unwrap();

    for event_id_value in event_ids(&events_doc) {
        let fitc_raw = raw_channel(&events_doc, &event_id_value, "FITC-A").unwrap();
        let pe_raw = raw_channel(&events_doc, &event_id_value, "PE-A").unwrap();
        let fitc_comp = inverse[0][0] * fitc_raw + inverse[0][1] * pe_raw;
        let pe_comp = inverse[1][0] * fitc_raw + inverse[1][1] * pe_raw;

        let mut diagnostics = Vec::new();
        if let Some(raw_event) = raw_event(&events_doc, &event_id_value) {
            if let Some(items) = raw_event["diagnostics"].as_array() {
                diagnostics.extend(items.iter().cloned());
            }
        }
        let fitc_extreme = fitc_raw >= 262144.0;
        let pe_extreme = pe_raw > 100000.0;
        if fitc_extreme {
            diagnostics.push(json!("FITC value retained at ADC saturation; no clipping"));
        }
        if pe_extreme {
            diagnostics.push(json!("PE value retained outside ordinary linear range; no clipping"));
        }

        for (channel, raw, comp, transformed) in [
            ("FITC-A", fitc_raw, fitc_comp, asinh_transform(fitc_comp, cofactor)),
            ("PE-A", pe_raw, pe_comp, asinh_transform(pe_comp, cofactor)),
        ] {
            let compensation_id = format!("compensated::mat-v1::{}::{}", event_id_value, channel);
            derived_push(data, json!({
                "stable_id": compensation_id,
                "derived_kind": "compensated_value",
                "event_id": event_id_value,
                "channel": channel,
                "raw_value": raw,
                "raw_unit": "raw fluorescence count",
                "value": comp,
                "unit": "compensated fluorescence count",
                "coordinate_basis": "raw fluorescence counts compensated by mat-v1",
                "diagnostics": diagnostics,
                "provenance": object_provenance(&[
                    ("source_event", "event-001"),
                    ("event_material_version", "evt-v1"),
                    ("matrix_version", "mat-v1"),
                    ("spillover_version", "spo-v1")
                ])
            }), "event-001", "active");

            let transform_id = format!("transformed::trn-v1::{}::{}", event_id_value, channel);
            derived_push(data, json!({
                "stable_id": transform_id,
                "derived_kind": "transformed_value",
                "event_id": event_id_value,
                "channel": channel,
                "value": transformed,
                "unit": "dimensionless",
                "coordinate_basis": "asinh(compensated / 150)",
                "diagnostics": diagnostics,
                "provenance": object_provenance(&[
                    ("source_event", "event-001"),
                    ("compensated_value", &compensation_id),
                    ("transform_version", "trn-v1")
                ])
            }), "event-001", "active");
        }
    }
    for gate in gates_doc["gates"].as_array().cloned().unwrap_or_default() {
        let gate_id = gate["gate_id"].as_str().unwrap().to_string();
        let poly: Vec<[f64; 2]> = serde_json::from_value(gate["polygon"].clone()).unwrap_or_default();
        for event_id_value in event_ids(&events_doc) {
            let point = if gate["channels"][0] == "FSC-A" || gate_id == "g-root" {
                (
                    raw_channel(&events_doc, &event_id_value, "FSC-A").unwrap(),
                    raw_channel(&events_doc, &event_id_value, "SSC-A").unwrap(),
                )
            } else {
                let fitc = transformed_value(data, &event_id_value, "FITC-A", "mat-v1", "trn-v1");
                let pe = transformed_value(data, &event_id_value, "PE-A", "mat-v1", "trn-v1");
                (fitc, pe)
            };
            let membership = point_in_polygon(point, &poly, true);
            let boundary = point_in_polygon(point, &poly, false) != membership;
            let id = format!("membership::gate-v1::{}::{}", gate_id, event_id_value);
            let mut diagnostics = Vec::new();
            if boundary {
                diagnostics.push(json!("POINT_ON_GATE_BOUNDARY; both include and exclude interpretations are retained"));
            }
            if let Some(raw_event) = raw_event(&events_doc, &event_id_value) {
                if let Some(items) = raw_event["diagnostics"].as_array() {
                    diagnostics.extend(items.iter().cloned());
                }
            }
            let basis = gate["coordinate_basis"].as_str().unwrap_or("gate coordinates").to_string();
            derived_push(data, json!({
                "stable_id": id,
                "derived_kind": "gate_membership",
                "gate_id": gate_id,
                "gate_version": "gate-v1",
                "lineage_version": "lin-v1",
                "event_id": event_id_value,
                "event_material_version": "evt-v1",
                "coordinate": [point.0, point.1],
                "included": membership,
                "on_boundary": boundary,
                "coordinate_basis": basis,
                "diagnostics": diagnostics,
                "provenance": object_provenance(&[
                    ("source_event", "event-001"),
                    ("gate_version", "gate-v1"),
                    ("lineage_version", "lin-v1"),
                    ("event_material_version", "evt-v1")
                ])
            }), "event-001", "active");
        }
    }

    append_event(
        data,
        "RULE_JUDGMENT",
        "baseline",
        event_id,
        json!({
            "rule_id": "BOUNDARY_POINT_CANDIDATES_RETAINED",
            "points": ["E004", "E005"],
            "gate_versions": ["gate-v1"]
        }),
        Some(json!({
            "rule_id": "BOUNDARY_POINT_CANDIDATES_RETAINED",
            "state": "ambiguous",
            "basis": "boundary membership is not resolved by rounding or a hidden tie-breaker"
        })),
        vec![
            json!({"kind":"derived", "id":"membership::gate-v1::g-lymph::E004"}),
            json!({"kind":"derived", "id":"membership::gate-v1::g-cd4::E005"}),
            json!({"kind":"derived", "id":"membership::gate-v1::g-cd8::E005"})
        ]
    );

    append_event(
        data,
        "RULE_JUDGMENT",
        "baseline",
        "event-002",
        json!({
            "rule_id": "EXTREME_SIGNAL_ORIGINAL_RETAINED",
            "events": ["E006"],
            "raw_versions": ["evt-v1"]
        }),
        Some(json!({
            "rule_id": "EXTREME_SIGNAL_ORIGINAL_RETAINED",
            "state": "diagnostic_retained",
            "basis": "saturated or out-of-range values remain at their original position and are not dropped for plotting"
        })),
        vec![
            json!({"kind":"raw_channel", "id":"raw::E006::FITC-A::evt-v1"}),
            json!({"kind":"raw_channel", "id":"raw::E006::PE-A::evt-v1"})
        ]
    );

    build_population_candidates(data, &gates_doc, &edges_doc, "baseline");
}

fn transformed_value(data: &AppData, event_id: &str, channel: &str, matrix_version: &str, transform_version: &str) -> f64 {
    let id = format!("transformed::{}::{}::{}", transform_version, event_id, channel);
    data.derived
        .iter()
        .find(|object| object["stable_id"] == id && object["provenance"]["matrix_version"] == matrix_version)
        .and_then(|object| object["value"].as_f64())
        .unwrap_or_else(|| {
            let comp_id = format!("compensated::{}::{}::{}", matrix_version, event_id, channel);
            let comp = data.derived.iter().find(|object| object["stable_id"] == comp_id).and_then(|object| object["value"].as_f64()).unwrap_or(0.0);
            let cofactor = if transform_version == "trn-v2-cofactor-120" { 120.0 } else { 150.0 };
            asinh_transform(comp, cofactor)
        })
}

fn membership_at(data: &AppData, gate_id: &str, gate_version: &str, event_id: &str) -> bool {
    let id = format!("membership::{}::{}::{}", gate_version, gate_id, event_id);
    data.derived.iter().find(|object| object["stable_id"] == id).and_then(|object| object["included"].as_bool()).unwrap_or(false)
}

fn candidate_group_object(group_id: &str, title: &str, options: Vec<Value>, dependencies: Vec<Value>, rule_id: &str, ambiguity: &str) -> Value {
    json!({
        "stable_id": format!("candidate-group::{}", group_id),
        "derived_kind": "candidate_group",
        "group_id": group_id,
        "title": title,
        "status": "open",
        "rule_id": rule_id,
        "ambiguity": ambiguity,
        "dependencies": dependencies,
        "options": options,
        "provenance": object_provenance(&[("source_event", "event-001"), ("scope", "baseline")])
    })
}

fn build_population_candidates(data: &mut AppData, gates_doc: &Value, edges_doc: &Value, scope: &str) {
    let existing = data.derived.iter().any(|object| object["derived_kind"] == "candidate_group" && object["scope"] == scope);
    if !existing {
        derived_push(data, candidate_group_object(
            "E004-LYMPH-BOUNDARY",
            "E004 位于 Lymphocytes 边界",
            vec![
                json!({"option_id":"include","label":"纳入 E004","legal":true}),
                json!({"option_id":"exclude","label":"排除 E004","legal":true})
            ],
            vec![
                active_ref("gate_polygon", "gate-v1"),
                active_ref("raw_channel_values", "evt-v1")
            ],
            "BOUNDARY_POINT_CANDIDATES_RETAINED",
            "closed-boundary and open-boundary interpretations are both legal at FSC 48000"
        ), "event-001", "active");
        derived_push(data, candidate_group_object(
            "E005-CD4-CD8-EDGE",
            "E005 位于 CD4/CD8 共享边",
            vec![
                json!({"option_id":"assign-cd4","label":"判给 CD4+","legal":true}),
                json!({"option_id":"assign-cd8","label":"判给 CD8+","legal":true})
            ],
            vec![
                active_ref("gate_polygon", "gate-v1"),
                active_ref("transform_parameters", "trn-v1"),
                active_ref("compensation_matrix", "mat-v1")
            ],
            "BOUNDARY_POINT_CANDIDATES_RETAINED",
            "the event is on the shared edge of adjacent sibling populations"
        ), "event-001", "active");
        derived_push(data, candidate_group_object(
            "E006-EXTREME-SIGNAL",
            "E006 是 FITC 饱和/PE 超高信号",
            vec![
                json!({"option_id":"retain-in-cd3","label":"保留在 CD3+ 候选","legal":true}),
                json!({"option_id":"diagnostic-hold","label":"进入诊断保留区","legal":true})
            ],
            vec![
                active_ref("raw_channel_values", "evt-v1"),
                active_ref("transform_parameters", "trn-v1")
            ],
            "EXTREME_SIGNAL_ORIGINAL_RETAINED",
            "geometry supports inclusion but acquisition saturation leaves interpretation uncertain"
        ), "event-001", "active");
        let mut matrix_group = candidate_group_object(
            "MATRIX-NEAR-SINGULAR",
            "mat-v2-near-singular 近奇异补偿矩阵",
            vec![
                json!({"option_id":"reject-inverse","label":"拒绝求异并保留诊断","legal":true,"recommended":true}),
                json!({"option_id":"attempt-inverse","label":"尝试默默求逆","legal":false,"blocking_rule":"NEAR_SINGULAR_COMPENSATION_MATRIX_NOT_INVERTED"})
            ],
            vec![
                active_ref("compensation_matrix", "mat-v2-near-singular"),
                active_ref("spillover_metadata", "spo-v1")
            ],
            "NEAR_SINGULAR_COMPENSATION_MATRIX_NOT_INVERTED",
            "the matrix is nearly singular and has no accepted unique inverse",
        );
        matrix_group["scope"] = json!("baseline");
        matrix_group["status"] = json!("rejected");
        matrix_group["selected_option"] = json!("reject-inverse");
        derived_push(data, matrix_group, "event-002", "rejected_candidate");
    }

    let all_events = event_ids(&material_by_version(data, "raw_channel_values", "evt-v1"));
    let source_event = if scope == "baseline" { "event-001" } else { "event-001" };
    push_fixed_population(data, gates_doc, "g-root", &all_events, None, source_event, scope);
    let root_id = format!("population::{}::g-root::option=fixed", scope);
    push_fixed_population(data, gates_doc, "g-singlets", &all_events, Some(&root_id), source_event, scope);
    let singlets_id = format!("population::{}::g-singlets::option=fixed", scope);

    let choices = [
        ("include", "retain-in-cd3", "assign-cd4"),
        ("include", "retain-in-cd3", "assign-cd8"),
        ("include", "diagnostic-hold", "assign-cd4"),
        ("include", "diagnostic-hold", "assign-cd8"),
        ("exclude", "retain-in-cd3", "assign-cd4"),
        ("exclude", "retain-in-cd3", "assign-cd8"),
        ("exclude", "diagnostic-hold", "assign-cd4"),
        ("exclude", "diagnostic-hold", "assign-cd8"),
    ];

    for (e4, e6, e5) in choices {
        let lymph_option = format!("E004={}", e4);
        let lymph_id = push_option_population(
            data, gates_doc, edges_doc, "g-lymph", scope, &singlets_id, &lymph_option,
            vec![json!({"group":"E004-LYMPH-BOUNDARY","option":e4})],
            |event_id| membership_at(data, "g-lymph", "gate-v1", event_id) && (e4 == "include" || event_id != "E004"),
            source_event,
        );

        let cd3_option = format!("E004={};E006={}", e4, e6);
        let cd3_id = push_option_population(
            data, gates_doc, edges_doc, "g-cd3", scope, &lymph_id, &cd3_option,
            vec![
                json!({"group":"E004-LYMPH-BOUNDARY","option":e4}),
                json!({"group":"E006-EXTREME-SIGNAL","option":e6})
            ],
            |event_id| {
                if event_id == "E006" { return e6 == "retain-in-cd3"; }
                if event_id == "E004" { return e4 == "include"; }
                membership_at(data, "g-cd3", "gate-v1", event_id)
            },
            source_event,
        );

        push_option_population(
            data, gates_doc, edges_doc, "g-cd4", scope, &cd3_id,
            &format!("{};E005={}", cd3_option, e5),
            vec![json!({"group":"E005-CD4-CD8-EDGE","option":e5})],
            |event_id| {
                if event_id == "E005" { return e5 == "assign-cd4"; }
                if event_id == "E004" { return e4 == "include"; }
                membership_at(data, "g-cd4", "gate-v1", event_id)
            },
            source_event,
        );

        push_option_population(
            data, gates_doc, edges_doc, "g-cd8", scope, &cd3_id,
            &format!("{};E005={}", cd3_option, e5),
            vec![json!({"group":"E005-CD4-CD8-EDGE","option":e5})],
            |event_id| {
                if event_id == "E005" { return e5 == "assign-cd8"; }
                membership_at(data, "g-cd8", "gate-v1", event_id)
            },
            source_event,
        );
    }
}

fn population_members(all_events: &[String], predicate: &dyn Fn(&str) -> bool) -> Vec<String> {
    all_events.iter().filter(|event_id| predicate(event_id)).cloned().collect()
}

fn push_population_object(
    data: &mut AppData,
    gates_doc: &Value,
    gate_id: &str,
    stable_id: String,
    scope: &str,
    members: Vec<String>,
    denominator_population_id: Option<&str>,
    option_signature: &str,
    candidate_decisions: Vec<Value>,
    source_event: &str,
) -> String {
    let denominator_count = if let Some(parent_id) = denominator_population_id {
        data.derived
            .iter()
            .find(|object| object["stable_id"] == parent_id && object["derived_kind"] == "population_option")
            .and_then(|object| object["event_count"].as_i64())
            .unwrap_or(0)
    } else {
        members.len() as i64
    };
    let percent = if denominator_count == 0 { None } else { Some(members.len() as f64 * 100.0 / denominator_count as f64) };
    let parent = parent_for(&material_by_version(data, "parent_child_relation", "lin-v1"), gate_id);
    let object = json!({
        "stable_id": stable_id,
        "derived_kind": "population_option",
        "scope": scope,
        "gate_id": gate_id,
        "gate_name": gate_name(gates_doc, gate_id),
        "gate_version": "gate-v1",
        "lineage_version": "lin-v1",
        "event_material_version": "evt-v1",
        "option_signature": option_signature,
        "candidate_decisions": candidate_decisions,
        "members": members,
        "event_count": members.len(),
        "percentage": percent,
        "percentage_unit": "% of pinned parent population",
        "denominator": {
            "pinned": denominator_population_id.is_some(),
            "parent_gate_id": parent,
            "parent_population_option_id": denominator_population_id,
            "parent_version_snapshot": denominator_population_id,
            "parent_event_count": denominator_count,
            "rule_id": "PERCENT_DENOMINATOR_PINNED_TO_PARENT_POPULATION_VERSION"
        },
        "provenance": object_provenance(&[
            ("source_event", source_event),
            ("scope", scope),
            ("gate_version", "gate-v1"),
            ("lineage_version", "lin-v1"),
            ("event_material_version", "evt-v1")
        ])
    });
    let exists = data.derived.iter().any(|item| item["stable_id"] == stable_id);
    if !exists {
        derived_push(data, object, source_event, "candidate");
    }
    stable_id
}

fn push_fixed_population(
    data: &mut AppData,
    gates_doc: &Value,
    gate_id: &str,
    all_events: &[String],
    parent_id: Option<&str>,
    source_event: &str,
    scope: &str,
) -> String {
    let members = population_members(all_events, &|event_id| membership_at(data, gate_id, "gate-v1", event_id));
    let stable_id = format!("population::{}::{}::option=fixed", scope, gate_id);
    push_population_object(data, gates_doc, gate_id, stable_id, scope, members, parent_id, "fixed", vec![], source_event)
}

fn push_option_population(
    data: &mut AppData,
    gates_doc: &Value,
    _edges_doc: &Value,
    gate_id: &str,
    scope: &str,
    parent_id: &str,
    signature: &str,
    decisions: Vec<Value>,
    predicate: impl Fn(&str) -> bool,
    source_event: &str,
) -> String {
    let all_events = event_ids(&material_by_version(data, "raw_channel_values", "evt-v1"));
    let members = population_members(&all_events, &predicate);
    let stable_id = format!("population::{}::{}::{}", scope, gate_id, stable_id_signature(signature));
    push_population_object(data, gates_doc, gate_id, stable_id, scope, members, Some(parent_id), signature, decisions, source_event)
}

fn stable_id_signature(input: &str) -> String {
    input
        .replace('=', "-")
        .replace(';', "--")
        .replace(',', "-")
        .replace(' ', "_")
}
