//! Core lineage engine: append-only events, branches, optimistic concurrency,
//! derived recomputation, decisions and invalidation.
//!
//! Storage model: every state-changing action is one immutable `events` row.
//! A branch's visible state at any point is replayed from the log. Derived
//! objects are never overwritten when upstream changes; a new version is
//! produced and the previous one remains readable and is marked stale.

use super::db::Db;
use super::model::*;
use crate::engine::math::{self, BoundaryKind, LogicleParams, MatrixStatus};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct EventRecord {
    pub event_id: i64,
    pub branch_id: String,
    pub seq: i64,
    pub kind: String,
    pub node_kind: Option<String>,
    pub node_id: Option<String>,
    pub version_uid: Option<String>,
    pub payload: Value,
    pub parent_event: Option<i64>,
    pub base_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchInfo {
    pub branch_id: String,
    pub name: String,
    pub parent_branch: Option<String>,
    pub parent_seq: Option<i64>,
    pub head_seq: i64,
}

#[derive(Debug, Deserialize)]
pub struct ActionInput {
    pub branch_id: String,
    pub kind: String,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub parent_event: Option<i64>,
    /// Optimistic-concurrency token: head hash the client read.
    #[serde(default)]
    pub base_hash: Option<String>,
    /// For singular decisions.
    #[serde(default)]
    pub chosen_candidate_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ActionOutput {
    pub event_id: i64,
    pub seq: i64,
    pub branch_id: String,
    pub version_uid: Option<String>,
    pub head_hash: String,
    /// Events produced together with the action (derived recomputation).
    pub produced: Vec<ProducedEvent>,
    /// Node versions invalidated by this action (IDs only, nothing deleted).
    pub invalidated: Vec<String>,
    /// Error candidate context returned on 409.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<ConflictDetail>,
}

#[derive(Debug, Serialize)]
pub struct ProducedEvent {
    pub event_id: i64,
    pub seq: i64,
    pub kind: String,
    pub node_id: Option<String>,
    pub version_uid: Option<String>,
    pub blocked: bool,
}

#[derive(Debug, Serialize)]
pub struct ConflictDetail {
    pub your_parent_event: Option<i64>,
    pub current_head_seq: i64,
    pub divergent_events: Vec<EventRecord>,
}

pub struct Engine {
    pub db: Db,
}

fn hash_head(branch_id: &str, seq: i64) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    branch_id.hash(&mut h);
    seq.hash(&mut h);
    format!("{:016x}", h.finish())
}

impl Engine {
    pub fn new(db: Db) -> Self {
        Engine { db }
    }

    pub fn list_branches(&self) -> Vec<BranchInfo> {
        let c = self.db.lock_conn();
        let mut stmt = c
            .prepare(
                "SELECT branch_id,name,parent_branch,parent_seq,head_seq FROM branches
                 ORDER BY created_event_id",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok(BranchInfo {
                branch_id: r.get(0)?,
                name: r.get(1)?,
                parent_branch: r.get(2)?,
                parent_seq: r.get(3)?,
                head_seq: r.get(4)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn ensure_branch(&self, id: &str, name: &str) -> Result<(), String> {
        let mut c = self.db.lock_conn();
        let tx = c.transaction().map_err(|e| e.to_string())?;
        let exists: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM branches WHERE branch_id=?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        if exists == 0 {
            tx.execute(
                "INSERT INTO branches(branch_id,name,parent_branch,parent_seq,created_event_id,head_seq)
                 VALUES(?1,?2,NULL,NULL,0,0)",
                params![id, name],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// Create a branch forking from `parent` at a point in time. All events up
    /// to and including `at_seq` remain visible; later parent events do not.
    pub fn fork_branch(
        &self,
        new_id: &str,
        name: &str,
        parent: &str,
        at_seq: Option<i64>,
    ) -> Result<(), String> {
        let mut c = self.db.lock_conn();
        let tx = c.transaction().map_err(|e| e.to_string())?;
        let head: i64 = tx
            .query_row(
                "SELECT head_seq FROM branches WHERE branch_id=?1",
                params![parent],
                |r| r.get(0),
            )
            .map_err(|_| "parent branch not found".to_string())?;
        let at = at_seq.unwrap_or(head).min(head);
        tx.execute(
            "INSERT INTO branches(branch_id,name,parent_branch,parent_seq,created_event_id,head_seq)
             VALUES(?1,?2,?3,?4,0,0)",
            params![new_id, name, parent, at],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn head_seq(&self, branch: &str) -> Result<i64, String> {
        let c = self.db.lock_conn();
        c.query_row(
            "SELECT head_seq FROM branches WHERE branch_id=?1",
            params![branch],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
    }

    pub fn head_hash(&self, branch: &str) -> Result<String, String> {
        Ok(hash_head(branch, self.head_seq(branch)?))
    }

    fn fetch_event(conn: &Connection, event_id: i64) -> Result<EventRecord, String> {
        conn.query_row(
            "SELECT event_id,branch_id,seq,kind,node_kind,node_id,version_uid,payload,parent_event,base_hash
             FROM events WHERE event_id=?1",
            params![event_id],
            |r| {
                let payload: String = r.get(7)?;
                Ok(EventRecord {
                    event_id: r.get(0)?,
                    branch_id: r.get(1)?,
                    seq: r.get(2)?,
                    kind: r.get(3)?,
                    node_kind: r.get(4)?,
                    node_id: r.get(5)?,
                    version_uid: r.get(6)?,
                    payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                    parent_event: r.get(8)?,
                    base_hash: r.get(9)?,
                })
            },
        )
        .map_err(|e| e.to_string())
    }

    pub fn get_event(&self, event_id: i64) -> Result<EventRecord, String> {
        let c = self.db.lock_conn();
        Self::fetch_event(&c, event_id)
    }
}

impl Engine {
    fn list_events(&self, branch: &str) -> Result<Vec<EventRecord>, String> {
        let c = self.db.lock_conn();
        self.list_events_conn(&c, branch)
    }

    fn list_events_conn(&self, conn: &Connection, branch: &str) -> Result<Vec<EventRecord>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT event_id,branch_id,seq,kind,node_kind,node_id,version_uid,payload,parent_event,base_hash
                 FROM events WHERE branch_id=?1 ORDER BY seq",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![branch], |r| {
                let payload: String = r.get(7)?;
                Ok(EventRecord {
                    event_id: r.get(0)?,
                    branch_id: r.get(1)?,
                    seq: r.get(2)?,
                    kind: r.get(3)?,
                    node_kind: r.get(4)?,
                    node_id: r.get(5)?,
                    version_uid: r.get(6)?,
                    payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                    parent_event: r.get(8)?,
                    base_hash: r.get(9)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    /// Apply one action. The whole action and any derived recomputation it
    /// triggers commit in a single transaction; a failure rolls everything back.
    pub fn apply(&self, input: &ActionInput) -> Result<ActionOutput, String> {
        let branch = input.branch_id.clone();
        let kind = input.kind.clone();
        let mut c = self.db.lock_conn();
        let tx = c.transaction().map_err(|e| e.to_string())?;

        let head: i64 = tx
            .query_row(
                "SELECT head_seq FROM branches WHERE branch_id=?1",
                params![branch],
                |r| r.get(0),
            )
            .map_err(|_| format!("unknown branch {branch}"))?;

        if let Some(token) = &input.base_hash {
            if token != &hash_head(&branch, head) {
                let divergent = self.list_events_conn(&tx, &branch)?;
                let after_parent = match input.parent_event {
                    Some(pe) => divergent.into_iter().filter(|e| e.event_id > pe).collect(),
                    None => divergent,
                };
                return Err(serde_json::to_string(&ConflictDetail {
                    your_parent_event: input.parent_event,
                    current_head_seq: head,
                    divergent_events: after_parent,
                })
                .unwrap());
            }
        }

        let seq = head + 1;
        let payload_str = serde_json::to_string(&input.payload).unwrap();
        let node_kind = node_kind_for(&kind);
        let node_id = input.node_id.clone();
        let event_id = self.db.append_event_tx(
            &tx,
            &branch,
            seq,
            &kind,
            node_kind,
            node_id.as_deref(),
            None,
            &payload_str,
            input.parent_event,
            input.base_hash.as_deref(),
        )?;
        let version_uid = match node_kind {
            Some(_) => Some(format!("{node_id}@{event_id}")),
            None => None,
        };
        if version_uid.is_some() {
            tx.execute(
                "UPDATE events SET version_uid=?1 WHERE event_id=?2",
                params![version_uid, event_id],
            )
            .map_err(|e| e.to_string())?;
        }

        let mut produced = Vec::new();
        let mut invalidated = Vec::new();

        match kind.as_str() {
            "compensation_requested" => {
                let res = self.produce_compensation(&tx, &branch, event_id, input)?;
                if res.is_some() {
                    let (p, inv) = res.unwrap();
                    produced.push(p.clone());
                    invalidated = inv;
                }
            }
            "compensated_requested" => {
                let p = self.do_recompute_compensated(&tx, &branch, event_id)?;
                invalidated = self.find_downstream(
                    &tx,
                    &branch,
                    std::slice::from_ref(&("compensated_events".to_string(), "cev#main".to_string())),
                );
                produced.push(p);
            }
            "population_requested" => {
                let gate_id = input
                    .node_id
                    .clone()
                    .ok_or("population_requested needs node_id")?;
                let p = self.do_recompute_population(&tx, &branch, &gate_id, event_id)?;
                invalidated = self.find_downstream(
                    &tx,
                    &branch,
                    std::slice::from_ref(&("population".to_string(), pop_node(&gate_id))),
                );
                produced.push(p);
            }
            "decision_recorded" => {
                self.record_decision(&tx, &branch, event_id, input)?;
            }
            "decision_reversed" => {
                self.reverse_decision(&tx, &branch, event_id, input)?;
            }
            _ => {
                // Authored revision or import: invalidate only downstream
                // derived versions in dependency closure; recompute on demand
                // via explicit recompute_* actions, so stale results stay put.
                invalidated = self.invalidate_for_authored(&tx, &branch, &kind, node_id.as_deref());
            }
        }

        tx.execute(
            "UPDATE branches SET head_seq=?1 WHERE branch_id=?2",
            params![seq, branch],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;

        let head_hash = hash_head(&branch, seq);
        Ok(ActionOutput {
            event_id,
            seq,
            branch_id,
            version_uid,
            head_hash,
            produced,
            invalidated,
            conflict: None,
        })
    }

    fn invalidate_for_authored(
        &self,
        conn: &Connection,
        branch: &str,
        kind: &str,
        node_id: Option<&str>,
    ) -> Vec<String> {
        let upstream = match kind.strip_suffix("_versioned") {
            Some(base) => base,
            None => match kind {
                "spillover_revised" => "spillover",
                "transform_revised" => "transform_params",
                "gate_revised" | "gate_renamed" => "gate_polygon",
                "edges_revised" => "gate_edges",
                "raw_events_imported" => "raw_events",
                _ => return Vec::new(),
            },
        };
        let node = node_id.unwrap_or_default();
        self.find_downstream(
            conn,
            branch,
            std::slice::from_ref(&(upstream.to_string(), node)),
        )
    }

    /// Transitive invalidation closure: given (kind,node) seeds, return every
    /// derived version-uid that (transitively) depends on them. Nothing is
    /// deleted or overwritten; the list is surfaced for diagnostics.
    fn find_downstream(
        &self,
        conn: &Connection,
        branch: &str,
        seeds: &[(String, String)],
    ) -> Vec<String> {
        let mut found = Vec::new();
        let mut stack: Vec<(String, String)> = seeds.iter().cloned().collect();
        let mut seen = std::collections::HashSet::new();
        while let Some((kind, node)) = stack.pop() {
            if !seen.insert((kind.clone(), node.clone())) {
                continue;
            }
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT d.producer_event_id, e.node_kind, e.node_id, e.version_uid
                     FROM deps d JOIN events e ON e.event_id=d.producer_event_id
                     WHERE e.branch_id=?1 AND d.upstream_kind=?2 AND (?3='' OR d.upstream_node=?3)",
                )
                .unwrap();
            let rows = stmt
                .query_map(params![branch, kind, node], |r| {
                    let pid: i64 = r.get(0)?;
                    let pk: String = r.get(1)?;
                    let pn: String = r.get(2)?;
                    let pvu: String = r.get(3)?;
                    Ok((pid, pk, pn, pvu))
                })
                .unwrap();
            for r in rows.flatten() {
                found.push(r.3);
                stack.push((r.1, r.2));
            }
        }
        found.sort();
        found.dedup();
        found
    }
}

fn node_kind_for(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "raw_events_imported" => "raw_events",
        "spillover_versioned" | "spillover_revised" => "spillover",
        "compensation_versioned" => "compensation_matrix",
        "compensation_blocked" => "compensation_matrix",
        "transform_versioned" | "transform_revised" => "transform_params",
        "gate_versioned" | "gate_revised" | "gate_renamed" => "gate_polygon",
        "edges_versioned" | "edges_revised" => "gate_edges",
        "compensated_events_versioned" => "compensated_events",
        "population_versioned" => "population",
        _ => return None,
    })
}

impl Engine {
    fn current_payload(
        &self,
        conn: &Connection,
        branch: &str,
        node_kind: &str,
        node_id: Option<&str>,
    ) -> Result<(EventRecord, Value), String> {
        let mut sql = format!(
            "SELECT event_id,branch_id,seq,kind,node_kind,node_id,version_uid,payload,parent_event,base_hash
             FROM events WHERE branch_id=?1 AND node_kind=?2"
        );
        if node_id.is_some() {
            sql.push_str(" AND node_id=?3 ORDER BY seq DESC LIMIT 1");
        } else {
            sql.push_str(" ORDER BY seq DESC LIMIT 1");
        }
        let row = if let Some(n) = node_id {
            conn.query_row(&sql, params![branch, node_kind, n], map_event)
        } else {
            conn.query_row(&sql, params![branch, node_kind], map_event)
        };
        row.map_err(|e| format!("missing current {node_kind}: {e}"))
    }

    fn insert_derived(
        &self,
        conn: &Connection,
        branch: &str,
        kind: &str,
        node_kind: &str,
        node_id: &str,
        payload: &Value,
        deps: &[(String, String, String)],
        head_hint: i64,
    ) -> Result<ProducedEvent, String> {
        let seq = conn.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE branch_id=?1", rusqlite::params![branch], |r| r.get::<_,i64>(0)).map_err(|e| e.to_string())?;
        let payload_str = serde_json::to_string(payload).unwrap();
        let id = self.db.append_event_tx(
            conn,
            branch,
            seq,
            kind,
            Some(node_kind),
            Some(node_id),
            None,
            &payload_str,
            None,
            None,
        )?;
        let vuid = format!("{node_id}@{id}");
        conn.execute(
            "UPDATE events SET version_uid=?1 WHERE event_id=?2",
            params![vuid, id],
        )
        .map_err(|e| e.to_string())?;
        for (k, n, v) in deps {
            conn.execute(
                "INSERT INTO deps(producer_event_id,upstream_kind,upstream_node,upstream_version_uid)
                 VALUES(?1,?2,?3,?4)
                 ON CONFLICT DO NOTHING",
                params![id, k, n, v],
            )
            .map_err(|e| e.to_string())?;
        }
        conn.execute(
            "UPDATE branches SET head_seq=?1 WHERE branch_id=?2",
            params![seq, branch],
        )
        .map_err(|e| e.to_string())?;
        let _ = head_hint;
        Ok(ProducedEvent {
            event_id: id,
            seq,
            kind: kind.to_string(),
            node_id: Some(node_id.to_string()),
            version_uid: Some(vuid),
            blocked: false,
        })
    }

    fn produce_compensation(
        &self,
        conn: &Connection,
        branch: &str,
        req_event: i64,
        input: &ActionInput,
    ) -> Result<Option<(ProducedEvent, Vec<String>)>, String> {
        let chosen: Option<String> = input
            .payload
            .get("chosen_candidate_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| input.chosen_candidate_id.clone());
        let (sp_ev, sp_val) = self.current_payload(conn, branch, "spillover", None)?;
        let spill: SpilloverMeta =
            serde_json::from_value(sp_val).map_err(|e| e.to_string())?;
        let report = math::analyze_matrix(&spill.matrix);
        let sp_vuid = sp_ev.version_uid.clone().unwrap();
        let deps = vec![(
            "spillover".to_string(),
            sp_ev.node_id.clone().unwrap_or_default(),
            sp_vuid.clone(),
        )];
        match report.status {
            MatrixStatus::Invertible => {
                let inv = math::invert_matrix(&spill.matrix).unwrap();
                let comp = CompensationMatrix {
                    channels: spill.channels.clone(),
                    matrix: inv,
                    rcond: report.rcond,
                    determinant: report.determinant,
                    basis: "inverse of spillover matrix S^-1; measured ADC counts".to_string(),
                    note: report.reason,
                };
                let p = self.insert_derived(
                    conn,
                    branch,
                    "compensation_versioned",
                    "compensation_matrix",
                    "comp#main",
                    &serde_json::to_value(&comp).unwrap(),
                    &deps,
                    req_event,
                )?;
                let inv_list = self.find_downstream(
                    conn,
                    branch,
                    std::slice::from_ref(&("compensation_matrix".to_string(),
                        "comp#main".to_string())),
                );
                Ok(Some((p, inv_list)))
            }
            MatrixStatus::NearSingular => {
                if let Some(cid) = chosen {
                    let cands = math::ridge_candidates(&spill.matrix);
                    let cand = cands
                        .iter()
                        .find(|c| c.id == cid)
                        .ok_or_else(|| format!("unknown candidate {cid}"))?;
                    let comp = CompensationMatrix {
                        channels: spill.channels.clone(),
                        matrix: cand.matrix.clone(),
                        rcond: cand.rcond,
                        determinant: report.determinant,
                        basis: format!(
                            "ridge-regularized pseudo-compensation λ={}; NOT an exact inverse",
                            cand.lambda
                        ),
                        note: format!(
                            "{}; chosen by human decision; max residual {}",
                            report.reason, cand.max_residual
                        ),
                    };
                    let p = self.insert_derived(
                        conn,
                        branch,
                        "compensation_versioned",
                        "compensation_matrix",
                        "comp#main",
                        &serde_json::to_value(&comp).unwrap(),
                        &deps,
                        req_event,
                    )?;
                    let inv_list = self.find_downstream(
                        conn,
                        branch,
                        std::slice::from_ref(&("compensation_matrix".to_string(),
                            "comp#main".to_string())),
                    );
                    return Ok(Some((p, inv_list)));
                }
                // Withhold inversion; persist all legal candidates explicitly.
                let blocked = CompensationMatrix {
                    channels: spill.channels.clone(),
                    matrix: vec![],
                    rcond: report.rcond,
                    determinant: report.determinant,
                    basis: "inversion withheld: near-singular spillover".to_string(),
                    note: report.reason,
                };
                let p = self.insert_derived(
                    conn,
                    branch,
                    "compensation_blocked",
                    "compensation_matrix",
                    "comp#main",
                    &serde_json::to_value(&blocked).unwrap(),
                    &deps,
                    req_event,
                )?;
                let cands = math::ridge_candidates(&spill.matrix);
                for cand in &cands {
                    conn.execute(
                        "INSERT INTO candidates(candidate_id,branch_id,node_id,produced_event_id,kind,label,payload)
                         VALUES(?1,?2,?3,?4,'ridge',?5,?6)
                         ON CONFLICT(candidate_id) DO UPDATE SET payload=excluded.payload",
                        params![
                            cand.id,
                            branch,
                            "comp#main",
                            p.event_id,
                            cand.label,
                            serde_json::to_string(cand).unwrap()
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                }
                Ok(Some((p, vec![])))
            }
        }
    }

    fn record_decision(
        &self,
        conn: &Connection,
        branch: &str,
        event_id: i64,
        input: &ActionInput,
    ) -> Result<(), String> {
        let node = input
            .node_id
            .clone()
            .ok_or("decision requires node_id")?;
        let key = format!("{branch}:{node}");
        let chosen = input
            .payload
            .get("chosen_candidate_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        conn.execute(
            "INSERT INTO decisions(decision_key,branch_id,node_id,active,chosen_candidate_id,decided_event_id)
             VALUES(?1,?2,?3,1,?4,?5)
             ON CONFLICT(decision_key) DO UPDATE SET active=1, chosen_candidate_id=excluded.chosen_candidate_id,
                 decided_event_id=excluded.decided_event_id",
            params![key, branch, node, chosen, event_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn reverse_decision(
        &self,
        conn: &Connection,
        branch: &str,
        event_id: i64,
        input: &ActionInput,
    ) -> Result<(), String> {
        let node = input
            .node_id
            .clone()
            .ok_or("decision reversal requires node_id")?;
        let key = format!("{branch}:{node}");
        conn.execute(
            "INSERT INTO decisions(decision_key,branch_id,node_id,active,chosen_candidate_id,decided_event_id)
             VALUES(?1,?2,?3,0,NULL,?4)
             ON CONFLICT(decision_key) DO UPDATE SET active=0, chosen_candidate_id=NULL,
                 decided_event_id=excluded.decided_event_id",
            params![key, branch, node, event_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn map_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> {
    let payload: String = r.get(7)?;
    Ok(EventRecord {
        event_id: r.get(0)?,
        branch_id: r.get(1)?,
        seq: r.get(2)?,
        kind: r.get(3)?,
        node_kind: r.get(4)?,
        node_id: r.get(5)?,
        version_uid: r.get(6)?,
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        parent_event: r.get(8)?,
        base_hash: r.get(9)?,
    })
}

impl Engine {
    pub fn recompute_compensated(&self, branch: &str) -> Result<ActionOutput, String> {
        self.apply(&ActionInput {
            branch_id: branch.to_string(),
            kind: "compensated_requested".to_string(),
            node_id: None,
            payload: Value::Null,
            parent_event: None,
            base_hash: None,
            chosen_candidate_id: None,
        })
    }

    pub fn recompute_population(
        &self,
        branch: &str,
        gate_id: &str,
        base_hash: Option<String>,
    ) -> Result<ActionOutput, String> {
        self.apply(&ActionInput {
            branch_id: branch.to_string(),
            kind: "population_requested".to_string(),
            node_id: Some(gate_id.to_string()),
            payload: serde_json::json!({"gate_id": gate_id}),
            parent_event: None,
            base_hash,
            chosen_candidate_id: None,
        })
    }

    fn do_recompute_compensated(
        &self,
        conn: &Connection,
        branch: &str,
        req_event: i64,
    ) -> Result<ProducedEvent, String> {
        let (raw_ev, raw_val) = self.current_payload(conn, branch, "raw_events", None)?;
        let (comp_ev, comp_val) =
            self.current_payload(conn, branch, "compensation_matrix", Some("comp#main"))?;
        if comp_ev.kind == "compensation_blocked" {
            return Err("compensation blocked: near-singular matrix; a human decision is required".to_string());
        }
        let raw: RawBatch = serde_json::from_value(raw_val).unwrap();
        let comp: CompensationMatrix = serde_json::from_value(comp_val).unwrap();
        let idx: Vec<usize> = comp
            .channels
            .iter()
            .map(|ch| {
                raw.channel_names
                    .iter()
                    .position(|c| c == ch)
                    .ok_or_else(|| format!("channel {ch} missing from raw batch"))
            })
            .collect::<Result<_, _>>()?;
        let mut values = Vec::with_capacity(raw.event_count);
        let mut diagnostics = Vec::new();
        for (ei, row) in raw.values.iter().enumerate() {
            let mut measured = Vec::with_capacity(idx.len());
            let mut missing = false;
            for &&ci in &idx {
                match row[ci].numeric {
                    Some(v) => measured.push(v),
                    None => {
                        missing = true;
                        diagnostics.push(format!(
                            "event {ei} channel {}: token \"{}\" not finite; row retained with null",
                            raw.channel_names[ci], row[ci].token
                        ));
                    }
                }
            }
            if missing {
                values.push(vec![None; idx.len()]);
                continue;
            }
            let out = math::mat_vec(&comp.matrix, &measured);
            let mut mapped = Vec::with_capacity(out.len());
            for (k, v) in out.into_iter().enumerate() {
                if v.is_finite() {
                    mapped.push(Some(v));
                } else {
                    mapped.push(None);
                    diagnostics.push(format!(
                        "event {ei} channel {}: compensated value non-finite under {}; retained as null",
                        comp.channels[k], comp_ev.version_uid.as_ref().unwrap()
                    ));
                }
            }
            values.push(mapped);
        }
        let set = CompensatedEventSet {
            raw_version: raw_ev.version_uid.clone().unwrap(),
            spillover_version: String::new(),
            compensation_version: comp_ev.version_uid.clone().unwrap(),
            channels: comp.channels.clone(),
            values,
            diagnostics,
            blocked: false,
            candidates: vec![],
        };
        let deps = vec![
            (
                "raw_events".to_string(),
                raw_ev.node_id.clone().unwrap_or_default(),
                set.raw_version.clone(),
            ),
            (
                "compensation_matrix".to_string(),
                "comp#main".to_string(),
                set.compensation_version.clone(),
            ),
        ];
        self.insert_derived(
            conn,
            branch,
            "compensated_events_versioned",
            "compensated_events",
            "cev#main",
            &serde_json::to_value(&set).unwrap(),
            &deps,
            req_event,
        )
    }

    fn do_recompute_population(
        &self,
        conn: &Connection,
        branch: &str,
        gate_id: &str,
        req_event: i64,
    ) -> Result<ProducedEvent, String> {
        let c = conn;
        let (gate_ev, gate_val) =
            self.current_payload(c, branch, "gate_polygon", Some(gate_id))?;
        let gate: GatePolygon = serde_json::from_value(gate_val).unwrap();
        let (edges_ev, edges_val) = self.current_payload(c, branch, "gate_edges", None)?;
        let edges: GateEdges = serde_json::from_value(edges_val).unwrap();
        let (tr_ev, tr_val) = self.current_payload(c, branch, "transform_params", None)?;
        let transform: TransformParams = serde_json::from_value(tr_val).unwrap();
        let (cev_ev, cev_val) =
            self.current_payload(c, branch, "compensated_events", Some("cev#main"))?;
        let cev: CompensatedEventSet = serde_json::from_value(cev_val).unwrap();
        let (raw_ev, raw_val) = self.current_payload(c, branch, "raw_events", None)?;
        let raw: RawBatch = serde_json::from_value(raw_val).unwrap();

        let edge = edges
            .edges
            .iter()
            .find(|e| e.child_gate == gate_id)
            .ok_or_else(|| format!("gate {gate_id} has no parent edge"))?;

        // Parent population version is PINNED at the one currently visible for
        // the parent gate. Percentages never silently follow "latest".
        let (denom_version, denom_label, denominator_count, parent_members): (
            String,
            String,
            usize,
            Option<std::collections::HashSet<usize>>,
        ) = match &edge.parent_gate {
            Some(pg) => {
                let (p_ev, p_val) =
                    self.current_payload(c, branch, "population", Some(&pop_node(pg)))?;
                let p: Population = serde_json::from_value(p_val).unwrap();
                let members: std::collections::HashSet<usize> = p
                    .event_classes
                    .iter()
                    .filter(|c| c.included)
                    .map(|c| c.event_index)
                    .collect();
                (
                    p_ev.version_uid.clone().unwrap(),
                    format!("population of gate {pg} ({})", p_ev.version_uid.as_ref().unwrap()),
                    p.event_classes.len().max(p.numerator),
                    Some(members),
                )
            }
            None => (
                "all-events@root".to_string(),
                "all events (ungated input)".to_string(),
                raw.event_count,
                None,
            ),
        };

        let params = LogicleParams {
            t: transform.t,
            w: transform.w,
            m: transform.m,
            a: transform.a,
        };
        let xi = raw
            .channel_names
            .iter()
            .position(|c| c == &gate.x_channel)
            .ok_or("gate x channel missing")?;
        let yi = raw
            .channel_names
            .iter()
            .position(|c| c == &gate.y_channel)
            .ok_or("gate y channel missing")?;
        let comp_xi = cev.channels.iter().position(|c| c == &gate.x_channel);
        let comp_yi = cev.channels.iter().position(|c| c == &gate.y_channel);
        let use_comp = gate.coordinate_basis.starts_with("logicle");

        let poly: Vec<(f64, f64)> = gate.vertices.iter().map(|p| (p[0], p[1])).collect();
        let mut classes = Vec::new();
        let mut numerator = 0usize;
        let mut boundary_count = 0usize;
        for ei in 0..raw.event_count {
            if let Some(members) = &parent_members {
                if !members.contains(&ei) {
                    continue;
                }
            }
            let (x, y) = if use_comp {
                let xraw = comp_xi
                    .and_then(|cx| cev.values[ei][cx])
                    .flatten()
                    .ok_or_else(|| {
                        format!(
                            "event {ei} has no finite compensated value for {}",
                            gate.x_channel
                        )
                    })?;
                let yraw = comp_yi
                    .and_then(|cy| cev.values[ei][cy])
                    .flatten()
                    .ok_or_else(|| {
                        format!(
                            "event {ei} has no finite compensated value for {}",
                            gate.y_channel
                        )
                    })?;
                (params.transform(xraw), params.transform(yraw))
            } else {
                let xraw = raw.values[ei][xi].numeric.ok_or_else(|| {
                    format!("event {ei} missing finite raw {}", gate.x_channel)
                })?;
                let yraw = raw.values[ei][yi].numeric.ok_or_else(|| {
                    format!("event {ei} missing finite raw {}", gate.y_channel)
                })?;
                (xraw, yraw)
            };
            let kind = math::classify_polygon((x, y), &poly);
            if kind == BoundaryKind::Boundary {
                boundary_count += 1;
            }
            let included = math::included(kind);
            if included {
                numerator += 1;
            }
            classes.push(EventClass {
                event_index: ei,
                x,
                y,
                classification: match kind {
                    BoundaryKind::Inside => "inside".to_string(),
                    BoundaryKind::Boundary => "boundary".to_string(),
                    BoundaryKind::Outside => "outside".to_string(),
                },
                included,
            });
        }
        let percent = if denominator_count == 0 {
            0.0
        } else {
            100.0 * numerator as f64 / denominator_count as f64
        };
        let pop = Population {
            gate_id: gate_id.clone(),
            gate_version: gate_ev.version_uid.clone().unwrap(),
            edges_version: edges_ev.version_uid.clone().unwrap(),
            transform_version: tr_ev.version_uid.clone().unwrap(),
            compensated_version: cev_ev.version_uid.clone().unwrap(),
            raw_version: raw_ev.version_uid.clone().unwrap(),
            denominator_version: denom_version.clone(),
            denominator_label,
            parent_gate: edge.parent_gate.clone(),
            event_classes: classes,
            numerator,
            denominator: denominator_count,
            percent,
            boundary_count,
        };
        let node = pop_node(gate_id);
        let deps = vec![
            ("gate_polygon".to_string(), gate_id.to_string(), pop.gate_version.clone()),
            ("gate_edges".to_string(), edges_ev.node_id.clone().unwrap_or_else(||"edges#main".to_string()), pop.edges_version.clone()),
            ("transform_params".to_string(), tr_ev.node_id.clone().unwrap_or_else(||"transform#main".to_string()), pop.transform_version.clone()),
            ("compensated_events".to_string(), "cev#main".to_string(), pop.compensated_version.clone()),
            ("raw_events".to_string(), raw_ev.node_id.clone().unwrap_or_else(||"raw#main".to_string()), pop.raw_version.clone()),
            (
                "population".to_string(),
                edge.parent_gate.clone().unwrap_or_default(),
                denom_version,
            ),
        ];
        self.insert_derived(
            c,
            branch,
            "population_versioned",
            "population",
            &node,
            &serde_json::to_value(&pop).unwrap(),
            &deps,
            req_event,
        )
    }
}

fn pop_node(gate: &str) -> String {
    format!("pop#{gate}")
}

impl Engine {
    /// Insert one authored/imported event inside a caller transaction. Used by
    /// the offline case seeder; normal clients go through `apply`.
    pub fn insert_authored_tx(
        &self,
        conn: &Connection,
        branch: &str,
        kind: &str,
        node_id: &str,
        payload: &Value,
    ) -> Result<i64, String> {
        let seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE branch_id=?1",
                params![branch],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let nk = node_kind_for(kind).unwrap();
        let id = self.db.append_event_tx(
            conn,
            branch,
            seq,
            kind,
            Some(nk),
            Some(node_id),
            None,
            &serde_json::to_string(payload).unwrap(),
            None,
            None,
        )?;
        let vuid = format!("{node_id}@{id}");
        conn.execute(
            "UPDATE events SET version_uid=?1 WHERE event_id=?2",
            params![vuid, id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE branches SET head_seq=?1 WHERE branch_id=?2",
            params![seq, branch],
        )
        .map_err(|e| e.to_string())?;
        Ok(id)
    }

    pub fn put_raw_material(
        &self,
        conn: &Connection,
        sha1: &str,
        name: &str,
        bytes: &[u8],
        event_id: i64,
    ) -> Result<(), String> {
        conn.execute(
            "INSERT OR IGNORE INTO raw_material(sha1,file_name,bytes,imported_event_id)
             VALUES(?1,?2,?3,?4)",
            params![sha1, name, bytes, event_id],
        )
        .map_err(|e| e.to_string())
    }

    pub fn raw_material(&self, sha1: &str) -> Option<Vec<u8>> {
        self.db
            .lock_conn()
            .query_row(
                "SELECT bytes FROM raw_material WHERE sha1=?1",
                params![sha1],
                |r| r.get(0),
            )
            .ok()
    }
}

impl Engine {
    pub fn insert_request_tx(
        &self,
        conn: &Connection,
        branch: &str,
        kind: &str,
    ) -> Result<i64, String> {
        let seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE branch_id=?1",
                params![branch],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let id = self.db.append_event_tx(
            conn,
            branch,
            seq,
            kind,
            None,
            None,
            None,
            "{}",
            None,
            None,
        )?;
        conn.execute(
            "UPDATE branches SET head_seq=?1 WHERE branch_id=?2",
            params![seq, branch],
        )
        .map_err(|e| e.to_string())?;
        Ok(id)
    }
}
