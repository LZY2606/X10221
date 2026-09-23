//! Pure flow-cytometry calculations: compensation, transformation,
//! polygon gating and population statistics.
//!
//! No persistence here — every function is deterministic so unit tests and
//! branch replay share exactly the same arithmetic.

use crate::matrix::{invert_spillover, Candidate, CompCandidates, Mat, Singular};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum TransformBasis {
    /// `asinh(value / cofactor)`, input in the channel's native unit.
    Asinh,
    /// Identity transform (already linear scale).
    Linear,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TransformSpec {
    pub basis: TransformBasis,
    pub cofactor: f64,
    pub input_unit: String,
}

impl TransformSpec {
    pub fn label(&self) -> String {
        match self.basis {
            TransformBasis::Asinh => format!("asinh(v/{} [{}])", self.cofactor, self.input_unit),
            TransformBasis::Linear => format!("linear [{}]", self.input_unit),
        }
    }

    pub fn apply(&self, v: f64) -> f64 {
        match self.basis {
            TransformBasis::Asinh => (v / self.cofactor).asinh(),
            TransformBasis::Linear => v,
        }
    }

    pub fn invert(&self, y: f64) -> f64 {
        match self.basis {
            TransformBasis::Asinh => self.cofactor * y.sinh(),
            TransformBasis::Linear => y,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum Membership {
    Inside,
    Outside,
    /// Exactly on an edge: either in/out is a legal interpretation.
    Boundary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Winding-number ray test plus exact-edge detection in transformed space.
pub fn polygon_membership(p: &Point, poly: &[Point]) -> Membership {
    let n = poly.len();
    if n < 3 {
        return Membership::Outside;
    }
    for i in 0..n {
        let a = &poly[i];
        let b = &poly[(i + 1) % n];
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        let dot = (p.x - a.x) * (p.x - b.x) + (p.y - a.y) * (p.y - b.y);
        if cross.abs() < 1e-9 && dot <= 1e-9 {
            return Membership::Boundary;
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i].x, poly[i].y);
        let (xj, yj) = (poly[j].x, poly[j].y);
        if (yi > p.y) != (yj > p.y)
            && p.x < (xj - xi) * (p.y - yi) / (yj - yi) + xi
        {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        Membership::Inside
    } else {
        Membership::Outside
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateSpec {
    /// Coordinate basis of the vertices, e.g.
    /// `asinh(FITC-A,cofactor=150) vs asinh(PE-A,cofactor=150)`.
    pub coordinate_basis: String,
    pub x_channel: String,
    pub y_channel: String,
    pub transform: TransformSpec,
    pub vertices: Vec<Point>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GateEval {
    pub inside: Vec<u32>,
    pub outside: Vec<u32>,
    pub boundary: Vec<u32>,
    pub non_finite: Vec<NonFinite>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NonFinite {
    pub event_index: u32,
    pub channel: String,
    pub code: String,
}

/// Evaluate a gate against parent events already expressed in compensated
/// native units. `rows[event][channel_index]` is `None` when the source value
/// could not be converted (NaN/Inf kept at source, not plotted).
pub fn evaluate_gate(
    rows: &[Vec<Option<f64>>],
    x_idx: usize,
    y_idx: usize,
    gate: &GateSpec,
) -> GateEval {
    let mut ev = GateEval {
        inside: Vec::new(),
        outside: Vec::new(),
        boundary: Vec::new(),
        non_finite: Vec::new(),
    };
    for (i, row) in rows.iter().enumerate() {
        let (xv, yv) = match (row.get(x_idx).copied().flatten(), row.get(y_idx).copied().flatten()) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                if row.get(x_idx).copied().flatten().is_none() {
                    ev.non_finite.push(NonFinite {
                        event_index: i as u32,
                        channel: gate.x_channel.clone(),
                        code: "gate-coordinate-nonfinite",
                    });
                }
                if row.get(y_idx).copied().flatten().is_none() {
                    ev.non_finite.push(NonFinite {
                        event_index: i as u32,
                        channel: gate.y_channel.clone(),
                        code: "gate-coordinate-nonfinite",
                    });
                }
                ev.outside.push(i as u32);
                continue;
            }
        };
        let p = Point {
            x: gate.transform.apply(xv),
            y: gate.transform.apply(yv),
        };
        match polygon_membership(&p, &gate.vertices) {
            Membership::Inside => ev.inside.push(i as u32),
            Membership::Outside => ev.outside.push(i as u32),
            Membership::Boundary => ev.boundary.push(i as u32),
        }
    }
    ev
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompResult {
    /// Compensated rows; channels carrying NaN/Inf stay `None` and keep their
    /// source diagnostic.
    pub rows: Vec<Vec<Option<f64>>>,
    pub diagnostics: Vec<CompDiagnostic>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CompDiagnostic {
    pub event_index: u32,
    pub output_channel: String,
    pub code: String,
}

/// Apply a compensation matrix (already-inverted spillover) to raw rows.
/// A non-finite input contaminates every output channel it feeds, so the whole
/// output row is marked un-evaluable rather than clipped for plotting.
pub fn compensate(inverse: &Mat, raw: &[Vec<Option<f64>>]) -> CompResult {
    let mut diagnostics = Vec::new();
    let rows = raw
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let contaminated: Vec<usize> = row
                .iter()
                .enumerate()
                .filter_map(|(c, v)| if v.is_none() { Some(c) } else { None })
                .collect();
            if contaminated.is_empty() {
                let nums: Vec<f64> = row.iter().map(|v| v.unwrap()).collect();
                inverse
                    .mul_vec(&nums)
                    .into_iter()
                    .map(Some)
                    .collect()
            } else {
                // Output channels with any coefficient on a bad input channel
                // are themselves non-finite; unaffected outputs still compute.
                let nums: Vec<f64> = row.iter().map(|v| v.unwrap_or(0.0)).collect();
                let computed = inverse.mul_vec(&nums);
                computed
                    .iter()
                    .enumerate()
                    .map(|(out_c, val)| {
                        let feeds = contaminated
                            .iter()
                            .any(|&bad| inverse.at(out_c, bad).abs() > 0.0);
                        if feeds {
                            diagnostics.push(CompDiagnostic {
                                event_index: i as u32,
                                output_channel: out_c.to_string(),
                                code: "comp-contaminated-by-nonfinite-input",
                            });
                            None
                        } else {
                            Some(*val)
                        }
                    })
                    .collect()
            }
        })
        .collect();
    CompResult { rows, diagnostics }
}

/// Result of asking for a spillover inverse. Exact singularity is an error;
/// near-singularity yields candidates that a human must adjudicate.
pub enum SpilloverInverse {
    Invertible(Mat),
    NearSingular {
        rcond: f64,
        min_rel_pivot: f64,
        candidates: Vec<Candidate>,
    },
    Singular(Singular),
}

pub fn spillover_inverse(spill: &Mat) -> SpilloverInverse {
    match invert_spillover(spill) {
        Ok((Some(inv), _diag)) => SpilloverInverse::Invertible(inv),
        Ok((None, CompCandidates { rcond, min_rel_pivot, candidates })) => {
            SpilloverInverse::NearSingular { rcond, min_rel_pivot, candidates }
        }
        Err(e) => SpilloverInverse::Singular(e),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PercentageCandidate {
    pub id: String,
    pub interpretation: String,
    pub count: u32,
    pub denominator: u32,
    pub percent: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PopulationStat {
    /// Pinned parent result row id — the denominator version is fixed here and
    /// must never silently follow the parent's latest recomputation.
    pub parent_result_id: String,
    pub parent_label: String,
    pub candidates: Vec<PercentageCandidate>,
    pub boundary_events: Vec<u32>,
    pub non_finite_events: Vec<u32>,
    pub ambiguous: bool,
}

/// Build percentage candidates for a gate evaluation.
/// When boundary points exist there are exactly two legal counts
/// (boundary excluded / boundary included); the caller must record both.
pub fn population_stat(
    parent_result_id: &str,
    parent_label: &str,
    denominator: u32,
    eval: &GateEval,
) -> PopulationStat {
    let inside = eval.inside.len() as u32;
    let boundary = eval.boundary.len() as u32;
    let pct = |c: u32| -> f64 {
        if denominator == 0 {
            0.0
        } else {
            (c as f64) * 100.0 / denominator as f64
        }
    };
    let mut candidates = vec![PercentageCandidate {
        id: "boundary-excluded".to_string(),
        interpretation: "edge points counted outside".to_string(),
        count: inside,
        denominator,
        percent: pct(inside),
    }];
    if boundary > 0 {
        candidates.push(PercentageCandidate {
            id: "boundary-included".to_string(),
            interpretation: "edge points counted inside".to_string(),
            count: inside + boundary,
            denominator,
            percent: pct(inside + boundary),
        });
    }
    PopulationStat {
        parent_result_id: parent_result_id.to_string(),
        parent_label: parent_label.to_string(),
        candidates,
        boundary_events: eval.boundary.clone(),
        non_finite_events: eval.non_finite.iter().map(|d| d.event_index).collect(),
        ambiguous: boundary > 0,
    }
}
