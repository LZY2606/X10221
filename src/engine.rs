//! Pure computation layer: candidates, coordinates, gate membership and
//! dependency blast-radius computation. No I/O or persistence here.

use crate::geometry;
use crate::matrix::{self, InversionCandidate};
use crate::types::{
    BoundaryCandidateSet, CompAdjudication, CompensatedEvents, CompensationMatrix, Gate,
    GateLink, MatrixCandidateSet, Point, PopResult, RawEvents, TransformFamily, TransformParams,
};
use anyhow::{anyhow, bail, Result};
use std::collections::{BTreeMap, BTreeSet};

pub fn matrix_candidates(
    matrix_version: &str,
    comp: &CompensationMatrix,
) -> Result<MatrixCandidateSet> {
    Ok(MatrixCandidateSet {
        matrix_version: matrix_version.to_string(),
        candidates: matrix::analyze(comp)?,
    })
}

pub fn build_compensated(
    raw_version: &str,
    matrix_version: &str,
    raw: &RawEvents,
    channels: &[String],
    chosen: &InversionCandidate,
) -> Result<CompensatedEvents> {
    let index: BTreeMap<String, usize> = raw
        .channels
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.clone(), i))
        .collect();
    let mut channel_idx = Vec::with_capacity(channels.len());
    for ch in channels {
        channel_idx.push(
            *index
                .get(ch)
                .ok_or_else(|| anyhow!("compensation references unknown channel {}", ch))?,
        );
    }
    let mut values = Vec::with_capacity(raw.events.len());
    for ev in &raw.events {
        let vector: Vec<f64> = channel_idx.iter().map(|&i| ev.values[i]).collect();
        values.push(matrix::apply_inverse(&chosen.inverse, &vector));
    }
    Ok(CompensatedEvents {
        raw_version: raw_version.to_string(),
        matrix_version: matrix_version.to_string(),
        candidate_kind: chosen.kind.clone(),
        epsilon: chosen.epsilon,
        channels: channels.to_vec(),
        values,
    })
}

pub fn channel_value(
    raw: &RawEvents,
    comp: Option<&CompensatedEvents>,
    channel: &str,
    input: &str,
    event_index: usize,
) -> Result<f64> {
    match input {
        "raw" => {
            let idx = raw
                .channels
                .iter()
                .position(|c| c.name == channel)
                .ok_or_else(|| anyhow!("unknown raw channel {}", channel))?;
            Ok(raw.events[event_index].values[idx])
        }
        "compensated" => {
            let comp = comp.ok_or_else(|| {
                anyhow!("channel {} needs compensation, which is not yet adjudicated", channel)
            })?;
            let idx = comp
                .channels
                .iter()
                .position(|c| c == channel)
                .ok_or_else(|| anyhow!("unknown compensated channel {}", channel))?;
            Ok(comp.values[event_index][idx])
        }
        other => bail!("unknown transform input '{}'", other),
    }
}

pub fn transform_value(family: &TransformFamily, value: f64) -> f64 {
    geometry::apply(family, value)
}

fn transform_for<'a>(
    transforms: &'a BTreeMap<String, TransformParams>,
    channel: &str,
) -> Option<&'a TransformParams> {
    transforms.values().find(|t| t.channel == channel)
}

pub struct GateEvaluation {
    pub members: Vec<u32>,
    pub boundary: Vec<u32>,
    pub boundary_set: Option<BoundaryCandidateSet>,
    pub points: Vec<Point>,
}

pub fn evaluate_gate(
    gate_version: &str,
    gate: &Gate,
    raw: &RawEvents,
    comp: Option<&CompensatedEvents>,
    transforms: &BTreeMap<String, TransformParams>,
) -> Result<GateEvaluation> {
    let tx = transform_for(transforms, &gate.x_channel).ok_or_else(|| {
        anyhow!("no transform parameters registered for channel {}", gate.x_channel)
    })?;
    let ty = transform_for(transforms, &gate.y_channel).ok_or_else(|| {
        anyhow!("no transform parameters registered for channel {}", gate.y_channel)
    })?;
    if tx.input == "compensated" || ty.input == "compensated" {
        if comp.is_none() {
            bail!(
                "gate {} depends on compensated channels before compensation was adjudicated",
                gate.stable_id
            );
        }
    }
    let mut points = Vec::with_capacity(raw.events.len());
    for (i, _ev) in raw.events.iter().enumerate() {
        let vx = channel_value(raw, comp, &gate.x_channel, &tx.input, i)?;
        let vy = channel_value(raw, comp, &gate.y_channel, &ty.input, i)?;
        points.push(Point {
            x: geometry::apply(&tx.family, vx),
            y: geometry::apply(&ty.family, vy),
        });
    }

    let mut members = Vec::new();
    let mut boundary_idx = Vec::new();
    let mut boundary_points = Vec::new();
    for (i, p) in points.iter().enumerate() {
        if geometry::on_polygon_boundary(p, &gate.polygon) {
            boundary_idx.push(i as u32);
            boundary_points.push(p.clone());
        } else if geometry::strictly_inside(p, &gate.polygon) {
            members.push(i as u32);
        }
    }
    let boundary_set = if boundary_idx.is_empty() {
        None
    } else {
        Some(BoundaryCandidateSet {
            gate_version: gate_version.to_string(),
            candidates: geometry::evaluate_boundary(
                boundary_idx.clone(),
                boundary_points,
                &gate.polygon,
            ),
        })
    };
    Ok(GateEvaluation {
        members,
        boundary: boundary_idx,
        boundary_set,
        points,
    })
}

/// Gate stable ids downstream of `changed` (including itself), computed from
/// the currently effective link versions.
pub fn blast_radius(links: &BTreeMap<String, GateLink>, changed: &BTreeSet<String>) -> BTreeSet<String> {
    // child -> parent
    let mut children_of: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
    for link in links.values() {
        children_of
            .entry(link.parent_stable_id.clone())
            .or_default()
            .push(link.child_stable_id.clone());
    }
    let mut affected = BTreeSet::new();
    let mut stack: Vec<String> = changed.iter().cloned().collect();
    while let Some(id) = stack.pop() {
        if affected.insert(id.clone()) {
            if let Some(kids) = children_of.get(&Some(id)) {
                for kid in kids {
                    stack.push(kid.clone());
                }
            }
        }
    }
    affected
}

#[allow(clippy::too_many_arguments)]
pub fn population_result(
    gate_stable_id: &str,
    gate_version: &str,
    link_version: &str,
    transform_version: &str,
    comp_adj: Option<&CompAdjudication>,
    member_indices: Vec<u32>,
    boundary_indices: &[u32],
    boundary_policy: &str,
    parent_pop: Option<&PopResult>,
    link: &GateLink,
) -> PopResult {
    let mut members = member_indices;
    if boundary_policy == "EDGE_INCLUSIVE" {
        members.extend_from_slice(boundary_indices);
        members.sort_unstable();
        members.dedup();
    } else if boundary_policy == "BOTTOM_LEFT_INCLUSIVE" {
        // Which boundary events are inside was already resolved into members
        // by the caller; nothing extra to add.
    }
    members.sort_unstable();
    members.dedup();
    let count = members.len() as u64;
    let parent_count = match &parent_pop {
        Some(p) => p.count,
        None => 0,
    };
    let percent = if link.parent_stable_id.is_none() {
        100.0
    } else if parent_count == 0 {
        0.0
    } else {
        count as f64 * 100.0 / parent_count as f64
    };
    PopResult {
        gate_stable_id: gate_stable_id.to_string(),
        gate_version: gate_version.to_string(),
        link_version: link_version.to_string(),
        transform_version: transform_version.to_string(),
        compensation_version: comp_adj.map(|a| a.compensated_version.clone()),
        parent_pop_version: parent_pop.map(|p| p.version_id()),
        parent_gate_stable_id: link.parent_stable_id.clone(),
        count,
        parent_count,
        percent_of_parent: percent,
        percent_of_parent_unit: "% of parent population (pinned version)".to_string(),
        member_event_indices: members,
        boundary_event_indices: boundary_indices.to_vec(),
        boundary_policy: boundary_policy.to_string(),
    }
}

impl PopResult {
    pub fn version_id(&self) -> String {
        crate::hash::content_id("pop", self)
    }
}

impl TransformParams {
    pub fn version_id(&self) -> String {
        crate::hash::content_id("xform", self)
    }
}

impl Gate {
    pub fn version_id(&self) -> String {
        crate::hash::content_id("gate", self)
    }
    /// Geometry fingerprint: a rename keeps membership and downstream results.
    pub fn membership_fingerprint(&self) -> String {
        serde_json::json!({
            "x_channel": self.x_channel,
            "y_channel": self.y_channel,
            "basis": self.basis,
            "polygon": self.polygon,
        })
        .to_string()
    }
}

impl GateLink {
    pub fn version_id(&self) -> String {
        crate::hash::content_id("link", self)
    }
}

impl CompensationMatrix {
    pub fn version_id(&self) -> String {
        crate::hash::content_id("matrix", self)
    }
}

impl CompensatedEvents {
    pub fn version_id(&self) -> String {
        crate::hash::content_id("comp", self)
    }
}

/// Linear family cofactor placeholder kept for test readability.
pub fn linear(scale: f64) -> TransformFamily {
    TransformFamily::Linear { scale }
}
