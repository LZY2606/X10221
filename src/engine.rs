//! Core engine: append-only event timeline replayed into a derived snapshot.
//!
//! All derived state (compensation, transforms, gate memberships, percentages)
//! is recomputed by replaying the branch ancestry timeline, so returning to any
//! old event restores the exact dependency versions and visible conclusions.
//! New upstream gates mark downstream percentages stale rather than overwriting.

use crate::compute::{self, MatrixAnalysis};
use crate::domain::*;
use crate::store::{Store, CURRENT_SCHEMA};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const BASELINE_BRANCH: &str = "branch_main";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRequest {
    pub branch_id: String,
    #[serde(default)]
    pub expected_head: Option<String>,
    pub event_type: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitResponse {
    pub event_id: String,
    pub branch_id: String,
    pub seq: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchInfo {
    pub branch_id: String,
    pub label: String,
    pub parent_branch: Option<String>,
    pub base_event_id: Option<String>,
    pub head_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactView {
    pub kind: String,
    pub artifact_id: String,
    pub version: String,
    pub introducing_event_id: String,
    pub fingerprint: String,
    pub content: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventView {
    pub event_id: String,
    pub seq: i64,
    pub parent_event_id: Option<String>,
    pub event_type: String,
    pub label: Option<String>,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScatterPoint {
    pub uid: String,
    pub event_id: String,
    pub x: f64,
    pub y: f64,
    pub x_compensated: Option<f64>,
    pub flags: Vec<String>,
    pub in_gates: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TreeNode {
    pub uid: String,
    pub gate_id: String,
    pub name: String,
    pub parent_gate_id: Option<String>,
    pub percent_of_parent: Option<f64>,
    pub numerator: usize,
    pub denominator: usize,
    pub denominator_ref: Option<DenominatorRef>,
    pub status: String,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DenominatorRef {
    pub parent_gate_id: Option<String>,
    pub parent_gate_version: String,
    pub parent_result_event_id: String,
    pub scope_event_id: String,
    pub denominator: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompensationView {
    pub determinant: f64,
    pub near_singular: bool,
    pub threshold: f64,
    pub decided: Option<String>,
    pub candidates: Vec<compute::MatrixCandidate>,
    pub rows_before: Vec<Vec<f64>>,
    pub rows_after: Option<Vec<Vec<f64>>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JudgmentEvent {
    pub code: String,
    pub event_id: String,
    pub subject: String,
    pub evidence: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub branch_id: String,
    pub branch_label: String,
    pub head_event_id: String,
    pub scope_event_id: Option<String>,
    pub artifacts: BTreeMap<String, ArtifactView>,
    pub events: Vec<EventView>,
    pub scatter: Vec<ScatterPoint>,
    pub tree: Vec<TreeNode>,
    pub compensation: CompensationView,
    pub diagnostics: Vec<compute::ChannelDiagnostic>,
    pub judgments: Vec<JudgmentEvent>,
    pub transform_revision_count: i64,
}

fn fp(v: &Value) -> String {
    crate::store::fingerprint(&canonical(v))
}

pub fn canonical(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map.iter().collect();
            serde_json::to_string(&sorted).unwrap_or_default()
        }
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

pub struct CaseFiles {
    pub raw_events: &'static str,
    pub spill_meta: &'static str,
    pub matrix: &'static str,
    pub transform: &'static str,
    pub gates: &'static str,
    pub lineage: &'static str,
}

pub const CASE: CaseFiles = CaseFiles {
    raw_events: include_str!("../cases/flow-boundary-chamber/initial/raw_events.json"),
    spill_meta: include_str!("../cases/flow-boundary-chamber/initial/spillover_metadata.json"),
    matrix: include_str!("../cases/flow-boundary-chamber/initial/compensation_matrix.json"),
    transform: include_str!("../cases/flow-boundary-chamber/initial/transform_params.json"),
    gates: include_str!("../cases/flow-boundary-chamber/initial/gates.json"),
    lineage: include_str!("../cases/flow-boundary-chamber/initial/lineage.json"),
};

const KIND_FILES: [(ArtifactKind, &str); 6] = [
    (ArtifactKind::RawEvents, "raw_events"),
    (ArtifactKind::SpilloverMeta, "spill_meta"),
    (ArtifactKind::Matrix, "matrix"),
    (ArtifactKind::Transform, "transform"),
    (ArtifactKind::Gates, "gates"),
    (ArtifactKind::Lineage, "lineage"),
];

fn case_json(name: &str) -> Value {
    let s = match name {
        "raw_events" => CASE.raw_events,
        "spill_meta" => CASE.spill_meta,
        "matrix" => CASE.matrix,
        "transform" => CASE.transform,
        "gates" => CASE.gates,
        "lineage" => CASE.lineage,
        _ => unreachable!(),
    };
    serde_json::from_str(s).expect("embedded case JSON must parse")
}

impl Store {
    pub fn schema_current(&self) -> bool {
        self.schema_version().unwrap_or(0) == CURRENT_SCHEMA
    }

    /// Atomically load the offline case pack; failure rolls everything back.
    pub fn load_case(&self) -> rusqlite::Result<String> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        let exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM branches WHERE branch_id=?1",
            params![BASELINE_BRANCH],
            |r| r.get(0),
        )?;
        if exists > 0 {
            let head: Option<String> = tx
                .query_row(
                    "SELECT event_id FROM events WHERE branch_id=?1 ORDER BY seq DESC LIMIT 1",
                    params![BASELINE_BRANCH],
                    |r| r.get(0),
                )
                .ok();
            return Ok(head.unwrap_or_default());
        }
        let event_id = "ev_import_0001".to_string();
        tx.execute(
            "INSERT INTO branches(branch_id,parent_branch,base_event_id,label,created_event_seq)
             VALUES(?1,NULL,NULL,'main review line',0)",
            params![BASELINE_BRANCH],
        )?;
        tx.execute(
            "INSERT INTO events(event_id,branch_id,seq,parent_event_id,event_type,payload,created_at)
             VALUES(?1,?2,0,NULL,'import_baseline',?3,datetime('now'))",
            params![
                event_id,
                BASELINE_BRANCH,
                json!({"label":"initial import of flow-boundary-chamber"}).to_string()
            ],
        )?;
        for (kind, name) in KIND_FILES {
            let content = case_json(name);
            let artifact_id = content["artifact_id"].as_str().unwrap_or("art").to_string();
            let version = content["version"].as_str().unwrap_or("v1").to_string();
            tx.execute(
                "INSERT INTO artifacts(artifact_id,kind,version,branch_id,event_id,content,fingerprint)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    artifact_id,
                    kind.as_str(),
                    version,
                    BASELINE_BRANCH,
                    event_id,
                    content.to_string(),
                    fp(&content)
                ],
            )?;
        }
        tx.commit()?;
        Ok(event_id)
    }

    pub fn list_branches(&self) -> rusqlite::Result<Vec<BranchInfo>> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare(
            "SELECT branch_id,label,parent_branch,base_event_id FROM branches ORDER BY branch_id",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let bid: String = r.get(0)?;
            let head: Option<String> = c
                .query_row(
                    "SELECT event_id FROM events WHERE branch_id=?1 ORDER BY seq DESC LIMIT 1",
                    params![bid],
                    |r| r.get(0),
                )
                .ok();
            out.push(BranchInfo {
                branch_id: bid,
                label: r.get(1)?,
                parent_branch: r.get(2)?,
                base_event_id: r.get(3)?,
                head_event_id: head,
            });
        }
        Ok(out)
    }

    /// Append an event with optimistic concurrency. A mismatched expected head
    /// is rejected (HTTP 409 at the API layer) with the server head payload.
    pub fn commit_event(
        &self,
        req: &CommitRequest,
        event_id: &str,
    ) -> rusqlite::Result<Result<CommitResponse, ConflictInfo>> {
        if !self.branch_exists(&req.branch_id)? {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some("unknown branch".into()),
            ));
        }
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        let actual: Option<(String, i64)> = {
            let mut stmt = tx.prepare(
                "SELECT event_id,seq FROM events WHERE branch_id=?1 ORDER BY seq DESC LIMIT 1",
            )?;
            let mut rows = stmt.query(params![req.branch_id])?;
            match rows.next()? {
                Some(r) => Some((r.get(0)?, r.get(1)?)),
                None => None,
            }
        };
        if let Some(expected) = &req.expected_head {
            let actual_id = actual.as_ref().map(|(id, _)| id.as_str()).unwrap_or("");
            if actual_id != expected {
                let server_payload: Value = tx
                    .query_row(
                        "SELECT payload FROM events WHERE event_id=?1",
                        params![actual_id],
                        |r| {
                            let s: String = r.get(0)?;
                            Ok(serde_json::from_str(&s).unwrap_or(Value::Null))
                        },
                    )
                    .unwrap_or(Value::Null);
                return Ok(Err(ConflictInfo {
                    message: format!(
                        "branch {} moved to {} since you read head {}",
                        req.branch_id, actual_id, expected
                    ),
                    your_payload: req.payload.clone(),
                    server_head: server_payload,
                }));
            }
        }
        let seq = actual.map(|(_, s)| s + 1).unwrap_or(0);
        let parent = actual.map(|(id, _)| id);
        tx.execute(
            "INSERT INTO events(event_id,branch_id,seq,parent_event_id,event_type,payload,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,datetime('now'))",
            params![
                event_id,
                req.branch_id,
                seq,
                parent,
                req.event_type,
                req.payload.to_string()
            ],
        )?;
        tx.commit()?;
        Ok(Ok(CommitResponse {
            event_id: event_id.to_string(),
            branch_id: req.branch_id.clone(),
            seq,
        }))
    }

    pub fn create_branch(
        &self,
        branch_id: &str,
        label: &str,
        parent_branch: &str,
        base_event_id: &str,
    ) -> rusqlite::Result<()> {
        let mut c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO branches(branch_id,parent_branch,base_event_id,label) VALUES(?1,?2,?3,?4)",
            params![branch_id, parent_branch, base_event_id, label],
        )?;
        Ok(())
    }

    pub fn insert_judgment(
        &self,
        branch_id: &str,
        event_id: &str,
        seq: i64,
        code: &str,
        subject: &str,
        evidence: &Value,
    ) -> rusqlite::Result<()> {
        let mut c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO judgments(code,branch_id,event_id,subject,evidence,seq)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![code, branch_id, event_id, subject, evidence.to_string(), seq],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictInfo {
    pub message: String,
    pub your_payload: Value,
    pub server_head: Value,
}
