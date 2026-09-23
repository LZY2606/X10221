//! Polygon gates with an explicit, deterministic boundary rule:
//! points within BOUNDARY_EPS of an edge count as inside AND are reported
//! as boundary events so the review page can highlight them.

pub const BOUNDARY_EPS: f64 = 1e-4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Inside,
    Boundary,
    Outside,
}

impl Membership {
    pub fn is_member(self) -> bool {
        matches!(self, Membership::Inside | Membership::Boundary)
    }
}

fn point_to_segment_dist(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len_sq).clamp(0.0, 1.0)
    };
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

pub fn boundary_distance(x: f64, y: f64, poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let a = poly[i];
            let b = poly[(i + 1) % n];
            point_to_segment_dist(x, y, a[0], a[1], b[0], b[1])
        })
        .fold(f64::INFINITY, f64::min)
}

pub fn inside_ray_cast(x: f64, y: f64, poly: &[[f64; 2]]) -> bool {
    let n = poly.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn classify(x: f64, y: f64, poly: &[[f64; 2]]) -> Membership {
    if boundary_distance(x, y, poly) <= BOUNDARY_EPS {
        return Membership::Boundary;
    }
    if inside_ray_cast(x, y, poly) {
        Membership::Inside
    } else {
        Membership::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<[f64; 2]> {
        vec![[4.0, 0.0], [8.0, 0.0], [8.0, 8.0], [4.0, 8.0]]
    }

    #[test]
    fn interior_and_exterior() {
        let p = square();
        assert_eq!(classify(6.0, 4.0, &p), Membership::Inside);
        assert_eq!(classify(2.0, 4.0, &p), Membership::Outside);
    }

    #[test]
    fn edge_and_vertex_points_are_boundary_members() {
        let p = square();
        assert_eq!(classify(4.0, 2.0, &p), Membership::Boundary); // left edge
        assert_eq!(classify(4.0, 0.0, &p), Membership::Boundary); // vertex
        assert_eq!(classify(6.0, 8.0, &p), Membership::Boundary); // top edge
        assert!(Membership::Boundary.is_member());
    }

    #[test]
    fn near_edge_within_epsilon_is_boundary() {
        let p = square();
        assert_eq!(classify(4.0 + 5e-5, 3.0, &p), Membership::Boundary);
        assert_eq!(classify(4.0 + 5e-3, 3.0, &p), Membership::Inside);
    }
}
