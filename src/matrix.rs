//! 小型稠密线性代数：显式求逆、1-范数倒数条件数估计、近奇异守卫。
//!
//! 关键规则（rule::NEAR_SINGULAR_GUARD）：
//! 当 rcond <= RCOND_BLOCK_THRESHOLD 时，不允许把逆矩阵当作唯一答案默默写出；
//! 调用方必须保留“无法唯一决定”的全部合法候选并交人工裁定。

/// 带部分主元的高斯消元；返回 PA=LU 下的逆矩阵（行优先），奇异时返回 None。
pub fn inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut a = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n).map(|i| {
        let mut r = vec![0.0; n];
        r[i] = 1.0;
        r
    }).collect();

    for col in 0..n {
        let mut pivot = col;
        let mut best = a[col][col].abs();
        for r in (col + 1)..n {
            if a[r][col].abs() > best {
                best = a[r][col].abs();
                pivot = r;
            }
        }
        if best <= f64::EPSILON {
            return None;
        }
        if pivot != col {
            a.swap(pivot, col);
            inv.swap(pivot, col);
        }
        let piv = a[col][col];
        for k in 0..n {
            a[col][k] /= piv;
            inv[col][k] /= piv;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = a[r][col];
            if factor == 0.0 {
                continue;
            }
            for k in 0..n {
                a[r][k] -= factor * a[col][k];
                inv[r][k] -= factor * inv[col][k];
            }
        }
    }
    Some(inv)
}

pub fn one_norm(m: &[Vec<f64>]) -> f64 {
    if m.is_empty() {
        return 0.0;
    }
    let n = m.len();
    (0..n)
        .map(|c| (0..n).map(|r| m[r][c].abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

pub fn mat_vec(m: &[Vec<f64>], x: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(x).map(|(a, b)| a * b).sum())
        .collect()
}

/// 1-范数倒数条件数估计：rcond = 1/(||A||_1 * ||A^-1||_1)。
/// 奇异或不可逆时返回 0.0。
pub fn rcond(m: &[Vec<f64>]) -> f64 {
    let norm_a = one_norm(m);
    match inverse(m) {
        Some(inv) if norm_a > 0.0 => {
            let norm_inv = one_norm(&inv);
            if norm_inv.is_infinite() || norm_inv <= 0.0 {
                0.0
            } else {
                1.0 / (norm_a * norm_inv)
            }
        }
        _ => 0.0,
    }
}

/// (A + λI) 岭正则化的逆；近奇异时仍有定义，但结果依赖 λ，不是唯一答案。
pub fn ridge_inverse(m: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut reg = m.to_vec();
    for i in 0..n {
        reg[i][i] += lambda;
    }
    inverse(&reg)
}

/// 丢掉指定通道（行列）后的子矩阵（用于“放弃该荧光素维度”候选）。
pub fn drop_dimension(m: &[Vec<f64>], idx: usize) -> Vec<Vec<f64>> {
    let n = m.len();
    let keep: Vec<usize> = (0..n).filter(|&i| i != idx).collect();
    keep.iter()
        .map(|&r| keep.iter().map(|&c| m[r][c]).collect())
        .collect()
}

/// 补偿候选：近奇异时不得只保留一个。
#[derive(Debug, Clone, PartialEq)]
pub struct CompCandidate {
    pub kind: String,
    pub matrix: Vec<Vec<f64>>,
        pub rationale: String,
}

/// 评估 spillover 方阵。
/// * 可逆且 rcond 高于阈值：返回单个 `explicit_inverse` 候选；
/// * 近奇异/奇异：返回 Err，err 内带全部合法候选（岭回归、降维）与诊断，
///   由上层显式保存为多候选，不允许默默选一个。
pub fn evaluate_spillover(m: &[Vec<f64>]) -> Result<Vec<CompCandidate>, NearSingular> {
    let n = m.len();
    let rc = rcond(m);
    if rc > crate::domain::rule::RCOND_BLOCK_THRESHOLD {
        let inv = inverse(m).expect("rcond 非零意味着可逆");
        return Ok(vec![CompCandidate {
            kind: "explicit_inverse".into(),
            matrix: inv,
            rationale: format!("rcond={rc:.3e} 高于阈值，显式求逆唯一可用"),
        }]);
    }
    let mut candidates = Vec::new();
    let scale = one_norm(m).max(1.0);
    if let Some(reg) = ridge_inverse(m, 1e-6 * scale) {
        candidates.push(CompCandidate {
            kind: "ridge_regularized_inverse".into(),
            matrix: reg,
            rationale: "岭正则 λ=1e-6·||S||₁；对正则强度敏感，非唯一解".into(),
        });
    }
    for idx in 0..n {
        let sub = drop_dimension(m, idx);
        if rcond(&sub) > crate::domain::rule::RCOND_BLOCK_THRESHOLD {
            if let Some(inv) = inverse(&sub) {
                candidates.push(CompCandidate {
                    kind: format!("drop_dimension_{idx}"),
                    matrix: inv,
                    rationale: format!("放弃第 {idx} 个荧光素维度后条件数可接受"),
                });
            }
        }
    }
    Err(NearSingular {
        rcond: rc,
        threshold: crate::domain::rule::RCOND_BLOCK_THRESHOLD,
        candidates,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct NearSingular {
    pub rcond: f64,
    pub threshold: f64,
    pub candidates: Vec<CompCandidate>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_inverts_and_is_well_conditioned() {
        let id = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let inv = inverse(&id).unwrap();
        assert_eq!(inv, id);
        assert!(rcond(&id) > 0.9);
    }

    #[test]
    fn well_conditioned_spillover_has_single_explicit_candidate() {
        // cases 中健康 spillover 同形态矩阵
        let s = vec![
            vec![1.00, 0.12, 0.03],
            vec![0.08, 1.00, 0.15],
            vec![0.02, 0.05, 1.00],
        ];
        let out = evaluate_spillover(&s).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "explicit_inverse");
        // A * A^-1 ≈ I
        let inv = &out[0].matrix;
        for r in 0..3 {
            for c in 0..3 {
                let v: f64 = (0..3).map(|k| s[r][k] * inv[k][c]).sum();
                assert!((v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn near_singular_matrix_is_blocked_and_keeps_candidates() {
        // 两列光谱几乎共线（与案例包 v2 同形态）
        let s = vec![
            vec![1.0, 0.14, 0.020],
            vec![0.9, 1.00, 0.030],
            vec![0.8, 0.90, 1.000],
        ];
        let err = evaluate_spillover(&s).unwrap_err();
        assert!(err.rcond <= err.threshold);
        assert!(!err.candidates.is_empty(), "必须保留合法候选而非直接失败");
        assert!(err
            .candidates
            .iter()
            .any(|c| c.kind == "ridge_regularized_inverse"));
    }

    #[test]
    fn exactly_singular_has_zero_rcond() {
        let s = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        assert_eq!(rcond(&s), 0.0);
        let err = evaluate_spillover(&s).unwrap_err();
        assert!(err.rcond == 0.0);
    }
}
