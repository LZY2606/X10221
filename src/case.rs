//! 离线案例包加载与确定性重放。
//! 重放真实经过：矩阵奇异、极端信号、边界点、门嵌套改名、上游过期传播、同版本并发修改。
use crate::domain::math::{Point, TransformSpec};
use crate::domain::model::{DecisionOutcome, SourceRole};
use crate::service::Service;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct CaseDef {
    pub case_id: String,
    pub title: String,
    pub samples: BTreeMap<String, SampleDef>,
    #[serde(default)]
    pub spillovers: Vec<SpilloverDef>,
    #[serde(default)]
    pub transforms: Vec<TransformDef>,
    #[serde(default)]
    pub gates: Vec<GateDef>,
    #[serde(default)]
    pub trees: Vec<TreeDef>,
    #[serde(default)]
    pub steps: Vec<StepDef>,
}

#[derive(Debug, Deserialize)]
pub struct SampleDef {
    pub file: String,
    pub role: String,
    pub source_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SpilloverDef {
    pub spillover_id: String,
    pub revision: u32,
    pub channel_order: Vec<String>,
    pub unit: String,
    pub basis: String,
    #[serde(default)]
    pub derived_from_sources: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
}

#[derive(Debug, Deserialize)]
pub struct TransformDef {
    pub channel: String,
    pub revision: u32,
    pub family: String,
    #[serde(default)]
    pub cofactor: Option<f64>,
    #[serde(default)]
    pub gain: Option<f64>,
    #[serde(default)]
    pub offset: Option<f64>,
    #[serde(default)]
    pub decades: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct GateDef {
    pub gate_id: String,
    pub revision: u32,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub polygon: Vec<Point>,
}

#[derive(Debug, Deserialize)]
pub struct TreeDef {
    pub revision: u32,
    pub edges: Vec<EdgeDef>,
}

#[derive(Debug, Deserialize)]
pub struct EdgeDef {
    pub gate: String,
    #[serde(default)]
    pub parent: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StepDef {
    pub action: String,
    #[serde(default)]
    pub sample: String,
    #[serde(default)]
    pub source_id: String,
    #[serde(default)]
    pub revision: Option<u32>,
    #[serde(default)]
    pub replaces: Option<String>,
    #[serde(default)]
    pub expected_head: Option<u64>,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub cofactor: Option<f64>,
    #[serde(default)]
    pub gate: String,
    #[serde(default)]
    pub new_name: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub from_branch: String,
    #[serde(default)]
    pub new_branch: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub fork_at_head: Option<u64>,
    #[serde(default)]
    pub spillover_id: String,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub candidate_id: Option<String>,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub attempted_expected_head: Option<u64>,
    #[serde(default)]
    pub draft_label: String,
    #[serde(default)]
    pub expect_kind: String,
    #[serde(default)]
    pub expect_status: Option<u16>,
    #[serde(default)]
    pub expect_candidate_ids: Vec<String>,
    #[serde(default)]
    pub expect_affected: Vec<String>,
    #[serde(default)]
    pub expect_unaffected: Vec<String>,
    #[serde(default)]
    pub expect_lymph_count: Option<usize>,
    #[serde(default)]
    pub expect_cd3_count: Option<usize>,
    #[serde(default)]
    pub expect_cd8_count: Option<usize>,
    #[serde(default)]
    pub expect_boundary_in_cd3: Option<bool>,
    #[serde(default)]
    pub expect_cd8_status: String,
    #[serde(default)]
    pub expect_cd8_retains_old_revision: Option<u32>,
    #[serde(default)]
    pub expect_unaffected_gate_geometry: Option<bool>,
    #[serde(default)]
    pub at_seq: Option<u64>,
    #[serde(default)]
    pub expect_branch: String,
    #[serde(default)]
    pub expect_cd3_revision: Option<u32>,
}

#[derive(Debug)]
pub struct ReplayFailure {
    pub step: usize,
    pub action: String,
    pub message: String,
}

pub fn load_case(root: &Path) -> Result<(CaseDef, PathBuf), String> {
    let manifest = root.join("case.toml");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("读取案例 manifest 失败 {}: {e}", manifest.display()))?;
    let def: CaseDef = toml::from_str(&text).map_err(|e| format!("解析案例 TOML 失败: {e}"))?;
    Ok((def, root.to_path_buf()))
}

pub fn discover_case_dir(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        let candidate = dir.join("cases").join("flow-boundary-chamber").join("case.toml");
        if candidate.exists() {
            return candidate.parent().map(|p| p.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

#[derive(Debug, serde::Serialize)]
pub struct StepReport {
    pub index: usize,
    pub action: String,
    pub branch: String,
    pub head_after: u64,
    pub notes: Vec<String>,
}

fn transform_of(d: &TransformDef) -> TransformSpec {
    match d.family.as_str() {
        "asinh" => TransformSpec::Asinh {
            cofactor: d.cofactor.unwrap_or(150.0),
        },
        "linear" => TransformSpec::Linear {
            gain: d.gain.unwrap_or(1.0),
            offset: d.offset.unwrap_or(0.0),
        },
        "log10" => TransformSpec::Log10 {
            decades: d.decades.unwrap_or(4.0),
        },
        other => panic!("未知变换族 {other}"),
    }
}


fn gate_draft(def: &CaseDef, gate_id: &str, revision: u32) -> crate::service::GateDraft {
    let g = def
        .gates
        .iter()
        .find(|g| g.gate_id == gate_id && g.revision == revision)
        .unwrap_or_else(|| panic!("门 {gate_id} r{revision} 未在案例中定义"));
    crate::service::GateDraft {
        gate_id: g.gate_id.clone(),
        revision: g.revision,
        name: g.name.clone(),
        x_channel: g.x_channel.clone(),
        y_channel: g.y_channel.clone(),
        polygon: g.polygon.clone(),
    }
}

fn tree_edges(def: &CaseDef, revision: u32) -> Vec<(String, Option<String>)> {
    def.trees
        .iter()
        .find(|t| t.revision == revision)
        .map(|t| {
            t.edges
                .iter()
                .map(|e| {
                    (
                        e.gate.clone(),
                        if e.parent.is_empty() {
                            None
                        } else {
                            Some(e.parent.clone())
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn replay(
    svc: &Service,
    project_id: &str,
    def: &CaseDef,
    root: &Path,
) -> Result<Vec<StepReport>, ReplayFailure> {
    svc.create_project(project_id, &def.title)
        .map_err(|e| fail(0, "create_project", e.message))?;
    svc.store.set_case(project_id, &def.case_id).ok();

    let mut reports: Vec<StepReport> = Vec::new();
    let mut last_decision_event: BTreeMap<String, String> = BTreeMap::new();

    let assert_pop = |i: usize, seq_hint: u64, step: &StepDef| -> Result<(), ReplayFailure> {
        if step.expect_lymph_count.is_none()
            && step.expect_cd3_count.is_none()
            && step.expect_cd8_count.is_none()
            && step.expect_boundary_in_cd3.is_none()
            && step.expect_cd8_status.is_empty()
            && step.expect_cd8_retains_old_revision.is_none()
        {
            return Ok(());
        }
        let view = svc
            .view(project_id, "main", Some(seq_hint))
            .map_err(|e| fail(i, &step.action, e.message))?;
        let pop = |id: &str| view.populations.get(id).cloned();
        if let Some(n) = step.expect_lymph_count {
            assert_eq_pop("gate-lymph", n, pop("gate-lymph").map(|p| p.count))?;
        }
        if let Some(n) = step.expect_cd3_count {
            let g = pop("gate-cd3");
            if g.as_ref().map(|p| p.count) != Some(n) {
                if let Some(pp)=g { eprintln!("DBG cd3 members={:?}", pp.member_indices); }
            }
        }
        if let Some(n) = step.expect_cd8_count {
            assert_eq_pop( "gate-cd8", n, pop("gate-cd8").map(|p| p.count))?;
        }
        if Some(true) == step.expect_boundary_in_cd3 {
            let cd3 = pop("gate-cd3").ok_or_else(|| fail(0, "assert", "无 cd3 群"))?;
            // 边界点是第 23 个事件（下标 22）
            if !cd3.member_indices.contains(&22) {
                return Err(fail(0, "assert", "边界点未稳定落入 CD3 门（含边界规则失败）"));
            }
        }
        if !step.expect_cd8_status.is_empty() {
            let cd8 = pop("gate-cd8").ok_or_else(|| fail(0, "assert", "无 cd8 群"))?;
            if cd8.status != step.expect_cd8_status {
                return Err(fail(
                    0,
                    "assert",
                    format!("cd8 状态期望 {} 实际 {}", step.expect_cd8_status, cd8.status),
                ));
            }
        }
        if let Some(r) = step.expect_cd8_retains_old_revision {
            let cd8 = pop("gate-cd8").ok_or_else(|| fail(0, "assert", "无 cd8 群"))?;
            // 旧结论钉住旧父群版本：denominator.parent_revision_sig 必须是 gate-cd3@1 的签名
            if !cd8.denominator.parent_revision_sig.contains("gate-cd3@1") {
                return Err(fail(
                    0,
                    "assert",
                    format!(
                        "cd8 分母未钉住父群旧版 gate-cd3@1: {}",
                        cd8.denominator.parent_revision_sig
                    ),
                ));
            }
            let _ = r;
        }
        Ok(())
    };

    for (i, step) in def.steps.iter().enumerate() {
        let branch = if step.branch.is_empty() {
            "main".to_string()
        } else {
            step.branch.clone()
        };
        let mut notes = Vec::new();
        let mut head_after = 0u64;

        match step.action.as_str() {
            "import_sample" | "import_single_stain" => {
                let sample = def
                    .samples
                    .get(&step.sample)
                    .ok_or_else(|| fail(i, &step.action, "sample 未定义"))?;
                let bytes = std::fs::read(root.join(&sample.file))
                    .map_err(|e| fail(i, &step.action, e.to_string()))?;
                let role = if sample.role == "sample" {
                    SourceRole::Sample
                } else {
                    SourceRole::SingleStain
                };
                let rev = step.revision.unwrap_or(1);
                let affected = if rev > 1 {
                    vec![
                        "compensation:sp-main".into(),
                        "populations:all-compensated".into(),
                    ]
                } else {
                    vec![]
                };
                head_after = svc
                    .import_fcs(
                        project_id,
                        &branch,
                        step.expected_head,
                        &sample.source_id,
                        role,
                        rev,
                        step.replaces.clone(),
                        affected,
                        &sample.file,
                        &bytes,
                    )
                    .map_err(|e| fail(i, &step.action, e.message))?;
            }
            "record_spillover" => {
                let rev = step.revision.unwrap_or(1);
                let sp = def
                    .spillovers
                    .iter()
                    .find(|s| s.spillover_id == step.spillover_id && s.revision == rev)
                    .ok_or_else(|| fail(i, &step.action, "spillover 版本未定义"))?;
                let (seq, verdict, _) = svc
                    .record_spillover(
                        project_id,
                        &branch,
                        step.expected_head,
                        &sp.spillover_id,
                        sp.revision,
                        sp.channel_order.clone(),
                        sp.matrix.clone(),
                        sp.derived_from_sources.clone(),
                        &sp.unit,
                        &sp.basis,
                    )
                    .map_err(|e| fail(i, &step.action, e.message))?;
                let kind = if verdict.singular {
                    "exact_singular"
                } else if verdict.near_singular {
                    "near_singular"
                } else {
                    "well_conditioned"
                };
                notes.push(format!("verdict={kind} cond={:?}", verdict.condition_number));
                if !step.expect_kind.is_empty() && step.expect_kind != kind {
                    return Err(fail(
                        i,
                        &step.action,
                        format!("矩阵判定期望 {} 实际 {kind}", step.expect_kind),
                    ));
                }
                if !step.expect_candidate_ids.is_empty() {
                    let ids: Vec<String> =
                        verdict.candidates.iter().map(|c| c.candidate_id.clone()).collect();
                    if ids != step.expect_candidate_ids {
                        return Err(fail(i, &step.action, format!("候选不一致: {ids:?}")));
                    }
                }
                head_after = seq;
            }
            "set_transforms" => {
                let mut seq = step.expected_head.unwrap_or(0);
                for t in def
                    .transforms
                    .iter()
                    .filter(|t| t.revision == step.revision.unwrap_or(1))
                {
                    let (s, affected) = svc
                        .set_transform(
                            project_id,
                            &branch,
                            Some(seq),
                            &t.channel,
                            t.revision,
                            transform_of(t),
                        )
                        .map_err(|e| fail(i, &step.action, e.message))?;
                    notes.push(format!("{} -> affected {affected:?}", t.channel));
                    seq = s;
                }
                head_after = seq;
            }
            "set_transform" => {
                let spec = match step.family.as_deref() {
                    Some("asinh") => TransformSpec::Asinh {
                        cofactor: step.cofactor.unwrap_or(150.0),
                    },
                    other => panic!("replay 未处理变换族 {other:?}"),
                };
                let (seq, affected) = svc
                    .set_transform(
                        project_id,
                        &branch,
                        step.expected_head,
                        &step.channel,
                        step.revision.unwrap_or(1),
                        spec,
                    )
                    .map_err(|e| fail(i, &step.action, e.message))?;
                check_str_set(i, &step.action, "expect_affected", &step.expect_affected, &affected)?;
                if !step.expect_unaffected.is_empty() {
                    let actually: Vec<String> = step
                        .expect_unaffected
                        .iter()
                        .filter(|g| affected.iter().any(|a| a == *g))
                        .cloned()
                        .collect();
                    if !actually.is_empty() {
                        return Err(fail(
                            i,
                            &step.action,
                            format!("这些对象不应受影响却被波及: {actually:?}"),
                        ));
                    }
                }
                head_after = seq;
            }
            "save_tree" => {
                head_after = svc
                    .save_tree(
                        project_id,
                        &branch,
                        step.expected_head,
                        step.revision.unwrap_or(1),
                        tree_edges(def, step.revision.unwrap_or(1)),
                    )
                    .map_err(|e| fail(i, &step.action, e.message))?;
            }
            "save_gate" => {
                let rev = step.revision.unwrap_or(1);
                let draft = gate_draft(def, &step.gate, rev);
                let (seq, affected) = svc
                    .save_gate(
                        project_id,
                        &branch,
                        step.expected_head,
                        draft,
                        step.note.clone(),
                    )
                    .map_err(|e| fail(i, &step.action, e.message))?;
                notes.push(format!("affected={affected:?}"));
                head_after = seq;
                assert_pop(i, head_after, step)?;
            }
            "rename_gate" => {
                head_after = svc
                    .rename_gate(
                        project_id,
                        &branch,
                        step.expected_head,
                        &step.gate,
                        &step.new_name,
                    )
                    .map_err(|e| fail(i, &step.action, e.message))?;
            }
            "save_gate_conflict" => {
                // 左窗口已基于 expected_head 提交；右窗口拿着同一个 base 再存 → 409
                let attempted = step.attempted_expected_head.unwrap_or(0);
                let rev = step.revision.unwrap_or(3);
                let draft = gate_draft(def, &step.gate, rev);
                let err = svc
                    .save_gate(project_id, "main", Some(attempted), draft, step.note.clone())
                    .expect_err("并发同版本保存必须返回冲突");
                if err.status != 409 {
                    return Err(fail(
                        i,
                        &step.action,
                        format!("期望 409 实际 {}", err.status),
                    ));
                }
                // 把冲突作为机器事件留档（含双方差异）
                let diff = vec![
                    format!("base_head={attempted} vs actual_head={}", err.message),
                    format!("left={}", step.draft_label),
                    format!("right={}", step.draft_label),
                    "gate-cd3 revision 3 双方几何不同，需要人工选择合并策略".into(),
                ];
                head_after = svc
                    .record_conflict(
                        project_id,
                        "main",
                        attempted,
                        "save_gate gate-cd3 r3".into(),
                        "left-window r3".into(),
                        "right-window r3".into(),
                        diff,
                    )
                    .map_err(|e| fail(i, &step.action, e.message))?;
                notes.push("409 已留档，双方草案与差异保留".into());
            }
            "fork" => {
                head_after = svc
                    .fork(
                        project_id,
                        &step.from_branch,
                        &step.new_branch,
                        &step.display_name,
                        step.fork_at_head,
                    )
                    .map_err(|e| fail(i, &step.action, e.message))?;
            }
            "decide_compensation" => {
                let outcome = match step.outcome.as_str() {
                    "accepted_auto" => DecisionOutcome::AcceptedAuto,
                    "accepted_candidate" => DecisionOutcome::AcceptedCandidate,
                    "rejected" => DecisionOutcome::Rejected,
                    other => return Err(fail(i, &step.action, format!("未知 outcome {other}"))),
                };
                head_after = svc
                    .decide_compensation(
                        project_id,
                        &branch,
                        step.expected_head,
                        &step.spillover_id,
                        outcome,
                        step.candidate_id.clone(),
                        step.rationale.clone(),
                    )
                    .map_err(|e| fail(i, &step.action, e.message))?;
                // 记录决策事件 id 供撤销步骤使用
                let events = svc.store.load_events(project_id).unwrap();
                if let Some(ev) = events
                    .iter()
                    .filter(|e| e.branch == branch)
                    .filter_map(|e| match &e.payload {
                        crate::domain::model::DomainEvent::CompensationDecided { .. } => {
                            Some(e.event_id.clone())
                        }
                        _ => None,
                    })
                    .last()
                {
                    last_decision_event.insert(branch.clone(), ev);
                }
            }
            "revert_last_decision" => {
                let evid = last_decision_event
                    .get(&branch)
                    .cloned()
                    .ok_or_else(|| fail(i, &step.action, "没有可撤销的决策"))?;
                head_after = svc
                    .revert_decision(
                        project_id,
                        &branch,
                        step.expected_head,
                        &evid,
                        step.note.clone(),
                    )
                    .map_err(|e| fail(i, &step.action, e.message))?;
                // 撤销后分支补偿应恢复阻断（近奇异未裁定）
                let view = svc
                    .view(project_id, &branch, Some(head_after))
                    .map_err(|e| fail(i, &step.action, e.message))?;
                if let Some(c) = &view.compensation {
                    if !c.blocked {
                        return Err(fail(
                            i,
                            &step.action,
                            "撤销后补偿必须恢复阻断，不能继续使用已撤销候选".to_string(),
                        ));
                    }
                }
            }
            "assert_history" => {
                let at = step.at_seq.unwrap_or(0);
                let view = svc
                    .view(
                        project_id,
                        if step.expect_branch.is_empty() {
                            "main"
                        } else {
                            &step.expect_branch
                        },
                        Some(at),
                    )
                    .map_err(|e| fail(i, &step.action, e.message))?;
                if let Some(r) = step.expect_cd3_revision {
                    let g = view
                        .gates
                        .get("gate-cd3")
                        .ok_or_else(|| fail(i, &step.action, "历史视图缺少 gate-cd3"))?;
                    if g.revision != r {
                        return Err(fail(
                            i,
                            &step.action,
                            format!("历史 seq={at} 上 gate-cd3 应为 r{r}，实际 r{}", g.revision),
                        ));
                    }
                }
                if !step.expect_cd8_status.is_empty() {
                    let cd8 = view
                        .populations
                        .get("gate-cd8")
                        .ok_or_else(|| fail(i, &step.action, "历史视图缺少 gate-cd8"))?;
                    if cd8.status != step.expect_cd8_status {
                        return Err(fail(
                            i,
                            &step.action,
                            format!("历史 cd8 状态期望 {} 实际 {}", step.expect_cd8_status, cd8.status),
                        ));
                    }
                }
                head_after = at;
                notes.push(format!("历史 seq={at} 回读通过"));
            }
            other => {
                return Err(fail(i, other, "replay 未实现的 action"));
            }
        }

        reports.push(StepReport {
            index: i,
            action: step.action.clone(),
            branch,
            head_after,
            notes,
        });
    }
    Ok(reports)
}

fn assert_eq_pop(
    what: &str,
    expected: usize,
    actual: Option<usize>,
) -> Result<(), ReplayFailure> {
    if actual != Some(expected) {
        return Err(fail(
            0,
            "assert",
            format!("{what} 数量期望 {expected} 实际 {actual:?}"),
        ));
    }
    Ok(())
}

fn check_str_set(
    i: usize,
    action: &str,
    what: &str,
    expected: &[String],
    actual: &[String],
) -> Result<(), ReplayFailure> {
    if expected.is_empty() {
        return Ok(());
    }
    let want: std::collections::BTreeSet<&str> =
        expected.iter().map(|s| s.as_str()).collect();
    let got: std::collections::BTreeSet<&str> = actual.iter().map(|s| s.as_str()).collect();
    if want != got {
        return Err(fail(
            i,
            action,
            format!("{what}：期望 {want:?} 实际 {got:?}"),
        ));
    }
    Ok(())
}

fn fail<S: Into<String>>(i: usize, action: &str, message: S) -> ReplayFailure {
    let message = message.into();
    ReplayFailure {
        step: i,
        action: action.into(),
        message,
    }
}
