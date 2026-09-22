//! 纯函数评估层：给定 State + 材料，产出每个节点的当前投影、历史快照与分母来源。
use crate::engine::{as_number, stable_id, State};
use crate::linalg::{self, inspect, mat_vec, pinv_apply, tikhonov_apply, tikhonov_lambda};
use crate::model::*;
use rusqlite::Transaction;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const FLUO: [&str; 3] = ["FITC-A", "PE-A", "APC-A"];

pub fn load<T: serde::de::DeserializeOwned>(
    t: &Transaction,
    r: &MatRef,
) -> rusqlite::Result<Option<T>> {
    let raw = crate::store_ops::material_raw(t, &r.kind, &r.id, r.version)?;
    Ok(raw.and_then(|v| serde_json::from_value(v).ok()))
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub cfg_hash: String,
    pub deps: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CellValue {
    pub event_id: String,
    pub value: Option<f64>,
    pub raw: Value,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CompResult {
    pub subject: String,
    pub classification: String,
    pub determinant: f64,
    pub condition_1: f64,
    pub rcond_1: f64,
    /// 当前生效候选（none-inversion/pinv/tikhonov/explicit）
    pub adopted: Option<String>,
    pub candidates: Vec<Candidate>,
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub legal: bool,
    pub values: Vec<CellValue>,
    pub rationale: String,
}

pub struct Eval {
    pub state: State,
    pub event_ref: MatRef,
    pub event_file: EventFile,
    pub spill: Spillover,
    pub comp_ref: MatRef,
    pub comp: Compensation,
    pub transform: Transform,
    pub gates: BTreeMap<String, (MatRef, Gate)>,
    pub lineage_ref: MatRef,
    pub lineage: Lineage,
    /// node_id -> (cfg_hash, 投影结果 json, basis json, deps)
    pub nodes: BTreeMap<String, NodeView>,
    pub diagnostics: Vec<Value>,
}

#[derive(Clone)]
pub struct NodeView {
    pub node: Node,
    pub data: Value,
    pub basis: Value,
}

impl Eval {
    pub fn build(t: &Transaction, state: State) -> Result<Eval, String> {
        let event_ref = state
            .mat_ref("events", "fcs-baseline")
            .or_else(|| any_events(&state, t, "events"))
            .ok_or("缺少 events 材料")?;
        let event_file: EventFile = load(t, &event_ref)
            .map_err(|e| e.to_string())?
            .ok_or("events 材料无法解析")?;
        let spill_ref = state
            .mat_ref("spillover", "spill-3c")
            .or_else(|| any_events(&state, t, "spillover"))
            .ok_or("缺少 spillover 材料")?;
        let spill: Spillover = load(t, &spill_ref)
            .map_err(|e| e.to_string())?
            .ok_or("spillover 无法解析")?;
        let comp_ref = state
            .mat_ref("compensation", "comp-3c")
            .or_else(|| any_events(&state, t, "compensation"))
            .ok_or("缺少 compensation 材料")?;
        let comp: Compensation = load(t, &comp_ref)
            .map_err(|e| e.to_string())?
            .ok_or("compensation 无法解析")?;
        let tf_ref = state
            .mat_ref("transform", "xform-default")
            .or_else(|| any_events(&state, t, "transform"))
            .ok_or("缺少 transform 材料")?;
        let transform: Transform =
            load(t, &tf_ref).map_err(|e| e.to_string())?.ok_or("transform 无法解析")?;
        let lineage_ref = state
            .mat_ref("lineage", "lineage-main")
            .or_else(|| any_events(&state, t, "lineage"))
            .ok_or("缺少 lineage 材料")?;
        let lineage: Lineage = load(t, &lineage_ref)
            .map_err(|e| e.to_string())?
            .ok_or("lineage 无法解析")?;
        let mut gates = BTreeMap::new();
        for p in &lineage.populations {
            let g_ref = p.gate.clone();
            if let Some(version) = state.mat_version("gate", &g_ref.id) {
                let mut r = g_ref.clone();
                r.version = version;
                let gate: Gate = load(t, &r).map_err(|e| e.to_string())?.ok_or("gate 无法解析")?;
                gates.insert(p.id.clone(), (r, gate));
            }
        }
        let mut ev = Eval {
            state,
            event_ref,
            event_file,
            spill,
            comp_ref,
            comp,
            transform,
            gates,
            lineage_ref,
            lineage,
            nodes: BTreeMap::new(),
            diagnostics: vec![],
        };
        ev.evaluate(t)?;
        Ok(ev)
    }
}

fn any_events(_st: &State, t: &Transaction, kind: &str) -> Option<MatRef> {
    let mut stmt = t
        .prepare(
            "SELECT id, MAX(version) FROM materials WHERE kind=?1 GROUP BY id ORDER BY id LIMIT 1",
        )
        .ok()?;
    let row = stmt
        .query_row([kind], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .ok()?;
    Some(MatRef { kind: kind.to_string(), id: row.0, version: row.1 as u32 })
}

pub fn transform_value(method: &str, x: f64, cofactor: Option<f64>, decades: Option<f64>) -> f64 {
    match method {
        "linear" => x,
        "asinh" => asinh_scaled(x, cofactor),
        "log10" => {
            if x > 0.0 {
                x.log10()
            } else {
                // 非正数不进入对数坐标：返回 null 由调用方诊断，不静默裁剪。
                f64::NAN
            }
        }
        _ => x,
    }
}

fn asinh_scaled(x: f64, _cofactor: Option<f64>) -> f64 {
    // asinh 输出坐标基准：asinh(x)（无量纲比值），cofactor 记录于变换参数版本。
    x.asinh()
}

impl Eval {
    fn evaluate(&mut self, _t: &Transaction) -> Result<(), String> {
        // 1) 原始通道节点（保留单位/坐标基准与原始表示）
        for ch in &self.event_file.channels {
            let mut values = vec![];
            for cell in &self.event_file.events {
                let raw = cell.values.get(&ch.name).cloned().unwrap_or(Value::Null);
                let (num, diag) = match &raw {
                    Value::Number(_) => (as_number(&raw), None),
                    Value::String(s) => match as_number(&raw) {
                        Some(_) => (as_number(&raw), Some(format!("string-coerced:{}", s))),
                        None => (None, Some(format!("unconvertible:{}", s))),
                    },
                    Value::Null => (None, Some("missing".to_string())),
                    other => (None, Some(format!("unsupported-type:{}", other))),
                };
                if let Some(d) = &diag {
                    self.diagnostics.push(json!({
                        "scope": "raw", "channel": ch.name, "event": cell.id,
                        "severity": if d.starts_with("unconvertible") { "error" } else { "warn" },
                        "message": d, "raw": raw,
                    }));
                }
                values.push(CellValue { event_id: cell.id.clone(), value: num, raw, diagnostic: diag });
            }
            let cfg = format!("{}|{}|{}", self.event_ref, ch.name, ch.unit);
            let hash = stable_id("cfg", &[&cfg]);
            let id = stable_id("raw", &[self.event_ref.to_string().as_str(), &ch.name]);
            self.nodes.insert(
                id.clone(),
                NodeView {
                    node: Node {
                        id,
                        kind: "raw-channel".into(),
                        label: format!("原始通道 {} ({})", ch.name, ch.unit),
                        cfg_hash: hash.clone(),
                        deps: vec![],
                    },
                    data: json!({ "channel": ch, "values": values }),
                    basis: json!({ "events_ref": self.event_ref.to_string(),
                                   "coordinate_basis": self.event_file.coordinate_basis,
                                   "rules": RULES_VERSION }),
                },
            );
        }

        // 2) 补偿节点
        let comp = self.evaluate_compensation();

        // 3) 变换节点（仅对补偿后荧光 + 散射线性通道）
        let transformed = self.evaluate_transforms(&comp);

        // 4) 门与群体
        self.evaluate_gates_populations(&comp, &transformed);
        Ok(())
    }

    fn raw_numbers(&self) -> BTreeMap<String, Vec<f64>> {
        let mut m = BTreeMap::new();
        for ch in &FLUO {
            let vals: Vec<f64> = self
                .event_file
                .events
                .iter()
                .map(|c| as_number(c.values.get(*ch).unwrap_or(&Value::Null)).unwrap_or(0.0))
                .collect();
            m.insert(ch.to_string(), vals);
        }
        m
    }

    fn evaluate_compensation(&mut self) -> CompResult {
        let s = &self.spill.spillover;
        let rep = inspect(s);
        let subject = format!("compensation:{}", self.comp.id);
        let decision = self.state.decisions.get(&subject);
        let mut candidates: Vec<Candidate> = vec![];
        let raws = self.raw_numbers();

        // 显式逆矩阵候选（材料中给出的解释，独立版本化）
        if let Some(inv) = &self.comp.inverse {
            let legal = rep.classification == "invertible";
            let values = self.comp_values(|x| mat_vec(inv, x), &raws);
            candidates.push(Candidate {
                id: format!("explicit@{}", self.comp_ref.version),
                kind: "explicit-inverse".into(),
                label: format!("材料显式逆矩阵 {}", self.comp_ref.version),
                legal,
                values,
                rationale: format!("det={:.3e} rcond={:.3e}", rep.determinant, rep.rcond_1),
            });
        }

        match rep.classification.as_str() {
            "invertible" => {
                if let Some(inv) = &rep.inverse {
                    let values = self.comp_values(|x| mat_vec(inv, x), &raws);
                    candidates.push(Candidate {
                        id: "auto-inverse".into(),
                        kind: "inverse".into(),
                        label: "解析逆矩阵（安全求逆）".into(),
                        legal: true,
                        values,
                        rationale: format!(
                            "det={:.6e}; cond1={:.3}; rcond={:.3e} >= {}",
                            rep.determinant, rep.condition_1, rep.rcond_1, NEAR_SINGULAR_RCOND
                        ),
                    });
                }
            }
            "near-singular" => {
                // 绝不默默求逆：保留全部合法候选，等待人工裁定。
                let pv: Vec<Vec<f64>> = self
                    .event_file
                    .events
                    .iter()
                    .map(|_| vec![])
                    .collect();
                let _ = pv;
                let values_pinv = self.comp_values(|x| pinv_apply(s, x).unwrap_or_default(), &raws);
                candidates.push(Candidate {
                    id: "pinv".into(),
                    kind: "pseudoinverse".into(),
                    label: "Moore–Penrose 伪逆（最小二乘候选）".into(),
                    legal: true,
                    values: values_pinv,
                    rationale: format!(
                        "rcond={:.3e} < {}：数值逆不可信，显式标注为伪逆候选",
                        rep.rcond_1, NEAR_SINGULAR_RCOND
                    ),
                });
                let lam = tikhonov_lambda(s);
                let values_tik =
                    self.comp_values(|x| tikhonov_apply(s, x, lam).unwrap_or_default(), &raws);
                candidates.push(Candidate {
                    id: "tikhonov".into(),
                    kind: "regularized".into(),
                    label: format!("Tikhonov 正则逆 (λ={:.2e})", lam),
                    legal: true,
                    values: values_tik,
                    rationale: "正则化候选；偏差-方差权衡需人工确认".into(),
                });
            }
            _ => {
                // 精确奇异：无数值逆候选，只能选择不补偿或修订来源。
                self.diagnostics.push(json!({
                    "scope": "compensation", "severity": "blocked",
                    "message": "spillover 矩阵线性相关，补偿被阻断，等待来源修订",
                    "det": rep.determinant, "subject": subject,
                }));
            }
        }
        // 不补偿候选始终合法（用于保留可读结果）
        let values_none = self.comp_values(|x| x.to_vec(), &raws);
        candidates.push(Candidate {
            id: "none-inversion".into(),
            kind: "none".into(),
            label: "不应用补偿（保留未补偿信号）".into(),
            legal: true,
            values: values_none,
            rationale: "合法回退候选；不声称完成光谱解混".into(),
        });

        let adopted = decision.and_then(|d| {
            if candidates.iter().any(|c| c.id == d.candidate) {
                Some(d.candidate.clone())
            } else {
                None
            }
        });
        // 可逆且无裁定时，自动解析逆作为默认（材料未显式给逆时）
        let adopted = adopted.or_else(|| {
            if rep.classification == "invertible" {
                if self.comp.inverse.is_some() {
                    Some(format!("explicit@{}", self.comp_ref.version))
                } else {
                    Some("auto-inverse".into())
                }
            } else {
                None
            }
        });

        CompResult {
            subject,
            classification: rep.classification,
            determinant: rep.determinant,
            condition_1: rep.condition_1,
            rcond_1: rep.rcond_1,
            adopted,
            candidates,
        }
    }

    fn comp_values<F: Fn(&[f64]) -> Vec<f64>>(
        &self,
        f: F,
        raws: &BTreeMap<String, Vec<f64>>,
    ) -> Vec<CellValue> {
        self.event_file
            .events
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let v: Vec<f64> = FLUO
                    .iter()
                    .map(|ch| *raws.get(*ch).and_then(|x| x.get(i)).unwrap_or(&0.0))
                    .collect();
                let out = f(&v);
                // 标量结果只承载 FITC 通道占位；完整向量在 basis 中
                CellValue {
                    event_id: cell.id.clone(),
                    value: out.first().copied(),
                    raw: serde_json::to_value(&out).unwrap_or(Value::Null),
                    diagnostic: None,
                }
            })
            .collect()
    }

    fn compensated_matrix(&self, comp: &CompResult) -> Option<Vec<Vec<f64>>> {
        let cand = comp.candidates.iter().find(|c| Some(c.id.clone()) == comp.adopted)?;
        cand.values
            .iter()
            .map(|cv| {
                let arr: Vec<f64> =
                    serde_json::from_value(cv.raw.clone()).unwrap_or_default();
                Some(arr)
            })
            .collect::<Option<Vec<_>>>()
    }

    fn evaluate_transforms(&mut self, comp: &CompResult) -> BTreeMap<String, Vec<Option<f64>>> {
        // channel -> 每个事件的变换后坐标（None=无法计算，保留诊断，不丢弃）
        let mut coords: BTreeMap<String, Vec<Option<f64>>> = BTreeMap::new();
        let adopted_matrix = self.compensated_matrix(comp);
        for am in &self.transform.axes {
            let ch = &am.channel;
            let is_fluo = FLUO.contains(&ch.as_str());
            let mut vals: Vec<Option<f64>> = vec![];
            for (i, cell) in self.event_file.events.iter().enumerate() {
                let raw_v = as_number(cell.values.get(ch).unwrap_or(&Value::Null));
                if raw_v.is_none() {
                    self.diagnostics.push(json!({
                        "scope": "transform", "channel": ch, "event": cell.id,
                        "severity": "error",
                        "message": "无法转换的值保留在原位，派生坐标为 null",
                    }));
                    vals.push(None);
                    continue;
                }
                let x = if is_fluo {
                    // 荧光通道：先补偿（采用钉住的候选），再变换
                    match &adopted_matrix {
                        Some(m) => m.get(i).and_then(|row| {
                            FLUO.iter().position(|c| c == ch).and_then(|j| row.get(j).copied())
                        }),
                        None => None,
                    }
                } else {
                    raw_v
                };
                match x {
                    Some(n) if n.is_finite() => {
                        let y = match self.transform.method.as_str() {
                            "linear" => n,
                            "asinh" => (n / self.transform.cofactor.unwrap_or(150.0)).asinh(),
                            "log10" if n > 0.0 => n.log10(),
                            "log10" => {
                                self.diagnostics.push(json!({
                                    "scope": "transform", "channel": ch, "event": cell.id,
                                    "severity": "error",
                                    "message": "log10 遇到非正值，坐标为 null（不静默裁剪）",
                                    "value": n,
                                }));
                                f64::NAN
                            }
                            _ => n,
                        };
                        if y.is_finite() {
                            vals.push(Some(y));
                        } else {
                            vals.push(None);
                        }
                    }
                    _ => vals.push(None),
                }
            }
            coords.insert(ch.clone(), vals);

            let basis_source = if is_fluo {
                format!("{}|{}", comp.subject, comp.adopted.clone().unwrap_or_else(|| "pending".into()))
            } else {
                self.event_ref.to_string()
            };
            let cfg = format!(
                "{}|{}|{}|{}|{}",
                basis_source,
                self.transform_ref_dbg(),
                self.transform.method,
                self.transform.cofactor.unwrap_or(0.0),
                ch
            );
            let hash = stable_id("cfg", &[&cfg]);
            let id = stable_id("xform", &[
                self.transform_ref_dbg().as_str(),
                ch,
                comp.adopted.as_deref().unwrap_or("pending"),
            ]);
            let values_json: Vec<Value> = self
                .event_file
                .events
                .iter()
                .zip(&coords[ch])
                .map(|(cell, v)| json!({
                    "event": cell.id,
                    "value": v,
                    "id": stable_id("xval", &[id.as_str(), &cell.id]),
                }))
                .collect();
            self.nodes.insert(
                id.clone(),
                NodeView {
                    node: Node {
                        id,
                        kind: "transform-channel".into(),
                        label: format!(
                            "{} 变换 {} → {}",
                            self.transform.method, ch, self.transform.output_unit
                        ),
                        cfg_hash: hash,
                        deps: if is_fluo {
                            vec![stable_id("comp", &[self.comp_ref.to_string().as_str()])]
                        } else {
                            vec![stable_id("raw", &[self.event_ref.to_string().as_str(), ch])]
                        },
                    },
                    data: json!({ "channel": ch, "values": values_json,
                                  "unit": self.transform.output_unit }),
                    basis: json!({
                        "transform_ref": self.transform_ref_dbg(),
                        "source": basis_source,
                        "method": self.transform.method,
                        "cofactor": self.transform.cofactor,
                        "rules": RULES_VERSION,
                    }),
                },
            );
        }
        coords
    }

    fn transform_ref_dbg(&self) -> String {
        // 当前生效 transform 引用（从 state 查，评估器内统一用 id）
        let v = self.state.mat_version("transform", &self.transform.id).unwrap_or(self.transform.version);
        format!("transform:{}@v{}", self.transform.id, v)
    }

    fn gate_coords(
        &self,
        gate: &Gate,
        coords: &BTreeMap<String, Vec<Option<f64>>>,
    ) -> Vec<Option<[f64; 2]>> {
        self.event_file
            .events
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let get = |ax: &GateAxis| -> Option<f64> {
                    match ax.space.as_str() {
                        "linear" => as_number(
                            self.event_file.events[i]
                                .values
                                .get(&ax.channel)
                                .unwrap_or(&Value::Null),
                        ),
                        "transformed" => coords.get(&ax.channel).and_then(|v| v.get(i).copied()).flatten(),
                        _ => None,
                    }
                };
                Some([get(&gate.x)?, get(&gate.y)?])
            })
            .collect()
    }

    fn evaluate_gates_populations(
        &mut self,
        comp: &CompResult,
        coords: &BTreeMap<String, Vec<Option<f64>>>,
    ) {
        let comp_blocked =
            comp.classification != "invertible" && comp.adopted.is_none();
        let comp_hash = stable_id(
            "cfg",
            &[
                "comp",
                &self.comp_ref.to_string(),
                comp.adopted.as_deref().unwrap_or("pending"),
                RULES_VERSION,
            ],
        );

        // gate 节点
        let mut gate_nodes: BTreeMap<String, String> = BTreeMap::new();
        for (pop_id, (gref, gate)) in &self.gates {
            let pts = self.gate_coords(gate, coords);
            let memberships: Vec<Value> = pts
                .iter()
                .enumerate()
                .map(|(i, pt)| {
                    let cell = &self.event_file.events[i];
                    let inside = pt.map(|p| point_in_polygon(p, &gate.polygon));
                    json!({
                        "event": cell.id,
                        "inside": inside,
                        "coordinate": pt,
                        "id": stable_id("gmem", &[gref.to_string().as_str(), &cell.id]),
                    })
                })
                .collect();
            let tf_dep_x = if gate.x.space == "transformed" {
                stable_id("xform", &[
                    self.transform_ref_dbg().as_str(),
                    &gate.x.channel,
                    comp.adopted.as_deref().unwrap_or("pending"),
                ])
            } else {
                stable_id("raw", &[self.event_ref.to_string().as_str(), &gate.x.channel])
            };
            let tf_dep_y = if gate.y.space == "transformed" {
                stable_id("xform", &[
                    self.transform_ref_dbg().as_str(),
                    &gate.y.channel,
                    comp.adopted.as_deref().unwrap_or("pending"),
                ])
            } else {
                stable_id("raw", &[self.event_ref.to_string().as_str(), &gate.y.channel])
            };
            let cfg = format!(
                "{}|{}|{}|{}|{}",
                gref,
                self.transform_ref_dbg(),
                self.event_ref,
                serde_json::to_string(&gate.polygon).unwrap_or_default(),
                gate.boundary_rule
            );
            let hash = stable_id("cfg", &[&cfg]);
            let id = stable_id("gate", &[gref.to_string().as_str()]);
            gate_nodes.insert(pop_id.clone(), id.clone());
            self.nodes.insert(
                id.clone(),
                NodeView {
                    node: Node {
                        id,
                        kind: "gate".into(),
                        label: format!("门 {} {}", gref, gate.name),
                        cfg_hash: hash,
                        deps: vec![tf_dep_x, tf_dep_y],
                    },
                    data: json!({
                        "gate_ref": gref.to_string(),
                        "name": gate.name,
                        "polygon": gate.polygon,
                        "x_axis": gate.x,
                        "y_axis": gate.y,
                        "memberships": memberships,
                        "compensation_blocked": comp_blocked,
                    }),
                    basis: json!({
                        "gate_ref": gref.to_string(),
                        "transform": self.transform_ref_dbg(),
                        "events": self.event_ref.to_string(),
                        "boundary_rule": gate.boundary_rule,
                        "comp_cfg": comp_hash,
                        "rules": RULES_VERSION,
                    }),
                },
            );
        }

        // 群体节点：分母钉住父群版本
        let root_count = self.event_file.events.len();
        // 父集合递归
        let mut membership_sets: BTreeMap<String, std::collections::BTreeSet<String>> =
            BTreeMap::new();
        membership_sets.insert(
            "__root__".into(),
            self.event_file.events.iter().map(|c| c.id.clone()).collect(),
        );
        let pop_by_id: BTreeMap<String, &Population> =
            self.lineage.populations.iter().map(|p| (p.id.clone(), p)).collect();
        // 按 lineage 声明顺序（父子已在案例中排好序）
        for pop in &self.lineage.populations {
            let mut set = std::collections::BTreeSet::new();
            if let Some((gref, gate)) = self.gates.get(&pop.id) {
                let node = &self.nodes[&gate_nodes[&pop.id]];
                let parent_set = pop
                    .parent
                    .as_ref()
                    .and_then(|pid| membership_sets.get(pid))
                    .cloned()
                    .unwrap_or_else(|| {
                        self.event_file.events.iter().map(|c| c.id.clone()).collect()
                    });
                for m in node.data["memberships"].as_array().unwrap() {
                    let eid = m["event"].as_str().unwrap();
                    if m["inside"].as_bool() == Some(true) && parent_set.contains(eid) {
                        set.insert(eid.to_string());
                    }
                }
                let parent_ref = pop.parent.as_deref().map(|pid| {
                    format!(
                        "population:{}@{}",
                        pid,
                        self.state
                            .mat_version("lineage", &self.lineage.id)
                            .unwrap_or(self.lineage.version)
                    )
                });
                // 分母：父群在同一配置下的成员数（钉住父群版本与事件版本）
                let denom = parent_set.len();
                let num = set.len();
                let percent = if denom == 0 {
                    None
                } else {
                    Some(100.0 * num as f64 / denom as f64)
                };
                let cfg = format!(
                    "pop|{}|{}|{}|{}",
                    self.lineage_ref, gref, self.event_ref, pop.parent.as_deref().unwrap_or("__root__")
                );
                let hash = stable_id("cfg", &[&cfg]);
                let id = stable_id("pop", &[self.lineage_ref.to_string().as_str(), &pop.id]);
                let gate_dep = gate_nodes[&pop.id].clone();
                let deps = match &pop.parent {
                    Some(pid) => vec![gate_dep, stable_id("pop", &[self.lineage_ref.to_string().as_str(), pid])],
                    None => vec![gate_dep],
                };
                let gate_name = gate.name.clone();
                self.nodes.insert(
                    id,
                    NodeView {
                        node: Node {
                            id: stable_id("pop", &[self.lineage_ref.to_string().as_str(), &pop.id]),
                            kind: "population".into(),
                            label: format!("群体 {}（门 {}）", pop.name, gate_name),
                            cfg_hash: hash,
                            deps,
                        },
                        data: json!({
                            "population_id": pop.id,
                            "name": pop.name,
                            "gate_ref": gref.to_string(),
                            "parent": pop.parent,
                            "numerator": num,
                            "denominator": denom,
                            "percent": percent,
                            "members": set.iter().collect::<Vec<_>>(),
                            "denominator_source": {
                                "population": parent_ref,
                                "root_count": root_count,
                                "events_ref": self.event_ref.to_string(),
                                "lineage_ref": self.lineage_ref.to_string(),
                                "pinned": true,
                            },
                            "compensation_blocked": comp_blocked,
                        }),
                        basis: json!({
                            "lineage_ref": self.lineage_ref.to_string(),
                            "gate_ref": gref.to_string(),
                            "events_ref": self.event_ref.to_string(),
                            "parent_version_pinned": true,
                            "rules": RULES_VERSION,
                        }),
                    },
                );
            }
            membership_sets.insert(pop.id.clone(), set);
        }

        // 补偿主节点
        let candidates_json: Vec<Value> = comp
            .candidates
            .iter()
            .map(|c| {
                json!({
                    "id": c.id, "kind": c.kind, "label": c.label,
                    "legal": c.legal, "rationale": c.rationale,
                    "values": c.values.iter().map(|cv| json!({
                        "event": cv.event_id,
                        "vector": cv.raw,
                        "id": stable_id("cval", &[
                            self.comp_ref.to_string().as_str(),
                            &c.id, &cv.event_id]),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let id = stable_id("comp", &[self.comp_ref.to_string().as_str()]);
        self.nodes.insert(
            id.clone(),
            NodeView {
                node: Node {
                    id,
                    kind: "compensation".into(),
                    label: format!("补偿 {}（引用 {}）", self.comp_ref, self.comp.spillover_ref),
                    cfg_hash: comp_hash.clone(),
                    deps: vec![],
                },
                data: json!({
                    "subject": comp.subject,
                    "spillover_ref": self.comp.spillover_ref.to_string(),
                    "classification": comp.classification,
                    "determinant": comp.determinant,
                    "condition_1": comp.condition_1,
                    "rcond_1": comp.rcond_1,
                    "adopted": comp.adopted,
                    "candidates": candidates_json,
                    "threshold_rcond": NEAR_SINGULAR_RCOND,
                }),
                basis: json!({
                    "comp_ref": self.comp_ref.to_string(),
                    "spillover_ref": self.comp.spillover_ref.to_string(),
                    "rules": RULES_VERSION,
                    "rule": "近奇异矩阵不得默默求逆；全部合法候选保留至人工裁定",
                }),
            },
        );
        let _ = pop_by_id;
    }
}
