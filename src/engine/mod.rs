//! 推导引擎：补偿、变换、门控。规则在此显式执行并留下机器可读诊断。
//!
//! 硬规则：
//! 1. 近奇异补偿矩阵不能被默默求逆：严格奇异 -> error；近奇异 -> blocked，
//!    blocked 时保留全部合法候选（reject / pseudoinverse）。
//! 2. 门百分比的分母钉住父群版本（父门 stable_id + 事件版本 + 计数快照）。
//! 3. 上游修订后旧结果只标记 stale，绝不覆盖；新结果另立。

pub mod gate;
pub mod matrix;

use crate::domain::{
    CompensationArtifact, CompensationResult, EventsArtifact, ResultStatus,
};
use matrix::{analyze, inverse, matmul, pseudoinverse, NEAR_SINGULAR_COND};

/// 补偿评估。哨兵/不可解析通道所在事件不参与矩阵乘法，但原始值仍保留并出诊断。
pub fn evaluate_compensation(
    events: &EventsArtifact,
    comp: &CompensationArtifact,
) -> CompensationResult {
    let stats = match analyze(&comp.matrix) {
        Some(s) => s,
        None => {
            return CompensationResult {
                status: ResultStatus::Error,
                condition_number: None,
                determinant: None,
                compensated: vec![],
                candidates: vec![],
                diagnostics: vec!["补偿矩阵形状非法（非方阵或缺项）".into()],
            };
        }
    };

    let channel_idx: Vec<Option<usize>> = comp
        .detectors
        .iter()
        .map(|d| events.channels.iter().position(|c| c.detector == *d))
        .collect();
    if channel_idx.iter().any(|o| o.is_none()) {
        return CompensationResult {
            status: ResultStatus::Error,
            condition_number: Some(stats.condition_number),
            determinant: Some(stats.determinant),
            compensated: vec![],
            candidates: vec![],
            diagnostics: vec!["补偿矩阵引用了事件中不存在的检测器通道".into()],
        };
    }
    let channel_idx: Vec<usize> = channel_idx.into_iter().flatten().collect();

    let mut diagnostics = skipped_diagnostics(events, &channel_idx, comp);
    let mut compensated: Vec<Vec<f64>> = vec![Vec::new(); events.events.len()];

    if stats.determinant.abs() < 1e-12 || stats.condition_number.is_infinite() {
        diagnostics.push(format!(
            "补偿矩阵严格奇异（det={:.3e}）：无逆矩阵，拒绝输出补偿值",
            stats.determinant
        ));
        diagnostics.push("规则 NEAR-SINGULAR-NO-INV：严格奇异不提供伪逆候选".into());
        return CompensationResult {
            status: ResultStatus::Error,
            condition_number: Some(stats.condition_number),
            determinant: Some(stats.determinant),
            compensated,
            candidates: vec![],
            diagnostics,
        };
    }

    if stats.condition_number > NEAR_SINGULAR_COND {
        diagnostics.push(format!(
            "补偿矩阵近奇异：det={:.3e}, cond2≈{:.3e} > {NEAR_SINGULAR_COND:.0e}；不默默求逆",
            stats.determinant, stats.condition_number
        ));
        diagnostics.push("已保留全部合法候选：reject（拒绝）/ pseudoinverse（伪逆，需显式采纳）".into());
        if let Ok(pinv) = pseudoinverse(&comp.matrix) {
            fill_compensated(events, &channel_idx, &pinv, &mut compensated);
        }
        return CompensationResult {
            status: ResultStatus::Blocked,
            condition_number: Some(stats.condition_number),
            determinant: Some(stats.determinant),
            compensated,
            candidates: vec!["reject".into(), "pseudoinverse".into()],
            diagnostics,
        };
    }

    let inv = inverse(&comp.matrix).expect("良态矩阵可求逆");
    fill_compensated(events, &channel_idx, &inv, &mut compensated);
    CompensationResult {
        status: ResultStatus::Ok,
        condition_number: Some(stats.condition_number),
        determinant: Some(stats.determinant),
        compensated,
        candidates: vec!["inverse".into()],
        diagnostics,
    }
}

fn skipped_diagnostics(
    events: &EventsArtifact,
    channel_idx: &[usize],
    comp: &CompensationArtifact,
) -> Vec<String> {
    let mut out = Vec::new();
    for ev in &events.events {
        for (di, ci) in channel_idx.iter().enumerate() {
            if ev.values[*ci].numeric_opt().is_none() {
                out.push(format!(
                    "事件#{} 通道{} 为哨兵/不可解析（raw={:?}），该事件不参与补偿，原始值保留",
                    ev.index,
                    comp.detectors[di],
                    ev.values[*ci].raw()
                ));
            }
        }
    }
    dedup(out)
}

fn fill_compensated(
    events: &EventsArtifact,
    channel_idx: &[usize],
    inv: &[Vec<f64>],
    out: &mut [Vec<f64>],
) {
    for (row, ev) in events.events.iter().enumerate() {
        let raw: Option<Vec<f64>> = channel_idx
            .iter()
            .map(|ci| ev.values[*ci].numeric_opt())
            .collect();
        if let Some(raw) = raw {
            let mut r = matmul(&vec![raw], inv).pop().unwrap_or_default();
            for v in r.iter_mut() {
                if v.abs() < 1e-9 {
                    *v = 0.0;
                }
            }
            out[row] = r;
        }
    }
}

/// asinh/linear 变换。cofactor 受保护避免除零；坐标基准由调用方随结果标注。
pub fn transform_value(v: f64, cofactor: f64, method: &str) -> f64 {
    match method {
        "linear" => v,
        _ => (v / cofactor.max(1e-12)).asinh(),
    }
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}
