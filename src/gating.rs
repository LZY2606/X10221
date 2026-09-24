//! 门控：多边形包含判定（边界点确定性规则）与群体计数。
//! 群体百分比分母钉住父群版本；边界点计入并标记，不静默丢弃。

use serde::{Deserialize, Serialize};

pub const BOUNDARY_EPS: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointPosition {
    Inside,
    Outside,
    /// 落在多边形边上：按确定规则计入内侧，并附 boundary 标记。
    Boundary,
}

fn point_to_segment_dist2(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
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
    (px - cx) * (px - cx) + (py - cy) * (py - cy)
}

/// 射线法 + 边界检测。`scale` 用于把 eps 换算到坐标量级。
pub fn classify(px: f64, py: f64, polygon: &[[f64; 2]], eps: f64) -> PointPosition {
    let n = polygon.len();
    if n < 3 {
        return PointPosition::Outside;
    }
    let eps2 = eps * eps;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (polygon[i][0], polygon[i][1]);
        let (xj, yj) = (polygon[j][0], polygon[j][1]);
        if point_to_segment_dist2(px, py, xi, yi, xj, yj) <= eps2 {
            return PointPosition::Boundary;
        }
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        PointPosition::Inside
    } else {
        PointPosition::Outside
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PopulationResult {
    pub count: usize,
    pub boundary_count: usize,
    pub extreme_count: usize,
    pub member_indices: Vec<usize>,
}

/// 在父群成员范围内计数（分母 = 父群成员数，父群版本由调用方钉住）。
pub fn count_population(
    xs: &[f64],
    ys: &[f64],
    polygon: &[[f64; 2]],
    parent_members: Option<&[usize]>,
    extreme_events: &[usize],
    eps: f64,
) -> PopulationResult {
    let extreme: std::collections::BTreeSet<usize> = extreme_events.iter().copied().collect();
    let candidates: Vec<usize> = match parent_members {
        Some(m) => m.to_vec(),
        None => (0..xs.len()).collect(),
    };
    let mut result = PopulationResult {
        count: 0,
        boundary_count: 0,
        extreme_count: 0,
        member_indices: Vec::new(),
    };
    for idx in candidates {
        match classify(xs[idx], ys[idx], polygon, eps) {
            PointPosition::Inside | PointPosition::Boundary => {
                result.count += 1;
                if extreme.contains(&idx) {
                    result.extreme_count += 1;
                }
                result.member_indices.push(idx);
            }
            PointPosition::Outside => {}
        }
    }
    result.boundary_count = result
        .member_indices
        .iter()
        .filter(|&&i| classify(xs[i], ys[i], polygon, eps) == PointPosition::Boundary)
        .count();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn inside_outside_boundary() {
        let sq = square();
        assert_eq!(classify(5.0, 5.0, &sq, BOUNDARY_EPS), PointPosition::Inside);
        assert_eq!(classify(15.0, 5.0, &sq, BOUNDARY_EPS), PointPosition::Outside);
        // 边界点：恰好落在门上沿
        assert_eq!(classify(5.0, 10.0, &sq, BOUNDARY_EPS), PointPosition::Boundary);
        // 顶点也是边界
        assert_eq!(classify(0.0, 0.0, &sq, BOUNDARY_EPS), PointPosition::Boundary);
    }

    #[test]
    fn boundary_points_counted_and_flagged() {
        let sq = square();
        let xs = vec![5.0, 5.0, 20.0];
        let ys = vec![5.0, 10.0, 5.0];
        let r = count_population(&xs, &ys, &sq, None, &[], BOUNDARY_EPS);
        assert_eq!(r.count, 2);
        assert_eq!(r.boundary_count, 1);
    }

    #[test]
    fn parent_scope_limits_denominator() {
        let sq = square();
        let xs = vec![5.0, 6.0, 7.0];
        let ys = vec![5.0, 5.0, 5.0];
        let r = count_population(&xs, &ys, &sq, Some(&[0, 2]), &[], BOUNDARY_EPS);
        assert_eq!(r.count, 2);
        assert_eq!(r.member_indices, vec![0, 2]);
    }
}
