//! Transforms and polygon membership with explicit boundary handling.

use crate::types::{BoundaryCandidate, BoundaryCandidates, Point, TransformFamily};
use std::f64::consts::LN_10;

pub fn apply(family: &TransformFamily, value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    match family {
        TransformFamily::Linear { scale } => value * scale,
        TransformFamily::Asinh { cofactor } => (value / cofactor).asinh() / LN_10,
    }
}

pub fn distance_to_segment(p: &Point, a: &Point, b: &Point) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length2 = dx * dx + dy * dy;
    let t = if length2 == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / length2).clamp(0.0, 1.0)
    };
    let qx = a.x + t * dx;
    let qy = a.y + t * dy;
    ((p.x - qx).powi(2) + (p.y - qy).powi(2)).sqrt()
}

const EDGE_TOLERANCE: f64 = 1e-7;

pub fn on_polygon_boundary(p: &Point, polygon: &[Point]) -> bool {
    if polygon.len() < 2 {
        return false;
    }
    for w in polygon.windows(2) {
        if distance_to_segment(p, &w[0], &w[1]) <= EDGE_TOLERANCE {
            return true;
        }
    }
    let last = polygon.len() - 1;
    distance_to_segment(p, &polygon[last], &polygon[0]) <= EDGE_TOLERANCE
}

/// Ray-casting interior test ignoring boundary points (callers resolve
/// boundary membership separately via adjudication).
pub fn strictly_inside(p: &Point, polygon: &[Point]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (pi, pj) = (&polygon[i], &polygon[j]);
        if (pi.y > p.y) != (pj.y > p.y) {
            let x_cross = pi.x + (p.y - pi.y) * (pj.x - pi.x) / (pj.y - pi.y);
            if x_cross > p.x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

pub fn evaluate_boundary(
    event_indices: Vec<u32>,
    boundary_points: Vec<Point>,
    polygon: &[Point],
) -> BoundaryCandidates {
    let inside_inclusive: Vec<u32> = event_indices.clone();
    // Tie-break convention: edges whose interior lies above/right count the
    // point in ("bottom/left edges inclusive"), others out.
    let inside_edge_rule: Vec<u32> = event_indices
        .iter()
        .zip(boundary_points.iter())
        .filter(|(_, p)| bottom_left_edge_inside(p, polygon))
        .map(|(idx, _)| *idx)
        .collect();
    let uniquely_determined = event_indices.is_empty();
    BoundaryCandidates {
        event_indices,
        candidates: vec![
            BoundaryCandidate {
                convention: "EDGE_INCLUSIVE".to_string(),
                label: "boundary points are inside".to_string(),
                inside_event_indices: inside_inclusive,
                rationale: "closed polygon: on-edge events belong to the population".to_string(),
            },
            BoundaryCandidate {
                convention: "EDGE_EXCLUSIVE".to_string(),
                label: "boundary points are outside".to_string(),
                inside_event_indices: vec![],
                rationale: "half-open polygon: on-edge events go to the sibling/parent".to_string(),
            },
            BoundaryCandidate {
                convention: "BOTTOM_LEFT_INCLUSIVE".to_string(),
                label: "lower/left edges inside, upper/right outside".to_string(),
                inside_event_indices: inside_edge_rule,
                rationale: "tie-breaking by edge orientation, matching common scanline rules"
                    .to_string(),
            },
        ],
        uniquely_determined,
    }
}

fn bottom_left_edge_inside(p: &Point, polygon: &[Point]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let mut edges: Vec<(&Point, &Point)> = Vec::new();
    for w in polygon.windows(2) {
        edges.push((&w[0], &w[1]));
    }
    edges.push((&polygon[n - 1], &polygon[0]));
    for (a, b) in edges {
        if distance_to_segment(p, a, b) > EDGE_TOLERANCE {
            continue;
        }
        let mx = (a.x + b.x) / 2.0;
        let my = (a.y + b.y) / 2.0;
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len = (dx * dx + dy * dy).sqrt().max(1e-300);
        // Both unit normals; pick the one pointing into the polygon.
        let nx = -dy / len;
        let ny = dx / len;
        let eps = 1e-6;
        let inward = if strictly_inside(
            &Point { x: mx + nx * eps, y: my + ny * eps },
            polygon,
        ) {
            (nx, ny)
        } else if strictly_inside(
            &Point { x: mx - nx * eps, y: my - ny * eps },
            polygon,
        ) {
            (-nx, -ny)
        } else {
            return false;
        };
        // Bottom edge: interior points up (+y). Left edge: interior points right (+x).
        return inward.1 > 0.5 || inward.0 > 0.5;
    }
    false
}
