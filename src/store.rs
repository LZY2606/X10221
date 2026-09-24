use crate::db;
use crate::derive;
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;

pub const MATERIAL_KINDS: [&str; 6] = [
    "fcs_events",
    "spillover",
    "compensation_matrix",
    "transform_params",
    "gate_polygons",
    "gate_tree",
];

#[derive(Debug)]
pub enum ApiErr {
    BadRequest(String),
    NotFound(String),
    Conflict(Value),
    Internal(String),
}

impl ApiErr {
    pub fn bad(msg: impl Into<String>) -> Self {
        ApiErr::BadRequest(msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        ApiErr::NotFound(msg.into())
    }
    fn internal(msg: impl Into<String>) -> Self {
        ApiErr::Internal(msg.into())
    }
}

impl From<rusqlite::Error> for ApiErr {
    fn from(e: rusqlite::Error) -> Self {
        ApiErr::internal(e.to_string())
    }
}

impl From<serde_json::Error> for ApiErr {
    fn from(e: serde_json::Error) -> Self {
        ApiErr::internal(e.to_string())
    }
}

pub struct Store {
    pub conn: Connection,
}

struct ChangeSet {
    all: bool,
    gates: Vec<String>,
    channels: Vec<String>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store, ApiErr> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Ok(Store { conn })
    }

    pub fn open_memory() -> Result<Store, ApiErr> {
        let conn = Connection::open_in_memory()?;
        Ok(Store { conn })
    }

    pub fn migrate(&self) -> Result<(), ApiErr> {
        db::migrate(&self.conn)?;
        Ok(())
    }

    pub fn reset(&self) -> Result<(), ApiErr> {
        let tx = self.conn.unchecked_transaction()?;
        for t in ["diagnostics", "results", "events", "drafts", "materials"] {
            tx.execute(&format!("DELETE FROM {t}"), [])?;
        }
        tx.execute("DELETE FROM sqlite_sequence WHERE name='events'", [])?;
        tx.commit()?;
        Ok(())
    }

    pub fn append_event(&self, draft_id: Option<&str>, kind: &str, payload: Value) -> Result<i64, ApiErr> {
        self.conn.execute(
            "INSERT INTO events(id, draft_id, kind, payload, created_at) VALUES ('', ?1, ?2, ?3, ?4)",
            params![draft_id, kind, payload.to_string(), db::now_millis()],
        )?;
        let seq = self.conn.last_insert_rowid();
        self.conn.execute(
            "UPDATE events SET id = ?1 WHERE seq = ?2",
            params![format!("evt:{seq}"), seq],
        )?;
        Ok(seq)
    }

    pub fn commit_material(&self, kind: &str, content: Value, raw: Option<String>) -> Result<Value, ApiErr> {
        if !MATERIAL_KINDS.contains(&kind) {
            return Err(ApiErr::bad(format!("unknown material kind: {kind}")));
        }
        let prev: Option<(i64, Value)> = self.latest_material(kind)?;
        let version = prev.as_ref().map(|(v, _)| v + 1).unwrap_or(1);
        let changes = change_set(kind, prev.as_ref().map(|(_, c)| c), &content);
        let id = format!("mat:{kind}:{version}");
        self.conn.execute(
            "INSERT INTO materials(id, kind, version, content, raw, committed_seq, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
            params![id, kind, version, content.to_string(), raw, db::now_millis()],
        )?;
        let seq = self.append_event(
            None,
            "material_committed",
            json!({
                "material": id, "kind": kind, "version": version,
                "change_set": { "all": changes.all, "gates": changes.gates, "channels": changes.channels },
            }),
        )?;
        self.conn.execute(
            "UPDATE materials SET committed_seq = ?1 WHERE id = ?2",
            params![seq, id],
        )?;
        let staled = self.mark_stale(kind, version, &changes, seq)?;
        Ok(json!({
            "id": id, "kind": kind, "version": version, "seq": seq,
            "staled": staled,
        }))
    }

    fn mark_stale(&self, kind: &str, version: i64, changes: &ChangeSet, seq: i64) -> Result<Vec<String>, ApiErr> {
        if !changes.all && changes.gates.is_empty() && changes.channels.is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.version, r.content FROM results r
             INNER JOIN (SELECT id, MAX(version) mv FROM results GROUP BY id) latest
             ON r.id = latest.id AND r.version = latest.mv
             WHERE r.stale = 0",
        )?;
        let rows: Vec<(String, i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        let mut affected: Vec<String> = Vec::new();
        for (rid, rver, content) in rows {
            let content: Value = serde_json::from_str(&content).unwrap_or(Value::Null);
            let deps = content
                .get("deps")
                .cloned()
                .unwrap_or_else(|| json!({"gates": [], "channels": []}));
            let gates: Vec<String> = serde_json::from_value(deps["gates"].clone()).unwrap_or_default();
            let channels: Vec<String> = serde_json::from_value(deps["channels"].clone()).unwrap_or_default();
            let hit = changes.all
                || changes.gates.iter().any(|g| gates.contains(g))
                || changes.channels.iter().any(|c| channels.contains(c));
            if hit {
                self.conn.execute(
                    "UPDATE results SET stale = 1 WHERE id = ?1 AND version = ?2",
                    params![rid, rver],
                )?;
                affected.push(rid);
            }
        }
        if !affected.is_empty() {
            self.append_event(
                None,
                "stale_marked",
                json!({
                    "rule": derive::RULE_STALE,
                    "cause": { "kind": kind, "version": version },
                    "affected": affected,
                    "note": "downstream results marked stale; previous values retained, not overwritten",
                }),
            )?;
            let _ = seq;
        }
        Ok(affected)
    }

    pub fn latest_material(&self, kind: &str) -> Result<Option<(i64, Value)>, ApiErr> {
        let mut stmt = self
            .conn
            .prepare("SELECT version, content FROM materials WHERE kind = ?1 ORDER BY version DESC LIMIT 1")?;
        let mut rows = stmt.query(params![kind])?;
        if let Some(row) = rows.next()? {
            let v: i64 = row.get(0)?;
            let c: String = row.get(1)?;
            Ok(Some((v, serde_json::from_str(&c)?)))
        } else {
            Ok(None)
        }
    }

    pub fn materials_summary(&self) -> Result<Vec<Value>, ApiErr> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, version, id, committed_seq, created_at FROM materials ORDER BY kind, version",
        )?;
        let rows: Vec<(String, i64, String, i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<Result<_, _>>()?;
        let mut by_kind: Map<String, Value> = Map::new();
        for (kind, version, id, seq, at) in rows {
            let entry = by_kind
                .entry(kind.clone())
                .or_insert_with(|| json!({"kind": kind, "versions": []}));
            entry["versions"].as_array_mut().unwrap().push(json!({
                "version": version, "id": id, "committed_seq": seq, "created_at": at,
            }));
        }
        let mut out: Vec<Value> = by_kind.values().cloned().collect();
        for e in &mut out {
            let maxv = e["versions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["version"].as_i64().unwrap())
                .max()
                .unwrap_or(0);
            e["latest_version"] = json!(maxv);
        }
        out.sort_by(|a, b| a["kind"].as_str().cmp(&b["kind"].as_str()));
        Ok(out)
    }

    pub fn material_content(&self, kind: &str, version: i64) -> Result<Value, ApiErr> {
        let mut stmt = self
            .conn
            .prepare("SELECT content FROM materials WHERE kind = ?1 AND version = ?2")?;
        let mut rows = stmt.query(params![kind, version])?;
        match rows.next()? {
            Some(row) => {
                let c: String = row.get(0)?;
                Ok(serde_json::from_str(&c)?)
            }
            None => Err(ApiErr::not_found(format!("{kind} v{version}"))),
        }
    }

    pub fn create_draft(&self, id: &str, name: &str) -> Result<(), ApiErr> {
        let state = json!({"pins": {}, "rulings": {}, "branch": "br-0", "history": []});
        self.conn.execute(
            "INSERT OR IGNORE INTO drafts(id, name, state, head_seq) VALUES (?1, ?2, ?3, 0)",
            params![id, name, state.to_string()],
        )?;
        Ok(())
    }

    pub fn get_draft(&self, id: &str) -> Result<(Value, i64), ApiErr> {
        let mut stmt = self.conn.prepare("SELECT state, head_seq FROM drafts WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => {
                let s: String = row.get(0)?;
                let h: i64 = row.get(1)?;
                Ok((serde_json::from_str(&s)?, h))
            }
            None => Err(ApiErr::not_found(format!("draft {id}"))),
        }
    }

    pub fn list_drafts(&self) -> Result<Vec<Value>, ApiErr> {
        let mut stmt = self.conn.prepare("SELECT id, name, state, head_seq FROM drafts ORDER BY id")?;
        let rows: Vec<(String, String, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<_, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(id, name, state, head)| {
                json!({
                    "id": id, "name": name, "head_seq": head,
                    "state": serde_json::from_str::<Value>(&state).unwrap_or(Value::Null),
                })
            })
            .collect())
    }

    fn set_draft_state(&self, id: &str, state: &Value, head: i64) -> Result<(), ApiErr> {
        self.conn.execute(
            "UPDATE drafts SET state = ?1, head_seq = ?2 WHERE id = ?3",
            params![state.to_string(), head, id],
        )?;
        Ok(())
    }

    pub fn save_ruling(
        &self,
        draft_id: &str,
        base_seq: i64,
        target: &str,
        decision: &str,
    ) -> Result<Value, ApiErr> {
        let (mut state, head) = self.get_draft(draft_id)?;
        if head != base_seq {
            let intervening = self.events_since(draft_id, base_seq)?;
            return Err(ApiErr::Conflict(json!({
                "error": "version_conflict",
                "rule": "same_version_concurrent_modification",
                "draft": draft_id,
                "expected_seq": base_seq,
                "current_seq": head,
                "your_change": {"target": target, "decision": decision},
                "current_rulings": state["rulings"].clone(),
                "intervening_events": intervening,
            })));
        }
        let before = state.clone();
        let branch = format!("br-{}", head + 1);
        state["rulings"][target] = json!(decision);
        state["branch"] = json!(branch);
        state["history"]
            .as_array_mut()
            .map(|h| h.push(json!({"target": target, "decision": decision, "branch": branch})));
        let seq = self.append_event(
            Some(draft_id),
            "ruling",
            json!({
                "draft": draft_id, "target": target, "decision": decision,
                "branch": branch, "state_before": before, "state_after": state,
            }),
        )?;
        self.set_draft_state(draft_id, &state, seq)?;
        self.refresh_diagnostics()?;
        Ok(json!({"seq": seq, "branch": branch, "state": state}))
    }

    pub fn undo(&self, draft_id: &str) -> Result<Value, ApiErr> {
        let (_, head) = self.get_draft(draft_id)?;
        let undone: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT payload FROM events WHERE draft_id = ?1 AND kind = 'undo'")?;
            let rows: Vec<String> = stmt
                .query_map(params![draft_id], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            rows.iter()
                .filter_map(|p| serde_json::from_str::<Value>(p).ok()?["undone_seq"].as_i64())
                .collect()
        };
        let mut stmt = self.conn.prepare(
            "SELECT seq, payload FROM events WHERE draft_id = ?1 AND kind = 'ruling' ORDER BY seq DESC",
        )?;
        let rows: Vec<(i64, String)> = stmt
            .query_map(params![draft_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let target = rows
            .into_iter()
            .find(|(seq, _)| !undone.contains(seq))
            .ok_or_else(|| ApiErr::bad("nothing to undo"))?;
        let payload: Value = serde_json::from_str(&target.1)?;
        let restored = payload["state_before"].clone();
        let seq = self.append_event(
            Some(draft_id),
            "undo",
            json!({
                "draft": draft_id, "undone_seq": target.0,
                "restored_state": restored,
                "note": "ruling reverted; previous branch restored",
            }),
        )?;
        self.set_draft_state(draft_id, &restored, seq)?;
        self.refresh_diagnostics()?;
        let _ = head;
        Ok(json!({"seq": seq, "undone_seq": target.0, "state": restored}))
    }

    fn refresh_diagnostics(&self) -> Result<(), ApiErr> {
        let drafts = self.list_drafts()?;
        let mut comp_resolved = false;
        let mut boundary_resolved = false;
        for d in &drafts {
            let rulings = &d["state"]["rulings"];
            if let Some(v) = rulings.get("compensation").and_then(|v| v.as_str()) {
                if v == "regularize" || v == "skip" {
                    comp_resolved = true;
                }
            }
            if let Some(v) = rulings.get("boundary_policy").and_then(|v| v.as_str()) {
                if v == "include" || v == "exclude" {
                    boundary_resolved = true;
                }
            }
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, code, status FROM diagnostics WHERE severity = 'blocking' OR code = 'boundary_point'")?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        for (id, code, status) in rows {
            let should_resolve = match code.as_str() {
                "near_singular_matrix" | "singular_matrix" => comp_resolved,
                "boundary_point" => boundary_resolved,
                _ => false,
            };
            if should_resolve && status != "resolved" {
                let seq = self.append_event(
                    None,
                    "diagnostic_resolved",
                    json!({"diagnostic": id, "code": code}),
                )?;
                self.conn.execute(
                    "UPDATE diagnostics SET status = 'resolved', resolved_seq = ?1 WHERE id = ?2",
                    params![seq, id],
                )?;
            } else if !should_resolve && status == "resolved" {
                self.conn.execute(
                    "UPDATE diagnostics SET status = 'open', resolved_seq = NULL WHERE id = ?1",
                    params![id],
                )?;
            }
        }
        Ok(())
    }

    pub fn derive(&self, draft_id: &str) -> Result<Value, ApiErr> {
        let (state, _) = self.get_draft(draft_id)?;
        let mut pins: Map<String, Value> = Map::new();
        for kind in MATERIAL_KINDS {
            let pinned = state["pins"].get(kind).and_then(|v| v.as_i64());
            let version = match pinned {
                Some(v) => v,
                None => self
                    .latest_material(kind)?
                    .map(|(v, _)| v)
                    .ok_or_else(|| ApiErr::bad(format!("material {kind} not loaded; seed the case pack first")))?,
            };
            pins.insert(kind.to_string(), json!(version));
        }
        let fcs: derive::FcsData = serde_json::from_value(
            self.material_content("fcs_events", pins["fcs_events"].as_i64().unwrap())?,
        )
        .map_err(|e| ApiErr::bad(format!("fcs_events parse: {e}")))?;
        let comp: derive::CompensationMatrix = serde_json::from_value(
            self.material_content("compensation_matrix", pins["compensation_matrix"].as_i64().unwrap())?,
        )
        .map_err(|e| ApiErr::bad(format!("compensation_matrix parse: {e}")))?;
        let tp: derive::TransformParams = serde_json::from_value(
            self.material_content("transform_params", pins["transform_params"].as_i64().unwrap())?,
        )
        .map_err(|e| ApiErr::bad(format!("transform_params parse: {e}")))?;
        let gates: derive::GateSet = serde_json::from_value(
            self.material_content("gate_polygons", pins["gate_polygons"].as_i64().unwrap())?,
        )
        .map_err(|e| ApiErr::bad(format!("gate_polygons parse: {e}")))?;
        let tree: derive::GateTree = serde_json::from_value(
            self.material_content("gate_tree", pins["gate_tree"].as_i64().unwrap())?,
        )
        .map_err(|e| ApiErr::bad(format!("gate_tree parse: {e}")))?;
        let rulings: HashMap<String, String> =
            serde_json::from_value(state["rulings"].clone()).unwrap_or_default();
        let out = derive::run(&fcs, &comp, &tp, &gates, &tree, &rulings);

        let seq = self.append_event(
            Some(draft_id),
            "derivation_run",
            json!({
                "draft": draft_id,
                "pins": pins,
                "rule": derive::RULE_DENOMINATOR,
                "comp_mode": out.comp_mode,
                "matrix": out.matrix_detail,
                "total_events": out.total_events,
            }),
        )?;

        let mut node_versions: HashMap<String, i64> = HashMap::new();
        let mut outputs: Vec<Value> = Vec::new();
        for p in &out.populations {
            let rid = format!("pop:{draft_id}:{}", p.node_id);
            let next_ver: i64 = self.conn.query_row(
                "SELECT COALESCE(MAX(version), 0) + 1 FROM results WHERE id = ?1",
                params![rid],
                |r| r.get(0),
            )?;
            node_versions.insert(p.node_id.clone(), next_ver);
            let denominator = match &p.parent {
                None => json!({
                    "population": "all_events",
                    "version": pins["fcs_events"].as_i64().unwrap(),
                    "count": out.total_events,
                }),
                Some(par) => {
                    let par_id = format!("pop:{draft_id}:{par}");
                    json!({
                        "population": par_id,
                        "version": node_versions.get(par).copied().unwrap_or(0),
                        "count": p.denominator_count,
                    })
                }
            };
            let content = json!({
                "node_id": p.node_id,
                "name": p.name,
                "parent": p.parent,
                "status": p.status,
                "count": p.count,
                "count_include_boundary": p.count_include_boundary,
                "count_exclude_boundary": p.count_exclude_boundary,
                "boundary_events": p.boundary_events,
                "comp_variants": p.comp_variants,
                "percent": p.percent,
                "deps": {"gates": p.deps_gates, "channels": p.deps_channels},
            });
            self.conn.execute(
                "INSERT INTO results(id, version, draft_id, node_id, content, inputs, stale, created_seq, denominator)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8)",
                params![
                    rid,
                    next_ver,
                    draft_id,
                    p.node_id,
                    content.to_string(),
                    Value::Object(pins.clone()).to_string(),
                    seq,
                    denominator.to_string(),
                ],
            )?;
            outputs.push(json!({"id": rid, "version": next_ver}));
        }

        for d in &out.diagnostics {
            let exists: bool = self.conn.query_row(
                "SELECT COUNT(*) FROM diagnostics WHERE id = ?1",
                params![d.id],
                |r| r.get::<_, i64>(0),
            )? > 0;
            if exists {
                self.conn.execute(
                    "UPDATE diagnostics SET detail = ?1, candidates = ?2, severity = ?3 WHERE id = ?4",
                    params![d.detail.to_string(), serde_json::to_string(&d.candidates)?, d.severity, d.id],
                )?;
            } else {
                self.conn.execute(
                    "INSERT INTO diagnostics(id, code, subject, severity, detail, candidates, status, created_seq)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7)",
                    params![
                        d.id,
                        d.code,
                        d.subject,
                        d.severity,
                        d.detail.to_string(),
                        serde_json::to_string(&d.candidates)?,
                        seq,
                    ],
                )?;
                self.append_event(
                    Some(draft_id),
                    "diagnostic",
                    json!({
                        "diagnostic": d.id, "code": d.code, "subject": d.subject,
                        "severity": d.severity, "detail": d.detail, "candidates": d.candidates,
                    }),
                )?;
            }
        }
        self.refresh_diagnostics()?;

        let mut new_state = state.clone();
        new_state["pins"] = Value::Object(pins.clone());
        self.set_draft_state(draft_id, &new_state, seq)?;
        Ok(json!({
            "seq": seq,
            "pins": pins,
            "outputs": outputs,
            "diagnostics": out.diagnostics.iter().map(|d| d.id.clone()).collect::<Vec<_>>(),
            "comp_mode": out.comp_mode,
        }))
    }

    pub fn results_for(&self, draft_id: &str) -> Result<Vec<Value>, ApiErr> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.version, r.node_id, r.content, r.inputs, r.stale, r.created_seq, r.denominator
             FROM results r
             INNER JOIN (SELECT id, MAX(version) mv FROM results GROUP BY id) latest
             ON r.id = latest.id AND r.version = latest.mv
             WHERE r.draft_id = ?1
             ORDER BY r.created_seq DESC, r.id",
        )?;
        let rows = stmt
            .query_map(params![draft_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out: Vec<Value> = rows
            .into_iter()
            .map(|(id, version, node_id, content, inputs, stale, created_seq, denominator)| {
                json!({
                    "id": id, "version": version, "node_id": node_id,
                    "content": serde_json::from_str::<Value>(&content).unwrap_or(Value::Null),
                    "inputs": serde_json::from_str::<Value>(&inputs).unwrap_or(Value::Null),
                    "stale": stale == 1,
                    "created_seq": created_seq,
                    "denominator": denominator.and_then(|d| serde_json::from_str::<Value>(&d).ok()),
                })
            })
            .collect();
        out.sort_by(|a, b| {
            let ka = format!("{:020}:{}", a["created_seq"].as_i64().unwrap_or(0), a["id"].as_str().unwrap_or(""));
            let kb = format!("{:020}:{}", b["created_seq"].as_i64().unwrap_or(0), b["id"].as_str().unwrap_or(""));
            ka.cmp(&kb)
        });
        Ok(out)
    }

    pub fn diagnostics(&self) -> Result<Vec<Value>, ApiErr> {
        let mut stmt = self.conn.prepare(
            "SELECT id, code, subject, severity, detail, candidates, status, created_seq, resolved_seq
             FROM diagnostics ORDER BY created_seq, id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, Option<i64>>(8)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(id, code, subject, severity, detail, candidates, status, created, resolved)| {
                json!({
                    "id": id, "code": code, "subject": subject, "severity": severity,
                    "detail": serde_json::from_str::<Value>(&detail).unwrap_or(Value::Null),
                    "candidates": serde_json::from_str::<Value>(&candidates).unwrap_or(Value::Null),
                    "status": status, "created_seq": created, "resolved_seq": resolved,
                })
            })
            .collect())
    }

    pub fn events_since(&self, draft_id: &str, since: i64) -> Result<Vec<Value>, ApiErr> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, id, draft_id, kind, payload, created_at FROM events
             WHERE seq > ?1 AND (draft_id = ?2 OR draft_id IS NULL) ORDER BY seq",
        )?;
        let rows = stmt
            .query_map(params![since, draft_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(seq, id, draft, kind, payload, at)| {
                json!({
                    "seq": seq, "id": id, "draft_id": draft, "kind": kind,
                    "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                    "created_at": at,
                })
            })
            .collect())
    }

    pub fn events(&self, limit: i64) -> Result<Vec<Value>, ApiErr> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, id, draft_id, kind, payload, created_at FROM events ORDER BY seq DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out: Vec<Value> = rows
            .into_iter()
            .map(|(seq, id, draft, kind, payload, at)| {
                json!({
                    "seq": seq, "id": id, "draft_id": draft, "kind": kind,
                    "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                    "created_at": at,
                })
            })
            .collect();
        out.sort_by_key(|e| e["seq"].as_i64().unwrap_or(0));
        Ok(out)
    }

    pub fn view_at(&self, seq: i64) -> Result<Value, ApiErr> {
        let mut materials = Map::new();
        for kind in MATERIAL_KINDS {
            let mut stmt = self.conn.prepare(
                "SELECT version, id FROM materials WHERE kind = ?1 AND committed_seq <= ?2
                 ORDER BY version DESC LIMIT 1",
            )?;
            let mut rows = stmt.query(params![kind, seq])?;
            if let Some(row) = rows.next()? {
                materials.insert(
                    kind.to_string(),
                    json!({"version": row.get::<_, i64>(0)?, "id": row.get::<_, String>(1)?}),
                );
            }
        }
        let stale_events: Vec<(i64, Value)> = {
            let mut stmt = self.conn.prepare(
                "SELECT seq, payload FROM events WHERE kind = 'stale_marked' AND seq <= ?1 ORDER BY seq",
            )?;
            let rows: Vec<(i64, String)> = stmt
                .query_map(params![seq], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            rows.into_iter()
                .filter_map(|(s, p)| serde_json::from_str::<Value>(&p).ok().map(|v| (s, v)))
                .collect()
        };
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.version, r.draft_id, r.content, r.created_seq, r.denominator FROM results r
             INNER JOIN (SELECT id, MAX(version) mv FROM results WHERE created_seq <= ?1 GROUP BY id) latest
             ON r.id = latest.id AND r.version = latest.mv
             WHERE r.created_seq <= ?1
             ORDER BY r.id",
        )?;
        let rows: Vec<(String, i64, String, String, i64, Option<String>)> = stmt
            .query_map(params![seq], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
            })?
            .collect::<Result<_, _>>()?;
        let results: Vec<Value> = rows
            .into_iter()
            .map(|(id, version, draft, content, created_seq, denominator)| {
                let stale = stale_events.iter().any(|(s, p)| {
                    *s > created_seq
                        && p["affected"]
                            .as_array()
                            .map(|a| a.iter().any(|x| x.as_str() == Some(id.as_str())))
                            .unwrap_or(false)
                });
                json!({
                    "id": id, "version": version, "draft_id": draft,
                    "content": serde_json::from_str::<Value>(&content).unwrap_or(Value::Null),
                    "created_seq": created_seq,
                    "stale": stale,
                    "denominator": denominator.and_then(|d| serde_json::from_str::<Value>(&d).ok()),
                })
            })
            .collect();
        let mut stmt = self.conn.prepare(
            "SELECT id, code, subject, detail, candidates, status, created_seq, resolved_seq
             FROM diagnostics WHERE created_seq <= ?1 ORDER BY created_seq, id",
        )?;
        let diags: Vec<Value> = stmt
            .query_map(params![seq], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|(id, code, subject, detail, candidates, status, created, resolved)| {
                let visible_status = match resolved {
                    Some(r) if r <= seq => "resolved".to_string(),
                    _ => {
                        if resolved.is_some() {
                            "open".to_string()
                        } else {
                            status
                        }
                    }
                };
                json!({
                    "id": id, "code": code, "subject": subject,
                    "detail": serde_json::from_str::<Value>(&detail).unwrap_or(Value::Null),
                    "candidates": serde_json::from_str::<Value>(&candidates).unwrap_or(Value::Null),
                    "status": visible_status, "created_seq": created,
                })
            })
            .collect();
        let events = self.events(10_000)?;
        let events: Vec<Value> = events
            .into_iter()
            .filter(|e| e["seq"].as_i64().unwrap_or(0) <= seq)
            .collect();
        Ok(json!({
            "seq": seq,
            "materials": materials,
            "results": results,
            "diagnostics": diags,
            "events": events,
            "note": "historical view reconstructed from event log; dependency versions and conclusions as of this event",
        }))
    }

    pub fn export_archive(&self) -> Result<Value, ApiErr> {
        let materials = {
            let mut stmt = self.conn.prepare(
                "SELECT id, kind, version, content, raw, committed_seq, created_at FROM materials
                 ORDER BY kind, version",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?,
                    "version": r.get::<_, i64>(2)?,
                    "content": serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
                    "raw": r.get::<_, Option<String>>(4)?,
                    "committed_seq": r.get::<_, i64>(5)?, "created_at": r.get::<_, String>(6)?,
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let drafts = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, name, state, head_seq FROM drafts ORDER BY id")?;
            let rows = stmt.query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?,
                    "state": serde_json::from_str::<Value>(&r.get::<_, String>(2)?).unwrap_or(Value::Null),
                    "head_seq": r.get::<_, i64>(3)?,
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let events = self.events(1_000_000)?;
        let results = {
            let mut stmt = self.conn.prepare(
            "SELECT id, version, draft_id, node_id, content, inputs, stale, created_seq, denominator
                 FROM results ORDER BY id, version",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?, "version": r.get::<_, i64>(1)?,
                    "draft_id": r.get::<_, String>(2)?, "node_id": r.get::<_, String>(3)?,
                    "content": serde_json::from_str::<Value>(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                    "inputs": serde_json::from_str::<Value>(&r.get::<_, String>(5)?).unwrap_or(Value::Null),
                    "stale": r.get::<_, i64>(6)?,
                    "created_seq": r.get::<_, i64>(7)?,
                    "denominator": r.get::<_, Option<String>>(8)?
                        .and_then(|d| serde_json::from_str::<Value>(&d).ok()),
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let diagnostics = self.diagnostics()?;
        Ok(json!({
            "format": "flowline-archive/1",
            "materials": materials,
            "drafts": drafts,
            "events": events,
            "results": results,
            "diagnostics": diagnostics,
        }))
    }

    pub fn import_archive(&self, archive: &Value) -> Result<Value, ApiErr> {
        if archive["format"].as_str() != Some("flowline-archive/1") {
            return Err(ApiErr::bad("unknown archive format"));
        }
        let materials = archive["materials"].as_array().cloned().unwrap_or_default();
        let drafts = archive["drafts"].as_array().cloned().unwrap_or_default();
        let events = archive["events"].as_array().cloned().unwrap_or_default();
        let results = archive["results"].as_array().cloned().unwrap_or_default();
        let diagnostics = archive["diagnostics"].as_array().cloned().unwrap_or_default();
        let tx = self.conn.unchecked_transaction()?;
        for t in ["diagnostics", "results", "events", "drafts", "materials"] {
            tx.execute(&format!("DELETE FROM {t}"), [])?;
        }
        for m in &materials {
            tx.execute(
                "INSERT INTO materials(id, kind, version, content, raw, committed_seq, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    m["id"].as_str().unwrap_or(""),
                    m["kind"].as_str().unwrap_or(""),
                    m["version"].as_i64().unwrap_or(0),
                    m["content"].to_string(),
                    m["raw"].as_str().map(|s| s.to_string()),
                    m["committed_seq"].as_i64().unwrap_or(0),
                    m["created_at"].as_str().unwrap_or(""),
                ],
            )?;
        }
        for d in &drafts {
            tx.execute(
                "INSERT INTO drafts(id, name, state, head_seq) VALUES (?1, ?2, ?3, ?4)",
                params![
                    d["id"].as_str().unwrap_or(""),
                    d["name"].as_str().unwrap_or(""),
                    d["state"].to_string(),
                    d["head_seq"].as_i64().unwrap_or(0),
                ],
            )?;
        }
        let mut max_seq = 0i64;
        for e in &events {
            let seq = e["seq"].as_i64().unwrap_or(0);
            max_seq = max_seq.max(seq);
            tx.execute(
                "INSERT INTO events(seq, id, draft_id, kind, payload, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    seq,
                    e["id"].as_str().unwrap_or(""),
                    e["draft_id"].as_str().map(|s| s.to_string()),
                    e["kind"].as_str().unwrap_or(""),
                    e["payload"].to_string(),
                    e["created_at"].as_str().unwrap_or(""),
                ],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO sqlite_sequence(name, seq) VALUES('events', ?1)",
            params![max_seq],
        )?;
        for r in &results {
            tx.execute(
                "INSERT INTO results(id, version, draft_id, node_id, content, inputs, stale, created_seq, denominator)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    r["id"].as_str().unwrap_or(""),
                    r["version"].as_i64().unwrap_or(0),
                    r["draft_id"].as_str().unwrap_or(""),
                    r["node_id"].as_str().unwrap_or(""),
                    r["content"].to_string(),
                    r["inputs"].to_string(),
                    r["stale"].as_i64().unwrap_or(0),
                    r["created_seq"].as_i64().unwrap_or(0),
                    if r["denominator"].is_null() { None } else { Some(r["denominator"].to_string()) },
                ],
            )?;
        }
        for d in &diagnostics {
            tx.execute(
                "INSERT INTO diagnostics(id, code, subject, severity, detail, candidates, status, created_seq, resolved_seq)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    d["id"].as_str().unwrap_or(""),
                    d["code"].as_str().unwrap_or(""),
                    d["subject"].as_str().unwrap_or(""),
                    d["severity"].as_str().unwrap_or("info"),
                    d["detail"].to_string(),
                    d["candidates"].to_string(),
                    d["status"].as_str().unwrap_or("open"),
                    d["created_seq"].as_i64().unwrap_or(0),
                    d["resolved_seq"].as_i64(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(json!({
            "imported": {
                "materials": materials.len(),
                "drafts": drafts.len(),
                "events": events.len(),
                "results": results.len(),
                "diagnostics": diagnostics.len(),
            }
        }))
    }

    pub fn compare(&self, a: &str, b: &str) -> Result<Value, ApiErr> {
        let ra = self.results_for(a)?;
        let rb = self.results_for(b)?;
        let mut nodes: Vec<String> = ra.iter().chain(rb.iter())
            .map(|r| r["node_id"].as_str().unwrap_or("").to_string())
            .collect();
        nodes.sort();
        nodes.dedup();
        let rows: Vec<Value> = nodes
            .into_iter()
            .map(|n| {
                let fa = ra.iter().find(|r| r["node_id"].as_str() == Some(n.as_str()));
                let fb = rb.iter().find(|r| r["node_id"].as_str() == Some(n.as_str()));
                json!({
                    "node_id": n,
                    "a": fa.cloned().unwrap_or(Value::Null),
                    "b": fb.cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        Ok(json!({"draft_a": a, "draft_b": b, "rows": rows}))
    }
}

fn change_set(kind: &str, old: Option<&Value>, new: &Value) -> ChangeSet {
    let Some(old) = old else {
        return ChangeSet { all: true, gates: vec![], channels: vec![] };
    };
    if old == new {
        return ChangeSet { all: false, gates: vec![], channels: vec![] };
    }
    match kind {
        "transform_params" => {
            let mut channels = vec![];
            let empty = json!({});
            let oc = &old["cofactor"];
            let nc = &new["cofactor"];
            let oc = if oc.is_object() { oc } else { &empty };
            let nc = if nc.is_object() { nc } else { &empty };
            let keys: Vec<&str> = oc
                .as_object()
                .into_iter()
                .chain(nc.as_object())
                .flat_map(|m| m.keys().map(|k| k.as_str()))
                .collect();
            for k in keys {
                if oc.get(k) != nc.get(k) && !channels.contains(&k.to_string()) {
                    channels.push(k.to_string());
                }
            }
            ChangeSet { all: false, gates: vec![], channels }
        }
        "gate_polygons" => {
            let mut gates = vec![];
            let empty = json!({});
            let og = &old["gates"];
            let ng = &new["gates"];
            let og = if og.is_object() { og } else { &empty };
            let ng = if ng.is_object() { ng } else { &empty };
            let keys: Vec<&str> = og
                .as_object()
                .into_iter()
                .chain(ng.as_object())
                .flat_map(|m| m.keys().map(|k| k.as_str()))
                .collect();
            for k in keys {
                if og.get(k) != ng.get(k) && !gates.contains(&k.to_string()) {
                    gates.push(k.to_string());
                }
            }
            ChangeSet { all: false, gates, channels: vec![] }
        }
        "gate_tree" => {
            let parents_changed = {
                let pmap = |nodes: &Value| -> HashMap<String, String> {
                    nodes
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|n| {
                                    (
                                        n["id"].as_str().unwrap_or("").to_string(),
                                        n["parent"].as_str().unwrap_or("").to_string(),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                };
                pmap(&old["nodes"]) != pmap(&new["nodes"])
            };
            if parents_changed {
                ChangeSet { all: true, gates: vec![], channels: vec![] }
            } else {
                ChangeSet { all: false, gates: vec![], channels: vec![] }
            }
        }
        _ => ChangeSet { all: true, gates: vec![], channels: vec![] },
    }
}
