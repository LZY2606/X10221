//! 工作区编排：把版本化制品、事件日志、推导引擎粘合为可复核流程。
//!
//! 职责：
//! - 加载离线案例包（真实 FCS + 分版本制品 JSON）到某分支；
//! - 评估补偿/门树，记录结果并保存输入依赖版本与来源链；
//! - 制品修订时按依赖关系计算失效范围（局部，不全局替换）；
//! - 人工裁定多候选；同版本并发修改由 store 的 base_version 冲突检测拒绝。

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::json;

use crate::domain::{
    Artifact, ArtifactData, ArtifactKind, CompensationArtifact, CompensationResult,
    DenominatorRef, EventsArtifact, GateLinkArtifact, GatePolygonArtifact, GateResult,
    ResultStatus, TransformArtifact,
};
use crate::engine::{evaluate_compensation, gate};
use crate::store::{EventPayload, Store, StoreError};

pub const ROOT_PARENT: &str = "root:all-events:v1";

/// 结果摘要（审阅页/并排视图用；所有显示值带 stable id）。
#[derive(Debug, Serialize, Clone)]
pub struct CompSummary {
    pub result_id: String,
    pub seq: u64,
    pub status: String,
    pub comp_artifact_id: String,
    pub events_artifact_id: String,
    pub condition_number: Option<f64>,
    pub determinant: Option<f64>,
    pub candidates: Vec<String>,
    pub diagnostics: Vec<String>,
    pub stale_reason: String,
    pub inputs: serde_json::Value,
    pub provenance: Vec<(String, String)>,
}

#[derive(Debug, Serialize, Clone)]
pub struct GateSummary {
    pub result_id: String,
    pub seq: u64,
    pub gate_stem: String,
    pub display_name: String,
    pub parent: String,
    pub status: String,
    pub member_count: usize,
    pub boundary: Vec<u64>,
    pub percent: Option<f64>,
    pub denominator: Option<DenominatorRef>,
    pub candidates: Vec<String>,
    pub diagnostics: Vec<String>,
    pub stale_reason: String,
    pub inputs: serde_json::Value,
    pub provenance: Vec<(String, String)>,
}

pub struct Workspace<'a> {
    pub store: &'a mut Store,
}

impl<'a> Workspace<'a> {
    pub fn new(store: &'a mut Store) -> Self {
        Workspace { store }
    }

    /// 把案例包里的 v1 制品导入一个分支，并记录 scenario S0。
    /// 全部成功才提交；任何失败由事务回滚（commit_artifact 各自成事务，
    /// 但失败时未提交内容不会写入；已提交内容是“成功导入的前置来源”，
    /// 调用方可在整批失败后删除分支——导入接口 load_case 用单事务批量版本）。
    pub fn load_case(
        &mut self,
        branch: &str,
        case: &CaseBundle,
        events: &EventsArtifact,
    ) -> Result<LoadReport, StoreError> {
        self.store.ensure_branch(branch, "case import")?;
        let mut committed = Vec::new();

        let ev = self.commit_data(
            branch,
            ArtifactKind::Events,
            "panel",
            &ArtifactData::Events(events.clone()),
            "S0 导入 FCS 原始通道值",
        )?;
        committed.push(ev.id.clone());

        let sp = case.spillover_v1()?;
        let a = self.commit_data(
            branch,
            ArtifactKind::Spillover,
            "panel",
            &ArtifactData::Spillover(sp),
            "S0 导入 $SPILLOVER 元数据原文",
        )?;
        committed.push(a.id.clone());

        let comp = case.comp_v1()?;
        let a = self.commit_data(
            branch,
            ArtifactKind::Compensation,
            "panel",
            &ArtifactData::Compensation(comp),
            "S0 补偿矩阵 v1（与 spillover 分开版本化）",
        )?;
        committed.push(a.id.clone());

        let tr = case.transform_v1()?;
        let a = self.commit_data(
            branch,
            ArtifactKind::Transform,
            "panel",
            &ArtifactData::Transform(tr),
            "S0 变换参数 v1",
        )?;
        committed.push(a.id.clone());

        let poly = case.gate("lymph", 1)?;
        let a = self.commit_data(
            branch,
            ArtifactKind::GatePolygon,
            "lymph",
            &ArtifactData::GatePolygon(poly),
            "S0 门多边形 lymph v1",
        )?;
        committed.push(a.id.clone());

        let c3 = case.gate("cd3", 1)?;
        let a = self.commit_data(
            branch,
            ArtifactKind::GatePolygon,
            "cd3",
            &ArtifactData::GatePolygon(c3),
            "S0 门多边形 cd3 v1",
        )?;
        committed.push(a.id.clone());

        let links = case.gate_links_v1()?;
        for (stem, link) in links {
            let a = self.commit_data(
                branch,
                ArtifactKind::GateLink,
                &stem,
                &ArtifactData::GateLink(link),
                "S0 父子关系（与几何分开版本化）",
            )?;
            committed.push(a.id);
        }

        self.store.append_event(
            branch,
            &EventPayload::ScenarioStep {
                code: "S0-import".into(),
                title: "初始导入：六类来源分开版本化".into(),
                accepted: true,
                note: "原始通道值/spillover/补偿/变换/门多边形/父子关系各自独立 stable id".into(),
            },
        )?;

        Ok(LoadReport {
            artifact_ids: committed,
        })
    }

    fn commit_data(
        &mut self,
        branch: &str,
        kind: ArtifactKind,
        stem: &str,
        data: &ArtifactData,
        reason: &str,
    ) -> Result<Artifact, StoreError> {
        // 初次导入允许无 base_version；版本号由 store 单调分配。
        self.store
            .commit_artifact(branch, kind, stem, data, reason, None)
    }
}

#[derive(Debug, Serialize)]
pub struct LoadReport {
    pub artifact_ids: Vec<String>,
}

// ---------- 案例包内存表示 ----------

#[derive(Debug, Clone)]
pub struct CaseBundle {
    pub case_id: String,
    pub title: String,
    pub raw: serde_json::Value,
}

impl CaseBundle {
    pub fn from_json(value: serde_json::Value) -> Result<CaseBundle, StoreError> {
        let case_id = value
            .get("case_id")
            .and_then(|v| v.as_str())
            .unwrap_or("case")
            .to_string();
        let title = value
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(CaseBundle {
            case_id,
            title,
            raw: value,
        })
    }

    pub fn spillover_v1(&self) -> Result<crate::domain::SpilloverArtifact, StoreError> {
        let v = self.find_artifact("spillover", "spillover:panel:v1")?;
        Ok(crate::domain::SpilloverArtifact {
            raw_keyword: serde_json::to_string(&v).unwrap_or_default(),
            keyword_name: "$SPILLOVER".into(),
            detectors: str_array(v, "detectors")?,
            matrix: matrix(v)?,
            unit: get_str(v, "unit")?,
        })
    }

    pub fn comp_v1(&self) -> Result<CompensationArtifact, StoreError> {
        self.comp("comp:panel:v1")
    }

    pub fn comp(&self, id_hint: &str) -> Result<CompensationArtifact, StoreError> {
        let v = self.find_artifact("compensation", id_hint)?;
        Ok(CompensationArtifact {
            detectors: str_array(v, "detectors")?,
            matrix: matrix(v)?,
            basis: v
                .get("basis")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            unit: get_str(v, "unit")?,
        })
    }

    pub fn transform_v1(&self) -> Result<TransformArtifact, StoreError> {
        self.transform("transform:panel:v1")
    }

    pub fn transform(&self, id_hint: &str) -> Result<TransformArtifact, StoreError> {
        let v = self.find_artifact("transform", id_hint)?;
        let params = v
            .get("params")
            .and_then(|x| x.as_array())
            .ok_or_else(|| StoreError("transform 缺少 params".into()))?;
        let mut out = Vec::new();
        for p in params {
            out.push(crate::domain::TransformParam {
                detector: get_str(p, "detector")?,
                method: get_str(p, "method")?,
                cofactor: p
                    .get("cofactor")
                    .and_then(|x| x.as_f64())
                    .ok_or_else(|| StoreError("cofactor 缺失".into()))?,
                coordinate_basis: get_str(p, "coordinate_basis")?,
            });
        }
        Ok(TransformArtifact { params: out })
    }

    pub fn gate(&self, name: &str, version: u32) -> Result<GatePolygonArtifact, StoreError> {
        let v = self.find_artifact(
            "gate_polygon",
            &format!("gate-poly:{name}:v{version}"),
        )?;
        let verts = v
            .get("vertices")
            .and_then(|x| x.as_array())
            .ok_or_else(|| StoreError("gate 缺少 vertices".into()))?;
        let mut vertices = Vec::new();
        for pt in verts {
            let pair = pt
                .as_array()
                .ok_or_else(|| StoreError("顶点不是数组".into()))?;
            vertices.push([
                pair.get(0).and_then(|x| x.as_f64()).unwrap_or(0.0),
                pair.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0),
            ]);
        }
        Ok(GatePolygonArtifact {
            name: get_str(v, "name")?,
            x_detector: get_str(v, "x_detector")?,
            y_detector: get_str(v, "y_detector")?,
            coordinate_basis: get_str(v, "coordinate_basis")?,
            vertices,
            boundary_policy: get_str(v, "boundary_policy")?,
        })
    }

    pub fn gate_link(&self, child: &str, version: u32) -> Result<GateLinkArtifact, StoreError> {
        let v = self.find_artifact(
            "gate_link",
            &format!("gate-link:{child}:v{version}"),
        )?;
        Ok(GateLinkArtifact {
            child: get_str(v, "child")?,
            parent: get_str(v, "parent")?,
            display_name: get_str(v, "display_name")?,
        })
    }

    pub fn gate_links_v1(&self) -> Result<Vec<(String, GateLinkArtifact)>, StoreError> {
        Ok(vec![
            ("lymph".into(), self.gate_link("lymph", 1)?),
            ("cd3".into(), self.gate_link("cd3", 1)?),
        ])
    }

    fn find_artifact(
        &self,
        bucket: &str,
        id_hint: &str,
    ) -> Result<&serde_json::Value, StoreError> {
        let arr = self
            .raw
            .pointer(&format!("/artifacts/{bucket}"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| StoreError(format!("案例包缺少 artifacts/{bucket}")))?;
        arr.iter()
            .find(|v| v.get("id_hint").and_then(|x| x.as_str()) == Some(id_hint))
            .ok_or_else(|| StoreError(format!("案例包缺少 {id_hint}")))
    }
}

fn get_str(v: &serde_json::Value, key: &str) -> Result<String, StoreError> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| StoreError(format!("字段 {key} 缺失")))
}

fn str_array(v: &serde_json::Value, key: &str) -> Result<Vec<String>, StoreError> {
    Ok(v.get(key)
        .and_then(|x| x.as_array())
        .ok_or_else(|| StoreError(format!("字段 {key} 缺失")))?
        .iter()
        .filter_map(|x| x.as_str().map(|s| s.to_string()))
        .collect())
}

fn matrix(v: &serde_json::Value) -> Result<Vec<Vec<f64>>, StoreError> {
    let arr = v
        .get("matrix")
        .and_then(|x| x.as_array())
        .ok_or_else(|| StoreError("matrix 缺失".into()))?;
    let mut out = Vec::new();
    for row in arr {
        let r = row
            .as_array()
            .ok_or_else(|| StoreError("matrix 行不是数组".into()))?
            .iter()
            .map(|x| x.as_f64().unwrap_or(0.0))
            .collect();
        out.push(r);
    }
    Ok(out)
}

fn _unused(_: BTreeMap<String, ()>, _: BTreeSet<String>) {}

// ---------- 评估管线 ----------

impl<'a> Workspace<'a> {
    /// 当前分支的有效制品 id（kind:stem -> artifact）。
    fn current(&self, branch: &str) -> Result<BTreeMap<String, Artifact>, StoreError> {
        let mut out = BTreeMap::new();
        for a in self.store.all_artifacts()? {
            // 只取该分支当前指针
            if let Some(pointer) = self
                .store
                .branch_artifact(branch, ArtifactKind::parse(&a.kind).unwrap(), &a.stem)?
            {
                if pointer.id == a.id {
                    out.insert(format!("{}:{}", a.kind, a.stem), a);
                }
            }
        }
        Ok(out)
    }

    fn events_artifact(
        &self,
        branch: &str,
        current: &BTreeMap<String, Artifact>,
    ) -> Result<EventsArtifact, StoreError> {
        match current.get("events:panel") {
            Some(Artifact {
                data: ArtifactData::Events(e),
                ..
            }) => Ok(e.clone()),
            _ => Err(StoreError("分支缺少 events:panel".into())),
        }
    }

    /// 评估当前补偿并记录结果（含输入依赖版本 + 来源链）。
    pub fn run_compensation(&mut self, branch: &str) -> Result<CompSummary, StoreError> {
        let current = self.current(branch)?;
        let events = self.events_artifact(branch, &current)?;
        let comp_art = current
            .get("comp:panel")
            .cloned()
            .ok_or_else(|| StoreError("分支缺少 comp:panel".into()))?;
        let comp = match &comp_art.data {
            ArtifactData::Compensation(c) => c.clone(),
            _ => return Err(StoreError("comp:panel 类型错误".into())),
        };
        let result = evaluate_compensation(&events, &comp);
        let inputs = json!({
            "events": "events:panel:v1",
            "comp": comp_art.id,
        });
        // inputs 钉住真实当前 id：
        let ev_id = current
            .get("events:panel")
            .map(|a| a.id.clone())
            .unwrap_or_else(|| "events:panel:v1".into());
        let inputs = json!({ "events": ev_id, "comp": comp_art.id });
        let provenance = vec![
            (ev_id, "events".into()),
            (comp_art.id.clone(), "compensation".into()),
        ];
        let (id, seq) = self.store.record_result(
            branch,
            "comp",
            result.status.as_str(),
            &inputs,
            Some(&result),
            None,
            &provenance,
        )?;
        self.summarize_comp(&id, seq, &result, &comp_art.id, &inputs)
    }

    fn summarize_comp(
        &self,
        id: &str,
        seq: u64,
        result: &CompensationResult,
        comp_id: &str,
        inputs: &serde_json::Value,
    ) -> Result<CompSummary, StoreError> {
        Ok(CompSummary {
            result_id: id.into(),
            seq,
            status: result.status.as_str().into(),
            comp_artifact_id: comp_id.into(),
            events_artifact_id: inputs
                .get("events")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            condition_number: result.condition_number,
            determinant: result.determinant,
            candidates: result.candidates.clone(),
            diagnostics: result.diagnostics.clone(),
            stale_reason: String::new(),
            inputs: inputs.clone(),
            provenance: self.store.provenance(id)?,
        })
    }

    /// 评估门树：根->叶。每个门的分母钉住“父群结果版本”（不是临时重算）。
    pub fn run_gate_tree(&mut self, branch: &str) -> Result<Vec<GateSummary>, StoreError> {
        let current = self.current(branch)?;
        let events = self.events_artifact(branch, &current)?;
        let comp_art = current
            .get("comp:panel")
            .cloned()
            .ok_or_else(|| StoreError("分支缺少 comp:panel".into()))?;
        let comp = match &comp_art.data {
            ArtifactData::Compensation(c) => c.clone(),
            _ => return Err(StoreError("comp:panel 类型错误".into())),
        };
        let transform = match current.get("transform:panel") {
            Some(Artifact {
                data: ArtifactData::Transform(t),
                ..
            }) => t.clone(),
            _ => return Err(StoreError("分支缺少 transform:panel".into())),
        };

        // 先算补偿坐标（沿用引擎规则；近奇异/奇异时只允许 inverse 已被接受的情形）。
        let comp_result = evaluate_compensation(&events, &comp);
        let comp_ok = comp_result.status == ResultStatus::Ok;
        let compensated = comp_result.compensated;

        // 收集门与连接，按父子拓扑。
        let mut polys: BTreeMap<String, (Artifact, GatePolygonArtifact)> = BTreeMap::new();
        let mut links: BTreeMap<String, (Artifact, GateLinkArtifact)> = BTreeMap::new();
        for (key, art) in &current {
            match (&art.data, key) {
                (ArtifactData::GatePolygon(g), _) => {
                    polys.insert(art.stem.clone(), (art.clone(), g.clone()));
                }
                (ArtifactData::GateLink(l), _) => {
                    links.insert(art.stem.clone(), (art.clone(), l.clone()));
                }
                _ => {}
            }
        }

        // 拓扑顺序：父在前。父 id 形如 gate-poly:lymph:v1，取 stem=lymph。
        let mut ordered: Vec<String> = Vec::new();
        let mut visiting = BTreeSet::new();
        for stem in links.keys() {
            self.toposort(stem, &links, &mut visiting, &mut ordered);
        }

        let ev_id = current
            .get("events:panel")
            .map(|a| a.id.clone())
            .unwrap_or_default();

        // 已采纳的父群成员集（含裁定后固定的成员）。
        let mut accepted_members: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        let mut accepted_results: BTreeMap<String, GateResult> = BTreeMap::new();
        let mut summaries = Vec::new();

        // 根群 = 所有有坐标的事件？根分母用全部事件数（原始事件计数）。
        let root_count = events.events.len() as u64;

        for stem in &ordered {
            let (poly_art, poly) = polys
                .get(stem)
                .cloned()
                .ok_or_else(|| StoreError(format!("门 {stem} 缺少多边形几何")))?;
            let (link_art, link) = links
                .get(stem)
                .cloned()
                .ok_or_else(|| StoreError(format!("门 {stem} 缺少父子连接")))?;

            // 父群成员与父版本身份。
            let (parent_members, parent_gate_id, parent_events_id, parent_count) =
                if link.parent == ROOT_PARENT {
                    let all: Vec<u64> = events.events.iter().map(|e| e.index).collect();
                    (all, ROOT_PARENT.to_string(), ev_id.clone(), root_count)
                } else {
                    let parent_stem = parse_stem_from_gate_id(&link.parent)
                        .ok_or_else(|| StoreError(format!("无法解析父门 id {}", link.parent)))?;
                    match accepted_results.get(&parent_stem) {
                        Some(pr) => {
                            let denom = pr.denominator.clone();
                            let count = pr.member_indices.len() as u64;
                            (
                                pr.member_indices.clone(),
                                format!("gate-poly:{}", parent_stem),
                                denom.map(|d| d.parent_events_id).unwrap_or_else(|| ev_id.clone()),
                                count,
                            )
                        }
                        None => {
                            return Err(StoreError(format!(
                                "父门 {parent_stem} 尚无可用（已采纳/ok）结果，门 {stem} 不能计算"
                            )))
                        }
                    };
                };

            let denom = DenominatorRef {
                parent_gate_id: parent_gate_id.clone(),
                parent_events_id,
                parent_count,
            };

            let mut gres = if comp_ok {
                gate::evaluate_gate(
                    &poly,
                    &link,
                    &events,
                    &transform,
                    &compensated,
                    &comp.detectors,
                    &parent_members,
                    Some(denom.clone()),
                )
            } else {
                GateResult {
                    gate_id: poly_art.id.clone(),
                    status: ResultStatus::Blocked,
                    member_indices: Vec::new(),
                    boundary_indices: Vec::new(),
                    candidates: Vec::new(),
                    denominator: Some(denom.clone()),
                    percent: None,
                    diagnostics: vec![
                        "补偿矩阵未通过（奇异/近奇异未裁定）：门结果阻断，等待上游解决".into(),
                    ],
                }
            };
            gres.gate_id = poly_art.id.clone();

            let inputs = json!({
                "events": ev_id,
                "comp": comp_art.id,
                "transform": current.get("transform:panel").map(|a| a.id.clone())
                    .unwrap_or_default(),
                "gate_poly": poly_art.id,
                "gate_link": link_art.id,
                "parent_result_version": parent_gate_id,
            });
            let provenance = vec![
                (ev_id.clone(), "events".into()),
                (comp_art.id.clone(), "compensation".into()),
                (
                    current
                        .get("transform:panel")
                        .map(|a| a.id.clone())
                        .unwrap_or_default(),
                    "transform".into(),
                ),
                (poly_art.id.clone(), "gate-poly".into()),
                (link_art.id.clone(), "gate-link".into()),
            ];
            let (id, seq) = self.store.record_result(
                branch,
                &format!("gate:{stem}"),
                gres.status.as_str(),
                &inputs,
                None,
                Some(&gres),
                &provenance,
            )?;
            let summary = GateSummary {
                result_id: id,
                seq,
                gate_stem: stem.clone(),
                display_name: link.display_name.clone(),
                parent: link.parent.clone(),
                status: gres.status.as_str().into(),
                member_count: gres.member_indices.len(),
                boundary: gres.boundary_indices.clone(),
                percent: gres.percent,
                denominator: gres.denominator.clone(),
                candidates: gres.candidates.iter().map(|c| c.label.clone()).collect(),
                diagnostics: gres.diagnostics.clone(),
                stale_reason: String::new(),
                inputs,
                provenance: Vec::new(),
            };
            // 只有 ok 的结果自动作为下游父群版本；blocked 等待裁定。
            if gres.status == ResultStatus::Ok {
                accepted_members.insert(stem.clone(), gres.member_indices.clone());
                accepted_results.insert(stem.clone(), gres);
            }
            summaries.push(summary);
        }
        Ok(summaries)
    }

    fn toposort(
        &self,
        stem: &str,
        links: &BTreeMap<String, (Artifact, GateLinkArtifact)>,
        visiting: &mut BTreeSet<String>,
        ordered: &mut Vec<String>,
    ) {
        if ordered.iter().any(|s| s == stem) {
            return;
        }
        if !visiting.insert(stem.to_string()) {
            return; // 防御：环
        }
        if let Some((_, link)) = links.get(stem) {
            if link.parent != ROOT_PARENT {
                if let Some(parent_stem) = parse_stem_from_gate_id(&link.parent) {
                    self.toposort(&parent_stem, links, visiting, ordered);
                }
            }
        }
        ordered.push(stem.to_string());
    }
}

/// 从 `gate-poly:lymph:v1` 提取 stem `lymph`。
fn parse_stem_from_gate_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("gate-poly:")?;
    let stem = rest.rsplit_once(":v").map(|(s, _)| s).unwrap_or(rest);
    Some(stem.to_string())
}

// ---------- 修订、局部失效、裁定、分支 ----------

impl<'a> Workspace<'a> {
    /// 提交一个修订版本，并按依赖关系只失效受影响范围。
    /// 返回受影响（被标记 stale）的结果。
    pub fn revise(
        &mut self,
        branch: &str,
        kind: ArtifactKind,
        stem: &str,
        data: &ArtifactData,
        reason: &str,
        base_version: u32,
    ) -> Result<ReviseReport, StoreError> {
        let art = self.store.commit_artifact(
            branch,
            kind,
            stem,
            data,
            reason,
            Some(base_version),
        )?;
        // 依赖失效：引用了该 kind:stem 的结果全部 stale（不覆盖内容）。
        let affected = crate::store::invalidate_dependents(
            &mut self.store.conn,
            branch,
            kind.as_str(),
            stem,
            reason,
        )?;
        // 上游门修订还要向下传播：父门结果 stale 后，子门结果也应 stale。
        let mut propagated = Vec::new();
        if matches!(kind, ArtifactKind::GatePolygon | ArtifactKind::GateLink) {
            propagated = self.propagate_downstream(branch, reason)?;
        }
        Ok(ReviseReport {
            artifact: art,
            stale_results: affected.into_iter().map(|(id, _)| id).collect(),
            propagated,
        })
    }

    /// 沿门树向下传播：若某门最新结果 stale/blocked/error，则其子门最新结果也 stale。
    fn propagate_downstream(
        &mut self,
        branch: &str,
        reason: &str,
    ) -> Result<Vec<String>, StoreError> {
        let mut changed = Vec::new();
        loop {
            let results = self.store.results(branch)?;
            // 每 scope 取最新一条。
            let mut latest: BTreeMap<String, crate::store::ResultRow> = BTreeMap::new();
            for r in results {
                latest.insert(r.scope.clone(), r);
            }
            let bad: BTreeSet<String> = latest
                .iter()
                .filter(|(_, r)| r.status != "ok")
                .map(|(scope, _)| scope.clone())
                .collect();
            let links = self.current_links(branch)?;
            let mut progress = false;
            for (stem, link) in &links {
                if link.parent == ROOT_PARENT {
                    continue;
                }
                let parent_stem = match parse_stem_from_gate_id(&link.parent) {
                    Some(s) => s,
                    None => continue,
                };
                let parent_scope = format!("gate:{parent_stem}");
                let child_scope = format!("gate:{stem}");
                if bad.contains(&parent_scope) {
                    if let Some(child_row) = latest.get(&child_scope) {
                        if child_row.status == "ok" {
                            crate::store::stale_result(
                                &mut self.store.conn,
                                branch,
                                &child_row.id,
                                &format!("{reason}；上游父门 {parent_stem} 过期，向下游传播"),
                            )?;
                            changed.push(child_row.id.clone());
                            progress = true;
                        }
                    }
                }
            }
            if !progress {
                break;
            }
        }
        Ok(changed)
    }

    fn current_links(
        &self,
        branch: &str,
    ) -> Result<BTreeMap<String, GateLinkArtifact>, StoreError> {
        let mut out = BTreeMap::new();
        for a in self.store.all_artifacts()? {
            if let ArtifactData::GateLink(l) = &a.data {
                if let Some(ptr) =
                    self.store
                        .branch_artifact(branch, ArtifactKind::GateLink, &a.stem)?
                {
                    if ptr.id == a.id {
                        out.insert(a.stem.clone(), l.clone());
                    }
                }
            }
        }
        Ok(out)
    }

    /// 人工裁定一个 blocked 结果，生成新事件；被否定的旧结果保留为 stale 历史。
    /// candidate:
    /// - 补偿：`pseudoinverse`（接受）或 `reject`（否定）
    /// - 门：`boundary-inside` / `boundary-outside`
    pub fn decide(
        &mut self,
        branch: &str,
        result_id: &str,
        candidate: &str,
        action: &str,
        note: &str,
    ) -> Result<DecisionReport, StoreError> {
        let row = self
            .store
            .result(result_id)?
            .ok_or_else(|| StoreError(format!("结果 {result_id} 不存在")))?;
        if row.branch != branch {
            return Err(StoreError("结果不属于该分支".into()));
        }

        // 记录裁定事件与决定行（不改原数据）。
        let seq = self.store.append_event(
            branch,
            &EventPayload::Decision {
                result_id: result_id.into(),
                candidate: candidate.into(),
                action: action.into(),
                note: note.into(),
            },
        )?;

        // 把被裁定结果标记 stale：历史保留但不再作为当前可见结论。
        crate::store::stale_result(
            &mut self.store.conn,
            branch,
            result_id,
            &format!("人工 {action}({candidate})：原 blocked 结果归档为历史"),
        )?;

        // 生成采用后的新结果行（ok）。若否定（reject），产生 error 终态行。
        let current = self.current(branch)?;
        let events = self.events_artifact(branch, &current)?;
        let comp_art = current
            .get("comp:panel")
            .cloned()
            .ok_or_else(|| StoreError("分支缺少 comp:panel".into()))?;
        let comp = match &comp_art.data {
            ArtifactData::Compensation(c) => c.clone(),
            _ => return Err(StoreError("comp:panel 类型错误".into())),
        };

        let inputs = row.inputs.clone();

        if let Some(scope_kind) = row.scope.strip_prefix("gate:") {
            let stem = scope_kind.to_string();
            let (poly_art, poly) = current
                .iter()
                .find_map(|(_, a)| match &a.data {
                    ArtifactData::GatePolygon(g) if a.stem == stem => Some((a.clone(), g.clone())),
                    _ => None,
                })
                .ok_or_else(|| StoreError("当前分支缺少门几何".into()))?;
            let (_link_art, link) = current
                .iter()
                .find_map(|(_, a)| match &a.data {
                    ArtifactData::GateLink(l) if l.child == stem => Some((a.clone(), l.clone())),
                    _ => None,
                })
                .ok_or_else(|| StoreError("当前分支缺少门连接".into()))?;
            let transform = match current.get("transform:panel") {
                Some(Artifact {
                    data: ArtifactData::Transform(t),
                    ..
                }) => t.clone(),
                _ => return Err(StoreError("分支缺少 transform:panel".into())),
            };
            let comp_res = evaluate_compensation(&events, &comp);
            // 父群成员：复用已采纳的最近 ok 结果（与 run_gate_tree 同一规则）。
            let (parent_members, denom) =
                self.parent_context(branch, &current, &events, &link)?;
            let mut gres = gate::evaluate_gate(
                &poly,
                &link,
                &events,
                &transform,
                &comp_res.compensated,
                &comp.detectors,
                &parent_members,
                denom,
            );
            // 固定所选候选成员集。
            let chosen = gres
                .candidates
                .iter()
                .find(|c| c.label == candidate)
                .cloned()
                .ok_or_else(|| {
                    StoreError(format!("候选 {candidate} 不是该结果的合法候选"))
                })?;
            gres.member_indices = chosen.member_indices;
            gres.candidates = Vec::new();
            gres.status = if action == "reject" {
                ResultStatus::Error
            } else {
                ResultStatus::Ok
            };
            gres.gate_id = poly_art.id.clone();
            let prov = vec![
                (
                    current
                        .get("events:panel")
                        .map(|a| a.id.clone())
                        .unwrap_or_default(),
                    "events".into(),
                ),
                (comp_art.id.clone(), "compensation".into()),
                (poly_art.id.clone(), "gate-poly".into()),
            ];
            let (new_id, _new_seq) = self.store.record_result(
                branch,
                &format!("gate:{stem}"),
                gres.status.as_str(),
                &inputs,
                None,
                Some(&gres),
                &prov,
            )?;
            return Ok(DecisionReport {
                decision_seq: seq,
                new_result_id: new_id,
            });
        }

        // comp scope
        let engine = evaluate_compensation(&events, &comp);
        let (status, mut result) = match (candidate, action) {
            ("pseudoinverse", "accept") => (ResultStatus::Ok, engine),
            ("reject", "reject") => {
                let mut r = engine;
                r.compensated = Vec::new();
                r.candidates = Vec::new();
                r.diagnostics.push(
                    "人工 reject：拒绝近奇异矩阵的全部输出，下游保持阻断".into(),
                );
                (ResultStatus::Error, r)
            }
            _ => {
                return Err(StoreError(format!(
                    "补偿结果的合法候选为 pseudoinverse/reject，收到 {candidate}/{action}"
                )))
            }
        };
        if candidate == "pseudoinverse" && action == "accept" {
            result.candidates = vec!["pseudoinverse".into()];
            result.diagnostics.push(
                "人工 accept(pseudoinverse)：显式采用伪逆；该选择作为分支裁定可撤销".into(),
            );
        }
        let prov = vec![
            (
                current
                    .get("events:panel")
                    .map(|a| a.id.clone())
                    .unwrap_or_default(),
                "events".into(),
            ),
            (comp_art.id.clone(), "compensation".into())],
        ];
        let (new_id, _) = self.store.record_result(
            branch,
            "comp",
            status.as_str(),
            &inputs,
            Some(&result),
            None,
            &prov,
        )?;
        Ok(DecisionReport {
            decision_seq: seq,
            new_result_id: new_id,
        })
    }

    /// 组装门的父群成员与钉版分母（用于裁定时重算同一个门）。
    fn parent_context(
        &self,
        branch: &str,
        current: &BTreeMap<String, Artifact>,
        events: &EventsArtifact,
        link: &GateLinkArtifact,
    ) -> Result<(Vec<u64>, Option<DenominatorRef>), StoreError> {
        let ev_id = current
            .get("events:panel")
            .map(|a| a.id.clone())
            .unwrap_or_default();
        if link.parent == ROOT_PARENT {
            let all: Vec<u64> = events.events.iter().map(|e| e.index).collect();
            let count = all.len() as u64;
            return Ok((
                all,
                Some(DenominatorRef {
                    parent_gate_id: ROOT_PARENT.into(),
                    parent_events_id: ev_id,
                    parent_count: count,
                }),
            ));
        }
        let parent_stem = parse_stem_from_gate_id(&link.parent)
            .ok_or_else(|| StoreError(format!("父 id 非法 {}", link.parent)))?;
        // 取父 scope 最新 ok 结果（裁定后的固定成员），分母钉住该父群版本。
        let mut rows = self.store.results(branch)?;
        rows.retain(|r| r.scope == format!("gate:{parent_stem}") && r.status == "ok");
        rows.sort_by_key(|r| r.seq);
        let parent_row = rows
            .last()
            .ok_or_else(|| StoreError(format!("父门 {parent_stem} 无已采纳结果")))?;
        let gate = parent_row
            .gate
            .clone()
            .ok_or_else(|| StoreError("父结果缺少门数据")))?;
        let denom = gate.denominator.clone().map(|_| DenominatorRef {
            parent_gate_id: format!("gate-poly:{parent_stem}"),
            parent_events_id: ev_id.clone(),
            parent_count: gate.member_indices.len() as u64,
        });
        Ok((gate.member_indices, denom))
    }
}

#[derive(Debug, Serialize)]
pub struct ReviseReport {
    pub artifact: Artifact,
    pub stale_results: Vec<String>,
    pub propagated: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DecisionReport {
    pub decision_seq: u64,
    pub new_result_id: String,
}
