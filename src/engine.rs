//! Derivation engine: compensation, transform, gating and event replay.
//!
//! The engine never mutates history: revising an upstream artifact marks
//! derived results stale and produces a new result on demand, leaving the
//! previous result (and the event it was pinned to) readable.

use crate::domain::*;
use std::collections::{BTreeMap, BTreeSet};

pub const SINGULARITY_CONDITION_THRESHOLD: f64 = 30.0;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Diag {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CompensationCandidate {
    pub kind: String,
    pub usable: bool,
    pub condition_number: Option<f64>,
    pub note: String,
}

/// Result of attempting to compensate with a matrix version.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CompensationAssessment {
    pub matrix_id: String,
    pub labels: Vec<String>,
    pub candidates: Vec<CompensationCandidate>,
    /// Inverse coefficients are only populated for the `refuse` case when the
    /// matrix is safely invertible. A near-singular matrix never gets an
    /// implicit inverse.
    pub inverse: Option<Vec<Vec<f64>>>,
    pub singular: bool,
    pub near_singular: bool,
    pub condition_number: Option<f64>,
    pub diagnostics: Vec<Diag>,
}

// --------------------------------------------------------------------------
// Linear algebra (small dense matrices)
// --------------------------------------------------------------------------

pub fn det(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    if n == 0 {
        return 0.0;
    }
    let mut a = m.to_vec();
    let mut d = 1.0;
    for i in 0..n {
        let mut pivot = i;
        for r in (i + 1)..n {
            if a[r][i].abs() > a[pivot][i].abs() {
                pivot = r;
            }
        }
        if a[pivot][i].abs() < 1e-12 {
            return 0.0;
        }
        if pivot != i {
            a.swap(pivot, i);
            d = -d;
        }
        d *= a[i][i];
        for r in (i + 1)..n {
            let f = a[r][i] / a[i][i];
            for c in i..n {
                a[r][c] -= f * a[i][c];
            }
        }
    }
    d
}

fn invert(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv = vec![vec![0.0; n]; n];
    for (i, row) in inv.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for i in 0..n {
        let mut pivot = i;
        for r in (i + 1)..n {
            if a[r][i].abs() > a[pivot][i].abs() {
                pivot = r;
            }
        }
        if a[pivot][i].abs() < 1e-12 {
            return None;
        }
        a.swap(pivot, i);
        inv.swap(pivot, i);
        let pv = a[i][i];
        for c in 0..n {
            a[i][c] /= pv;
            inv[i][c] /= pv;
        }
        for r in 0..n {
            if r == i {
                continue;
            }
            let f = a[r][i];
            if f == 0.0 {
                continue;
            }
            for c in 0..n {
                a[r][c] -= f * a[i][c];
                inv[r][c] -= f * inv[i][c];
            }
        }
    }
    Some(inv)
}

fn frobenius_pair(m: &[Vec<f64>], inv: &[Vec<f64>]) -> f64 {
    let norm = |x: &[Vec<f64>]| x.iter().flatten().map(|v| v * v).sum::<f64>().sqrt();
    norm(m) * norm(inv)
}

/// Ridge-regularized inverse (S + ridge*I)^-1. A legal alternative candidate,
/// never applied silently.
fn regularized_inverse(m: &[Vec<f64>], ridge: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut a = m.to_vec();
    for i in 0..n {
        a[i][i] += ridge;
    }
    invert(&a)
}

pub fn assess_matrix(matrix: &MatrixVersion, labels: &[String]) -> CompensationAssessment {
    let m = &matrix.coefficients;
    let n = m.len();
    let mut diagnostics = Vec::new();
    let square = n > 0 && m.iter().all(|r| r.len() == n) && n == labels.len();
    let detv = if square { det(m) } else { 0.0 };
    if !square {
        diagnostics.push(Diag {
            code: "matrix-shape".into(),
            message: "compensation matrix is not square / label aligned".into(),
        });
    }

    let inv = if square { invert(m) } else { None };
    let cond = if let (true, Some(iv)) = (square, inv.as_ref()) {
        Some(frobenius_pair(m, iv))
    } else {
        None
    };
    let singular = !square || detv.abs() < 1e-12 || inv.is_none();
    let near_singular = !singular && matches!(cond, Some(c) if c >= SINGULARITY_CONDITION_THRESHOLD);

    if singular {
        diagnostics.push(Diag {
            code: "matrix-singular".into(),
            message: format!("compensation matrix '{}' is singular (det={:.3e}); it must not be silently inverted", matrix.id, detv),
        });
    } else if near_singular {
        diagnostics.push(Diag {
            code: "matrix-near-singular".into(),
            message: format!(
                "compensation matrix '{}' is near-singular (condition ~{:.1}); explicit reviewer choice required",
                matrix.id,
                cond.unwrap()
            ),
        });
    }

    let mut candidates = Vec::new();
    candidates.push(CompensationCandidate {
        kind: "refuse".into(),
        usable: !singular,
        condition_number: cond,
        note: if singular {
            "inverse undefined; compensation blocked until matrix is revised".into()
        } else if near_singular {
            "inverse exists but is numerically unstable; explicit refusal or alternative required".into()
        } else {
            "matrix is well conditioned; inverse may be applied".into()
        },
    });
    if let Some(ridge_inv) = if square {
        regularized_inverse(m, 0.05)
    } else {
        None
    } {
        let c = Some(frobenius_pair(m, &ridge_inv));
        candidates.push(CompensationCandidate {
            kind: "regularized".into(),
            usable: true,
            condition_number: c,
            note: "ridge-regularized inverse (S + 0.05 I)^-1; legal alternative, requires explicit choice".into(),
        });
    }
    candidates.push(CompensationCandidate {
        kind: "passthrough".into(),
        usable: true,
        condition_number: None,
        note: "skip compensation and gate on raw spillover-contaminated signal; legal alternative, requires explicit choice".into(),
    });

    CompensationAssessment {
        matrix_id: matrix.id.clone(),
        labels: labels.to_vec(),
        candidates,
        inverse: if singular || near_singular { None } else { inv },
        singular,
        near_singular,
        condition_number: cond,
        diagnostics,
    }
}

// --------------------------------------------------------------------------
// Transform
// --------------------------------------------------------------------------

pub fn asinh_scale(x: f64, cofactor: f64) -> f64 {
    (x / cofactor).asinh() * cofactor
}

// --------------------------------------------------------------------------
// Point-in-polygon with explicit boundary handling
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Inside,
    Outside,
    Boundary,
}

fn on_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64, eps: f64) -> bool {
    let cross = (px - ax) * (by - ay) - (py - ay) * (bx - ax);
    if cross.abs() > eps {
        return false;
    }
    let dot = (px - ax) * (px - bx) + (py - ay) * (py - by);
    dot <= eps
}

pub fn point_in_polygon(point: [f64; 2], poly: &[[f64; 2]], rule: BoundaryRule) -> Membership {
    if poly.len() < 3 {
        return Membership::Outside;
    }
    let [x, y] = point;
    let eps = 1e-9_f64;
    let n = poly.len();
    for i in 0..n {
        let [ax, ay] = poly[i];
        let [bx, by] = poly[(i + 1) % n];
        if on_segment(x, y, ax, ay, bx, by, eps) {
            return match rule {
                BoundaryRule::Include => Membership::Inside,
                BoundaryRule::Exclude => Membership::Boundary,
            };
        }
    }
    let mut inside = false;
    for i in 0..n {
        let [ax, ay] = poly[i];
        let [bx, by] = poly[(i + 1) % n];
        let intersects = (ay > y) != (by > y)
            && x < (bx - ax) * (y - ay) / (by - ay) + ax;
        if intersects {
            inside = !inside;
        }
    }
    if inside { Membership::Inside } else { Membership::Outside }
}

// --------------------------------------------------------------------------
// Rehydrated state
// --------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct EventCoordinate {
    pub event_id: String,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub membership: String,
    pub notes: Vec<String>,
}

/// A derived population at one point in history. `denom_parent_event` /
/// `denom_parent_version` pin the percentage denominator to the exact parent
/// population version; `stale` marks that an upstream artifact changed and this
/// stored result no longer reflects the current inputs (it is never
/// overwritten).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PopulationResult {
    pub gate_logical: String,
    pub gate_version_id: String,
    pub derived_at_event: String,
    pub parent_logical: Option<String>,
    pub denom_parent_event: Option<String>,
    pub denom_parent_version: Option<String>,
    pub numerator: usize,
    pub denominator: usize,
    pub boundary: usize,
    pub indeterminate: usize,
    pub percent: Option<f64>,
    pub stale: bool,
    pub stale_reason: Option<String>,
    pub blocked: bool,
    pub block_reason: Option<String>,
    pub input_fp: String,
    pub coordinates: Vec<EventCoordinate>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Ruling {
    pub event_id: String,
    pub gate_logical: String,
    pub verdict: String,
    pub rationale: String,
    pub annulled: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Rehydrated {
    pub branch_id: String,
    pub head_event: Option<String>,
    pub package_id: Option<String>,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<EventRecord>,
    pub spillover: Option<SpilloverMeta>,
    pub matrix: Option<MatrixVersion>,
    pub transform: Option<TransformParams>,
    pub gates: Vec<GateDef>,
    pub relations: Vec<GateRelation>,
    pub assessment: Option<CompensationAssessment>,
    pub compensation_mode: String,
    pub populations: Vec<PopulationResult>,
    pub rulings: Vec<Ruling>,
    pub diagnostics: Vec<Diag>,
}

#[derive(Debug, Clone)]
struct Working {
    package_id: Option<String>,
    channels: Vec<ChannelSpec>,
    by_channel: BTreeMap<String, usize>,
    events: Vec<EventRecord>,
    spillover: Option<SpilloverMeta>,
    matrix: Option<MatrixVersion>,
    transform: Option<TransformParams>,
    gates: BTreeMap<String, GateDef>,
    relations: BTreeMap<String, Option<String>>,
    assessment: Option<CompensationAssessment>,
    compensation_mode: String,
    active_inverse: Option<Vec<Vec<f64>>>,
    /// logical gate -> last stored result
    stored: BTreeMap<String, PopulationResult>,
    order: Vec<String>,
    rulings: Vec<Ruling>,
    diagnostics: Vec<Diag>,
}

impl Working {
    fn empty() -> Self {
        Working {
            package_id: None,
            channels: Vec::new(),
            by_channel: BTreeMap::new(),
            events: Vec::new(),
            spillover: None,
            matrix: None,
            transform: None,
            gates: BTreeMap::new(),
            relations: BTreeMap::new(),
            assessment: None,
            compensation_mode: "unset".into(),
            stored: BTreeMap::new(),
            order: Vec::new(),
            rulings: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn channel_index(&self, id: &str) -> Option<usize> {
        self.by_channel.get(id).copied()
    }

    fn raw(&self, ev: &EventRecord, idx: usize) -> Option<f64> {
        ev.values.get(idx).and_then(|v| v.numeric)
    }

    /// Coordinates of one event in a gate's declared axis space. Blocked
    /// compensation yields `None` fluorescence coordinates.

fn channel_diagnostics(channel: &ChannelSpec, v: &ChannelValue) -> Vec<String> {
    let mut d = v.diagnostics.clone();
    if let Some(n) = v.numeric {
        if n < channel.range_min - 1e-9 || n > channel.range_max + 1e-9 {
            d.push(format!(
                "out-of-range: {} raw {} outside [{},{}] {}",
                channel.id, n, channel.range_min, channel.range_max, channel.unit
            ));
        }
    } else {
        d.push(format!(
            "unconvertible: '{}' on {} retained verbatim; value not plottable",
            v.raw_text, channel.id
        ));
    }
    d
}

impl Working {
    fn refresh_assessment(&mut self) {
        if let (Some(m), Some(s)) = (self.matrix.as_ref(), self.spillover.as_ref()) {
            let a = assess_matrix(m, &s.labels);
            let blocked = a.singular || a.near_singular;
            self.assessment = Some(a);
            if blocked {
                self.compensation_mode = "blocked";
                self.active_inverse = None;
            }
        }
    }

    fn chosen_inverse(&self) -> Option<&Vec<Vec<f64>>> {
        self.active_inverse.as_ref()
    }

    /// Build the compensated fluorescence vector for one event.
    fn compensated_vector(&self, ev: &EventRecord) -> BTreeMap<String, Option<f64>> {
        let mut out = BTreeMap::new();
        let labels = match self.spillover.as_ref() {
            Some(s) => s.labels.clone(),
            None => return out,
        };
        if self.compensation_mode == "passthrough" {
            for lab in &labels {
                if let Some(ci) = self.channel_index(lab) {
                    out.insert(lab.clone(), self.raw(ev, ci));
                }
            }
            return out;
        }
        let inv = match self.chosen_inverse() {
            Some(i) => i,
            None => {
                for lab in &labels {
                    out.insert(lab.clone(), None);
                }
                return out;
            }
        };
        let n = labels.len();
        let mut raw = Vec::with_capacity(n);
        for lab in &labels {
            let ci = self.channel_index(lab);
            raw.push(ci.and_then(|i| self.raw(ev, i)));
        }
        for (row, lab) in labels.iter().enumerate() {
            let mut acc = 0.0;
            let mut ok = true;
            for c in 0..n {
                match raw[c] {
                    Some(v) => acc += inv[row][c] * v,
                    None => ok = false,
                }
            }
            out.insert(lab.clone(), if ok { Some(acc) } else { None });
        }
        out
    }

    fn coordinate_for(
        &self,
        ev: &EventRecord,
        comp: &BTreeMap<String, Option<f64>>,
        axis: &AxisRef,
        notes: &mut Vec<String>,
    ) -> Option<f64> {
        match axis.space {
            AxisSpace::Raw => self.channel_index(&axis.channel_id).and_then(|i| self.raw(ev, i)),
            AxisSpace::Compensated => match comp.get(&axis.channel_id).copied().flatten() {
                Some(v) => Some(v),
                None => {
                    notes.push("compensation-unavailable".into());
                    None
                }
            },
            AxisSpace::CompensatedAsinh => match comp.get(&axis.channel_id).copied().flatten() {
                Some(v) => {
                    let cf = self.transform.as_ref().map(|t| t.cofactor).unwrap_or(150.0);
                    Some(asinh_scale(v, cf))
                }
                None => {
                    notes.push("compensation-unavailable".into());
                    None
                }
            },
        }
    }

    /// Parents in tree order. Returns gates so that a parent precedes child.
    fn topological_gates(&self) -> Vec<GateDef> {
        let mut result = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let roots: Vec<String> = self
            .relations
            .iter()
            .filter(|(_, p)| p.is_none())
            .map(|(c, _)| c.clone())
            .collect();
        fn walk(
            logical: &str,
            gates: &BTreeMap<String, GateDef>,
            relations: &BTreeMap<String, Option<String>>,
            seen: &mut BTreeSet<String>,
            out: &mut Vec<GateDef>,
        ) {
            if seen.contains(logical) {
                return;
            }
            if let Some(p) = relations.get(logical).and_then(|p| p.clone()) {
                walk(&p, gates, relations, seen, out);
            }
            if let Some(g) = gates.get(logical) {
                if seen.insert(logical.to_string()) {
                    out.push(g.clone());
                }
            }
        }
        for r in &roots {
            walk(r, &self.gates, &self.relations, &mut seen, &mut result);
        }
        for key in self.gates.keys() {
            walk(key, &self.gates, &self.relations, &mut seen, &mut result);
        }
        result
    }

    fn parent_result<'a>(
        gates: &'a [PopulationResult],
        parent: &Option<String>,
    ) -> Option<&'a PopulationResult> {
        parent.as_ref().and_then(|p| gates.iter().find(|r| &r.gate_logical == p))
    }

    /// Derive the population for one gate against the current inputs,
    /// pinning its denominator to the parent result as it exists now.
    fn derive_gate(&self, gate: &GateDef, parent: Option<&PopulationResult>, at_event: &str)
        -> PopulationResult
    {
        let mut coordinates = Vec::new();
        let mut numerator = 0usize;
        let mut boundary = 0usize;
        let mut indeterminate = 0usize;
        let mut blocked = false;
        let mut block_reason = None;

        let gated_ids: BTreeSet<String> = parent
            .map(|p| p.coordinates.iter()
                .filter(|c| c.membership == "inside")
                .map(|c| c.event_id.clone())
                .collect())
            .unwrap_or_default();

        for ev in &self.events {
            if let Some(ids) = &gated_ids {
                if !ids.contains(&ev.id) {
                    continue;
                }
            }
            let comp = self.compensated_vector(ev);
            let mut notes = Vec::new();
            let x = self.coordinate_for(ev, &comp, &gate.x, &mut notes);
            let y = self.coordinate_for(ev, &comp, &gate.y, &mut notes);

            let (x, y) = match (x, y) {
                (Some(x), Some(y)) => (x, y),
                _ => {
                    blocked = true;
                    block_reason = Some("coordinates unavailable (blocked compensation or unconvertible value)".into());
                    indeterminate += 1;
                    coordinates.push(EventCoordinate {
                        event_id: ev.id.clone(),
                        x: None,
                        y: None,
                        membership: "indeterminate".into(),
                        notes,
                    });
                    continue;
                }
            };

            match point_in_polygon([x, y], &gate.polygon, gate.boundary_rule) {
                Membership::Inside => {
                    numerator += 1;
                    coordinates.push(EventCoordinate {
                        event_id: ev.id.clone(),
                        x: Some(x),
                        y: Some(y),
                        membership: "inside".into(),
                        notes,
                    });
                }
                Membership::Boundary => {
                    boundary += 1;
                    indeterminate += 1;
                    coordinates.push(EventCoordinate {
                        event_id: ev.id.clone(),
                        x: Some(x),
                        y: Some(y),
                        membership: "boundary".into(),
                        notes,
                    });
                }
                Membership::Outside => {
                    coordinates.push(EventCoordinate {
                        event_id: ev.id.clone(),
                        x: Some(x),
                        y: Some(y),
                        membership: "outside".into(),
                        notes,
                    });
                }
            }
        }

        let denominator = parent.map(|p| p.numerator).unwrap_or_else(|| self.events.len());
        let percent = if denominator == 0 {
            None
        } else {
            Some(100.0 * numerator as f64 / denominator as f64)
        };

        PopulationResult {
            gate_logical: gate.logical_id.clone(),
            gate_version_id: gate.version_id.clone(),
            derived_at_event: at_event.to_string(),
            parent_logical: parent.map(|p| p.gate_logical.clone()),
            denom_parent_event: parent.map(|p| p.derived_at_event.clone()),
            denom_parent_version: parent.map(|p| p.gate_version_id.clone()),
            numerator,
            denominator,
            boundary,
            indeterminate,
            percent,
            stale: false,
            stale_reason: None,
            blocked,
            block_reason,
            input_fp: String::new(),
            coordinates,
        }
    }



// --------------------------------------------------------------------------
// Input fingerprints and stale propagation
// --------------------------------------------------------------------------

impl Working {
    fn gate_channels(&self, logical: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        if let Some(g) = self.gates.get(logical) {
            out.insert(g.x.channel_id.clone());
            out.insert(g.y.channel_id.clone());
        }
        out
    }

    /// All logical gates in a subtree rooted at `root` (inclusive).
    fn subtree(&self, root: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::from([root.to_string()]);
        let mut changed = true;
        while changed {
            changed = false;
            for (child, parent) in &self.relations {
                if let Some(p) = parent {
                    if out.contains(p) && out.insert(child.clone()) {
                        changed = true;
                    }
                }
            }
        }
        out
    }

    /// Raw-value fingerprint of the channels a gate reads, over the events it
    /// currently gates (root gates see all events; child gates see the parent
    /// membership). Derived layer normalizes, raw bytes stay here.
    fn gate_fingerprint(&self, gate: &GateDef, parent_fp: Option<&str>) -> String {
        let mut src = Vec::new();
        for ev in &self.events {
            for ch in [&gate.x.channel_id, &gate.y.channel_id] {
                if let Some(ci) = self.channel_index(ch) {
                    if let Some(v) = ev.values.get(ci) {
                        src.push(format!("{}={}", ev.id, v.raw_text));
                    }
                }
            }
        }
        let basis = format!(
            "g={}|m={}|t={}|src={}",
            gate.version_id,
            self.active_matrix_fp(),
            self.transform.as_ref().map(|t| t.id.clone()).unwrap_or_default(),
            src.join(";")
        );
        match parent_fp {
            Some(p) => format!("{}|p={}", basis, p),
            None => basis,
        }
    }

    fn active_matrix_fp(&self) -> String {
        let mid = self.matrix.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        match self.compensation_mode.as_str() {
            "blocked" | "unset" => format!("{}:blocked", mid),
            "passthrough" => format!("{}:passthrough", mid),
            other => format!("{}:{}", mid, other),
        }
    }

    /// Recompute fingerprints tree-wise and mark stored results stale when
    /// their inputs changed. Stored numbers are never overwritten here.
    fn mark_stale(&mut self, at_event: &str, reason: &str, affected: &BTreeSet<String>) {
        let ordered = self.topological_gates();
        let mut parent_fps: BTreeMap<String, String> = BTreeMap::new();
        for gate in &ordered {
            let parent_logical = self
                .relations
                .get(&gate.logical_id)
                .and_then(|p| p.clone());
            let parent_fp = parent_logical.and_then(|p| parent_fps.get(&p).cloned());
            let fp = self.gate_fingerprint(gate, parent_fp.as_deref());
            parent_fps.insert(gate.logical_id.clone(), fp.clone());

            if affected.contains(&gate.logical_id) {
                if let Some(old) = self.stored.get(&gate.logical_id) {
                    if !old.stale || old.input_fp != fp {
                        let mut stale = old.clone();
                        stale.stale = true;
                        stale.stale_reason =
                            Some(format!("{} (event {})", reason, at_event));
                        self.stored.insert(gate.logical_id.clone(), stale);
                    }
                }
            }
        }
    }

    /// Produce a fresh result for one gate (and dependencies). Only a redrive
    /// replaces stored numbers; the denominator is pinned to the current
    /// parent population version.
    fn redrive_gate(&mut self, logical: &str, at_event: &str) -> Option<PopulationResult> {
        let ordered = self.topological_gates();
        let mut fresh: BTreeMap<String, PopulationResult> = BTreeMap::new();
        let mut target = None;
        for gate in &ordered {
            let parent_logical = self
                .relations
                .get(&gate.logical_id)
                .and_then(|p| p.clone());
            let parent = parent_logical.and_then(|p| fresh.get(&p));
            let mut result = self.derive_gate(gate, parent, at_event);
            let pfp = parent.map(|p| p.input_fp.clone());
            result.input_fp = self.gate_fingerprint(gate, pfp.as_deref());
            if gate.logical_id == logical {
                target = Some(result.clone());
            }
            fresh.insert(gate.logical_id.clone(), result);
        }
        if let Some(r) = target {
            self.stored.insert(logical.to_string(), r.clone());
            Some(r)
        } else {
            None
        }
    }

    fn initial_derive(&mut self, at_event: &str) {
        let ordered = self.topological_gates();
        let mut fresh: BTreeMap<String, PopulationResult> = BTreeMap::new();
        for gate in &ordered {
            let parent_logical = self
                .relations
                .get(&gate.logical_id)
                .and_then(|p| p.clone());
            let parent = parent_logical.and_then(|p| fresh.get(&p));
            let mut result = self.derive_gate(gate, parent, at_event);
            let pfp = parent.map(|p| p.input_fp.clone());
            result.input_fp = self.gate_fingerprint(gate, pfp.as_deref());
            fresh.insert(gate.logical_id.clone(), result);
        }
        for (k, v) in fresh {
            self.stored.insert(k, v);
        }
        self.order = ordered.iter().map(|g| g.logical_id.clone()).collect();
    }
}

/// Gates affected by a raw-source patch: any gate whose axes read a patched
/// channel, plus all descendants (their denominators move).
pub fn patch_affected_gates(
    relations: &BTreeMap<String, Option<String>>,
    gates: &BTreeMap<String, GateDef>,
    patched_channels: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    for (id, gate) in gates {
        let reads = gate.x.channel_id != gate.y.channel_id;
        let _ = reads;
        if patched_channels.contains(&gate.x.channel_id)
            || patched_channels.contains(&gate.y.channel_id)
        {
            roots.insert(id.clone());
        }
    }
    let mut out = roots.clone();
    for root in roots {
        let mut stack = vec![root];
        while let Some(cur) = stack.pop() {
            for (child, parent) in relations {
                if parent.as_deref() == Some(cur.as_str()) && out.insert(child.clone()) {
                    stack.push(child.clone());
                }
            }
        }
    }
    out
}

// --------------------------------------------------------------------------
// Event replay
// --------------------------------------------------------------------------

/// Outcome of replaying one event, carrying the computed invalidation scope.
pub struct AppliedEffect {
    pub affected_gates: BTreeSet<String>,
    pub reason: String,
}

fn apply_event(w: &mut Working, ev: &Event) -> AppliedEffect {
    let mut affected: BTreeSet<String> = BTreeSet::new();
    let mut reason = String::new();
    match &ev.payload {
        EventPayload::PackageImported { package } => {
            w.package_id = Some(package.package_id.clone());
            w.channels = package.channels.clone();
            w.by_channel = package
                .channels
                .iter()
                .enumerate()
                .map(|(i, c)| (c.id.clone(), i))
                .collect();
            w.events = package.events.clone();
            w.spillover = Some(package.spillover.clone());
            w.matrix = Some(package.matrix.clone());
            w.transform = Some(package.transform.clone());
            w.gates = package
                .gates
                .iter()
                .map(|g| (g.logical_id.clone(), g.clone()))
                .collect();
            w.relations = package
                .relations
                .iter()
                .map(|r| (r.child_logical.clone(), r.parent_logical.clone()))
                .collect();
            w.rulings.clear();
            // raw-layer diagnostics, computed at import, never cleaned away
            for ev in &w.events {
                for val in &ev.values {
                    if let Some(ch) = w.channels.iter().find(|c| c.id == val.channel_id) {
                        for d in channel_diagnostics(ch, val) {
                            w.diagnostics.push(Diag {
                                code: "raw-value".into(),
                                message: format!("event {}: {}", ev.id, d),
                            });
                        }
                    }
                }
            }
            w.refresh_assessment();
            w.initial_derive(ev.id.as_str());
            reason = "package imported".into();
        }
        EventPayload::SourcePatched { changes, reason: why } => {
            let mut patched_channels: BTreeSet<String> = BTreeSet::new();
            for ch in changes {
                patched_channels.insert(ch.channel_id.clone());
                if let Some(event) = w.events.iter_mut().find(|e| e.id == ch.event_id) {
                    if let Some(v) =
                        event.values.iter_mut().find(|v| v.channel_id == ch.channel_id)
                    {
                        v.raw_text = ch.new_raw_text.clone();
                        let parsed = ch.new_raw_text.trim().parse::<f64>().ok();
                        v.numeric = parsed;
                        v.diagnostics.clear();
                        if let Some(spec) = w.channels.iter().find(|c| c.id == ch.channel_id) {
                            v.diagnostics = channel_diagnostics(spec, v);
                        }
                    }
                }
            }
            affected = patch_affected_gates(&w.relations, &w.gates, &patched_channels);
            reason = format!("source patch: {}", why);
            w.mark_stale(ev.id.as_str(), &reason, &affected);
        }
        EventPayload::MatrixRevised { matrix } => {
            w.matrix = Some(matrix.clone());
            w.active_inverse = None;
            w.compensation_mode = "unset".into();
            w.refresh_assessment();
            // matrix feeds every fluorescence gate
            affected = w
                .gates
                .iter()
                .filter(|(_, g)| g.x.space != AxisSpace::Raw || g.y.space != AxisSpace::Raw)
                .map(|(id, _)| id.clone())
                .collect();
            reason = format!("compensation matrix revised to {}", matrix.id);
            w.mark_stale(ev.id.as_str(), &reason, &affected);
        }
        EventPayload::TransformRevised { transform } => {
            w.transform = Some(transform.clone());
            affected = w
                .gates
                .iter()
                .filter(|(_, g)| {
                    g.x.space == AxisSpace::CompensatedAsinh
                        || g.y.space == AxisSpace::CompensatedAsinh
                })
                .map(|(id, _)| id.clone())
                .collect();
            reason = format!("transform parameters revised to {}", transform.id);
            w.mark_stale(ev.id.as_str(), &reason, &affected);
        }
        EventPayload::GateRevised { gate } => {
            let logical = gate.logical_id.clone();
            // renaming keeps the logical id and parent relations; downstream
            // identities are stable, but numbers go stale
            w.gates.insert(logical.clone(), gate.clone());
            affected = w.subtree(&logical);
            reason = format!("gate '{}' revised to version {}", logical, gate.version_id);
            w.mark_stale(ev.id.as_str(), &reason, &affected);
        }
        EventPayload::CompensationChosen { matrix_id, choice, rationale: _ } => {
            let _ = matrix_id;
            match choice {
                CompensationChoiceKind::Refuse => {
                    w.compensation_mode = "blocked";
                    w.active_inverse = None;
                }
                CompensationChoiceKind::Passthrough => {
                    w.compensation_mode = "passthrough";
                    w.active_inverse = None;
                }
                CompensationChoiceKind::Regularized => {
                    if let Some(m) = w.matrix.as_ref() {
                        w.active_inverse = regularized_inverse(&m.coefficients, 0.05);
                        w.compensation_mode = "regularized".into();
                    }
                }
            }
            affected = w
                .gates
                .keys()
                .filter(|id| {
                    w.gates
                        .get(*id)
                        .map(|g| g.x.space != AxisSpace::Raw || g.y.space != AxisSpace::Raw)
                        .unwrap_or(false)
                })
                .cloned()
                .collect();
            reason = format!("compensation policy chosen: {:?}", choice);
            w.mark_stale(ev.id.as_str(), &reason, &affected);
        }
        EventPayload::PopulationRuled { gate_logical, verdict, rationale } => {
            w.rulings.push(Ruling {
                event_id: ev.id.clone(),
                gate_logical: gate_logical.clone(),
                verdict: format!("{:?}", verdict).to_lowercase(),
                rationale: rationale.clone(),
                annulled: false,
            });
            reason = format!("{:?} ruling on {}", verdict, gate_logical);
        }
        EventPayload::RulingReverted { annulled_event, note } => {
            for r in w.rulings.iter_mut() {
                if &r.event_id == annulled_event {
                    r.annulled = true;
                }
            }
            reason = format!("ruling {} reverted: {}", annulled_event, note);
        }
        EventPayload::GateRedriven { gate_logical, note } => {
            w.redrive_gate(gate_logical, ev.id.as_str());
            // children of the redriven gate get their denominators re-pinned
            // lazily on review; mark them stale
            for (child, parent) in &w.relations {
                if parent.as_deref() == Some(gate_logical.as_str()) {
                    affected.insert(child.clone());
                }
            }
            reason = format!("gate {} redriven: {}", gate_logical, note);
            w.mark_stale(ev.id.as_str(), &reason, &affected);
        }
    }
    AppliedEffect {
        affected_gates: affected,
        reason,
    }
}

/// Replay a branch's events from the empty state. Returning the rehydrated
/// head state lets the UI render any historical point with the exact artifact
/// versions and pinned denominators that existed then.
pub fn rehydrate(branch_id: &str, head: Option<&str>, events: &[Event]) -> Rehydrated {
    let mut w = Working::empty();
    let mut last_head = None;
    for ev in events {
        apply_event(&mut w, ev);
        last_head = Some(ev.id.clone());
        if let Some(h) = head {
            if ev.id == h {
                break;
            }
        }
    }
    let ordered_ids = w.topological_gates().iter().map(|g| g.logical_id.clone()).collect::<Vec<_>>();
    let populations = ordered_ids
        .iter()
        .filter_map(|id| w.stored.get(id).cloned())
        .collect();
    Rehydrated {
        branch_id: branch_id.to_string(),
        head_event: head.map(|h| h.to_string()).or(last_head),
        package_id: w.package_id,
        channels: w.channels,
        events: w.events,
        spillover: w.spillover,
        matrix: w.matrix,
        transform: w.transform,
        gates: w.topological_gates(),
        relations: w
            .relations
            .iter()
            .map(|(c, p)| GateRelation {
                child_logical: c.clone(),
                parent_logical: p.clone(),
            })
            .collect(),
        assessment: w.assessment,
        compensation_mode: w.compensation_mode,
        populations,
        rulings: w.rulings,
        diagnostics: w.diagnostics,
    }
}
