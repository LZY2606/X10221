//! 派生层引擎：从已裁定的来源版本计算补偿事件与门群体结果。
//!
//! 纯函数：输入全部来源对象 + 上游已接受的门结果（作为钉住的分母），
//! 输出候选门结果/补偿候选。引擎不读写数据库、不做“全局替换”。

use std::collections::{BTreeMap, HashMap};

use crate::domain::{
    GatePayload, LineagePayload, RawDataPayload, SpilloverPayload, TransformPayload,
};
use crate::geometry::{point_in_polygon, spec_for, transform_value, Containment, TransformError};
use crate::matrix::{mat_vec, evaluate_spillover};

#[derive(Debug, Clone)]
pub struct PreparedEvent {
    pub index: u32,
    /// 每个轴坐标（含溢出/负信号诊断），按 gate.space 需要惰性求值后缓存。
    pub diagnostics: Vec<(String, String)>, // (channel, diagnostic)
}

/// 准备好的案例上下文：某分支某时刻所有已接受来源版本。
#[derive(Debug, Clone)]
pub struct Context {
    pub raw: RawDataPayload,
    pub spillover: SpilloverPayload,
    pub transform: TransformPayload,
    pub gates: BTreeMap<String, GatePayload>,
    pub lineage: LineagePayload,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GateResultCandidate {
    /// 稳定候选身份：include_boundary / exclude_boundary
    pub candidate_id: String,
    pub member_indices: Vec<u32>,
    pub numerator: u64,
    pub percent: f64,
    pub denominator: u64,
    /// 分母钉住的父门结果（父门 id + 结果批次 id）；根门钉 raw。
    pub denominator_source: String,
    pub boundary_indices: Vec<u32>,
    pub unconvertible_indices: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct GateResult {
    pub gate_id: String,
    pub gate_version_ref: String,
    pub parent_result_ref: Option<String>,
    pub parent_gate: Option<String>,
    pub candidates: Vec<GateResultCandidate>,
    pub unique: bool,
    pub basis_inputs: Vec<String>,
}

/// 父群成员（已接受候选的成员集合）；根门为全体原始事件。
struct ParentScope {
    members: Vec<u32>,
    result_ref: Option<String>,
    gate_id: Option<String>,
}

fn compensated_values(
    ctx: &Context,
    spill_indices: &HashMap<String, usize>,
    inv: &[Vec<f64>],
    row: &[f64],
) -> Vec<f64> {
    let measured: Vec<f64> = ctx
        .spillover
        .labels
        .iter()
        .map(|lab| {
            let ci = spill_indices[lab];
            row[ci]
        })
        .collect();
    let comp = mat_vec(inv, &measured);
    let mut out = row.to_vec();
    for (i, lab) in ctx.spillover.labels.iter().enumerate() {
        let ci = spill_indices[lab];
        out[ci] = comp[i];
    }
    out
}

fn channel_index(ctx: &Context, name: &str) -> Option<usize> {
    ctx.raw.channels.iter().position(|c| c.name == name)
}

/// 在某个门的坐标空间中计算一个事件的坐标。
/// Err 表示该事件在此门空间内无法唯一转换（留在原位置，不计入任何候选）。
fn event_coords(
    ctx: &Context,
    gate: &GatePayload,
    event_row: &[crate::domain::RawValue],
    use_compensated: bool,
    spill_indices: Option<&HashMap<String, usize>>,
    inv: Option<&[Vec<f64>]>,
) -> Result<[f64; 2], Vec<(String, String)>> {
    let raw_row: Vec<f64> = event_row.iter().map(|v| v.value).collect();
    let effective: Vec<f64> = if use_compensated {
        compensated_values(
            ctx,
            spill_indices.expect("compensated 需要 spillover 索引"),
            inv.expect("compensated 需要已接受的逆矩阵"),
            &raw_row,
        )
    } else {
        raw_row
    };
    let mut diags = Vec::new();
    let mut one = |ch: &str| -> Result<f64, ()> {
        let ci = channel_index(ctx, ch).ok_or(())?;
        let rv = &event_row[ci];
        let val = effective[ci];
        // 每个门轴必须在变换参数中有显式规格（linear/log），坐标基准可追溯。
        let spec = spec_for(&ctx.transform, ch)
            .unwrap_or_else(|| panic!("门 {ch} 轴缺少 transform_params 中的显式规格"));
        // 即使补偿后数值变化，溢出标记仍绑定原始事件，不能默默纳入画图
        match transform_value(val, rv.flag.as_deref(), spec) {
            Ok(v) => Ok(v),
            Err(TransformError::Overflow { flag, raw }) => {
                diags.push((ch.to_string(), format!("OVF 仪器溢出({flag})，原始值 {raw} 保留于 raw 层")));
                Err(())
            }
            Err(TransformError::NegativeForLog { raw }) => {
                diags.push((ch.to_string(), format!("负值 {raw} 无法进入对数坐标，保留于 raw 层")));
                Err(())
            }
        }
    };
    let x = one(&gate.space.x_channel);
    let y = one(&gate.space.y_channel);
    match (x, y) {
        (Ok(x), Ok(y)) => Ok([x, y]),
        _ => Err(diags),
    }
}

/// 对单个门求值。
///
/// * `accepted_comp_inv` —— 已被人工接受的补偿逆矩阵；None 且门空间为 compensated 时 panic
///   （调用顺序保证先有补偿裁定）；
/// * `parent_result_ref/members` —— 钉住的父群版本（百分比分母）。
#[allow(clippy::too_many_arguments)]
pub fn evaluate_gate(
    ctx: &Context,
    gate: &GatePayload,
    gate_version_ref: String,
    parent: ParentScope,
    accepted_comp_inv: Option<&[Vec<f64>]>,
) -> GateResult {
    let use_comp = gate.space.coordinate_basis == "compensated";
    let spill_indices: HashMap<String, usize> = ctx
        .spillover
        .labels
        .iter()
        .enumerate()
        .map(|(i, l)| (l.clone(), channel_index(ctx, l).unwrap_or(i)))
        .collect();

    let mut inside: Vec<u32> = Vec::new();
    let mut boundary: Vec<u32> = Vec::new();
    let mut unconvertible: Vec<u32> = Vec::new();

    for idx in &parent.members {
        let event = &ctx.raw.events[*idx as usize];
        let coords = event_coords(
            ctx,
            gate,
            &event.channels,
            use_comp,
            if use_comp { Some(&spill_indices) } else { None },
            accepted_comp_inv,
        );
        match coords {
            Ok(p) => match point_in_polygon(&gate.polygon, p) {
                Containment::Inside => inside.push(*idx),
                Containment::OnBoundary => boundary.push(*idx),
                Containment::Outside => {}
            },
            Err(_) => unconvertible.push(*idx),
        }
    }

    let denominator = parent.members.len() as u64;
    let denominator_source = match (&parent.result_ref, &parent.gate_id) {
        (Some(r), Some(g)) => format!("gate:{g}/result:{r}"),
        _ => "raw_events".into(),
    };
    let mk = |candidate_id: &str, extra: &[u32]| -> GateResultCandidate {
        let mut members = inside.clone();
        members.extend_from_slice(extra);
        members.sort_unstable();
        let numerator = members.len() as u64;
        GateResultCandidate {
            candidate_id: candidate_id.into(),
            numerator,
            percent: if denominator == 0 {
                0.0
            } else {
                100.0 * numerator as f64 / denominator as f64
            },
            denominator,
            denominator_source: denominator_source.clone(),
            member_indices: members,
            boundary_indices: boundary.clone(),
            unconvertible_indices: unconvertible.clone(),
        }
    };

    let candidates = if boundary.is_empty() {
        vec![mk("unique", &[])]
    } else {
        vec![
            mk("include_boundary", &boundary),
            mk("exclude_boundary", &[]),
        ]
    };

    GateResult {
        gate_id: gate.gate_id.clone(),
        gate_version_ref,
        parent_result_ref: parent.result_ref,
        parent_gate: parent.gate_id,
        unique: boundary.is_empty(),
        candidates,
        basis_inputs: vec![
            gate_version_ref,
            ctx.spillover.labels.join(","),
        ],
    }
}

/// 补偿评估结果。
pub enum CompensationOutcome {
    Unique {
        inverse: Vec<Vec<f64>>,
    },
    Blocked {
        rcond: f64,
        threshold: f64,
        candidates: Vec<(String, Vec<Vec<f64>>, String)>,
    },
}

pub fn evaluate_compensation(ctx: &Context) -> CompensationOutcome {
    match evaluate_spillover(&ctx.spillover.matrix) {
        Ok(cands) => CompensationOutcome::Unique {
            inverse: cands.into_iter().next().expect("健康矩阵恰好一个候选").matrix,
        },
        Err(e) => CompensationOutcome::Blocked {
            rcond: e.rcond,
            threshold: e.threshold,
            candidates: e
                .candidates
                .into_iter()
                .map(|c| (c.kind, c.matrix, c.rationale))
                .collect(),
        },
    }
}

/// 按父子拓扑从根向叶计算全部门。返回顺序可直接作为计算顺序（父先于子）。
pub fn topological_order(lineage: &LineagePayload) -> Vec<String> {
    let parent: HashMap<&str, Option<&str>> = lineage
        .parent_of
        .iter()
        .map(|(g, p)| (g.as_str(), p.as_deref()))
        .collect();
    let mut order = Vec::new();
    let mut visit = |g: &str, order: &mut Vec<String>, stack: &mut Vec<String>| {
        if order.iter().any(|x| x == g) {
            return;
        }
        if stack.iter().any(|x| x == g) {
            panic!("门父子关系存在环：{g}");
        }
        stack.push(g.to_string());
        if let Some(Some(p)) = parent.get(g) {
            let p = (*p).to_string();
            visit(&p, order, stack);
        }
        stack.pop();
        order.push(g.to_string());
    };
    let roots: Vec<String> = lineage
        .parent_of
        .iter()
        .filter(|(_, p)| p.is_none())
        .map(|(g, _)| g.clone())
        .collect();
    for r in roots {
        let mut stack = Vec::new();
        visit(&r, &mut order, &mut stack);
    }
    order
}

/// 把拓扑顺序与已接受父结果组合，批量求值。
/// `gate_versions`: gate_id -> 对象引用；`accepted_parent_runs`: gate_id -> 结果批次引用+成员。
pub fn evaluate_all(
    ctx: &Context,
    gate_versions: &HashMap<String, String>,
    accepted_parent_runs: &HashMap<String, (String, Vec<u32>)>,
    comp_inv: Option<&[Vec<f64>]>,
) -> Vec<GateResult> {
    let order = topological_order(&ctx.lineage);
    let mut produced: HashMap<String, GateResult> = HashMap::new();
    let mut out = Vec::new();
    for gid in order {
        let gate = ctx
            .gates
            .get(&gid)
            .unwrap_or_else(|| panic!("门 {gid} 在 lineage 中但缺少多边形定义"));
        let parent = ctx
            .lineage
            .parent_of
            .iter()
            .find(|(g, _)| g == &gid)
            .and_then(|(_, p)| p.clone());
        let scope = match parent {
            None => ParentScope {
                members: ctx.raw.events.iter().map(|e| e.index).collect(),
                result_ref: None,
                gate_id: None,
            },
            Some(pid) => {
                let (rref, members) = accepted_parent_runs
                    .get(&pid)
                    .or_else(|| {
                        produced
                            .get(&pid)
                            .and_then(|r| r.candidates.first().map(|c| (r.gate_version_ref.clone(), c.member_indices.clone())))
                    })
                    .unwrap_or_else(|| panic!("父门 {pid} 尚无已接受结果"));
                ParentScope {
                    members: members.clone(),
                    result_ref: Some(rref.clone()),
                    gate_id: Some(pid),
                }
            }
        };
        let res = evaluate_gate(
            ctx,
            gate,
            gate_versions
                .get(&gid)
                .cloned()
                .unwrap_or_else(|| format!("gate:{gid}")),
            scope,
            comp_inv,
        );
        produced.insert(gid.clone(), res.clone());
        out.push(res);
    }
    out
}
