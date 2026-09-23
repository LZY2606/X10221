//! 补偿矩阵数值层。
//!
//! 规则 RULE-COMP-001：近奇异/奇异补偿矩阵不能被默默求逆。
//! - 行列式绝对值 < 1e-12：判定为奇异，拒绝求逆；
//! - 估计条件数 >= COND_LIMIT（默认 30）：判定为近奇异，求逆结果不唯一，
//!   必须保留全部合法候选并交由人工裁定，不允许返回单一“静默”结果。

pub const COND_LIMIT: f64 = 30.0;
pub const SINGULAR_DET_EPS: f64 = 1e-12;
pub const REGULARIZATION_LAMBDA: f64 = 1e-3;

#[derive(Clone, Debug, PartialEq)]
pub struct Matrix {
    pub n: usize,
    pub rows: Vec<Vec<f64>>,
}

impl Matrix {
    pub fn new(rows: Vec<Vec<f64>>) -> Self {
        let n = rows.len();
        Matrix { n, rows }
    }

    pub fn identity(n: usize) -> Self {
        let rows = (0..n)
            .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
            .collect();
        Matrix { n, rows }
    }

    pub fn determinant(&self) -> f64 {
        let mut a = self.rows.clone();
        let mut det = 1.0;
        for col in 0..self.n {
            let Some(pivot_row) = (col..self.n)
                .max_by(|x, y| a[*x][col].abs().partial_cmp(&a[*y][col]).unwrap())
            else {
                return 0.0;
            };
            if a[pivot_row][col].abs() < f64::EPSILON {
                return 0.0;
            }
            if pivot_row != col {
                a.swap(pivot_row, col);
                det = -det;
            }
            let pivot = a[col][col];
            det *= pivot;
            for row in (col + 1)..self.n {
                let factor = a[row][col] / pivot;
                for c in col..self.n {
                    a[row][c] -= factor * a[col][c];
                }
            }
        }
        det
    }

    /// 高斯-若尔当逆；数值不可逆时返回 None。
    pub fn inverse(&self) -> Option<Matrix> {
        let n = self.n;
        let mut a: Vec<Vec<f64>> = Vec::with_capacity(n);
        for i in 0..n {
            let mut row = self.rows[i].clone();
            row.extend(Matrix::identity(n).rows[i].iter().copied());
            a.push(row);
        }
        for col in 0..n {
            let Some(pivot_row) = (col..n)
                .max_by(|x, y| a[*x][col].abs().partial_cmp(&a[*y][col]).unwrap())
            else {
                return None;
            };
            if a[pivot_row][col].abs() < SINGULAR_DET_EPS {
                return None;
            }
            a.swap(pivot_row, col);
            let pivot = a[col][col];
            for c in 0..2 * n {
                a[col][c] /= pivot;
            }
            for row in 0..n {
                if row == col {
                    continue;
                }
                let factor = a[row][col];
                for c in 0..2 * n {
                    a[row][c] -= factor * a[col][c];
                }
            }
        }
        let rows = a.into_iter().map(|r| r[n..].to_vec()).collect();
        Some(Matrix { n, rows })
    }

    /// 2-范数条件数的下界估计：||A||_inf * ||A^-1||_inf。
    pub fn condition_number(&self) -> Option<f64> {
        let inv = self.inverse()?;
        Some(infinity_norm(&self) * infinity_norm(&inv))
    }
}

pub fn infinity_norm(m: &Matrix) -> f64 {
    m.rows
        .iter()
        .map(|r| r.iter().map(|v| v.abs()).sum())
        .fold(0.0, f64::max)
}

#[derive(Clone, Debug)]
pub struct CompCandidate {
    pub kind: String,
    pub matrix: Matrix,
    pub note: String,
}

#[derive(Clone, Debug)]
pub struct CompAnalysis {
    pub determinant: f64,
    pub condition: Option<f64>,
    pub classification: String,
    pub candidates: Vec<CompCandidate>,
}

/// 补偿矩阵评估：返回分类与全部合法候选。
/// - `singular`：无候选（不能求逆）；
/// - `near_singular`：直接逆 + 岭正则逆两个候选，均带诊断；
/// - `well_conditioned`：直接逆单一候选。
pub fn analyze_compensation(matrix: &Matrix) -> CompAnalysis {
    let det = matrix.determinant();
    let condition = matrix.condition_number();

    if det.abs() < SINGULAR_DET_EPS {
        return CompAnalysis {
            determinant: det,
            condition: None,
            classification: "singular".into(),
            candidates: vec![],
        };
    }

    let direct = matrix.inverse().expect("non-singular matrix is invertible");
    let is_near = condition.map(|c| c >= COND_LIMIT).unwrap_or(false);
    if is_near {
        let ridge = ridge_inverse(matrix, REGULARIZATION_LAMBDA);
        CompAnalysis {
            determinant: det,
            condition,
            classification: "near_singular".into(),
            candidates: vec![
                CompCandidate {
                    kind: "direct_inverse".into(),
                    matrix: direct,
                    note: format!(
                        "条件数 {:.2} 超过阈值 {COND_LIMIT}，逆矩阵数值不稳定",
                        condition.unwrap_or(f64::NAN)
                    ),
                },
                CompCandidate {
                    kind: "ridge_inverse".into(),
                    matrix: ridge,
                    note: format!("λ = {REGULARIZATION_LAMBDA} 的岭正则逆，偏差换稳定"),
                },
            ],
        }
    } else {
        CompAnalysis {
            determinant: det,
            condition,
            classification: "well_conditioned".into(),
            candidates: vec![CompCandidate {
                kind: "direct_inverse".into(),
                matrix: direct,
                note: "条件数可接受，直接求逆".into(),
            }],
        }
    }
}

/// 岭逆：(A^T A + λI)^-1 A^T（方阵情形）。
pub fn ridge_inverse(a: &Matrix, lambda: f64) -> Matrix {
    let n = a.n;
    let at = transpose(a);
    let ata = multiply(&at, a);
    let mut reg = ata;
    for i in 0..n {
        reg.rows[i][i] += lambda;
    }
    let inv = reg.inverse().expect("regularized matrix invertible");
    multiply(&inv, &at)
}

pub fn transpose(a: &Matrix) -> Matrix {
    let rows = (0..a.n)
        .map(|i| (0..a.n).map(|j| a.rows[j][i]).collect())
        .collect();
    Matrix { n: a.n, rows }
}

pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let n = a.n;
    let rows = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| (0..n).map(|k| a.rows[i][k] * b.rows[k][j]).sum())
                .collect()
        })
        .collect();
    Matrix { n, rows }
}

pub fn matvec(a: &Matrix, v: &[f64]) -> Vec<f64> {
    a.rows
        .iter()
        .map(|r| r.iter().zip(v).map(|(x, y)| x * y).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn identity_inverse() {
        let m = Matrix::identity(3);
        let inv = m.inverse().unwrap();
        assert_eq!(inv, m);
        let a = analyze_compensation(&m);
        assert_eq!(a.classification, "well_conditioned");
        assert_eq!(a.candidates.len(), 1);
    }

    #[test]
    fn singular_matrix_has_no_candidate() {
        let m = Matrix::new(vec![vec![1.0, 1.0], vec![1.0, 1.0]]);
        let a = analyze_compensation(&m);
        assert_eq!(a.classification, "singular");
        assert!(a.candidates.is_empty());
        assert!(close(a.determinant, 0.0));
    }

    #[test]
    fn near_singular_keeps_two_candidates() {
        // 行列式 1e-7，条件数约 4e7
        let m = Matrix::new(vec![vec![1.0, 1.0], vec![1.0, 1.0000001]]);
        let a = analyze_compensation(&m);
        assert_eq!(a.classification, "near_singular");
        assert_eq!(a.candidates.len(), 2);
        assert!(a.condition.unwrap() >= COND_LIMIT);
    }

    #[test]
    fn ridge_inverse_is_stable_and_close() {
        let m = Matrix::new(vec![vec![1.0, 1.0], vec![1.0, 1.0000001]]);
        let direct = m.inverse().unwrap();
        let ridge = ridge_inverse(&m, 1e-3);
        let back = multiply(&m, &ridge);
        for i in 0..2 {
            for j in 0..2 {
                assert!(back.rows[i][j].is_finite());
            }
        }
        let direct_back = multiply(&m, &direct);
        for i in 0..2 {
            assert!(close(direct_back.rows[i][i], 1.0));
        }
    }

    #[test]
    fn matvec_basic() {
        let m = Matrix::identity(2);
        assert_eq!(matvec(&m, &[3.0, 4.0]), vec![3.0, 4.0]);
    }
}
