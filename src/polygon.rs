//! 门多边形点包含判定。
//!
//! 固定规则 `edge-exclusive/v1`：标准奇偶射线法；恰好落在多边形边上的点判为**外**。
//! 规则版本写入门材料，保证边界点结论可按当时规则复现。边界采用精确比较，
//! 固定案例中的边界点以整数坐标构造，避免浮点歧义。

/// 判定点 p 是否在多边形内。返回 `Some(true/false)`；多边形退化返回 None。
pub fn contains(vertices: &[[f64; 2]], p: [f64; 2]) -> Option<bool> {
    let n = vertices.len();
    if n < 3 {
        return None;
    }
    // 边界点：显式排除。
    for i in 0..n {
        let a = vertices[i];
        let b = vertices[(i + 1) % n];
        if on_segment(a, b, p) {
            return Some(false);
        }
    }
    // 奇偶射线法（水平向右射线），顶点约定：y1 <= py < y2，排除水平边。
    let (px, py) = (p[0], p[1]);
    let mut inside = false;
    for i in 0..n {
        let (x1, y1) = (vertices[i][0], vertices[i][1]);
        let (x2, y2) = (vertices[(i + 1) % n][0], vertices[(i + 1) % n][1]);
        if (y1 > py) != (y2 > py) {
            let xint = x1 + (py - y1) * (x2 - x1) / (y2 - y1);
            if px < xint {
                inside = !inside;
            }
        }
    }
    Some(inside)
}

fn cross(ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    ax * by - ay * bx
}

fn on_segment(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    // 精确比较：固定案例的边界点以整数坐标构造，避免浮点歧义。
    let cr = cross(b[0] - a[0], b[1] - a[1], p[0] - a[0], p[1] - a[1]);
    if cr != 0.0 {
        return false;
    }
    p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn inside_and_outside() {
        let s = square();
        assert_eq!(contains(&s, [5.0, 5.0]), Some(true));
        assert_eq!(contains(&s, [15.0, 5.0]), Some(false));
    }

    #[test]
    fn point_on_edge_is_exclusive() {
        let s = square();
        assert_eq!(contains(&s, [5.0, 0.0]), Some(false)); // 底边
        assert_eq!(contains(&s, [10.0, 5.0]), Some(false)); // 右边
        assert_eq!(contains(&s, [0.0, 0.0]), Some(false)); // 顶点
    }

    #[test]
    fn degenerate_polygon_none() {
        assert_eq!(contains(&[[0.0, 0.0], [1.0, 1.0]], [0.5, 0.5]), None);
    }
}
