//! Point-in-polygon gating with explicit boundary classification.
//! Boundary points are INSIDE and flagged `on_boundary` (ray casting plus an
//! epsilon edge-distance test). Coordinate basis must match the gate basis.

use serde_json::{json, Value};

pub fn point_in_poly(px: f64, py: f64, poly: &[[f64; 2]]) -> (bool, bool) {
    let n = poly.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > py) != (yj > py) {
            let x_cross = (xj - xi) * (py - yi) / (yj - yi) + xi;
            if px < x_cross {
                inside = !inside;
            }
        }
        j = i;
    }
    let on_boundary = on_edge(px, py, poly);
    (inside || on_boundary, on_boundary)
}

fn on_edge(px: f64, py: f64, poly: &[[f64; 2]]) -> bool {
    const EPS: f64 = 1e-9;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let len2 = dx * dx + dy * dy;
        if len2 == 0.0 {
            if (px - a[0]).abs() < EPS && (py - a[1]).abs() < EPS { return true; }
            continue;
        }
        let mut t = ((px - a[0]) * dx + (py - a[1]) * dy) / len2;
        if !t.is_finite() { continue; }
        t = t.clamp(0.0, 1.0);
        let qx = a[0] + t * dx;
        let qy = a[1] + t * dy;
        let d = ((px - qx).powi(2) + (py - qy).powi(2)).sqrt();
        if d < EPS { return true; }
    }
    false
}

/// Evaluate a single gate over an event universe (already parent-filtered).
/// `channels` maps column index -> channel name; `basis` rows are the values
/// the gate coordinates are expressed in.
pub fn evaluate_gate(
    gate: &Value,
    universe: &[i64],
    basis: &std::collections::HashMap<i64, Vec<f64>>,
    channels: &[String],
) -> Value {
    let x_name = gate["x_channel"].as_str().unwrap_or("").to_string();
    let y_name = gate["y_channel"].as_str().unwrap_or("").to_string();
    let xi = channels.iter().position(|c| *c == x_name);
    let yi = channels.iter().position(|c| *c == y_name);
    let verts: Vec<[f64; 2]> = gate["polygon"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| [p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0)])
                .collect()
        })
        .unwrap_or_default();

    let mut members = Vec::new();
    let mut boundary = Vec::new();
    let mut excluded = Vec::new();
    for &ev in universe {
        let coords = basis.get(&ev);
        let (x, y) = match (coords, xi, yi) {
            (Some(row), Some(xi), Some(yi)) => (row.get(xi).copied(), row.get(yi).copied()),
            _ => (None, None),
        };
        match (x, y) {
            (Some(x), Some(y)) if x.is_finite() && y.is_finite() => {
                let (inside, on) = point_in_poly(x, y, &verts);
                if inside {
                    members.push(ev);
                    if on { boundary.push(ev); }
                } else {
                    excluded.push(ev);
                }
            }
            _ => excluded.push(ev),
        }
    }
    json!({
        "members": members,
        "boundary_members": boundary,
        "excluded": excluded,
        "universe_size": universe.len(),
        "coordinate_basis": gate["coordinate_basis"],
        "unit": gate.get("unit").cloned().unwrap_or(json!("asinh_units"))
    })
}
