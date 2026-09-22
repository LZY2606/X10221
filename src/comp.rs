//! 补偿矩阵数值层：单侧 Jacobi SVD、近奇异检测、显式多候选。
//!
//! 规则 RULE.NS-01：当 rcond(S) 小于阈值（1e-3）时矩阵判定为近奇异，
//! 禁止“默默求逆”：直接逆被标记为 unstable 且不可被采纳；
//! 系统必须保留全部合法候选（两个 ridge 正则化水平），裁定只能显式选择其一。

pub const NEAR_SINGULAR_RCOND_THRESHOLD: f64 = 1e-3;

#[derive(Debug, Clone)]
pub struct Svd3 {
    pub u: [[f64; 3]; 3],
    pub sigma: [f64; 3],
    pub vt: [[f64; 3]; 3],
}

fn norm_col(a: &[[f64; 3]; 3], col: usize) -> f64 {
    (a[0][col] * a[0][col] + a[1][col] * a[1][col] + a[2][col] * a[2][col]).sqrt()
}

/// 单侧 Jacobi（在 A 上消去 A^T A 的非对角），得到 A = U S V^T。
pub fn svd3(a_in: [[f64; 3]; 3]) -> Svd3 {
    let mut a = a_in;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..60 {
        let mut off = 0.0;
        for i in 0..3 {
            for j in (i + 1)..3 {
                off += {
                    let mut s = 0.0;
                    for k in 0..3 {
                        s += a[k][i] * a[k][j];
                    }
                    s.abs()
                };
            }
        }
        if off < 1e-14 {
            break;
        }
        for p in 0..3 {
            for q in (p + 1)..3 {
                let mut alpha = 0.0;
                let mut beta = 0.0;
                let mut gamma = 0.0;
                for k in 0..3 {
                    alpha += a[k][p] * a[k][p];
                    beta += a[k][q] * a[k][q];
                    gamma += a[k][p] * a[k][q];
                }
                if gamma.abs() < 1e-15 {
                    continue;
                }
                let zeta = (beta - alpha) / (2.0 * gamma);
                let t = if zeta >= 0.0 {
                    1.0 / (zeta + (1.0 + zeta * zeta).sqrt())
                } else {
                    -1.0 / (-zeta + (1.0 + zeta * zeta).sqrt())
                };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = t * c;
                for k in 0..3 {
                    let ap = a[k][p];
                    let aq = a[k][q];
                    a[k][p] = c * ap - s * aq;
                    a[k][q] = s * ap + c * aq;
                    let vp = v[k][p];
                    let vq = v[k][q];
                    v[k][p] = c * vp - s * vq;
                    v[k][q] = s * vp + c * vq;
                }
            }
        }
    }
    let mut sigma = [norm_col(&a, 0), norm_col(&a, 1), norm_col(&a, 2)];
    let mut u = [[0.0f64; 3]; 3];
    for j in 0..3 {
        if sigma[j] > 1e-300 {
            for i in 0..3 {
                u[i][j] = a[i][j] / sigma[j];
            }
        }
    }
    // 接近 0 的列：补出正交向量（3 维场景固定处理）。
    for j in 0..3 {
        if sigma[j] <= 1e-300 {
            sigma[j] = 0.0;
            let mut cand = [0.0f64; 3];
            'outer: for axis in 0..3 {
                let mut e = [0.0; 3];
                e[axis] = 1.0;
                for k in 0..3 {
                    if sigma[k] > 1e-300 {
                        let mut dot = 0.0;
                        for r in 0..3 {
                            dot += u[r][k] * e[r];
                        }
                        for r in 0..3 {
                            e[r] -= dot * u[r][k];
                        }
                    }
                }
                let n = (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt();
                if n > 1e-9 {
                    for r in 0..3 {
                        cand[r] = e[r] / n;
                    }
                    break 'outer;
                }
            }
            for r in 0..3 {
                u[r][j] = cand[r];
            }
        }
    }
    // V^T
    let mut vt = [[0.0f64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            vt[i][j] = v[j][i];
        }
    }
    Svd3 { u, sigma, vt }
}

#[derive(Debug, Clone)]
pub struct CompDiagnosis {
    pub rcond: f64,
    pub sigma_min: f64,
    pub sigma_max: f64,
    pub near_singular: bool,
    pub threshold: f64,
}

pub fn diagnose(s: &[Vec<f64>]) -> CompDiagnosis {
    let m = to3(s);
    let svd = svd3(m);
    let smin = svd.sigma.iter().cloned().fold(f64::INFINITY, f64::min);
    let smax = svd.sigma.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let rcond = if smax > 0.0 { smin / smax } else { 0.0 };
    CompDiagnosis {
        rcond,
        sigma_min: smin,
        sigma_max: smax,
        near_singular: rcond < NEAR_SINGULAR_RCOND_THRESHOLD,
        threshold: NEAR_SINGULAR_RCOND_THRESHOLD,
    }
}

fn to3(s: &[Vec<f64>]) -> [[f64; 3]; 3] {
    let mut m = [[0.0f64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = s[i][j];
        }
    }
    m
}

/// 直接逆 S^-1。仅在良态时合法；近奇异时仍可计算用于对照，但必须标记 unstable。
pub fn direct_inverse(s: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let m = to3(s);
    let inv = inv3(m);
    inv.iter().map(|r| r.to_vec()).collect()
}

fn inv3(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut id = [[0.0f64; 3]; 3];
    for i in 0..3 {
        id[i][i] = 1.0;
    }
    let mut a = m;
    let mut b = id;
    for col in 0..3 {
        let mut pivot = col;
        for r in (col + 1)..3 {
            if a[r][col].abs() > a[pivot][col].abs() {
                pivot = r;
            }
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let piv = a[col][col];
        if piv.abs() < 1e-300 {
            // 真正奇异：返回 NaN 矩阵，调用方必须走候选路径，不能默默使用。
            return [[f64::NAN; 3]; 3];
        }
        for j in 0..3 {
            a[col][j] /= piv;
            b[col][j] /= piv;
        }
        for r in 0..3 {
            if r == col {
                continue;
            }
            let f = a[r][col];
            for j in 0..3 {
                a[r][j] -= f * a[col][j];
                b[r][j] -= f * b[col][j];
            }
        }
    }
    b
}

/// Tikhonov/ridge 正则化解：(S^T S + lambda^2 I)^-1 S^T，单位为 inverse_spillover。
pub fn ridge_inverse(s: &[Vec<f64>], lambda: f64) -> Vec<Vec<f64>> {
    let n = s.len();
    let mut ata = vec![vec![0.0f64; n]; n];
    let mut at = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            at[j][i] = s[i][j];
        }
    }
    for i in 0..n {
        for j in 0..n {
            let mut acc = 0.0;
            for k in 0..n {
                acc += at[i][k] * s[k][j];
            }
            ata[i][j] = acc;
        }
        ata[i][i] += lambda * lambda;
    }
    let inv = inv_n(&ata);
    let mut out = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut acc = 0.0;
            for k in 0..n {
                acc += inv[i][k] * at[k][j];
            }
            out[i][j] = acc;
        }
    }
    out
}

fn inv_n(a_in: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a_in.len();
    let mut a = a_in.to_vec();
    let mut b = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        b[i][i] = 1.0;
    }
    for col in 0..n {
        let mut pivot = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[pivot][col].abs() {
                pivot = r;
            }
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let piv = a[col][col];
        for j in 0..n {
            a[col][j] /= piv;
            b[col][j] /= piv;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let f = a[r][col];
            for j in 0..n {
                a[r][j] -= f * a[col][j];
                b[r][j] -= f * b[col][j];
            }
        }
    }
    b
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct InverseCandidate {
    pub candidate_id: String,
    pub method: String,
    pub lambda: Option<f64>,
    pub matrix: Vec<Vec<f64>>,
    pub adoptable: bool,
    pub reason: String,
    pub rule: String,
}

/// 生成全部合法候选。近奇异时直接逆不可采纳；良态时直接逆是唯一候选。
pub fn inverse_candidates(s: &[Vec<f64>]) -> (CompDiagnosis, Vec<InverseCandidate>) {
    let d = diagnose(s);
    let mut out = Vec::new();
    let direct = direct_inverse(s);
    let direct_ok = !d.near_singular && direct.iter().flatten().all(|v| v.is_finite());
    out.push(InverseCandidate {
        candidate_id: "INV.direct".into(),
        method: "direct_inverse".into(),
        lambda: None,
        matrix: direct,
        adoptable: direct_ok,
        reason: if d.near_singular {
            format!("rcond={:.3e} < {}，直接逆放大噪声，禁止采纳", d.rcond, NEAR_SINGULAR_RCOND_THRESHOLD)
        } else {
            "良态矩阵，直接逆可用".into()
        },
        rule: "RULE.NS-01".into(),
    });
    if d.near_singular {
        for (id, lambda) in [("INV.ridge-small", 1e-3f64), ("INV.ridge-large", 1e-1f64)] {
            out.push(InverseCandidate {
                candidate_id: id.into(),
                method: "tikhonov_ridge".into(),
                lambda: Some(lambda),
                matrix: ridge_inverse(s, lambda),
                adoptable: true,
                reason: format!("Tikhonov 正则化 lambda={lambda}；两个合法候选都保留，须显式裁定"),
                rule: "RULE.NS-01".into(),
            });
        }
    }
    (d, out)
}

/// 用补偿矩阵对单个事件的 spillover 通道做线性解混：comp = M * raw。
pub fn apply_inverse(minv: &[Vec<f64>], raw: &[f64]) -> Vec<f64> {
    let n = minv.len();
    let mut out = vec![0.0f64; n];
    for i in 0..n {
        let mut acc = 0.0;
        for j in 0..n {
            acc += minv[i][j] * raw[j];
        }
        out[i] = acc;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool { (a - b).abs() < tol }

    #[test]
    fn svd_reconstructs_well_conditioned() {
        let m = [[1.0, 0.05, 0.0], [0.02, 1.0, 0.04], [0.0, 0.03, 1.0]];
        let svd = svd3(m);
        for i in 0..3 {
            for j in 0..3 {
                let mut v = 0.0;
                for k in 0..3 {
                    v += svd.u[i][k] * svd.sigma[k] * svd.vt[k][j];
                }
                assert!(close(v, m[i][j], 1e-9), "{i},{j}: {v} vs {}", m[i][j]);
            }
        }
    }

    #[test]
    fn well_conditioned_has_single_adoptable_direct() {
        let s = vec![
            vec![1.0, 0.05, 0.0],
            vec![0.02, 1.0, 0.04],
            vec![0.0, 0.03, 1.0],
        ];
        let (d, cands) = inverse_candidates(&s);
        assert!(!d.near_singular);
        assert_eq!(cands.len(), 1);
        assert!(cands[0].adoptable);
    }

    #[test]
    fn near_singular_rejects_direct_and_keeps_two_ridges() {
        // 两列几乎共线（PE 与 APC 的泄漏未分开）。
        let e = 1e-6;
        let s = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 1.0 - e],
            vec![0.0, 1.0 - e, 1.0],
        ];
        let (d, cands) = inverse_candidates(&s);
        assert!(d.near_singular, "rcond={}", d.rcond);
        assert!(d.rcond > 1e-8, "rcond should be near-singular not exact zero: {}", d.rcond);
        let direct = &cands[0];
        assert!(!direct.adoptable, "近奇异直接逆不可被采纳");
        let ridges: Vec<_> = cands.iter().filter(|c| c.method == "tikhonov_ridge").collect();
        assert_eq!(ridges.len(), 2, "必须保留全部两个合法候选");
        assert!(ridges.iter().all(|c| c.adoptable));
    }

    #[test]
    fn ridge_stabilizes_amplification() {
        let e = 1e-6;
        let s = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 1.0 - e],
            vec![0.0, 1.0 - e, 1.0],
        ];
        let raw = vec![100.0, 5000.0, 4999.0];
        let direct = direct_inverse(&s);
        let dv = apply_inverse(&direct, &raw);
        let ridge = ridge_inverse(&s, 1e-1);
        let rv = apply_inverse(&ridge, &raw);
        let amp_direct = dv.iter().map(|v| v.abs()).fold(0.0, f64::max);
        let amp_ridge = rv.iter().map(|v| v.abs()).fold(0.0, f64::max);
        assert!(amp_direct > 1e5, "直接逆应出现十万级放大，实际 {amp_direct}");
        assert!(amp_ridge < amp_direct / 100.0);
    }
}
