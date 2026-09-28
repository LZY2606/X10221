//! 派生层：补偿策略解析、门控计数、百分比分母版本钉住、依赖指纹与失效传播。
//!
//! 规则（与案例档案中的机器可读取事件对应）：
//! 1. 近奇异补偿矩阵不被默默求逆：生成全部合法候选，等待人工裁定；缺省回退到上一版良好矩阵。
//! 2. 门的百分比分母钉住父群版本：群体行保存 `parent_pop_id`（含父依赖指纹）与快照分母。
//! 3. 修订上游门后，依赖指纹不再匹配的下游群体标记 stale，旧结果保留、不被覆盖。

use crate::domain::{
    analyze_matrix, classify_point, default_lambda, identity_n, mat_vec, matrix_from_json,
    regularized_inverse, transform_value, Contain,
};
use crate::store::{PopRow, Store};
use serde_json::{json, Value};

pub struct ComputeReport {
    pub pop_ids: Vec<String>,
    pub staled: Vec<String>,
    pub new_candidates: Vec<String>,
    pub comp_used_id: String,
}

pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn material_version(id: &str) -> i64 {
    id.rsplit(":v")
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

struct Prepared {
    channels: Vec<String>,
    cofactors: Vec<f64>,
    points: Vec<Vec<f64>>,
}

/// 规范化原始事件：无法转换/极端信号的事件不进入派生层（诊断在导入时已留在原位置）。
fn prepare_events(
    raw: &Value,
    transform: &Value,
    inv: &[Vec<f64>],
) -> (Prepared, Vec<(usize, String)>) {
    let channels: Vec<String> = raw["channels"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| c["name"].as_str().unwrap_or("?").to_string())
                .collect()
        })
        .unwrap_or_default();
    let cofactors: Vec<f64> = channels
        .iter()
        .map(|c| {
            transform["cofactors"]
                .get(c)
                .and_then(|v| v.as_f64())
                .unwrap_or(1000.0)
        })
        .collect();
    let mut skipped = Vec::new();
    let mut points = Vec::new();
    for (ei, ev) in raw["events"].as_array().unwrap_or(&Vec::new()).iter().enumerate() {
        let mut vals = Vec::new();
        let mut bad = None;
        for (ci, cell) in ev.as_array().unwrap_or(&Vec::new()).iter().enumerate() {
            match cell.as_f64() {
                Some(v) if v.is_finite() => {
                    let max = raw["channels"][ci]["range_max"]
                        .as_f64()
                        .unwrap_or(f64::MAX);
                    if v.abs() > max {
                        bad = Some(format!("evt#{ei}.{} extreme={v}", channels[ci]));
                    }
                    vals.push(v);
                }
                _ => bad = Some(format!("evt#{ei}.{} unconvertible={cell}", channels[ci])),
            }
        }
        if let Some(reason) = bad {
            skipped.push((ei, reason));
            continue;
        }
        let compensated = mat_vec(inv, &vals);
        let display: Vec<f64> = compensated
            .iter()
            .zip(&cofactors)
            .map(|(v, c)| transform_value(*v, *c))
            .collect();
        points.push(display);
    }
    (
        Prepared {
            channels,
            cofactors,
            points,
        },
        skipped,
    )
}

fn identity_n(n: usize) -> Vec<Vec<f64>> {
    (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect()
}
