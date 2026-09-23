use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawChannelValue {
    pub raw_repr: String,
    pub value: Option<f64>,
    pub unit: String,
    pub coordinate_basis: String,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsEvent {
    pub event_id: String,
    pub stable_id: String,
    pub values: std::collections::BTreeMap<String, RawChannelValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsFile {
    pub events: Vec<FcsEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensationMatrix {
    pub stable_id: String,
    pub version: String,
    pub detector_order: Vec<String>,
    pub rows: Vec<Vec<f64>>,
    pub unit: String,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformParams {
    pub stable_id: String,
    pub version: String,
    pub family: String,
    pub parameters: std::collections::BTreeMap<String, f64>,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gate {
    pub gate_id: String,
    pub stable_id: String,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub polygon: Vec<[f64; 2]>,
    pub coordinate_unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSet {
    pub stable_id: String,
    pub version: String,
    pub coordinate_basis: String,
    pub boundary_rule: String,
    pub gates: Vec<Gate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageEdge {
    pub edge_id: String,
    pub stable_id: String,
    pub child_gate: String,
    pub parent_population: String,
    pub child_population: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lineage {
    pub stable_id: String,
    pub version: String,
    pub root_population_id: String,
    pub edges: Vec<LineageEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixVerdict {
    Invertible,
    Singular,
    NearSingular,
}

#[derive(Debug, Clone)]
pub struct MatrixAnalysis {
    pub verdict: MatrixVerdict,
    pub determinant: f64,
    pub rcond_estimate: f64,
    pub inverse: Option<Vec<Vec<f64>>>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DerivedPoint {
    pub event_id: String,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub compensated: std::collections::BTreeMap<String, Option<f64>>,
    pub coordinate_basis: String,
    pub unit: String,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GateMembership {
    pub event_id: String,
    pub gate_id: String,
    pub inside: bool,
    pub boundary: bool,
    pub ambiguous: bool,
    pub legal_inside: Vec<bool>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopulationCandidate {
    pub candidate_id: String,
    pub population_id: String,
    pub interpretation: String,
    pub members: Vec<String>,
    pub count: usize,
    pub numerator_ids: Vec<String>,
    pub denominator_population_version: Value,
    pub denominator_count: usize,
    pub percent: Option<f64>,
    pub parent_refs: Value,
    pub refs: Value,
    pub status: String,
    pub reason_code: String,
    pub diagnostics: Vec<String>,
}

pub const NEAR_SINGULAR_DETERMINANT: f64 = 1.0e-9;
pub const NEAR_SINGULAR_RCOND: f64 = 1.0e-6;

pub fn stable_hash(value: &Value) -> String {
    let text = canonical_json(value);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", hash)
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")),
        Value::Array(values) => {
            let inner: Vec<String> = values.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys.iter().map(|key| format!("\"{}\":{}", key, canonical_json(&map[*key]))).collect();
            format!("{{{}}}", inner.join(","))
        }
    }
}

pub fn analyze_matrix(matrix: &CompensationMatrix) -> MatrixAnalysis {
    let rows = &matrix.rows;
    let mut diagnostics = Vec::new();
    if rows.len() != 2 || rows.iter().any(|r| r.len() != 2) {
        return MatrixAnalysis {
            verdict: MatrixVerdict::Singular,
            determinant: 0.0,
            rcond_estimate: 0.0,
            inverse: None,
            diagnostics: vec!["matrix-shape-unsupported".to_string()],
        };
    }
    let a = rows[0][0];
    let b = rows[0][1];
    let c = rows[1][0];
    let d = rows[1][1];
    let determinant = a * d - b * c;
    let scale = (a.abs().max(d.abs())).max(1.0);
    let rcond_estimate = determinant.abs() / scale.powi(2);
    if determinant.abs() < f64::EPSILON {
        diagnostics.push("exactly-singular-compensation-matrix".to_string());
        diagnostics.push("silent-inverse-refused".to_string());
        MatrixAnalysis { verdict: MatrixVerdict::Singular, determinant, rcond_estimate, inverse: None, diagnostics }
    } else if determinant.abs() < NEAR_SINGULAR_DETERMINANT || rcond_estimate < NEAR_SINGULAR_RCOND {
        diagnostics.push("near-singular-compensation-matrix".to_string());
        diagnostics.push("silent-inverse-refused".to_string());
        MatrixAnalysis { verdict: MatrixVerdict::NearSingular, determinant, rcond_estimate, inverse: None, diagnostics }
    } else {
        let inverse = vec![
            vec![d / determinant, -b / determinant],
            vec![-c / determinant, a / determinant],
        ];
        MatrixAnalysis { verdict: MatrixVerdict::Invertible, determinant, rcond_estimate, inverse: Some(inverse), diagnostics }
    }
}

pub fn asinh_transform(value: f64, transform: &TransformParams) -> f64 {
    let m = *transform.parameters.get("cofactor_m").unwrap_or(&4.5);
    let t = *transform.parameters.get("top_of_scale_t").unwrap_or(&1000.0);
    let a = *transform.parameters.get("additional_zero_a").unwrap_or(&0.0);
    ((m * ((value - a) / t)).asinh()) / (m.asinh())
}

pub fn derive_points(
    fcs: &FcsFile,
    matrix: &CompensationMatrix,
    transform: &TransformParams,
    analysis: &MatrixAnalysis,
) -> Vec<DerivedPoint> {
    let x_channel = matrix.detector_order[0].clone();
    let y_channel = matrix.detector_order[1].clone();
    fcs.events
        .iter()
        .map(|event| {
            let mut compensated = std::collections::BTreeMap::new();
            let mut diagnostics = Vec::new();
            for channel in &matrix.detector_order {
                let raw = event.values.get(channel).and_then(|v| v.value);
                let comp = if let (Some(raw), Some(inverse)) = (raw, &analysis.inverse) {
                    let row = if channel == &matrix.detector_order[0] { 0 } else { 1 };
                    let x0 = event.values.get(&matrix.detector_order[0]).and_then(|v| v.value);
                    let x1 = event.values.get(&matrix.detector_order[1]).and_then(|v| v.value);
                    match (x0, x1) {
                        (Some(a), Some(b)) => Some(inverse[row][0] * a + inverse[row][1] * b),
                        _ => {
                            diagnostics.push(format!("{channel}-compensation-blocked-by-unconvertible-input"));
                            None
                        }
                    }
                } else {
                    if raw.is_none() {
                        diagnostics.push(format!("{channel}-unconvertible-raw-value-preserved"));
                    }
                    if analysis.inverse.is_none() {
                        diagnostics.push(format!("{channel}-compensation-blocked-by-matrix"));
                    }
                    None
                };
                compensated.insert(channel.clone(), comp);
            }
            let x = compensated.get(&x_channel).copied().flatten().map(|v| asinh_transform(v, transform));
            let y = compensated.get(&y_channel).copied().flatten().map(|v| asinh_transform(v, transform));
            if event.values.values().any(|v| v.diagnostics.iter().any(|d| d.contains("OVERFLOW") || d.contains("extreme"))) {
                diagnostics.push("extreme-signal-retained-in-raw-layer".to_string());
            }
            if x.map(|v| !v.is_finite()).unwrap_or(false) || y.map(|v| !v.is_finite()).unwrap_or(false) {
                diagnostics.push("non-finite-derived-coordinate-not-plotted".to_string());
            }
            DerivedPoint {
                event_id: event.event_id.clone(),
                x,
                y,
                compensated,
                coordinate_basis: transform.coordinate_basis.clone(),
                unit: "dimensionless asinh coordinate".to_string(),
                diagnostics,
            }
        })
        .collect()
}

fn distance_to_segment(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let vx = b[0] - a[0];
    let vy = b[1] - a[1];
    let wx = point[0] - a[0];
    let wy = point[1] - a[1];
    let length_squared = vx * vx + vy * vy;
    let t = if length_squared == 0.0 { 0.0 } else { ((wx * vx + wy * vy) / length_squared).clamp(0.0, 1.0) };
    let px = a[0] + t * vx;
    let py = a[1] + t * vy;
    ((point[0] - px).powi(2) + (point[1] - py).powi(2)).sqrt()
}

fn point_in_polygon(point: [f64; 2], polygon: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let xi = polygon[i][0];
        let yi = polygon[i][1];
        let xj = polygon[j][0];
        let yj = polygon[j][1];
        let intersect = ((yi > point[1]) != (yj > point[1]))
            && (point[0] < (xj - xi) * (point[1] - yi) / (yj - yi) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn gate_memberships(points: &[DerivedPoint], gates: &GateSet, epsilon: f64) -> Vec<GateMembership> {
    let mut result = Vec::new();
    for gate in &gates.gates {
        for point in points {
            let known = match (point.x, point.y) {
                (Some(x), Some(y)) => Some([x, y]),
                _ => None,
            };
            let (inside, boundary, ambiguous, legal, mut diagnostics) = if let Some(known) = known {
                let inside = point_in_polygon(known, &gate.polygon);
                let boundary = gate.polygon.iter().enumerate().any(|(index, a)| {
                    let b = gate.polygon[(index + 1) % gate.polygon.len()];
                    distance_to_segment(known, *a, b) <= epsilon
                });
                let diagnostics = if boundary { vec!["boundary-point-inclusive-rule-recorded".to_string()] } else { Vec::new() };
                (inside, boundary, false, vec![inside], diagnostics)
            } else {
                let mut diagnostics = point.diagnostics.clone();
                diagnostics.push("all-legal-gate-outcomes-retained".to_string());
                (false, false, true, vec![false, true], diagnostics)
            };
            let raw_diag = point.diagnostics.iter().any(|d| d.contains("extreme-signal"));
            if raw_diag {
                diagnostics.push("extreme-signal-candidate-not-cleaned".to_string());
            }
            result.push(GateMembership {
                event_id: point.event_id.clone(),
                gate_id: gate.gate_id.clone(),
                inside,
                boundary,
                ambiguous,
                legal_inside: legal,
                diagnostics,
            });
        }
    }
    result
}

fn cartesian(options: &[Vec<bool>]) -> Vec<Vec<bool>> {
    let mut out = vec![Vec::new()];
    for values in options {
        let mut next = Vec::new();
        for prefix in &out {
            for value in values {
                let mut candidate = prefix.clone();
                candidate.push(*value);
                next.push(candidate);
            }
        }
        out = next;
    }
    out
}

fn population_slug(population_id: &str) -> String {
    population_id.trim_start_matches("POP-").to_ascii_lowercase()
}

pub fn population_candidates(
    fcs: &FcsFile,
    lineage: &Lineage,
    gates: &GateSet,
    memberships: &[GateMembership],
    refs: &Value,
) -> Vec<PopulationCandidate> {
    let mut output = Vec::new();
    let event_ids: Vec<String> = fcs.events.iter().map(|e| e.event_id.clone()).collect();
    let gate_by_id: std::collections::BTreeMap<_, _> = gates.gates.iter().map(|g| (g.gate_id.clone(), g)).collect();

    let root_refs_hash = &stable_hash(&refs)[..8];
    output.push(PopulationCandidate {
        candidate_id: format!("cand-pop-{}-count-{}-r{}", population_slug(&lineage.root_population_id), event_ids.len(), root_refs_hash),
        population_id: lineage.root_population_id.clone(),
        interpretation: "all-imported-events".to_string(),
        members: event_ids.clone(),
        count: event_ids.len(),
        numerator_ids: event_ids.clone(),
        denominator_population_version: json!({"type": "fixed-root", "population": lineage.root_population_id, "version": "root-v1", "pinned": true}),
        denominator_count: event_ids.len(),
        percent: Some(100.0),
        parent_refs: json!({"type": "fixed-root"}),
        refs: refs.clone(),
        status: "proposed".to_string(),
        reason_code: "root-denominator-fixed".to_string(),
        diagnostics: vec!["unconvertible-events-retained-in-raw-layer".to_string()],
    });

    let mut by_population: std::collections::BTreeMap<String, Vec<PopulationCandidate>> = std::collections::BTreeMap::new();
    by_population.insert(lineage.root_population_id.clone(), output.clone());

    for edge in &lineage.edges {
        let gate = gate_by_id.get(&edge.child_gate).expect("lineage gate exists");
        let parents = by_population.get(&edge.parent_population).cloned().unwrap_or_default();
        let mut children = Vec::new();
        for parent in parents {
            let gate_rows: Vec<&GateMembership> = memberships.iter().filter(|m| m.gate_id == gate.gate_id).collect();
            let mut known_members = Vec::new();
            let mut unknown_events = Vec::new();
            let mut unknown_options = Vec::new();
            for event_id in &parent.members {
                let row = gate_rows.iter().find(|m| m.event_id == *event_id).expect("membership exists");
                if row.ambiguous {
                    unknown_events.push(event_id.clone());
                    unknown_options.push(row.legal_inside.clone());
                } else if row.inside {
                    known_members.push(event_id.clone());
                }
            }
            for (choice_index, choices) in cartesian(&unknown_options).into_iter().enumerate() {
                let mut members = known_members.clone();
                let mut interpretation_parts = Vec::new();
                for (event_id, inside) in unknown_events.iter().zip(choices.iter()) {
                    interpretation_parts.push(format!("{event_id}={inside}"));
                    if *inside {
                        members.push(event_id.clone());
                    }
                }
                members.sort();
                let interpretation = if interpretation_parts.is_empty() {
                    "deterministic".to_string()
                } else {
                    interpretation_parts.join(",")
                };
                let count = members.len();
                let candidate_id = format!(
                    "cand-pop-{}-p{}-c{}",
                    population_slug(&edge.child_population),
                    parent.count,
                    count
                );
                let refs_hash = &stable_hash(&refs)[..8];
                let candidate_id = format!("{candidate_id}-r{refs_hash}");
                let candidate_id = if children.iter().any(|c: &PopulationCandidate| c.candidate_id == candidate_id) {
                    format!("{candidate_id}-alt{choice_index}")
                } else {
                    candidate_id
                };
                let percent = if parent.count == 0 { None } else { Some(count as f64 * 100.0 / parent.count as f64) };
                let mut diagnostics = Vec::new();
                if !unknown_events.is_empty() {
                    diagnostics.push("ambiguous-event-candidates-retained".to_string());
                }
                if gate_rows.iter().any(|m| m.boundary && parent.members.contains(&m.event_id)) {
                    diagnostics.push("boundary-point-included-by-closed-gate".to_string());
                }
                children.push(PopulationCandidate {
                    candidate_id: candidate_id.clone(),
                    population_id: edge.child_population.clone(),
                    interpretation,
                    numerator_ids: members.clone(),
                    members,
                    count,
                    denominator_population_version: json!({
                        "population_id": parent.population_id,
                        "candidate_id": parent.candidate_id,
                        "version": parent.candidate_id,
                        "pinned": true
                    }),
                    denominator_count: parent.count,
                    percent,
                    parent_refs: json!({
                        "parent_candidate_id": parent.candidate_id,
                        "parent_population_id": parent.population_id,
                        "parent_refs": parent.refs
                    }),
                    refs: json!({
                        "sources": refs,
                        "gate_id": gate.gate_id,
                        "gate_version": gates.version,
                        "lineage_version": lineage.version,
                        "parent_candidate_id": parent.candidate_id
                    }),
                    status: "proposed".to_string(),
                    reason_code: "all-legal-candidates-retained".to_string(),
                    diagnostics,
                });
            }
        }
        output.extend(children.clone());
        by_population.insert(edge.child_population.clone(), children);
    }
    output
}
