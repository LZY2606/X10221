//! Pure projection: replay visible events into population results.
//!
//! Rules enforced here:
//! * a near-singular compensation matrix is never silently inverted
//!   (a multi-option candidate is emitted instead);
//! * every percentage denominator is pinned to an exact parent-population
//!   snapshot version (material versions + sequence + count);
//! * revising an upstream gate marks downstream results stale without
//!   overwriting them; refresh is a new event producing new snapshots.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::engine::model::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PinnedVersions {
    pub raw_events: String,
    pub spillover: Option<String>,
    pub comp_matrix: Option<String>,
    pub transform: String,
    pub gate_polygons: BTreeMap<String, String>,
    pub gate_tree: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DenominatorRef {
    pub parent_population: String,
    pub parent_snapshot_id: Option<String>,
    pub parent_count: i64,
    pub pinned_versions: PinnedVersions,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PopStatus {
    Current,
    Stale,
    Blocked,
    PendingCandidate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PopSnapshot {
    pub snapshot_id: String,
    pub population_id: String,
    pub gate_id: String,
    pub name: String,
    pub parent: Option<String>,
    pub seq: i64,
    pub status: PopStatus,
    pub count: Option<i64>,
    pub percent_of_parent: Option<f64>,
    pub denominator: Option<DenominatorRef>,
    pub included_events: Vec<String>,
    pub boundary_events: Vec<String>,
    pub stale_reason: Option<String>,
    pub old_snapshot_id: Option<String>,
    pub pinned_versions: PinnedVersions,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateOption {
    pub id: String,
    pub label: String,
    pub detail: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub candidate_id: String,
    pub subject: String,
    pub kind: String,
    pub seq: i64,
    pub question: String,
    pub options: Vec<CandidateOption>,
    pub accepted: Option<String>,
    pub superseded: bool,
    pub rationale: String,
    pub basis: serde_json::Value,
}

#[derive(Clone, Debug, Default, Debug)]
pub struct HeadState {
    pub materials: BTreeMap<String, MaterialTrack>,
    pub gates: BTreeMap<String, GateTrack>,
    pub populations: BTreeMap<String, PopSnapshot>,
    pub candidates: BTreeMap<String, Candidate>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct MaterialTrack {
    pub kind: String,
    pub versions: Vec<(i64, String, ArtifactContent)>,
}

#[derive(Clone, Debug, Default)]
pub struct GateTrack {
    pub polygon_versions: Vec<(i64, String, GatePolygon)>,
    pub tree: Option<(i64, String, GateTree)>,
}

impl HeadState {
    pub fn latest(&self, material_id: &str) -> Option<&(i64, String, ArtifactContent)> {
        self.materials.get(material_id).and_then(|m| m.versions.last())
    }
}

pub fn find_material_by_kind<'a>(state: &'a HeadState, kind: &str) -> Option<&'a MaterialTrack> {
    state
        .materials
        .values()
        .find(|m| m.kind == kind && !m.versions.is_empty())
}

pub fn material_version_ids(state: &HeadState) -> (String, Option<String>, Option<String>, String, String) {
    let raw = find_material_by_kind(state, "raw_events")
        .and_then(|m| m.versions.last())
        .map(|(_, vid, _)| vid.clone())
        .unwrap_or_else(|| "missing".into());
    let spill = find_material_by_kind(state, "spillover")
        .and_then(|m| m.versions.last())
        .map(|(_, vid, _)| vid.clone());
    let comp = find_material_by_kind(state, "comp_matrix")
        .and_then(|m| m.versions.last())
        .map(|(_, vid, _)| vid.clone());
    let transform = find_material_by_kind(state, "transform")
        .and_then(|m| m.versions.last())
        .map(|(_, vid, _)| vid.clone())
        .unwrap_or_else(|| "missing".into());
    let tree = find_material_by_kind(state, "gate_tree")
        .and_then(|m| m.versions.last())
        .map(|(_, vid, _)| vid.clone())
        .unwrap_or_else(|| "missing".into());
    (raw, spill, comp, transform, tree)
}

/// Extract per-channel raw signals (numeric parse failures retained as flags).
fn event_channels(fcs: &FcsFile) -> Vec<String> {
    fcs.channels.iter().map(|c| c.id.clone()).collect()
}

fn compensated_space(gate: &GatePolygon) -> bool {
    gate.space == "compensated"
}

fn transform_set<'a>(state: &'a HeadState) -> Option<&'a TransformSet> {
    match find_material_by_kind(state, "transform")?.versions.last()?.2 {
        ArtifactContent::Transform(t) => Some(t),
        _ => None,
    }
}

fn comp_matrix(state: &HeadState) -> Option<&CompMatrix> {
    match find_material_by_kind(state, "comp_matrix")?.versions.last()?.2 {
        ArtifactContent::CompMatrix(c) => Some(c),
        _ => None,
    }
}

fn fcs(state: &HeadState) -> Option<&FcsFile> {
    match find_material_by_kind(state, "raw_events")?.versions.last()?.2 {
        ArtifactContent::RawEvents(f) => Some(f),
        _ => None,
    }
}

fn gate_tree(state: &HeadState) -> Option<&GateTree> {
    match find_material_by_kind(state, "gate_tree")?.versions.last()?.2 {
        ArtifactContent::GateTree(t) => Some(t),
        _ => None,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StaleInfo {
    pub reason: String,
    pub old_snapshot_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct StaleMap(pub BTreeMap<String, StaleInfo>);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PerEventPoint {
    pub event_id: String,
    pub raw: BTreeMap<String, f64>,
    pub compensated: Option<BTreeMap<String, f64>>,
    pub transformed: BTreeMap<String, f64>,
    pub flags: BTreeMap<String, Vec<String>>,
    pub diagnostics: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoundResult {
    pub snapshots: Vec<PopSnapshot>,
    pub new_candidates: Vec<Candidate>,
    pub points: Vec<PerEventPoint>,
    pub notes: Vec<String>,
}

fn comp_decision(state: &HeadState, seq: i64) -> Option<(String, Candidate)> {
    state
        .candidates
        .values()
        .filter(|c| c.kind == "near_singular_comp" && !c.superseded)
        .max_by_key(|c| c.seq)
        .map(|c| (c.accepted.clone().unwrap_or_default(), c.clone()))
}

fn boundary_decision(state: &HeadState, gate_id: &str) -> Option<(String, Candidate)> {
    let subject = format!("gate:{gate_id}:boundary");
    state
        .candidates
        .values()
        .find(|c| c.kind == "gate_boundary" && c.subject == subject && !c.superseded)
        .map(|c| (c.accepted.clone().unwrap_or_default(), c.clone()))
}

/// One full evaluation round at event `seq`. Gates listed in `stale` are not
/// recomputed: their existing snapshots survive and the caller appends stale
/// markers instead.
pub fn evaluate_round(
    seq: i64,
    state: &HeadState,
    stale: &StaleMap,
    branch: &str,
) -> RoundResult {
    let mut notes = Vec::new();
    let mut new_candidates = Vec::new();
    let fcs = match fcs(state) {
        Some(f) => f,
        None => {
            return RoundResult { snapshots: vec![], new_candidates: vec![], points: vec![], notes };
        }
    };
    let channels = event_channels(fcs);
    let tree = gate_tree(state);
    let transforms = transform_set(state);
    let matrix = comp_matrix(state);

    // Near-singular analysis + candidate emission.
    let mut comp_blocked = false;
    let mut comp_pending = false;
    let mut comp_forced = false;
    if let Some(matrix) = matrix {
        let report = crate::engine::compensation::analyze(matrix);
        if report.near_singular {
            let subject = "comp_matrix:near_singular".to_string();
            let existing = state.candidates.values().any(|c| {
                c.kind == "near_singular_comp"
                    && c.subject == subject
                    && !c.superseded
                    && c.basis.get("version_id").and_then(|v| v.as_str())
                        == state
                            .materials
                            .values()
                            .find(|m| m.kind == "comp_matrix")
                            .and_then(|m| m.versions.last())
                            .map(|(_, v, _)| v.as_str())
            });
            if !existing {
                let cid = crate::engine::ids::candidate_id(branch, seq, &subject);
                new_candidates.push(Candidate {
                    candidate_id: cid,
                    subject,
                    kind: "near_singular_comp".into(),
                    seq,
                    question:
                        "Compensation matrix is near-singular; it must not be inverted silently."
                            .into(),
                    options: vec![
                        CandidateOption {
                            id: "reject".into(),
                            label: "Reject matrix: block compensated gates".into(),
                            detail: serde_json::json!({
                                "condition_number": report.condition_number,
                                "determinant": report.determinant,
                                "threshold": report.threshold,
                            }),
                        },
                        CandidateOption {
                            id: "accept_warning".into(),
                            label: "Accept with instability warning (manual override)".into(),
                            detail: serde_json::json!({
                                "condition_number": report.condition_number,
                                "determinant": report.determinant,
                            }),
                        },
                        CandidateOption {
                            id: "remeasure".into(),
                            label: "Re-measure single-stain controls".into(),
                            detail: serde_json::json!({"requires": "new_spillover_version"}),
                        },
                    ],
                    accepted: None,
                    superseded: false,
                    rationale: String::new(),
                    basis: serde_json::json!({
                        "version_id": state
                            .materials
                            .values()
                            .find(|m| m.kind == "comp_matrix")
                            .and_then(|m| m.versions.last())
                            .map(|(_, v, _)| v.clone())
                            .unwrap_or_default(),
                        "condition_number": report.condition_number,
                        "determinant": report.determinant,
                        "threshold": report.threshold,
                    }),
                });
            }
            match comp_decision(state, seq) {
                Some((choice, _)) if choice == "accept_warning" => comp_forced = true,
                Some((choice, _)) if choice == "reject" => comp_blocked = true,
                _ => comp_pending = true,
            }
            notes.push(format!(
                "near-singular matrix: cond={:.3e} det={:.3e} blocked={comp_blocked} pending={comp_pending} forced={comp_forced}",
                report.condition_number, report.determinant
            ));
        }
    }

    // Per-event raw -> compensated -> transformed points with diagnostics.
    let mut points: Vec<PerEventPoint> = Vec::new();
    let spec_map: BTreeMap<&str, &ChannelSpec> =
        fcs.channels.iter().map(|c| (c.id.as_str(), c)).collect();
    for ev in &fcs.events {
        let mut raw = BTreeMap::new();
        let mut flags = BTreeMap::new();
        let mut diagnostics = BTreeMap::new();
        let mut order: BTreeMap<&str, f64> = BTreeMap::new();
        let mut parsed_order: Vec<f64> = Vec::new();
        for ch in &channels {
            let rv = ev.values.iter().find(|v| &v.channel == ch);
            let value = rv.and_then(|v| v.value);
            if let Some(value) = value {
                raw.insert(ch.clone(), value);
                order.insert(ch.as_str(), value);
                parsed_order.push(value);
            }
            if let Some(rv) = rv {
                if !rv.flags.is_empty() {
                    flags.insert(ch.clone(), rv.flags.clone());
                }
                if let Some(d) = &rv.diagnostic {
                    diagnostics.insert(ch.clone(), d.clone());
                }
                if rv.value.is_none() {
                    diagnostics
                        .insert(ch.clone(), format!("unconvertible raw `{}`: kept in place", rv.raw_text));
                }
            }
        }
        // detector-level extreme diagnostics against declared channel bounds
        for ch in &channels {
            if let (Some(value), Some(spec)) = (raw.get(ch), spec_map.get(ch.as_str())) {
                let raw_text = ev
                    .values
                    .iter()
                    .find(|v| &v.channel == ch)
                    .map(|v| v.raw_text.as_str())
                    .unwrap_or("");
                let (ef, ed) =
                    crate::engine::transform::extreme_flags(*value, raw_text, spec.raw_max, spec.raw_min);
                if !ef.is_empty() {
                    flags.entry(ch.clone()).or_default().extend(ef);
                }
                if let Some(d) = ed {
                    diagnostics.insert(ch.clone(), d);
                }
            }
        }

        let mut compensated: Option<BTreeMap<String, f64>> = None;
        if let Some(matrix) = matrix {
            let report = crate::engine::compensation::analyze(matrix);
            let usable = !report.near_singular || comp_forced;
            if usable {
                let vec: Vec<f64> = matrix
                    .order
                    .iter()
                    .map(|c| order.get(c.as_str()).copied().unwrap_or(0.0))
                    .collect();
                let out = if report.near_singular {
                    // accepted override: still compute, but flag instability
                    let n = matrix.order.len();
                    matrix
                        .matrix
                        .iter()
                        .take(n)
                        .map(|row| row.iter().take(n).zip(&vec).map(|(a, x)| a * x).sum())
                        .collect::<Vec<f64>>()
                } else {
                    crate::engine::compensation::apply(matrix, &vec)
                        .unwrap_or_default()
                };
                let map: BTreeMap<String, f64> =
                    matrix.order.iter().cloned().zip(out).collect();
                compensated = Some(map.clone());
            }
        } else {
            // no matrix: identity compensation (raw passthrough)
            compensated = Some(raw.clone());
        }

        let mut transformed = BTreeMap::new();
        if let Some(set) = transforms {
            for p in &set.params {
                let source = if p.basis == "compensated" {
                    compensated.as_ref().and_then(|m| m.get(&p.channel))
                } else {
                    raw.get(&p.channel)
                };
                if let Some(value) = source {
                    transformed.insert(p.channel.clone(), crate::engine::transform::apply_param(p, *value));
                }
            }
        } else {
            for (k, v) in &raw {
                transformed.insert(k.clone(), *v);
            }
        }

        points.push(PerEventPoint {
            event_id: ev.event_id.clone(),
            raw,
            compensated,
            transformed,
            flags,
            diagnostics,
        });
    }

    // Order gates by lineage depth (root-hanging first).
    let mut ordered_gate_ids: Vec<String> = Vec::new();
    if let Some(tree) = tree {
        let parents: BTreeMap<String, Option<String>> = tree
            .edges
            .iter()
            .map(|e| (e.gate_id.clone(), e.parent.clone()))
            .collect();
        let mut remaining: Vec<String> = tree.nodes.iter().map(|n| n.gate_id.clone()).collect();
        let mut guard = 0;
        while !remaining.is_empty() && guard < 1000 {
            guard += 1;
            let ready: Vec<String> = remaining
                .iter()
                .filter(|g| {
                    parents
                        .get(*g)
                        .map(|p| match p {
                            Some(par) => ordered_gate_ids.contains(par),
                            None => true,
                        })
                        .unwrap_or(true)
                })
                .cloned()
                .collect();
            if ready.is_empty() {
                ordered_gate_ids.extend(remaining.drain(..));
                break;
            }
            for g in ready {
                ordered_gate_ids.push(g.clone());
                remaining.retain(|r| r != &g);
            }
        }
    }

    let (raw_vid, spill_vid, comp_vid, trans_vid, tree_vid) = material_version_ids(state);
    let mut snapshots = Vec::new();
    // membership pools per gate and for root
    let mut pools: BTreeMap<String, Vec<String>> = BTreeMap::new();
    pools.insert("ROOT".into(), fcs.events.iter().map(|e| e.event_id.clone()).collect());
    let mut snapshot_by_gate: BTreeMap<String, PopSnapshot> = BTreeMap::new();

    for gate_id in &ordered_gate_ids {
        let track = state.gates.get(gate_id);
        let polygon = track.and_then(|t| t.polygon_versions.last()).map(|(_, _, p)| p.clone());
        let name = track
            .and_then(|t| t.polygon_versions.last())
            .map(|(_, _, p)| p.name.clone())
            .or_else(|| {
                tree.and_then(|tr| tr.nodes.iter().find(|n| &n.gate_id == gate_id))
                    .map(|n| n.name.clone())
            })
            .unwrap_or_else(|| gate_id.clone());
        let parent = tree
            .and_then(|tr| tr.edges.iter().find(|e| &e.gate_id == gate_id))
            .and_then(|e| e.parent.clone());

        let polygon_versions: BTreeMap<String, String> = state
            .gates
            .iter()
            .filter_map(|(gid, t)| t.polygon_versions.last().map(|(_, v, _)| (gid.clone(), v.clone())))
            .collect();

        let pinned = PinnedVersions {
            raw_events: raw_vid.clone(),
            spillover: spill_vid.clone(),
            comp_matrix: comp_vid.clone(),
            transform: trans_vid.clone(),
            gate_polygons: polygon_versions.clone(),
            gate_tree: tree_vid.clone(),
        };

        let mut make_snapshot = |status: PopStatus,
                                 count: Option<i64>,
                                 included: Vec<String>,
                                 boundary: Vec<String>,
                                 stale_reason: Option<String>,
                                 old_snapshot_id: Option<String>,
                                 denom: Option<DenominatorRef>,
                                 pct: Option<f64>| {
            let snapshot_id = format!(
                "snap-{:016x}",
                crate::engine::ids::fnv1a_64(
                    format!("{branch}|{seq}|{gate_id}|{:?}|{:?}", status, count).as_bytes()
                )
            );
            PopSnapshot {
                snapshot_id,
                population_id: format!("pop-{gate_id}@{seq}"),
                gate_id: gate_id.clone(),
                name: name.clone(),
                parent: parent.clone(),
                seq,
                status,
                count,
                percent_of_parent: pct,
                denominator: denom,
                included_events: included,
                boundary_events: boundary,
                stale_reason,
                old_snapshot_id,
                pinned_versions: pinned.clone(),
            }
        };

        // Stale gate: do not recompute; the prior snapshot survives untouched.
        if let Some(info) = stale.0.get(gate_id) {
            let old = state.populations.get(gate_id).cloned();
            pools.insert(gate_id.clone(), old.iter().flat_map(|s| s.included_events.clone()).collect());
            let snap = make_snapshot(
                PopStatus::Stale,
                old.as_ref().and_then(|s| s.count),
                old.iter().flat_map(|s| s.included_events.clone()).collect(),
                old.iter().flat_map(|s| s.boundary_events.clone()).collect(),
                Some(info.reason.clone()),
                Some(info.old_snapshot_id.clone()),
                old.as_ref().and_then(|s| s.denominator.clone()),
                old.as_ref().and_then(|s| s.percent_of_parent),
            );
            snapshot_by_gate.insert(gate_id.clone(), snap.clone());
            snapshots.push(snap);
            continue;
        }

        let polygon = match polygon {
            Some(p) => p,
            None => {
                let snap = make_snapshot(PopStatus::Blocked, None, vec![], vec![],
                    Some("gate polygon missing".into()), None, None, None);
                snapshot_by_gate.insert(gate_id.clone(), snap.clone());
                snapshots.push(snap);
                continue;
            }
        };

        // Compensated gate with unusable matrix.
        if compensated_space(&polygon) && comp_pending {
            let snap = make_snapshot(PopStatus::PendingCandidate, None, vec![], vec![],
                Some("compensation matrix near-singular; candidate unresolved".into()), None, None, None);
            snapshot_by_gate.insert(gate_id.clone(), snap.clone());
            snapshots.push(snap);
            continue;
        }
        if compensated_space(&polygon) && comp_blocked {
            let old = state.populations.get(gate_id).cloned();
            let snap = make_snapshot(PopStatus::Blocked, None, vec![], vec![],
                Some("compensation matrix rejected (near-singular)".into()),
                old.as_ref().map(|s| s.snapshot_id.clone()), None, None);
            snapshot_by_gate.insert(gate_id.clone(), snap.clone());
            snapshots.push(snap);
            continue;
        }

        // Candidate pool: events in the parent gate (or all events at root).
        let parent_pool: Vec<String> = match &parent {
            Some(p) => pools.get(p).cloned().unwrap_or_default(),
            None => pools.get("ROOT").cloned().unwrap_or_default(),
        };

        let use_comp = compensated_space(&polygon);
        let point_pairs: Vec<(String, f64, f64)> = points
            .iter()
            .filter(|pt| parent_pool.contains(&pt.event_id))
            .map(|pt| {
                let source = if use_comp {
                    pt.compensated.as_ref()
                } else {
                    None
                };
                let map = source.map_or(&pt.raw, |m| m);
                let x = pt
                    .transformed
                    .get(&polygon.x_channel)
                    .copied()
                    .unwrap_or_else(|| *map.get(&polygon.x_channel).unwrap_or(&f64::NAN));
                let y = pt
                    .transformed
                    .get(&polygon.y_channel)
                    .copied()
                    .unwrap_or_else(|| *map.get(&polygon.y_channel).unwrap_or(&f64::NAN));
                (pt.event_id.clone(), x, y)
            })
            .collect();

        let (inc, exc, boundary_ids) =
            crate::engine::polygon::count_points(&point_pairs, &polygon.vertices, &polygon.boundary, 1e-7);

        let convention = match polygon.boundary.as_str() {
            "include" => Some(crate::engine::polygon::BoundaryConvention::Include),
            "exclude" => Some(crate::engine::polygon::BoundaryConvention::Exclude),
            _ => match boundary_decision(state, gate_id) {
                Some((choice, cand)) if choice == "include" => {
                    Some(crate::engine::polygon::BoundaryConvention::Include)
                }
                Some((choice, _)) if choice == "exclude" => {
                    Some(crate::engine::polygon::BoundaryConvention::Exclude)
                }
                _ => None,
            }
        };

        if polygon.boundary == "unspecified" && !boundary_ids.is_empty() && convention.is_none() {
            let subject = format!("gate:{gate_id}:boundary");
            let existing = state.candidates.values().any(|c| {
                c.kind == "gate_boundary"
                    && c.subject == subject
                    && !c.superseded
                    && c.basis
                        .get("polygon_version")
                        .and_then(|v| v.as_str())
                        == polygon_versions.get(gate_id).map(String::as_str)
            });
            if !existing {
                let cid = crate::engine::ids::candidate_id(branch, seq, &subject);
                new_candidates.push(Candidate {
                    candidate_id: cid,
                    subject,
                    kind: "gate_boundary".into(),
                    seq,
                    question: format!(
                        "{boundary_count} event(s) sit exactly on the boundary of gate `{gate_id}`; \
                         polygon does not fix the convention.",
                        boundary_count = boundary_ids.len()
                    ),
                    options: vec![
                        CandidateOption {
                            id: "include".into(),
                            label: "Include boundary points".into(),
                            detail: serde_json::json!({"count": inc}),
                        },
                        CandidateOption {
                            id: "exclude".into(),
                            label: "Exclude boundary points".into(),
                            detail: serde_json::json!({"count": exc}),
                        },
                    ],
                    accepted: None,
                    superseded: false,
                    rationale: String::new(),
                    basis: serde_json::json!({
                        "polygon_version": polygon_versions.get(gate_id).cloned().unwrap_or_default(),
                        "boundary_events": boundary_ids,
                        "include_count": inc,
                        "exclude_count": exc,
                    }),
                });
            }
            let snap = make_snapshot(PopStatus::PendingCandidate, None, vec![], boundary_ids,
                Some("boundary convention unresolved; both legal counts retained".into()), None, None, None);
            snapshot_by_gate.insert(gate_id.clone(), snap.clone());
            snapshots.push(snap);
            continue;
        }

        let convention = convention.unwrap_or(crate::engine::polygon::BoundaryConvention::Include);
        let count = match convention {
            crate::engine::polygon::BoundaryConvention::Include => inc,
            crate::engine::polygon::BoundaryConvention::Exclude => exc,
        };
        let included: Vec<String> = point_pairs
            .iter()
            .filter(|(_, x, y)| {
                crate::engine::polygon::inside(
                    *x,
                    *y,
                    &polygon.vertices,
                    convention,
                    1e-7,
                )
            })
            .map(|(id, _, _)| id.clone())
            .collect();
        pools.insert(gate_id.clone(), included.clone());

        // Denominator pinned to exact parent snapshot version.
        // Denominator pinned to the parent snapshot produced in THIS round
        // (which itself carries pinned material versions), or the root pool.
        let parent_source = parent.as_ref().and_then(|p| snapshot_by_gate.get(p));
        let parent_count = match parent_source {
            Some(s) => s.count,
            None => Some(fcs.events.len() as i64),
        };
        let denom = match parent_source {
            Some(s) => Some(DenominatorRef {
                parent_population: parent.clone().unwrap_or_else(|| "ROOT".into()),
                parent_snapshot_id: Some(s.snapshot_id.clone()),
                parent_count: s.count.unwrap_or_default(),
                pinned_versions: s.pinned_versions.clone(),
            }),
            None => Some(DenominatorRef {
                parent_population: "ROOT".into(),
                parent_snapshot_id: None,
                parent_count: fcs.events.len() as i64,
                pinned_versions: pinned.clone(),
            }),
        };
        let pct = parent_count.map(|d| if d == 0 { 0.0 } else { count as f64 * 100.0 / d as f64 });
        let snap = make_snapshot(PopStatus::Current, Some(count), included, boundary_ids,
            None, None, denom, pct);
        snapshot_by_gate.insert(gate_id.clone(), snap.clone());
        snapshots.push(snap);
    }

    RoundResult { snapshots, new_candidates, points, notes }
}
