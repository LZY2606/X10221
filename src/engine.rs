//! Event-sourced review engine.
//!
//! The append-only ledger is the truth. Branch tip state is derived by
//! replaying events; historical views replay to a specific event. Source
//! material and interpretation are stored separately, reviewers never mutate
//! raw data, and source revisions mark stale downstream results without
//! overwriting them.

use crate::cid::cid_of;
use crate::compute::{
    coordinate, downstream, root_membership, run_compensation, run_gate, CompensationResult,
    DepSet, GateResult, PopResult,
};
use crate::db::{open, CURRENT_SCHEMA};
use crate::domain::*;
use crate::error::{Error, Result};
use crate::fcs;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const ROOT_POP: &str = "__root__";
pub const BOUNDARY_RULE_DEFAULT: crate::gate::BoundaryRule = crate::gate::BoundaryRule::Inclusive;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocVersion {
    pub doc_id: String,
    pub version: i64,
    pub cid: String,
}

/// A material element visible on the review page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialInfo {
    pub kind: String,
    pub doc_id: String,
    pub version: i64,
    pub cid: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateView {
    pub gate: GateDoc,
    pub parent: Option<String>,
    pub result: Option<GateResult>,
    pub population: Option<PopResult>,
    pub stale: bool,
    pub stale_reasons: Vec<String>,
    pub dep: DepSet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensationView {
    pub doc: Option<DocVersion>,
    pub spillover: Option<DocVersion>,
    pub result: Option<CompensationResult>,
    pub selected_candidate: Option<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSnapshot {
    pub branch_id: String,
    pub branch_name: String,
    pub parent_id: Option<String>,
    pub revision: i64,
    pub schema_version: u32,
    pub materials: Vec<MaterialInfo>,
    pub events_doc: Option<DocVersion>,
    pub compensation: CompensationView,
    pub transform_doc: Option<DocVersion>,
    pub lineage_doc: Option<DocVersion>,
    pub gates: Vec<GateView>,
    pub root_membership_cid: Option<String>,
    pub root_count: usize,
    pub events: Option<EventsDoc>,
    pub decisions: Vec<DecisionRecord>,
    pub stale: BTreeMap<String, Vec<String>>,
    pub diagnostics: Vec<ValueDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub decision_id: String,
    pub event_id: String,
    pub seq: i64,
    pub verdict: String,
    pub target_kind: String,
    pub target_id: String,
    pub adopted: bool,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub event_id: String,
    pub seq: i64,
    pub branch_id: String,
    pub kind: String,
    pub basis: Value,
    pub supersedes: Option<String>,
    pub created_at: i64,
    pub attachments: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

pub struct Engine {
    pub conn: Connection,
}

impl Engine {
    pub fn open(path: &str) -> Result<Self> {
        Ok(Self { conn: open(path)? })
    }
    pub fn from_conn(conn: Connection) -> Self {
        Self { conn }
    }

    pub fn schema_version(&self) -> Result<u32> {
        Ok(self.conn.query_row(
            "SELECT value FROM schema_meta WHERE key='version'", [], |r| r.get::<_, i64>(0))? as u32)
    }

    pub fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    // -- documents ----------------------------------------------------------

    fn latest_doc(&self, kind: &str, doc_id: &str) -> Result<Option<(i64, String, String)>> {
        self.conn
            .query_row(
                "SELECT version, cid, content FROM documents WHERE kind=?1 AND doc_id=?2 ORDER BY version DESC LIMIT 1",
                params![kind, doc_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e.into()),
            })
    }

    fn doc_at(
        &self,
        kind: &str,
        doc_id: &str,
        version: i64,
    ) -> Result<(String, String)> {
        self.conn.query_row(
            "SELECT cid, content FROM documents WHERE kind=?1 AND doc_id=?2 AND version=?3",
            params![kind, doc_id, version],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Error::NotFound(format!("{kind}/{doc_id}v{version}")),
            e => e.into(),
        })
    }

    pub fn put_doc<T: Serialize>(
        &mut self,
        kind: &str,
        doc_id: &str,
        content: &T,
    ) -> Result<DocVersion> {
        let json = serde_json::to_string(content)?;
        let cid = cid_of(kind, content);
        let next: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version),0)+1 FROM documents WHERE kind=?1 AND doc_id=?2",
                params![kind, doc_id],
                |r| r.get(0),
            )?;
        let parent: Option<String> = self
            .conn
            .query_row(
                "SELECT cid FROM documents WHERE kind=?1 AND doc_id=?2 ORDER BY version DESC LIMIT 1",
                params![kind, doc_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();
        self.conn.execute(
            "INSERT INTO documents(kind,doc_id,version,cid,parent_cid,content) VALUES(?1,?2,?3,?4,?5,?6)",
            params![kind, doc_id, next, cid, parent, json],
        )?;
        Ok(DocVersion { doc_id: doc_id.into(), version: next, cid })
    }

    // -- branches -----------------------------------------------------------

    pub fn create_branch(&mut self, name: &str, parent_id: Option<String>, forked_from: Option<String>) -> Result<String> {
        let id = format!("br_{}", &cid_of("branch", &(name, parent_id.clone(), self.now_ms()))[4..20]);
        self.conn.execute(
            "INSERT INTO branches(branch_id,name,parent_id,forked_from,revision,open,created_at)
             VALUES(?1,?2,?3,?4,0,1,?5)",
            params![id, name, parent_id, forked_from, self.now_ms()],
        )?;
        Ok(id)
    }

    pub fn list_branches(&self) -> Result<Vec<(String, String, i64, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT branch_id,name,revision,parent_id FROM branches WHERE open=1 ORDER BY created_at")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn branch_revision(&self, branch_id: &str) -> Result<i64> {
        self.conn.query_row(
            "SELECT revision FROM branches WHERE branch_id=?1", params![branch_id],
            |r| r.get(0)).map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Error::NotFound(format!("branch {branch_id}")),
            e => e.into(),
        })
    }

    /// Append an event with optimistic concurrency. `base_revision` is the
    /// revision the caller read; a concurrent commit moves the tip first and
    /// the later submitter gets a Conflict containing both diffs.
    fn append_event(
        &mut self,
        branch_id: &str,
        kind: &str,
        basis: Value,
        base_revision: i64,
        supersedes: Option<String>,
        attachments: BTreeMap<String, String>,
        ref_blobs: Vec<String>,
        ref_docs: Vec<String>,
    ) -> Result<String> {
        let current = self.branch_revision(branch_id)?;
        if base_revision != current {
            return Err(Error::Conflict { base: base_revision, tip: current });
        }
        let next = current + 1;
        let event_id = format!(
            "ev_{}",
            &cid_of("event", &(branch_id, next, kind, &basis, self.now_ms()))[4..24]
        );
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO branch_events(event_id,seq,branch_id,kind,basis_json,ref_blob_ids,ref_doc_ids,supersedes,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                event_id, next, branch_id, kind,
                serde_json::to_string(&basis)?,
                serde_json::to_string(&ref_blobs)?,
                serde_json::to_string(&ref_docs)?,
                supersedes, self.now_ms()
            ],
        )?;
        for (name, content) in &attachments {
            tx.execute(
                "INSERT INTO event_attachments(event_id,name,content) VALUES(?1,?2,?3)",
                params![event_id, name, content],
            )?;
        }
        tx.execute("UPDATE branches SET revision=?1 WHERE branch_id=?2", params![next, branch_id])?;
        tx.commit()?;
        Ok(event_id)
    }

    pub fn events(&self, branch_id: &str) -> Result<Vec<EventRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id,seq,branch_id,kind,basis_json,supersedes,created_at
             FROM branch_events WHERE branch_id=?1 ORDER BY seq")?;
        let mut out = Vec::new();
        let rows = stmt.query_map(params![branch_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?,
                r.get::<_, String>(3)?, r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?, r.get::<_, i64>(6)?))
        })?;
        for row in rows {
            let (event_id, seq, b, kind, basis_json, supersedes, created_at) = row?;
            let mut att = BTreeMap::new();
            {
                let mut s = self.conn.prepare(
                    "SELECT name,content FROM event_attachments WHERE event_id=?1")?;
                let a = s.query_map(params![event_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                for x in a { let (k, v): (String, String) = x?; att.insert(k, v); }
            }
            out.push(EventRecord {
                event_id, seq, branch_id: b, kind,
                basis: serde_json::from_str(&basis_json)?,
                supersedes, created_at, attachments: att,
            });
        }
        Ok(out)
    }
}
