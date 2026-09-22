//! 应用服务层：把审阅动作变成追加事件，并维护乐观并发与失效范围。
use crate::domain::engine::{fold, ViewState};
use crate::domain::math::{assess_matrix, MatrixVerdict, Point, TransformSpec};
use crate::domain::model::{
    DecisionOutcome, DomainEvent, Envelope, SourceRole, ALGORITHM_VERSION,
};
use crate::fcs::FcsFile;
use crate::store::{Store, StoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub struct Service {
    pub store: Arc<Store>,
}

#[derive(Debug)]
pub struct CommandError {
    pub status: u16,
    pub message: String,
    pub conflict: Option<ConflictDetail>,
}
impl CommandError {
    pub fn bad(msg: impl Into<String>) -> Self {
        CommandError {
            status: 400,
            message: msg.into(),
            conflict: None,
        }
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        CommandError {
            status: 404,
            message: msg.into(),
            conflict: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictDetail {
    pub expected_head: u64,
    pub actual_head: u64,
    pub attempted_action: String,
    pub left_draft: String,
    pub right_draft: String,
    pub diff: Vec<String>,
}

pub type CmdResult<T> = Result<T, CommandError>;

fn sha_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl Service {
    pub fn new(store: Arc<Store>) -> Self {
        Service { store }
    }

    pub fn view(&self, project_id: &str, branch: &str, at_seq: Option<u64>) -> CmdResult<ViewState> {
        let events = self.store.load_events(project_id).map_err(store_err)?;
        Ok(fold(&events, branch, at_seq))
    }

    pub fn fcs_of(&self, project_id: &str, source_id: &str) -> CmdResult<FcsFile> {
        let bytes = self.store.load_blob(project_id, source_id).map_err(store_err)?;
        FcsFile::parse(&bytes).map_err(|e| CommandError::bad(e.to_string()))
    }

    fn append(
        &self,
        project_id: &str,
        branch: &str,
        parent_branch: Option<String>,
        branch_start_seq: u64,
        expected_head: Option<u64>,
        payload: DomainEvent,
        blob: Option<(&str, &[u8])>,
    ) -> Result<(u64, Envelope), CommandError> {
        let events = self.store.load_events(project_id).map_err(store_err)?;
        let current = self.store.head_seq(project_id).map_err(store_err)?;
        if let Some(exp) = expected_head {
            if exp != current {
                return Err(CommandError {
                    status: 409,
                    message: format!(
                        "版本冲突：期望在 #{exp} 之后提交，当前头已到 #{current}"
                    ),
                    conflict: None,
                });
            }
        }
        let seq = current + 1;
        let event_id = make_event_id(seq, branch, &payload);
        let env = Envelope {
            seq,
            branch: branch.to_string(),
            parent_branch,
            branch_start_seq,
            event_id,
            payload,
        };
        let assigned = self.store.append(project_id, expected_head, &env, blob).map_err(
            |e| match e {
                StoreError::Conflict { expected, actual } => CommandError {
                    status: 409,
                    message: format!("版本冲突：期望 #{expected}，实际 #{actual}"),
                    conflict: None,
                },
                other => CommandError::bad(other.to_string()),
            },
        )?;
        let _ = events;
        Ok((assigned, env))
    }

    pub fn create_project(&self, project_id: &str, title: &str) -> CmdResult<u64> {
        if self.store.project_exists(project_id).map_err(store_err)? {
            return Err(CommandError::bad("项目已存在"));
        }
        self.store
            .create_project(project_id, title)
            .map_err(store_err)?;
        let (_seq, _env) = self.append(
            project_id,
            "main",
            None,
            0,
            None,
            DomainEvent::ProjectCreated {
                project_id: project_id.into(),
                title: title.into(),
            },
            None,
        )?;
        Ok(_seq)
    }

    pub fn import_fcs(
        &self,
        project_id: &str,
        branch: &str,
        expected_head: Option<u64>,
        source_id: &str,
        role: SourceRole,
        revision: u32,
        replaces: Option<String>,
        affected_objects: Vec<String>,
        filename: &str,
        bytes: &[u8],
    ) -> CmdResult<u64> {
        // 先完整解析：解析失败不会写任何东西（无半份记录）
        let fcs = FcsFile::parse(bytes).map_err(|e| CommandError::bad(e.to_string()))?;
        let sha = sha_hex(bytes);
        let payload = DomainEvent::FcsImported {
            source_id: source_id.into(),
            role,
            revision,
            replaces,
            filename: filename.into(),
            sha256: sha.clone(),
            byte_len: bytes.len(),
            fcs,
            affected_objects,
        };
        let (seq, _) = self.append(
            project_id,
            branch,
            None,
            0,
            expected_head,
            payload,
            Some((source_id, bytes)),
        )?;
        Ok(seq)
    }
}

fn make_event_id(seq: u64, branch: &str, payload: &DomainEvent) -> String {
    let mut h = Sha256::new();
    h.update(seq.to_le_bytes());
    h.update(branch.as_bytes());
    h.update(payload.kind().as_bytes());
    h.update(serde_json::to_vec(payload).unwrap_or_default());
    format!("evt-{:06}-{}", seq, &format!("{:x}", h.finalize())[..10])
}

fn store_err(e: StoreError) -> CommandError {
    CommandError::bad(e.to_string())
}

#[derive(Debug, Clone, Deserialize)]
pub struct GateDraft {
    pub gate_id: String,
    pub revision: u32,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub polygon: Vec<Point>,
}

impl Service {
    pub fn record_spillover(
        &self,
        project_id: &str,
        branch: &str,
        expected_head: Option<u64>,
        spillover_id: &str,
        revision: u32,
        channel_order: Vec<String>,
        matrix: Vec<Vec<f64>>,
        derived_from_sources: Vec<String>,
        unit: &str,
        basis: &str,
    ) -> CmdResult<(u64, MatrixVerdict, Vec<String>)> {
        let verdict = assess_matrix(&matrix);
        let affected = vec![
            format!("compensation:{spillover_id}"),
            "populations:all-compensated".into(),
        ];
        let (seq, _) = self.append(
            project_id,
            branch,
            None,
            0,
            expected_head,
            DomainEvent::SpilloverRecorded {
                spillover_id: spillover_id.into(),
                revision,
                channel_order,
                matrix,
                unit: unit.into(),
                basis: basis.into(),
                derived_from_sources,
            },
            None,
        )?;
        let (seq2, _) = self.append(
            project_id,
            branch,
            None,
            0,
            Some(seq),
            DomainEvent::CompensationAssessed {
                spillover_id: spillover_id.into(),
                verdict: verdict.clone(),
                affected_objects: affected.clone(),
            },
            None,
        )?;
        Ok((seq2, verdict, affected))
    }

    pub fn set_transform(
        &self,
        project_id: &str,
        branch: &str,
        expected_head: Option<u64>,
        channel: &str,
        revision: u32,
        spec: TransformSpec,
    ) -> CmdResult<(u64, Vec<String>)> {
        // 局部失效：只影响使用该通道的门
        let view = self.view(project_id, branch, expected_head)?;
        let affected: Vec<String> = view
            .gates
            .values()
            .filter(|g| g.x_channel == channel || g.y_channel == channel)
            .map(|g| format!("gate:{}", g.gate_id))
            .collect();
        let (seq, _) = self.append(
            project_id,
            branch,
            None,
            0,
            expected_head,
            DomainEvent::TransformSet {
                transform_id: format!("transform-{channel}"),
                revision,
                channel: channel.into(),
                spec,
            },
            None,
        )?;
        Ok((seq, affected))
    }

    /// 保存门几何；修订已有门时计算下游失效范围并写 GateRevised（旧结论保留）。
    pub fn save_gate(
        &self,
        project_id: &str,
        branch: &str,
        expected_head: Option<u64>,
        draft: GateDraft,
        reason: String,
    ) -> CmdResult<(u64, Vec<String>)> {
        let view = self.view(project_id, branch, expected_head)?;
        let existing = view.gates.get(&draft.gate_id).cloned();
        let coordinate_basis = format!(
            "x/y transformed via {}; algorithm={}",
            ALGORITHM_VERSION, ALGORITHM_VERSION
        );
        let (seq, _) = self.append(
            project_id,
            branch,
            None,
            0,
            expected_head,
            DomainEvent::GateGeometrySaved {
                gate_id: draft.gate_id.clone(),
                revision: draft.revision,
                name: draft.name.clone(),
                x_channel: draft.x_channel,
                y_channel: draft.y_channel,
                coordinate_basis,
                polygon: draft.polygon,
            },
            None,
        )?;
        let mut affected = vec![format!("gate:{}", draft.gate_id)];
        if let Some(old) = existing {
            if old.revision < draft.revision {
                for desc in descendants(&view, &draft.gate_id) {
                    affected.push(format!("gate:{desc}"));
                }
                let (seq2, _) = self.append(
                    project_id,
                    branch,
                    None,
                    0,
                    Some(seq),
                    DomainEvent::GateRevised {
                        gate_id: draft.gate_id.clone(),
                        new_revision: draft.revision,
                        affected_objects: affected.clone(),
                        reason,
                    },
                    None,
                )?;
                return Ok((seq2, affected));
            }
        }
        Ok((seq, affected))
    }

    pub fn rename_gate(
        &self,
        project_id: &str,
        branch: &str,
        expected_head: Option<u64>,
        gate_id: &str,
        new_name: &str,
    ) -> CmdResult<u64> {
        let (seq, _) = self.append(
            project_id,
            branch,
            None,
            0,
            expected_head,
            DomainEvent::GateRenamed {
                gate_id: gate_id.into(),
                new_name: new_name.into(),
            },
            None,
        )?;
        Ok(seq)
    }

    pub fn save_tree(
        &self,
        project_id: &str,
        branch: &str,
        expected_head: Option<u64>,
        revision: u32,
        edges: Vec<(String, Option<String>)>,
    ) -> CmdResult<u64> {
        if let Some(cycle) = detect_cycle(&edges) {
            return Err(CommandError::bad(format!("门树存在环（涉及 {cycle}）")));
        }
        let (seq, _) = self.append(
            project_id,
            branch,
            None,
            0,
            expected_head,
            DomainEvent::GateTreeSaved {
                tree_id: "tree-main".into(),
                revision,
                edges,
            },
            None,
        )?;
        Ok(seq)
    }

    pub fn decide_compensation(
        &self,
        project_id: &str,
        branch: &str,
        expected_head: Option<u64>,
        spillover_id: &str,
        outcome: DecisionOutcome,
        candidate_id: Option<String>,
        rationale: String,
    ) -> CmdResult<u64> {
        let view = self.view(project_id, branch, expected_head)?;
        let comp = view
            .compensation
            .as_ref()
            .ok_or_else(|| CommandError::bad("尚无补偿矩阵可供裁定"))?;
        if comp.spillover_id != spillover_id {
            return Err(CommandError::bad("spillover id 不匹配"));
        }
        if outcome == DecisionOutcome::AcceptedCandidate {
            let cid = candidate_id
                .as_ref()
                .ok_or_else(|| CommandError::bad("接受候选必须提供 candidate_id"))?;
            if !comp.verdict.candidates.iter().any(|c| &c.candidate_id == cid) {
                return Err(CommandError::bad("候选不存在；无法唯一决定时只能从合法候选中选"));
            }
        }
        let decision_id = format!(
            "dec-{}",
            &sha_hex(format!("{branch}:{spillover_id}:{:?}", outcome).as_bytes())[..10]
        );
        let (seq, _) = self.append(
            project_id,
            branch,
            None,
            0,
            expected_head,
            DomainEvent::CompensationDecided {
                decision_id,
                spillover_id: spillover_id.into(),
                outcome,
                candidate_id,
                rationale,
            },
            None,
        )?;
        Ok(seq)
    }

    pub fn fork(
        &self,
        project_id: &str,
        from_branch: &str,
        new_branch: &str,
        display_name: &str,
        expected_head: Option<u64>,
    ) -> CmdResult<u64> {
        let events = self.store.load_events(project_id).map_err(store_err)?;
        if events.iter().any(|e| {
            matches!(&e.payload, DomainEvent::ForkCreated { new_branch: nb, .. } if nb == new_branch)
        }) {
            return Err(CommandError::bad("分支名已存在"));
        }
        let forked_from_seq = expected_head
            .or_else(|| events.iter().filter(|e| e.branch == from_branch).map(|e| e.seq).max())
            .ok_or_else(|| CommandError::bad("源分支为空，无法分叉"))?;
        // fork 事件归属新分支，但它的 seq 仍在 fork 点之后；
        // 新分支可见性算法把它当作链入口，祖先事件继承自父分支截止 forked_from_seq。
        let seq = self
            .append(
                project_id,
                new_branch,
                Some(from_branch.into()),
                forked_from_seq + 1,
                None,
                DomainEvent::ForkCreated {
                    new_branch: new_branch.into(),
                    display_name: display_name.into(),
                    forked_from_seq,
                },
                None,
            )?
            .0;
        Ok(seq)
    }

    pub fn revert_decision(
        &self,
        project_id: &str,
        branch: &str,
        expected_head: Option<u64>,
        reverted_event_id: &str,
        note: String,
    ) -> CmdResult<u64> {
        let events = self.store.load_events(project_id).map_err(store_err)?;
        let exists = events.iter().any(|e| e.event_id == reverted_event_id);
        if !exists {
            return Err(CommandError::not_found("要撤销的裁定事件不存在"));
        }
        let (seq, _) = self.append(
            project_id,
            branch,
            None,
            0,
            expected_head,
            DomainEvent::DecisionReverted {
                reverted_event_id: reverted_event_id.into(),
                note,
            },
            None,
        )?;
        Ok(seq)
    }

    /// 两个窗口同时保存：后提交者拿到 409 + 双方差异，并把冲突作为机器事件留档。
    pub fn record_conflict(
        &self,
        project_id: &str,
        branch: &str,
        expected_head: u64,
        attempted_action: String,
        left_draft: String,
        right_draft: String,
        diff: Vec<String>,
    ) -> CmdResult<u64> {
        let actual = self.store.head_seq(project_id).map_err(store_err)?;
        let conflict_id = format!("cf-{}", &sha_hex(format!("{branch}:{expected_head}:{actual}").as_bytes())[..10]);
        let (seq, _) = self.append(
            project_id,
            branch,
            None,
            0,
            None,
            DomainEvent::VersionConflictRecorded {
                conflict_id,
                branch: branch.into(),
                expected_head,
                actual_head: actual,
                attempted_action,
                left_draft,
                right_draft,
                diff,
            },
            None,
        )?;
        Ok(seq)
    }
}

fn descendants(view: &ViewState, gate_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut frontier: Vec<String> = vec![gate_id.into()];
    while let Some(g) = frontier.pop() {
        for gate in view.gates.values() {
            if gate.parent.as_deref() == Some(g.as_str()) && !out.contains(&gate.gate_id) {
                out.push(gate.gate_id.clone());
                frontier.push(gate.gate_id.clone());
            }
        }
    }
    out
}

fn detect_cycle(edges: &[(String, Option<String>)]) -> Option<String> {
    for (g, _) in edges {
        let mut cur = g.clone();
        for _ in 0..edges.len() + 1 {
            let parent = edges
                .iter()
                .find_map(|(c, p)| (c == &cur).then_some(p.clone()).flatten());
            match parent {
                Some(p) => {
                    if p == *g {
                        return Some(g.clone());
                    }
                    cur = p;
                }
                None => break,
            }
        }
    }
    None
}
