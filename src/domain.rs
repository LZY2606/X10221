use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const CASE_ID: &str = "flow-boundary-chamber-v1";
pub const MANIFEST: &str = include_str!("../case/flow-boundary-chamber/manifest.json");
pub const RAW_EVENTS: &str = include_str!("../case/flow-boundary-chamber/raw-events.json");
pub const SPILLOVER: &str = include_str!("../case/flow-boundary-chamber/spillover.json");
pub const COMP_MATRICES: &str = include_str!("../case/flow-boundary-chamber/compensation-matrices.json");
pub const TRANSFORMS: &str = include_str!("../case/flow-boundary-chamber/transform-parameters.json");
pub const GATES: &str = include_str!("../case/flow-boundary-chamber/gates.json");
pub const PARENT_LINKS: &str = include_str!("../case/flow-boundary-chamber/parent-links.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBundle {
    pub path: String,
    pub role: String,
    pub content: String,
    pub sha256: String,
}

pub fn case_files() -> Vec<FileBundle> {
    [
        ("manifest.json", "manifest", MANIFEST),
        ("raw-events.json", "fcs-events", RAW_EVENTS),
        ("spillover.json", "spillover-metadata", SPILLOVER),
        ("compensation-matrices.json", "compensation-matrices", COMP_MATRICES),
        ("transform-parameters.json", "transform-parameters", TRANSFORMS),
        ("gates.json", "gate-polygons", GATES),
        ("parent-links.json", "parent-links", PARENT_LINKS),
    ]
    .iter()
    .map(|(path, role, content)| FileBundle {
        path: (*path).to_string(),
        role: (*role).to_string(),
        content: (*content).to_string(),
        sha256: sha256_text(content),
    })
    .collect()
}

pub fn sha256_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn sha256_json(value: &Value) -> String {
    let canonical = serde_json::to_string(value).unwrap_or_default();
    sha256_text(&canonical)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompensationStatus {
    Accepted,
    Candidate,
    NearSingularBlocked,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensatedValue {
    pub channel: String,
    pub raw_value: Option<f64>,
    pub compensated_value: Option<f64>,
    pub raw_unit: String,
    pub compensated_unit: String,
    pub diagnostics: Vec<Value>,
}

pub fn determinant3(matrix: &[[f64; 3]; 3]) -> f64 {
    matrix[0][0] * (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1])
        - matrix[0][1] * (matrix[1][0] * matrix[2][2] - matrix[1][2] * matrix[2][0])
        + matrix[0][2] * (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0])
}

pub fn invert_spillover(matrix: &[Vec<f64>], threshold: f64) -> Result<Vec<Vec<f64>>, String> {
    if matrix.len() != 3 || matrix.iter().any(|row| row.len() != 3) {
        return Err("fixture compensation must be a 3x3 matrix".to_string());
    }
    let mut fixed = [[0.0; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            fixed[row][col] = matrix[row][col];
        }
    }
    let determinant = determinant3(&fixed);
    if !determinant.is_finite() {
        return Err("compensation matrix contains non-finite coefficients".to_string());
    }
    if determinant.abs() < threshold {
        return Err(format!(
            "near-singular compensation matrix blocked: |det|={determinant:.3e} is below threshold {threshold:.3e}; no inverse was applied"
        ));
    }

    let mut inverse = [[0.0; 3]; 3];
 for row in 0..3 {
        for col in 0..3 {
            let minor = [
                [fixed[(row + 1) % 3][(col + 1) % 3], fixed[(row + 1) % 3][(col + 2) % 3]],
                [fixed[(row + 2) % 3][(col + 1) % 3], fixed[(row + 2) % 3][(col + 2) % 3]],
            ];
            let cofactor = minor[0][0] * minor[1][1] - minor[0][1] * minor[1][0];
            inverse[col][row] = cofactor / determinant;
        }
    }
    Ok(inverse.iter().map(|row| row.to_vec()).collect())
}

pub fn asinh_value(value: f64, cofactor: f64) -> f64 {
    (value / cofactor).asinh()
}

pub fn point_in_polygon(x: f64, y: f64, polygon: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let mut previous = polygon.len() - 1;
    for current in 0..polygon.len() {
        let (px, py) = (polygon[previous][0], polygon[previous][1]);
        let (cx, cy) = (polygon[current][0], polygon[current][1]);
        let intersects = ((cy > y) != (py > y))
            && (x < (px - cx) * (y - cy) / (py - cy) + cx);
        if intersects {
            inside = !inside;
        }
        previous = current;
    }
    if inside {
        return true;
    }
    let tolerance = 1e-9;
    for pair in polygon.windows(2).chain(std::iter::once([polygon[0], polygon[polygon.len() - 1]]).iter()) {
        let (ax, ay) = (pair[0][0], pair[0][1]);
        let (bx, by) = (pair[1][0], pair[1][1]);
        let cross = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
        let within_x = x >= ax.min(bx) - tolerance && x <= ax.max(bx) + tolerance;
        let within_y = y >= ay.min(by) - tolerance && y <= ay.max(by) + tolerance;
        if cross.abs() <= tolerance && within_x && within_y {
            return true;
        }
    }
    false
}

pub fn gate_membership(
    event_id: &str,
    coordinates: &[(String, Option<f64>)],
    gate: &Value,
) -> Result<bool, String> {
    let x_channel = gate["x_channel"].as_str().ok_or("gate missing x_channel")?;
    let y_channel = gate["y_channel"].as_str().ok_or("gate missing y_channel")?;
    let x = coordinate_for(event_id, coordinates, x_channel)?;
    let y = coordinate_for(event_id, coordinates, y_channel)?;
    let coordinates_value = gate["coordinates"].as_array().ok_or("gate missing polygon")?;
    let mut polygon = Vec::new();
    for point in coordinates_value {
        let x = point[0].as_f64().ok_or("invalid polygon x")?;
        let y = point[1].as_f64().ok_or("invalid polygon y")?;
        polygon.push([x, y]);
    }
    Ok(point_in_polygon(x, y, &polygon))
}

fn coordinate_for(event_id: &str, coordinates: &[(String, Option<f64>)], channel: &str) -> Result<f64, String> {
    coordinates
        .iter()
        .find(|(name, _)| name == channel)
        .and_then(|(_, value)| *value)
        .ok_or_else(|| format!("{event_id} has no usable {channel} coordinate; source value is retained but not plotted"))
}

pub fn parent_version_key(parent_gate_result_id: Option<&str>) -> String {
    parent_gate_result_id.unwrap_or("P-ROOT@ALL").to_string()
}

pub fn membership_hash(event_ids: &[String]) -> String {
    let mut joined = event_ids.join(",");
    joined.insert(0, '|');
    sha256_text(&joined)
}
