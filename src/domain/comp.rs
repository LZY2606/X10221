//! 补偿：spillover 元数据 -> 补偿矩阵工件 -> 补偿后事件。
//!
//! 不变量：
//! * 近奇异矩阵不会被默默求逆：[`matrix`] 为 None，候选全部保留，等待人工裁定。
//! * 非荧光通道（FSC/SSC）原样透传。

use super::matrix::{inverse, ridge_inverse, scaled_determinant, NEAR_SINGULAR_THRESHOLD};
use super::model::{CompCandidate, CompJudgement, CompMatrix, FcsDocument, SpilloverMeta};

/// 根据 spillover 元数据构造补偿矩阵工件（纯函数，不写库）。
pub fn build_comp_matrix(
    spillover: &SpilloverMeta,
    spillover_artifact: &str,
    spillover_version: u32,
) -> CompMatrix {
    let s = &spillover.spillover;
    let sd = scaled_determinant(s);
    let near = !sd.is_finite() || sd < NEAR_SINGULAR_THRESHOLD;
    if near {
        let lambdas = [1e-2, 1e-3, 1e-4];
        let mut candidates = Vec::new();
        let n = s.len();
        let identity: Vec<Vec<f64>> = (0..n)
            .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
            .collect();
        candidates.push(CompCandidate {
            id: "identity".into(),
            rationale: "不补偿：直接沿用原始信号，等待人工判定".into(),
            matrix: identity,
            ridge_lambda: None,
            scaled_determinant: sd,
        });
        for (i, lambda) in lambdas.iter().enumerate() {
            candidates.push(CompCandidate {
                id: format!("ridge-{i}"),
                rationale: format!("Tikhonov ridge 正则逆，λ={lambda}，存在偏置，结果非唯一"),
                matrix: ridge_inverse(s, *lambda),
                ridge_lambda: Some(*lambda),
                scaled_determinant: sd,
            });
        }
        CompMatrix {
            fluor_channels: spillover.fluor_channels.clone(),
            spillover_artifact: spillover_artifact.into(),
            spillover_version,
            scaled_determinant: sd,
            near_singular: true,
            singular_threshold: NEAR_SINGULAR_THRESHOLD,
            matrix: None,
            candidates,
            judgement: CompJudgement::NearSingularNoUniqueInverse {
                reason: format!(
                    "列归一化行列式 {sd:.3e} 小于阈值 {NEAR_SINGULAR_THRESHOLD:.0e}，逆不唯一；已保留全部候选"
                ),
            },
        }
    } else {
        let inv = inverse(s).expect("已判定条件良好，求逆必成功");
        CompMatrix {
            fluor_channels: spillover.fluor_channels.clone(),
            spillover_artifact: spillover_artifact.into(),
            spillover_version,
            scaled_determinant: sd,
            near_singular: false,
            singular_threshold: NEAR_SINGULAR_THRESHOLD,
            matrix: Some(inv),
            candidates: Vec::new(),
            judgement: CompJudgement::Ok,
        }
    }
}

/// 把补偿矩阵（或选中的候选矩阵）应用到事件矩阵。
/// 返回与输入等长等宽的新矩阵；非补偿通道原样透传。
pub fn apply_compensation(
    doc: &FcsDocument,
    comp: &CompMatrix,
    chosen_candidate: Option<&str>,
) -> Vec<Vec<f64>> {
    let n = comp.fluor_channels.len();
    let c: &[Vec<f64>] = if let Some(inv) = &comp.matrix {
        inv
    } else {
        let id = chosen_candidate.unwrap_or("identity");
        let cand = comp
            .candidates
            .iter()
            .find(|c| c.id == id)
            .or_else(|| comp.candidates.first())
            .expect("近奇异矩阵至少有一个候选");
        &cand.matrix
    };
    let idx: Vec<usize> = comp
        .fluor_channels
        .iter()
        .map(|name| {
            doc.channels
                .iter()
                .position(|ch| ch.name == *name)
                .unwrap_or_else(|| panic!("补偿引用了未知通道 {name}"))
        })
        .collect();
    let mut out = doc.events.clone();
    for row in out.iter_mut() {
        let fluor: Vec<f64> = idx.iter().map(|&i| row[i]).collect();
        for (i, &col) in idx.iter().enumerate() {
            let mut v = 0.0;
            for j in 0..n {
                v += c[i][j] * fluor[j];
            }
            row[col] = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{Basis, ChannelSpec, RawFlag, TubeRole};

    fn doc_with_events(events: Vec<Vec<f64>>) -> FcsDocument {
        let raw = Basis::RawCounts {
            bits: 16,
            max_code: 65535.0,
            gain: 1.0,
        };
        FcsDocument {
            filename: "t.fcs".into(),
            fcs_version: "FCS3.1".into(),
            data_type: "F".into(),
            byte_order: "little".into(),
            keywords: vec![],
            channels: vec![
                ChannelSpec {
                    name: "FITC-A".into(),
                    long_name: None,
                    detector: None,
                    fluor: None,
                    compensated: true,
                    basis: raw.clone(),
                },
                ChannelSpec {
                    name: "PE-A".into(),
                    long_name: None,
                    detector: None,
                    fluor: None,
                    compensated: true,
                    basis: raw,
                },
            ],
            flags: vec![vec![None; 2]; events.len()],
            events,
            role: TubeRole::Specimen { tube: "x".into() },
        }
    }

    #[test]
    fn well_conditioned_spillover_is_inverted_and_applied() {
        let spill = SpilloverMeta {
            fluor_channels: vec!["FITC-A".into(), "PE-A".into()],
            // FITC 在 PE 有 10% 溢漏
            spillover: vec![vec![1.0, 0.0], vec![0.1, 1.0]],
            controls: vec![],
            convention: "rows=detector cols=fluor".into(),
        };
        let comp = build_comp_matrix(&spill, "spill", 1);
        assert!(!comp.near_singular);
        let doc = doc_with_events(vec![vec![1000.0, 100.0]]);
        let out = apply_compensation(&doc, &comp, None);
        assert!((out[0][0] - 1000.0).abs() < 1e-9);
        // PE' = PE - 0.1*FITC = 0
        assert!(out[0][1].abs() < 1e-9);
        let _ = RawFlag::Saturated;
    }
}
