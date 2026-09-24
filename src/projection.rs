//! 投影：从事件链重放任意时刻状态；旧结论不被新事件覆盖，只能按 seq 回看。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::{
    ChannelDataset, CompensationProposal, DenominatorRef, GateVersion, PopulationResult,
    SpilloverSpec, TransformVersion, TreeEdgeVersion,
};
use crate::events::{EventPayload, ReviewAction, StaleMarkPayload, SubjectKind};
use crate::store::Db;

#[derive(Debug, Clone, Serialize)]
pub struct BranchInfo {
    pub branch_id: String,
    pub title: String,
    pub forked_from_branch: Option<String>,
    pub fork_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[derive(Debug, Clone, Serialize)]
pub struct DecisionState {
    pub decision_id: String,
    pub subject_kind: SubjectKind,
    pub subject_id: String,
    pub subject_version: u32,
    pub action: ReviewAction,
    pub candidate_id: Option<String>,
    pub note: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopState {
    pub result: PopulationResult,
    pub stale: bool,
    pub stale_reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct State {
    pub branch_id: String,
    pub seq: i64,
    pub title: String,
    pub forked_from_branch: Option<String>,
    pub fork_event_id: Option<String>,
    pub dataset: Option<ChannelDataset>,
    pub spillovers: BTreeMap<String, SpilloverSpec>,
    pub compensations: BTreeMap<String, CompensationProposal>,
    pub transforms: BTreeMap<String, TransformVersion>,
    pub gates: BTreeMap<String, GateVersion>,
    pub edges: BTreeMap<String, TreeEdgeVersion>,
    pub populations: BTreeMap<String, PopState>,
    pub decisions: BTreeMap<String, DecisionState>,
    pub archive_imports: Vec<String>,
}

impl State {
    pub fn accepted_compensation(&self) -> Option<&CompensationProposal> {
        self.compensations.values().find(|c| {
            self.decisions
                .values()
                .any(|d| d.active && d.subject_kind == SubjectKind::Compensation
                    && d.subject_id == c.compensation_id
                    && d.subject_version == c.version
                    && d.action == ReviewAction::Accept)
        })
    }
}

pub fn replay(events: Vec<crate::events::Event>) -> State {
    let mut state = State::default();
    let mut stale: BTreeMap<(String, u32), Vec<StaleMarkPayload>> = BTreeMap::new();
    for ev in events {
        state.seq = ev.seq;
        match ev.payload {
            EventPayload::BranchCreated { branch_id, title, forked_from_branch, fork_event_id } => {
                state.branch_id = branch_id;
                state.title = title;
                state.forked_from_branch = forked_from_branch;
                state.fork_event_id = fork_event_id;
            }
            EventPayload::BlobImported { .. } => {}
            EventPayload::DatasetRegistered { dataset } => {
                state.dataset = Some(dataset);
            }
            EventPayload::SpilloverRegistered { spec } => {
                state.spillovers.insert(spec.spillover_id.clone(), spec);
            }
            EventPayload::CompensationProposed { proposal } => {
                state.compensations.insert(proposal.compensation_id.clone(), proposal);
            }
            EventPayload::TransformSet { transform } => {
                state.transforms.insert(transform.transform_id.clone(), transform);
            }
            EventPayload::GateSet { gate } => {
                state.gates.insert(gate.gate_id.clone(), gate);
            }
            EventPayload::TreeEdgeSet { edge } => {
                state.edges.insert(edge.edge_id.clone(), edge);
            }
            EventPayload::PopulationComputed { result } => {
                state.populations.insert(
                    result.population_id.clone(),
                    PopState { result, stale: false, stale_reasons: vec![] },
                );
            }
            EventPayload::ReviewDecision { decision_id, subject_kind, subject_id,
                subject_version, action, candidate_id, note } => {
                state.decisions.insert(decision_id.clone(), DecisionState {
                    decision_id, subject_kind, subject_id, subject_version, action,
                    candidate_id, note, active: true,
                });
            }
            EventPayload::DecisionReverted { decision_id, note } => {
                if let Some(d) = state.decisions.get_mut(&decision_id) {
                    d.active = false;
                    d.note = format!("{}（已撤销: {}）", d.note, note);
                }
            }
            EventPayload::StaleMarks { marks } => {
                for mark in marks {
                    stale.entry((mark.population_id.clone(), mark.version)).or_default().push(mark);
                }
            }
            EventPayload::ArchiveImported { export_id } => {
                state.archive_imports.push(export_id);
            }
        }
    }
    for ((pid, ver), marks) in stale {
        if let Some(p) = state.populations.get_mut(&pid) {
            if p.result.version == ver {
                p.stale = true;
                p.stale_reasons = marks.iter().map(|m| m.reason.clone()).collect();
            }
        }
    }
    state
}

/// 分支事件链：fork 时把父分支到 fork 点的历史接在前面，实现“回到旧事件恢复当时依赖版本”。
pub fn branch_events(db: &Db, branch_id: &str, up_to_seq: Option<i64>)
    -> anyhow::Result<Vec<crate::events::Event>> {
    let branches = db.list_branches()?;
    let mut chain: Vec<crate::events::Event> = Vec::new();
    let mut current = branch_id.to_string();
    loop {
        let info = branches.iter().find(|b| b.branch_id == current);
        let (up, stop_at_fork) = if current == branch_id {
            (up_to_seq, false)
        } else {
            None, true
        };
        let events = db.events_on_branch(&current, up)?;
        let mut events = events;
        if stop_at_fork {
            // 找到 fork 事件：BranchCreated 携带 fork_event_id，截到它（含）。
            if let Some(fork_seq) = events.iter().find(|e| {
                matches!(&e.payload, EventPayload::BranchCreated { fork_event_id: Some(_), .. })
            }).map(|e| e.seq) {
                events.retain(|e| e.seq <= fork_seq);
            }
        }
        chain = events.into_iter().chain(chain).collect();
        match info.and_then(|b| b.forked_from_branch.clone()) {
            Some(parent) => current = parent,
            None => break,
        }
    }
    Ok(chain)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobRecord {
    pub blob_id: String,
    pub media_type: String,
    #[serde(with = "bytes_b64")]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchArchive {
    pub branch_id: String,
    pub title: String,
    pub forked_from_branch: Option<String>,
    pub fork_event_id: Option<String>,
    pub events: Vec<crate::events::Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Archive {
    pub format: String,
    pub export_id: String,
    pub created_at_unix: i64,
    pub branches: Vec<BranchArchive>,
    pub blobs: Vec<BlobRecord>,
}

pub fn export_archive(db: &Db, export_id: &str, at: i64) -> anyhow::Result<Archive> {
    let branches = db.list_branches()?;
    let mut out_branches = Vec::new();
    for b in branches {
        let events = db.events_on_branch(&b.branch_id, None)?;
        out_branches.push(BranchArchive {
            branch_id: b.branch_id,
            title: b.title,
            forked_from_branch: b.forked_from_branch,
            fork_event_id: b.fork_event_id,
            events,
        });
    }
    let blobs = db.all_blobs()?.into_iter()
        .map(|(blob_id, media_type, bytes)| BlobRecord { blob_id, media_type, bytes })
        .collect();
    Ok(Archive {
        format: "flow-gsb-archive/v1".to_string(),
        export_id: export_id.to_string(),
        created_at_unix: at,
        branches: out_branches,
        blobs,
    })
}

/// 导出档案里会被界面引用的派生校验值：分母引用必须真实存在。
pub fn check_denominator_integrity(state: &State) -> Vec<String> {
    let mut problems = Vec::new();
    for (id, p) in &state.populations {
        match &p.result.denominator {
            DenominatorRef::Dataset { dataset_id, version } => {
                match &state.dataset {
                    Some(d) if d.dataset_id == *dataset_id && d.version == *version => {}
                    _ => problems.push(format!("群体 {} 的分母引用了不存在的数据集 {}/v{}", id, dataset_id, version)),
                }
            }
            DenominatorRef::Population { population_id, version } => {
                match state.populations.get(population_id) {
                    Some(parent) if parent.result.version == *version => {}
                    _ => problems.push(format!("群体 {} 的分母钉住的父群 {}/v{} 缺失", id, population_id, version)),
                }
            }
        }
    }
    problems
}

mod bytes_b64 {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64_lite::b64_encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64_lite::b64_decode(&s).map_err(serde::de::Error::custom)
    }
}

mod base64_lite {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    pub fn b64_encode(data: &[u8]) -> String {
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
            let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(T[((n >> 18) & 63) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
            out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
            out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
        }
        out
    }
    pub fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
        let mut table = [255u8; 256];
        for (i, c) in T.iter().enumerate() {
            table[*c as usize] = i as u8;
        }
        let mut out = Vec::new();
        let mut buf = 0u32;
        let mut bits = 0u32;
        for c in s.bytes() {
            if c == b'=' || c == b'\n' || c == b'\r' {
                continue;
            }
            let v = table[c as usize];
            if v == 255 {
                return Err(format!("非法 base64 字符 {}", c as char));
            }
            buf = (buf << 6) | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push(((buf >> bits) & 0xff) as u8);
            }
        }
        Ok(out)
    }
}
