//! Application layer: transactional commands over the ledger.
//!
//! Every import is one transaction (failure rolls back, no half records).
//! Revisions append new versions; upstream changes mark downstream results
//! stale instead of replacing them. Optimistic tip checks give same-version
//! concurrent saves a deterministic conflict.

use crate::domain::*;
use crate::engine::{replay, BranchState, NEAR_SINGULAR_KAPPA};
use crate::store::{AppendError, Store};
use serde_json::json;
use std::collections::BTreeSet;

pub struct App {
    pub store: Store,
}

#[derive(serde::Serialize)]
pub struct CommandResult {
    pub event: LedgerEvent,
    pub stale_marked: Vec<String>,
    pub judgments: Vec<String>,
}

fn machine_judgment(code: &str, rule: &str, subject: &str, severity: &str, detail: serde_json::Value) -> serde_json::Value {
    json!({
        "judgment_code": code,
        "rule_id": rule,
        "subject_id": subject,
        "severity": severity,
        "detail": detail,
        "machine_readable": true,
    })
}

impl App {
    pub fn new(store: Store) -> Self {
        App { store }
    }

    pub fn create_branch(&self, id: &str, label: &str, base: Option<&str>, note: &str) -> Result<(), String> {
        let base_event = match base {
            Some(b) => self.store.branch_tip(b).ok().and_then(|(tip, _)| tip).map(Some).unwrap_or(None),
            None => None,
        };
        if self.store.branch_exists(id) {
            return Ok(());
        }
        self.store.create_branch(id, label, base, base_event.as_deref(), note)
    }

    /// Import one FCS file: parse, store raw bytes and append the raw event in
    /// a single append (parse errors touch nothing).
    pub fn import_fcs(
        &self,
        branch: &str,
        filename: &str,
        bytes: &[u8],
        expected_tip: Option<&str>,
    ) -> Result<(LedgerEvent, RawFile), String> {
        let parsed = crate::fcs::parse_fcs(filename, bytes).map_err(|e| format!("import rejected: {}", e))?;
        // Optional spillover carried inside the FCS keyword is recorded too.
        self.store.put_blob(&parsed.raw.sha256, bytes).map_err(|e| e.to_string())?;
        let payload = serde_json::to_value(&parsed.raw).unwrap();
        match self.store.append(branch, "importer", "raw.imported", &payload, expected_tip, None) {
            Ok(ev) => Ok((ev, parsed.raw)),
            Err(AppendError::Conflict(c)) => Err(conflict_json(&c)),
            Err(e) => Err(format!("{:?}", e)),
        }
    }

    pub fn record_spillover(
        &self,
        branch: &str,
        sp: &SpilloverMeta,
        expected_tip: Option<&str>,
    ) -> Result<LedgerEvent, String> {
        self.append_artifact(branch, "spillover.recorded", sp, expected_tip)
    }

    pub fn put_matrix(
        &self,
        branch: &str,
        m: &CompMatrix,
        expected_tip: Option<&str>,
    ) -> Result<CommandResult, String> {
        let event = self.append_artifact(branch, "comp_matrix.versioned", m, expected_tip)?;
        let verdict = crate::mathx::verdict(&m.rows, NEAR_SINGULAR_KAPPA);
        let mut judgments = Vec::new();
        if verdict.status != "ok" {
            let code = if verdict.status == "singular" {
                "singular-comp-matrix"
            } else {
                "near-singular-comp-matrix"
            };
            let j = machine_judgment(
                code,
                "RULE-COMP-001:near-singular-matrix-must-not-be-silently-inverted",
                &m.note,
                "error",
                json!({
                    "determinant": verdict.determinant,
                    "sigma_min": verdict.sigma_min,
                    "sigma_max": verdict.sigma_max,
                    "kappa": verdict.kappa,
                    "required_action": "retain-all-legal-candidates-no-automatic-inverse",
                }),
            );
            let ev = self
                .store
                .append(branch, "engine", "judgment.machine", &j, Some(&event.id), None)
                .map_err(|e| format!("{:?}", e))?;
            judgments.push(ev.id);
        }
        Ok(CommandResult {
            event,
            stale_marked: Vec::new(),
            judgments,
        })
    }

    pub fn put_transform(
        &self,
        branch: &str,
        t: &TransformSpec,
        expected_tip: Option<&str>,
    ) -> Result<CommandResult, String> {
        let event = self.append_artifact(branch, "transform.versioned", t, expected_tip)?;
        let state = replay(&self.store, branch);
        let scope = crate::compute::invalidation_scope(&state, "transform", &t.channel, false);
        let stale = self.mark_stale(branch, &event.id, scope, "transform parameters revised")?;
        Ok(CommandResult { event, stale_marked: stale, judgments: vec![] })
    }

    fn append_artifact<T: serde::Serialize>(
        &self,
        branch: &str,
        kind: &str,
        value: &T,
        expected_tip: Option<&str>,
    ) -> Result<LedgerEvent, String> {
        let payload = serde_json::to_value(value).unwrap();
        match self.store.append(branch, "editor", kind, &payload, expected_tip, None) {
            Ok(ev) => Ok(ev),
            Err(AppendError::Conflict(c)) => Err(conflict_json(&c)),
            Err(e) => Err(format!("{:?}", e)),
        }
    }

    /// Put a new gate revision. `geometry_changed=false` is a pure rename:
    /// identity is preserved and nothing downstream is invalidated.
    pub fn put_gate(
        &self,
        branch: &str,
        g: &PolygonGate,
        geometry_changed: bool,
        expected_tip: Option<&str>,
    ) -> Result<CommandResult, String> {
        let event = self.append_artifact(branch, "gate.versioned", g, expected_tip)?;
        let state = replay(&self.store, branch);
        let scope = crate::compute::invalidation_scope(&state, "gate", &g.gate_uid, geometry_changed);
        let stale = self.mark_stale(
            branch,
            &event.id,
            scope,
            if geometry_changed { "upstream gate geometry revised" } else { "gate renamed (no invalidation)" },
        )?;
        let judgments = if !geometry_changed {
            Vec::new()
        } else if !stale.is_empty() {
            let j = machine_judgment(
                "upstream-stale-propagated",
                "RULE-GATE-002:upstream-revision-marks-downstream-stale-without-overwrite",
                &g.gate_uid,
                "warning",
                json!({"stale_pop_ids": stale, "preserved": true}),
            );
            let ev = self
                .store
                .append(branch, "engine", "judgment.machine", &j, self.tip_of(branch).as_deref(), None)
                .map_err(|e| format!("{:?}", e))?;
            vec![ev.id]
        } else {
            Vec::new()
        };
        Ok(CommandResult { event, stale_marked: stale, judgments })
    }

    pub fn put_lineage(
        &self,
        branch: &str,
        l: &Lineage,
        expected_tip: Option<&str>,
    ) -> Result<LedgerEvent, String> {
        self.append_artifact(branch, "lineage.versioned", l, expected_tip)
    }

    pub fn compute_compensation(
        &self,
        branch: &str,
        raw_id: &str,
        matrix_key: &str,
        expected_tip: Option<&str>,
    ) -> Result<CommandResult, String> {
        let state = replay(&self.store, branch);
        let raw = state.raw.get(raw_id).cloned().ok_or_else(|| "raw not found".to_string())?;
        let matrix = state
            .matrices
            .get(matrix_key)
            .cloned()
            .ok_or_else(|| "matrix not found".to_string())?;
        let run = crate::compute::compensate(&raw, &matrix);
        let event = self.append_artifact(branch, "comp.computed", &run, expected_tip)?;
        let mut judgments = Vec::new();
        if run.status == "underdetermined" {
            let v = state.matrix_verdicts.get(matrix_key).cloned().unwrap();
            let j = machine_judgment(
                "compensation-underdetermined-candidates-retained",
                "RULE-COMP-001:near-singular-matrix-must-not-be-silently-inverted",
                &run.run_id,
                "error",
                json!({
                    "candidate_ids": run.candidates.iter().map(|c| c.candidate_id.clone()).collect::<Vec<_>>(),
                    "verdict": v.status,
                    "kappa": v.kappa,
                    "picked_candidate_id": null,
                }),
            );
            let ev = self
                .store
                .append(branch, "engine", "judgment.machine", &j, Some(&event.id), None)
                .map_err(|e| format!("{:?}", e))?;
            judgments.push(ev.id);
        }
        Ok(CommandResult { event, stale_marked: vec![], judgments })
    }

    pub fn materialize_pop(
        &self,
        branch: &str,
        gate_uid: &str,
        run_id: Option<&str>,
        candidate_id: Option<&str>,
        expected_tip: Option<&str>,
    ) -> Result<CommandResult, String> {
        let state = replay(&self.store, branch);
        let gate = state.gates.get(gate_uid).cloned().ok_or_else(|| "gate not found".to_string())?;
        let lineage = state.lineage.clone().ok_or_else(|| "lineage missing".to_string())?;
        let edge = lineage
            .edges
            .iter()
            .find(|e| e.gate_uid == gate_uid)
            .ok_or_else(|| "gate not in lineage".to_string())?;
        // Parent must already be materialized so the denominator can pin a
        // concrete parent population version.
        let input = if edge.parent == "root" {
            None
        } else {
            let parent = state
                .pops
                .iter()
                .filter(|p| p.gate_uid == edge.parent && !p.stale)
                .last()
                .cloned()
                .ok_or_else(|| "parent population not materialized yet".to_string())?;
            Some(parent)
        };
        let raw = state
            .raw
            .values()
            .find(|r| run_id.map(|rid| state.runs.get(rid).map(|run| run.raw_id == r.sha256).unwrap_or(false)).unwrap_or(true))
            .cloned()
            .ok_or_else(|| "raw not found".to_string())?;
        let run = run_id.and_then(|id| state.runs.get(id).cloned());
        if let Some(r) = &run {
            if r.status == "underdetermined" {
                let picked = candidate_id
                    .or(r.picked_candidate_id.as_deref())
                    .and_then(|cid| r.candidates.iter().find(|c| c.candidate_id == cid));
                if picked.is_none() {
                    return Err("cannot materialize: underdetermined compensation requires an explicit candidate decision".into());
                }
            }
        }
        let gate_art = state
            .artifacts
            .get(&format!("gate::{}", gate_uid))
            .map(|a| a.id.clone())
            .unwrap();
        let lin_art = state
            .artifacts
            .get("lineage::tree")
            .map(|a| a.id.clone())
            .unwrap();
        let pinned_event = match &input {
            Some(p) => p.denominator.pinned_event_id.clone(),
            None => state
                .artifacts
                .get(&format!("raw::{}", raw.sha256))
                .map(|a| a.event_id.clone())
                .unwrap(),
        };
        let pop = crate::compute::materialize(
            &state,
            &raw,
            &gate,
            &gate_art,
            &lin_art,
            input.as_ref(),
            run.as_ref(),
            candidate_id,
            &pinned_event,
        );
        let event = self.append_artifact(branch, "pop.materialized", &pop, expected_tip)?;
        // Emit denominator-provenance judgment so the rule is machine readable.
        let j = machine_judgment(
            "denominator-pinned-parent-version",
            "RULE-GATE-001:population-percentage-denominator-pinned-to-parent-version",
            &pop.pop_id,
            "info",
            json!({
                "parent_pop_id": pop.denominator.parent_pop_id,
                "parent_label": pop.denominator.parent_label,
                "pinned_event_id": pop.denominator.pinned_event_id,
                "denominator_count": pop.denominator.count,
                "percent": pop.percent,
            }),
        );
        self.store
            .append(branch, "engine", "judgment.machine", &j, Some(&event.id), None)
            .map_err(|e| format!("{:?}", e))?;
        let last = self.store.get_event(&event.id).unwrap();
        Ok(CommandResult { event: last, stale_marked: vec![], judgments: vec![] })
    }
