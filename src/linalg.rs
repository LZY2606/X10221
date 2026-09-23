//! 近奇异补偿矩阵检测与求逆。
//!
//! 规则 RULE-COMP-001：接近奇异的补偿矩阵不能被默默求逆。
//! 这里用带部分主元的 LU 分解估算 1-范数条件数倒数 `rcond = 1/cond1`，
//! 只在 rcond 足够大（数值稳定）时才给出"精确"逆；否则保留多个
//! Tikhonov 正则化候选，交人工裁定，绝不静默替换。

/// 近奇异阈值：rcond 低于该值视为近奇异，需要显式候选与裁定。
pub const NEAR_SINGULAR_RCOND: f64 = 1e-4;

#[derive(Debug, Clone)]
pub struct InversionReport {
    pub n: usize,
    pub rcond: f64,
    pub near_singular: bool,
    pub exactly_singular: bool,
    /// 精确逆；仅当 !near_singular && !exactly_singular 时为 Some。
    pub exact_inverse: Option<Vec<f64>>,
    /// 合法候选（含被采纳与未被采纳的全部候选）。
    pub candidates: Vec<InversionCandidate>,
}

#[derive(Debug, Clone)]
pub struct InversionCandidate {
    /// 稳定身份，如 `cand:exact`、`cand:tikhonov:1e-8`。
    pub id: String,
    pub kind: CandidateKind,
    pub regularization: f64,
    /// n*n 行优先矩阵。
    pub matrix: Vec<f64>,
    /// 相对残差 ||M*c - I|| / n（无量纲）。
    pub residual: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Exact,
    Regularized,
}

/// LU 分解结果（带部分主元）。
struct Lu {
    lu: Vec<f64>,
    piv: Vec<usize>,
    n: usize,
    singular: bool,
}

fn lu_decompose(a: &[f64], n: usize) -> Lu {
    let mut lu = a.to_vec();
    let mut piv: Vec<usize> = (0..n).collect();
    let mut singular = false;
    for k in 0..n {
        let mut max_row = k;
        let mut max_val = lu[k * n + k].abs();
        for i in (k + 1)..n {
            let v = lu[i * n + k].abs();
            if v > max_val {
                max_val = v;
                max_row = i;
            }
        }
        if max_val < 1e-300 {
            singular = true;
            continue;
        }
        if max_row != k {
            piv.swap(k, max_row);
            for j in 0..n {
                lu.swap(k * n + j, max_row * n + j);
            }
        }
        let pivot = lu[k * n + k];
        for i in (k + 1)..n {
            let factor = lu[i * n + k] / pivot;
            lu[i * n + k] = factor;
            for j in (k + 1)..n {
                lu[i * n + j] -= factor * lu[k * n + j];
            }
        }
    }
    Lu { lu, piv, n, singular }
}

fn lu_solve(lu: &Lu, b: &[f64]) -> Option<Vec<f64>> {
    let n = lu.n;
    if lu.singular {
        return None;
    }
    let mut x = vec![0.0; n];
    // 前向代入（含主元置换）
    for i in 0..n {
        let mut s = b[lu.piv[i]];
        for j in 0..i {
            s -= lu.lu[i * n + j] * x[j];
        }
        x[i] = s;
    }
    // 后向代入
    for i in (0..n).rev() {
        for j in (i + 1)..n {
            x[i] -= lu.lu[i * n + j] * x[j];
        }
        let pivot = lu.lu[i * n + i];
        if pivot.abs() < 1e-300 {
            return None;
        }
        x[i] /= pivot;
    }
    Some(x)
}

fn mat_vec(a: &[f64], n: usize, x: &[f64]) -> Vec<f64> {
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = 0.0;
        for j in 0..n {
            s += a[i * n + j] * x[j];
        }
        y[i] = s;
    }
    y
}

fn mat_mul(a: &[f64], b: &[f64], n: usize) -> Vec<f64> {
    let mut c = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += a[i * n + k] * b[k * n + j];
            }
            c[i * n + j] = s;
        }
    }
    c
}

fn norm1(a: &[f64], n: usize) -> f64 {
    let mut m = 0.0;
    for j in 0..n {
        let mut s = 0.0;
        for i in 0..n {
            s += a[i * n + j].abs();
        }
        m = m.max(s);
    }
    m
}

fn identity(n: usize) -> Vec<f64> {
    let mut m = vec![0.0; n * n];
    for i in 0..n {
        m[i * n + i] = 1.0;
    }
    m
}

/// 估算 rcond = 1/(||A||1 * ||A^-1||1)；奇异时返回 0。
pub fn reciprocal_condition(a: &[f64], n: usize) -> f64 {
    let a_norm = norm1(a, n);
    if a_norm == 0.0 {
        return 0.0;
    }
    let lu = lu_decompose(a, n);
    if lu.singular {
        return 0.0;
    }
    // 逐列求逆，得到 A^-1 的 1-范数
    let mut ainv_norm = 0.0;
    for col in 0..n {
        let mut e = vec![0.0; n];
        e[col] = 1.0;
        match lu_solve(&lu, &e) {
            Some(x) => {
                let s: f64 = x.iter().map(|v| v.abs()).sum();
                ainv_norm = ainv_norm.max(s);
            }
            None => return 0.0,
        }
    }
    if ainv_norm == 0.0 || ainv_norm.is_infinite() {
        return 0.0;
    }
    (1.0 / (a_norm * ainv_norm)).min(1.0)
}

/// Tikhonov 正则化逆：(A^T A + λI)^-1 A^T
fn regularized_inverse(a: &[f64], n: usize, lambda: f64) -> Vec<f64> {
    let mut ata = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += a[k * n + i] * a[k * n + j];
            }
            ata[i * n + j] = s;
        }
    }
    for i in 0..n {
        ata[i * n + i] += lambda;
    }
    let lu = lu_decompose(&ata, n);
    // 解 (AtA + λI) X = At，逐列
    let mut x = vec![0.0; n * n];
    for col in 0..n {
        let mut at_col = vec![0.0; n];
        for row in 0..n {
            at_col[row] = a[col * n + row];
        }
        let sol = lu_solve(&lu, &at_col).unwrap_or_else(|| vec![f64::NAN; n]);
        for row in 0..n {
            x[row * n + col] = sol[row];
        }
    }
    x
}

fn relative_residual(a: &[f64], cand: &[f64], n: usize) -> f64 {
    let prod = mat_mul(a, cand, n);
    let id = identity(n);
    let mut err = 0.0;
    for i in 0..n * n {
        err += (prod[i] - id[i]).abs();
    }
    err / n as f64
}

/// 补偿矩阵分析入口：输出 rcond、精确逆（若安全）与全部正则化候选。
pub fn analyze_compensation(a: &[f64], n: usize) -> InversionReport {
    assert_eq!(a.len(), n * n, "补偿矩阵维度不匹配");
    let rcond = reciprocal_condition(a, n);
    let lu = lu_decompose(a, n);
    let exactly_singular = lu.singular;
    let near_singular = !exactly_singular && rcond < NEAR_SINGULAR_RCOND;

    let mut candidates = Vec::new();

    if !exactly_singular && !near_singular {
        let mut inv = Vec::with_capacity(n * n);
        for col in 0..n {
            let mut e = vec![0.0; n];
            e[col] = 1.0;
            let x = lu_solve(&lu, &e).expect("非奇异矩阵应当可解");
            inv.extend_from_slice(&x);
        }
        // lu_solve 按列拼出的是 A^-1（列主拼接 == 行优先恒等）
        let residual = relative_residual(a, &inv, n);
        candidates.push(InversionCandidate {
            id: "cand:exact".to_string(),
            kind: CandidateKind::Exact,
            regularization: 0.0,
            matrix: inv,
            residual,
        });
    } else {
        // 奇异 / 近奇异：不静默求逆，保留多个 λ 候选供人工裁定
        const LAMBDAS: [f64; 4] = [1e-8, 1e-6, 1e-4, 1e-2];
        for &lambda in &LAMBDAS {
            let m = regularized_inverse(a, n, lambda);
            let residual = relative_residual(a, &m, n);
            if m.iter().all(|v| v.is_finite()) {
                candidates.push(InversionCandidate {
                    id: format!("cand:tikhonov:{lambda}"),
                    kind: CandidateKind::Regularized,
                    regularization: lambda,
                    matrix: m,
                    residual,
                });
            }
        }
    }

    InversionReport {
        n,
        rcond,
        near_singular,
        exactly_singular,
        exact_inverse: if candidates.len() == 1 && candidates[0].kind == CandidateKind::Exact {
            Some(candidates[0].matrix.clone())
        } else {
            None
        },
        candidates,
    }
}

/// 用候选逆对一批信号做补偿：y = M^-1 x（逐事件、逐通道）。
/// 返回 None 表示该值无法参与补偿（保留原值 + 诊断）。
pub fn apply_inverse(inv: &[f64], n: usize, x: &[f64]) -> Vec<Option<f64>> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        if x.iter().any(|v| !v.is_finite()) {
            out.push(None);
            continue;
        }
        let s = mat_vec(inv, n, x)[i];
        if s.is_finite() {
            out.push(Some(s));
        } else {
            out.push(None);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_gets_exact_inverse() {
        // 对角占优、条件良好
        let a = vec![1.0, 0.02, 0.0, 0.03, 1.0, 0.01, 0.0, 0.02, 1.0];
        let r = analyze_compensation(&a, 3);
        assert!(!r.near_singular && !r.exactly_singular);
        assert!(r.exact_inverse.is_some());
        assert_eq!(r.candidates.len(), 1);
        assert_eq!(r.candidates[0].id, "cand:exact");
        assert!(r.candidates[0].residual < 1e-10);
    }

    #[test]
    fn near_singular_keeps_all_candidates_no_silent_inverse() {
        // 两行几乎相同 -> 近奇异
        let a = vec![1.0, 1.0, 0.0, 1.0, 1.0 + 1e-7, 0.0, 0.0, 0.0, 1.0];
        let r = analyze_compensation(&a, 3);
        assert!(r.rcond < NEAR_SINGULAR_RCOND);
        assert!(r.exact_inverse.is_none(), "近奇异矩阵不得静默给出精确逆");
        assert!(r.candidates.len() >= 3, "必须保留多个合法候选");
        assert!(r.candidates.iter().all(|c| c.kind == CandidateKind::Regularized));
    }

    #[test]
    fn exactly_singular_has_no_exact_candidate() {
        let a = vec![1.0, 2.0, 3.0, 2.0, 4.0, 6.0, 1.0, 0.0, 1.0];
        let r = analyze_compensation(&a, 3);
        assert!(r.exactly_singular);
        assert_eq!(r.rcond, 0.0);
        assert!(r.exact_inverse.is_none());
        assert!(r.candidates.iter().all(|c| c.kind == CandidateKind::Regularized));
    }
}
