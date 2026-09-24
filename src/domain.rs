use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const NEAR_SINGULAR_LIMIT: f64 = 1e-4;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Ref {
    pub artifact: String,
    pub version: u64,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Judgment {
    pub id: String,
    pub code: String,
    pub status: String,
    pub message: String,
    #[serde(default)]
    pub affected: Vec<String>,
    #[serde(default)]
    pub references: Vec<Ref>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub gate: String,
    pub policy: String,
    pub members: Vec<String>,
    pub denominator_ref: Ref,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Population {
    pub gate: String,
    pub label: String,
    pub gate_version: u64,
    pub parent: String,
    pub parent_version: u64,
    pub denominator_ref: Ref,
    pub members: Vec<String>,
    pub denominator_count: usize,
    pub percentage: Option<f64>,
    pub status: String,
    pub candidate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DisplayValue {
    pub id: String,
    pub kind: String,
    pub event: Option<String>,
    pub channel: Option<String>,
    pub gate: Option<String>,
    pub value: Value,
    pub unit: String,
    pub basis: String,
    pub status: String,
    #[serde(default)]
    pub references: Vec<Ref>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub artifacts: BTreeMap<String, BTreeMap<u64, Artifact>>,
    pub branch: String,
    pub revision: u64,
    pub gate_decisions: BTreeMap<String, String>,
    pub populations: Vec<Population>,
    pub values: Vec<DisplayValue>,
    pub judgments: Vec<Judgment>,
    #[serde(default)]
    pub last_operation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub kind: String,
    pub version: u64,
    pub data: Value,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: u64,
    pub seq: u64,
    pub branch: String,
    pub parent_event: Option<u64>,
    pub revision: u64,
    pub kind: String,
    pub operation: Value,
    pub state: State,
    pub judgments: Vec<Judgment>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mutation {
    pub op: String,
    #[serde(default)]
    pub artifact: Option<String>,
    #[serde(default)]
    pub version: Option<u64>,
    #[serde(default)]
    pub gate: Option<String>,
    #[serde(default)]
    pub candidate: Option<String>,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub patch: Value,
    #[serde(default)]
    pub note: Option<String>,
}

fn val<'a>(object: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = object;
    for part in path {
        current = current.get(part)?;
    }
    Some(current)
}

pub fn parse_case(case_text: &str) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(case_text)?)
}

pub fn case_artifacts(case: &Value) -> BTreeMap<String, BTreeMap<u64, Artifact>> {
    let mut map = BTreeMap::new();
    let keys = ["raw_events", "spillover", "compensation_matrix", "transform", "gates", "lineage"];
    for key in keys {
        if let Some(data) = case.get(key) {
            let id = data.get("id").and_then(|v| v.as_str()).unwrap_or(key).to_string();
            let version = data.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u64;
            let kind = if key == "raw_events" { "raw_channel_values" } else { key };
            map.entry(id).or_insert_with(BTreeMap::new).insert(version, Artifact {
                kind: kind.to_string(),
                version,
                data: data.clone(),
                sources: vec!["embedded:case_pack/flow-boundary-chamber.case.json".to_string()],
            });
        }
    }
    map
}

pub fn initial_state(artifacts: BTreeMap<String, BTreeMap<u64, Artifact>>, branch: &str) -> State {
    derive_state(artifacts, branch, 1, BTreeMap::new(), &[], None)
}

fn latest<'a>(artifacts: &'a BTreeMap<String, BTreeMap<u64, Artifact>>, id: &str) -> Option<(u64, &'a Artifact)> {
    artifacts.get(id).and_then(|versions| versions.iter().next_back().map(|(v, a)| (*v, a)))
}

fn determinant(mut matrix: Vec<Vec<f64>>) -> f64 {
    let n = matrix.len();
    let mut determinant = 1.0;
    for column in 0..n {
        let pivot_row = (column..n).max_by(|a, b| {
            matrix[*a][column].abs().partial_cmp(&matrix[*b][column].abs()).unwrap_or(std::cmp::Ordering::Equal)
        }).unwrap_or(column);
        if pivot_row != column {
            matrix.swap(pivot_row, column);
            determinant = -determinant;
        }
        let pivot = matrix[column][column];
        if pivot.abs() < 1e-15 { return 0.0; }
        determinant *= pivot;
        for row in (column + 1)..n {
            let factor = matrix[row][column] / pivot;
            for target in column..n {
                matrix[row][target] -= factor * matrix[column][target];
            }
        }
    }
    determinant
}

fn invert_matrix(matrix: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = matrix.len();
    let mut augmented = Vec::new();
    for (r, row) in enumerate_rows(matrix) {
        let mut output = row.to_vec();
        output.extend((0..n).map(|c| if r == c { 1.0 } else { 0.0 }));
        augmented.push(output);
    }
    for column in 0..n {
        let pivot_row = (column..n).filter(|r| augmented[*r][column].abs() > 1e-12).min_by_key(|r| ordered_float_bits(augmented[*r][column].abs()))?;
        augmented.swap(column, pivot_row);
        let pivot = augmented[column][column];
        for value in &mut augmented[column] { *value /= pivot; }
        for row in 0..n {
            if row == column { continue; }
            let factor = augmented[row][column];
            for target in 0..augmented[row].len() {
                augmented[row][target] -= factor * augmented[column][target];
            }
        }
    }
    Some(augmented.into_iter().map(|row| row[n..].to_vec()).collect())
}

fn enumerate_rows(matrix: &[Vec<f64>]) -> impl Iterator<Item=(usize, &Vec<f64>)> {
    matrix.iter().enumerate()
}

fn ordered_float_bits(value: f64) -> u64 {
    value.to_bits()
}

fn matrix_from(data: &Value) -> Vec<Vec<f64>> {
    data.get("rows").and_then(|v| v.as_array()).map(|rows| {
        rows.iter().filter_map(|row| row.as_array().map(|cells| cells.iter().filter_map(|cell| cell.as_f64()).collect())).collect()
    }).unwrap_or_default()
}

fn raw_records(data: &Value) -> Vec<(String, serde_json::Map<String, Value>)> {
    data.get("events").and_then(|v| v.as_array()).map(|rows| {
        rows.iter().filter_map(|row| {
            let id = row.get(0)?.as_str()?.to_string();
            let channels = row.get(1)?.as_object()?.clone();
            Some((id, channels))
        }).collect()
    }).unwrap_or_default()
}

fn numeric(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
}

fn asinh_transform(value: f64, cofactor: f64) -> f64 {
    (value / cofactor).asinh()
}

fn polygon_contains(point: (f64, f64), vertices: &[Vec<f64>]) -> Option<bool> {
    let (x, y) = point;
    let mut inside = false;
    let mut on_edge = false;
    let mut previous = vertices.len() - 1;
    for current in 0..vertices.len() {
        let (px, py) = (vertices[previous][0], vertices[previous][1]);
        let (cx, cy) = (vertices[current][0], vertices[current][1]);
        let cross = (cx - px) * (y - py) - (cy - py) * (x - px);
        if cross.abs() < 1e-9 && x >= px.min(cx) - 1e-9 && x <= px.max(cx) + 1e-9 && y >= py.min(cy) - 1e-9 && y <= py.max(cy) + 1e-9 {
            on_edge = true;
        }
        let intersects = ((py > y) != (cy > y)) && (x < (cx - px) * (y - py) / (cy - py) + px);
        if intersects { inside = !inside; }
        previous = current;
    }
    if on_edge { None } else { Some(inside) }
}
