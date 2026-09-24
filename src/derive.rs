use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

pub const EXTREME_LIMIT: f64 = 1_048_576.0;
pub const NEAR_SINGULAR_COND: f64 = 1.0e8;
pub const SINGULAR_PIVOT: f64 = 1.0e-12;
pub const BOUNDARY_EPS: f64 = 1.0e-6;
pub const REG_LAMBDA: f64 = 1.0e-6;
pub const KNOWN_UNITS: [&str; 4] = ["fluorescence", "area", "height", "seconds"];

pub const RULE_NEAR_SINGULAR: &str = "near_singular_matrix_must_not_be_silently_inverted";
pub const RULE_DENOMINATOR: &str = "denominator_pinned_to_parent_population_version";
pub const RULE_STALE: &str = "upstream_revision_marks_stale_not_overwrite";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    pub unit: String,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsData {
    pub channels: Vec<Channel>,
    pub events: Vec<Vec<f64>>,
    pub raw_repr: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensationMatrix {
    pub channels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformParams {
    pub method: String,
    pub basis: String,
    pub cofactor: HashMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateDef {
    pub channel_x: String,
    pub channel_y: String,
    pub polygon: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSet {
    pub basis: String,
    pub gates: HashMap<String, GateDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub gate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateTree {
    pub nodes: Vec<TreeNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatrixVerdict {
    Ok { inv: Vec<Vec<f64>>, det: f64, cond: f64 },
    NearSingular { det: f64, cond: f64 },
    Singular { det: f64 },
}

fn inf_norm(m: &[Vec<f64>]) -> f64 {
    m.iter()
        .map(|row| row.iter().map(|v| v.abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

pub fn analyze_matrix(m: &[Vec<f64>]) -> MatrixVerdict {
    let n = m.len();
    let scale = m
        .iter()
        .flatten()
        .map(|v| v.abs())
        .fold(0.0, f64::max)
        .max(1e-300);
    let mut a: Vec<Vec<f64>> = m
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
            r
        })
        .collect();
    let mut det = 1.0;
    for col in 0..n {
        let mut piv = col;
        let mut best = a[col][col].abs();
        for r in (col + 1)..n {
            if a[r][col].abs() > best {
                best = a[r][col].abs();
                piv = r;
            }
        }
        if best <= SINGULAR_PIVOT * scale {
            return MatrixVerdict::Singular { det: 0.0 };
        }
        if piv != col {
            a.swap(piv, col);
            det = -det;
        }
        let d = a[col][col];
        det *= d;
        for r in 0..n {
            if r != col {
                let f = a[r][col] / d;
                if f != 0.0 {
                    for c in 0..(2 * n) {
                        a[r][c] -= f * a[col][c];
                    }
                }
            }
        }
    }
    for col in 0..n {
        let d = a[col][col];
        for c in 0..(2 * n) {
            a[col][c] /= d;
        }
    }
    let inv: Vec<Vec<f64>> = a.iter().map(|r| r[n..].to_vec()).collect();
    let cond = inf_norm(m) * inf_norm(&inv);
    if cond > NEAR_SINGULAR_COND {
        MatrixVerdict::NearSingular { det, cond }
    } else {
        MatrixVerdict::Ok { inv, det, cond }
    }
}

pub fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum())
        .collect()
}

pub fn regularized_inverse(m: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut mtm = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += m[k][i] * m[k][j];
            }
            mtm[i][j] = s + if i == j { lambda } else { 0.0 };
        }
    }
    let mt: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| m[j][i]).collect()).collect();
    match analyze_matrix(&mtm) {
        MatrixVerdict::Ok { inv, .. } => {
            let mut out = vec![vec![0.0; n]; n];
            for i in 0..n {
                for j in 0..n {
                    let mut s = 0.0;
                    for k in 0..n {
                        s += inv[i][k] * mt[k][j];
                    }
                    out[i][j] = s;
                }
            }
            Some(out)
        }
        _ => None,
    }
}

pub fn asinh_transform(v: f64, cofactor: f64) -> f64 {
    let x = v / cofactor;
    (x + (x * x + 1.0).sqrt()).ln()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointClass {
    Inside,
    Outside,
    Boundary,
}

fn dist_point_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

pub fn classify_point(px: f64, py: f64, poly: &[[f64; 2]]) -> PointClass {
    let n = poly.len();
    let mut min_d = f64::MAX;
    let mut scale = 1.0f64;
    for i in 0..n {
        let [ax, ay] = poly[i];
        let [bx, by] = poly[(i + 1) % n];
        scale = scale.max(ax.abs()).max(ay.abs()).max(bx.abs()).max(by.abs());
        min_d = min_d.min(dist_point_segment(px, py, ax, ay, bx, by));
    }
    if min_d <= BOUNDARY_EPS * scale {
        return PointClass::Boundary;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
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

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticOut {
    pub id: String,
    pub code: String,
    pub subject: String,
    pub severity: String,
    pub detail: Value,
    pub candidates: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopulationOut {
    pub node_id: String,
    pub name: String,
    pub parent: Option<String>,
    pub status: String,
    pub count: Option<usize>,
    pub count_include_boundary: usize,
    pub count_exclude_boundary: usize,
    pub boundary_events: Vec<usize>,
    pub comp_variants: HashMap<String, usize>,
    pub percent: Option<f64>,
    pub denominator_count: Option<usize>,
    pub deps_gates: Vec<String>,
    pub deps_channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineOutput {
    pub populations: Vec<PopulationOut>,
    pub diagnostics: Vec<DiagnosticOut>,
    pub total_events: usize,
    pub comp_mode: String,
    pub matrix_detail: Value,
}

enum CompMode {
    Direct(Vec<Vec<f64>>),
    Regularized(Vec<Vec<f64>>),
    Skip,
    Blocked,
}

pub fn run(
    fcs: &FcsData,
    comp: &CompensationMatrix,
    tp: &TransformParams,
    gates: &GateSet,
    tree: &GateTree,
    rulings: &HashMap<String, String>,
) -> EngineOutput {
    let mut diags: Vec<DiagnosticOut> = Vec::new();

    for ch in &fcs.channels {
        if !KNOWN_UNITS.contains(&ch.unit.as_str()) {
            diags.push(DiagnosticOut {
                id: format!("diag:unconvertible_unit:{}", ch.name),
                code: "unconvertible_unit".into(),
                subject: ch.name.clone(),
                severity: "info".into(),
                detail: json!({
                    "unit": ch.unit,
                    "basis": ch.basis,
                    "kept": "raw value left in original position; excluded from normalization",
                }),
                candidates: vec![],
            });
        }
    }

    let extreme: Vec<usize> = fcs
        .events
        .iter()
        .enumerate()
        .filter(|(_, ev)| ev.iter().any(|v| v.abs() > EXTREME_LIMIT))
        .map(|(i, _)| i)
        .collect();
    if !extreme.is_empty() {
        diags.push(DiagnosticOut {
            id: "diag:extreme_signal:fcs_events".into(),
            code: "extreme_signal".into(),
            subject: "fcs_events".into(),
            severity: "info".into(),
            detail: json!({
                "events": extreme,
                "limit": EXTREME_LIMIT,
                "kept": true,
                "note": "raw values preserved in place; not clipped, not dropped",
            }),
            candidates: vec![json!({"id": "keep_raw", "desc": "keep raw values with diagnostic flag"})],
        });
    }

    let verdict = analyze_matrix(&comp.matrix);
    let (mode, matrix_detail) = match &verdict {
        MatrixVerdict::Ok { inv, det, cond } => (
            CompMode::Direct(inv.clone()),
            json!({"det": det, "cond": cond, "verdict": "ok"}),
        ),
        MatrixVerdict::NearSingular { det, cond } => {
            diags.push(DiagnosticOut {
                id: "diag:near_singular_matrix:compensation_matrix".into(),
                code: "near_singular_matrix".into(),
                subject: "compensation_matrix".into(),
                severity: "blocking".into(),
                detail: json!({
                    "det": det, "cond": cond,
                    "rule": RULE_NEAR_SINGULAR,
                    "note": "matrix must not be silently inverted; reviewer ruling required",
                }),
                candidates: vec![
                    json!({"id": "regularize", "lambda": REG_LAMBDA, "desc": "Tikhonov-regularized pseudo-inverse"}),
                    json!({"id": "skip", "desc": "use uncompensated signals"}),
                ],
            });
            (comp_mode_from_ruling(&comp.matrix, rulings), json!({"det": det, "cond": cond, "verdict": "near_singular"}))
        }
        MatrixVerdict::Singular { det } => {
            diags.push(DiagnosticOut {
                id: "diag:singular_matrix:compensation_matrix".into(),
                code: "singular_matrix".into(),
                subject: "compensation_matrix".into(),
                severity: "blocking".into(),
                detail: json!({
                    "det": det,
                    "rule": RULE_NEAR_SINGULAR,
                    "note": "matrix is exactly singular; inversion impossible",
                }),
                candidates: vec![
                    json!({"id": "regularize", "lambda": REG_LAMBDA, "desc": "Tikhonov-regularized pseudo-inverse"}),
                    json!({"id": "skip", "desc": "use uncompensated signals"}),
                ],
            });
            (comp_mode_from_ruling(&comp.matrix, rulings), json!({"det": det, "verdict": "singular"}))
        }
    };

    let blocked = matches!(mode, CompMode::Blocked);
    let comp_mode_name = match &mode {
        CompMode::Direct(_) => "inverse",
        CompMode::Regularized(_) => "regularize",
        CompMode::Skip => "skip",
        CompMode::Blocked => "blocked",
    }
    .to_string();

    let channel_index: HashMap<String, usize> = fcs
        .channels
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.clone(), i))
        .collect();
    let comp_idx: Vec<usize> = comp
        .channels
        .iter()
        .filter_map(|c| channel_index.get(c).copied())
        .collect();

    let apply_variant = |variant: &str| -> Vec<Vec<f64>> {
        let inv: Option<Vec<Vec<f64>>> = match variant {
            "regularize" => regularized_inverse(&comp.matrix, REG_LAMBDA),
            "skip" => None,
            _ => match &verdict {
                MatrixVerdict::Ok { inv, .. } => Some(inv.clone()),
                _ => None,
            },
        };
        fcs.events
            .iter()
            .map(|ev| {
                let mut out = ev.clone();
                if let Some(inv) = &inv {
                    let sub: Vec<f64> = comp_idx.iter().map(|&i| ev[i]).collect();
                    let comped = mat_vec(inv, &sub);
                    for (k, &ci) in comp_idx.iter().enumerate() {
                        out[ci] = comped[k];
                    }
                }
                out
            })
            .collect()
    };

    let transform_all = |rows: &[Vec<f64>]| -> Vec<Vec<f64>> {
        rows.iter()
            .map(|ev| {
                fcs.channels
                    .iter()
                    .enumerate()
                    .map(|(i, ch)| {
                        if let Some(cof) = tp.cofactor.get(&ch.name) {
                            asinh_transform(ev[i], *cof)
                        } else {
                            ev[i]
                        }
                    })
                    .collect()
            })
            .collect()
    };

    let compensated_active: Vec<Vec<f64>> = match &mode {
        CompMode::Direct(inv) => fcs
            .events
            .iter()
            .map(|ev| {
                let mut out = ev.clone();
                let sub: Vec<f64> = comp_idx.iter().map(|&i| ev[i]).collect();
                let comped = mat_vec(inv, &sub);
                for (k, &ci) in comp_idx.iter().enumerate() {
                    out[ci] = comped[k];
                }
                out
            })
            .collect(),
        CompMode::Regularized(inv) => {
            let rows = apply_variant("regularize");
            let _ = inv;
            rows
        }
        CompMode::Skip => fcs.events.clone(),
        CompMode::Blocked => fcs.events.clone(),
    };
    let transformed_active = transform_all(&compensated_active);

    let variant_transformed: HashMap<String, Vec<Vec<f64>>> = if blocked {
        let mut m = HashMap::new();
        m.insert("regularize".to_string(), transform_all(&apply_variant("regularize")));
        m.insert("skip".to_string(), transform_all(&apply_variant("skip")));
        m
    } else {
        HashMap::new()
    };

    let boundary_policy = rulings.get("boundary_policy").cloned();

    let node_map: HashMap<&str, &TreeNode> = tree.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut memberships: HashMap<String, Vec<bool>> = HashMap::new();
    let mut populations: Vec<PopulationOut> = Vec::new();

    for node in &tree.nodes {
        let parent_mask: Vec<bool> = match &node.parent {
            Some(p) => memberships.get(p).cloned().unwrap_or_else(|| vec![true; fcs.events.len()]),
            None => vec![true; fcs.events.len()],
        };
        let mut mask = parent_mask.clone();
        let mut boundary_events: Vec<usize> = Vec::new();
        let mut inside_count: usize = parent_mask.iter().filter(|m| **m).count();
        let mut deps_gates: Vec<String> = Vec::new();
        let mut deps_channels: Vec<String> = Vec::new();
        {
            let mut cur: Option<&TreeNode> = Some(node);
            while let Some(n) = cur {
                if let Some(g) = &n.gate {
                    deps_gates.push(g.clone());
                    if let Some(gd) = gates.gates.get(g) {
                        for c in [&gd.channel_x, &gd.channel_y] {
                            if !deps_channels.contains(c) {
                                deps_channels.push(c.clone());
                            }
                        }
                    }
                }
                cur = n.parent.as_ref().and_then(|p| node_map.get(p.as_str()).copied());
            }
            deps_gates.reverse();
            deps_channels.reverse();
        }
        if let Some(gid) = &node.gate {
            if let Some(gd) = gates.gates.get(gid) {
                let xi = channel_index[&gd.channel_x];
                let yi = channel_index[&gd.channel_y];
                inside_count = 0;
                for (i, row) in transformed_active.iter().enumerate() {
                    if !parent_mask[i] {
                        mask[i] = false;
                        continue;
                    }
                    match classify_point(row[xi], row[yi], &gd.polygon) {
                        PointClass::Inside => {
                            inside_count += 1;
                            mask[i] = true;
                        }
                        PointClass::Outside => mask[i] = false,
                        PointClass::Boundary => {
                            boundary_events.push(i);
                            mask[i] = boundary_policy.as_deref() == Some("include");
                        }
                    }
                }
            }
        }
        let count_include = inside_count + boundary_events.len();
        let count_exclude = inside_count;
        let count = if blocked {
            None
        } else if boundary_events.is_empty() {
            Some(count_include)
        } else {
            match boundary_policy.as_deref() {
                Some("include") => Some(count_include),
                Some("exclude") => Some(count_exclude),
                _ => None,
            }
        };
        let mut comp_variants = HashMap::new();
        if blocked {
            for (name, rows) in &variant_transformed {
                let mut cnt = 0usize;
                if let Some(gid) = &node.gate {
                    if let Some(gd) = gates.gates.get(gid) {
                        let xi = channel_index[&gd.channel_x];
                        let yi = channel_index[&gd.channel_y];
                        for (i, row) in rows.iter().enumerate() {
                            if !parent_mask[i] {
                                continue;
                            }
                            if classify_point(row[xi], row[yi], &gd.polygon) == PointClass::Inside {
                                cnt += 1;
                            }
                        }
                    }
                } else {
                    cnt = parent_mask.iter().filter(|m| **m).count();
                }
                comp_variants.insert(name.clone(), cnt);
            }
        }
        let status = if blocked {
            "blocked"
        } else if count.is_none() {
            "undecided"
        } else {
            "ok"
        };
        memberships.insert(node.id.clone(), mask);
        populations.push(PopulationOut {
            node_id: node.id.clone(),
            name: node.name.clone(),
            parent: node.parent.clone(),
            status: status.to_string(),
            count,
            count_include_boundary: count_include,
            count_exclude_boundary: count_exclude,
            boundary_events,
            comp_variants,
            percent: None,
            denominator_count: None,
            deps_gates,
            deps_channels,
        });
    }

    let total = fcs.events.len();
    let mut counts_by_node: HashMap<String, Option<usize>> = HashMap::new();
    for p in &mut populations {
        let (denom_count, percent) = match &p.parent {
            None => (Some(total), p.count.map(|c| 100.0 * c as f64 / total.max(1) as f64)),
            Some(par) => {
                let pc = counts_by_node.get(par).copied().flatten();
                (pc, match (p.count, pc) {
                    (Some(c), Some(d)) if d > 0 => Some(100.0 * c as f64 / d as f64),
                    _ => None,
                })
            }
        };
        p.denominator_count = denom_count;
        p.percent = percent;
        counts_by_node.insert(p.node_id.clone(), p.count);
    }

    EngineOutput {
        populations,
        diagnostics: diags,
        total_events: total,
        comp_mode: comp_mode_name,
        matrix_detail,
    }
}

fn comp_mode_from_ruling(matrix: &[Vec<f64>], rulings: &HashMap<String, String>) -> CompMode {
    match rulings.get("compensation").map(|s| s.as_str()) {
        Some("regularize") => regularized_inverse(matrix, REG_LAMBDA)
            .map(CompMode::Regularized)
            .unwrap_or(CompMode::Skip),
        Some("skip") => CompMode::Skip,
        _ => CompMode::Blocked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn well_conditioned() -> Vec<Vec<f64>> {
        vec![
            vec![1.00, 0.15, 0.02],
            vec![0.05, 1.00, 0.10],
            vec![0.01, 0.08, 1.00],
        ]
    }

    #[test]
    fn detects_ok_matrix_and_inverts() {
        let m = well_conditioned();
        match analyze_matrix(&m) {
            MatrixVerdict::Ok { inv, det, cond } => {
                assert!((det - 0.9845).abs() < 1e-3);
                assert!(cond < 10.0);
                let prod = mat_vec(&inv, &[1.0, 2.0, 3.0]);
                let back = mat_vec(&m, &prod);
                for (a, b) in back.iter().zip([1.0, 2.0, 3.0].iter()) {
                    assert!((a - b).abs() < 1e-9);
                }
            }
            other => panic!("expected ok, got {:?}", other),
        }
    }

    #[test]
    fn flags_near_singular_without_inverting() {
        let mut m = well_conditioned();
        m[2] = vec![1.05, 1.15, 0.120000001];
        match analyze_matrix(&m) {
            MatrixVerdict::NearSingular { cond, .. } => assert!(cond > NEAR_SINGULAR_COND),
            other => panic!("expected near singular, got {:?}", other),
        }
    }

    #[test]
    fn flags_exactly_singular() {
        let mut m = well_conditioned();
        m[2] = vec![1.05, 1.15, 0.12];
        assert!(matches!(analyze_matrix(&m), MatrixVerdict::Singular { .. }));
    }

    #[test]
    fn regularized_inverse_exists_for_singular() {
        let mut m = well_conditioned();
        m[2] = vec![1.05, 1.15, 0.12];
        let inv = regularized_inverse(&m, REG_LAMBDA).expect("regularized inverse");
        assert!(inv.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn classifies_boundary_points() {
        let poly = vec![[1.0, 1.0], [3.0, 1.0], [3.0, 3.0], [1.0, 3.0]];
        assert_eq!(classify_point(2.0, 2.0, &poly), PointClass::Inside);
        assert_eq!(classify_point(5.0, 2.0, &poly), PointClass::Outside);
        assert_eq!(classify_point(1.0, 2.0, &poly), PointClass::Boundary);
        assert_eq!(classify_point(2.0, 3.0, &poly), PointClass::Boundary);
    }

    #[test]
    fn asinh_roundtrip_scale() {
        let v = 150.0 * 1.5f64.sinh();
        assert!((asinh_transform(v, 150.0) - 1.5).abs() < 1e-12);
    }
}
