//! Derivation rules. Every judgment is explicit and machine-readable:
//! - near-singular compensation matrices are never silently inverted;
//! - gate percentage denominators are pinned to the parent population version;
//! - revising an upstream gate marks downstream results stale, never overwrites.

use serde_json::{json, Value};

/// Condition-number estimate above which a matrix is treated as near-singular.
pub const NEAR_SINGULAR_COND: f64 = 1.0e10;
/// Distance to a polygon edge below which a point is "on the boundary".
pub const BOUNDARY_EPS: f64 = 1.0e-6;
/// Detector saturation ceiling for 18-bit ADC raw scale.
pub const DETECTOR_MAX: f64 = 262_144.0;

pub type Matrix = Vec<Vec<f64>>;

/// LU-based condition estimate. Returns (cond_estimate, min_pivot, det).
pub fn matrix_condition(m: &Matrix) -> (f64, f64, f64) {
    let n = m.len();
    let mut a = m.clone();
    let mut det = 1.0f64;
    let mut min_pivot = f64::INFINITY;
    let mut max_pivot = 0.0f64;
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        let p = a[piv][col].abs();
        if p < min_pivot {
            min_pivot = p;
        }
        if p > max_pivot {
            max_pivot = p;
        }
        if piv != col {
            a.swap(piv, col);
            det = -det;
        }
        det *= a[col][col];
        if a[col][col].abs() < 1e-300 {
            return (f64::INFINITY, 0.0, 0.0);
        }
        for r in (col + 1)..n {
            let f = a[r][col] / a[col][col];
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
        }
    }
    let cond = if min_pivot == 0.0 {
        f64::INFINITY
    } else {
        max_pivot / min_pivot
    };
    (cond, min_pivot, det)
}

/// Invert a matrix by Gauss-Jordan. Caller MUST check `matrix_condition` first;
/// this function refuses to invert near-singular matrices and returns None.
pub fn invert_checked(m: &Matrix) -> Option<Matrix> {
    let (cond, _, _) = matrix_condition(m);
    if !cond.is_finite() || cond > NEAR_SINGULAR_COND {
        return None;
    }
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
            r
        })
        .collect();
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(piv, col);
        let d = a[col][col];
        for c in 0..2 * n {
            a[col][c] /= d;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                for c in 0..2 * n {
                    a[r][c] -= f * a[col][c];
                }
            }
        }
    }
    Some(a.into_iter().map(|r| r[n..].to_vec()).collect())
}

/// Compensation judgment for a matrix version. Never silently inverts.
pub fn judge_compensation(matrix_id: &str, version: i64, m: &Matrix) -> Value {
    let (cond, min_pivot, det) = matrix_condition(m);
    if !cond.is_finite() || cond > NEAR_SINGULAR_COND {
        json!({
            "rule": "comp.near_singular",
            "matrix_id": matrix_id,
            "matrix_version": version,
            "status": "blocked",
            "condition_estimate": cond,
            "min_pivot": min_pivot,
            "determinant": det,
            "basis": "fluorescence-intensity/linear",
            "rationale": "near-singular compensation matrix must not be silently inverted; raw values kept in place",
            "action": "values_kept_raw_with_diagnostic"
        })
    } else {
        json!({
            "rule": "comp.invertible",
            "matrix_id": matrix_id,
            "matrix_version": version,
            "status": "ok",
            "condition_estimate": cond,
            "determinant": det,
            "basis": "fluorescence-intensity/linear"
        })
    }
}

/// Classify one raw signal against detector range.
pub fn judge_signal(channel: &str, raw: &str, value: Option<f64>) -> Option<Value> {
    match value {
        None => Some(json!({
            "rule": "signal.unconvertible",
            "channel": channel,
            "raw": raw,
            "status": "diagnostic",
            "rationale": "value kept in original position; not dropped for plotting"
        })),
        Some(v) if v < 0.0 || v > DETECTOR_MAX => Some(json!({
            "rule": "signal.extreme",
            "channel": channel,
            "raw": raw,
            "value": v,
            "unit": "fluorescence-intensity",
            "detector_max": DETECTOR_MAX,
            "status": "diagnostic",
            "rationale": "extreme signal outside detector range; kept raw with diagnostic"
        })),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointClass {
    Inside,
    Outside,
    Boundary,
}

/// Ray-casting point-in-polygon with boundary detection.
pub fn classify_point(x: f64, y: f64, poly: &[(f64, f64)]) -> PointClass {
    let n = poly.len();
    if n < 3 {
        return PointClass::Outside;
    }
    for i in 0..n {
        let (x1, y1) = poly[i];
        let (x2, y2) = poly[(i + 1) % n];
        if point_seg_dist(x, y, x1, y1, x2, y2) < BOUNDARY_EPS {
            return PointClass::Boundary;
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        PointClass::Inside
    } else {
        PointClass::Outside
    }
}

fn point_seg_dist(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-300 {
        0.0
    } else {
        (((px - x1) * dx + (py - y1) * dy) / len2).clamp(0.0, 1.0)
    };
    let cx = x1 + t * dx;
    let cy = y1 + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Apply a transform to a compensated value. Units/basis travel with the value.
pub fn apply_transform(kind: &str, params: &Value, v: f64) -> Option<f64> {
    match kind {
        "linear" => {
            let gain = params.get("gain").and_then(|g| g.as_f64()).unwrap_or(1.0);
            Some(v * gain)
        }
        "log" => {
            if v > 0.0 {
                Some(v.log10())
            } else {
                None
            }
        }
        "asinh" => {
            let cofactor = params
                .get("cofactor")
                .and_then(|c| c.as_f64())
                .unwrap_or(150.0);
            Some((v / cofactor).asinh())
        }
        _ => None,
    }
}

/// Compute which populations go stale when a dependency version changes.
/// `populations`: rows of (pop_id, draft_id, deps-json). Returns affected ids.
pub fn affected_populations(
    populations: &[(String, String, Value)],
    dep_kind: &str,
    dep_id: &str,
    dep_version: i64,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (pop_id, draft_id, deps) in populations {
        let pinned = deps.get(dep_kind).and_then(|d| d.get("id")).and_then(|i| i.as_str());
        let pinned_v = deps
            .get(dep_kind)
            .and_then(|d| d.get("version"))
            .and_then(|v| v.as_i64());
        if pinned == Some(dep_id) && pinned_v.map(|v| v < dep_version).unwrap_or(false) {
            out.push((pop_id.clone(), draft_id.clone()));
        }
    }
    out
}

/// Percent denominator rule: denominator must come from the pinned parent
/// population *version*, never from a silently-updated latest value.
pub fn percent_of_parent(count: u64, parent: &Value) -> Value {
    let parent_count = parent.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
    let pct = if parent_count == 0 {
        None
    } else {
        Some(count as f64 * 100.0 / parent_count as f64)
    };
    json!({
        "rule": "gate.percent_denominator",
        "denominator_source": {
            "population_id": parent.get("pop_id"),
            "population_version": parent.get("version"),
            "count": parent_count
        },
        "percent": pct,
        "rationale": "denominator pinned to parent population version"
    })
}
