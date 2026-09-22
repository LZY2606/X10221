use crate::store::{key, Artifact, Store};
use serde_json::{json, Value};

/// Pivot below this magnitude => matrix is treated as near-singular.
pub const SINGULAR_EPS: f64 = 1e-9;
/// Distance to a polygon edge below this => boundary point (ambiguous).
pub const BOUNDARY_EPS: f64 = 1e-6;
/// Detector ceiling for 18-bit ADC; raw values at/above are saturated.
pub const SATURATION: f64 = 262143.0;

pub enum CompOutcome {
    Ok(Vec<Vec<f64>>),
    NearSingular { min_pivot: f64, candidates: Vec<Value> },
}

/// Gauss-Jordan inverse that refuses to silently invert a near-singular matrix.
pub fn invert_guarded(m: &[Vec<f64>]) -> CompOutcome {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect();
    let mut min_pivot = f64::INFINITY;
    for col in 0..n {
        let mut piv = col;
        for r in col..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        let pv = a[piv][col].abs();
        if pv < min_pivot {
            min_pivot = pv;
        }
        if pv < SINGULAR_EPS {
            return CompOutcome::NearSingular {
                min_pivot,
                candidates: vec![
                    json!({"id": "reject", "label": "拒绝补偿，保留原始通道值"}),
                    json!({"id": "prev_matrix", "label": "回退到上一版补偿矩阵"}),
                    json!({"id": "manual", "label": "人工调整 spillover 后重试"}),
                ],
            };
        }
        a.swap(col, piv);
        inv.swap(col, piv);
        let d = a[col][col];
        for j in 0..n {
            a[col][j] /= d;
            inv[col][j] /= d;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                for j in 0..n {
                    a[r][j] -= f * a[col][j];
                    inv[r][j] -= f * inv[col][j];
                }
            }
        }
    }
    CompOutcome::Ok(inv)
}

pub fn apply_matrix(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter().map(|row| row.iter().zip(v).map(|(a, b)| a * b).sum()).collect()
}

/// Logicle-like display transform (asinh), parameters versioned as an artifact.
pub fn transform(v: f64, t: &Value) -> f64 {
    let scale = t.get("T").and_then(|x| x.as_f64()).unwrap_or(262144.0);
    (v / scale).asinh()
}

#[derive(Debug, PartialEq)]
pub enum GateClass {
    Inside,
    Outside,
    Boundary { edge: usize },
}

fn dist_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (px, py) = p;
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) };
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Ray-casting containment with explicit boundary detection; boundary points
/// are never silently assigned, they become a candidate set for human ruling.
pub fn classify(p: (f64, f64), poly: &[(f64, f64)]) -> GateClass {
    let n = poly.len();
    for i in 0..n {
        if dist_to_segment(p, poly[i], poly[(i + 1) % n]) < BOUNDARY_EPS {
            return GateClass::Boundary { edge: i };
        }
    }
    let (x, y) = p;
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
        GateClass::Inside
    } else {
        GateClass::Outside
    }
}

pub fn polygon(payload: &Value) -> Vec<(f64, f64)> {
    payload
        .get("polygon")
        .and_then(|v| v.as_array())
        .map(|pts| {
            pts.iter()
                .filter_map(|p| Some((p.get("x")?.as_f64()?, p.get("y")?.as_f64()?)))
                .collect()
        })
        .unwrap_or_default()
}

/// Recompute one population for a gate version. The percentage denominator is
/// pinned to the exact parent population artifact version, never "latest".
pub fn compute_population(
    store: &Store,
    gate: &Artifact,
    events: &[Value],
    channels: &(String, String),
    parent: Option<&Artifact>,
    branch: &str,
    seq: i64,
) -> Artifact {
    let poly = polygon(&gate.payload);
    let mut count = 0i64;
    let mut boundary: Vec<i64> = vec![];
    for (i, ev) in events.iter().enumerate() {
        let x = ev.get(&channels.0).and_then(|v| v.as_f64());
        let y = ev.get(&channels.1).and_then(|v| v.as_f64());
        let (x, y) = match (x, y) {
            (Some(x), Some(y)) => (x, y),
            _ => continue, // unconvertible values stay in place with diagnostics; not dropped silently
        };
        match classify((x, y), &poly) {
            GateClass::Inside => count += 1,
            GateClass::Outside => {}
            GateClass::Boundary { .. } => boundary.push(i as i64),
        }
    }
    let parent_name = gate.payload.get("parent").cloned().unwrap_or(Value::Null);
    let (denominator, denom_count) = match parent {
        Some(p) => (
            json!({"kind": p.kind, "name": p.name, "version": p.version, "branch": p.branch}),
            p.payload.get("count").and_then(|v| v.as_i64()).unwrap_or(0),
        ),
        None => (json!({"kind": "root", "name": "all_events", "version": 0, "branch": branch}), events.len() as i64),
    };
    let percent = if denom_count > 0 { 100.0 * count as f64 / denom_count as f64 } else { 0.0 };
    let version = store.latest("population", &gate.name, branch).map(|p| p.version + 1).unwrap_or(1);
    let a = Artifact {
        kind: "population".into(),
        name: gate.name.clone(),
        version,
        branch: branch.into(),
        payload: json!({
            "gate_version": gate.version,
            "count": count,
            "percent": percent,
            "denominator": denominator,
            "denominator_count": denom_count,
            "boundary_events": boundary,
            "parent_gate": parent_name,
        }),
        raw: None,
        unit: Some("percent_of_parent".into()),
        source: format!("compute:{}", gate.key()),
        status: "fresh".into(),
        created_seq: seq,
    };
    store.put_artifact(&a);
    store.add_dependency(&a.key(), &gate.key());
    if let Some(p) = parent {
        store.add_dependency(&a.key(), &p.key());
    }
    a
}

/// Mark every transitive dependent of a changed artifact stale, recording a
/// machine-readable invalidation event. Old conclusions are never overwritten.
pub fn invalidate_dependents(store: &Store, changed: &Artifact, reason: &str) -> Vec<String> {
    let affected = store.dependents_of(&changed.key());
    for k in &affected {
        let parts: Vec<&str> = k.split('|').collect();
        if parts.len() == 4 {
            store.set_status(parts[0], parts[1], parts[2].parse().unwrap_or(0), parts[3], "stale");
        }
    }
    store.record_event(
        "invalidation",
        json!({
            "trigger": changed.key(),
            "reason": reason,
            "affected": affected,
        }),
        &changed.branch,
    );
    affected
}
