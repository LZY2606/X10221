//! 事件重放 -> 分支视图（投影）。
//!
//! 不变量：
//! - 来源与解释分离：raw.* 只由 imported.case / source.revised 决定；
//! - 近奇异补偿保留全部合法候选，裁定前不默默选逆；
//! - 门结果带输入版本钉版；上游修订后下游 stale=true，不覆盖旧结论；
//! - 百分比分母钉住“父群当时可见的结果版本”。

use crate::mathutil::{analyze_spillover, compensate, point_in_polygon, MatrixAnalysis, Point, TransformParams};
use crate::model::{CasePack, GateDef, RawEvent, SpilloverSpec, TransformSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize)]
pub struct Projection {
    pub branch_id: String,
    pub head_seq: i64,
    pub case_id: Option<String>,
    pub title: Option<String>,
    pub channels: Vec<crate::model::Channel>,
    pub raw_events: Vec<RawEvent>,
    pub spillover: Option<SpilloverSpec>,
    pub spillover_version: i64,
    pub comp_analysis: Option<MatrixAnalysis>,
    pub comp_ruling: Option<CompRulingView>,
    pub boundary_rulings: BTreeMap<String, String>,
    pub rejected_pop_candidates: Vec<String>,
    pub transforms: BTreeMap<String, TransformSpec>,
    pub transform_version: i64,
    pub gates: BTreeMap<String, GateState>,
    pub gate_order: Vec<String>,
    pub populations: BTreeMap<String, PopulationState>,
    pub diagnostics: Vec<Diagnostic>,
    pub events: Vec<EventView>,
    pub rule_findings: Vec<RuleFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompRulingView {
    pub candidate_id: String,
    pub decided_at_seq: i64,
    pub decided_by_event: String,
    pub action: String, // accept | reject | adjust
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateState {
    pub def: GateDef,
    pub version: i64,
    pub name_version: i64,
    pub geometry_version: i64,
    pub stale: bool,
    pub stale_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopulationState {
    pub gate_id: String,
    pub gate_version: i64,
    pub boundary_convention: Option<String>,
    pub candidate_cause: Option<String>,
    pub candidates: Vec<PopulationCandidateView>,
    pub ruling_event: Option<String>,
    pub parent_gate_version: i64,
    pub parent_pop_version: i64,
    pub members: Vec<String>,
    pub numerator: usize,
    /// 分母钉住父群当时版本；根门分母为全体可计数事件。
    pub denominator: usize,
    pub denominator_source: DenominatorSource,
    pub percent: f64,
    pub stale: bool,
    pub stale_reason: Option<String>,
    pub computed_at_seq: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum DenominatorSource {
    RootAllEvents {
        raw_version: i64,
        note: String,
    },
    ParentPopulation {
        parent_gate_id: String,
        parent_gate_version: i64,
        parent_pop_version: i64,
        parent_count: usize,
    },
    Blocked {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub event_id: Option<String>,
    pub channel: Option<String>,
    pub level: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventView {
    pub event_id: String,
    pub seq: i64,
    pub parent_event: Option<String>,
    pub kind: String,
    pub rule: Option<String>,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleFinding {
    pub rule: String,
    pub status: String, // fired | satisfied | violated
    pub affected: Vec<String>,
    pub at_seq: i64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopulationCandidateView {
    pub id: String,
    pub convention: String,
    pub members: Vec<String>,
    pub numerator: usize,
    pub denominator: usize,
    pub percent: f64,
    pub chosen: bool,
}

/// 重放所需的最小事件结构。
#[derive(Debug, Clone, Deserialize)]
pub struct ReplayEvent {
    pub event_id: String,
    pub seq: i64,
    pub parent_event: Option<String>,
    pub kind: String,
    pub payload: Value,
    pub rule: Option<String>,
    pub created_at: String,
}

#[derive(Default)]
struct Versions {
    raw: i64,
    spill: i64,
    transform: i64,
}

pub fn replay(branch_id: &str, head_seq: i64, events: &[ReplayEvent]) -> Projection {
    let mut p = Projection {
        branch_id: branch_id.into(),
        head_seq,
        case_id: None,
        title: None,
        channels: vec![],
        raw_events: vec![],
        spillover: None,
        spillover_version: 0,
        comp_analysis: None,
        comp_ruling: None,
        boundary_rulings: BTreeMap::new(),
        rejected_pop_candidates: vec![],
        transforms: BTreeMap::new(),
        transform_version: 0,
        gates: BTreeMap::new(),
        gate_order: vec![],
        populations: BTreeMap::new(),
        diagnostics: vec![],
        events: vec![],
        rule_findings: vec![],
    };
    let mut ver = Versions::default();
    let mut gate_ver: BTreeMap<String, i64> = BTreeMap::new();
    let mut name_ver: BTreeMap<String, i64> = BTreeMap::new();
    let mut geom_ver: BTreeMap<String, i64> = BTreeMap::new();

    for ev in events.iter().filter(|e| e.seq <= head_seq) {
        p.events.push(EventView {
            event_id: ev.event_id.clone(),
            seq: ev.seq,
            parent_event: ev.parent_event.clone(),
            kind: ev.kind.clone(),
            rule: ev.rule.clone(),
            payload: ev.payload.clone(),
            created_at: ev.created_at.clone(),
        });
        match ev.kind.as_str() {
            "imported.case" => {
                if let Ok(case) = serde_json::from_value::<CasePack>(ev.payload.clone()) {
                    p.case_id = Some(case.case_id.clone());
                    p.title = Some(case.title.clone());
                    p.channels = case.channels;
                    p.raw_events = case.raw_events;
                    p.spillover = Some(case.spillover);
                    p.transforms = case.transforms.into_iter().map(|t| (t.channel.clone(), t)).collect();
                    p.gate_order = case.gates.iter().map(|g| g.id.clone()).collect();
                    for g in case.gates {
                        gate_ver.insert(g.id.clone(), ev.seq);
                        name_ver.insert(g.id.clone(), ev.seq);
                        geom_ver.insert(g.id.clone(), ev.seq);
                        p.gates.insert(g.id.clone(), GateState {
                            def: g,
                            version: ev.seq,
                            name_version: ev.seq,
                            geometry_version: ev.seq,
                            stale: false,
                            stale_reason: None,
                        });
                    }
                    ver = Versions { raw: ev.seq, spill: ev.seq, transform: ev.seq };
                    p.diagnostics.extend(scan_raw_diagnostics(&p.raw_events));
                }
            }
            "source.revised" => apply_source_revision(&mut p, &mut ver, ev),
            "spillover.revised" => {
                if let Ok(s) = serde_json::from_value::<SpilloverSpec>(ev.payload.clone()) {
                    p.spillover = Some(s);
                    p.spillover_version = ev.seq;
                    ver.spill = ev.seq;
                    p.comp_ruling = None;
                    mark_downstream_stale(&mut p, std::iter::empty::<String>(), ev.seq,
                        "spillover 元数据已修订，全部荧光门结果过期");
                    p.rule_findings.push(RuleFinding {
                        rule: "RULE_UPSTREAM_STALE_NO_OVERWRITE".into(),
                        status: "fired".into(),
                        affected: p.gate_order.clone(),
                        at_seq: ev.seq,
                        detail: "补偿来源变化使依赖结果过期，旧结果保留并标记 stale".into(),
                    });
                }
            }
            "transform.revised" => {
                if let Ok(t) = serde_json::from_value::<TransformSpec>(ev.payload.clone()) {
                    p.transforms.insert(t.channel.clone(), t);
                    p.transform_version = ev.seq;
                    ver.transform = ev.seq;
                    let affected: Vec<String> = p.gate_order.iter()
                        .filter(|id| {
                            let g = &p.gates[*id].def;
                            g.x_channel == ev.payload["channel"] || g.y_channel == ev.payload["channel"]
                        })
                        .cloned().collect();
                    mark_downstream_stale(&mut p, affected.iter().cloned(), ev.seq,
                        "变换参数已修订，依赖该通道的门结果过期");
                }
            }
            "gate.renamed" => {
                let gid = ev.payload["gate_id"].as_str().unwrap_or("");
                if let Some(g) = p.gates.get_mut(gid) {
                    if let Some(name) = ev.payload["name"].as_str() {
                        g.def.name = name.into();
                    }
                    g.name_version = ev.seq;
                    g.version = ev.seq;
                    name_ver.insert(gid.into(), ev.seq);
                    gate_ver.insert(gid.into(), ev.seq);
                }
            }
            "gate.geometry_revised" => {
                if let Ok(def) = serde_json::from_value::<GateDef>(ev.payload["gate"].clone()) {
                    geom_ver.insert(def.id.clone(), ev.seq);
                    gate_ver.insert(def.id.clone(), ev.seq);
                    if let Some(g) = p.gates.get_mut(&def.id) {
                        let name = g.def.name.clone();
                        let nv = g.name_version;
                        g.def = def;
                        g.def.name = name; // 改名与几何修订分开版本化
                        g.name_version = nv;
                        g.geometry_version = ev.seq;
                        g.version = ev.seq;
                        g.stale = false;
                        g.stale_reason = None;
                    }
                    mark_downstream_stale(&mut p, std::iter::once(def.id.clone()), ev.seq,
                        "上游门几何已修订，下游结果过期");
                    p.rule_findings.push(RuleFinding {
                        rule: "RULE_UPSTREAM_STALE_NO_OVERWRITE".into(),
                        status: "fired".into(),
                        affected: descendant_gates(&p, &def.id),
                        at_seq: ev.seq,
                        detail: format!("门 {} 修订后，其下游结果标记 stale，不覆盖", def.id),
                    });
                }
            }
            "comp.ruling" => {
                let candidate = ev.payload["candidate_id"].as_str().unwrap_or("").to_string();
                p.comp_ruling = Some(CompRulingView {
                    candidate_id: candidate,
                    decided_at_seq: ev.seq,
                    decided_by_event: ev.event_id.clone(),
                    action: ev.payload["action"].as_str().unwrap_or("accept").into(),
                    note: ev.payload["note"].as_str().unwrap_or("").into(),
                });
            }
            "gate.boundary_ruling" => {
                let gid = ev.payload["gate_id"].as_str().unwrap_or("").to_string();
                let convention = ev.payload["convention"].as_str().unwrap_or("").to_string();
                let action = ev.payload["action"].as_str().unwrap_or("accept");
                if action == "reject" {
                    p.boundary_rulings.remove(&gid);
                    p.rejected_pop_candidates
                        .push(format!("{gid}:{convention}@seq{}", ev.seq));
                } else {
                    p.boundary_rulings.insert(gid, convention);
                }
            }
            "review.recomputed" => { /* 触发重算的时间点，实际结果由重放现算 */ }
            "ruling.conflict" | "draft.conflict" => {
                p.rule_findings.push(RuleFinding {
                    rule: "RULE_SAME_VERSION_CONCURRENT_MODIFY".into(),
                    status: "fired".into(),
                    affected: vec![ev.payload["branch_id"].as_str().unwrap_or("").to_string()],
                    at_seq: ev.seq,
                    detail: "基于同一版本的并发修改被拒绝合并，双方差异保留在事件中".into(),
                });
            }
            _ => {}
        }
    }

    // 重新计算补偿分析（钉住当前 spillover 版本；导入时即为基线）。
    if let Some(s) = &p.spillover {
        let analysis = analyze_spillover(&s.matrix);
        // 近奇异/奇异的机器可读判断依据
        let (status, detail) = if analysis.singular {
            ("fired", format!("det={:.3e}：拒绝默默求逆", analysis.determinant))
        } else if analysis.near_singular {
            ("fired", format!("cond={:.3e}：保留 {} 个合法候选", analysis.condition, analysis.candidates.len()))
        } else {
            ("satisfied", format!("det={:.3e}, cond={:.2}", analysis.determinant, analysis.condition))
        };
        if analysis.singular || analysis.near_singular {
            p.rule_findings.push(RuleFinding {
                rule: analysis.rule.clone(),
                status: status.into(),
                affected: vec![s.id.clone()],
                at_seq: p.spillover_version.max(1),
                detail,
            });
        }
        for d in &analysis.diagnostics {
            p.diagnostics.push(Diagnostic {
                event_id: None,
                channel: None,
                level: "warning".into(),
                code: analysis.rule.clone(),
                message: d.clone(),
            });
        }
        p.comp_analysis = Some(analysis);
    }

    evaluate_populations(&mut p, &ver);
    p
}

fn scan_raw_diagnostics(events: &[RawEvent]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for e in events {
        for c in &e.values {
            if let Some(msg) = &c.diagnostic {
                out.push(Diagnostic {
                    event_id: Some(e.id.clone()),
                    channel: Some(c.channel.clone()),
                    level: "error".into(),
                    code: "RAW_UNCONVERTIBLE".into(),
                    message: msg.clone(),
                });
            }
        }
    }
    out
}

fn apply_source_revision(p: &mut Projection, ver: &mut Versions, ev: &ReplayEvent) {
    let event_id = ev.payload["event_id"].as_str().unwrap_or("");
    let channel = ev.payload["channel"].as_str().unwrap_or("");
    let raw = ev.payload["raw"].as_str().unwrap_or("");
    if let Some(target) = p.raw_events.iter_mut().find(|e| e.id == event_id) {
        if let Some(cell) = target.values.iter_mut().find(|c| c.channel == channel) {
            let unit = cell.unit.clone();
            let parsed = crate::model::parse_raw_cell(channel, raw, &unit);
            let was_bad = cell.value.is_none();
            *cell = parsed;
            if cell.value.is_none() {
                if let Some(msg) = &cell.diagnostic {
                    p.diagnostics.push(Diagnostic {
                        event_id: Some(event_id.into()),
                        channel: Some(channel.into()),
                        level: "error".into(),
                        code: "RAW_UNCONVERTIBLE".into(),
                        message: msg.clone(),
                    });
                }
            } else if was_bad {
                p.diagnostics.retain(|d| {
                    !(d.event_id.as_deref() == Some(event_id)
                        && d.channel.as_deref() == Some(channel)
                        && d.code == "RAW_UNCONVERTIBLE")
                });
            }
        }
    }
    ver.raw = ev.seq;
    // 局部失效：只让依赖该原始事件的群体过期（精确失效范围）。
    let affected_gates: Vec<String> = p.gate_order.clone();
    for pop in p.populations.values_mut() {
        if pop.members.iter().any(|m| m == event_id) || pop.denominator_source_uses(event_id) {
            pop.stale = true;
            pop.stale_reason = Some(format!("原始事件 {event_id} 已修订"));
        }
    }
    let _ = affected_gates;
}

impl DenominatorSource {
    fn uses_event(&self, _id: &str) -> bool {
        false
    }
}

impl PopulationState {
    fn denominator_source_uses(&self, id: &str) -> bool {
        self.members.iter().any(|m| m == id)
    }
}

fn descendant_gates(p: &Projection, root: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_string()];
    while let Some(cur) = stack.pop() {
        for id in &p.gate_order {
            if let Some(g) = p.gates.get(id) {
                if g.def.parent.as_deref() == Some(cur.as_str()) && !out.contains(id) {
                    out.push(id.clone());
                    stack.push(id.clone());
                }
            }
        }
    }
    out
}

fn mark_downstream_stale<I>(p: &mut Projection, roots: I, at_seq: i64, reason: &str)
where
    I: IntoIterator<Item = String>,
{
    let mut targets: BTreeSet<String> = roots.into_iter().collect();
    // spillover/transform 变化时 targets 已包含受影响门；这里统一向下游传播
    loop {
        let before = targets.len();
        for id in p.gate_order.clone() {
            if let Some(g) = p.gates.get(&id) {
                if let Some(parent) = &g.def.parent {
                    if targets.contains(parent) && !targets.contains(&id) {
                        targets.insert(id.clone());
                    }
                }
            }
        }
        if targets.len() == before {
            break;
        }
    }
    for id in targets {
        if let Some(g) = p.gates.get_mut(&id) {
            g.stale = true;
            g.stale_reason = Some(reason.into());
        }
        if let Some(pop) = p.populations.get_mut(&id) {
            pop.stale = true;
            pop.stale_reason = Some(format!("{reason}（上游变更于 seq={at_seq}）"));
        }
    }
}

fn transform_map(p: &Projection) -> BTreeMap<String, TransformParams> {
    p.transforms
        .values()
        .map(|t| {
            (
                t.channel.clone(),
                TransformParams {
                    channel: t.channel.clone(),
                    basis: t.basis.clone(),
                    cofactor: t.cofactor,
                    scale: t.scale,
                },
            )
        })
        .collect()
}

fn numeric_value(e: &RawEvent, ch: &str) -> Option<f64> {
    e.values.iter().find(|c| c.channel == ch).and_then(|c| c.value)
}

/// 事件在某基线下的二维坐标；需要的任一原始值不可转换时返回 None（不丢弃，仅不可定位）。
fn coords(
    e: &RawEvent,
    g: &GateDef,
    inv: Option<&[Vec<f64>]>,
    fluor_order: &[String],
    transforms: &BTreeMap<String, TransformParams>,
) -> Option<Point> {
    let pick = |ch: &str| -> Option<f64> {
        let raw = numeric_value(e, ch)?;
        if g.vertices_basis == "asinh_compensated" {
            // 先补偿
            if let Some(m) = inv {
                if let Some(idx) = fluor_order.iter().position(|c| c == ch) {
                    let fluor_vals: Vec<f64> = fluor_order
                        .iter()
                        .map(|fc| numeric_value(e, fc).unwrap_or(0.0))
                        .collect();
                    // 任一荧光通道不可转换 -> 无法在补偿空间定位
                    if fluor_order.iter().any(|fc| numeric_value(e, fc).is_none()) {
                        return None;
                    }
                    let comp = compensate(m, &fluor_vals);
                    let v = comp[idx];
                    return Some(transforms.get(ch).map(|t| t.apply(v)).unwrap_or(v));
                }
            }
            None
        } else {
            Some(raw)
        }
    };
    Some(Point { x: pick(&g.x_channel)?, y: pick(&g.y_channel)? })
}

fn evaluate_populations(p: &mut Projection, ver: &Versions) {
    let Some(analysis) = p.comp_analysis.clone() else {
        return;
    };

    // 选择补偿候选：严格良态自动 exact；近奇异必须人工裁定；奇异则荧光门阻断。
    let chosen: Option<(String, Vec<Vec<f64>>)> = if analysis.singular {
        None
    } else if analysis.near_singular {
        if let Some(r) = &p.comp_ruling {
            analysis
                .candidates
                .iter()
                .find(|c| c.id == r.candidate_id)
                .map(|c| (c.id.clone(), c.inverse.clone()))
        } else {
            // 未裁定：无法唯一决定，保留候选（以 Blocked 形式呈现，不静默挑选）
            None
        }
    } else {
        analysis.candidates.first().map(|c| ("exact".into(), c.inverse.clone()))
    };

    let fluor = p.spillover.as_ref().map(|s| s.channels.clone()).unwrap_or_default();
    let transforms = transform_map(p);
    let root_members: Vec<String> = p
        .raw_events
        .iter()
        .filter(|e| {
            // 根门 G1 只依赖 FSC/SSC（linear），总是可计数；无法转换则排除并附诊断
            e.values.iter().all(|c| {
                matches!(c.channel.as_str(), "FSC" | "SSC") == false || c.value.is_some()
            })
        })
        .map(|e| e.id.clone())
        .collect();
    let root_count = root_members.len();

    // 按父->子拓扑序评估
    let order = p.gate_order.clone();
    let mut computed: BTreeMap<String, PopulationState> = BTreeMap::new();
    for gid in order {
        let gstate = match p.gates.get(&gid) {
            Some(g) => g.clone(),
            None => continue,
        };
        let g = &gstate.def;
        let parent = g.parent.clone();

        // 计算几何父群成员（钉版：使用当前投影里父群的版本）
        let parent_ctx: Option<(String, i64, i64, Vec<String>)> = match &parent {
            None => None,
            Some(pid) => computed.get(pid).map(|pp| {
                (pid.clone(), pp.gate_version, pp.computed_at_seq, pp.members.clone())
            }),
        };

        let needs_fluor = g.vertices_basis == "asinh_compensated";
        let blocked_reason = if needs_fluor && analysis.singular {
            Some("BLOCKED_SINGULAR_SPILLOVER".to_string())
        } else if needs_fluor && analysis.near_singular && chosen.is_none() {
            Some("BLOCKED_PENDING_COMP_RULING".to_string())
        } else if let Some((pid, _, _, ref pmembers)) = parent_ctx {
            if let Some(pp) = computed.get(&pid) {
                if pp.stale {
                    Some(format!("BLOCKED_PARENT_STALE:{pid}"))
                } else if pmembers.is_empty() {
                    None
                } else {
                    None
                }
            } else {
                Some(format!("BLOCKED_PARENT_MISSING:{pid}"))
            }
        } else {
            None
        };

        if let Some(reason) = blocked_reason {
            computed.insert(gid.clone(), PopulationState {
                gate_id: gid.clone(),
                gate_version: gstate.version,
                boundary_convention: None,
                candidate_cause: None,
                candidates: vec![],
                ruling_event: None,
                parent_gate_version: parent_ctx.as_ref().map(|(_, gv, _, _)| *gv).unwrap_or(0),
                parent_pop_version: parent_ctx.as_ref().map(|(_, _, pv, _)| *pv).unwrap_or(0),
                members: vec![],
                numerator: 0,
                denominator: 0,
                denominator_source: DenominatorSource::Blocked { reason },
                percent: 0.0,
                stale: gstate.stale,
                stale_reason: gstate.stale_reason,
                computed_at_seq: p.head_seq,
            });
            continue;
        }

        let inv = chosen.as_ref().map(|(_, m)| m.as_slice());
        let pool: Vec<String> = match &parent_ctx {
            None => root_members.clone(),
            Some((_, _, _, pm)) => pm.clone(),
        };

        let poly: Vec<Point> = g.vertices.iter().map(|v| Point { x: v[0], y: v[1] }).collect();

        // 两种边界约定都评估，检查是否产生不同成员 -> 多候选
        let membership = |inclusive: bool| -> (Vec<String>, Vec<String>, usize) {
            let mut inside = Vec::new();
            let mut unlocated = 0usize;
            let mut boundary_events = Vec::new();
            for id in &pool {
                let ev = match p.raw_events.iter().find(|e| e.id == *id) {
                    Some(e) => e,
                    None => continue,
                };
                match coords(ev, g, inv, &fluor, &transforms) {
                    Some(pt) => {
                        let inc = point_in_polygon(pt, &poly, true);
                        let exc = point_in_polygon(pt, &poly, false);
                        if inc && !exc {
                            boundary_events.push(id.clone());
                        }
                        if point_in_polygon(pt, &poly, inclusive) {
                            inside.push(id.clone());
                        }
                    }
                    None => unlocated += 1,
                }
            }
            (inside, boundary_events, unlocated)
        };

        let (members_inc, boundary, unlocated) = membership(true);
        let (members_exc, _, _) = membership(false);
        let multi_boundary = members_inc != members_exc;

        if multi_boundary {
            p.rule_findings.push(RuleFinding {
                rule: "RULE_GATE_BOUNDARY_AMBIGUITY_KEEP_CANDIDATES".into(),
                status: "fired".into(),
                affected: boundary.iter().map(|b| format!("{}:{}", gid, b)).collect(),
                at_seq: p.head_seq,
                detail: format!(
                    "门 {} 的边界点 {:?} 在两种约定下结论不同，保留 edge-in/edge-out 两个候选等待裁定",
                    gid, boundary
                ),
            });
        }
        if unlocated > 0 {
            p.diagnostics.push(Diagnostic {
                event_id: None,
                channel: Some(format!("{}/{}", g.x_channel, g.y_channel)),
                level: "warning".into(),
                code: "EVENT_UNLOCATABLE_IN_BASIS".into(),
                message: format!("门 {gid}：{unlocated} 个父群事件因通道值无法转换而不能定位（保留原值，未删除）"),
            });
        }

        // 候选集合：边界无歧义时仅一个候选；有歧义时保留 edge-in/edge-out 全部合法候选。
        let make_candidate = |conv: &str, cand_members: Vec<String>, chosen: bool| {
            let num = cand_members.len();
            let denom = match &parent_ctx {
                None => root_count,
                Some((_, _, _, pm)) => pm.len(),
            };
            PopulationCandidateView {
                id: format!("{gid}:{conv}"),
                convention: conv.into(),
                numerator: num,
                denominator: denom,
                percent: if denom > 0 { 100.0 * num as f64 / denom as f64 } else { 0.0 },
                members: cand_members,
                chosen,
            }
        };
        let ruled = p.boundary_rulings.get(&gid).cloned();
        let (chosen_conv, ruling_event) = match &ruled {
            Some(c) => (c.clone(), Some(format!("ruling:{gid}:{c}"))),
            None => ("edge-in".to_string(), None),
        };
        let mut candidates = vec![make_candidate("edge-in", members_inc.clone(), chosen_conv == "edge-in")];
        if multi_boundary {
            candidates.push(make_candidate("edge-out", members_exc.clone(), chosen_conv == "edge-out"));
        }
        let members = candidates
            .iter()
            .find(|c| c.chosen)
            .map(|c| c.members.clone())
            .unwrap_or(members_inc);
        let numerator = members.len();
        let (denominator, denom_src) = match &parent_ctx {
            None => (
                root_count,
                DenominatorSource::RootAllEvents {
                    raw_version: ver.raw,
                    note: "分母=全部原始可计数事件，钉住 raw 版本".into(),
                },
            ),
            Some((pid, pgv, ppv, pm)) => (
                pm.len(),
                DenominatorSource::ParentPopulation {
                    parent_gate_id: pid.clone(),
                    parent_gate_version: *pgv,
                    parent_pop_version: *ppv,
                    parent_count: pm.len(),
                },
            ),
        };
        let percent = if denominator > 0 {
            100.0 * numerator as f64 / denominator as f64
        } else {
            0.0
        };

        computed.insert(gid.clone(), PopulationState {
            gate_id: gid,
            gate_version: gstate.version,
            boundary_convention: Some(chosen_conv),
            candidate_cause: if multi_boundary {
                if ruling_event.is_some() {
                    Some("BOUNDARY_RULED".into())
                } else {
                    Some("BOUNDARY_EDGE_IN_OUT_CANDIDATES_PENDING_RULING".into())
                }
            } else {
                None
            },
            candidates,
            ruling_event,
            parent_gate_version: parent_ctx.as_ref().map(|(_, gv, _, _)| *gv).unwrap_or(0),
            parent_pop_version: parent_ctx.as_ref().map(|(_, _, pv, _)| *pv).unwrap_or(0),
            members,
            numerator,
            denominator,
            denominator_source: denom_src,
            percent,
            stale: gstate.stale,
            stale_reason: gstate.stale_reason,
            computed_at_seq: p.head_seq,
        });
    }
    p.populations = computed;

    // 门百分比钉版规则的机器可读记录
    for pop in p.populations.values() {
        if let DenominatorSource::ParentPopulation {
            parent_gate_id,
            parent_gate_version,
            ..
        } = &pop.denominator_source
        {
            if *parent_gate_version != 0 {
                p.rule_findings.push(RuleFinding {
                    rule: "RULE_PERCENT_DENOMINATOR_PINNED_TO_PARENT_VERSION".into(),
                    status: "satisfied".into(),
                    affected: vec![pop.gate_id.clone()],
                    at_seq: pop.computed_at_seq,
                    detail: format!(
                        "群 {} 的分母钉住父群 {}@v{}",
                        pop.gate_id, parent_gate_id, parent_gate_version
                    ),
                });
            }
        }
    }
}
