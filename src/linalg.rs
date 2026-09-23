//! 补偿矩阵求解：部分主元高斯消元、近奇异判定与岭逆候选。
//! 规则 POLICY-COMP-001：近奇异补偿矩阵不能被默默求逆——必须返回机器可读判定，
//! 由审阅者显式裁定后才能产生受补偿的下游结果。

use serde::{Deserialize, Serialize};

/// 最小主元 / 最大主元 低于该阈值即判定近奇异（无量纲）
pub const NEAR_SINGULAR_THRESHOLD: f64 = 1e-6;
/// 完全奇异（存在零主元）阈值
pub const SINGULAR_THRESHOLD: f64 = 1e-12;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum SolveFinding {
    /// 可直接求逆
    Invertible {
        reciprocal_estimate: f64,
        min_pivot: f64,
        max_pivot: f64,
    },
    /// 近奇异：禁止默默求逆，给出全部合法候选
    NearSingular {
        reciprocal_estimate: f64,
        min_pivot: f64,
        max_pivot: f64,
        rule: String,
    },
    /// 结构奇异
    Singular {
        zero_pivot_index: usize,
        rule: String,
    },
}

impl SolveFinding {
    pub fn code(&self) -> &str {
        match self {
            SolveFinding::Invertible { .. } => "invertible",
            SolveFinding::NearSingular { .. } => "near_singular",
            SolveFinding::Singular { .. } => "singular",
        }
    }
}

pub fn is_square(a: &[Vec<f64>]) -> bool {
    let n = a.len();
    n > 0 && a.iter().all(|r| r.len() == n)
}

/// 带部分主元的高斯消元，返回主元绝对值序列与行列式。
pub fn pivots(mut a: Vec<Vec<f64>>) -> Option<(Vec<f64>, f64)> {
    let n = a.len();
    if !is_square(&a) {
        return None;
    }
    let mut piv = Vec::with_capacity(n);
    let mut det = 1.0_f64;
    for k in 0..n {
        let mut pivot_row = k;
        let mut best = a[k][k].abs();
        for i in (k + 1)..n {
            if a[i][k].abs() > best {
                best = a[i][k].abs();
                pivot_row = i;
            }
        }
        if pivot_row != k {
            a.swap(k, pivot_row);
            det = -det;
        }
        let p = a[k][k];
        piv.push(p.abs());
        if p.abs() < SINGULAR_THRESHOLD {
            det = 0.0;
            return Some((piv, det));
        }
        det *= p;
        for i in (k + 1)..n {
            let factor = a[i][k] / p;
            for j in k..n {
                let v = a[k][j] * factor;
                a[i][j] -= v;
            }
        }
    }
    Some((piv, det))
}

/// 对矩阵给出机器可读判定。
pub fn classify(a: &[Vec<f64>]) -> SolveFinding {
    let Some((piv, det)) = pivots(a.to_vec()) else {
        return SolveFinding::Singular {
            zero_pivot_index: 0,
            rule: "POLICY-COMP-001".into(),
        };
    };
    let n = piv.len();
    if let Some(idx) = piv.iter().position(|p| *p < SINGULAR_THRESHOLD) {
        let _ = det;
        return SolveFinding::Singular {
            zero_pivot_index: idx,
            rule: "POLICY-COMP-001".into(),
        };
    }
    let min_pivot = piv.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_pivot = piv.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let reciprocal_estimate = min_pivot / max_pivot;
    if reciprocal_estimate < NEAR_SINGULAR_THRESHOLD {
        SolveFinding::NearSingular {
            reciprocal_estimate,
            min_pivot,
            max_pivot,
            rule: "POLICY-COMP-001".to_string(),
        }
    } else {
        SolveFinding::Invertible {
            reciprocal_estimate,
            min_pivot,
            max_pivot,
        }
    }
}

/// 仅当判定为可逆时才计算逆矩阵；否则拒绝（不默默求逆）。
pub fn inverse(a: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, SolveFinding> {
    match classify(a) {
        SolveFinding::Invertible { .. } => {}
        other => return Err(other),
    }
    let n = a.len();
    let mut m = vec![vec![0.0_f64; 2 * n]; n];
    for i in 0..n {
        m[i][..n].copy_from_slice(&a[i]);
        m[i][n + i] = 1.0;
    }
    for k in 0..n {
        let mut pivot_row = k;
        let mut best = m[k][k].abs();
        for i in (k + 1)..n {
            if m[i][k].abs() > best {
                best = m[i][k].abs();
                pivot_row = i;
            }
        }
        m.swap(k, pivot_row);
        let p = m[k][k];
        for j in 0..2 * n {
            m[k][j] /= p;
        }
        for i in 0..n {
            if i == k {
                continue;
            }
            let factor = m[i][k];
            for j in 0..2 * n {
                let v = m[k][j] * factor;
                m[i][j] -= v;
            }
        }
    }
    Ok(m.into_iter().map(|r| r[n..].to_vec()).collect())
}

pub fn transpose(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let m = a.first().map(|r| r.len()).unwrap_or(0);
    let mut t = vec![vec![0.0; n]; m];
    for i in 0..n {
        for j in 0..m {
            t[j][i] = a[i][j];
        }
    }
    t
}

pub fn mat_mul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let k = b.len();
    let m = b.first().map(|r| r.len()).unwrap_or(0);
    let mut out = vec![vec![0.0; m]; n];
    for i in 0..n {
        for j in 0..m {
            let mut s = 0.0;
            for t in 0..k {
                s += a[i][t] * b[t][j];
            }
            out[i][j] = s;
        }
    }
    out
}

pub fn mat_vec(a: &[Vec<f64>], x: &[f64]) -> Vec<f64> {
    a.iter()
        .map(|row| row.iter().zip(x).map(|(a, b)| a * b).sum())
        .collect()
}

/// 岭逆候选：(AᵀA + λI)⁻¹ Aᵀ。仅在近奇异/奇异时作为合法候选提供，必须显式裁定。
pub fn ridge_inverse(a: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let at = transpose(a);
    let ata = mat_mul(&at, a);
    let n = ata.len();
    let mut reg = ata;
    for i in 0..n {
        reg[i][i] += lambda;
    }
    inverse(&reg).ok().map(|inv| mat_mul(&inv, &at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_well_conditioned() {
        let a = vec![vec![1.0, 0.1], vec![0.2, 1.0]];
        let inv = inverse(&a).expect("well conditioned");
        let prod = mat_mul(&a, &inv);
        for i in 0..2 {
            for j in 0..2 {
                assert!((prod[i][j] - if i == j { 1.0 } else { 0.0 }).abs() < 1e-9);
            }
        }
        assert_eq!(classify(&a).code(), "invertible");
    }

    #[test]
    fn rejects_exact_singular() {
        let a = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        assert!(matches!(classify(&a), SolveFinding::Singular { .. }));
        assert!(inverse(&a).is_err());
    }

    #[test]
    fn rejects_near_singular_without_silent_inverse() {
        // 行几乎共线：条件数极大
        let a = vec![vec![1.0, 1.0], vec![1.0, 1.0000001]];
        match classify(&a) {
            SolveFinding::NearSingular {
                reciprocal_estimate,
                rule,
                ..
            } => {
                assert!(reciprocal_estimate < NEAR_SINGULAR_THRESHOLD);
                assert_eq!(rule, "POLICY-COMP-001");
            }
            other => panic!("expected near singular, got {other:?}"),
        }
        assert!(inverse(&a).is_err(), "must not silently invert");
        // 但岭逆仍是合法候选
        assert!(ridge_inverse(&a, 0.01).is_some());
    }
}
