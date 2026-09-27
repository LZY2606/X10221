use crate::domain::*;
use crate::engine;
use rusqlite::{Connection, Transaction};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct MaterialInput {
    pub bytes: Vec<u8>,
    pub filename: String,
}

#[derive(Debug, Clone)]
pub struct ReplayState {
    pub events: Vec<EventRow>,
    pub materials: BTreeMap<String, CurrentMaterial>,
    pub results: Vec<DerivedResultRow>,
    pub decisions: Vec<DecisionRow>,
    pub invalidations: Vec<InvalidationRow>,
}

#[derive(Debug, Clone)]
pub struct EventRow {
    pub event_id: i64,
    pub global_seq: Option<i64>,
    pub branch_id: String,
    pub seq: i64,
    pub event_type: String,
    pub actor: String,
    pub created_at: String,
    pub summary: String,
    pub payload: Value,
    pub supersedes_event_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct CurrentMaterial {
    pub kind: String,
    pub material_id: String,
    pub version: i64,
    pub event_id: i64,
    pub branch_id: String,
    pub raw_bytes: Vec<u8>,
    pub canonical_json: String,
    pub sha256_hex: String,
    pub filename: String,
}

#[derive(Debug, Clone)]
pub struct DecisionRow {
    pub event_id: i64,
    pub branch_id: String,
    pub candidate_stable_id: String,
    pub decision: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct InvalidationRow {
    pub event_id: i64,
    pub changed_ref: String,
    pub stale_stable_id: String,
    pub scope: String,
    pub reason_code: String,
    pub message: String,
}

pub fn ensure_main(db: &mut Connection) -> anyhow::Result<()> {
    let tx = db.transaction()?;
    let exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM branches WHERE branch_id='main'",
        [],
        |row| row.get(0),
    )?;
    if exists == 0 {
        let event_id = insert_event(
            &tx,
            "main",
            1,
            "BRANCH_CREATED",
            "system",
            "Primary review branch created",
            json!({"branch_id":"main"}),
            None,
        )?;
        tx.execute(
            "INSERT INTO branches(branch_id,title,parent_branch_id,parent_event_id,created_event_id,fork_from_event_id) VALUES (?1,'Main lineage',NULL,NULL,?2,NULL)",
            rusqlite::params![event_id],
        )?;
        tx.commit()?;
    } else {
        tx.rollback()?;
    }
    Ok(())
}

pub fn commit_initial_case(db: &mut Connection, inputs: &[MaterialInput], actor: &str) -> anyhow::Result<i64> {
    ensure_main(db)?;
    if count_events(db, "main")? > 1 {
        anyhow::bail!("initial case has already been imported");
    }
    let tx = db.transaction()?;
    let event_id = append_event(
        &tx,
        "main",
        "SOURCE_IMPORT",
        actor,
        "Atomic initial FCS and gating lineage import",
        json!({"atomic_import": true, "material_count": inputs.len()}),
        None,
    )?;
    for input in inputs {
        insert_material(&tx, "main", event_id, input)?;
    }
    derive_all(&tx, "main", event_id)?;
    tx.commit()?;
    Ok(event_id)
}

pub fn commit_source_revision(
    db: &mut Connection,
    branch_id: &str,
    inputs: &[MaterialInput],
    actor: &str,
    summary: &str,
) -> anyhow::Result<i64> {
    let tx = db.transaction()?;
    let event_id = append_event(
        &tx,
        branch_id,
        "SOURCE_REVISION",
        actor,
        summary,
        json!({"atomic_material_count": inputs.len()}),
        None,
    )?;
    for input in inputs {
        insert_material(&tx, branch_id, event_id, input)?;
    }
    let state_before = replay_branch_tx(&tx, branch_id, Some(event_id - 1))?;
    let state_after = replay_branch_tx(&tx, branch_id, Some(event_id))?;
    invalidate_changed_dependencies(&tx, branch_id, event_id, &state_before, &state_after)?;
    derive_all(&tx, branch_id, event_id)?;
    tx.commit()?;
    Ok(event_id)
}

pub fn create_fork(db: &mut Connection, new_branch: &str, title: &str, base_branch: &str, actor: &str) -> anyhow::Result<i64> {
    let tx = db.transaction()?;
    let base_head = branch_head(&tx, base_branch)?;
    let create_event = append_event(
        &tx,
        new_branch,
        "BRANCH_CREATED",
        actor,
        &format!("Fork {title} from {base_branch}@{base_head}"),
        json!({"fork_from_branch": base_branch, "fork_from_event": base_head}),
        None,
    )?;
    tx.execute(
        "INSERT INTO branches(branch_id,title,parent_branch_id,parent_event_id,created_event_id,fork_from_event_id) VALUES (?1,?2,?3,?4,?5,?6)",
        rusqlite::params![new_branch, title, base_branch, base_head, create_event, base_head],
    )?;
    tx.commit()?;
    Ok(create_event)
}

pub fn record_decision(
    db: &mut Connection,
    branch_id: &str,
    candidate_stable_id: &str,
    decision: &str,
    reason: &str,
    expected_head: i64,
) -> anyhow::Result<i64> {
    if !["accept", "reject"].contains(&decision) {
        anyhow::bail!("unsupported decision {decision}");
    }
    let tx = db.transaction()?;
    let head = branch_head(&tx, branch_id)?;
    if head != expected_head {
        anyhow::bail!("VERSION_CONFLICT: expected head {expected_head}, current head {head}");
    }
    let candidate_exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM derived_results WHERE branch_id=?1 AND stable_id=?2",
        rusqlite::params![branch_id, candidate_stable_id],
        |row| row.get(0),
    )?;
    if candidate_exists == 0 {
        anyhow::bail!("candidate {candidate_stable_id} does not exist on branch {branch_id}");
    }
    let event_id = append_event(
        &tx,
        branch_id,
        "HUMAN_DECISION",
        "reviewer",
        reason,
        json!({"candidate_stable_id": candidate_stable_id, "decision": decision}),
        None,
    )?;
    tx.execute(
        "INSERT INTO decisions(event_id,branch_id,candidate_stable_id,decision,reason) VALUES (?1,?2,?3,?4,?5)",
        rusqlite::params![event_id, branch_id, candidate_stable_id, decision, reason],
    )?;
    tx.commit()?;
    Ok(event_id)
}

pub fn undo_decision(db: &mut Connection, branch_id: &str, candidate_stable_id: &str, reason: &str) -> anyhow::Result<i64> {
    let tx = db.transaction()?;
    let latest: Option<(i64, String)> = tx.query_row(
        "SELECT event_id,decision FROM decisions WHERE branch_id=?1 AND candidate_stable_id=?2 ORDER BY event_id DESC LIMIT 1",
        rusqlite::params![branch_id, candidate_stable_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional_owned()?;
    let Some((_, previous)) = latest else {
        anyhow::bail!("no decision to undo for {candidate_stable_id}");
    };
    let undo = if previous == "accept" { "undo_accept" } else { "undo_reject" };
    let event_id = append_event(
        &tx,
        branch_id,
        "DECISION_UNDONE",
        "reviewer",
        reason,
        json!({"candidate_stable_id": candidate_stable_id, "undo": undo}),
        None,
    )?;
    tx.execute(
        "INSERT INTO decisions(event_id,branch_id,candidate_stable_id,decision,reason) VALUES (?1,?2,?3,?4,?5)",
        rusqlite::params![event_id, branch_id, candidate_stable_id, undo, reason],
    )?;
    tx.commit()?;
    Ok(event_id)
}

pub fn recompute_descendants(db: &mut Connection, branch_id: &str, gate_id: &str, actor: &str) -> anyhow::Result<i64> {
    let tx = db.transaction()?;
    let event_id = append_event(
        &tx,
        branch_id,
        "DESCENDANTS_RECOMPUTED",
        actor,
        &format!("Recompute descendants using accepted {gate_id} population"),
        json!({"anchor_gate": gate_id}),
        None,
    )?;
    derive_all(&tx, branch_id, event_id)?;
    tx.commit()?;
    Ok(event_id)
}

fn insert_event(
    tx: &Transaction,
    branch_id: &str,
    seq: i64,
    event_type: &str,
    actor: &str,
    summary: &str,
    payload: Value,
    supersedes: Option<i64>,
) -> anyhow::Result<i64> {
    tx.execute(
        "INSERT INTO events(branch_id,seq,event_type,actor,summary,payload_json,supersedes_event_id,global_seq) VALUES (?1,?2,?3,?4,?5,?6,?7, COALESCE((SELECT MAX(global_seq)+1 FROM events),1))",
        rusqlite::params![branch_id, seq, event_type, actor, summary, payload.to_string(), supersedes],
    )?;
    Ok(tx.last_insert_rowid())
}

fn append_event(
    tx: &Transaction,
    branch_id: &str,
    event_type: &str,
    actor: &str,
    summary: &str,
    payload: Value,
    supersedes: Option<i64>,
) -> anyhow::Result<i64> {
    let seq: i64 = tx.query_row(
        "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE branch_id=?1",
        rusqlite::params![branch_id],
        |row| row.get(0),
    )?;
    insert_event(tx, branch_id, seq, event_type, actor, summary, payload, supersedes)
}

fn branch_head(tx: &Transaction, branch_id: &str) -> anyhow::Result<i64> {
    tx.query_row(
        "SELECT COALESCE(MAX(event_id),0) FROM events WHERE branch_id=?1",
        rusqlite::params![branch_id],
        |row| row.get(0),
    ).map_err(Into::into)
}

fn count_events(db: &Connection, branch_id: &str) -> anyhow::Result<i64> {
    db.query_row(
        "SELECT COUNT(*) FROM events WHERE branch_id=?1",
        rusqlite::params![branch_id],
        |row| row.get(0),
    ).map_err(Into::into)
}

fn insert_material(tx: &Transaction, branch_id: &str, event_id: i64, input: &MaterialInput) -> anyhow::Result<()> {
    let raw = std::str::from_utf8(&input.bytes)
        .map_err(|error| anyhow::anyhow!("material {} must be UTF-8 JSON: {error}", input.filename))?;
    let json_value: Value = serde_json::from_str(raw)
        .map_err(|error| anyhow::anyhow!("material {} is not valid JSON: {error}", input.filename))?;
    let material_id = json_value
        .get("material_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("material {} lacks material_id", input.filename))?
        .to_string();
    let version = json_value
        .get("version")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("material {} lacks version", input.filename))?;
    let kind = kind_for_material_id(&material_id).to_string();
    let canonical = canonical_json(&json_value)?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let sha = format!("{:x}", hasher.finalize());
    tx.execute(
        "INSERT INTO materials(material_kind,material_id,version,event_id,branch_id,raw_bytes,canonical_json,sha256_hex,content_type,filename)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'application/json',?9)",
        rusqlite::params![kind, material_id, version, event_id, branch_id, raw, canonical, sha, input.filename],
    ).map_err(|error| anyhow::anyhow!("failed atomic insert for {}@v{}: {error}", material_id, version))?;
    tx.execute(
        "INSERT INTO material_refs(event_id,material_kind,material_id,version,role) VALUES (?1,?2,?3,?4,'direct_source')",
        rusqlite::params![event_id, kind, material_id, version],
    )?;
    Ok(())
}

pub fn canonical_json(value: &Value) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"  ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
    serde::Serialize::serialize(value, &mut serializer)?;
    Ok(String::from_utf8(bytes)?)
}

trait OptionalOwned<T> {
    fn optional_owned(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalOwned<T> for rusqlite::Result<T> {
    fn optional_owned(self) -> rusqlite::Result<Option<T>> {
        use rusqlite::OptionalExtension;
        self.optional()
    }
}

pub fn replay(db: &Connection, branch_id: &str, at_event: Option<i64>) -> anyhow::Result<ReplayState> {
    replay_branch_tx(db, branch_id, at_event)
}

fn replay_branch_tx(
    db: &Connection,
    branch_id: &str,
    at_event: Option<i64>,
) -> anyhow::Result<ReplayState> {
    let path = branch_path(db, branch_id)?;
    let cutoff = at_event.unwrap_or_else(|| path.last().map(|(_, head)| *head).unwrap_or(0));
    let mut event_ids = Vec::new();
    for (index, (branch, head)) in path.iter().enumerate() {
        let limit = if index + 1 == path.len() {
            cutoff.min(*head)
        } else {
            *head
        };
        let mut stmt = db.prepare(
            "SELECT event_id FROM events WHERE branch_id=?1 AND event_id<=?2 ORDER BY global_seq, event_id",
        )?;
        let ids = stmt.query_map(rusqlite::params![branch, limit], |row| row.get::<_, i64>(0))?;
        for id in ids {
            event_ids.push(id?);
        }
    }
    event_ids.sort_by_key(|id| *id);
    event_ids.dedup();
    let events = load_events(db, &event_ids)?;
    let visible: HashSet<i64> = event_ids.iter().copied().collect();
    let materials = load_materials(db, &visible)?;
    let results = load_results(db, &path, cutoff)?;
    let decisions = load_decisions(db, &path, cutoff)?;
    let invalidations = load_invalidations(db, &path, cutoff)?;
    Ok(ReplayState {
        events,
        materials,
        results,
        decisions,
        invalidations,
    })
}

fn branch_path(db: &Connection, start: &str) -> anyhow::Result<Vec<(String, i64)>> {
    let mut reversed = Vec::new();
    let mut current = start.to_string();
    loop {
        let row: Option<(Option<String>, Option<i64>)> = db
            .query_row(
                "SELECT parent_branch_id, parent_event_id FROM branches WHERE branch_id=?1",
                rusqlite::params![current],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional_none()?;
        let Some((parent, parent_head)) = row else {
            anyhow::bail!("unknown branch {current}");
        };
        let head: i64 = db.query_row(
            "SELECT COALESCE(MAX(event_id),0) FROM events WHERE branch_id=?1",
            rusqlite::params![current],
            |row| row.get(0),
        )?;
        reversed.push((current.clone(), head));
        match parent {
            Some(parent) => current = parent,
            None => {
                let _ = parent_head;
                break;
            }
        }
    }
    reversed.reverse();
    Ok(reversed)
}

fn optional_none<T>(result: rusqlite::Result<T>) -> rusqlite::Result<Option<T>> {
    use rusqlite::OptionalExtension;
    result.optional()
}

trait ResultOptionalExt<T> {
    fn optional_none(self) -> rusqlite::Result<Option<T>>;
}

impl<T> ResultOptionalExt<T> for rusqlite::Result<T> {
    fn optional_none(self) -> rusqlite::Result<Option<T>> {
        optional_none(self)
    }
}

fn load_events(db: &Connection, ids: &[i64]) -> anyhow::Result<Vec<EventRow>> {
    let mut out = Vec::new();
    for id in ids {
        out.push(db.query_row(
            "SELECT event_id,global_seq,branch_id,seq,event_type,actor,created_at,summary,payload_json,supersedes_event_id FROM events WHERE event_id=?1",
            rusqlite::params![id],
            map_event,
        )?);
    }
    Ok(out)
}

fn map_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRow> {
    let payload_text: String = row.get(8)?;
    Ok(EventRow {
        event_id: row.get(0)?,
        global_seq: row.get(1)?,
        branch_id: row.get(2)?,
        seq: row.get(3)?,
        event_type: row.get(4)?,
        actor: row.get(5)?,
        created_at: row.get(6)?,
        summary: row.get(7)?,
        payload: serde_json::from_str(&payload_text).unwrap_or(Value::Null),
        supersedes_event_id: row.get(9)?,
    })
}

fn load_materials(db: &Connection, visible: &HashSet<i64>) -> anyhow::Result<BTreeMap<String, CurrentMaterial>> {
    let mut stmt = db.prepare(
        "SELECT material_kind,material_id,version,event_id,branch_id,raw_bytes,canonical_json,sha256_hex,filename FROM materials ORDER BY version",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(CurrentMaterial {
            kind: row.get(0)?,
            material_id: row.get(1)?,
            version: row.get(2)?,
            event_id: row.get(3)?,
            branch_id: row.get(4)?,
            raw_bytes: row.get(5)?,
            canonical_json: row.get(6)?,
            sha256_hex: row.get(7)?,
            filename: row.get(8)?,
        })
    })?;
    let mut latest = BTreeMap::new();
    for row in rows {
        let row = row?;
        if visible.contains(&row.event_id) {
            latest.insert(row.material_id.clone(), row);
        }
    }
    Ok(latest)
}

fn load_results(db: &Connection, path: &[(String, i64)], cutoff: i64) -> anyhow::Result<Vec<DerivedResultRow>> {
    let mut out = Vec::new();
    for (index, (branch, head)) in path.iter().enumerate() {
        let limit = if index + 1 == path.len() { cutoff.min(*head) } else { *head };
        let mut stmt = db.prepare(
            "SELECT stable_id,event_id,branch_id,object_id,result_kind,status,input_refs_json,value_json,denominator_ref_json,diagnostics_json FROM derived_results WHERE branch_id=?1 AND event_id<=?2 ORDER BY stable_id",
        )?;
        let rows = stmt.query_map(rusqlite::params![branch, limit], |row| {
            let input_refs: String = row.get(6)?;
            let value: String = row.get(7)?;
            let denominator: Option<String> = row.get(8)?;
            let diagnostics: String = row.get(9)?;
            Ok(DerivedResultRow {
                stable_id: row.get(0)?,
                event_id: row.get(1)?,
                branch_id: row.get(2)?,
                object_id: row.get(3)?,
                result_kind: row.get(4)?,
                status: row.get(5)?,
                input_refs: serde_json::from_str(&input_refs).unwrap_or(Value::Null),
                value: serde_json::from_str(&value).unwrap_or(Value::Null),
                denominator_ref: denominator.and_then(|text| serde_json::from_str(&text).ok()),
                diagnostics: serde_json::from_str(&diagnostics).unwrap_or_default(),
            })
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

fn load_decisions(db: &Connection, path: &[(String, i64)], cutoff: i64) -> anyhow::Result<Vec<DecisionRow>> {
    let mut out = Vec::new();
    for (index, (branch, head)) in path.iter().enumerate() {
        let limit = if index + 1 == path.len() { cutoff.min(*head) } else { *head };
        let mut stmt = db.prepare(
            "SELECT event_id,branch_id,candidate_stable_id,decision,reason FROM decisions WHERE branch_id=?1 AND event_id<=?2 ORDER BY event_id",
        )?;
        let rows = stmt.query_map(rusqlite::params![branch, limit], |row| {
            Ok(DecisionRow {
                event_id: row.get(0)?,
                branch_id: row.get(1)?,
                candidate_stable_id: row.get(2)?,
                decision: row.get(3)?,
                reason: row.get(4)?,
            })
        })?;
        for row in rows { out.push(row?); }
    }
    Ok(out)
}

fn load_invalidations(db: &Connection, path: &[(String, i64)], cutoff: i64) -> anyhow::Result<Vec<InvalidationRow>> {
    let mut out = Vec::new();
    for (index, (branch, head)) in path.iter().enumerate() {
        let limit = if index + 1 == path.len() { cutoff.min(*head) } else { *head };
        let mut stmt = db.prepare(
            "SELECT event_id,branch_id,changed_ref,stale_stable_id,invalidation_scope,reason_code,message FROM invalidations WHERE branch_id=?1 AND event_id<=?2 ORDER BY event_id",
        )?;
        let rows = stmt.query_map(rusqlite::params![branch, limit], |row| {
            Ok(InvalidationRow {
                event_id: row.get(0)?,
                branch_id: row.get(1)?,
                changed_ref: row.get(2)?,
                stale_stable_id: row.get(3)?,
                scope: row.get(4)?,
                reason_code: row.get(5)?,
                message: row.get(6)?,
            })
        })?;
        for row in rows { out.push(row?); }
    }
    Ok(out)
}

pub fn material_json<T: serde::de::DeserializeOwned>(material: &CurrentMaterial) -> anyhow::Result<T> {
    serde_json::from_str(&material.canonical_json).map_err(Into::into)
}

pub fn effective_status(row: &DerivedResultRow, state: &ReplayState) -> String {
    if state.invalidations.iter().any(|item| item.stale_stable_id == row.stable_id) {
        return "stale".to_string();
    }
    let mut status = row.status.clone();
    for decision in &state.decisions {
        if decision.candidate_stable_id == row.stable_id {
            status = match decision.decision.as_str() {
                "accept" => "accepted".to_string(),
                "reject" => "rejected".to_string(),
                "undo_accept" | "undo_reject" => row.status.clone(),
                other => other.to_string(),
            };
        }
    }
    status
}
