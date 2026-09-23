use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const CASE_ID: &str = "flow-boundary-chamber";
pub const NEAR_SINGULAR_LIMIT: f64 = 1.0e-5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseFile {
    pub file_id: String,
    pub path: String,
    pub material_kind: String,
    pub version: String,
    pub media_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseManifest {
    pub case_id: String,
    pub title: String,
    pub coordinate_system: String,
    pub files: Vec<CaseFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixStatus {
    Invertible,
    NearSingular,
    Singular,
}

#[derive(Debug, Clone)]
pub struct MatrixCheck {
    pub status: MatrixStatus,
    pub determinant: f64,
    pub reciprocal_condition: f64,
    pub inverse: Option<[[f64; 2]; 2]>,
    pub diagnostics: Vec<String>,
}

pub fn check_compensation_matrix(cells: &BTreeMap<String, BTreeMap<String, f64>>) -> MatrixCheck {
    let a = cells.get("FITC-A").and_then(|r| r.get("FITC-A")).copied().unwrap_or(0.0);
    let b = cells.get("FITC-A").and_then(|r| r.get("PE-A")).copied().unwrap_or(0.0);
    let c = cells.get("PE-A").and_then(|r| r.get("FITC-A")).copied().unwrap_or(0.0);
    let d = cells.get("PE-A").and_then(|r| r.get("PE-A")).copied().unwrap_or(0.0);
    let determinant = a * d - b * c;
    let row_scale = (a.hypot(b)).max(c.hypot(d));
    let inverse_scale = d.hypot(-b).max((-c).hypot(a));
    let reciprocal_condition = if row_scale == 0.0 || inverse_scale == 0.0 {
        0.0
    } else {
        determinant.abs() / (row_scale * inverse_scale)
    };

    let mut diagnostics = Vec::new();
    if determinant == 0.0 {
        diagnostics.push("COMP_MATRIX_SINGULAR".to_string());
        MatrixCheck {
            status: MatrixStatus::Singular,
            determinant,
            reciprocal_condition,
            inverse: None,
            diagnostics,
        }
    } else if reciprocal_condition < NEAR_SINGULAR_LIMIT {
        diagnostics.push("COMP_MATRIX_NEAR_SINGULAR_NO_SILENT_INVERSE".to_string());
        MatrixCheck {
            status: MatrixStatus::NearSingular,
            determinant,
            reciprocal_condition,
            inverse: None,
            diagnostics,
        }
    } else {
        let inverse = [
            [d / determinant, -b / determinant],
            [-c / determinant, a / determinant],
        ];
        MatrixCheck {
            status: MatrixStatus::Invertible,
            determinant,
            reciprocal_condition,
            inverse: Some(inverse),
            diagnostics,
        }
    }
}

pub fn asinh_transform(value: f64, cofactor: f64) -> f64 {
    (value / cofactor).asinh()
}

pub fn point_on_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> bool {
    let cross = (p.1 - a.1) * (b.0 - a.0) - (p.0 - a.0) * (b.1 - a.1);
    let dot = (p.0 - a.0) * (p.0 - b.0) + (p.1 - a.1) * (p.1 - b.1);
    cross.abs() < 1.0e-9 && dot <= 1.0e-9
}

pub fn point_in_polygon(point: (f64, f64), polygon: &[[f64; 2]], include_boundary: bool) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let xi = polygon[i][0];
        let yi = polygon[i][1];
        let xj = polygon[j][0];
        let yj = polygon[j][1];
        let a = (xi, yi);
        let b = (xj, yj);
        if include_boundary && point_on_segment(point, a, b) {
            return true;
        }
        let intersects = (yi > point.1) != (yj > point.1)
            && point.0 < (xj - xi) * (point.1 - yi) / (yj - yi) + xi;
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn numeric_cells(matrix: &Value) -> BTreeMap<String, BTreeMap<String, f64>> {
    let mut cells = BTreeMap::new();
    for cell in matrix.get("cells").and_then(Value::as_array).cloned().unwrap_or_default() {
        let row = cell["row"].as_str().unwrap_or_default().to_string();
        let column = cell["column"].as_str().unwrap_or_default().to_string();
        if let Some(value) = cell["value"].as_f64() {
            cells.entry(row).or_insert_with(BTreeMap::new).insert(column, value);
        }
    }
    cells
}

pub fn material_documents(records: &[Value], kind: &str, version: &str) -> Vec<Value> {
    records
        .iter()
        .filter(|r| r["material_kind"] == kind && r["version"] == version)
        .filter_map(|r| serde_json::from_str(r["raw_json"].as_str().unwrap_or("{}")).ok())
        .collect()
}

pub fn material_document(records: &[Value], kind: &str, version: &str) -> Value {
    material_documents(records, kind, version)
        .into_iter()
        .next()
        .unwrap_or_else(|| json!({}))
}

pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    format!("{}::{}", prefix, parts.join("::"))
}

pub fn value_label(value: f64) -> String {
    format!("{:.4}", value).trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn display_value(id: &str, label: impl Into<String>, value: impl Serialize, unit: &str, basis: &str, provenance: Value) -> Value {
    json!({
        "stable_id": id,
        "label": label.into(),
        "value": value,
        "unit": unit,
        "coordinate_basis": basis,
        "provenance": provenance
    })
}

pub fn event_ids(events_doc: &Value) -> Vec<String> {
    let mut ids: Vec<String> = events_doc["events"]
        .as_array()
        .map(|events| events.iter().map(|event| event["event_id"].as_str().unwrap_or_default().to_string()).collect())
        .unwrap_or_default();
    ids.sort();
    ids
}

pub fn raw_channel(events_doc: &Value, event_id: &str, channel: &str) -> Option<f64> {
    events_doc["events"]
        .as_array()?
        .iter()
        .find(|event| event["event_id"] == event_id)?
        ["channels"][channel]["value"]
        .as_f64()
}

pub fn raw_event(events_doc: &Value, event_id: &str) -> Option<Value> {
    events_doc["events"]
        .as_array()?
        .iter()
        .find(|event| event["event_id"] == event_id)
        .cloned()
}

pub fn gate_doc(gates_doc: &Value, gate_id: &str) -> Option<Value> {
    gates_doc["gates"]
        .as_array()?
        .iter()
        .find(|gate| gate["gate_id"] == gate_id)
        .cloned()
}

pub fn polygon(gates_doc: &Value, gate_id: &str) -> Vec<[f64; 2]> {
    gate_doc(gates_doc, gate_id)
        .and_then(|gate| serde_json::from_value(gate["polygon"].clone()).ok())
        .unwrap_or_default()
}

pub fn gate_name(gates_doc: &Value, gate_id: &str) -> String {
    gate_doc(gates_doc, gate_id)
        .and_then(|gate| gate["name"].as_str().map(str::to_string))
        .unwrap_or_else(|| gate_id.to_string())
}

pub fn parent_for(edges_doc: &Value, gate_id: &str) -> Option<String> {
    edges_doc["edges"]
        .as_array()?
        .iter()
        .find(|edge| edge["child_gate"] == gate_id)
        .and_then(|edge| edge["parent_gate"].as_str())
        .map(str::to_string)
}

pub fn children_of(edges_doc: &Value, gate_id: &str) -> Vec<String> {
    let mut children: Vec<(i64, String)> = edges_doc["edges"]
        .as_array()
        .map(|edges| {
            edges
                .iter()
                .filter(|edge| edge["parent_gate"] == gate_id)
                .map(|edge| (edge["order"].as_i64().unwrap_or(0), edge["child_gate"].as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    children.sort_by_key(|(order, _)| *order);
    children.into_iter().map(|(_, id)| id).collect()
}
