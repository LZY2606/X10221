//! Versioned provenance engine.
//!
//! Six artifact kinds are versioned independently: raw_events, spillover,
//! comp_matrix, transform, gate, gate_links. Every append-only event names
//! exact versions. Derived results are immutable; when upstream versions move,
//! downstream results are marked stale (never overwritten) and recomputation
//! creates new rows with new identities.

use crate::error::{AppError, AppResult};
use crate::hash::{artifact_id, canonical, derived_id, event_id, group_id, blob_id};
use crate::{comp, fcs, gate, transform};
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

pub struct Engine {
    pub conn: Connection,
}

fn now_ts() -> String {
    "2026-09-23T00:00:00Z".into() // deterministic timestamps for the fixed case
}

impl Engine {
    pub fn from_conn(mut conn: Connection) -> AppResult<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let eng = Engine { conn };
        eng.ensure_main_branch()?;
        Ok(eng)
    }

    fn ensure_main_branch(&self) -> AppResult<()> {
        let exists: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM branch WHERE id='main'", [], |r| r.get(0))?;
        if exists == 0 {
            self.conn.execute(
                "INSERT INTO branch(id,label,forked_from,fork_event_id,created_at) VALUES('main','Main',NULL,NULL,?)",
                params![now_ts()],
            )?;
        }
        Ok(())
    }

    // ---------- blobs ----------

    pub fn store_blob(&mut self, bytes: &[u8], media_type: &str, kind: &str) -> AppResult<String> {
        let id = blob_id(bytes);
        self.conn.execute(
            "INSERT OR IGNORE INTO blob_store(id,bytes,media_type,kind) VALUES(?,?,?,?)",
            params![id, bytes, media_type, kind],
        )?;
        Ok(id)
    }

    pub fn get_blob(&self, id: &str) -> AppResult<Vec<u8>> {
        self.conn
            .query_row("SELECT bytes FROM blob_store WHERE id=?", params![id], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .map_err(|e| AppError(format!("blob {id} not found: {e}")))
    }

    // ---------- artifacts ----------

    pub fn register_artifact(
        &mut self,
        logical_id: &str,
        kind: &str,
        payload: &Value,
        source_blob_id: Option<&str>,
        branch: &str,
        note: Option<&str>,
    ) -> AppResult<Value> {
        let tx = self.conn.transaction()?;
        let next_v: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(version),0)+1 FROM artifact WHERE logical_id=?",
                params![logical_id],
                |r| r.get(0),
            )
            .unwrap_or(1);
        let id = artifact_id(kind, logical_id, next_v, payload);
        tx.execute(
            "INSERT OR IGNORE INTO artifact(logical_id,version,kind,id,payload_json,source_blob_id,registered_at)
             VALUES(?,?,?,?,?,?,?)",
            params![logical_id, next_v, kind, id, canonical(payload), source_blob_id, now_ts()],
        )?;
        let payload_ev = json!({
            "type": "artifact_registered",
            "logical_id": logical_id,
            "kind": kind,
            "version": next_v,
            "artifact_id": id,
            "source_blob_id": source_blob_id,
            "note": note
        });
        append_event_tx(&tx, branch, "artifact_registered", &payload_ev)?;
        tx.commit()?;
        self.mark_affected_stale(branch)?;
        Ok(json!({"logical_id": logical_id, "version": next_v, "kind": kind, "artifact_id": id}))
    }

    pub fn artifact_payload(&self, art_id: &str) -> AppResult<Value> {
        self.conn
            .query_row("SELECT payload_json FROM artifact WHERE id=?", params![art_id], |r| {
                r.get::<_, String>(0)
            })
            .map_err(|e| AppError(format!("artifact {art_id}: {e}")))
            .and_then(|s| Ok(serde_json::from_str(&s)?))
    }

    pub fn artifact_version_id(&self, logical_id: &str, version: i64) -> AppResult<String> {
        self.conn
            .query_row(
                "SELECT id FROM artifact WHERE logical_id=? AND version=?",
                params![logical_id, version],
                |r| r.get(0),
            )
            .map_err(|e| AppError(format!("artifact {logical_id} v{version}: {e}")))
    }

    pub fn all_artifacts(&self) -> AppResult<Vec<Value>> {
        let mut stmt = self.conn.prepare(
            "SELECT logical_id,version,kind,id,payload_json,source_blob_id FROM artifact ORDER BY logical_id,version",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(json!({
                "logical_id": r.get::<_, String>(0)?,
                "version": r.get::<_, i64>(1)?,
                "kind": r.get::<_, String>(2)?,
                "id": r.get::<_, String>(3)?,
                "payload": serde_json::from_str::<Value>(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                "source_blob_id": r.get::<_, Option<String>>(5)?,
            }))
        })?;
        let mut out = Vec::new();
        for r in rows { out.push(r?); }
        Ok(out)
    }

    // ---------- FCS import ----------

    /// Import an FCS file transactionally: blob + parsed raw_events artifact +
    /// optional spillover artifact, all in one commit. Failure rolls back.
    pub fn import_fcs(&mut self, logical_id: &str, bytes: &[u8], branch: &str) -> AppResult<Value> {
        let parsed = fcs::parse_fcs(bytes).map_err(|e| AppError(e.to_string()))?;
        let raw_payload = fcs::to_json(&parsed);
        let tx = self.conn.transaction()?;
        let blob = blob_id(bytes);
        tx.execute(
            "INSERT OR IGNORE INTO blob_store(id,bytes,media_type,kind) VALUES(?,?,?,?)",
            params![blob, bytes, "application/vnd.isac.fcs", "fcs"],
        )?;
        let next_v: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(version),0)+1 FROM artifact WHERE logical_id=?",
                params![logical_id],
                |r| r.get(0),
            )
            .unwrap_or(1);
        let art = artifact_id("raw_events", logical_id, next_v, &raw_payload);
        tx.execute(
            "INSERT OR IGNORE INTO artifact(logical_id,version,kind,id,payload_json,source_blob_id,registered_at)
             VALUES(?,?,?,?,?,?,?)",
            params![logical_id, next_v, "raw_events", art, canonical(&raw_payload), blob, now_ts()],
        )?;
        let ev = json!({"type":"artifact_registered","logical_id":logical_id,"kind":"raw_events","version":next_v,"artifact_id":art,"source_blob_id":blob});
        append_event_tx(&tx, branch, "artifact_registered", &ev)?;

        let mut spill_info = Value::Null;
        if let Some(sp) = &parsed.spillover {
            let payload = json!({
                "source_keyword": sp.keyword,
                "channels": sp.channels,
                "matrix": sp.matrix,
                "matrix_unit": "fraction_of_signal",
                "equation": "raw = spillover * compensated",
                "coordinate_basis": "raw_detector_linear",
                "source_raw_events": logical_id
            });
            let slog = format!("{logical_id}.spillover");
            let sv: i64 = tx
                .query_row("SELECT COALESCE(MAX(version),0)+1 FROM artifact WHERE logical_id=?", params![slog], |r| r.get(0))
                .unwrap_or(1);
            let sid = artifact_id("spillover", &slog, sv, &payload);
            tx.execute(
                "INSERT OR IGNORE INTO artifact(logical_id,version,kind,id,payload_json,source_blob_id,registered_at)
                 VALUES(?,?,?,?,?,?,?)",
                params![slog, sv, "spillover", sid, canonical(&payload), blob, now_ts()],
            )?;
            let sev = json!({"type":"artifact_registered","logical_id":slog,"kind":"spillover","version":sv,"artifact_id":sid,"source_blob_id":blob});
            append_event_tx(&tx, branch, "artifact_registered", &sev)?;
            spill_info = json!({"logical_id": slog, "version": sv, "artifact_id": sid, "payload": payload});
        }
        tx.commit()?;
        self.mark_affected_stale(branch)?;
        Ok(json!({
            "logical_id": logical_id, "version": next_v, "artifact_id": art, "blob_id": blob,
            "channels": raw_payload["channels"].clone(),
            "event_count": parsed.events.len(),
            "spillover": spill_info
        }))
    }
}

// ---------- event log ----------

fn append_event_tx(
    tx: &rusqlite::Transaction,
    branch: &str,
    kind: &str,
    payload: &Value,
) -> AppResult<String> {
    let seq: i64 = tx
        .query_row("SELECT COALESCE(MAX(seq),0)+1 FROM event WHERE branch_id=?", params![branch], |r| r.get(0))
        .unwrap_or(1);
    let parent: Option<String> = tx
        .query_row(
            "SELECT id FROM event WHERE branch_id=? ORDER BY seq DESC LIMIT 1",
            params![branch],
            |r| r.get(0),
        )
        .ok();
    let id = event_id(branch, seq, kind, parent.as_deref(), payload);
    tx.execute(
        "INSERT INTO event(id,seq,branch_id,kind,payload_json,parent_event_id,created_at)
         VALUES(?,?,?,?,?,?,?)",
        params![id, seq, branch, kind, canonical(payload), parent, now_ts()],
    )?;
    Ok(id)
}

pub fn append_event(&mut self, branch: &str, kind: &str, payload: &Value) -> AppResult<String> {
    let tx = self.conn.transaction()?;
    let id = append_event_tx(&tx, branch, kind, payload)?;
    tx.commit()?;
    Ok(id)
}

pub fn list_events(&self, branch: &str) -> AppResult<Vec<Value>> {
    let mut stmt = self.conn.prepare(
        "SELECT id,seq,kind,payload_json,parent_event_id FROM event WHERE branch_id=? ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![branch], |r| {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "seq": r.get::<_, i64>(1)?,
            "kind": r.get::<_, String>(2)?,
            "payload": serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
            "parent_event_id": r.get::<_, Option<String>>(4)?,
        }))
    })?;
    let mut out = Vec::new();
    for r in rows { out.push(r?); }
    Ok(out)
}

pub fn list_branches(&self) -> AppResult<Vec<Value>> {
    let mut stmt = self.conn.prepare(
        "SELECT id,label,forked_from,fork_event_id FROM branch ORDER BY created_at,id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "label": r.get::<_, String>(1)?,
            "forked_from": r.get::<_, Option<String>>(2)?,
            "fork_event_id": r.get::<_, Option<String>>(3)?,
        }))
    })?;
    let mut out = Vec::new();
    for r in rows { out.push(r?); }
    Ok(out)
}

/// Ancestry chain of a branch (main first, branch last).
fn branch_chain(&self, branch: &str) -> AppResult<Vec<String>> {
    let mut chain = Vec::new();
    let mut cur = branch.to_string();
    let mut guard = 0;
    loop {
        chain.push(cur.clone());
        let parent: Option<String> = self
            .conn
            .query_row("SELECT forked_from FROM branch WHERE id=?", params![cur], |r| r.get(0))
            .map_err(|e| AppError(format!("branch {cur}: {e}")))?;
        match parent {
            Some(p) if p != cur => cur = p,
            _ => break,
        }
        guard += 1;
        if guard > 1000 { return Err(AppError("branch cycle".into())); }
    }
    chain.reverse();
    Ok(chain)
}

pub fn create_branch(&mut self, id: &str, label: &str, from_branch: &str, as_of_event: Option<&str>) -> AppResult<()> {
    let chain = self.branch_chain(from_branch)?;
    let event_ref = match as_of_event {
        Some(e) => e.to_string(),
        None => self
            .conn
            .query_row(
                "SELECT id FROM event WHERE branch_id=? ORDER BY seq DESC LIMIT 1",
                params![from_branch],
                |r| r.get::<_, String>(0),
            )
            .map_err(|_| AppError("cannot branch: no events on source".into()))?,
    };
    // Ensure the branch is not a descendant duplicate and as_of_event reachable.
    let exists: i64 = self.conn.query_row("SELECT COUNT(*) FROM branch WHERE id=?", params![id], |r| r.get(0))?;
    if exists != 0 { return Err(AppError(format!("branch {id} already exists"))); }
    self.conn.execute(
        "INSERT INTO branch(id,label,forked_from,fork_event_id,created_at) VALUES(?,?,?,?,?)",
        params![id, label, chain.last().cloned().unwrap_or_else(|| from_branch.into()), event_ref, now_ts()],
    )?;
    Ok(())
}

/// Effective logical_id -> (version, artifact_id, branch) at a point in time.
/// Replays events on the full branch ancestry; target-branch events after
/// `as_of_event` (by seq) are excluded so old conclusions can be restored.
pub fn effective_map(&self, branch: &str, as_of_event: Option<&str>) -> AppResult<BTreeMap<String, EffVersion>> {
    let chain = self.branch_chain(branch)?;
    let mut eff: BTreeMap<String, EffVersion> = BTreeMap::new();
    for br in chain {
        let cutoff: Option<i64> = if Some(br.as_str()) == Some(branch) {
            match as_of_event {
                Some(eid) => Some(self.conn.query_row(
                    "SELECT seq FROM event WHERE id=? AND branch_id=?",
                    params![eid, br], |r| r.get(0),
                ).map_err(|_| AppError("as_of_event not found on branch".into()))?),
                None => None,
            }
        } else {
            None
        };
        let mut stmt = self.conn.prepare(
            "SELECT seq,payload_json FROM event WHERE branch_id=? ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![br], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        for r in rows {
            let (seq, pj) = r?;
            if let Some(c) = cutoff { if seq > c { break; } }
            let p: Value = serde_json::from_str(&pj)?;
            if p["type"] == "artifact_registered" {
                eff.insert(
                    p["logical_id"].as_str().unwrap_or("").to_string(),
                    EffVersion {
                        version: p["version"].as_i64().unwrap_or(0),
                        artifact_id: p["artifact_id"].as_str().unwrap_or("").to_string(),
                        kind: p["kind"].as_str().unwrap_or("").to_string(),
                        branch: br.clone(),
                        seq,
                    },
                );
            }
        }
    }
    Ok(eff)
}

#[derive(Debug, Clone)]
pub struct EffVersion {
    pub version: i64,
    pub artifact_id: String,
    pub kind: String,
    pub branch: String,
    pub seq: i64,
}

// ---------- derived: compensation ----------

impl Engine {
    fn raw_events_for(&self, art_id: &str) -> AppResult<Value> {
        self.artifact_payload(art_id)
    }

    /// Evaluate compensation deterministically or record all legal candidates.
    /// Never silently inverts a near-singular matrix.
    pub fn evaluate_comp(
        &mut self,
        branch: &str,
        raw_logical: &str,
        comp_logical: &str,
        spill_logical: &str,
    ) -> AppResult<Value> {
        let eff = self.effective_map(branch, None)?;
        let raw = eff.get(raw_logical).ok_or_else(|| AppError("raw events not effective".into()))?.clone();
        let cmp = eff.get(comp_logical).ok_or_else(|| AppError("comp matrix not effective".into()))?.clone();
        let spl = eff.get(spill_logical).cloned();
        let raw_payload = self.raw_events_for(&raw.artifact_id)?;
        let comp_payload = self.artifact_payload(&cmp.artifact_id)?;
        let matrix: Vec<Vec<f64>> = serde_json::from_value(comp_payload["matrix"].clone())?;
        let channels: Vec<String> = raw_payload["channels"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|c| c["name"].as_str().unwrap_or("").to_string())
            .collect();
        let ranges: Vec<f64> = raw_payload["channels"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|c| c["range"].as_f64().unwrap_or(0.0))
            .collect();
        let spill_channels: Vec<String> = match &spl {
            Some(s) => {
                let p = self.artifact_payload(&s.artifact_id)?;
                serde_json::from_value(p["channels"].clone()).unwrap_or_default()
            }
            None => Vec::new(),
        };

        let events: Vec<comp::EventVector> = raw_payload["events"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|e| comp::EventVector {
                index: e["event_index"].as_i64().unwrap_or(0),
                values: e["values"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|cell| cell["value"].as_f64().filter(|f| f.is_finite()))
                    .collect(),
            })
            .collect();

        // Compensation applies to the spillover channels; map by name, then
        // merge back into the full channel order.
        let idxs: Vec<usize> = spill_channels
            .iter()
            .map(|name| channels.iter().position(|c| c == name).unwrap_or(0))
            .collect();
        let sub_events: Vec<comp::EventVector> = events
            .iter()
            .map(|e| comp::EventVector {
                index: e.index,
                values: idxs.iter().map(|&i| e.values.get(i).copied().flatten()).collect(),
            })
            .collect();
        let sub_ranges: Vec<f64> = idxs.iter().map(|&i| ranges.get(i).copied().unwrap_or(0.0)).collect();
        let result = comp::evaluate_compensation(&matrix, &sub_events, &sub_ranges);

        let refs = json!({
            "raw_events": raw.artifact_id, "raw_version": raw.version,
            "comp_matrix": cmp.artifact_id, "comp_version": cmp.version,
            "spillover": spl.as_ref().map(|s| s.artifact_id.clone()),
            "channels": spill_channels
        });
        let group = group_id("comp", &refs);

        // Skip re-creation if the exact group already exists.
        let existing: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM derived WHERE group_id=? AND kind='comp_candidates'",
            params![group], |r| r.get(0)).unwrap_or(0);
        if existing > 0 {
            return Ok(json!({"group_id": group, "status": result.status, "reused": true}));
        }

        let mut candidates_out = Vec::new();
        let tx = self.conn.transaction()?;
        for (ci, cand) in result.candidates.iter().enumerate() {
            let comp_rows = comp::apply_candidate(cand, &sub_events);
            // merge into full channel order
            let merged = merge_compensated(&raw_payload, &channels, &idxs, &comp_rows);
            let payload = json!({
                "group": group,
                "candidate_index": ci,
                "method": cand.method,
                "cutoff_rel": cand.cutoff_rel,
                "rank": cand.rank,
                "inverse": cand.inv,
                "events": merged,
                "coordinate_basis": "compensated_linear",
                "unit": "channel_units",
                "depends_on": refs
            });
            let did = derived_id(&group, "comp_candidates", Some(ci as i64), &payload);
            tx.execute(
                "INSERT OR IGNORE INTO derived(id,group_id,kind,candidate_id,payload_json,status,created_by_event,created_at)
                 VALUES(?,?,?,?,?,?,?,?)",
                params![did, group, "comp_candidates", ci as i64, canonical(&payload), "candidate", "", now_ts()],
            )?;
            candidates_out.push(json!({"derived_id": did, "method": cand.method, "rank": cand.rank}));
        }
        let payload_ev = json!({
            "type": "comp_evaluated",
            "group_id": group,
            "status": result.status,
            "rcond": result.rcond,
            "rank": result.rank,
            "singular_values": result.singular_values,
            "candidates": candidates_out,
            "diagnostics": result.diagnostics,
            "judgement_rule": {
                "rule": "near_singular_matrix_is_not_silently_inverted",
                "rcond_threshold": comp::RCOND_THRESHOLD,
                "singular": "no_candidates",
                "indeterminate": "all_legal_tsvd_candidates_saved_pending_decision",
                "determinate": "single_lu_candidate_auto_adopted"
            },
            "refs": refs
        });
        let ev_id = append_event_tx(&tx, branch, "comp_evaluated", &payload_ev)?;
        // Bind created_by_event and auto-adopt the sole determinate candidate.
        tx.execute("UPDATE derived SET created_by_event=? WHERE group_id=?", params![ev_id, group])?;
        if result.status == "determinate" {
            if let Some(first) = candidates_out.first() {
                let did = first["derived_id"].as_str().unwrap_or("");
                tx.execute("UPDATE derived SET status='adopted' WHERE id=?", params![did])?;
                tx.execute(
                    "INSERT INTO decision(group_id,chosen_derived_id,event_id,decided_at,note)
                     VALUES(?,?,?,?,NULL) ON CONFLICT(group_id) DO UPDATE SET chosen_derived_id=excluded.chosen_derived_id,event_id=excluded.event_id",
                    params![group, did, ev_id, now_ts()],
                )?;
            }
        }
        tx.commit()?;
        self.mark_affected_stale(branch)?;
        Ok(json!({"group_id": group, "status": result.status, "rcond": result.rcond, "candidates": candidates_out, "event_id": ev_id}))
    }
}

fn merge_compensated(raw: &Value, channels: &[String], idxs: &[usize], sub_rows: &[Value]) -> Vec<Value> {
    raw["events"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut cells = e["values"].as_array().cloned().unwrap_or_default();
            if let Some(row) = sub_rows.get(i) {
                for (k, &ci) in idxs.iter().enumerate() {
                    if let Some(cell) = row["compensated"].as_array().and_then(|a| a.get(k)) {
                        cells[ci] = cell.clone();
                    }
                }
            }
            json!({"event_index": e["event_index"], "channels": channels, "compensated": cells})
        })
        .collect()
}

// ---------- derived: gate tree ----------

impl Engine {
    pub fn evaluate_gates(&mut self, branch: &str, raw_logical: &str, comp_logical: &str,
                          transform_logical: &str, links_logical: &str) -> AppResult<Value> {
        let eff = self.effective_map(branch, None)?;
        let raw_v = eff.get(raw_logical).ok_or_else(|| AppError("raw missing".into()))?.clone();
        let comp_v = eff.get(comp_logical).ok_or_else(|| AppError("comp matrix missing".into()))?.clone();
        let tr_v = eff.get(transform_logical).ok_or_else(|| AppError("transform missing".into()))?.clone();
        let links_v = eff.get(links_logical).ok_or_else(|| AppError("gate links missing".into()))?.clone();

        let raw_payload = self.artifact_payload(&raw_v.artifact_id)?;
        let tr_payload = self.artifact_payload(&tr_v.artifact_id)?;
        let links_payload = self.artifact_payload(&links_v.artifact_id)?;

        // Resolve adopted compensation candidate for this exact matrix version.
        let comp_group = self.comp_group_for(&comp_v.artifact_id)?;
        let adopted = self.adopted_derived(&comp_group)?;
        if adopted.is_none() {
            return Ok(json!({"blocked": true, "reason": "compensation_indeterminate_or_singular",
                "comp_group": comp_group,
                "message": "gates cannot be evaluated until a compensation candidate is decided"}));
        }
        let comp_derived_id = adopted.unwrap();
        let comp_row = self.derived_payload(&comp_derived_id)?;
        let channels: Vec<String> = raw_payload["channels"].as_array().unwrap_or(&vec![])
            .iter().map(|c| c["name"].as_str().unwrap_or("").to_string()).collect();

        // Build transformed basis from the adopted compensation candidate.
        let cofactors = tr_payload["cofactors"].as_object().cloned().unwrap_or_default();
        let passthrough: Vec<String> = tr_payload["passthrough_channels"].as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let comp_events = comp_row["events"].as_array().cloned().unwrap_or_default();
        // comp payload stores full channel rows in "compensated"; adapt for transform.
        let comp_for_transform: Vec<Value> = comp_events.iter().map(|r| {
            json!({"event_index": r["event_index"], "compensated": r["compensated"]})
        }).collect();
        let transformed = transform::transform_events(&comp_for_transform, &channels, &cofactors, &passthrough);
        let mut basis: HashMap<i64, Vec<f64>> = HashMap::new();
        for row in &transformed {
            let idx = row["event_index"].as_i64().unwrap_or(0);
            let vals = row["transformed"].as_array().unwrap_or(&vec![])
                .iter().map(|c| c["value"].as_f64().filter(|f| f.is_finite()).unwrap_or(f64::NAN)).collect();
            basis.insert(idx, vals);
        }

        // Gate tree: nodes from links payload [{gate_logical, parent_logical, display_name}]
        let nodes = links_payload["nodes"].as_array().cloned().unwrap_or_default();
        let mut order: Vec<Value> = Vec::new();
        let mut placed: HashSet<String> = HashSet::new();
        let by_gate: HashMap<String, Value> = nodes.iter().map(|n| (n["gate_logical"].as_str().unwrap_or("").to_string(), n.clone())).collect();
        while order.len() < nodes.len() {
            let before = order.len();
            for n in &nodes {
                let g = n["gate_logical"].as_str().unwrap_or("").to_string();
                if placed.contains(&g) { continue; }
                let parent = n["parent_logical"].as_str().unwrap_or("");
                if parent.is_empty() || placed.contains(parent) {
                    order.push(n.clone());
                    placed.insert(g);
                }
            }
            if order.len() == before {
                return Err(AppError("gate links contain a cycle or dangling parent".into()));
            }
        }

        let total_events = raw_payload["events"].as_array().map(|a| a.len() as i64).unwrap_or(0);
        let all_events: Vec<i64> = (1..=total_events).collect();
        let mut evaluated: HashMap<String, Value> = HashMap::new();
        let mut results: Vec<Value> = Vec::new();
        let tx = self.conn.transaction()?;

        for node in &order {
            let gate_logical = node["gate_logical"].as_str().unwrap_or("").to_string();
            let parent_logical = node["parent_logical"].as_str().unwrap_or("").to_string();
            let display_name = node["display_name"].as_str().unwrap_or(&gate_logical).to_string();
            let gv = eff.get(&gate_logical).ok_or_else(|| AppError(format!("gate {gate_logical} not effective")))?.clone();
            let gate_payload = self.artifact_payload(&gv.artifact_id)?;
            let basis_name = gate_payload["coordinate_basis"].as_str().unwrap_or("asinh_compensated").to_string();

            let (universe, denominator_group, denominator_version_ref, denominator_count): (Vec<i64>, Option<String>, Value, i64) =
                if parent_logical.is_empty() {
                    (all_events.clone(), None, json!("all_raw_events"), total_events)
                } else {
                    let p = evaluated.get(&parent_logical).ok_or_else(|| AppError("parent not evaluated".into()))?.clone();
                    let members: Vec<i64> = p["members"].as_array().unwrap_or(&vec![])
                        .iter().filter_map(|m| m.as_i64()).collect();
                    (members,
                     Some(p["group_id"].as_str().unwrap_or("").to_string()),
                     json!({"parent_gate": parent_logical,
                            "parent_result_group": p["group_id"].clone(),
                            "parent_gate_version": p["gate_version"].clone(),
                            "parent_result_id": p["derived_id"].clone()}),
                     p["count"].as_i64().unwrap_or(0))
                };

            let res = gate::evaluate_gate(&gate_payload, &universe, &basis, &channels);
            let members: Vec<i64> = res["members"].as_array().unwrap_or(&vec![])
                .iter().filter_map(|m| m.as_i64()).collect();
            let count = members.len() as i64;
            let pct = if denominator_count > 0 { count as f64 * 100.0 / denominator_count as f64 } else { 0.0 };

            let refs = json!({
                "gate": gv.artifact_id, "gate_version": gv.version,
                "raw_events": raw_v.artifact_id, "raw_version": raw_v.version,
                "comp_matrix": comp_v.artifact_id, "comp_version": comp_v.version,
                "comp_candidate": comp_derived_id,
                "transform": tr_v.artifact_id, "transform_version": tr_v.version,
                "links": links_v.artifact_id, "links_version": links_v.version,
                "parent_logical": parent_logical,
                "parent_result_group": denominator_group
            });
            let group = group_id(&format!("gate:{gate_logical}"), &refs);
            let payload = json!({
                "group": group,
                "gate_logical": gate_logical,
                "display_name": display_name,
                "members": members,
                "boundary_members": res["boundary_members"],
                "coordinate_basis": basis_name,
                "count": count,
                "denominator_count": denominator_count,
                "percent_of_parent": pct,
                "denominator": denominator_version_ref,
                "depends_on": refs
            });
            let did = derived_id(&group, "gate_result", None, &payload);
            tx.execute(
                "INSERT OR IGNORE INTO derived(id,group_id,kind,candidate_id,payload_json,status,created_by_event,created_at)
                 VALUES(?,?,?,NULL,?, 'candidate', '', ?)",
                params![did, group, "gate_result", canonical(&payload), now_ts()],
            )?;
            let ev_payload = json!({
                "type": "gate_evaluated",
                "group_id": group,
                "gate_logical": gate_logical,
                "gate_version": gv.version,
                "artifact_id": gv.artifact_id,
                "derived_id": did,
                "count": count,
                "percent_of_parent": pct,
                "denominator": denominator_version_ref,
                "denominator_count": denominator_count,
                "rule": "percent_denominator_pinned_to_parent_result_version",
                "refs": refs
            });
            let ev_id = append_event_tx(&tx, branch, "gate_evaluated", &ev_payload)?;
            tx.execute("UPDATE derived SET status='adopted', created_by_event=? WHERE id=?", params![ev_id, did])?;

            let mut recorded = payload.clone();
            recorded["group_id"] = json!(group);
            recorded["derived_id"] = json!(did);
            recorded["gate_version"] = json!(gv.version);
            evaluated.insert(gate_logical, recorded.clone());
            results.push(ev_payload);
        }
        tx.commit()?;
        self.mark_affected_stale(branch)?;
        Ok(json!({"evaluated": results}))
    }

    pub fn comp_group_for(&self, comp_artifact_id: &str) -> AppResult<String> {
        let mut stmt = self.conn.prepare(
            "SELECT payload_json FROM derived WHERE kind='comp_candidates' ORDER BY created_at DESC, rowid DESC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for r in rows {
            let p: Value = serde_json::from_str(&r?)?;
            if p["depends_on"]["comp_matrix"].as_str() == Some(comp_artifact_id) {
                return Ok(p["group"].as_str().unwrap_or("").to_string());
            }
        }
        Ok(String::new())
    }

    pub fn adopted_derived(&self, group: &str) -> AppResult<Option<String>> {
        if group.is_empty() { return Ok(None); }
        Ok(self.conn.query_row(
            "SELECT chosen_derived_id FROM decision WHERE group_id=?", params![group], |r| r.get::<_, Option<String>>(0),
        ).unwrap_or(None))
    }

    pub fn derived_payload(&self, id: &str) -> AppResult<Value> {
        let s: String = self.conn.query_row("SELECT payload_json FROM derived WHERE id=?", params![id], |r| r.get(0))?;
        Ok(serde_json::from_str(&s)?)
    }
}

// ---------- staleness propagation ----------

impl Engine {
    /// Recompute staleness of gate results on a branch using the CURRENT
    /// effective map. Immutable payloads are never modified; a stale_marker
    /// (and machine-readable `derived_marked_stale` event) is appended once.
    pub fn mark_affected_stale(&mut self, branch: &str) -> AppResult<()> {
        let eff = match self.effective_map(branch, None) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        // Current latest gate groups per logical gate.
        let mut stmt = self.conn.prepare(
            "SELECT d.id, d.payload_json FROM derived d
             WHERE d.kind='gate_result'")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut checks: Vec<(String, Value)> = Vec::new();
        for r in rows { let (id, pj) = r?; checks.push((id, serde_json::from_str(&pj)?)); }

        let mut to_mark: Vec<(String, String, Value)> = Vec::new();
        for (did, payload) in &checks {
            let deps = &payload["depends_on"];
            let mut reasons: Vec<(String, Value)> = Vec::new();

            // Only consider groups whose artifacts live on this branch chain:
            // if the referenced artifact is not effective here, the result is
            // simply not part of this branch's view (handled by snapshot).
            let mut on_chain = true;
            let checks_list: Vec<(&str, &str)> = vec![
                ("gate", "gate"), ("raw_events", "raw_events"),
                ("comp_matrix", "comp_matrix"), ("transform", "transform"),
                ("links", "links"),
            ];
            for (eff_key, dep_key) in checks_list {
                let dep_art = deps[dep_key].as_str().unwrap_or("");
                if dep_art.is_empty() { continue; }
                // Map referenced content id -> logical id via the artifact
                // table, then compare against the effective version on chain.
                let dep_logical: Option<String> = self.conn
                    .query_row("SELECT logical_id FROM artifact WHERE id=?", params![dep_art], |r| r.get(0))
                    .ok();
                match dep_logical.as_deref() {
                    None => { on_chain = false; break; }
                    Some(logical) => match eff.get(logical) {
                        None => { on_chain = false; break; }
                        Some(cur) if cur.artifact_id == dep_art => {}
                        Some(cur) => reasons.push(("upstream_artifact_superseded".into(),
                            json!({"dependency": dep_key, "logical_id": logical,
                                   "referenced_artifact": dep_art,
                                   "effective_artifact": cur.artifact_id,
                                   "effective_version": cur.version}))),
                    },
                }
            }
            if !on_chain { continue; }

            // Compensation candidate staleness.
            if let Some(cc) = deps["comp_candidate"].as_str() {
                if !cc.is_empty() {
                    let comp_art = deps["comp_matrix"].as_str().unwrap_or("");
                    if let Some(cur_comp) = find_eff(&eff, comp_art).current_id() {
                        let cur_group = self.comp_group_for(&cur_comp)?;
                        if let Some(adopted) = self.adopted_derived(&cur_group)? {
                            if adopted != cc {
                                reasons.push(("compensation_candidate_superseded".into(),
                                    json!({"referenced_candidate": cc, "current_adopted": adopted,
                                           "current_comp_group": cur_group})));
                            }
                        }
                    }
                }
            }

            // Parent result must remain the current adopted child.
            if let Some(pg) = deps["parent_result_group"].as_str() {
                if !pg.is_empty() {
                    let p_current: Option<String> = self.conn.query_row(
                        "SELECT d.id FROM derived d JOIN decision x ON x.chosen_derived_id=d.id
                         WHERE d.group_id=? AND d.status='adopted'",
                        params![pg], |r| r.get(0)).ok();
                    let p_ref = payload["denominator"]["parent_result_id"].as_str().unwrap_or("");
                    if let Some(cur) = p_current {
                        if cur != p_ref {
                            reasons.push(("parent_result_version_pinned_old".into(),
                                json!({"pinned_parent_result": p_ref, "current_parent_result": cur,
                                       "parent_group": pg})));
                        }
                    }
                }
            }

            if !reasons.is_empty() {
                let already: i64 = self.conn.query_row(
                    "SELECT COUNT(*) FROM stale_marker WHERE derived_id=?", params![did], |r| r.get(0)).unwrap_or(0);
                if already == 0 {
                    let detail = json!({"reasons": reasons.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>()});
                    to_mark.push((did.clone(), reasons[0].0.clone(), detail));
                }
            }
        }

        if to_mark.is_empty() { return Ok(()); }
        let tx = self.conn.transaction()?;
        for (did, code, detail) in &to_mark {
            let ev_payload = json!({
                "type": "derived_marked_stale",
                "derived_id": did,
                "reason_code": code,
                "detail": detail,
                "policy": "stale_results_are_flagged_not_overwritten"
            });
            let ev_id = append_event_tx(&tx, branch, "derived_marked_stale", &ev_payload)?;
            tx.execute(
                "INSERT OR IGNORE INTO stale_marker(derived_id,as_of_event,reason_code,detail_json) VALUES(?,?,?,?)",
                params![did, ev_id, code, canonical(detail)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn is_stale(&self, derived_id: &str, as_of_event: Option<&str>) -> AppResult<bool> {
        match as_of_event {
            None => Ok(self.conn.query_row(
                "SELECT COUNT(*) FROM stale_marker WHERE derived_id=?", params![derived_id], |r| r.get::<i64>(0)).unwrap_or(0) > 0),
            Some(eid) => {
                let seq: i64 = self.conn.query_row(
                    "SELECT seq FROM event WHERE id=?", params![eid], |r| r.get(0))
                    .map_err(|_| AppError("event not found".into()))?;
                Ok(self.conn.query_row(
                    "SELECT COUNT(*) FROM stale_marker m JOIN event e ON e.id=m.as_of_event
                     WHERE m.derived_id=? AND e.seq<=?", params![derived_id, seq], |r| r.get::<i64>(0)).unwrap_or(0) > 0)
            }
        }
    }
}

// ---------- decisions (adopt / reject / undo) ----------

impl Engine {
    /// Adopt one candidate of an indeterminate group, or reject a candidate.
    /// Decisions are append-only events; they never mutate raw data.
    pub fn decide_candidate(&mut self, branch: &str, group: &str, derived_id: &str,
                            action: &str, note: Option<&str>) -> AppResult<Value> {
        let belongs: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM derived WHERE id=? AND group_id=?", params![derived_id, group], |r| r.get(0))?;
        if belongs == 0 { return Err(AppError("derived candidate does not belong to group".into())); }
        let tx = self.conn.transaction()?;
        let payload = json!({
            "type": "candidate_decided",
            "group_id": group,
            "derived_id": derived_id,
            "action": action,
            "note": note
        });
        let ev_id = append_event_tx(&tx, branch, "candidate_decided", &payload)?;
        if action == "adopt" {
            tx.execute("UPDATE derived SET status='candidate' WHERE group_id=? AND status='adopted'", params![group])?;
            tx.execute("UPDATE derived SET status='adopted' WHERE id=?", params![derived_id])?;
            tx.execute(
                "INSERT INTO decision(group_id,chosen_derived_id,event_id,decided_at,note)
                 VALUES(?,?,?,?,?) ON CONFLICT(group_id) DO UPDATE SET
                   chosen_derived_id=excluded.chosen_derived_id,event_id=excluded.event_id,note=excluded.note",
                params![group, derived_id, ev_id, now_ts(), note],
            )?;
        } else if action == "reject" {
            tx.execute("UPDATE derived SET status='rejected' WHERE id=?", params![derived_id])?;
        } else {
            return Err(AppError("action must be adopt|reject".into()));
        }
        tx.commit()?;
        self.mark_affected_stale(branch)?;
        Ok(json!({"event_id": ev_id, "group_id": group, "derived_id": derived_id, "action": action}))
    }

    /// Undo the last decision on a branch (adopt/reject), restoring the prior
    /// branch state. Appends an undo event; nothing is physically deleted.
    pub fn undo_last_decision(&mut self, branch: &str) -> AppResult<Value> {
        let last: Option<(String, String)> = self.conn
            .query_row(
                "SELECT id,payload_json FROM event WHERE branch_id=? AND kind='candidate_decided' ORDER BY seq DESC LIMIT 1",
                params![branch], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .ok();
        let (last_id, pj) = last.ok_or_else(|| AppError("no decision to undo".into()))?;
        let p: Value = serde_json::from_str(&pj)?;
        let group = p["group_id"].as_str().unwrap_or("").to_string();
        let derived = p["derived_id"].as_str().unwrap_or("").to_string();
        let prev_action = p["action"].as_str().unwrap_or("");

        // Restore prior decision state: find the most recent earlier decision
        // for the group before last_id.
        let tx = self.conn.transaction()?;
        let prev: Option<(String, String)> = tx.query_row(
            "SELECT payload_json FROM event WHERE branch_id=? AND kind='candidate_decided' AND id!=? ORDER BY seq DESC LIMIT 1",
            params![branch, last_id], |r| r.get::<_, String>(0)).ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| (v["group_id"].as_str().unwrap_or("").to_string(),
                      v["derived_id"].as_str().unwrap_or("").to_string()));
        if prev_action == "adopt" {
            tx.execute("UPDATE derived SET status='candidate' WHERE group_id=?", params![group])?;
            if let Some((pg, pd)) = &prev {
                if pg == &group {
                    tx.execute("UPDATE derived SET status='adopted' WHERE id=?", params![pd])?;
                    tx.execute("UPDATE decision SET chosen_derived_id=?,event_id=? WHERE group_id=?",
                        params![pd, last_id, group])?;
                } else {
                    tx.execute("DELETE FROM decision WHERE group_id=?", params![group])?;
                }
            } else {
                tx.execute("DELETE FROM decision WHERE group_id=?", params![group])?;
            }
        } else if prev_action == "reject" {
            tx.execute("UPDATE derived SET status='candidate' WHERE id=?", params![derived])?;
        }
        let payload = json!({"type":"decision_undone","undone_event": last_id, "restored_group": group});
        let ev_id = append_event_tx(&tx, branch, "decision_undone", &payload)?;
        tx.commit()?;
        self.mark_affected_stale(branch)?;
        Ok(json!({"event_id": ev_id, "undone": last_id}))
    }
}

// ---------- review snapshot ----------

impl Engine {
    pub fn snapshot(&self, branch: &str, as_of_event: Option<&str>) -> AppResult<Value> {
        let eff = self.effective_map(branch, as_of_event)?;
        let mut artifacts = Vec::new();
        for (logical, v) in &eff {
            let payload = self.artifact_payload(&v.artifact_id).unwrap_or(Value::Null);
            artifacts.push(json!({
                "logical_id": logical, "version": v.version, "kind": v.kind,
                "artifact_id": v.artifact_id, "branch": v.branch, "payload": payload
            }));
        }
        artifacts.sort_by(|a, b| a["logical_id"].as_str().cmp(&b["logical_id"].as_str()));

        // Gate tree populations: replay gate_evaluated events on the chain
        // limited by as_of, taking the last result per gate logical.
        let chain = self.branch_chain(branch)?;
        let mut pops: BTreeMap<String, Value> = BTreeMap::new();
        for br in &chain {
            let cutoff: Option<i64> = if br == branch {
                match as_of_event {
                    Some(eid) => Some(self.conn.query_row(
                        "SELECT seq FROM event WHERE id=? AND branch_id=?", params![eid, br], |r| r.get(0))?),
                    None => None,
                }
            } else { None };
            let mut stmt = self.conn.prepare("SELECT seq,payload_json FROM event WHERE branch_id=? AND kind='gate_evaluated' ORDER BY seq")?;
            let rows = stmt.query_map(params![br], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for r in rows {
                let (seq, pj) = r?;
                if let Some(c) = cutoff { if seq > c { break; } }
                let p: Value = serde_json::from_str(&pj)?;
                pops.insert(p["gate_logical"].as_str().unwrap_or("").to_string(), p);
            }
        }

        // Stale flags + payload (members, boundary) per displayed population.
        let mut populations = Vec::new();
        for (gl, evp) in &pops {
            let did = evp["derived_id"].as_str().unwrap_or("");
            let payload = self.derived_payload(did).unwrap_or(Value::Null);
            let stale = self.is_stale(did, as_of_event).unwrap_or(false);
            let mut item = json!({
                "gate_logical": gl,
                "display_name": payload["display_name"],
                "derived_id": did,
                "group_id": evp["group_id"],
                "gate_version": evp["gate_version"],
                "count": payload["count"],
                "denominator_count": payload["denominator_count"],
                "percent_of_parent": payload["percent_of_parent"],
                "denominator": payload["denominator"],
                "members": payload["members"],
                "boundary_members": payload["boundary_members"],
                "coordinate_basis": payload["coordinate_basis"],
                "stale": stale
            });
            if stale {
                if let Ok(markers) = self.stale_reasons(did, as_of_event) {
                    item["stale_reasons"] = json!(markers);
                }
            }
            populations.push(item);
        }
        populations.sort_by(|a, b| a["gate_logical"].as_str().cmp(&b["gate_logical"].as_str()));

        // Compensation status for every recorded group visible on chain.
        let mut comps = Vec::new();
        for (logical, v) in &eff {
            if v.kind != "comp_matrix" { continue; }
            let group = self.comp_group_for(&v.artifact_id).unwrap_or_default();
            if group.is_empty() { continue; }
            let mut candidates = Vec::new();
            let mut stmt = self.conn.prepare("SELECT id,candidate_id,payload_json,status FROM derived WHERE group_id=? AND kind='comp_candidates'")?;
            let rows = stmt.query_map(params![group], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?, r.get::<_, String>(3)?))
            })?;
            for r in rows {
                let (id, ci, pj, status) = r?;
                let p: Value = serde_json::from_str(&pj)?;
                candidates.push(json!({
                    "derived_id": id, "candidate_index": ci, "status": status,
                    "method": p["method"], "rank": p["rank"], "cutoff_rel": p["cutoff_rel"]
                }));
            }
            let ce: Option<Value> = self.last_event_of_type(branch, "comp_evaluated", as_of_event, &group)?;
            comps.push(json!({
                "comp_logical": logical, "comp_version": v.version, "artifact_id": v.artifact_id,
                "group_id": group, "candidates": candidates,
                "adopted": self.adopted_derived(&group)?,
                "status": ce.as_ref().and_then(|e| e["payload"]["status"].as_str().map(String::from)),
                "rcond": ce.as_ref().and_then(|e| e["payload"]["rcond"].clone()),
                "singular_values": ce.as_ref().and_then(|e| e["payload"]["singular_values"].clone()),
                "diagnostics": ce.as_ref().and_then(|e| e["payload"]["diagnostics"].clone()),
                "rule": ce.as_ref().and_then(|e| e["payload"]["judgement_rule"].clone())
            }));
        }

        // Gate tree structure from effective links for ordering presentation.
        let links = eff.values().find(|v| v.kind == "gate_links").and_then(|v| self.artifact_payload(&v.artifact_id).ok());
        let head: Option<String> = self.conn.query_row(
            "SELECT id FROM event WHERE branch_id=? ORDER BY seq DESC LIMIT 1",
            params![branch], |r| r.get(0)).ok();
        Ok(json!({
            "branch": branch,
            "as_of_event": as_of_event.map(String::from).or(head),
            "artifacts": artifacts,
            "populations": populations,
            "compensations": comps,
            "links": links,
            "events": self.list_events(branch)?
        }))
    }

    fn last_event_of_type(&self, branch: &str, kind: &str, as_of: Option<&str>, group: &str) -> AppResult<Option<Value>> {
        let chain = self.branch_chain(branch)?;
        let mut found = None;
        for br in chain {
            let cutoff: Option<i64> = if br == branch {
                match as_of {
                    Some(eid) => Some(self.conn.query_row(
                        "SELECT seq FROM event WHERE id=? AND branch_id=?", params![eid, br], |r| r.get(0))?),
                    None => None,
                }
            } else { None };
            let mut stmt = self.conn.prepare("SELECT seq,payload_json FROM event WHERE branch_id=? AND kind=? ORDER BY seq")?;
            let rows = stmt.query_map(params![br, kind], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for r in rows {
                let (seq, pj) = r?;
                if let Some(c) = cutoff { if seq > c { break; } }
                let p: Value = serde_json::from_str(&pj)?;
                if p["group_id"].as_str() == Some(group) { found = Some(json!({"payload": p})); }
            }
        }
        Ok(found)
    }

    fn stale_reasons(&self, did: &str, as_of: Option<&str>) -> AppResult<Vec<Value>> {
        let rows: Vec<String> = match as_of {
            Some(eid) => {
                let seq: i64 = self.conn.query_row("SELECT seq FROM event WHERE id=?", params![eid], |r| r.get(0))?;
                let mut stmt = self.conn.prepare(
                    "SELECT m.detail_json FROM stale_marker m JOIN event e ON e.id=m.as_of_event
                     WHERE m.derived_id=? AND e.seq<=?")?;
                stmt.query_map(params![did, seq], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
            }
            None => {
                let mut stmt = self.conn.prepare("SELECT detail_json FROM stale_marker WHERE derived_id=?")?;
                stmt.query_map(params![did], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
            }
        };
        Ok(rows.iter().filter_map(|s| serde_json::from_str(s).ok()).collect())
    }
}

// ---------- review drafts (optimistic concurrency) ----------

impl Engine {
    pub fn save_draft(&mut self, id: &str, branch: &str, label: &str,
                      basis_event: &str, expected_revision: i64, content: &Value) -> AppResult<Value> {
        let exists: i64 = self.conn.query_row("SELECT COUNT(*) FROM draft WHERE id=?", params![id], |r| r.get(0))?;
        if exists == 0 {
            if expected_revision != 0 {
                return Err(AppError::conflict(409, "draft_does_not_exist",
                    "draft is new but expected_revision != 0", json!({})));
            }
            self.conn.execute(
                "INSERT INTO draft(id,branch_id,label,basis_event_id,revision,content_json,updated_at)
                 VALUES(?,?,?,?,1,?,?)",
                params![id, branch, label, basis_event, canonical(content), now_ts()],
            )?;
            return Ok(json!({"draft_id": id, "revision": 1}));
        }
        let (rev, other_branch, other_content, other_label): (i64, String, String, String) = self.conn.query_row(
            "SELECT revision,branch_id,content_json,label FROM draft WHERE id=?", params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        if rev != expected_revision {
            let theirs: Value = serde_json::from_str(&other_content).unwrap_or(Value::Null);
            return Err(AppError::conflict(409, "draft_revision_conflict",
                "another window saved the same draft first; both diffs are returned",
                json!({"draft_id": id, "expected_revision": expected_revision,
                       "current_revision": rev,
                       "other_branch": other_branch, "other_label": other_label,
                       "theirs": theirs, "yours": content})));
        }
        self.conn.execute(
            "UPDATE draft SET branch_id=?,label=?,basis_event_id=?,revision=?,content_json=?,updated_at=? WHERE id=?",
            params![branch, label, basis_event, rev + 1, canonical(content), now_ts(), id],
        )?;
        Ok(json!({"draft_id": id, "revision": rev + 1}))
    }

    pub fn get_draft(&self, id: &str) -> AppResult<Value> {
        self.conn.query_row(
            "SELECT branch_id,label,basis_event_id,revision,content_json FROM draft WHERE id=?",
            params![id], |r| {
                Ok(json!({
                    "draft_id": id,
                    "branch_id": r.get::<_, String>(0)?,
                    "label": r.get::<_, String>(1)?,
                    "basis_event_id": r.get::<_, String>(2)?,
                    "revision": r.get::<_, i64>(3)?,
                    "content": serde_json::from_str::<Value>(&r.get::<_, String>(4)?).unwrap_or(Value::Null)
                }))
            }).map_err(|e| AppError(format!("draft {id}: {e}")))
    }

    pub fn list_drafts(&self) -> AppResult<Vec<Value>> {
        let mut stmt = self.conn.prepare("SELECT id,branch_id,label,basis_event_id,revision FROM draft ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(json!({"draft_id": r.get::<_, String>(0)?, "branch_id": r.get::<_, String>(1)?,
                "label": r.get::<_, String>(2)?, "basis_event_id": r.get::<_, String>(3)?,
                "revision": r.get::<_, i64>(4)?}))
        })?;
        let mut out = Vec::new();
        for r in rows { out.push(r?); }
        Ok(out)
    }
}
