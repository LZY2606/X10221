//! Spillover compensation: raw x = M * y  =>  y = M^-1 * x.
//!
//! Determinate matrices are inverted. Near-singular matrices are NOT silently
//! inverted: we produce explicit truncated-SVD candidates at two cutoffs and
//! leave the group indeterminate pending a human decision. Exactly singular
//! matrices produce no candidates at all.

use crate::linalg::{invert_strict, matvec, truncated_pinv, InvertStatus};
use serde_json::{json, Value};

pub const RCOND_THRESHOLD: f64 = 1e-3;

#[derive(Debug, Clone)]
pub struct EventVector {
    pub index: i64,
    pub values: Vec<Option<f64>>,
}

#[derive(Debug, Clone)]
pub struct CompResult {
    pub status: String,
    pub rcond: Option<f64>,
    pub rank: Option<usize>,
    pub singular_values: Vec<f64>,
    /// One candidate per legal inversion choice; empty when unsolvable.
    pub candidates: Vec<CompCandidate>,
    pub diagnostics: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct CompCandidate {
    pub method: String,
    pub cutoff_rel: Option<f64>,
    pub inv: Vec<Vec<f64>>,
    pub rank: Option<usize>,
}

pub fn evaluate_compensation(matrix: &[Vec<f64>], events: &[EventVector], channel_ranges: &[f64]) -> CompResult {
    let mut diagnostics = Vec::new();

    // Extreme signal diagnostics are recorded against the raw layer and do not
    // remove the event from compensation.
    for ev in events {
        for (c, maybe) in ev.values.iter().enumerate() {
            if let Some(v) = maybe {
                let r = channel_ranges.get(c).copied().unwrap_or(0.0);
                if v.abs() > r && r > 0.0 {
                    diagnostics.push(json!({
                        "code": "signal_out_of_documented_range",
                        "event_index": ev.index,
                        "channel_index": c,
                        "value": v,
                        "documented_range_max": r,
                        "unit": "channel_units",
                        "action": "retained_with_diagnostic"
                    }));
                }
                if v < &0.0 {
                    diagnostics.push(json!({
                        "code": "negative_raw_signal",
                        "event_index": ev.index,
                        "channel_index": c,
                        "value": v,
                        "unit": "channel_units"
                    }));
                }
            }
        }
    }

    match invert_strict(matrix, RCOND_THRESHOLD) {
        Ok(r) => CompResult {
            status: "determinate".into(),
            rcond: Some(r.rcond),
            rank: Some(matrix.len()),
            singular_values: vec![],
            candidates: vec![CompCandidate {
                method: "lu_partial_pivot".into(),
                cutoff_rel: None,
                inv: r.inv,
                rank: Some(matrix.len()),
            }],
            diagnostics,
        },
        Err(status) => match status {
            InvertStatus::Singular { rcond } => CompResult {
                status: "singular".into(),
                rcond: Some(rcond),
                rank: None,
                singular_values: vec![],
                candidates: vec![],
                diagnostics,
            },
            InvertStatus::NearSingular { rcond } => {
                let (inv_a, rank_a, s_a) = truncated_pinv(matrix, 1e-6);
                let (inv_b, rank_b, s_b) = truncated_pinv(matrix, 1e-3);
                let singular_values = {
                    let svd = crate::linalg::svd(matrix);
                    svd.s
                };
                let mut cands = vec![
                    CompCandidate { method: "tsvd_rel_1e-6".into(), cutoff_rel: Some(1e-6), inv: inv_a, rank: Some(rank_a) },
                ];
                if rank_b != rank_a || &s_b != &s_a {
                    cands.push(CompCandidate { method: "tsvd_rel_1e-3".into(), cutoff_rel: Some(1e-3), inv: inv_b, rank: Some(rank_b) });
                }
                CompResult {
                    status: "indeterminate".into(),
                    rcond: Some(rcond),
                    rank: None,
                    singular_values,
                    candidates: cands,
                    diagnostics,
                }
            }
            InvertStatus::Determinate => unreachable!(),
        },
    }
}

pub fn apply_candidate(cand: &CompCandidate, events: &[EventVector]) -> Vec<Value> {
    events
        .iter()
        .map(|ev| {
            let finite: Vec<Option<f64>> = ev.values.clone();
            let all_finite = finite.iter().all(|x| x.is_some());
            let comp: Vec<Option<f64>> = if all_finite {
                let x: Vec<f64> = finite.iter().map(|x| x.unwrap()).collect();
                matvec(&cand.inv, &x).into_iter().map(Some).collect()
            } else {
                // Events with a non-finite/non-convertible channel cannot be
                // compensated uniquely: keep nulls in place, skip that event.
                finite
            };
            json!({
                "event_index": ev.index,
                "compensated": comp.iter().map(|v| match v {
                    Some(x) => json!({"value": x, "unit": "channel_units", "coordinate_basis": "compensated_linear"}),
                    None => json!({"value": Value::Null, "unit": "channel_units", "coordinate_basis": "compensated_linear", "note": "uncomputable_non_finite_input"}),
                }).collect::<Vec<_>>()
            })
        })
        .collect()
}
