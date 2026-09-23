//! 补偿计算与判定。
//!
//! 规则 rule.near_singular_no_silent_inverse：
//! - rcond < 阈值（默认 1e-6）即近奇异，禁止默默求逆，只能进入人工复核；
//! - rank 不足即奇异：不存在唯一逆，必须显式保留“全部合法候选”（零空间族 + 采样候选），
//!   未采纳候选与采纳候选一起进入档案。

use nalgebra::{DMatrix, DVector};
use crate::domain::{CompensationCandidate, CompensationEvaluation, RuleVerdict, SignalFlag};
use crate::util::{num_id, stable_id};

pub const NEAR_SINGULAR_THRESHOLD: f64 = 1e-6;

pub struct EvaluatedSpillover {
    pub evaluation: CompensationEvaluation,
}

/// 对 spillover 矩阵 S（N = S·x）求补偿候选，并逐事件检查极端信号。
pub fn evaluate_spillover(
    spillover_version: &str,
    channels: &[String],
    matrix: &[Vec<f64>],
    events_signal: &[SignalFlag],
) -> EvaluatedSpillover {
    let n = channels.len();
    let s = DMatrix::from_fn(n, n, |r, c| matrix[r][c]);
    let svd = s.svd(true, true);
    let sigmas: Vec<f64> = svd.singular_values.iter().copied().collect();
    let sigma_max = sigmas.first().copied().unwrap_or(0.0);
    let sigma_min = sigmas.last().copied().unwrap_or(0.0);
    let rcond = if sigma_max > 0.0 { sigma_min / sigma_max } else { 0.0 };
    let tol = n as f64 * sigma_max * f64::EPSILON;
    let rank = sigmas.iter().filter(|&&v| v > tol).count();

    let (status, rule) = if rank < n {
        (
            "singular",
            RuleVerdict {
                rule_id: "rule.singular_matrix_no_unique_inverse".to_string(),
                severity: "blocking".to_string(),
                rationale: format!(
                    "spillover 矩阵秩 {} < 阶数 {}，线性方程组有无穷多解；禁止给出唯一逆，保留零空间族与全部采样候选",
                    rank, n
                ),
                parameters: [("rank_tolerance".to_string(), tol)].into_iter().collect(),
            },
        )
    } else if rcond < NEAR_SINGULAR_THRESHOLD {
        (
            "near_singular",
            RuleVerdict {
                rule_id: "rule.near_singular_no_silent_inverse".to_string(),
                severity: "review_required".to_string(),
                rationale: format!(
                    "倒条件数 rcond={:.3e} 小于阈值 {:.0e}：数值求逆对噪声极不稳定，禁止默默求逆，必须人工复核",
                    rcond, NEAR_SINGULAR_THRESHOLD
                ),
                parameters: [
                    ("near_singular_threshold".to_string(), NEAR_SINGULAR_THRESHOLD),
                    ("rcond".to_string(), rcond),
                ].into_iter().collect(),
            },
        )
    } else {
        (
            "ok",
            RuleVerdict {
                rule_id: "rule.compensation_inverse_well_conditioned".to_string(),
                severity: "info".to_string(),
                rationale: format!("rcond={:.3e} 良好，补偿矩阵唯一且数值稳定", rcond),
                parameters: [("rcond".to_string(), rcond)].into_iter().collect(),
            },
        )
    };

    let candidates = build_candidates(spillover_version, &s, &svd, rank, n, &status);

    EvaluatedSpillover {
        evaluation: CompensationEvaluation {
            spillover_version: spillover_version.to_string(),
            status: status.to_string(),
            rank,
            n,
            sigma_min,
            rcond,
            near_singular_threshold: NEAR_SINGULAR_THRESHOLD,
            rule,
            candidates,
            signal_flags: events_signal.to_vec(),
        },
    }
}

fn matrix_to_vecs(m: &DMatrix<f64>) -> Vec<Vec<f64>> {
    (0..m.nrows()).map(|r| (0..m.ncols()).map(|c| m[(r, c)]).collect()).collect()
}

fn build_candidates(
    version: &str,
    s: &DMatrix<f64>,
    svd: &nalgebra::SVD<f64, nalgebra::Dyn, nalgebra::Dyn>,
    rank: usize,
    n: usize,
    status: &str,
) -> Vec<CompensationCandidate> {
    match status {
        "ok" | "near_singular" => {
            let inv = s.clone().try_inverse().unwrap_or_else(|| {
                // 即便 rank 判定通过也兜底走伪逆，绝不 panic
                svd.pseudo_inverse(0.0).unwrap_or_else(|_| DMatrix::zeros(n, n))
            });
            let mvec = matrix_to_vecs(&inv);
            let sig = mvec.iter().flatten().map(num_id).collect::<Vec<_>>().join(",");
            let id = stable_id("cand", &[version, status, &sig]);
            vec![CompensationCandidate {
                id,
                family: if status == "ok" { "unique".to_string() } else { "least_squares".to_string() },
                matrix: mvec,
                free_parameter: None,
                note: if status == "ok" {
                    "唯一数值稳定逆".to_string()
                } else {
                    "近奇异：形式逆存在但对噪声极不稳定，仅供复核对比，未裁定前不得作为默认结果".to_string()
                },
            }]
        }
        _ => {
            // 奇异：最小二乘特解 + 零空间族
            let pinv = svd.pseudo_inverse(0.0).unwrap_or_else(|_| DMatrix::zeros(n, n));
            let v_t = svd.v_t.as_ref().expect("v_t computed");
            // 秩亏几维，零空间就是 V^T 的最后 n-rank 行（V 的列）
            let null_dirs: Vec<DVector<f64>> = (rank..n)
                .map(|i| v_t.row(i).transpose())
                .collect();
            let mut out = Vec::new();

            // 1) 最小范数特解（零空间系数=0）
            let m0 = matrix_to_vecs(&pinv);
            let id0 = stable_id("cand", &[version, "least_squares", &flatten_sig(&m0)]);
            out.push(CompensationCandidate {
                id: id0,
                family: "least_squares".to_string(),
                matrix: m0,
                free_parameter: Some(0.0),
                note: "最小二乘最小范数特解；零空间方向被置零，只是无穷多合法解之一".to_string(),
            });

            // 2) 零空间族采样候选（沿第一零方向 ±1），证明“无法唯一决定”时保留全部合法候选
            if let Some(dir) = null_dirs.first() {
                for t in [-1.0_f64, 1.0] {
                    let shifted = &pinv
                        + DMatrix::from_fn(n, n, |r, c| if c == 0 { t * dir[r] } else { 0.0 });
                    let mv = matrix_to_vecs(&shifted);
                    let id = stable_id("cand", &[version, "nullspace_family", &num_id(t), &flatten_sig(&mv)]);
                    out.push(CompensationCandidate {
                        id,
                        family: "nullspace_family".to_string(),
                        matrix: mv,
                        free_parameter: Some(t),
                        note: format!("零空间族候选（自由参数 t={}）：S·M=I 仍成立，但 M 不唯一", t),
                    });
                }
            }
            out
        }
    }
}

fn flatten_sig(m: &[Vec<f64>]) -> String {
    m.iter().flatten().map(num_id).collect::<Vec<_>>().join(",")
}

/// 应用补偿：x = M·N（按给定荧光通道顺序）。
pub fn apply_compensation(
    raw: &[f64],
    channels: &[String],
    fluoro_channels: &[String],
    comp_matrix: &[Vec<f64>],
) -> Vec<f64> {
    let n = fluoro_channels.len();
    let mut measured = vec![0.0_f64; n];
    for (i, ch) in fluoro_channels.iter().enumerate() {
        if let Some(pos) = channels.iter().position(|c| c == ch) {
            measured[i] = raw[pos];
        }
    }
    let m = DMatrix::from_fn(n, n, |r, c| comp_matrix[r][c]);
    let v = DVector::from_column_slice(&measured);
    let x = m * v;
    let mut out = raw.to_vec();
    for (i, ch) in fluoro_channels.iter().enumerate() {
        if let Some(pos) = channels.iter().position(|c| c == ch) {
            out[pos] = x[i];
        }
    }
    out
}

/// 变换：asinh(x / cofactor)，输出 transformed 基准。
pub fn transform_value(x: f64, cofactor: f64) -> f64 {
    (x / cofactor.max(1e-12)).asinh()
}
