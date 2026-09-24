//! 领域计算：补偿矩阵、变换、门多边形。所有函数纯计算，不触碰存储。

/// 近奇异判定阈值（条件数）。超过该值的补偿矩阵禁止默默求逆。
pub const NEAR_SINGULAR_COND: f64 = 1.0e6;
/// 边界点判定容差（相对坐标尺度）。
pub const BOUNDARY_EPS: f64 = 1.0e-6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix2 {
    pub m: [[f64; 2]; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatrixClass {
    /// 可安全求逆：行列式、条件数、逆矩阵。
    Invertible { det: f64, cond: f64, inv: Matrix2 },
    /// 近奇异：拒绝求逆，必须走候选裁定。
    NearSingular { det: f64, cond: f64 },
}

impl Matrix2 {
    pub fn det(&self) -> f64 {
        self.m[0][0] * self.m[1][1] - self.m[0][1] * self.m[1][0]
    }
    /// 2x2 条件数：sqrt(λmax/λmin)，λ 为 MᵀM 的特征值。
    pub fn cond(&self) -> f64 {
        let a = self.m[0][0];
        let b = self.m[0][1];
        let c = self.m[1][0];
        let d = self.m[1][1];
        let tr = a * a + b * b + c * c + d * d;
        let det_mm = self.det() * self.det();
        let disc = (tr * tr - 4.0 * det_mm).max(0.0).sqrt();
        let lmax = (tr + disc) / 2.0;
        let lmin = (tr - disc) / 2.0;
        if lmin <= 0.0 {
            return f64::INFINITY;
        }
        (lmax / lmin).sqrt()
    }
    pub fn invert(&self) -> Option<Matrix2> {
        let det = self.det();
        if det == 0.0 {
            return None;
        }
        Some(Matrix2 {
            m: [
                [self.m[1][1] / det, -self.m[0][1] / det],
                [-self.m[1][0] / det, self.m[0][0] / det],
            ],
        })
    }
    pub fn classify(&self) -> MatrixClass {
        let det = self.det();
        let cond = self.cond();
        if !det.is_finite() || !cond.is_finite() || cond > NEAR_SINGULAR_COND || det.abs() < 1e-12 {
            MatrixClass::NearSingular { det, cond }
        } else {
            MatrixClass::Invertible {
                det,
                cond,
                inv: self.invert().expect("invertible"),
            }
        }
    }
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.m[0][0] * x + self.m[0][1] * y,
            self.m[1][0] * x + self.m[1][1] * y,
        )
    }
}

/// asinh 变换：asinh(v/cofactor)*scale。仅在派生层调用；原始值不入此函数。
pub fn asinh_transform(v: f64, cofactor: f64, scale: f64) -> f64 {
    (v / cofactor).asinh() * scale
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointPosition {
    Inside,
    Outside,
    /// 恰好落在多边形边上：必须保留双候选，不得静默归类。
    Boundary,
}

/// 射线法 + 边界检测。
pub fn point_in_polygon(x: f64, y: f64, poly: &[(f64, f64)]) -> PointPosition {
    let n = poly.len();
    if n < 3 {
        return PointPosition::Outside;
    }
    let scale = poly
        .iter()
        .map(|p| p.0.abs().max(p.1.abs()))
        .fold(1.0_f64, f64::max);
    let eps = BOUNDARY_EPS * scale;
    for i in 0..n {
        let (x1, y1) = poly[i];
        let (x2, y2) = poly[(i + 1) % n];
        // 点到线段距离（叉积）与包围盒检查
        let cross = (x - x1) * (y2 - y1) - (y - y1) * (x2 - x1);
        let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
        if len > 0.0 && (cross / len).abs() <= eps {
            let dot = (x - x1) * (x2 - x1) + (y - y1) * (y2 - y1);
            if dot >= -eps && dot <= len * len + eps {
                return PointPosition::Boundary;
            }
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_matrix_inverts() {
        let m = Matrix2 { m: [[1.0, 0.12], [0.03, 1.0]] };
        match m.classify() {
            MatrixClass::Invertible { det, cond, inv } => {
                assert!((det - 0.9964).abs() < 1e-9);
                assert!(cond < 10.0);
                let (x, y) = inv.apply(1.0, 0.12);
                assert!((x - 1.0).abs() < 1e-9 && y.abs() < 1e-9);
            }
            other => panic!("expected invertible, got {other:?}"),
        }
    }

    #[test]
    fn near_singular_matrix_rejected() {
        let m = Matrix2 { m: [[1.0, 0.9999999], [0.9999999, 1.0]] };
        match m.classify() {
            MatrixClass::NearSingular { det, cond } => {
                assert!(det.abs() < 1e-5);
                assert!(cond > NEAR_SINGULAR_COND);
            }
            other => panic!("expected near-singular, got {other:?}"),
        }
    }

    #[test]
    fn boundary_point_detected() {
        let poly = [(40000.0, 20000.0), (120000.0, 20000.0), (120000.0, 90000.0), (40000.0, 90000.0)];
        assert_eq!(point_in_polygon(40000.0, 50000.0, &poly), PointPosition::Boundary);
        assert_eq!(point_in_polygon(80000.0, 50000.0, &poly), PointPosition::Inside);
        assert_eq!(point_in_polygon(200000.0, 150000.0, &poly), PointPosition::Outside);
    }
}
