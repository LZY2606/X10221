//! 审阅操作命令：每条命令产生新事件；视图层操作不改写原始数据。

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::app::commands::*;
use crate::app::service::{err, Service, SrvResult};
use crate::domain::derive::derive_tree;
use crate::domain::model::DenominatorRef;
use crate::app::service_import::gate_content_rev;
use crate::domain::events::{EventKind, Judgement};
use crate::domain::gate::{CoordBasis, Point, PolygonGate};
use crate::domain::ids::{content_rev_parts, stable_id};
use crate::domain::matrix::{invert_spillover, InversionStatus, RIDGE_LAMBDA};
use crate::domain::model::*;
use crate::domain::project::*;
use crate::domain::transform::TransformSpec;
use crate::casepack;

const LIVE_AT: &str = "2026-09-25T12:00:00+09:00";

impl Service {
    fn with_branch_state<F, T>(&self, branch: &str, f: F) -> SrvResult<T>
    where
        F: FnOnce(&mut Connection, &BranchState) -> SrvResult<T>,
    {
        let mut conn = self.conn.lock().unwrap();
        let events = crate::db::list_events(&conn, branch).map_err(|e| err("db", e.0))?;
        let state = replay(branch, &events);
        f(&mut conn, &state)
    }

    // ---------- 补偿矩阵修订（引入近奇异/奇异 spillover） ----------

    pub fn revise_compensation(&self, branch: &str, base: Option<&str>, matrix_id: &str, new_spill: Vec<Vec<f64>>, note: &str, actor: &str) -> SrvResult<Value> {
        self.with_branch_state(branch, |conn, state| {
            let prev = state.matrices.get(matrix_id).ok_or_else(|| err("NOT_FOUND", format!("矩阵 {matrix_id} 不存在")))?;
            let spill_prev = state.spillovers.values().find(|s| s.rev == prev.source_spillover_rev)
                .ok_or_else(|| err("NOT_FOUND", "spillover 元数据缺失"))?;
            let ordinal = prev.ordinal + 1;
            let inv = invert_spillover(&new_spill);
            let status = match inv.status {
                InversionStatus::Ok => "ok",
                InversionStatus::NeedsDecision => "needs_decision",
                InversionStatus::Rejected => "rejected",
            };
            let spill_rev = content_rev_parts(&[
                "spillover",
                &spill_prev.channels.join(","),
                &crate::app::service_import::canon_matrix(&new_spill),
                &spill_prev.single_stain_blobs.join(","),
                &ordinal.to_string(),
            ]);
            let new_spill_meta = SpilloverMeta {
                stable_id: spill_prev.stable_id.clone(),
                ordinal,
                rev: spill_rev.clone(),
                channels: spill_prev.channels.clone(),
                single_stain_blobs: spill_prev.single_stain_blobs.clone(),
                source_note: format!("{}；修订 {}", spill_prev.source_note, note),
            };
            let matrix_rev = content_rev_parts(&[
                "compmatrix", matrix_id, &spill_rev,
                &crate::app::service_import::canon_matrix(&new_spill), status, &ordinal.to_string(),
            ]);
            let new_matrix = CompensationMatrix {
                stable_id: matrix_id.into(),
                ordinal,
                rev: matrix_rev.clone(),
                channels: spill_prev.channels.clone(),
                spillover: new_spill,
                source_spillover_rev: spill_rev,
                rcond: inv.rcond,
                status: status.into(),
                chosen_candidate: if inv.status == InversionStatus::Ok { Some("exact_inverse".into()) } else { None },
                candidates: inv.candidates,
                diagnostics: inv.diagnostics.clone(),
            };
            let mut judgements = vec![Judgement {
                rule_id: "rule.no-silent-inverse".into(),
                status: match inv.status {
                    InversionStatus::Ok => "satisfied".into(),
                    InversionStatus::NeedsDecision => "blocked_pending_adjudication".into(),
                    InversionStatus::Rejected => "rejected".into(),
                },
                message: format!("rcond={:.3e}；{}", inv.rcond, inv.diagnostics.join("；")),
                affected: vec![matrix_id.into()],
            }];
            // 下游群结果全部失效（补偿变了，所有门坐标里的荧光值都受影响）
            let mut stale = Vec::new();
            for gid in &state.gate_order {
                if let Some(gv) = state.latest_gate_version(gid) {
                    stale.push(StaleMark {
                        gate_id: gid.clone(),
                        stale_gate_rev: gv.rev.clone(),
                        because_source_rev: matrix_rev.clone(),
                        because_kind: "compensation".into(),
                        affected_path: state.ancestry(gid),
                    });
                }
            }
            if !stale.is_empty() {
                judgements.push(judgement_stale_not_overwrite(&stale));
            }

            let payload = json!({
                "spillover": serde_json::to_value(&new_spill_meta).unwrap(),
                "matrix": serde_json::to_value(&new_matrix).unwrap(),
                "stale": serde_json::to_value(&stale).unwrap(),
                "depends_on": [{"kind": "matrix", "rev": prev.rev}],
                "note": note,
            });
            let out = self.append(conn, branch, EventKind::CompensationRevised, LIVE_AT, payload, judgements, base, actor)?;
            Ok(json!({"event_id": out.event_id, "seq": out.seq, "matrix_status": status,
                      "candidates": new_matrix.candidates.iter().map(|c| c.kind.clone()).collect::<Vec<_>>(),
                      "stale_scope": stale.iter().map(|m| m.gate_id.clone()).collect::<Vec<_>>()}))
        })
    }

    /// 近奇异候选裁定：accept(+candidate) 或 reject。
    pub fn adjudicate_compensation(&self, cmd: &AdjudicateComp) -> SrvResult<Value> {
        self.with_branch_state(&cmd.branch, |conn, state| {
            let m = state.matrices.get(&cmd.matrix_id).ok_or_else(|| err("NOT_FOUND", "矩阵不存在"))?;
            let base = cmd.base_head_hash.as_deref();
            let actor = cmd.actor.clone().unwrap_or_else(|| "reviewer".into());
            let adjudication_id = format!("adj#comp#{}", m.ordinal);

            if cmd.decision == "accept" {
                if m.status != "needs_decision" {
                    return Err(err("NOT_NEEDS_DECISION", "只有 needs_decision 的近奇异矩阵需要显式裁定"));
                }
                let cand = cmd.candidate.clone().ok_or_else(|| err("BAD_REQUEST", "accept 需要指定 candidate"))?;
                if !m.candidates.iter().any(|c| c.kind == cand) {
                    return Err(err("BAD_CANDIDATE", format!("候选 {cand} 不是合法候选；无法唯一决定时保留全部合法候选")));
                }
                let payload = json!({
                    "adjudication_id": adjudication_id,
                    "matrix_id": cmd.matrix_id,
                    "decision": "accept",
                    "candidate": cand,
                    "base_rev": m.rev,
                    "note": cmd.note,
                });
                let judgements = vec![Judgement {
                    rule_id: "rule.no-silent-inverse".into(),
                    status: "adjudicated".into(),
                    message: format!("审阅者显式接受候选 {cand}（此前未做任何默默选择）"),
                    affected: vec![cmd.matrix_id.clone()],
                }];
                let out = self.append(conn, &cmd.branch, EventKind::CompensationAdjudicated, LIVE_AT, payload, judgements, base, &actor)?;
                return Ok(json!({"event_id": out.event_id, "seq": out.seq, "decision": "accept", "candidate": cand}));
            } else if cmd.decision == "reject" {
                let payload = json!({
                    "adjudication_id": adjudication_id,
                    "matrix_id": cmd.matrix_id,
                    "decision": "reject",
                    "base_rev": m.rev,
                    "note": cmd.note,
                });
                let judgements = vec![Judgement {
                    rule_id: "rule.no-silent-inverse".into(),
                    status: "rejected".into(),
                    message: "审阅者拒绝该补偿矩阵；不产生补偿，候选保留可读".into(),
                    affected: vec![cmd.matrix_id.clone()],
                }];
                let out = self.append(conn, &cmd.branch, EventKind::CompensationAdjudicated, LIVE_AT, payload, judgements, base, &actor)?;
                return Ok(json!({"event_id": out.event_id, "seq": out.seq, "decision": "reject"}));
            }
            Err(err("BAD_REQUEST", "decision 必须是 accept 或 reject"))
        })
    }
}

impl Service {
    /// 修订门：改名（身份不变）/ 改多边形 / 重挂父门。下游结果标过期不覆盖。
    pub fn revise_gate(&self, cmd: &ReviseGate) -> SrvResult<Value> {
        self.with_branch_state(&cmd.branch, |conn, state| {
            let latest = state.latest_gate_version(&cmd.gate_id)
                .ok_or_else(|| err("NOT_FOUND", format!("门 {} 不存在", cmd.gate_id)))?
                .gate.clone();
            let ordinal = latest.ordinal + 1;
            let name = cmd.new_name.clone().unwrap_or(latest.name.clone());
            let parent = cmd.new_parent.clone().or(latest.parent_gate_id.clone());
            let polygon_raw = cmd.polygon.as_ref();
            let points: Vec<(f64, f64)> = match polygon_raw {
                Some(pts) => pts.iter().map(|p| (p.x, p.y)).collect(),
                None => latest.polygon.iter().map(|p| (p.x, p.y)).collect(),
            };
            if points.len() < 3 {
                return Err(err("BAD_GATE", "多边形至少需要 3 个顶点；本次修订回滚"));
            }
            // 防止成环
            if let Some(p) = &parent {
                if p == &cmd.gate_id {
                    return Err(err("CYCLE", "门不能以自身为父"));
                }
                let chain = state.ancestry(p);
                if chain.contains(&cmd.gate_id) {
                    return Err(err("CYCLE", "重挂父门会造成谱系环"));
                }
            }
            let rev = gate_content_rev(&cmd.gate_id, ordinal, &name, parent.as_deref(), &points);
            let transform_rev_for = |ch: &str| state.transforms.values().find(|t| t.channel == ch).map(|t| t.rev.clone());
            let gate = PolygonGate {
                stable_id: cmd.gate_id.clone(),
                name,
                parent_gate_id: parent,
                basis: CoordBasis {
                    x_channel: latest.basis.x_channel.clone(),
                    y_channel: latest.basis.y_channel.clone(),
                    x_transform_rev: transform_rev_for(&latest.basis.x_channel),
                    y_transform_rev: transform_rev_for(&latest.basis.y_channel),
                },
                polygon: points.iter().map(|(x, y)| Point { x: *x, y: *y }).collect(),
                ordinal,
                rev: rev.clone(),
            };

            // 失效范围：后代门（不包含被修订门自身——它有新定义，等待重新派生）
            let descendants = state.descendants_of(&cmd.gate_id);
            let mut stale = Vec::new();
            for gid in &descendants {
                if let Some(gv) = state.latest_gate_version(gid) {
                    stale.push(StaleMark {
                        gate_id: gid.clone(),
                        stale_gate_rev: gv.rev.clone(),
                        because_source_rev: rev.clone(),
                        because_kind: "gate".into(),
                        affected_path: state.ancestry(gid),
                    });
                }
            }
            // 被修订门自身的旧结果也标过期（旧值保留，不覆盖）
            stale.push(StaleMark {
                gate_id: cmd.gate_id.clone(),
                stale_gate_rev: latest.rev.clone(),
                because_source_rev: rev.clone(),
                because_kind: "gate-self".into(),
                affected_path: vec![cmd.gate_id.clone()],
            });

            let mut judgements = vec![];
            if cmd.new_name.is_some() {
                judgements.push(Judgement {
                    rule_id: "rule.identity-stable-on-rename".into(),
                    status: "satisfied".into(),
                    message: format!("门改名但稳定身份 {} 不变，父子引用不中断", cmd.gate_id),
                    affected: vec![cmd.gate_id.clone()],
                });
            }
            judgements.push(judgement_stale_not_overwrite(&stale));

            let payload = json!({
                "gate": serde_json::to_value(&GateVersion { rev, gate }).unwrap(),
                "stale": serde_json::to_value(&stale).unwrap(),
                "depends_on": [{"kind": "gate", "rev": latest.rev}],
                "note": cmd.note,
            });
            let actor = cmd.actor.clone().unwrap_or_else(|| "reviewer".into());
            let out = self.append(conn, &cmd.branch, EventKind::GateRevised, LIVE_AT, payload, judgements, cmd.base_head_hash.as_deref(), &actor)?;
            Ok(json!({"event_id": out.event_id, "seq": out.seq, "ordinal": ordinal,
                      "stale_scope": stale.iter().map(|m| m.gate_id.clone()).collect::<Vec<_>>()}))
        })
    }

    /// 边界点显式裁定并重新派生该分支群结果（新事件，旧结论保留）。
    pub fn adjudicate_boundary(&self, cmd: &BoundaryCall) -> SrvResult<Value> {
        self.with_branch_state(&cmd.branch, |conn, state| {
            let gate = state.gate(&cmd.gate_id).ok_or_else(|| err("NOT_FOUND", "门不存在"))?;
            if !gate.polygon.iter().any(|p| true) {
                return Err(err("BAD_GATE", "门无多边形"));
            }
            let include = match cmd.call.as_str() {
                "include" => true,
                "exclude" => false,
                _ => return Err(err("BAD_REQUEST", "call 必须是 include/exclude")),
            };
            // 收集该分支已有的边界裁定
            let mut decisions: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
            for ev in &state.events {
                if ev.kind == EventKind::BoundaryAdjudicated.as_str() {
                    if let (Some(gid), Some(eid), Some(c)) = (
                        ev.payload.get("gate_id").and_then(|v| v.as_str()),
                        ev.payload.get("event_id").and_then(|v| v.as_str()),
                        ev.payload.get("include").and_then(|v| v.as_bool()),
                    ) {
                        let aid0 = ev.payload.get("adjudication_id").and_then(|v| v.as_str()).unwrap_or("");
                        if state.undone.contains(aid0) {
                            continue;
                        }
                        decisions.entry(gid.to_string()).or_default().insert(eid.to_string(), c);
                    }
                }
            }
            decisions.entry(cmd.gate_id.clone()).or_default().insert(cmd.event_id.clone(), include);

            let matrix = state.active_matrix();
            if matrix.is_none() {
                return Err(err("NO_COMPENSATION", "补偿矩阵尚未可用（近奇异未裁定/已拒绝），不能派生荧光群"));
            }

            let pops = crate::domain::derive::derive_tree(state, &decisions);
            let mut judgements = Vec::new();
            for p in &pops {
                judgements.push(judgement_denominator_pinned(p));
            }
            judgements.push(Judgement {
                rule_id: "rule.boundary-explicit".into(),
                status: "adjudicated".into(),
                message: format!("边界事件 {} 在 {} 上被显式{}", cmd.event_id, cmd.gate_id,
                                 if include { "计入" } else { "排除" }),
                affected: vec![cmd.gate_id.clone(), cmd.event_id.clone()],
            });
            let adjudication_id = format!("adj#boundary#{}#{}", cmd.gate_id.replace('#', "_"), cmd.event_id.replace('#', "_"));
            let payload = json!({
                "adjudication_id": adjudication_id,
                "gate_id": cmd.gate_id,
                "event_id": cmd.event_id,
                "include": include,
                "populations": serde_json::to_value(&pops).unwrap(),
                "note": cmd.note,
            });
            let actor = cmd.actor.clone().unwrap_or_else(|| "reviewer".into());
            let out = self.append(conn, &cmd.branch, EventKind::BoundaryAdjudicated, LIVE_AT, payload, judgements, cmd.base_head_hash.as_deref(), &actor)?;
            let target = pops.iter().find(|p| p.gate_id == cmd.gate_id);
            Ok(json!({"event_id": out.event_id, "seq": out.seq,
                      "count": target.map(|p| p.count), "percent": target.map(|p| p.percent)}))
        })
    }

    /// 重算派生（补偿裁定后调用）：新 PopulationsDerived 事件，分母钉版。
    pub fn derive_now(&self, branch: &str, base: Option<&str>, actor: &str) -> SrvResult<Value> {
        self.with_branch_state(branch, |conn, state| {
            if state.active_matrix().is_none() {
                return Err(err("NO_COMPENSATION", "补偿矩阵未就绪"));
            }
            // 携带已接受的边界裁定
            let mut decisions: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
            for ev in &state.events {
                if ev.kind == EventKind::BoundaryAdjudicated.as_str() {
                    let aid = ev.payload.get("adjudication_id").and_then(|v| v.as_str()).unwrap_or("");
                    if state.undone.contains(aid) {
                        continue;
                    }
                    if let (Some(gid), Some(eid), Some(c)) = (
                        ev.payload.get("gate_id").and_then(|v| v.as_str()),
                        ev.payload.get("event_id").and_then(|v| v.as_str()),
                        ev.payload.get("include").and_then(|v| v.as_bool()),
                    ) {
                        decisions.entry(gid.to_string()).or_default().insert(eid.to_string(), c);
                    }
                }
            }
            let pops = crate::domain::derive::derive_tree(state, &decisions);
            let judgements: Vec<Judgement> = pops.iter().map(judgement_denominator_pinned).collect();
            let payload = json!({"populations": serde_json::to_value(&pops).unwrap()});
            let out = self.append(conn, branch, EventKind::PopulationsDerived, LIVE_AT, payload, judgements, base, actor)?;
            Ok(json!({"event_id": out.event_id, "seq": out.seq,
                      "populations": pops.iter().map(|p| json!({"gate_id": p.gate_id, "count": p.count, "percent": p.percent}))
                                             .collect::<Vec<_>>()}))
        })
    }

    /// 创建草案分支（两个窗口并排）。
    pub fn fork_draft(&self, cmd: &ForkDraft) -> SrvResult<Value> {
        let mut conn = self.conn.lock().unwrap();
        let events = crate::db::list_events(&conn, &cmd.branch).map_err(|e| err("db", e.0))?;
        if events.is_empty() {
            return Err(err("NOT_FOUND", "源分支为空，无法分叉"));
        }
        if crate::db::head_hash(&conn, &cmd.new_branch).map_err(|e| err("db", e.0))?.is_some() {
            return Err(err("DRAFT_EXISTS", format!("草案 {} 已存在", cmd.new_branch)));
        }
        // 复制历史事件（保持 event_id/seq/hash 不变），组成新链
        let actor = cmd.actor.clone().unwrap_or_else(|| "reviewer".into());
        for ev in &events {
            crate::db::append_event(
                &mut conn, &cmd.new_branch, &ev.kind, &ev.at, &ev.payload,
                &serde_json::to_value(&ev.judgements).unwrap_or(json!([])),
                if ev.seq == 1 { None } else { Some(&events[(ev.seq - 2) as usize].hash) },
                &actor,
                |_, eid, prev, _, _| {
                    // 复制时强制使用原 hash：通过让 event_id/prev 与原始一致并重建
                    let _ = (eid, prev);
                    ev.hash.clone()
                },
            ).map_err(|e| err("db", e.0))?;
        }
        let base_seq = events.last().unwrap().seq;
        // 分叉事件（新分支第 base_seq+1 个事件）
        let payload = json!({
            "forked_branch": cmd.branch,
            "at_seq": base_seq,
            "label": cmd.label,
        });
        let out = self.append(&mut conn, &cmd.new_branch, EventKind::DraftForked, LIVE_AT, payload, vec![],
            cmd.base_head_hash.as_deref(), &actor)?;
        crate::db::create_draft(&mut conn, &cmd.new_branch, &cmd.branch, base_seq, &cmd.label, LIVE_AT)
            .map_err(|e| err("db", e.0))?;
        Ok(json!({"branch": cmd.new_branch, "base_seq": base_seq, "event_id": out.event_id}))
    }

    /// 撤销裁定：生成 decision_undone 事件，并把撤销前的派生结果原样重放一次，
    /// 使视图恢复前一分支结论（历史事件均保留）。
    pub fn undo_decision(&self, cmd: &UndoCall) -> SrvResult<Value> {
        self.with_branch_state(&cmd.branch, |conn, state| {
            if state.undone.contains(&cmd.adjudication_id) {
                return Err(err("ALREADY_UNDONE", "该裁定已撤销"));
            }
            let actor = cmd.actor.clone().unwrap_or_else(|| "reviewer".into());
            // 找到目标裁定事件
            let target = state.events.iter().find(|e| {
                e.payload.get("adjudication_id").and_then(|v| v.as_str()) == Some(cmd.adjudication_id.as_str())
            });
            let target = match target {
                Some(t) => t,
                None => return Err(err("NOT_FOUND", "找不到要撤销的裁定")),
            };
            let mut judgements = vec![Judgement {
                rule_id: "rule.undo-restores-branch".into(),
                status: "satisfied".into(),
                message: format!("撤销 {}：恢复前一分支的依赖版本与可见结论，不删除任何历史事件", cmd.adjudication_id),
                affected: vec![cmd.adjudication_id.clone()],
            }];

            let payload = json!({
                "adjudication_id": cmd.adjudication_id,
                "undone_event_id": target.event_id,
            });
            let out = self.append(conn, &cmd.branch, EventKind::DecisionUndone, LIVE_AT, payload.clone(),
                judgements.clone(), cmd.base_head_hash.as_deref(), &actor)?;

            // 撤销后重放至撤销前一刻，重新派生一次以恢复旧结论（作为新派生事件，旧结论不覆盖）
            let mut pre_events = state.events.clone();
            pre_events.retain(|e| e.seq < target.seq);
            let pre = replay(&cmd.branch, &pre_events);
            if pre.active_matrix().is_some() {
                let pops = crate::domain::derive::derive_tree(&pre, &BTreeMap::new());
                let j2: Vec<Judgement> = pops.iter().map(judgement_denominator_pinned).collect();
                judgements.extend(j2);
                let p2 = json!({"populations": serde_json::to_value(&pops).unwrap(),
                                "restored_from_event": target.event_id});
                let _ = self.append(conn, &cmd.branch, EventKind::PopulationsDerived, LIVE_AT, p2,
                    judgements, Some(&out.hash), &actor);
            }
            Ok(json!({"event_id": out.event_id, "seq": out.seq, "undone": cmd.adjudication_id}))
        })
    }
}

impl Service {
    /// 修订某通道变换参数（一次参数修订）。引用该通道变换的门结果标过期。
    pub fn revise_transform(&self, cmd: &ReviseTransform) -> SrvResult<Value> {
        self.with_branch_state(&cmd.branch, |conn, state| {
            let prev = state.transforms.values().find(|t| t.channel == cmd.channel)
                .ok_or_else(|| err("NOT_FOUND", format!("通道 {} 无变换参数", cmd.channel)))?;
            let spec = match &cmd.spec {
                TransformIn::Linear { offset, scale, unit } => TransformSpec::Linear { offset: *offset, scale: *scale, unit: unit.clone() },
                TransformIn::ArcSinh { cofactor, unit } => TransformSpec::ArcSinh { cofactor: *cofactor, unit: unit.clone() },
            };
            if let (None, Some(diag)) = (spec.apply(1.0).0, spec.apply(1.0).1) {
                return Err(err("BAD_TRANSFORM", format!("变换参数无效：{diag}；修订回滚")));
            }
            let ordinal = prev.ordinal + 1;
            let id = prev.stable_id.clone();
            let rev = content_rev_parts(&["transform", &cmd.channel, &ordinal.to_string(),
                                         &serde_json::to_string(&spec).unwrap_or_default()]);
            let tp = TransformParam { stable_id: id.clone(), ordinal, rev: rev.clone(), channel: cmd.channel.clone(), spec };

            // 失效范围：basis 使用该通道的门
            let mut stale = Vec::new();
            for gid in &state.gate_order {
                let g = match state.gate(gid) { Some(g) => g, None => continue };
                if g.basis.x_channel == cmd.channel || g.basis.y_channel == cmd.channel {
                    for down in state.ancestry(gid) {
                        if down == *ROOT_GATE_ID { continue; }
                        if let Some(gv) = state.latest_gate_version(&down) {
                            let mark = StaleMark {
                                gate_id: down.clone(),
                                stale_gate_rev: gv.rev.clone(),
                                because_source_rev: rev.clone(),
                                because_kind: "transform".into(),
                                affected_path: state.ancestry(&down),
                            };
                            if !stale.contains(&mark) { stale.push(mark); }
                        }
                    }
                }
            }
            let judgements = vec![judgement_stale_not_overwrite(&stale)];
            let payload = json!({
                "transform": serde_json::to_value(&tp).unwrap(),
                "stale": serde_json::to_value(&stale).unwrap(),
                "depends_on": [{"kind": "transform", "rev": prev.rev}],
                "note": cmd.note,
            });
            let actor = cmd.actor.clone().unwrap_or_else(|| "reviewer".into());
            let out = self.append(conn, &cmd.branch, EventKind::TransformRevised, LIVE_AT, payload, judgements,
                cmd.base_head_hash.as_deref(), &actor)?;
            Ok(json!({"event_id": out.event_id, "seq": out.seq, "rev": rev,
                      "stale_scope": stale.iter().map(|m| m.gate_id.clone()).collect::<Vec<_>>()}))
        })
    }
}

const ROOT_GATE_ID: &str = "gate#root";
