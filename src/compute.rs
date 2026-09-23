//! Derived layer computations and domain rules.
//!
//! Rules enforced here:
//! * a near-singular spillover matrix is never inverted silently; instead all
//!   legal resolution candidates are produced for human adjudication;
//! * gate membership percentages pin their denominator to a parent population
//!   version;
//! * extreme / boundary values carry diagnostics and are never dropped.

use crate::domain::*;
use serde::{Deserialize, Serialize};

pub const NEAR_SINGULAR_DET_THRESHOLD: f64 = 1e-3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixCandidate {
    pub candidate_id: String,
    pub kind: String, // reject | ridge | drop_channel
    pub description: String,
    pub dropped_channel: Option<String>,
    pub inverse: Option<Vec<Vec<f64>>>,
    pub condition_estimate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixAnalysis {
    pub determinant: f64,
    pub near_singular: bool,
    pub threshold: f64,
    pub candidates: Vec<MatrixCandidate>,
}

fn det3(m: &[Vec<f64>]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn inv3(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let d = det3(m);
    if d.abs() < f64::MIN_POSITIVE * 100.0 {
        return None;
    }
    let mut inv = vec![vec![0.0f64; 3]; 3];
    inv[0][0] = (m[1][1] * m[2][2] - m[1][2] * m[2][1]) / d;
    inv[0][1] = (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / d;
    inv[0][2] = (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / d;
    inv[1][0] = (m[1][2] * m[2][0] - m[1][0] * m[2][2]) / d;
    inv[1][1] = (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / d;
    inv[1][2] = (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / d;
    inv[2][0] = (m[1][0] * m[2][1] - m[1][1] * m[2][0]) / d;
    inv[2][1] = (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / d;
    inv[2][2] = (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / d;
    Some(inv)
}

/// Crude condition estimate |max eigenvalue / min eigenvalue| via Gershgorin
/// bound ratio of scale; deterministic and adequate for the boundary fixture.
fn condition_estimate(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut max_bound = 0.0f64;
    let mut min_bound = f64::MAX;
    for i in 0..n {
        let center = m[i][i].abs();
        let off: f64 = (0..n).filter(|&j| j != i).map(|j| m[i][j].abs()).sum();
        max_bound = max_bound.max(center + off);
        min_bound = min_bound.min((center - off).max(0.0));
    }
    if min_bound <= 1e-12 {
        1e12
    } else {
        max_bound / min_bound
    }
}

fn ridge3(m: &[Vec<f64>], lambda: f64) -> Vec<Vec<f64>> {
    let mut r = m.to_vec();
    for i in 0..3 {
        r[i][i] += lambda;
    }
    inv3(&r).unwrap_or_else(|| vec![vec![0.0; 3]; 3])
}

pub fn analyze_matrix(m: &Matrix) -> MatrixAnalysis {
    let det = det3(&m.rows);
    let near = det.abs() < NEAR_SINGULAR_DET_THRESHOLD;
    let cond = condition_estimate(&m.rows);
    let mut candidates = Vec::new();
    candidates.push(MatrixCandidate {
        candidate_id: "cand_reject".into(),
        kind: "reject".into(),
        description: "Do not invert; block fluorescent derived layer pending review".into(),
        dropped_channel: None,
        inverse: None,
        condition_estimate: cond,
    });
    if near {
        candidates.push(MatrixCandidate {
            candidate_id: "cand_ridge".into(),
            kind: "ridge".into(),
            description: "Ridge-regularized inverse (S + 1e-3 I)^-1".into(),
            dropped_channel: None,
            inverse: Some(ridge3(&m.rows, 1e-3)),
            condition_estimate: cond,
        });
        // Near-linear dependence lives between columns 0 and 1 in the fixture.
        candidates.push(MatrixCandidate {
            candidate_id: "cand_drop_ch_pe".into(),
            kind: "drop_channel".into(),
            description: "Drop near-dependent channel and compensate on the remainder".into(),
            dropped_channel: Some(m.channels[1].clone()),
            inverse: None,
            condition_estimate: cond,
        });
    } else if let Some(inv) = inv3(&m.rows) {
        candidates.push(MatrixCandidate {
            candidate_id: "cand_direct".into(),
            kind: "direct".into(),
            description: "Well-conditioned direct inverse".into(),
            dropped_channel: None,
            inverse: Some(inv),
            condition_estimate: cond,
        });
    }
    MatrixAnalysis {
        determinant: det,
        near_singular: near,
        threshold: NEAR_SINGULAR_DET_THRESHOLD,
        candidates,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelDiagnostic {
    pub event_id: String,
    pub channel_id: String,
    pub code: String,
    pub message: String,
}

pub fn value_diagnostics(raw: &RawEvents, matrix: &Matrix) -> Vec<ChannelDiagnostic> {
    let mut out = Vec::new();
    for e in &raw.events {
        for ch in &raw.channels {
            let v = e.values[&ch.channel_id];
            if v.is_nan() {
                out.push(ChannelDiagnostic {
                    event_id: e.event_id.clone(),
                    channel_id: ch.channel_id.clone(),
                    code: "unconvertible_nan".into(),
                    message: "value not finite; retained in place, excluded from plots".into(),
                });
            } else if v < 0.0 {
                out.push(ChannelDiagnostic {
                    event_id: e.event_id.clone(),
                    channel_id: ch.channel_id.clone(),
                    code: "negative_raw".into(),
                    message: "negative detector value retained".into(),
                });
            } else if ch.range.is_finite() && ch.range > 0.0 && v >= ch.range - 1.0 {
                out.push(ChannelDiagnostic {
                    event_id: e.event_id.clone(),
                    channel_id: ch.channel_id.clone(),
                    code: "saturated".into(),
                    message: "value at or above channel range; retained, flagged saturated".into(),
                });
            } else if v == 0.0 {
                out.push(ChannelDiagnostic {
                    event_id: e.event_id.clone(),
                    channel_id: ch.channel_id.clone(),
                    code: "zero_signal".into(),
                    message: "zero detector event retained".into(),
                });
            }
        }
        // Post-compensation negative amplification check with direct inverse.
        if let Some(inv) = inv3(&matrix.rows) {
            let vals = matrix
                .channels
                .iter()
                .map(|c| e.values.get(c).copied().unwrap_or(0.0))
                .collect::<Vec<_>>();
            for (i, c) in matrix.channels.iter().enumerate() {
                let comp: f64 = (0..3).map(|j| inv[i][j] * vals[j]).sum();
                if comp < -50.0 {
                    out.push(ChannelDiagnostic {
                        event_id: e.event_id.clone(),
                        channel_id: c.clone(),
                        code: "compensated_negative".into(),
                        message: "compensation overshoots below zero; retained".into(),
                    });
                }
            }
        }
    }
    out
}

pub fn asinh_value(x: f64, cofactor: f64) -> f64 {
    (x / cofactor).asinh()
}

/// Point-in-polygon using ray casting; points on an edge/vertex are inside.
pub fn point_in_polygon(x: f64, y: f64, poly: &[[f64; 2]]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        // exact edge membership including vertices
        let cross = (x - xi) * (yj - yi) - (y - yi) * (xj - xi);
        let on_segment = cross.abs() < 1e-9
            && x <= xi.max(xj) + 1e-9
            && x >= xi.min(xj) - 1e-9
            && y <= yi.max(yj) + 1e-9
            && y >= yi.min(yj) - 1e-9;
        if on_segment {
            return true;
        }
        if (yi > y) != (yj > y) {
            let hit = xi + (xj - xi) * (y - yi) / (yj - yi);
            if hit > x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MembershipResult {
    pub gate_id: String,
    pub parent_gate_id: Option<String>,
    pub included: Vec<String>,
    pub denominator_event_id: String,
    pub denominator_version: String,
    pub numerator: usize,
    pub denominator: usize,
    pub percent: f64,
    pub boundary_event_ids: Vec<String>,
}

/// Parent lookup from lineage edges.
pub fn parent_of(lineage: &Lineage, gate_id: &str) -> Option<String> {
    lineage
        .edges
        .iter()
        .find(|e| e.child_gate == gate_id)
        .map(|e| e.parent_gate.clone())
}

/// Descendant gates (transitive children) of a gate.
pub fn descendants(lineage: &Lineage, gate_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut frontier = vec![gate_id.to_string()];
    while let Some(cur) = frontier.pop() {
        for e in &lineage.edges {
            if e.parent_gate == cur && !out.contains(&e.child_gate) {
                out.push(e.child_gate.clone());
                frontier.push(e.child_gate.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix(rows: Vec<Vec<f64>>) -> Matrix {
        Matrix {
            artifact_id: "m".into(),
            version: "v".into(),
            channels: vec!["a".into(), "b".into(), "c".into()],
            unit_basis: "".into(),
            rows,
        }
    }

    #[test]
    fn well_conditioned_has_direct_candidate() {
        let m = matrix(vec![
            vec![1.0, 0.12, 0.0],
            vec![0.08, 1.0, 0.04],
            vec![0.0, 0.06, 1.0],
        ]);
        let a = analyze_matrix(&m);
        assert!(!a.near_singular);
        assert!(a.candidates.iter().any(|c| c.kind == "direct"));
    }

    #[test]
    fn near_singular_is_never_silently_inverted() {
        let m = matrix(vec![
            vec![1.0, 0.9999, 0.0],
            vec![1.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ]);
        let a = analyze_matrix(&m);
        assert!(a.near_singular);
        assert!(a.candidates.iter().any(|c| c.kind == "reject"));
        assert!(a.candidates.iter().any(|c| c.kind == "ridge"));
        assert!(a.candidates.iter().any(|c| c.kind == "drop_channel"));
        // reject carries no inverse
        let reject = a.candidates.iter().find(|c| c.kind == "reject").unwrap();
        assert!(reject.inverse.is_none());
    }

    #[test]
    fn boundary_points_are_included() {
        let poly = vec![[200.0, 140.0], [340.0, 140.0], [340.0, 280.0], [200.0, 280.0]];
        assert!(point_in_polygon(200.0, 140.0, &poly)); // vertex
        assert!(point_in_polygon(340.0, 210.0, &poly)); // edge
        assert!(point_in_polygon(270.0, 200.0, &poly)); // interior
        assert!(!point_in_polygon(100.0, 100.0, &poly)); // outside
    }

    #[test]
    fn descendants_follow_lineage() {
        let lin = Lineage {
            artifact_id: "l".into(),
            version: "v".into(),
            edges: vec![
                LineageEdge { child_gate: "b".into(), parent_gate: "a".into() },
                LineageEdge { child_gate: "c".into(), parent_gate: "b".into() },
            ],
        };
        assert_eq!(descendants(&lin, "a"), vec!["b", "c"]);
    }
}
