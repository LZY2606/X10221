//! 评估引擎：在给定 refs（钉住的版本集合）上计算通道散点与门谱系群体。
//! 引擎只读材料表，不回写原始数据；门判定结果携带完整来源链。
//! 依赖失效按材料依赖图精确计算，不做全局替换。

use crate::domain::*;
use crate::geometry::point_in_polygon;
use crate::matrix::{assess as assess_matrix, InvertReport};
use crate::transform::{apply_transform, extreme_diagnostic};
use crate::events::Refs;
use serde_json::json;
#[allow(unused_imports)] use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct MaterialBundle {
    pub raw: RawEvents,
    pub meta: SpilloverMeta,
    pub comp: CompensationMatrix,
    pub transform: TransformParams,
    pub gates: Vec<Gate>,
    pub hierarchy: GateHierarchy,
}

#[derive(Debug, Clone, Default)]
pub struct LoadedMaterials {
    pub raw: BTreeMap<(String, u32), RawEvents>,
    pub meta: BTreeMap<(String, u32), SpilloverMeta>,
    pub comp: BTreeMap<(String, u32), CompensationMatrix>,
    pub transform: BTreeMap<(String, u32), TransformParams>,
    pub gates: BTreeMap<(String, u32), Gate>,
    pub hierarchy: BTreeMap<(String, u32), GateHierarchy>,
}

pub fn parse_ref(r: &str) -> (&str, u32) {
    let (id, ver) = r.rsplit_once('/').unwrap_or((r, "0"));
    (id, ver.parse().unwrap_or(0))
}

/// 评估单个事件在变换后坐标系下的各通道值（不可转换通道保留 None + 诊断）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct EventPoint {
    pub event_id: String,
    /// 补偿前（原始线性）
    pub before: BTreeMap<String, Option<f64>>,
    /// 补偿后线性
    pub after: BTreeMap<String, Option<f64>>,
    /// 变换后坐标
    pub transformed: BTreeMap<String, Option<f64>>,
    pub diagnostics: BTreeMap<String, String>,
}

pub fn evaluate_points(b: &MaterialBundle, inversion: &InvertReport) -> Vec<EventPoint> {
    let labels = &b.comp.labels;
    let inv = inversion.inverse.clone();
    let tmap: BTreeMap<&str, &ChannelTransform> =
        b.transform.transforms.iter().map(|t| (t.channel.as_str(), t)).collect();
    let mut out = Vec::new();
    for ev in &b.raw.events {
        let chmap: BTreeMap<&str, &Measured> =
            ev.channels.iter().map(|(k, v)| (k.as_str(), v)).collect();
        let mut before = BTreeMap::new();
        let mut after = BTreeMap::new();
        let mut transformed = BTreeMap::new();
        let mut diagnostics = BTreeMap::new();
        let mut raw_vec: Vec<Option<f64>> = Vec::new();
        for lab in labels {
            let m = chmap.get(lab.as_str());
            let v = m.and_then(|x| x.value);
            raw_vec.push(v);
            before.insert(lab.clone(), v);
            if let Some(m) = m {
                if let Some(d) = &m.diagnostic {
                    diagnostics.insert(lab.clone(), d.clone());
                }
            }
        }
        let all_numeric = raw_vec.iter().all(|v| v.is_some());
        for (i, lab) in labels.iter().enumerate() {
            let val = if all_numeric {
                inv.as_ref().map(|m| {
                    let x: Vec<f64> = raw_vec.iter().map(|v| v.unwrap()).collect();
                    crate::matrix::apply(m, &x)[i]
                })
            } else {
                None
            };
            after.insert(lab.clone(), val);
        }
        for ch in &b.raw.channels {
            let m = chmap.get(ch.as_str());
            let src = if labels.contains(ch) {
                after.get(ch).copied().flatten()
            } else {
                m.and_then(|x| x.value)
            };
            let tv = match (src, tmap.get(ch.as_str())) {
                (Some(v), Some(t)) => {
                    if let Some(d) = extreme_diagnostic(v) {
                        diagnostics.entry(ch.clone()).or_insert_with(|| d.to_string());
                    }
                    Some(apply_transform(v, t))
                }
                (v, Some(t)) if t.method == "linear" => v,
                _ => None,
            };
            transformed.insert(ch.clone(), tv);
        }
        out.push(EventPoint {
            event_id: ev.event_id.clone(),
            before,
            after,
            transformed,
            diagnostics,
        });
    }
    out
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PopResult {
    pub gate_id: String,
    pub gate_ref: String,
    pub gate_name: String,
    pub parent_gate: Option<String>,
    /// 父群门版本的钉住引用（根群为 None）
    pub parent_ref: Option<String>,
    pub denominator_ref: String,
    pub numerator: usize,
    pub denominator: usize,
    pub fraction: f64,
    pub percent: f64,
    pub members: Vec<String>,
    pub coord_basis: String,
}

/// 在给定（已按层级排序的）门集合上求各群成员。父门缺失时该门返回 None 成员。
pub fn evaluate_populations(
    b: &MaterialBundle,
    points: &[EventPoint],
) -> BTreeMap<String, PopResult> {
    let parent_of: BTreeMap<&str, Option<&str>> = b
        .hierarchy
        .parent_of
        .iter()
        .map(|(g, p)| (g.as_str(), p.as_deref()))
        .collect();
    let gmap: BTreeMap<&str, &Gate> = b.gates.iter().map(|g| (g.gate_id.as_str(), g)).collect();
    let pmap: BTreeMap<&str, &EventPoint> =
        points.iter().map(|p| (p.event_id.as_str(), p)).collect();
    let root_n = b.raw.events.len();
    let mut membership: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // 拓扑顺序：反复取父已算的门
    let mut pending: Vec<&str> = gmap.keys().copied().collect();
    while !pending.is_empty() {
        let mut progressed = false;
        let mut next = Vec::new();
        for gid in pending {
            let parent = parent_of.get(gid).copied().flatten();
            let parent_ready = parent.map(|p| membership.contains_key(p)).unwrap_or(true);
            if parent_ready {
                let gate = gmap[gid];
                let include = gate.boundary_convention == "raycast-include";
                let mut mem = Vec::new();
                for p in points {
                    let x = p.transformed.get(&gate.x_channel).copied().flatten();
                    let y = p.transformed.get(&gate.y_channel).copied().flatten();
                    let (Some(x), Some(y)) = (x, y) else { continue };
                    if !point_in_polygon(&gate.polygon, [x, y], include) {
                        continue;
                    }
                    if let Some(pid) = parent {
                        if !membership[pid].contains(&p.event_id) {
                            continue;
                        }
                    }
                    mem.push(p.event_id.clone());
                }
                membership.insert(gid.to_string(), mem);
                progressed = true;
            } else {
                next.push(gid);
            }
        }
        pending = next;
        if !progressed {
            break;
        }
    }
    let mut results = BTreeMap::new();
    for (gid, mem) in &membership {
        let gate = gmap[gid.as_str()];
        let parent = parent_of.get(gid.as_str()).copied().flatten();
        let denominator = parent
            .map(|p| membership.get(p).map(|m| m.len()).unwrap_or(0))
            .unwrap_or(root_n);
        let n = mem.len();
        let fraction = if denominator == 0 { 0.0 } else { n as f64 / denominator as f64 };
        results.insert(
            gid.clone(),
            PopResult {
                gate_id: gid.clone(),
                gate_ref: format!("{}/v{}", gate.gate_id, gate.version),
                gate_name: gate.name.clone(),
                parent_gate: parent.map(|p| p.to_string()),
                parent_ref: parent.map(|p| {
                    let pg = gmap[p];
                    format!("{}/v{}", pg.gate_id, pg.version)
                }),
                denominator_ref: match parent {
                    Some(p) => {
                        let pg = gmap[p];
                        format!("pop://{p}@{}/v{}", pg.gate_id, pg.version)
                    }
                    None => format!("pop://root@{}/v{}", b.raw.material_id, b.raw.version),
                },
                numerator: n,
                denominator,
                fraction,
                percent: fraction * 100.0,
                members: mem.clone(),
                coord_basis: gate.coordinate_basis.clone(),
            },
        );
    }
    results
}

pub fn assess_compensation(b: &MaterialBundle) -> InvertReport {
    assess_matrix(&b.comp.rows)
}

/// 材料依赖：哪类材料变化会使哪些门的群体结论失效。
/// raw_events -> 全部群；compensation -> 全部荧光门；transform -> 用到荧光通道的门；
/// gate 几何 -> 该门及其子孙；hierarchy -> 全部；spillover_meta 不直接进入数值。
pub fn downstream_gates(changed: &[String], b: &MaterialBundle) -> Vec<String> {
    let parent_of: BTreeMap<&str, Option<&str>> = b
        .hierarchy
        .parent_of
        .iter()
        .map(|(g, p)| (g.as_str(), p.as_deref()))
        .collect();
    let gmap: BTreeMap<&str, &Gate> = b.gates.iter().map(|g| (g.gate_id.as_str(), g)).collect();
    let mut affected = std::collections::BTreeSet::new();
    for c in changed {
        if c == "raw_events" || c == "compensation" || c == "hierarchy" {
            for g in gmap.keys() {
                affected.insert(g.to_string());
            }
        } else if c == "transform" {
            for g in b.gates.iter() {
                if g.x_channel != "FSC-A" && g.x_channel != "SSC-A"
                    || g.y_channel != "FSC-A" && g.y_channel != "SSC-A"
                {
                    affected.insert(g.gate_id.clone());
                }
            }
        } else if let Some(gid) = c.strip_prefix("gate:") {
            let mut stack = vec![gid.to_string()];
            while let Some(cur) = stack.pop() {
                if affected.insert(cur.clone()) {
                    for (child, par) in &parent_of {
                        if *par == Some(cur.as_str()) {
                            stack.push(child.to_string());
                        }
                    }
                }
            }
        }
    }
    affected.into_iter().collect()
}

pub fn points_json(points: &[EventPoint]) -> serde_json::Value {
    json!(points)
}

/// 供 web/场景层使用的公开 bundle 构造。
pub fn bundle_for(loaded: &LoadedMaterials, refs: &Refs) -> Option<MaterialBundle> {
    let raw_key = ref_key_str(refs.raw_events.as_ref()?);
    let meta_key = ref_key_str(refs.spillover_meta.as_ref()?);
    let comp_key = ref_key_str(refs.compensation.as_ref()?);
    let tr_key = ref_key_str(refs.transform.as_ref()?);
    let hier_key = ref_key_str(refs.hierarchy.as_ref()?);
    let mut gates = Vec::new();
    for gref in refs.gates.values() {
        let key = ref_key_str(gref);
        gates.push(loaded.gates.get(&key)?.clone());
    }
    Some(MaterialBundle {
        raw: loaded.raw.get(&raw_key)?.clone(),
        meta: loaded.meta.get(&meta_key)?.clone(),
        comp: loaded.comp.get(&comp_key)?.clone(),
        transform: loaded.transform.get(&tr_key)?.clone(),
        gates,
        hierarchy: loaded.hierarchy.get(&hier_key)?.clone(),
    })
}

fn ref_key_str(r: &str) -> (String, u32) {
    let (id, v) = parse_ref(r);
    (id.to_string(), v)
}
