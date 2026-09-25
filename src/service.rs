//! 应用编排：把固定案例装入库，并提供逐幕重放动作与审阅视图。
//! 所有动作都产生事件；视图操作不改原数据；裁定可撤销（回到旧事件即恢复旧结论）。

use crate::casepack as cp;
use crate::engine::{self, Bundle, ResolvedMaterials};
use crate::error::{Error, Result};
use crate::event::{AuditEvent, EventKind};
use crate::lineage::LineageDoc;
use crate::materials::{MaterialBody, MaterialEnvelope};
use crate::store::Store;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub struct App {
    pub store: Store,
}

impl App {
    pub fn new(store: Store) -> Self {
        App { store }
    }

    // ---------- 固定案例装载（仅空库） ----------

    pub fn load_fixed_case(&mut self) -> Result<Value> {
        if !self.store.is_empty()? {
            return Err(Error::BadRequest("database already contains a case".into()));
        }
        self.store.ensure_branch(cp::scene::BRANCH_MAIN, "草案 A：基线复核", None)?;
        self.store.ensure_branch(cp::scene::BRANCH_SINGULAR, "草案 B：奇异矩阵反例", None)?;
        self.store.set_branch_adopted(cp::scene::BRANCH_SINGULAR, false)?;

        // 1) 原始 FCS 字节作为 blob；通道值解释正文分开版本化。
        let fcs_bytes = cp::fcs_bytes();
        let blob_id = self.store.put_blob("flow-boundary-suite.fcs", &fcs_bytes)?;
        let fcs = crate::fcs::parse_fcs(&fcs_bytes)?;
        let channels_id = self.put_material(
            "channels",
            "channels",
            &MaterialBody::Channels(crate::materials::RawChannelDoc { fcs: fcs.clone() }),
            Some(&blob_id),
            Some("flow-boundary-suite.fcs"),
            "初始导入：FCS 原始通道值",
        )?;
        self.record(
            cp::scene::BRANCH_MAIN,
            EventKind::MaterialImported,
            "导入 FCS 原始通道值",
            &json!({"material": channels_id, "blob": blob_id, "kind": "channels"}),
            None,
            "platform",
        )?;

        // 2) spillover 元数据与补偿矩阵分开版本化。
        let spill_id = self.put_material(
            "spillover",
            "spillover",
            &MaterialBody::Spillover(cp::spillover()),
            Some(&blob_id),
            Some("$SPILLOVER keyword"),
            "单染对照 spillover 元数据",
        )?;
        let matrix_id = self.put_material(
            "comp_matrix",
            "comp_matrix",
            &MaterialBody::CompMatrix(cp::good_matrix()),
            None,
            None,
            "基线补偿矩阵（良态）",
        )?;
        self.record(cp::scene::BRANCH_MAIN, EventKind::MaterialImported,
            "spillover 元数据与补偿矩阵分开版本化",
            &json!({"materials": [spill_id, matrix_id]}), None, "platform")?;

        // 3) 变换参数。
        let xform_id = self.put_material(
            "transform",
            "transform",
            &MaterialBody::Transform(cp::transforms()),
            None, None, "asinh cofactor=150",
        )?;

        // 4) 四个门多边形，各自独立版本化。
        let mut gate_ids = BTreeMap::new();
        for (gid, def) in [
            ("lym", cp::gate_lym()),
            ("cd3", cp::gate_cd3()),
            ("cd4", cp::gate_cd4()),
            ("cd8", cp::gate_cd8()),
        ] {
            let id = self.put_material(
                "gate",
                &format!("gate_{gid}"),
                &MaterialBody::Gate(cp::gate_wrapper(gid, def)),
                None, None, &format!("门 {gid} v1"),
            )?;
            gate_ids.insert(gid.to_string(), id);
        }

        // 5) 父子关系版本化（节点钉住具体门版本）。
        let mut lin = cp::lineage();
        for node in &mut lin.nodes {
            if let crate::lineage::LineageNode::Gate { id, gate_material_version_id, .. } = node {
                *gate_material_version_id = gate_ids[id].clone();
            }
        }
        lin.validate().map_err(Error::BadRequest)?;
        let lin_id = self.put_material(
            "lineage",
            "lineage",
            &MaterialBody::Lineage(lin.clone()),
            None, None, "门控系谱 v1",
        )?;
        self.record(cp::scene::BRANCH_MAIN, EventKind::MaterialImported,
            "变换参数、门多边形与父子关系分开版本化",
            &json!({"transform": xform_id, "gates": gate_ids, "lineage": lin_id}),
            None, "platform")?;

        // 6) 基线推导（良态矩阵自动唯一逆）。
        let resolved = self.resolve_current(Some(&lin))?;
        let bundle = engine::evaluate(&resolved, None);
        self.persist_bundle(cp::scene::BRANCH_MAIN, &bundle)?;
        self.record(cp::scene::BRANCH_MAIN, EventKind::Derivation,
            "基线推导：补偿+门控+群体百分比",
            &json!({
                "comp_analysis": bundle.analysis_id,
                "compensated": bundle.compensated_id,
                "verdict": format!("{:?}", bundle.verdict),
                "populations": bundle.pop_ids,
                "finding": "MATRIX_INVERTIBLE_UNIQUE",
            }),
            Some(&engine::snapshot_refs(&bundle)), "engine")?;
        self.judgement(cp::scene::BRANCH_MAIN,
            "百分比分母钉住父群版本",
            &json!({
                "rule": "PERCENT_DENOMINATOR_PINNED_TO_PARENT_VERSION",
                "affected_objects": bundle.pop_ids.keys().collect::<Vec<_>>(),
                "status": "satisfied",
            }), &bundle)?;

        // 7) 第二分支：严格奇异矩阵——不产生补偿后结果，结论被保留但分支不采纳。
        let singular_matrix_id = self.put_material(
            "comp_matrix",
            "comp_matrix",
            &MaterialBody::CompMatrix(cp::singular_matrix()),
            None, None, "反例：严格奇异矩阵（行 2 = 2 × 行 1）",
        )?;
        self.record(cp::scene::BRANCH_SINGULAR, EventKind::MaterialImported,
            "导入严格奇异补偿矩阵",
            &json!({"material": singular_matrix_id}), None, "platform")?;
        let resolved_s = self.resolve_with(&lin, &gate_ids, Some(&singular_matrix_id))?;
        let bundle_s = engine::evaluate(&resolved_s, None);
        assert_eq!(bundle_s.verdict, crate::comp::CompVerdict::Singular);
        self.persist_bundle(cp::scene::BRANCH_SINGULAR, &bundle_s)?;
        self.record(cp::scene::BRANCH_SINGULAR, EventKind::Derivation,
            "奇异矩阵：补偿被阻断",
            &json!({"comp_analysis": bundle_s.analysis_id, "compensated": Value::Null,
                    "verdict": "Singular"}),
            Some(&engine::snapshot_refs(&bundle_s)), "engine")?;
        self.judgement(cp::scene::BRANCH_SINGULAR,
            "严格奇异矩阵不能被求逆",
            &json!({
                "rule": "MATRIX_SINGULAR",
                "also": ["COMPENSATION_BLOCKED"],
                "input_fragment": "rows[1] == 2 * rows[0]",
                "affected_objects": ["compensated", "pop:all", "pop:lym", "pop:cd3", "pop:cd4", "pop:cd8"],
                "status": "blocked",
            }), &bundle_s)?;

        Ok(json!({
            "case": cp::CASE_ID,
            "branches": [cp::scene::BRANCH_MAIN, cp::scene::BRANCH_SINGULAR],
            "materials": self.store.list_materials()?,
        }))
    }
}

// ---------- helpers ----------

impl App {
    fn put_material(
        &mut self,
        kind: &str,
        logical: &str,
        body: &MaterialBody,
        blob: Option<&str>,
        source_name: Option<&str>,
        note: &str,
    ) -> Result<String> {
        let seq = self.store.next_material_seq(logical)?;
        let json = serde_json::to_string(body)?;
        self.store.write_material(kind, logical, seq, &json, blob, source_name, note)
    }

    /// 追加事件：base 缺省取分支头。
    pub fn record(
        &mut self,
        branch: &str,
        kind: EventKind,
        summary: &str,
        basis: &Value,
        snapshot: Option<Value>,
        by: &str,
    ) -> Result<AuditEvent> {
        let base = self.store.last_event(branch)?.map(|e| e.id);
        self.store.append_event(
            branch,
            kind,
            summary,
            basis,
            &snapshot.unwrap_or(json!({})),
            base.as_deref(),
            by,
        )
    }

    /// 以显式 base 追加（用于制造同版本并发冲突）。
    pub fn record_based(
        &mut self,
        branch: &str,
        kind: EventKind,
        summary: &str,
        basis: &Value,
        snapshot: &Value,
        base: Option<&str>,
        by: &str,
    ) -> Result<AuditEvent> {
        self.store.append_event(branch, kind, summary, basis, snapshot, base, by)
    }

    fn judgement(&mut self, branch: &str, summary: &str, basis: &Value, bundle: &Bundle) -> Result<AuditEvent> {
        self.record(
            branch,
            EventKind::Judgement,
            summary,
            basis,
            Some(self.full_snapshot(bundle)),
            "rule-engine",
        )
    }

    fn full_snapshot(&self, bundle: &Bundle) -> Value {
        let mut snap = engine::snapshot_refs(bundle);
        let mats: BTreeMap<String, String> = [
            ("channels", "channels"),
            ("spillover", "spillover"),
            ("comp_matrix", "comp_matrix"),
            ("transform", "transform"),
            ("lineage", "lineage"),
        ]
        .into_iter()
        .filter_map(|(key, logical)| {
            self.store.latest_material(logical).ok().flatten().map(|e| (key.to_string(), e.id))
        })
        .collect();
        snap["materials"] = json!(mats);
        snap
    }

    fn persist_bundle(&mut self, branch: &str, bundle: &Bundle) -> Result<()> {
        self.store.save_derived(
            &bundle.analysis_id,
            "comp_analysis",
            None,
            branch,
            &bundle.analysis_wrapper,
        )?;
        if let (Some(id), Some(cw)) = (&bundle.compensated_id, &bundle.compensated) {
            self.store.save_derived(id, "compensated", None, branch, cw)?;
        }
        for (node, id) in &bundle.pop_ids {
            let wrap = bundle.pops.get(node).expect("pop wrapper");
            self.store.save_derived(id, "pop", Some(node), branch, wrap)?;
        }
        Ok(())
    }

    /// 解析分支当前可见材料版本（系谱节点钉住哪版门，就用哪版）。
    pub fn resolve_current(&self, lineage_override: Option<&LineageDoc>) -> Result<ResolvedMaterials> {
        let env = |logical: &str| -> Result<MaterialEnvelope> {
            self.store
                .latest_material(logical)?
                .ok_or_else(|| Error::NotFound(format!("material {logical}")))
        };
        let lin_env = env("lineage")?;
        let lin = match &lin_env.body {
            MaterialBody::Lineage(l) => l.clone(),
            _ => unreachable!(),
        };
        let lin = lineage_override.cloned().unwrap_or(lin);
        let mut gates = BTreeMap::new();
        for node in &lin.nodes {
            if let crate::lineage::LineageNode::Gate { id, gate_material_version_id, .. } = node {
                let envelope = self.store.get_material_envelope(gate_material_version_id)?;
                gates.insert(id.clone(), envelope);
            }
        }
        Ok(ResolvedMaterials {
            channels: env("channels")?,
            spillover: env("spillover")?,
            comp_matrix: env("comp_matrix")?,
            transform: env("transform")?,
            lineage: MaterialEnvelope { body: MaterialBody::Lineage(lin), ..lin_env },
            gates,
        })
    }

    /// 用显式系谱、门映射与矩阵版本解析（第二分支/历史恢复用）。
    pub fn resolve_with(
        &self,
        lin: &LineageDoc,
        gate_ids: &BTreeMap<String, String>,
        matrix_id: Option<&str>,
    ) -> Result<ResolvedMaterials> {
        let env = |logical: &str| -> Result<MaterialEnvelope> {
            self.store
                .latest_material(logical)?
                .ok_or_else(|| Error::NotFound(format!("material {logical}")))
        };
        let lin_env = env("lineage")?;
        let mut gates = BTreeMap::new();
        for node in &lin.nodes {
            if let crate::lineage::LineageNode::Gate { id, .. } = node {
                let mid = gate_ids.get(id).cloned().ok_or_else(|| Error::NotFound(format!("gate map {id}")))?;
                gates.insert(id.clone(), self.store.get_material_envelope(&mid)?);
            }
        }
        let matrix = match matrix_id {
            Some(id) => self.store.get_material_envelope(id)?,
            None => env("comp_matrix")?,
        };
        Ok(ResolvedMaterials {
            channels: env("channels")?,
            spillover: env("spillover")?,
            comp_matrix: matrix,
            transform: env("transform")?,
            lineage: MaterialEnvelope { body: MaterialBody::Lineage(lin.clone()), ..lin_env },
            gates,
        })
    }
}

// ---------- 分幕重放 ----------

impl App {
    /// 幕 1：门嵌套改名。身份与门版本不变，仅产生改名事件，不重算、不失效。
    pub fn scene_rename(&mut self, branch: &str, node_id: &str, new_label: &str) -> Result<Value> {
        let current = self.current_lineage()?;
        let old_label = current.node(node_id).map(|n| n.label().to_string());
        let mut lin = current;
        if !lin.rename(node_id, new_label.to_string()) {
            return Err(Error::NotFound(format!("node {node_id}")));
        }
        lin.validate().map_err(Error::BadRequest)?;
        let id = self.put_material("lineage", "lineage", &MaterialBody::Lineage(lin.clone()),
            None, None, &format!("改名 {node_id}: {old_label:?} -> {new_label}"))?;
        self.record(branch, EventKind::MaterialImported,
            &format!("门嵌套改名 {node_id} -> {new_label}"),
            &json!({"material": id, "node_id": node_id, "old_label": old_label, "new_label": new_label,
                    "identity_preserved": true, "stale_scope": []}),
            None, "reviewer")?;
        Ok(json!({"lineage": id, "stale_scope": []}))
    }

    /// 幕 2：参数修订（变换 cofactor 150 -> 300）。
    /// 失效范围按依赖关系计算：用 FITC/PE/PerCP 坐标的门的下游群。
    pub fn scene_param_revision(&mut self, branch: &str) -> Result<Value> {
        let id = self.put_material("transform", "transform",
            &MaterialBody::Transform(cp::transforms_v2()),
            None, None, "参数修订：FITC cofactor 150->300")?;
        // 所有门的 x 基都在 FITC 上 => 全部门控下游过期（但根群不过期）。
        let lin = self.current_lineage()?;
        let stale_scope: Vec<String> = lin
            .nodes
            .iter()
            .filter_map(|n| match n {
                crate::lineage::LineageNode::Gate { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        self.record(branch, EventKind::MaterialImported, "变换参数修订（cofactor 150->300）",
            &json!({"material": id, "stale_scope": stale_scope, "recomputation_policy": "append_not_overwrite"}),
            None, "reviewer")?;
        self.record(branch, EventKind::Judgement,
            "参数版本变化：旧群体结果标记过期，不覆盖",
            &json!({"rule": "UPSTREAM_REVISION_MARKS_STALE_NO_OVERWRITE",
                    "stale_scope": stale_scope, "status": "stale"}),
            None, "rule-engine")?;
        Ok(json!({"transform": id, "stale_scope": stale_scope}))
    }

    /// 幕 3：近奇异补偿矩阵。保留全部合法候选，不默默求逆；等待人工裁定。
    pub fn scene_near_singular(&mut self, branch: &str) -> Result<Value> {
        let id = self.put_material("comp_matrix", "comp_matrix",
            &MaterialBody::CompMatrix(cp::near_singular_matrix()),
            None, None, "反例：近奇异矩阵（多候选）")?;
        let lin = self.current_lineage()?;
        let gate_ids = self.current_gate_map(&lin)?;
        let resolved = self.resolve_with(&lin, &gate_ids, Some(&id))?;
        let bundle = engine::evaluate(&resolved, None);
        assert_eq!(bundle.verdict, crate::comp::CompVerdict::NearSingular);
        self.persist_bundle(branch, &bundle)?;
        self.record(branch, EventKind::MaterialImported, "导入近奇异补偿矩阵",
            &json!({"material": id}), None, "platform")?;
        self.record(branch, EventKind::Judgement,
            "近奇异补偿矩阵不能被默默求逆",
            &json!({
                "rule": "NEAR_SINGULAR_MATRIX_MULTI_CANDIDATE",
                "also": ["NO_SILENT_INVERSION"],
                "condition_number": bundle.analysis_wrapper.analysis.condition_number,
                "candidate_keys": bundle.analysis_wrapper.analysis.candidates.iter()
                    .map(|c| json!({"key": c.key, "label": c.label, "rationale": c.rationale}))
                    .collect::<Vec<_>>(),
                "status": "awaiting_decision",
            }),
            Some(self.full_snapshot(&bundle)), "rule-engine")?;
        Ok(json!({
            "comp_matrix": id,
            "analysis": bundle.analysis_id,
            "candidates": bundle.analysis_wrapper.analysis.candidates.iter()
                .map(|c| c.key.clone()).collect::<Vec<_>>(),
        }))
    }

    /// 接受一个近奇异候选（人工裁定，产生新事件；可撤销回到未裁定状态）。
    pub fn scene_choose_candidate(&mut self, branch: &str, candidate_key: &str, adopted: bool) -> Result<Value> {
        let lin = self.current_lineage()?;
        let gate_ids = self.current_gate_map(&lin)?;
        let resolved = self.resolve_with(&lin, &gate_ids, None)?;
        let bundle = engine::evaluate(&resolved, Some(candidate_key));
        self.persist_bundle(branch, &bundle)?;
        let decision = if adopted { "accepted" } else { "rejected" };
        let evt = self.record(branch, EventKind::Decision,
            &format!("{decision} 补偿候选 {candidate_key}"),
            &json!({"candidate_key": candidate_key, "decision": decision,
                    "comp_analysis": bundle.analysis_id,
                    "compensated": bundle.compensated_id,
                    "populations": bundle.pop_ids}),
            Some(self.full_snapshot(&bundle)), "reviewer")?;
        Ok(json!({"event": evt.id, "compensated": bundle.compensated_id, "populations": bundle.pop_ids}))
    }

    /// 幕 4：只影响局部依赖的源数据改动——修订 CD3 门（左边界 1.00->1.05）。
    /// 事件 5 从边界点变为门外；下游 cd3/cd4/cd8 过期并追加新结果，旧结果保留。
    pub fn scene_local_gate_change(&mut self, branch: &str) -> Result<Value> {
        let id = self.put_material("gate", "gate_cd3",
            &MaterialBody::Gate(cp::gate_wrapper("cd3", cp::gate_cd3_v2())),
            None, None, "CD3 门修订：左边界 1.00->1.05（事件5变为门外）")?;
        // 更新系谱对 cd3 门版本的引用（其余门不变）。
        let mut lin = self.current_lineage()?;
        for node in &mut lin.nodes {
            if let crate::lineage::LineageNode::Gate { id: nid, gate_material_version_id, .. } = node {
                if nid == "cd3" {
                    *gate_material_version_id = id.clone();
                }
            }
        }
        lin.validate().map_err(Error::BadRequest)?;
        let lin_id = self.put_material("lineage", "lineage",
            &MaterialBody::Lineage(lin.clone()), None, None, "系谱 v2：CD3 门引用升级")?;
        let stale_scope = lin.affected_scope("cd3");
        self.record(branch, EventKind::MaterialImported, "局部源数据改动：CD3 门边界修订",
            &json!({"gate": id, "lineage": lin_id, "stale_scope": stale_scope,
                    "input_fragment": "event[5] at x=1.0 edge -> x<1.05 outside",
                    "affected_branch": "downstream-only"}),
            None, "reviewer")?;
        let gate_ids = self.current_gate_map(&lin)?;
        let resolved = self.resolve_with(&lin, &gate_ids, None)?;
        let bundle = engine::evaluate(&resolved, Some("inverse"));
        self.persist_bundle(branch, &bundle)?;
        self.record(branch, EventKind::Judgement,
            "上游门修订：下游结果标记过期并追加，不覆盖",
            &json!({"rule": "UPSTREAM_REVISION_MARKS_STALE_NO_OVERWRITE",
                    "stale_scope": stale_scope,
                    "new_derived": engine::snapshot_refs(&bundle),
                    "history_retained": true, "status": "appended"}),
            Some(self.full_snapshot(&bundle)), "rule-engine")?;
        Ok(json!({"gate": id, "lineage": lin_id, "stale_scope": stale_scope,
                  "populations": bundle.pop_ids}))
    }

    /// 幕 5：同版本并发修改。两次保存都基于同一 base，第二次得到 409 与双方差异。
    /// 返回 (first_event_id, conflict_result)。
    pub fn scene_concurrent_conflict(&mut self, branch: &str) -> Result<(String, Result<Value>)> {
        let head = self.store.last_event(branch)?.ok_or_else(|| Error::BadRequest("empty branch".into()))?;
        let e1 = self.record_based(branch, EventKind::ViewAction,
            "窗口1：保存候选切换（accepted: inverse）",
            &json!({"window": 1, "selection": "inverse"}), &json!({}),
            Some(&head.id), "reviewer-window-1")?;
        let second = self
            .record_based(branch, EventKind::ViewAction,
                "窗口2：保存候选切换（accepted: pinv_truncated）",
                &json!({"window": 2, "selection": "pinv_truncated"}), &json!({}),
                Some(&head.id), "reviewer-window-2")
            .map(|e| json!({"event": e.id}));
        Ok((e1.id, second.map_err(|e| e)))
    }

    /// 视图动作：选择/切换候选，不改原数据。
    pub fn view_action(&mut self, branch: &str, action: &str, detail: Value) -> Result<AuditEvent> {
        self.record(branch, EventKind::ViewAction, action,
            &json!({"action": action, "detail": detail, "mutates_source": false}),
            None, "reviewer")
    }

    pub fn current_lineage(&self) -> Result<LineageDoc> {
        match &self.store.latest_material("lineage")?.ok_or_else(|| Error::NotFound("lineage".into()))?.body {
            MaterialBody::Lineage(l) => Ok(l.clone()),
            _ => unreachable!(),
        }
    }

    pub fn current_gate_map(&self, lin: &LineageDoc) -> Result<BTreeMap<String, String>> {
        let mut map = BTreeMap::new();
        for node in &lin.nodes {
            if let crate::lineage::LineageNode::Gate { id, gate_material_version_id, .. } = node {
                map.insert(id.clone(), gate_material_version_id.clone());
            }
        }
        Ok(map)
    }
}
