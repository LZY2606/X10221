//! Deterministic derivation layer.
//!
//! Everything here is a pure function of pinned artifact versions: importing
//! never rewrites data, and a later parameter/reference change only produces a
//! *new* derived snapshot. Old snapshots stay readable and are marked stale
//! relative to newer inputs rather than being overwritten.

use crate::model::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SINGULAR_MIN_PIVOT: f64 = 0.05;

// ---------- matrix analysis ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixAnalysis {
    pub n: usize,
    pub determinant: f64,
    pub min_pivot: f64,
    pub condition_estimate: f64,
    pub near_singular: bool,
    pub rule: String,
    pub pivots: Vec<f64>,
}

/// Gaussian elimination with partial pivoting on a copy of the matrix.
/// Returns analysis plus the permuted, eliminated matrix is not needed.
pub fn analyze_spillover(matrix: &[Vec<f64>]) -> Result<MatrixAnalysis, String> {
    let n = matrix.len();
    if n == 0 || matrix.iter().any(|r| r.len() != n) {
        return Err("spillover matrix must be square and non-empty".into());
    }
    let mut a = matrix.to_vec();
    let mut pivots = Vec::with_capacity(n);
    let mut det = 1.0f64;
    let mut swaps = 0usize;
    for col in 0..n {
        let mut best = col;
        for row in (col + 1)..n {
            if a[row][col].abs() > a[best][col].abs() {
                best = row;
            }
        }
        if best != col {
            a.swap(best, col);
            swaps += 1;
        }
        let pivot = a[col][col];
        pivots.push(pivot);
        det *= pivot;
        if pivot.abs() < 1e-15 {
            break;
        }
        for row in (col + 1)..n {
            let factor = a[row][col] / pivot;
            for k in col..n {
                a[row][k] -= factor * a[col][k];
            }
            a[row][col] = factor;
        }
    }
    if swaps % 2 == 1 {
        det = -det;
    }
    let min_pivot = pivots.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_pivot = pivots
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    let condition_estimate = if min_pivot.abs() < 1e-15 {
        f64::INFINITY
    } else {
        max_pivot.abs() / min_pivot.abs()
    };
    let near_singular = min_pivot.abs() < SINGULAR_MIN_PIVOT || det.abs() < 0.1;
    Ok(MatrixAnalysis {
        n,
        determinant: det,
        min_pivot,
        condition_estimate,
        near_singular,
        rule: format!(
            "partial-pivot gaussian elimination; reject if |min pivot| < {} or |det| < 0.1 (unit-diagonal 3x3)",
            SINGULAR_MIN_PIVOT
        ),
        pivots,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InverseCandidate {
    pub id: String,
    pub method: String,
    pub matrix: Vec<Vec<f64>>,
    pub lambda: Option<f64>,
    pub note: String,
}

fn identity(n: usize) -> Vec<Vec<f64>> {
    (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect()
}

fn mat_mul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut out = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += a[i][k] * b[k][j];
            }
            out[i][j] = s;
        }
    }
    out
}

fn mat_add(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    a.iter()
        .zip(b)
        .map(|(ra, rb)| ra.iter().zip(rb).map(|(x, y)| x + y).collect())
        .collect()
}

fn invert_gauss(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut a = m.to_vec();
    let mut inv = identity(n);
    for col in 0..n {
        let mut best = col;
        for row in (col + 1)..n {
            if a[row][col].abs() > a[best][col].abs() {
                best = row;
            }
        }
        if a[best][col].abs() < 1e-12 {
            return None;
        }
        a.swap(best, col);
        inv.swap(best, col);
        let pivot = a[col][col];
        for k in 0..n {
            a[col][k] /= pivot;
            inv[col][k] /= pivot;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            for k in 0..n {
                a[row][k] -= factor * a[col][k];
                inv[row][k] -= factor * inv[col][k];
            }
        }
    }
    Some(inv)
}

/// For a near-singular S, the unique inverse cannot be trusted. We retain all
/// legal, explicitly defined candidates: the formal inverse (flagged) plus two
/// ridge-regularised inverses with documented lambda. Nothing here is silently
/// chosen as "the" compensation.
pub fn inverse_candidates(sp: &SpilloverDoc, analysis: &MatrixAnalysis) -> Vec<InverseCandidate> {
    let n = sp.matrix.len();
    let id = identity(n);
    let mut out = Vec::new();
    if !analysis.near_singular {
        if let Some(inv) = invert_gauss(&sp.matrix) {
            out.push(InverseCandidate {
                id: "cand_exact_gauss".into(),
                method: "partial-pivot gaussian inverse".into(),
                matrix: inv,
                lambda: None,
                note: "well-conditioned spillover; unique inverse accepted by rule".into(),
            });
        }
        return out;
    }
    if let Some(inv) = invert_gauss(&sp.matrix) {
        out.push(InverseCandidate {
            id: "cand_formal_inverse_flagged".into(),
            method: "formal gaussian inverse (unstable, retained for audit)".into(),
            matrix: inv,
            lambda: None,
            note: format!(
                "min pivot {:.5}, det {:.5}; values amplify noise and must not be used silently",
                analysis.min_pivot, analysis.determinant
            ),
        });
    }
    for (lid, lambda) in [("cand_ridge_1e2", 1e-2), ("cand_ridge_1e3", 1e-3)] {
        // (S^T S + lambda I)^-1 S^T
        let sts = mat_mul(&transpose(&sp.matrix), &sp.matrix);
        let lambda_i = id
            .iter()
            .map(|r| r.iter().map(|x| x * lambda).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let reg = mat_add(&sts, &lambda_i);
        if let Some(inv_reg) = invert_gauss(&reg) {
            let ridge = mat_mul(&inv_reg, &transpose(&sp.matrix));
            out.push(InverseCandidate {
                id: lid.into(),
                method: "ridge-regularised inverse (S^T S + lambda I)^-1 S^T".into(),
                matrix: ridge,
                lambda: Some(lambda),
                note: "one of several legal regularisations; human adjudication required".into(),
            });
        }
    }
    out
}

fn transpose(m: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = m.len();
    (0..n)
        .map(|j| (0..n).map(|i| m[i][j]).collect())
        .collect()
}

// ---------- compensation / transforms ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelValue {
    pub repr: String,
    pub numeric: Option<f64>,
    pub unit: String,
    pub convertible: bool,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensatedEvent {
    pub id: String,
    pub raw: BTreeMap<String, f64>,
    pub compensated: BTreeMap<String, f64>,
    pub transformed: BTreeMap<String, f64>,
    pub flags: BTreeSet<String>,
    pub diagnostics: BTreeMap<String, String>,
}

pub fn asinh_normalized(x: f64, t: f64) -> f64 {
    let z = x / t;
    (z + (z * z + 1.0).sqrt()).ln() / (1.0_f64 + 2.0_f64.sqrt()).ln()
}

pub const EXTREME_COMP_MAGNITUDE: f64 = 4096.0;

pub fn derive_events(
    events_doc: &EventsDoc,
    sp: &SpilloverDoc,
    transform_doc: &TransformDoc,
    analysis: &MatrixAnalysis,
) -> Vec<CompensatedEvent> {
    let n = sp.channels.len();
    let inv = if !analysis.near_singular {
        invert_gauss(&sp.matrix)
    } else {
        None
    };
    events_doc
        .events
        .iter()
        .map(|e| {
            let mut raw = BTreeMap::new();
            let mut diagnostics = BTreeMap::new();
            let mut flags = BTreeSet::new();
            for (ch, val) in &e.values {
                if let Some(num) = val.numeric {
                    raw.insert(ch.clone(), num);
                } else {
                    flags.insert(format!("unconvertible:{ch}"));
                    diagnostics.insert(ch.clone(), val.diagnostic.clone().unwrap_or_default());
                }
            }
            let mut compensated: BTreeMap<String, f64> = BTreeMap::new();
            let mut transformed: BTreeMap<String, f64> = BTreeMap::new();
            match &inv {
                Some(inv) => {
                    for (i, ch) in sp.channels.iter().enumerate() {
                        let v: f64 = (0..n)
                            .map(|j| inv[i][j] * raw.get(&sp.channels[j]).copied().unwrap_or(0.0))
                            .sum();
                        compensated.insert(ch.clone(), v);
                        let tp = transform_doc.params.get(ch);
                        let t = tp.map(|p| p.t).unwrap_or(1000.0);
                        transformed.insert(ch.clone(), asinh_normalized(v, t));
                        if v.abs() > EXTREME_COMP_MAGNITUDE {
                            flags.insert(format!("extreme_compensated:{ch}"));
                        }
                        if v < 0.0 {
                            flags.insert(format!("negative_compensated:{ch}"));
                        }
                    }
                }
                None => {
                    flags.insert("compensation_blocked_near_singular".into());
                    for ch in &sp.channels {
                        if let Some(v) = raw.get(ch) {
                            compensated.insert(ch.clone(), *v);
                            let tp = transform_doc.params.get(ch);
                            let t = tp.map(|p| p.t).unwrap_or(1000.0);
                            transformed.insert(ch.clone(), asinh_normalized(*v, t));
                        }
                    }
                }
            }
            for (ch, val) in &e.values {
                if !val.convertible {
                    diagnostics.insert(ch.clone(), val.diagnostic.clone().unwrap_or_default());
                }
            }
            CompensatedEvent {
                id: e.id.clone(),
                raw,
                compensated,
                transformed,
                flags,
                diagnostics,
            }
        })
        .collect()
}

// ---------- gating ----------

fn point_coords(g: &GateDef, ev: &CompensatedEvent) -> Option<(f64, f64)> {
    let map = match g.space {
        GateSpace::Raw => &ev.raw,
        GateSpace::Compensated => &ev.compensated,
        GateSpace::Transformed => &ev.transformed,
    };
    Some((*map.get(&g.x_channel)?, *map.get(&g.y_channel)?))
}

/// Half-open polygon containment: points strictly inside are included, a
/// point exactly on an edge is included, and the rule is documented on the
/// gate itself (`boundary_rule`).
pub fn polygon_contains(vertices: &[[f64; 2]], px: f64, py: f64) -> bool {
    let n = vertices.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (vertices[i][0], vertices[i][1]);
        let (xj, yj) = (vertices[j][0], vertices[j][1]);
        let cross = (px - xi) * (yj - yi) - (py - yi) * (xj - xi);
        if cross == 0.0
            && px <= xi.max(xj)
            && px >= xi.min(xj)
            && py <= yi.max(yj)
            && py >= yi.min(yj)
        {
            return true; // exactly on an edge
        }
        if (yi > py) != (yj > py) {
            let x_hit = xi + (py - yi) * (xj - xi) / (yj - yi);
            if x_hit > px {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Membership {
    Included,
    Excluded,
    BoundaryIncluded,
    ExcludedUnconvertible,
    BlockedNearSingular,
}

pub fn gate_membership(g: &GateDef, ev: &CompensatedEvent) -> Membership {
    if ev.flags.contains("compensation_blocked_near_singular")
        && g.space == GateSpace::Compensated
    {
        return Membership::BlockedNearSingular;
    }
    let Some((x, y)) = point_coords(g, ev) else {
        return Membership::ExcludedUnconvertible;
    };
    let verts: Vec<[f64; 2]> = g.vertices.clone();
    let on_edge = verts.iter().any(|v| {
        // exact vertex
        v[0] == x && v[1] == y
    }) || {
        let n = verts.len();
        let mut e = false;
        let mut j = n - 1;
        for i in 0..n {
            let (a, b, c, d) = (verts[i][0], verts[i][1], verts[j][0], verts[j][1]);
            let cross = (x - a) * (d - b) - (y - b) * (c - a);
            if cross == 0.0
                && x <= a.max(c)
                && x >= a.min(c)
                && y <= b.max(d)
                && y >= b.min(d)
            {
                e = true;
            }
            j = i;
        }
        e
    };
    if polygon_contains(&verts, x, y) {
        if on_edge {
            Membership::BoundaryIncluded
        } else {
            Membership::Included
        }
    } else {
        Membership::Excluded
    }
}

// ---------- lineage / populations ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PopulationResult {
    pub gate_id: String,
    pub gate_label: String,
    pub parent_gate: Option<String>,
    pub included: Vec<String>,
    pub boundary_event_ids: Vec<String>,
    pub excluded_diagnostic_event_ids: Vec<String>,
    /// Denominator is pinned to the parent population *as it was computed
    /// against the pinned parent gate version*, never "latest".
    pub denominator_parent_gate_version: Option<u32>,
    pub denominator_event_ids: Vec<String>,
    pub numerator_count: u32,
    pub denominator_count: u32,
    pub percent: Option<f64>,
    pub status: String,
    pub dependency_versions: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedVersions {
    pub events: u32,
    pub spillover: u32,
    pub transform: u32,
    pub gates: u32,
    pub relations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedSnapshot {
    pub branch: String,
    pub as_of_seq: u32,
    pub pinned: PinnedVersions,
    pub matrix_analysis: MatrixAnalysis,
    pub inverse_candidates: Vec<InverseCandidate>,
    pub compensation_status: String,
    pub events: Vec<CompensatedEvent>,
    pub populations: Vec<PopulationResult>,
    pub gate_tree: Vec<TreeNode>,
    pub invalidated_scope: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub gate_id: String,
    pub label: String,
    pub parent: Option<String>,
    pub children: Vec<String>,
}

fn ordering(rel: &RelationsDoc) -> Vec<String> {
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &rel.edges {
        children.entry(e.parent.clone()).or_default().push(e.child.clone());
    }
    let mut order = Vec::new();
    fn walk(
        node: &str,
        children: &BTreeMap<String, Vec<String>>,
        order: &mut Vec<String>,
    ) {
        if node != "__root__" {
            order.push(node.to_string());
        }
        if let Some(kids) = children.get(node) {
            for k in kids {
                walk(k, children, order);
            }
        }
    }
    walk(&rel.root, &children, &mut order);
    order
}

pub fn gate_tree(gates: &GatesDoc, rel: &RelationsDoc) -> Vec<TreeNode> {
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut parent_of: BTreeMap<String, String> = BTreeMap::new();
    for e in &rel.edges {
        children
            .entry(e.parent.clone())
            .or_default()
            .push(e.child.clone());
        parent_of.insert(e.child.clone(), e.parent.clone());
    }
    let order = ordering(rel);
    order
        .iter()
        .map(|id| {
            let label = gates
                .gates
                .iter()
                .find(|g| &g.id == id)
                .map(|g| g.label.clone())
                .unwrap_or_else(|| id.clone());
            TreeNode {
                gate_id: id.clone(),
                label,
                parent: parent_of.get(id).filter(|p| p.as_str() != "__root__").cloned(),
                children: children.get(id).cloned().unwrap_or_default(),
            }
        })
        .collect()
}

fn pct(num: u32, den: u32) -> Option<f64> {
    if den == 0 {
        None
    } else {
        Some((num as f64) * 100.0 / (den as f64))
    }
}

pub fn compute_snapshot(
    branch: &str,
    as_of_seq: u32,
    pinned: &PinnedVersions,
    events_doc: &EventsDoc,
    sp: &SpilloverDoc,
    transform_doc: &TransformDoc,
    gates_doc: &GatesDoc,
    rel: &RelationsDoc,
) -> DerivedSnapshot {
    let analysis = analyze_spillover(&sp.matrix).expect("matrix doc validated at import");
    let candidates = inverse_candidates(sp, &analysis);
    let compensation_status = if analysis.near_singular {
        "blocked_pending_adjudication"
    } else {
        "accepted_unique_inverse"
    };
    let comp_events = derive_events(events_doc, sp, transform_doc, &analysis);

    let order = ordering(rel);
    let mut included_by_gate: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut populations = Vec::new();
    for gid in &order {
        let gate = match gates_doc.gates.iter().find(|g| &g.id == gid) {
            Some(g) => g,
            None => continue,
        };
        let parent = order
            .iter()
            .find(|p| rel.edges.iter().any(|e| &e.child == gid && &e.parent == p))
            .cloned();
        let parent_ids: Vec<String> = match &parent {
            None => comp_events
                .iter()
                .filter(|e| {
                    !e.values_any_unconvertible_blocking(&["FSC-A", "FSC-H", "SSC-A"])
                })
                .map(|e| e.id.clone())
                .collect(),
            Some(p) => included_by_gate.get(p).cloned().unwrap_or_default(),
        };
        let mut included = Vec::new();
        let mut boundary = Vec::new();
        let mut diag_excluded = Vec::new();
        for ev in &comp_events {
            if !parent_ids.contains(&ev.id) {
                continue;
            }
            let m = gate_membership(gate, ev);
            match m {
                Membership::Included => included.push(ev.id.clone()),
                Membership::BoundaryIncluded => {
                    boundary.push(ev.id.clone());
                    included.push(ev.id.clone());
                }
                Membership::ExcludedUnconvertible => diag_excluded.push(ev.id.clone()),
                Membership::BlockedNearSingular => {
                    diag_excluded.push(ev.id.clone());
                }
                Membership::Excluded => {}
            }
        }
        included.sort();
        included_by_gate.insert(gid.clone(), included.clone());
        let blocked = compensation_status == "blocked_pending_adjudication"
            && gate.space == GateSpace::Compensated;
        let status = if blocked {
            "stale_or_blocked"
        } else {
            "computed"
        };
        populations.push(PopulationResult {
            gate_id: gate.id.clone(),
            gate_label: gate.label.clone(),
            parent_gate: parent,
            numerator_count: included.len() as u32,
            denominator_count: parent_ids.len() as u32,
            percent: if blocked {
                None
            } else {
                pct(included.len() as u32, parent_ids.len() as u32)
            },
            denominator_parent_gate_version: None,
            denominator_event_ids: parent_ids,
            included,
            boundary_event_ids: boundary,
            excluded_diagnostic_event_ids: diag_excluded,
            status,
            dependency_versions: dep_versions(pinned),
        });
    }
    fill_denominator_versions(&mut populations, pinned, gates_doc);
    DerivedSnapshot {
        branch: branch.into(),
        as_of_seq,
        pinned: pinned.clone(),
        matrix_analysis: analysis,
        inverse_candidates: candidates,
        compensation_status: compensation_status.into(),
        events: comp_events,
        populations,
        gate_tree: gate_tree(gates_doc, rel),
        invalidated_scope: vec![],
    }
}

fn dep_versions(p: &PinnedVersions) -> BTreeMap<String, u32> {
    BTreeMap::from([
        ("events".into(), p.events),
        ("spillover".into(), p.spillover),
        ("transform".into(), p.transform),
        ("gates".into(), p.gates),
        ("relations".into(), p.relations),
    ])
}

fn fill_denominator_versions(
    pops: &mut [PopulationResult],
    pinned: &PinnedVersions,
    _gates: &GatesDoc,
) {
    for p in pops.iter_mut() {
        p.denominator_parent_gate_version = match &p.parent_gate {
            Some(_) => Some(pinned.gates),
            None => None,
        };
    }
}

impl CompensatedEvent {
    fn values_any_unconvertible_blocking(&self, chs: &[&str]) -> bool {
        chs.iter()
            .any(|c| self.flags.contains(&format!("unconvertible:{c}")))
    }
}

// ---------- invalidation scope ----------

/// Compute the set of objects whose derived result can differ when an artifact
/// version changes. Membership diff is used so an event-level correction only
/// invalidates gates that actually change, instead of globally replacing old
/// conclusions.
pub fn invalidation_scope(
    old: &DerivedSnapshot,
    new: &DerivedSnapshot,
    changed_kinds: &[ArtifactKind],
) -> Vec<String> {
    let mut scope: BTreeSet<String> = BTreeSet::new();
    for kind in changed_kinds {
        match kind {
            ArtifactKind::Spillover => {
                scope.insert("compensation".into());
                for p in &new.populations {
                    let g = p.gate_id.clone();
                    scope.insert(format!("population:{g}"));
                }
            }
            ArtifactKind::Transform => {
                scope.insert("display_transform".into());
                for p in &new.populations {
                    scope.insert(format!("population:{}", p.gate_id));
                }
            }
            ArtifactKind::Relations | ArtifactKind::Gates => {
                for p in &new.populations {
                    scope.insert(format!("population:{}", p.gate_id));
                }
            }
            ArtifactKind::Events => {}
        }
    }
    let old_by: BTreeMap<&str, &PopulationResult> =
        old.populations.iter().map(|p| (p.gate_id.as_str(), p)).collect();
    for p in &new.populations {
        if let Some(prev) = old_by.get(p.gate_id.as_str()) {
            let a: BTreeSet<&str> = prev.included.iter().map(|s| s.as_str()).collect();
            let b: BTreeSet<&str> = p.included.iter().map(|s| s.as_str()).collect();
            if a != b || prev.percent != p.percent {
                scope.insert(format!("population:{}", p.gate_id));
            }
        }
    }
    scope.into_iter().collect()
}

/// Gate ids downstream (inclusive) of `gate` in the lineage.
pub fn downstream_gates(rel: &RelationsDoc, gate: &str) -> Vec<String> {
    let mut out = vec![gate.to_string()];
    let mut stack = vec![gate.to_string()];
    while let Some(g) = stack.pop() {
        for e in &rel.edges {
            if e.parent == g && !out.contains(&e.child) {
                out.push(e.child.clone());
                stack.push(e.child.clone());
            }
        }
    }
    out
}
