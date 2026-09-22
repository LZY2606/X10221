//! Domain model: separately versioned sources, transforms, gates and populations.
//!
//! Every derived quantity records both its numeric result and the exact
//! versions of the inputs used to produce it.

use serde::{Deserialize, Serialize};
use crate::jsonnum::Jf64;

pub const CASE_ID: &str = "flow-boundary-chamber";
pub const MAIN_BRANCH: &str = "B:main";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnitSpec {
    pub scale: String,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelMeta {
    pub index: usize,
    pub name: String,
    pub detector: String,
    pub fluorophore: Option<String>,
    pub unit: UnitSpec,
    pub range_min: f64,
    pub range_max: f64,
}

/// Versioned resource 1/6: raw channel values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSetV {
    pub source_file: String,
    pub sha256: String,
    pub media_type: String,
    pub channels: Vec<ChannelMeta>,
    /// Rows in acquisition order; NaN/Inf are preserved as explicit JSON tokens.
    pub values: Vec<Vec<Jf64>>,
    pub diagnostics: Vec<String>,
}

/// Versioned resource 2/6: spillover metadata (instrument-reported, raw).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverMetaV {
    pub keyword: String,
    pub detectors: Vec<String>,
    pub fluorophores: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub unit: UnitSpec,
    pub raw_representation: String,
}

/// Versioned resource 3/6: compensation (unmixing) matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompMatrixV {
    pub detectors: Vec<String>,
    pub unmixing: Vec<Vec<f64>>,
    pub derived_from_spillover_id: String,
    pub derived_from_spillover_version: i64,
    pub derivation: String,
    pub near_singular_threshold: f64,
    pub condition_number_1norm: Option<f64>,
    pub unit: UnitSpec,
}

/// Versioned resource 4/6: per-channel transform parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformV {
    pub channel: String,
    pub method: String,
    pub cofactor: f64,
    pub offset: f64,
    pub input_unit: String,
    pub output_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Versioned resource 5/6: gate geometry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateV {
    pub name: String,
    pub gate_kind: String,
    pub x_channel: String,
    pub y_channel: Option<String>,
    pub transform_id: String,
    pub transform_version: i64,
    pub polygon: Option<Vec<Point>>,
    pub rectangle: Option<(Point, Point)>,
    pub coordinates_basis: String,
    /// Boundary semantics are explicit so reviewers can audit edge events.
    pub boundary_inclusive: bool,
    pub notes: Option<String>,
}

/// Versioned resource 6/6: parent/child relationships (the gate tree).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateNode {
    pub gate_id: String,
    pub parent_gate_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageV {
    pub nodes: Vec<GateNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PopulationResult {
    pub status: String,
    pub gate_id: String,
    pub gate_version: i64,
    pub parent_result_id: Option<String>,
    pub parent_result_version: Option<i64>,
    pub transform_id: String,
    pub transform_version: i64,
    pub comp_candidate_id: Option<String>,
    pub denominator_scope: String,
    pub denominator_event_count: Option<usize>,
    pub numerator_event_count: Option<usize>,
    pub percent: Option<f64>,
    pub member_event_indexes: Option<Vec<usize>>,
    pub diagnostics: Vec<String>,
}

pub fn asinh_transform(v: f64, cofactor: f64, offset: f64) -> f64 {
    if !v.is_finite() {
        return v;
    }
    ((v - offset) / cofactor).asinh()
}

pub fn apply_transforms(
    rows: &[Vec<f64>],
    channels: &[ChannelMeta],
    transforms: &[TransformV],
) -> Vec<Vec<f64>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(idx, v)| {
                    let name = &channels[idx].name;
                    match transforms.iter().find(|t| &t.channel == name) {
                        Some(t) if t.method == "asinh" => asinh_transform(*v, t.cofactor, t.offset),
                        _ => *v,
                    }
                })
                .collect()
        })
        .collect()
}

/// Ray-casting polygon membership. Boundary points are inclusive when requested,
/// tested with an explicit epsilon so edge events are auditable.
pub fn point_in_polygon(p: &Point, poly: &[Point], inclusive: bool) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    if inclusive {
        const EPS: f64 = 1e-9;
        for i in 0..n {
            let a = &poly[i];
            let b = &poly[(i + 1) % n];
            let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
            let dot = (p.x - a.x) * (p.x - b.x) + (p.y - a.y) * (p.y - b.y);
            let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
            if cross.abs() <= EPS * len.max(1.0) && dot <= EPS * len.max(1.0) {
                return true;
            }
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i].x, poly[i].y);
        let (xj, yj) = (poly[j].x, poly[j].y);
        let intersect = ((yi > p.y) != (yj > p.y))
            && (p.x < (xj - xi) * (p.y - yi) / (yj - yi) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn point_in_rectangle(p: &Point, a: &Point, b: &Point, inclusive: bool) -> bool {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    if inclusive {
        p.x >= x0 && p.x <= x1 && p.y >= y0 && p.y <= y1
    } else {
        p.x > x0 && p.x < x1 && p.y > y0 && p.y < y1
    }
}

pub fn gate_contains(gate: &GateV, x: f64, y: Option<f64>) -> bool {
    if !x.is_finite() || y.map(|v| !v.is_finite()).unwrap_or(false) {
        return false;
    }
    match gate.gate_kind.as_str() {
        "rectangle" => match (&gate.rectangle, y) {
            (Some((a, b)), Some(yv)) => point_in_rectangle(&Point { x, y: yv }, a, b, gate.boundary_inclusive),
            _ => false,
        },
        "polygon" => match (&gate.polygon, y) {
            (Some(poly), Some(yv)) => point_in_polygon(&Point { x, y: yv }, poly, gate.boundary_inclusive),
            _ => false,
        },
        "range" => match &gate.rectangle {
            Some((a, b)) => {
                let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
                if gate.boundary_inclusive { x >= x0 && x <= x1 } else { x > x0 && x < x1 }
            }
            None => false,
        },
        _ => false,
    }
}

pub fn channel_index(channels: &[ChannelMeta], name: &str) -> Option<usize> {
    channels.iter().position(|c| c.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(poly: Vec<Point>) -> GateV {
        GateV {
            name: "G".into(), gate_kind: "polygon".into(),
            x_channel: "FITC".into(), y_channel: Some("PE".into()),
            transform_id: "T".into(), transform_version: 1,
            polygon: Some(poly), rectangle: None,
            coordinates_basis: "asinh".into(), boundary_inclusive: true, notes: None,
        }
    }

    #[test]
    fn boundary_points_are_explicitly_included() {
        let g = gate(vec![Point { x: 0.0, y: 0.0 }, Point { x: 2.0, y: 0.0 }, Point { x: 2.0, y: 2.0 }]);
        assert!(gate_contains(&g, 1.0, Some(0.0))); // on edge
        assert!(gate_contains(&g, 1.0, Some(1.0)));
        assert!(!gate_contains(&g, 3.0, Some(1.0)));
    }

    #[test]
    fn nan_events_never_enter_gates() {
        let g = gate(vec![Point { x: 0.0, y: 0.0 }, Point { x: 2.0, y: 0.0 }, Point { x: 2.0, y: 2.0 }]);
        assert!(!gate_contains(&g, f64::NAN, Some(1.0)));
        assert!(!gate_contains(&g, 1.0, Some(f64::NAN)));
    }

    #[test]
    fn asinh_preserves_nan() {
        assert!(asinh_transform(f64::NAN, 150.0, 0.0).is_nan());
        assert!(asinh_transform(f64::INFINITY, 150.0, 0.0).is_infinite());
    }
}
