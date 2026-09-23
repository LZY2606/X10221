//! Derived computations. No silent normalization: derived layers are produced
//! explicitly and always reference exact input versions.

use crate::ids::hash_id;
use crate::lin;
use crate::model::*;
use serde_json::{json, Value};

/// reciprocal 2-norm condition estimate below which a spillover matrix is
/// declared near-singular and must not be silently inverted.
pub const NEAR_SINGULAR_RCOND_THRESHOLD: f64 = 1e-3;

pub fn rcond2(s: &[Vec<f64>]) -> f64 {
    let n = s.len();
    if n == 0 || s.iter().any(|r| r.len() != n) {
        return 0.0;
    }
    if lin::inverse(s).is_none() {
        return 0.0;
    }
    let st = lin::transpose(s);
    let sts = lin::mat_mul(&st, s).expect("square");
    let mut sv = lin::svd_singular_values_symmetric(&sts, 60);
    sv.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let max = sv.first().copied().unwrap_or(0.0);
    let min = sv.last().copied().unwrap_or(0.0);
    if max == 0.0 {
        0.0
    } else {
        min / max
    }
}

/// Build the compensation layer from a specific spillover version.
/// Near-singular matrices yield explicit ridge candidates only; the plain
/// inverse is suppressed so it can never be applied by default.
pub fn build_compensation(sp: &SpilloverV, spillover_layer_id: &str, threshold: f64) -> CompensationV {
    let s = &sp.matrix;
    let rcond = rcond2(s);
    let near = rcond < threshold;
    let mut diags = Vec::new();
    let mut cands = Vec::new();

    if !near {
        if let Some(inv) = lin::inverse(s) {
            let cid = hash_id("cand", &json!({"spillover":spillover_layer_id,"method":"exact_inverse"}));
            cands.push(CompCandidate {
                candidate_id: cid,
                method: "exact_inverse".into(),
                matrix: inv,
                ridge_lambda: None,
                invertible: true,
                explanation: "rcond above threshold; exact inverse is the unique legal compensation".into(),
            });
        }
    } else {
        diags.push(Diagnostic {
            code: "near_singular_spillover",
            event_index: None,
            channel: None,
            message: format!(
                "rcond={rcond:.3e} < threshold {threshold:.0e}; exact inverse suppressed, ridge candidates require explicit adjudication"
            ),
        });
        for lambda in [0.01_f64, 0.1] {
            if let Some(rc) = lin::ridge_inverse(s, lambda) {
                let cid = hash_id(
                    "cand",
                    &json!({"spillover":spillover_layer_id,"method":"ridge","lambda":lambda}),
                );
                cands.push(CompCandidate {
                    candidate_id: cid,
                    method: "ridge".into(),
                    matrix: rc,
                    ridge_lambda: Some(lambda),
                    invertible: true,
                    explanation: format!("Tikhonov ridge (S^T S + {lambda} I)^-1 S^T; legal but non-unique, shrinks spillover correction"),
                });
            }
        }
        if lin::inverse(s).is_none() {
            diags.push(Diagnostic {
                code: "singular_spillover",
                event_index: None,
                channel: None,
                message: "spillover matrix is exactly singular; no inverse exists".into(),
            });
        }
    }
    CompensationV {
        schema: "compensation/v1".into(),
        based_on_spillover_id: spillover_layer_id.to_string(),
        reciprocal_condition: rcond,
        near_singular: near,
        near_singular_threshold: threshold,
        candidates: cands,
        diagnostics: diags,
    }
}

/// asinh family transform. Inputs keep their declared unit; the output basis
/// is recorded rather than assumed by plots.
pub fn asinh_value(x: f64, cofactor: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let z = x / cofactor;
    (z + (1.0 + z * z).sqrt()).ln()
}

#[derive(Debug, Clone)]
pub struct PointResult {
    pub inside: bool,
    pub boundary: bool,
}

/// Ray-casting membership. Points within `eps` (transformed coordinates) of an
/// edge are boundary points: retained IN but explicitly flagged.
pub fn point_in_polygon(pt: [f64; 2], poly: &Polygon) -> PointResult {
    let (x, y) = (pt[0], pt[1]);
    let mut inside = false;
    let n = poly.points.len();
    for i in 0..n {
        let a = poly.points[i];
        let b = poly.points[(i + 1) % n];
        if ((a[1] > y) != (b[1] > y)) && {
            let xhit = a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            xhit < x
        } {
            inside = !inside;
        }
    }
    let mut boundary = false;
    if x.is_finite() && y.is_finite() {
        for i in 0..n {
            let a = poly.points[i];
            let b = poly.points[(i + 1) % n];
            if distance_to_segment(pt, a, b) <= poly.boundary_epsilon {
                boundary = true;
                break;
            }
        }
    }
    if boundary && poly.boundary_policy == "included_flagged" {
        PointResult { inside: true, boundary: true }
    } else {
        PointResult { inside, boundary }
    }
}

fn distance_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let vx = b[0] - a[0];
    let vy = b[1] - a[1];
    let wx = p[0] - a[0];
    let wy = p[1] - a[1];
    let len2 = vx * vx + vy * vy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        ((wx * vx + wy * vy) / len2).clamp(0.0, 1.0)
    };
    let dx = wx - t * vx;
    let dy = wy - t * vy;
    (dx * dx + dy * dy).sqrt()
}

/// Full derived pipeline bound to exact versions.
pub struct PipelineInputs<'a> {
    pub raw_layer_id: &'a str,
    pub raw: &'a RawChannelsV,
    pub spillover_layer_id: &'a str,
    pub sp: &'a SpilloverV,
    pub comp_layer_id: &'a str,
    pub comp: &'a CompCandidate,
    pub transform_layer_id: &'a str,
    pub tr: &'a TransformV,
    pub polygon_layer_id: &'a str,
    pub polys: &'a PolygonLibV,
    pub tree_layer_id: &'a str,
    pub tree: &'a GateTreeV,
}

#[derive(Debug, Clone)]
pub struct PopResult {
    pub node_id: String,
    pub name: String,
    pub parent: Option<String>,
    pub polygon_id: Option<String>,
    pub member_set_id: String,
    pub count: usize,
    pub pct: Option<f64>,
    pub denominator_node: Option<String>,
    pub denominator_member_set_id: Option<String>,
    pub boundary_count: usize,
    pub count_display_id: String,
    pub pct_display_id: Option<String>,
    pub dep_layers: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PipelineResult {
    pub derived_id: String,
    pub per_event: Vec<EventResult>,
    pub populations: Vec<PopResult>,
    pub comp_diff: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct EventResult {
    pub event_id: String,
    pub raw: Vec<f64>,
    pub compensated: Vec<f64>,
    pub transformed: Vec<f64>,
    pub memberships: Vec<String>,
    pub boundary_of: Vec<String>,
    pub flags: Vec<String>,
}

pub fn run_pipeline(inp: &PipelineInputs) -> PipelineResult {
    let raw = &inp.raw.events;
    let comp_mat = &inp.comp.matrix;
    let tr_by_ch: std::collections::HashMap<&str, &TransformParam> =
        inp.tr.transforms.iter().map(|t| (t.channel.as_str(), t)).collect();
    let poly_by_id: std::collections::HashMap<&str, &Polygon> =
        inp.polys.polygons.iter().map(|p| (p.polygon_id.as_str(), p)).collect();
    let node_by_id: std::collections::HashMap<&str, &GateNode> =
        inp.tree.nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();

    let dep_layers = |node: &GateNode| {
        let mut deps = vec![
            inp.raw_layer_id.to_string(),
            inp.spillover_layer_id.to_string(),
            inp.comp_layer_id.to_string(),
            inp.transform_layer_id.to_string(),
            inp.tree_layer_id.to_string(),
        ];
        if node.polygon_id.is_some() {
            deps.push(inp.polygon_layer_id.to_string());
        }
        deps
    };

    let mut per_event = Vec::with_capacity(raw.len());
    for (ei, row) in raw.iter().enumerate() {
        let mut flags: Vec<String> = Vec::new();
        // only the fluorochrome block participates in compensation
        let fluor_idx: Vec<usize> = inp
            .raw
            .channels
            .iter()
            .enumerate()
            .filter_map(|(i, c)| fluors.iter().position(|f| f == &c.name).map(|_| i))
            .collect();
        let mut compensated = row.clone();
        if fluor_idx.len() == comp_mat.len() {
            let xv: Vec<f64> = fluor_idx.iter().map(|&i| row[i]).collect();
            if xv.iter().all(|x| x.is_finite()) {
                let cv = lin::mat_vec(comp_mat, &xv).expect("dims");
                for (k, &i) in fluor_idx.iter().enumerate() {
                    compensated[i] = cv[k];
                }
            } else {
                flags.push("compensation_skipped_non_finite".into());
            }
        } else {
            flags.push("channel_block_mismatch".into());
        }
        let mut transformed = vec![0.0; row.len()];
        for (i, c) in inp.raw.channels.iter().enumerate() {
            match tr_by_ch.get(c.name.as_str()) {
                Some(t) if t.family == "asinh" => transformed[i] = asinh_value(compensated[i], t.cofactor),
                Some(t) if t.family == "linear_identity" => transformed[i] = compensated[i],
                _ => transformed[i] = compensated[i],
            }
        }
        per_event.push(EventResult {
            event_id: hash_id(
                "evt",
                &json!({"raw":inp.raw_layer_id,"event_index":ei}),
            ),
            raw: row.clone(),
            compensated,
            transformed,
            memberships: vec![inp.tree.root_name.clone()],
            boundary_of: vec![],
            flags,
        });
    }

    // Root: every raw event, including non-finite ones (never dropped).
    let root_indices: Vec<usize> = (0..raw.len()).collect();
    let root_set_id = hash_id(
        "members",
        &json!({"node":inp.tree.root_name,"raw":inp.raw_layer_id,"indices":root_indices}),
    );

    // evaluate nodes in parent order
    let mut order: Vec<&GateNode> = Vec::new();
    let mut placed = std::collections::HashSet::new();
    while order.len() < inp.tree.nodes.len() {
        let before = order.len();
        for node in inp.tree.nodes.iter() {
            if placed.contains(&node.node_id) {
                continue;
            }
            let ok = match &node.parent {
                None => true,
                Some(p) => p == &inp.tree.root_name || placed.contains(p),
            };
            if ok {
                order.push(node);
                placed.insert(&node.node_id);
            }
        }
        if order.len() == before {
            break; // malformed tree: stop
        }
    }

    let mut member_sets: std::collections::HashMap<String, (Vec<usize>, String)> =
        std::collections::HashMap::new();
    member_sets.insert(inp.tree.root_name.clone(), (root_indices.clone(), root_set_id.clone()));
    let mut boundary_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for node in &order {
        let parent_indices: &[usize] = match &node.parent {
            None | Some(p) if *p.as_ref().unwrap() == inp.tree.root_name => &root_indices,
            Some(p) => member_sets.get(p).map(|(v, _)| v.as_slice()).unwrap_or(&[]),
        };
        let mut mine: Vec<usize> = Vec::new();
        let mut boundary = 0usize;
        if let Some(pid) = &node.polygon_id {
            if let Some(poly) = poly_by_id.get(pid.as_str()) {
                let xi = inp
                    .raw
                    .channels
                    .iter()
                    .position(|c| c.name == poly.x_channel)
                    .expect("x channel");
                let yi = inp
                    .raw
                    .channels
                    .iter()
                    .position(|c| c.name == poly.y_channel)
                    .expect("y channel");
                for &ei in parent_indices {
                    let er = &per_event[ei];
                    if !er.transformed[xi].is_finite() || !er.transformed[yi].is_finite() {
                        continue; // cannot plot -> stays out, diagnostic already kept
                    }
                    let pr = point_in_polygon([er.transformed[xi], er.transformed[yi]], poly);
                    if pr.inside {
                        mine.push(ei);
                    }
                    if pr.boundary {
                        boundary += 1;
                        per_event[ei].boundary_of.push(poly.polygon_id.clone());
                    }
                }
            }
        } else {
            mine = parent_indices.to_vec();
        }
        let set_id = hash_id(
            "members",
            &json!({
                "node":node.node_id,
                "raw":inp.raw_layer_id,
                "comp":inp.comp_layer_id,
                "tr":inp.transform_layer_id,
                "poly_layer":inp.polygon_layer_id,
                "tree":inp.tree_layer_id,
                "indices":mine,
            }),
        );
        for &ei in &mine {
            per_event[ei].memberships.push(node.node_id.clone());
        }
        boundary_counts.insert(node.node_id.clone(), boundary);
        member_sets.insert(node.node_id.clone(), (mine, set_id));
    }

    let mut populations = Vec::new();
    // root population row
    let root_dep = vec![
        inp.raw_layer_id.to_string(),
        inp.spillover_layer_id.to_string(),
        inp.comp_layer_id.to_string(),
        inp.transform_layer_id.to_string(),
        inp.polygon_layer_id.to_string(),
        inp.tree_layer_id.to_string(),
    ];
    populations.push(PopResult {
        node_id: inp.tree.root_name.clone(),
        name: inp.tree.root_name.clone(),
        parent: None,
        polygon_id: None,
        member_set_id: root_set_id.clone(),
        count: root_indices.len(),
        pct: None,
        denominator_node: None,
        denominator_member_set_id: None,
        boundary_count: 0,
        count_display_id: hash_id(
            "disp",
            &json!({"kind":"count","pop":inp.tree.root_name,"members":root_set_id}),
        ),
        pct_display_id: None,
        dep_layers: root_dep,
    });

    for node in &order {
        let (members, set_id) = member_sets.get(&node.node_id).cloned().unwrap_or_default();
        let parent_name = node
            .parent
            .as_ref()
            .map(|p| node_by_id.get(p.as_str()).map(|n| n.name.clone()).unwrap_or_else(|| p.clone()));
        let (den_set, den_node) = match &node.parent {
            None => (None, None),
            Some(p) => (member_sets.get(p).map(|(_, id)| id.clone()), Some(p.clone())),
        };
        let pct = match (&den_set, &den_node) {
            (Some(did), Some(dn)) => {
                let dcount = member_sets.get(dn).map(|(v, _)| v.len()).unwrap_or(0);
                if dcount == 0 {
                    None
                } else {
                    Some(members.len() as f64 * 100.0 / dcount as f64)
                }
            }
            _ => None,
        };
        let pct_display_id = pct.as_ref().map(|_| {
            hash_id(
                "disp",
                &json!({"kind":"pct","pop":node.node_id,"members":set_id,"denominator":den_set}),
            )
        });
        populations.push(PopResult {
            node_id: node.node_id.clone(),
            name: node.name.clone(),
            parent: parent_name,
            polygon_id: node.polygon_id.clone(),
            member_set_id: set_id.clone(),
            count: members.len(),
            pct,
            denominator_node: den_node,
            denominator_member_set_id: den_set.clone(),
            boundary_count: boundary_counts.get(&node.node_id).copied().unwrap_or(0),
            count_display_id: hash_id(
                "disp",
                &json!({"kind":"count","pop":node.node_id,"members":set_id}),
            ),
            pct_display_id,
            dep_layers: dep_layers(node),
        });
    }

    // compensation before/after differences (mean of compensated channels)
    let mut comp_diff = Vec::new();
    for (ci, f) in inp.sp.fluorochromes.iter().enumerate() {
        let raw_mean = mean_finite(per_event.iter().map(|e| e.raw[channel_index(inp.raw, f)]));
        let comp_mean = mean_finite(per_event.iter().map(|e| e.compensated[channel_index(inp.raw, f)]));
        comp_diff.push(json!({
            "channel": f,
            "raw_mean_raw_units": raw_mean,
            "compensated_mean_raw_units": comp_mean,
            "delta": comp_mean.zip(raw_mean).map(|(c, r)| c - r),
            "unit": "raw detector units (mean, finite events)",
            "spillover_layer": inp.spillover_layer_id,
            "compensation_layer": inp.comp_layer_id,
            "candidate": inp.comp.candidate_id,
        }));
        let _ = ci;
    }

    let derived_id = hash_id(
        "derived",
        &json!({
            "raw":inp.raw_layer_id,"spill":inp.spillover_layer_id,
            "comp":inp.comp_layer_id,"tr":inp.transform_layer_id,
            "polys":inp.polygon_layer_id,"tree":inp.tree_layer_id,
        }),
    );

    PipelineResult { derived_id, per_event, populations, comp_diff }
}

fn channel_index(raw: &RawChannelsV, ch: &str) -> usize {
    raw.channels.iter().position(|c| c.name == ch).expect("channel")
}

fn mean_finite<I: Iterator<Item = f64>>(it: I) -> Option<f64> {
    let (mut n, mut s) = (0u64, 0.0);
    for x in it {
        if x.is_finite() {
            n += 1;
            s += x;
        }
    }
    if n == 0 { None } else { Some(s / n as f64) }
}
