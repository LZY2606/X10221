//! 纯领域内核：补偿矩阵、变换、门几何、依赖失效。
//! 不依赖数据库与 HTTP，便于单元测试。

use nalgebra::DMatrix;
use serde::{Deserialize, Serialize};

/// 可被静默求逆的倒数条件数下限（1/condition number）。
pub const RCOND_SILENT_INV: f64 = 1e-8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompStatus {
    /// 良态：普通逆可用。
    WellConditioned,
    /// 近奇异：禁止默默求逆，保留全部合法候选。
    NearSingular,
    /// 严格奇异：逆不存在。
    Singular,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompAnalysis {
    pub status: CompStatus,
    pub rcond: f64,
    pub determinant: f64,
    /// 全部合法补偿候选；良态时为 exact 单候选。
    pub candidates: Vec<CompCandidate>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompCandidate {
    /// 稳定身份：exact / ridge / pseudo-inverse / uncompensated
    pub strategy: String,
    /// 候选所用算子（对荧光通道方阵）；None 表示单位（不补偿）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<Vec<Vec<f64>>>,
    pub rationale: String,
    pub admissible: bool,
}

fn dmatrix(rows: &[Vec<f64>]) -> DMatrix<f64> {
    let n = rows.len();
    DMatrix::from_row_slice(n, n, &rows.iter().flatten().copied().collect::<Vec<f64>>())
}

/// 分析 spillover 矩阵。近奇异/奇异时绝不返回单一“默默求逆”结果，
/// 而是把每个合法候选算子都交给上层保存。
pub fn analyze_compensation(rows: &[Vec<f64>]) -> CompAnalysis {
    let s = dmatrix(rows);
    let n = s.nrows();
    let det = s.clone().determinant();
    let svd = s.clone().svd(true, true);
    let sing = svd.singular_values.iter().copied().collect::<Vec<f64>>();
    let smax = sing.iter().cloned().fold(0.0_f64, f64::max);
    let smin = sing.iter().cloned().fold(f64::INFINITY, f64::min);
    let rcond = if smax > 0.0 { smin / smax } else { 0.0 };

    let mut diagnostics = Vec::new();
    let mut candidates = Vec::new();

    let status = if rcond <= 1e-12 || det.abs() <= 1e-12 {
        CompStatus::Singular
    } else if rcond < RCOND_SILENT_INV {
        CompStatus::NearSingular
    } else {
        CompStatus::WellConditioned
    };

    match status {
        CompStatus::WellConditioned => {
            let inv = s.try_inverse().expect("rcond 足够，逆必须存在");
            candidates.push(CompCandidate {
                strategy: "exact".into(),
                operator: Some(matrix_rows(&inv)),
                rationale: format!("rcond={rcond:.3e} >= {RCOND_SILENT_INV:.0e}，普通逆可靠"),
                admissible: true,
            });
        }
        CompStatus::NearSingular => {
            diagnostics.push(format!(
                "近奇异：rcond={rcond:.3e} < {RCOND_SILENT_INV:.0e}，det={det:.3e}；不默默求逆"
            ));
            // 候选 1：不补偿（单位算子），始终合法。
            candidates.push(CompCandidate {
                strategy: "uncompensated".into(),
                operator: None,
                rationale: "拒绝不稳定求逆，保留原始测量信号".into(),
                admissible: true,
            });
            // 候选 2：Tikhonov/ridge 正则化 (SᵀS+λI)^{-1}Sᵀ，λ 按尺度固定。
            let lambda = 1e-9 * smax * smax;
            let id = DMatrix::identity(n, n);
            let st = s.transpose();
            let reg = (&st * &s + lambda * id)
                .try_inverse()
                .map(|m| m * st);
            candidates.push(CompCandidate {
                strategy: "ridge".into(),
                operator: reg.as_ref().map(matrix_rows),
                rationale: format!("Tikhonov 正则 λ={lambda:.3e}，结果对噪声稳定但有偏"),
                admissible: reg.is_some(),
            });
            // 候选 3：SVD 截断伪逆。近奇异时它会把近共线方向压掉，
            // 作为合法候选保留，但必须由人工选择。
            let pinv = svd.pseudo_inverse(1e-10).ok();
            candidates.push(CompCandidate {
                strategy: "pseudo-inverse".into(),
                operator: pinv.as_ref().map(matrix_rows),
                rationale: "SVD 截断伪逆；最小范数解，可能掩盖共线通道".into(),
                admissible: pinv.is_some(),
            });
        }
        CompStatus::Singular => {
            diagnostics.push(format!(
                "严格奇异：rcond={rcond:.3e}, det={det:.3e}；逆不存在，不得产出 exact"
            ));
            candidates.push(CompCandidate {
                strategy: "uncompensated".into(),
                operator: None,
                rationale: "矩阵奇异，唯一安全选项是不补偿".into(),
                admissible: true,
            });
            // 伪逆仍可给出最小范数解；ridge 也可解。它们合法但不是“逆”。
            let pinv = s.svd(true, true).pseudo_inverse(1e-10).ok();
            candidates.push(CompCandidate {
                strategy: "pseudo-inverse".into(),
                operator: pinv.as_ref().map(matrix_rows),
                rationale: "最小范数最小二乘解；非唯一真逆，需人工知悉".into(),
                admissible: pinv.is_some(),
            });
        }
    }

    CompAnalysis { status, rcond, determinant: det, candidates, diagnostics }
}

fn matrix_rows(m: &DMatrix<f64>) -> Vec<Vec<f64>> {
    (0..m.nrows()).map(|r| (0..m.ncols()).map(|c| m[(r, c)]).collect()).collect()
}

/// asinh 变换：输出 asinh-index，带 cofactor 参数。
pub fn asinh_transform(value: f64, cofactor: f64) -> f64 {
    (value / cofactor).asinh()
}

/// 点是否在多边形内；inclusive 规则下恰落在边上的点算入（边界点检验）。
pub fn point_in_polygon(x: f64, y: f64, poly: &[[f64; 2]], inclusive: bool) -> bool {
    let n = poly.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if inclusive && on_edge(x, y, xi, yi, xj, yj) {
            return true;
        }
        if (yi > y) != (yj > y) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn on_edge(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> bool {
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

    #[test]
    fn baseline_matrix_well_conditioned_exact_only() {
        let a = analyze_compensation(&[vec![1.0, 0.08], vec![0.05, 1.0]]);
        assert_eq!(a.status, CompStatus::WellConditioned);
        assert_eq!(a.candidates.len(), 1);
        assert_eq!(a.candidates[0].strategy, "exact");
    }

    #[test]
    fn near_singular_is_not_silently_inverted() {
        let a = analyze_compensation(&[vec![1.0, 0.999999999], vec![0.999999998, 1.0]]);
        assert_eq!(a.status, CompStatus::NearSingular);
        assert!(a.rcond < RCOND_SILENT_INV);
        let strategies = a.candidates.iter().map(|c| c.strategy.as_str()).collect::<Vec<_>>();
        assert!(strategies.contains(&"ridge"));
        assert!(strategies.contains(&"pseudo-inverse"));
        assert!(strategies.contains(&"uncompensated"));
        assert!(!strategies.contains(&"exact"));
    }

    #[test]
    fn singular_has_no_exact_inverse() {
        let a = analyze_compensation(&[vec![1.0, 1.0], vec![1.0, 1.0]]);
        assert_eq!(a.status, CompStatus::Singular);
        assert!(!a.candidates.iter().any(|c| c.strategy == "exact"));
    }

    #[test]
    fn boundary_point_counts_inside_when_inclusive() {
        let p = [[200.0, 10000.0], [100000.0, 10000.0], [100000.0, 80000.0], [200.0, 80000.0]];
        assert!(point_in_polygon(200.0, 50000.0, &p, true));
        assert!(point_in_polygon(50000.0, 10000.0, &p, true));
        assert!(!point_in_polygon(100.0, 50000.0, &p, true));
    }

    #[test]
    fn asinh_matches_expected() {
        let v = asinh_transform(8000.0, 1000.0);
        assert!((v - 2.776).abs() < 1e-2);
        assert!(asinh_transform(1000.0, 1000.0) > 0.8);
    }
}
