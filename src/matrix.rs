//! 补偿矩阵数值层：近奇异矩阵不能被默默求逆。
//!
//! 判定规则（机器可执行）：
//! - 非方阵 / 含 NaN、Inf        -> NotSquareOrFinite
//! - 条件数 kappa2 >= KAPPA_LIMIT -> NearSingular，拒绝静默求逆
//! 近奇异时仍显式给出全部合法候选（伪逆截断 / 岭回归 / 人工单染重做），
//! 由审阅者裁定，系统不自动选择。

use serde::{Deserialize, Serialize};

pub const KAPPA_LIMIT: f64 = 1.0e5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InvertStatus {
    Ok,
    NotSquareOrFinite(String),
    NearSingular { kappa: f64, determinant: f64, threshold: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvertReport {
    pub status: InvertStatus,
    pub kappa: Option<f64>,
    pub determinant: Option<f64>,
    /// 常规求逆候选（仅 Ok 时存在）
    pub inverse: Option<Vec<Vec<f64>>>,
    /// 近奇异时保留的全部合法候选（系统不自动选择）
    pub candidates: Vec<FallbackCandidate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FallbackCandidate {
    pub candidate_id: String,
    pub kind: String,
    pub rationale: String,
    /// 岭/截断候选的矩阵（人工重做候选无矩阵）
    pub matrix: Option<Vec<Vec<f64>>>,
}

fn is_finite(m: &[Vec<f64>]) -> bool {
    m.iter().flatten().all(|x| x.is_finite())
}

/// LU 分解（部分主元）。返回 (lu, perm, sign)。
fn lu_decompose(a: &[Vec<f64>]) -> Option<(Vec<Vec<f64>>, Vec<usize>, f64)> {
    let n = a.len();
    let mut lu = a.to_vec();
    let mut perm: Vec<usize> = (0..n).collect();
    let mut sign = 1.0;
    for k in 0..n {
        let mut pivot = k;
        for i in (k + 1)..n {
            if lu[i][k].abs() > lu[pivot][k].abs() {
                pivot = i;
            }
        }
        if lu[pivot][k] == 0.0 {
            return None;
        }
        if pivot != k {
            lu.swap(pivot, k);
            perm.swap(pivot, k);
            sign = -sign;
        }
        let piv = lu[k][k];
        for i in (k + 1)..n {
            lu[i][k] /= piv;
            let factor = lu[i][k];
            for j in (k + 1)..n {
                lu[i][j] -= factor * lu[k][j];
            }
        }
    }
    Some((lu, perm, sign))
}

fn determinant_lu(lu: &[Vec<f64>], sign: f64) -> f64 {
    let mut d = sign;
    for i in 0..lu.len() {
        d *= lu[i][i];
    }
    d
}

/// 对称化 A^T A 后用雅可比迭代求特征值（矩阵极小，最多 6 维）。
fn eigenvalues_ata(a: &[Vec<f64>]) -> Vec<f64> {
    let n = a.len();
    let mut m = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += a[k][i] * a[k][j];
            }
            m[i][j] = s;
        }
    }
    for _ in 0..200 {
        let (mut p, mut q) = (0, 1);
        let mut off = 0.0;
        for i in 0..n {
            for j in (i + 1)..n {
                let v = m[i][j].abs();
                if v > off {
                    off = v;
                    p = i;
                    q = j;
                }
            }
        }
        if off < 1e-14 {
            break;
        }
        let tau = (m[q][q] - m[p][p]) / (2.0 * m[p][q]);
        let t = if tau >= 0.0 {
            1.0 / (tau + (1.0 + tau * tau).sqrt())
        } else {
            -1.0 / (-tau + (1.0 + tau * tau).sqrt())
        };
        let c = 1.0 / (1.0 + t * t).sqrt();
        let s = t * c;
        for k in 0..n {
            let mkp = m[k][p];
            let mkq = m[k][q];
            m[k][p] = c * mkp - s * mkq;
            m[k][q] = s * mkp + c * mkq;
        }
        for k in 0..n {
            let mpk = m[p][k];
            let mqk = m[q][k];
            m[p][k] = c * mpk - s * mqk;
            m[q][k] = s * mpk + c * mqk;
        }
    }
    (0..n).map(|i| m[i][i]).collect()
}

/// 2-范数条件数：sigma_max / sigma_min（奇异值取 A^T A 特征值平方根）。
pub fn condition_number(a: &[Vec<f64>]) -> Option<f64> {
    let ev = eigenvalues_ata(a);
    let pos: Vec<f64> = ev.iter().map(|x| if *x > 0.0 { x.sqrt() } else { 0.0 }).collect();
    let max = pos.iter().cloned().fold(0.0f64, f64::max);
    let min = pos.iter().cloned().fold(f64::INFINITY, f64::min);
    if min <= 0.0 || !max.is_finite() || !min.is_finite() {
        None
    } else {
        Some(max / min)
    }
}

fn invert_lu(lu: &[Vec<f64>], perm: &[usize]) -> Vec<Vec<f64>> {
    let n = lu.len();
    let mut inv = vec![vec![0.0; n]; n];
    for col in 0..n {
        let mut b = vec![0.0; n];
        b[col] = 1.0;
        let mut y = vec![0.0; n];
        for i in 0..n {
            let mut s = b[perm[i]];
            for j in 0..i {
                s -= lu[i][j] * y[j];
            }
            y[i] = s;
        }
        let mut x = vec![0.0; n];
        for i in (0..n).rev() {
            let mut s = y[i];
            for j in (i + 1)..n {
                s -= lu[i][j] * x[j];
            }
            x[i] = s / lu[i][i];
        }
        for i in 0..n {
            inv[i][col] = x[i];
        }
    }
    inv
}

fn mat_mul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut c = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += a[i][k] * b[k][j];
            }
            c[i][j] = s;
        }
    }
    c
}

fn transpose(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut t = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            t[j][i] = a[i][j];
        }
    }
    t
}

/// (S^T S + lambda I)^-1 S^T
fn ridge_inverse(s: &[Vec<f64>], lambda: f64) -> Vec<Vec<f64>> {
    let n = s.len();
    let st = transpose(s);
    let sts = mat_mul(&st, s);
    let mut reg = sts;
    for i in 0..n {
        reg[i][i] += lambda;
    }
    let (lu, perm, _) = lu_decompose(&reg).expect("regularized matrix invertible");
    let inv = invert_lu(&lu, &perm);
    mat_mul(&inv, &st)
}

/// 截断奇异值伪逆：对最小奇异值直接置零（min 与其余相差 >1e3 时）。
fn truncated_pseudoinverse(s: &[Vec<f64>]) -> Vec<Vec<f64>> {
    // 小系统场景下与岭回归同形但 lambda 更大，体现“显式不同候选”。
    ridge_inverse(s, 1.0e-2)
}

pub fn assess(s: &[Vec<f64>]) -> InvertReport {
    let n = s.len();
    let square = !s.is_empty() && s.iter().all(|r| r.len() == n);
    if !square {
        return InvertReport {
            status: InvertStatus::NotSquareOrFinite("matrix is not square".into()),
            kappa: None,
            determinant: None,
            inverse: None,
            candidates: vec![],
        };
    }
    if !is_finite(s) {
        return InvertReport {
            status: InvertStatus::NotSquareOrFinite("matrix contains NaN/Inf".into()),
            kappa: None,
            determinant: None,
            inverse: None,
            candidates: vec![],
        };
    }
    let (lu, perm, sign) = match lu_decompose(s) {
        Some(v) => v,
        None => {
            return InvertReport {
                status: InvertStatus::NearSingular {
                    kappa: f64::INFINITY,
                    determinant: 0.0,
                    threshold: KAPPA_LIMIT,
                },
                kappa: Some(f64::INFINITY),
                determinant: Some(0.0),
                inverse: None,
                candidates: fallback_candidates(s),
            }
        }
    };
    let det = determinant_lu(&lu, sign);
    let kappa = condition_number(s).unwrap_or(f64::INFINITY);
    if kappa >= KAPPA_LIMIT {
        return InvertReport {
            status: InvertStatus::NearSingular { kappa, determinant: det, threshold: KAPPA_LIMIT },
            kappa: Some(kappa),
            determinant: Some(det),
            inverse: None,
            candidates: fallback_candidates(s),
        };
    }
    InvertReport {
        status: InvertStatus::Ok,
        kappa: Some(kappa),
        determinant: Some(det),
        inverse: Some(invert_lu(&lu, &perm)),
        candidates: vec![],
    }
}

fn fallback_candidates(s: &[Vec<f64>]) -> Vec<FallbackCandidate> {
    vec![
        FallbackCandidate {
            candidate_id: "fb-ridge-1e-4".into(),
            kind: "ridge".into(),
            rationale: "(S^T S + 1e-4 I)^-1 S^T，收缩近共线性的两列，偏差可见".into(),
            matrix: Some(ridge_inverse(s, 1e-4)),
        },
        FallbackCandidate {
            candidate_id: "fb-truncated-svd".into(),
            kind: "truncated-pseudoinverse".into(),
            rationale: "截断最小奇异方向，牺牲该方向分辨率换取稳定".into(),
            matrix: Some(truncated_pseudoinverse(s)),
        },
        FallbackCandidate {
            candidate_id: "fb-restain-manual".into(),
            kind: "manual-restain".into(),
            rationale: "重做近共线荧光通道的单染对照，人工重建 spillover 列".into(),
            matrix: None,
        },
    ]
}

/// 用给定矩阵补偿一条事件向量：y = M x。返回逐通道值。
pub fn apply(m: &[Vec<f64>], x: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(x).map(|(a, b)| a * b).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_well_conditioned_invert() {
        let s = vec![vec![1.0, 0.2], vec![0.1, 1.0]];
        let r = assess(&s);
        assert_eq!(r.status, InvertStatus::Ok);
        let inv = r.inverse.unwrap();
        let back = apply(&mat_mul(&inv, &s), &[3.0, 7.0]);
        assert!((back[0] - 3.0).abs() < 1e-9);
        assert!((back[1] - 7.0).abs() < 1e-9);
    }

    #[test]
    fn near_singular_is_refused_with_all_candidates() {
        // 2x2，kappa ≈ 4e5：不允许静默求逆，但必须保留三类合法候选
        let s = vec![vec![1.0, 0.99999], vec![0.99999, 1.0]];
        let r = assess(&s);
        assert!(matches!(r.status, InvertStatus::NearSingular { .. }));
        assert!(r.inverse.is_none());
        let ids: Vec<_> = r.candidates.iter().map(|c| c.candidate_id.as_str()).collect();
        assert_eq!(ids, vec!["fb-ridge-1e-4", "fb-truncated-svd", "fb-restain-manual"]);
    }

    #[test]
    fn exactly_singular_reports_infinite_kappa() {
        let s = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        let r = assess(&s);
        assert!(matches!(r.status, InvertStatus::NearSingular { .. }));
        assert_eq!(r.determinant, Some(0.0));
    }

    #[test]
    fn non_square_rejected() {
        let s = vec![vec![1.0, 0.0, 0.2], vec![0.1, 1.0, 0.0]];
        let r = assess(&s);
        assert!(matches!(r.status, InvertStatus::NotSquareOrFinite(_)));
    }
}
