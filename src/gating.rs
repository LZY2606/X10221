//! Point membership in gate polygons (with explicit boundary ambiguity) and
//! the per-channel transforms used to draw gates.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Transform {
    Linear { gain: f64 },
    Asinh { cofactor: f64 },
    Log { decades: f64 },
}

impl Transform {
    /// Applies the transform. A value that cannot be transformed (e.g. log of
    /// a non-positive value) stays at the raw layer with a diagnostic; we never
    /// fabricate or drop a point for plotting.
    pub fn apply(&self, x: f64) -> Result<f64, String> {
        if !x.is_finite() {
            return Err("non-finite raw value".into());
        }
        match self {
            Transform::Linear { gain } => Ok(x * gain),
            Transform::Asinh { cofactor } if *cofactor > 0.0 => Ok((x / cofactor).asinh()),
            Transform::Asinh { .. } => Err("asinh cofactor must be positive".into()),
            Transform::Log { .. } => {
                if x > 0.0 {
                    Ok(x.log10())
                } else {
                    Err("log transform undefined for non-positive value".into())
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Inside,
    Outside,
    Boundary,
}

fn point_segment_dist(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) };
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Membership with explicit boundary detection (distance < eps to an edge).
/// Boundary points are kept as two legal candidates (inclusive/exclusive).
pub fn point_in_polygon(poly: &[[f64; 2]], p: [f64; 2], eps: f64) -> Membership {
    let n = poly.len();
    if n < 3 {
        return Membership::Outside;
    }
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if point_segment_dist(p[0], p[1], a[0], a[1], b[0], b[1]) < eps {
            return Membership::Boundary;
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        let intersect = (yi > p[1]) != (yj > p[1])
            && p[0] < (xj - xi) * (p[1] - yi) / (yj - yi) + xi;
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    if inside { Membership::Inside } else { Membership::Outside }
}

pub const BOUNDARY_EPS: f64 = 4.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn inside_outside_boundary() {
        let s = square();
        assert_eq!(point_in_polygon(&s, [5.0, 5.0], 1e-6), Membership::Inside);
        assert_eq!(point_in_polygon(&s, [20.0, 5.0], 1e-6), Membership::Outside);
        assert_eq!(point_in_polygon(&s, [0.0, 5.0], 1e-6), Membership::Boundary);
        assert_eq!(point_in_polygon(&s, [10.0, 10.0], 1e-6), Membership::Boundary);
    }

    #[test]
    fn log_rejects_nonpositive() {
        assert!(Transform::Log { decades: 4.0 }.apply(0.0).is_err());
        assert!(Transform::Asinh { cofactor: 150.0 }.apply(0.0).is_ok());
        assert!((Transform::Linear { gain: 2.0 }.apply(3.0).unwrap() - 6.0).abs() < 1e-12);
    }
}
