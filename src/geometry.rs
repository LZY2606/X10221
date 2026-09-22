//! 门几何：射线法点在多边形内；边界点按门版本上的显式约定处理。

/// 射线法（x 正向）。落在多边形任意一条边上的点按 `include_boundary` 处理。
pub fn point_in_polygon(poly: &[[f64; 2]], point: [f64; 2], include_boundary: bool) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let (px, py) = (point[0], point[1]);
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if on_segment(xi, yi, xj, yj, px, py) {
            return include_boundary;
        }
        if (yi > py) != (yj > py) {
            let x_cross = (xj - xi) * (py - yi) / (yj - yi) + xi;
            if px < x_cross {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

fn on_segment(x1: f64, y1: f64, x2: f64, y2: f64, px: f64, py: f64) -> bool {
    let cross = (px - x1) * (y2 - y1) - (py - y1) * (x2 - x1);
    if cross.abs() > 1e-9 {
        return false;
    }
    let dot = (px - x1) * (px - x2) + (py - y1) * (py - y2);
    dot <= 1e-9
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn inside_outside_and_boundary() {
        let p = square();
        assert!(point_in_polygon(&p, [5.0, 5.0], true));
        assert!(!point_in_polygon(&p, [11.0, 5.0], true));
        // 底边中点：包含约定 -> true；排除约定 -> false
        assert!(point_in_polygon(&p, [5.0, 0.0], true));
        assert!(!point_in_polygon(&p, [5.0, 0.0], false));
        // 顶点也视为边界
        assert!(point_in_polygon(&p, [0.0, 0.0], true));
        assert!(!point_in_polygon(&p, [10.0, 10.0], false));
    }
}
