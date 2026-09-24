//! Pure derivation functions: compensation runs, population materialization
//! and invalidation scope. Everything here is deterministic so replay after
//! export/import reproduces identical derived ids.

use crate::domain::*;
use crate::engine::{BranchState, BOUNDARY_TOL, EXTREME_MAGNITUDE, NEAR_SINGULAR_KAPPA};
use std::collections::BTreeSet;

pub fn find_artifact_event(
    state: &BranchState,
    kind: &str,
    key: &str,
) -> Option<(String, ArtifactRec)> {
    state
        .artifacts
        .get(&format!("{}::{}", kind, key))
        .cloned()
        .map(|a| (a.event_id.clone(), a))
}

fn channel_index(raw: &RawFile, name: &str) -> Option<usize> {
    raw.channels.iter().position(|c| c.name == name)
}

/// Compute a compensation run. Near-singular / singular matrices never get a
/// silently-picked inverse: every legal candidate is retained.
pub fn compensate(raw: &RawFile, matrix: &CompMatrix) -> CompRun {
    let verdict = crate::mathx::verdict(&matrix.rows, NEAR_SINGULAR_KAPPA);
    let n_ch = matrix.detectors.len();
    let inverse = crate::mathx::invert(&matrix.rows);

    let mut candidates: Vec<CandidateResult> = Vec::new();
    let mut status = "unique".to_string();
    let mut unresolved_global = Vec::new();

    let build = |method: &str,
                 label: &str,
                 params: serde_json::Value,
                 inv: &[Vec<f64>],
                 unresolved: &[usize],
                 candidates: &mut Vec<CandidateResult>| {
        let mut rows = Vec::new();
        let mut flags = Vec::new();
        for ev in &raw.events {
            let mut inp = vec![None; n_ch];
            for (i, d) in matrix.detectors.iter().enumerate() {
                if let Some(ci) = channel_index(raw, d) {
                    inp[i] = ev[ci].value;
                    if ev[ci].quality == CellQuality::NonFinite {
                        inp[i] = None;
                    }
                }
            }
            let comp = crate::mathx::apply_matrix(inv, &inp);
            let mut row_flags = vec![Vec::new(); n_ch];
            for i in 0..n_ch {
                if unresolved.contains(&i) {
                    row_flags[i].push("unresolved".into());
                }
                if inp[i].is_none() {
                    row_flags[i].push("non-finite".into());
                }
                if let Some(x) = comp[i] {
                    if x.abs() > EXTREME_MAGNITUDE {
                        row_flags[i].push("extreme".into());
                    }
                }
            }
            flags.push(row_flags);
            rows.push(comp);
        }
        let cid = crate::ids::derived_id(
            "cand",
            &[
                &raw.sha256,
                &matrix.note,
                method,
                &serde_json::to_string(&params).unwrap(),
            ],
        );
        candidates.push(CandidateResult {
            candidate_id: cid,
            method: method.to_string(),
            method_label: label.to_string(),
            params,
            compensated: rows,
            flags,
            unresolved_channels: unresolved
                .iter()
                .map(|&i| matrix.detectors[i].clone())
                .collect(),
        });
    };

    if verdict.status == "ok" {
        let inv = inverse.expect("well-conditioned matrix inverts");
        build(
            "inverse",
            "exact inverse".into(),
            serde_json::json!({"method": "inverse"}),
            &inv,
            &[],
            &mut candidates,
        );
    } else {
        status = "underdetermined".to_string();
        let (pinv, unresolved) = crate::mathx::pseudo_inverse(&matrix.rows);
        unresolved_global = unresolved.iter().map(|&i| matrix.detectors[i].clone()).collect();
        build(
            "pseudoinverse",
            "Moore-Penrose pseudoinverse".into(),
            serde_json::json!({"method": "pseudoinverse", "tol": 1e-9}),
            &pinv,
            &unresolved,
            &mut candidates,
        );
        for lambda in [0.1_f64, 1.0_f64] {
            let r = crate::mathx::ridge_inverse(&matrix.rows, lambda);
            let unresolved_ridx: Vec<usize> = if verdict.status == "singular" {
                unresolved.clone()
            } else {
                Vec::new()
            };
            build(
                "ridge",
                format!("ridge regularized inverse (lambda={})", lambda),
                serde_json::json!({"method": "ridge", "lambda": lambda}),
                &r,
                &unresolved_ridx,
                &mut candidates,
            );
        }
    }

    let run_id = crate::ids::derived_id("run", &[&raw.sha256, &matrix.note]);
    let matrix_position: std::collections::BTreeMap<String, usize> = matrix
        .detectors
        .iter()
        .enumerate()
        .map(|(i, d)| (d.clone(), i))
        .collect();
    CompRun {
        run_id,
        raw_id: raw.sha256.clone(),
        matrix_id: crate::ids::artifact_id(
            "comp_matrix",
            &crate::engine::canonical(&serde_json::to_value(matrix).unwrap()),
        ),
        matrix_position,
        status,
        candidates,
        picked_candidate_id: None,
    }
}

fn coord_value(
    raw: &RawFile,
    run: Option<&CompRun>,
    candidate_id: Option<&str>,
    transforms: &std::collections::BTreeMap<String, TransformSpec>,
    event_idx: usize,
    axis: &AxisRef,
) -> Option<f64> {
    let ci = channel_index(raw, &axis.channel)?;
    match axis.basis.as_str() {
        "raw" => raw.events[event_idx][ci].value,
        "compensated" | "transformed" => {
            let run = run?;
            let cid = candidate_id.or(run.picked_candidate_id.as_deref())?;
            let cand = run.candidates.iter().find(|c| c.candidate_id == cid)?;
            let det_pos = run.matrix_position.get(&axis.channel).copied().unwrap_or(ci);
            let v = cand.compensated[event_idx].get(det_pos).copied().flatten()?;
            if axis.basis == "compensated" {
                return Some(v);
            }
            let t = transforms.get(&axis.channel)?;
            Some(apply_transform(t, v))
        }
        _ => None,
    }
}

pub fn apply_transform(t: &TransformSpec, x: f64) -> f64 {
    match t.kind.as_str() {
        "asinh" => crate::mathx::asinh_transform(
            x,
            t.cofactor.unwrap_or(150.0),
            t.gain.unwrap_or(1.0),
            t.offset.unwrap_or(0.0),
        ),
        _ => (x - t.offset.unwrap_or(0.0)) * t.gain.unwrap_or(1.0),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn materialize(
    state: &BranchState,
    raw: &RawFile,
    gate: &PolygonGate,
    gate_artifact_id: &str,
    lineage_artifact_id: &str,
    input: Option<&PopResult>,
    run: Option<&CompRun>,
    candidate_id: Option<&str>,
    pinned_parent_event_id: &str,
) -> PopResult {
    let parent_ids: std::collections::BTreeSet<u32> = match input {
        Some(p) => p.members.iter().copied().collect(),
        None => (0..raw.events.len() as u32).collect(),
    };
    let mut members = Vec::new();
    let mut boundary = Vec::new();
    for idx in 0..raw.events.len() {
        if !parent_ids.contains(&(idx as u32)) {
            continue;
        }
        let x = coord_value(raw, run, candidate_id, &state.transforms, idx, &gate.x);
        let y = coord_value(raw, run, candidate_id, &state.transforms, idx, &gate.y);
        if let (Some(x), Some(y)) = (x, y) {
            let scale = 1.0_f64.max(x.abs()).max(y.abs()) * 1e-9;
            let (inside, on_edge) =
                crate::mathx::point_in_polygon([x, y], &gate.polygon, BOUNDARY_TOL.max(scale));
            if inside {
                members.push(idx as u32);
            }
            if on_edge && inside {
                boundary.push(idx as u32);
            }
        }
    }
    let denom_count = match input {
        Some(p) => p.count,
        None => raw.events.len() as u32,
    };
    let percent = Some((members.len() as f64) / (denom_count.max(1) as f64) * 100.0);
    let transform_ids: Vec<String> = [gate.x.channel.clone(), gate.y.channel.clone()]
        .into_iter()
        .filter_map(|ch| find_artifact_event(state, "transform", &ch).map(|(id, _)| id))
        .collect();
    let pop_id = crate::ids::derived_id(
        "pop",
        &[
            &raw.sha256,
            gate_artifact_id,
            lineage_artifact_id,
            input.map(|p| p.pop_id.as_str()).unwrap_or("root"),
            run.map(|r| r.run_id.as_str()).unwrap_or("none"),
            candidate_id.unwrap_or("picked"),
            &format!("{:?}", members),
        ],
    );
    PopResult {
        pop_id,
        gate_uid: gate.gate_uid.clone(),
        label: gate.label.clone(),
        gate_artifact_id: gate_artifact_id.to_string(),
        lineage_artifact_id: lineage_artifact_id.to_string(),
        comp_run_id: run.map(|r| r.run_id.clone()),
        candidate_id: candidate_id.map(|s| s.to_string()),
        transform_ids,
        input_pop_id: input.map(|p| p.pop_id.clone()),
        count: members.len() as u32,
        percent,
        denominator: DenominatorRef {
            parent_pop_id: input.map(|p| p.pop_id.clone()),
            parent_gate_uid: input.map(|p| p.gate_uid.clone()),
            parent_label: input.map(|p| p.label.clone()).unwrap_or_else(|| "root".into()),
            pinned_event_id: pinned_parent_event_id.to_string(),
            count: denom_count,
        },
        members,
        boundary_members: boundary,
        basis: format!("{} x {}", gate.x.basis, gate.y.basis),
        stale: false,
        stale_reason: None,
    }
}

/// Populations whose conclusion depends on a revised artifact. The returned
/// ids describe the invalidation *scope*; callers mark them stale, never
/// overwrite them.
pub fn invalidation_scope(
    state: &BranchState,
    kind: &str,
    key: &str,
    geometry_changed: bool,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let matrix_art = find_artifact_event(state, "comp_matrix", key).map(|(_, a)| a.id);
    for pop in &state.pops {
        let hit = match kind {
            "raw" => {
                // Raw channel changes affect every population derived from that
                // raw file; the builder triggers only a genuinely local change
                // (single channel) and the scope is asserted in tests.
                if let Some(run_id) = &pop.comp_run_id {
                    state.runs.get(run_id).map(|r| r.raw_id == key).unwrap_or(false)
                } else {
                    true
                }
            }
            "comp_matrix" => {
                if let Some(run_id) = &pop.comp_run_id {
                    state
                        .runs
                        .get(run_id)
                        .map(|r| matrix_art.as_ref() == Some(&r.matrix_id))
                        .unwrap_or(false)
                } else {
                    false
                }
            }
            "transform" => find_artifact_event(state, "transform", key)
                .map(|(id, _)| pop.transform_ids.contains(&id))
                .unwrap_or(false),
            "gate" => {
                if !geometry_changed {
                    false // pure rename preserves identity: nothing invalidated
                } else {
                    pop.gate_uid == key || is_descendant(state, key, pop)
                }
            }
            _ => false,
        };
        if hit {
            out.insert(pop.pop_id.clone());
        }
    }
    out
}

fn is_descendant(state: &BranchState, ancestor_gate: &str, pop: &PopResult) -> bool {
    // Walk lineage parents from the pop's gate upward.
    let edges = match &state.lineage {
        Some(l) => &l.edges,
        None => return false,
    };
    let parent_of = |uid: &str| -> Option<String> {
        edges
            .iter()
            .find(|e| e.gate_uid == uid)
            .map(|e| e.parent.clone())
    };
    let mut cur = pop.gate_uid.clone();
    loop {
        match parent_of(&cur) {
            Some(p) if p == ancestor_gate => return true,
            Some(p) if p == "root" => return false,
            Some(p) => cur = p,
            None => return false,
        }
    }
}
