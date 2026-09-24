use crate::domain::types::{PolygonGate, Scaled};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InOut {
    In,
    Out,
    Boundary,
}

fn cross(ox: f64, oy: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    (ax - ox) * (by - oy) - (ay - oy) * (bx - ox)
}

fn on_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64, eps: f64) -> bool {
    let cr = cross(ax, ay, bx, by, px, py);
    if cr.abs() > eps * (1.0 + (ax.abs().max(bx.abs()))) {
        return false;
    }
    let dot = (px - ax) * (px - bx) + (py - ay) * (py - by);
    dot <= eps * eps
}

/// 多边形成员判定；边界点单独分类（Boundary），不悄悄归入内/外。
pub fn classify(gate: &PolygonGate, x: f64, y: f64) -> InOut {
    let v = &gate.vertices;
    let n = v.len();
    if n < 3 {
        return InOut::Out;
    }
    let span: f64 = v
        .iter()
        .flat_map(|p| [p[0].abs(), p[1].abs()])
        .fold(1.0, f64::max);
    let eps = 1e-9 * span;
    for i in 0..n {
        let a = v[i];
        let b = v[(i + 1) % n];
        if on_segment(x, y, a[0], a[1], b[0], b[1], eps) {
            return InOut::Boundary;
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (v[i][0], v[i][1]);
        let (xj, yj) = (v[j][0], v[j][1]);
        if (yi > y) != (yj > y) {
            let intersect = (xj - xi) * (y - yi) / (yj - yi) + xi;
            if x < intersect {
                inside = !inside;
            }
        }
        j = i;
    }
    if inside { InOut::In } else { InOut::Out }
}

/// 在指定两个通道的变换坐标上分类；坐标缺失视为无法决定。
pub fn classify_point(gate: &PolygonGate, channels: &[String], transformed: &[Scaled]) -> Option<InOut> {
    let xi = channels.iter().position(|c| c == &gate.x_channel)?;
    let yi = channels.iter().position(|c| c == &gate.y_channel)?;
    let x = transformed.get(xi)?.value?;
    let y = transformed.get(yi)?.value?;
    Some(classify(gate, x, y))
}
