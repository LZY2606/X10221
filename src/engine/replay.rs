//! Deterministic replay of the branch-visible event log into HeadState.
//! Every command is one event; recomputation after a material revision marks
//! downstream populations stale without overwriting prior snapshots.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::evaluate::{evaluate_round, HeadState, StaleInfo, StaleMap};
use crate::engine::invalidation::impact;
use crate::engine::lineage::renamed_nodes;
use crate::engine::model::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredEvent {
    pub seq: i64,
    pub branch: String,
    pub kind: String,
    pub code: String,
    pub command: Option<Command>,
    pub basis: serde_json::Value,
    pub conflict: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ReplayNotes {
    pub events: Vec<String>,
    pub impacts: Vec<serde_json::Value>,
    pub conflicts: Vec<serde_json::Value>,
}

#[derive(Clone, Debug)]
pub struct ReplayResult {
    pub state: HeadState,
    pub notes: ReplayNotes,
}

fn polygons_of(state: &HeadState) -> Vec<GatePolygon> {
    state
        .gates
        .values()
        .filter_map(|t| t.polygon_versions.last().map(|(_, _, p)| p.clone()))
        .collect()
}

fn tree_of(state: &HeadState) -> Option<GateTree> {
    state
        .gates
        .values()
        .find_map(|t| t.tree.as_ref().map(|(_, _, tr)| tr.clone()))
}

/// Replay visible events (already filtered to the branch ancestry).
pub fn replay(branch: &str, events: &[StoredEvent]) -> ReplayResult {
    let mut state = HeadState::default();
    let mut notes = ReplayNotes::default();
    let mut seq_cursor = 0i64;

    for ev in events {
        seq_cursor = ev.seq;
        match (&ev.kind, ev.command.as_ref()) {
            ("import", Some(Command::Import(cmd))) => {
                apply_import(branch, ev.seq, &mut state, &mut notes, &cmd.material_id, cmd.content.clone());
            }
            ("decision", Some(Command::Decision { candidate_id, choice, rationale })) => {
                if let Some(cand) = state.candidates.get_mut(candidate_id) {
                    if cand.accepted.is_some() {
                        notes.events.push(format!(
                            "seq {} decision ignored: candidate already decided",
                            ev.seq
                        ));
                    } else {
                        cand.accepted = Some(choice.clone());
                        cand.rationale = rationale.clone();
                        notes.events.push(format!(
                            "seq {} decision {} -> {}", ev.seq, candidate_id, choice
                        ));
                    }
                }
                // decision unblocks: full fresh evaluation round, no stale gates
                if state.candidates.contains_key(candidate_id) {
                    let round = evaluate_round(ev.seq, &state, &StaleMap::default(), branch);
                    absorb_round(ev.seq, &mut state, round);
                }
            }
            ("decision_undo", Some(Command::DecisionUndo { candidate_id })) => {
                if let Some(cand) = state.candidates.get_mut(candidate_id) {
                    cand.accepted = None;
                    cand.rationale.clear();
                    notes.events
                        .push(format!("seq {} decision undone: {}", ev.seq, candidate_id));
                }
                let round = evaluate_round(ev.seq, &state, &StaleMap::default(), branch);
                absorb_round(ev.seq, &mut state, round);
            }
            ("refresh", Some(Command::Refresh { population_ids })) => {
                // Explicit review action: recompute only refreshed populations;
                // descendants remain stale until individually reviewed.
                let mut stale = StaleMap::default();
                for (gid, snap) in &state.populations {
                    if !matches!(snap.status, crate::engine::evaluate::PopStatus::Stale) {
                        continue;
                    }
                    if !population_ids.contains(gid) {
                        stale.0.insert(
                            gid.clone(),
                            StaleInfo {
                                reason: snap.stale_reason.clone().unwrap_or_default(),
                                old_snapshot_id: snap.old_snapshot_id.clone()
                                    .unwrap_or_else(|| snap.snapshot_id.clone()),
                            },
                        );
                    }
                }
                let round = evaluate_round(ev.seq, &state, &stale, branch);
                absorb_round(ev.seq, &mut state, round);
                notes.events
                    .push(format!("seq {} refresh reviewed: {}", ev.seq, population_ids.join(",")));
            }
            ("conflict", _) => {
                notes
                    .conflicts
                    .push(ev.basis.clone());
                notes
                    .events
                    .push(format!("seq {} conflict recorded (no mutation)", ev.seq));
            }
            _ => {
                // branch markers and unknown events carry no projection change
            }
        }
    }
    let _ = seq_cursor;
    ReplayResult { state, notes }
}

fn absorb_round(
    state: &mut HeadState,
    round: crate::engine::evaluate::RoundResult,
) {
    for cand in round.new_candidates {
        state.candidates.insert(cand.candidate_id.clone(), cand);
    }
    for snap in round.snapshots {
        state.populations.insert(snap.gate_id.clone(), snap);
    }
    state.diagnostics = round.notes;
}

fn apply_import(
    branch: &str,
    seq: i64,
    state: &mut HeadState,
    notes: &mut ReplayNotes,
    material_id: &str,
    content: ArtifactContent,
) {
    let kind = content.kind().to_string();
    let version_id = format!("{kind}_v{seq}");

    // Register material track / version.
    let track = state
        .materials
        .entry(material_id.to_string())
        .or_insert_with(|| MaterialTrack {
            kind: kind.clone(),
            versions: Vec::new(),
        });
    track.kind = kind.clone();
    let old_content = track.versions.last().map(|(_, _, c)| c.clone());
    track.versions.push((seq, version_id.clone(), content.clone()));

    // Gate artifacts are mirrored into per-gate tracks (by stable gate_id).
    match &content {
        ArtifactContent::GatePolygon(poly) => {
            state
                .gates
                .entry(poly.gate_id.clone())
                .or_default()
                .polygon_versions
                .push((seq, version_id.clone(), poly.clone()));
        }
        ArtifactContent::GateTree(tree) => {
            if let Some(ArtifactContent::GateTree(old_tree)) = old_content.as_ref() {
                for (gid, old_name, new_name) in renamed_nodes(old_tree, tree) {
                    notes.events.push(format!(
                        "seq {seq} rename gate {gid}: `{old_name}` -> `{new_name}` (identity/edges unchanged)"
                    ));
                }
            }
            for node in &tree.nodes {
                state
                    .gates
                    .entry(node.gate_id.clone())
                    .or_default()
                    .tree = Some((seq, version_id.clone(), tree.clone()));
            }
        }
        _ => {}
    }

    // Rename-only tree revision: no populations invalidated.
    let mut rename_only = false;
    if let ArtifactContent::GateTree(tree) = &content {
        if let Some(ArtifactContent::GateTree(old_tree)) = old_content.as_ref() {
            if crate::engine::lineage::structural_hash(old_tree)
                == crate::engine::lineage::structural_hash(tree)
            {
                rename_only = true;
            }
        }
    }

    if rename_only {
        notes.impacts.push(serde_json::json!({
            "seq": seq,
            "material_id": material_id,
            "kind": kind,
            "affected_populations": [],
            "rename_only": true,
        }));
        let round = evaluate_round(seq, state, &StaleMap::default(), branch);
        absorb_round(state, round);
        return;
    }

    // Initial import (no prior results): full clean evaluation, no stale marks.
    let had_results = !state.populations.is_empty();
    if !had_results {
        let round = evaluate_round(seq, state, &StaleMap::default(), branch);
        notes.impacts.push(serde_json::json!({
            "seq": seq, "material_id": material_id, "kind": kind,
            "affected_populations": [], "initial": true,
        }));
        absorb_round(state, round);
        return;
    }

    // Revision: compute dependency-driven invalidation scope.
    let polygons = polygons_of(state);
    let polygon_refs: Vec<&GatePolygon> = polygons.iter().collect();
    let tree_opt = tree_of(state);
    let impact = impact(
        &kind,
        material_id,
        old_content.as_ref(),
        &content,
        tree_opt.as_ref(),
        &polygon_refs,
    );

    // Build stale map from the CURRENT snapshots (preserving old snapshot ids;
    // old rows are never overwritten - a stale row is a new append).
    let mut stale = StaleMap::default();
    for gid in &impact.affected_populations {
        if let Some(prev) = state.populations.get(gid) {
            stale.0.insert(
                gid.clone(),
                StaleInfo {
                    reason: impact.reason.clone(),
                    old_snapshot_id: prev.snapshot_id.clone(),
                },
            );
        }
    }

    notes.impacts.push(serde_json::json!({
        "seq": seq,
        "material_id": material_id,
        "kind": kind,
        "affected_populations": impact.affected_populations.iter().cloned().collect::<Vec<_>>(),
        "rename_only": impact.rename_only,
        "reason": impact.reason,
    }));

    let round = evaluate_round(seq, state, &stale, branch);
    absorb_round(state, round);
}
