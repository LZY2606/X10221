//! 补偿（spillover）矩阵：显式近奇异检测，禁止默默求逆。
//!
//! 判定：先做列均衡化（消除通道量级差异），再部分主元高斯消元。
//! - 最小主元 `== 0`：精确奇异；
//! - 最小主元 `< NEAR_SINGULAR_PIVOT`：近奇异，拒绝求逆；
//! - 条件数上界（最大主元/最小主元）`> CONDITION_LIMIT`：近奇异，拒绝。
//!
//! 无论接受或拒绝都返回机器可读的判定证据（判定码、指标）。

use serde::{Deserialize, Serialize};

pub const NEAR_SINGULAR_PIVOT: f64 = 1e-6;
pub const CONDITION_LIMIT: f64 = 1e6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMatrix {
    /// 行/列通道标签，顺序即矩阵坐标基准。
    pub channels: Vec<ChannelRef>,
    /// 行主序 n*n；measured = S · true。
    pub entries: Vec<f64>,
    /// 原始表示（导入时保留，例如 FCS $SPILL 字符串）。
    pub raw_repr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelRef {
    /// 通道/检测器名（例如 FL1-A）。
    pub name: String,
    /// 荧光/单位基准（例如 FITC, units: "fluorescence"）。
    pub fluorochrome: Option<String>,
    pub unit: String,
}

impl SpilloverMatrix {
    pub fn dim(&self) -> usize {
        self.channels.len()
    }

    pub fn row(&self, i: usize) -> &[f64] {
        &self.entries[i * self.dim()..(i + 1) * self.dim()]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatrixJudgment {
    pub accepted: bool,
    /// 机器可读判定码。
    pub code: String,
    /// 列均衡后的最小主元绝对值。
    pub min_pivot: f64,
    /// 主元比给出的条件数下界估计。
    pub condition_estimate: f64,
    /// 逆矩阵（行主序，仅 accepted 时存在）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inverse: Option<Vec<f64>>,
    pub reason: String,
}

/// 对矩阵尝试求逆；近奇异/奇异时拒绝并给出证据。
pub fn judge_inverse(m: &SpilloverMatrix) -> MatrixJudgment {
    let n = m.dim();
    if n == 0 || m.entries.len() != n * n {
        return MatrixJudgment {
            accepted: false,
            code: "matrix.malformed".to_string(),
            min_pivot: f64::NAN,
            condition_estimate: f64::NAN,
            inverse: None,
            reason: "矩阵为空或维度与条目数不一致".to_string(),
        };
    }

    // 列均衡化：E_jj = 1 / ||列_j||_1，A = S·E，S·x=b  =>  A·y=b, y=E^-1 x。
    let mut a = vec![0.0_f64; n * n];
    let mut scales = vec![1.0_f64; n];
    for j in 0..n {
        let norm: f64 = (0..n).map(|i| m.entries[i * n + j].abs()).sum();
        if norm == 0.0 {
            return MatrixJudgment {
                accepted: false,
                code: "matrix.zero_column".to_string(),
                min_pivot: 0.0,
                condition_estimate: f64::INFINITY,
                inverse: None,
                reason: format!("通道 {} 对应列全为零，矩阵不可逆", m.channels[j].name),
            };
        }
        scales[j] = norm;
        for i in 0..n {
            a[i * n + j] = m.entries[i * n + j] / norm;
        }
    }

    // 对增广 [A | I] 做部分主元高斯消元（在均衡化矩阵上）。
    let mut aug = vec![0.0_f64; n * 2 * n];
    for i in 0..n {
        for j in 0..n {
            aug[i * 2 * n + j] = a[i * n + j];
            aug[i * 2 * n + n + j] = if i == j { 1.0 } else { 0.0 };
        }
    }
    let mut pivots = Vec::with_capacity(n);
    for col in 0..n {
        // 选主元
        let mut pivot_row = col;
        let mut best = aug[col * 2 * n + col].abs();
        for r in (col + 1)..n {
            let v = aug[r * 2 * n + col].abs();
            if v > best {
                best = v;
                pivot_row = r;
            }
        }
        if best == 0.0 {
            return MatrixJudgment {
                accepted: false,
                code: "matrix.singular".to_string(),
                min_pivot: 0.0,
                condition_estimate: f64::INFINITY,
                inverse: None,
                reason: format!("第 {col} 列消元主元为 0，矩阵奇异"),
            };
        }
        if pivot_row != col {
            for k in 0..2 * n {
                aug.swap(col * 2 * n + k, pivot_row * 2 * n + k);
            }
        }
        pivots.push(best);
        let pivot = aug[col * 2 * n + col];
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = aug[r * 2 * n + col] / pivot;
            if factor == 0.0 {
                continue;
            }
            for k in col..2 * n {
                let v = aug[col * 2 * n + k] * factor;
                aug[r * 2 * n + k] -= v;
            }
        }
    }

    // 回代：归一化主元，取出 A^-1（均衡化矩阵的逆）。
    let mut inv_a = vec![0.0_f64; n * n];
    let mut min_pivot = f64::INFINITY;
    let mut max_pivot = 0.0_f64;
    for i in 0..n {
        let p = aug[i * 2 * n + i];
        let pa = p.abs();
        min_pivot = min_pivot.min(pa);
        max_pivot = max_pivot.max(pa);
        for j in 0..n {
            inv_a[i * n + j] = aug[i * 2 * n + n + j] / p;
        }
    }
    let condition_estimate = max_pivot / min_pivot;

    if !min_pivot.is_finite() || min_pivot < NEAR_SINGULAR_PIVOT {
        return MatrixJudgment {
            accepted: false,
            code: "matrix.near_singular_rejected".to_string(),
            min_pivot,
            condition_estimate,
            inverse: None,
            reason: format!(
                "均衡后最小主元 {min_pivot:.3e} 小于阈值 {NEAR_SINGULAR_PIVOT:.0e}，近奇异矩阵不被默默求逆"
            ),
        };
    }
    if condition_estimate > CONDITION_LIMIT {
        return MatrixJudgment {
            accepted: false,
            code: "matrix.near_singular_rejected".to_string(),
            min_pivot,
            condition_estimate,
            inverse: None,
            reason: format!(
                "条件数估计 {condition_estimate:.3e} 超过上限 {CONDITION_LIMIT:.0e}，补偿会被噪声放大，拒绝求逆"
            ),
        };
    }

    // S^-1 = E · A^-1：对 A^-1 的第 j 行乘以 scale_j（行 j 对应输出？）
    // A = S·E（列 j 除以 scale_j）=> A^-1 = E^-1·S^-1 = diag(scale)·S^-1
    // => S^-1 = diag(1/scale)·A^-1：第 i 行除以 scale_i。
    let mut inv_s = vec![0.0_f64; n * n];
    for i in 0..n {
        for j in 0..n {
            inv_s[i * n + j] = inv_a[i * n + j] / scales[i];
        }
    }

    MatrixJudgment {
        accepted: true,
        code: "matrix.inverted".to_string(),
        min_pivot,
        condition_estimate,
        inverse: Some(inv_s),
        reason: "矩阵通过近奇异检查并已求逆".to_string(),
    }
}

/// 用逆矩阵补偿一组信号：`true = S^-1 · measured`。
/// 非有限输入/溢出的坐标在原位置保留并附诊断，绝不丢弃事件。
pub fn compensate(inv: &[f64], measured: &[f64], n: usize) -> CompensationOutcome {
    let mut out = vec![0.0_f64; n];
    let mut diagnostics = Vec::new();
    for i in 0..n {
        if !measured.get(i).copied().unwrap_or(f64::NAN).is_finite() {
            out[i] = measured.get(i).copied().unwrap_or(f64::NAN);
            diagnostics.push(CoordDiagnostic {
                coord: i,
                code: "value.unconvertible".to_string(),
                detail: "输入信号非有限值（极端信号），保留原始值，不参与补偿数值计算".to_string(),
            });
            continue;
        }
        let mut acc = 0.0_f64;
        for j in 0..n {
            acc += inv[i * n + j] * measured[j];
        }
        if !acc.is_finite() {
            out[i] = measured[i];
            diagnostics.push(CoordDiagnostic {
                coord: i,
                code: "value.overflow".to_string(),
                detail: "补偿结果溢出，保留原始值并附诊断".to_string(),
            });
        } else {
            out[i] = acc;
        }
    }
    CompensationOutcome {
        values: out,
        diagnostics,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationOutcome {
    pub values: Vec<f64>,
    pub diagnostics: Vec<CoordDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoordDiagnostic {
    pub coord: usize,
    pub code: String,
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mat(entries: Vec<f64>, names: &[&str]) -> SpilloverMatrix {
        SpilloverMatrix {
            channels: names
                .iter()
                .map(|n| ChannelRef {
                    name: n.to_string(),
                    fluorochrome: None,
                    unit: "fluorescence".to_string(),
                })
                .collect(),
            entries,
            raw_repr: None,
        }
    }

    #[test]
    fn healthy_matrix_inverts_and_recovers_signal() {
        // measured = S·true
        let s = mat(vec![1.0, 0.1, 0.02, 1.0], &["FL1", "FL2"]);
        let j = judge_inverse(&s);
        assert!(j.accepted, "{}", j.reason);
        assert_eq!(j.code, "matrix.inverted");
        let inv = j.inverse.unwrap();
        let true_sig = vec![100.0, 50.0];
        let measured = vec![
            1.0 * true_sig[0] + 0.1 * true_sig[1],
            0.02 * true_sig[0] + 1.0 * true_sig[1],
        ];
        let out = compensate(&inv, &measured, 2);
        assert!((out.values[0] - true_sig[0]).abs() < 1e-9);
        assert!((out.values[1] - true_sig[1]).abs() < 1e-9);
        assert!(out.diagnostics.is_empty());
    }

    #[test]
    fn exactly_singular_is_rejected() {
        let s = mat(vec![1.0, 1.0, 1.0, 1.0], &["a", "b"]);
        let j = judge_inverse(&s);
        assert!(!j.accepted);
        assert_eq!(j.code, "matrix.singular");
    }

    #[test]
    fn near_singular_is_rejected_not_silently_inverted() {
        let eps = 1e-9;
        let s = mat(vec![1.0, 1.0, 1.0, 1.0 + eps], &["a", "b"]);
        let j = judge_inverse(&s);
        assert!(!j.accepted, "近奇异矩阵必须拒绝，而不是给出巨大数值的逆");
        assert_eq!(j.code, "matrix.near_singular_rejected");
        assert!(j.inverse.is_none());
        assert!(j.min_pivot < NEAR_SINGULAR_PIVOT);
    }

    #[test]
    fn scaled_channels_are_not_mistaken_for_singular() {
        // 两通道量级差 1e4，健康耦合不应因量级被判奇异。
        let s = mat(
            vec![1.0, 0.000_01, 100.0, 1.0],
            &["bright", "dim"],
        );
        let j = judge_inverse(&s);
        assert!(j.accepted, "{}", j.reason);
    }

    #[test]
    fn extreme_signals_are_kept_in_place_with_diagnostic() {
        let s = mat(vec![1.0, 0.1, 0.02, 1.0], &["FL1", "FL2"]);
        let inv = judge_inverse(&s).inverse.unwrap();
        let out = compensate(&inv, &[f64::INFINITY, 50.0], 2);
        assert_eq!(out.values[0], f64::INFINITY);
        assert!(out.diagnostics.iter().any(|d| d.code == "value.unconvertible"));
        // 事件没有被丢弃：仍有两个坐标。
        assert_eq!(out.values.len(), 2);
    }
}
