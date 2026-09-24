use crate::linear::{apply_inverse, invert_two_by_two, matrix_from_payload};
use crate::models::*;
use crate::geometry::point_in_polygon;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

const NEAR_SINGULAR_THRESHOLD: f64 = 1e-8;

impl Default for ReplayState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn replay(events: &[Event], branches: &[Branch]) -> ReplayState {
    let mut state = ReplayState::new();
    for branch in branches {
        state.branches.insert(branch.id.clone(), branch.clone());
        state.branch_events.insert(branch.id.clone(), Vec::new());
    }
    for event in events {
        apply_event(&mut state, event);
    }
    state
}

fn apply_event(state: &mut ReplayState, event: &Event) {
    state.events.push(event.clone());
    state
        .branch_events
        .entry(event.branch_id.clone())
        .or_default()
        .push(event.clone());
    match event.event_type.as_str() {
        "raw_channels.imported" => state.raw_versions.push(material(event)),
        "spillover.imported" => state.spillover_versions.push(material(event)),
        "compensation.recorded" => state.compensation_versions.push(material(event)),
        "transform.parameters.recorded" => state.transform_versions.push(material(event)),
        "gate.polygon.recorded" | "gate.polygon.revised" | "gate.renamed" => state
            .gate_polygons
            .entry(event.payload["gate_id"].as_str().unwrap_or("unknown").to_string())
            .or_default()
            .push(material(event)),
        "gate.relation.recorded" => state.gate_relations.push(material(event)),
        "gate.population.computed" | "gate.population.candidate" => add_population(state, event),
        "population.stale" => mark_stale(state, event),
        "decision.recorded" => add_judgment(state, event),
        "compensation.near_singular_rejected" | "transform.rejected" | "relation.ambiguous" => {
            state.diagnostics.push(event.payload.clone())
        }
        _ => {}
    }
}

fn material(event: &Event) -> MaterialVersion {
    MaterialVersion {
        kind: event.event_type.split('.').next().unwrap_or("material").to_string(),
        id: event
            .payload
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        version: event
            .payload
            .get("version")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as u32,
        event_id: event.event_id.clone(),
        payload: event.payload.clone(),
    }
}

fn add_population(state: &mut ReplayState, event: &Event) {
    let payload = &event.payload;
    let result = PopulationResult {
        id: payload["id"].as_str().unwrap_or("population").to_string(),
        version: payload["version"].as_u64().unwrap_or(1) as u32,
        gate_id: payload["gate_id"].as_str().unwrap_or("gate").to_string(),
        parent_population_id: payload["parent_population_id"].as_str().map(str::to_string),
        parent_gate_version: payload["parent_gate_version"].as_u64().map(|v| v as u32),
        transform_version: payload["transform_version"].as_u64().unwrap_or(1) as u32,
        included_event_ids: string_array(&payload["included_event_ids"]),
        denominator_event_ids: string_array(&payload["denominator_event_ids"]),
        numerator: payload["numerator"].as_u64().unwrap_or(0) as usize,
        denominator: payload["denominator"].as_u64().unwrap_or(0) as usize,
        percent: payload["percent"].as_f64().unwrap_or(0.0),
        percent_unit: payload["percent_unit"].as_str().unwrap_or("%").to_string(),
        boundary_policy: payload["boundary_policy"].as_str().unwrap_or("include").to_string(),
        candidates: Vec::new(),
        valid: payload["valid"].as_bool().unwrap_or(true),
        stale: payload["stale"].as_bool().unwrap_or(false),
        reason: payload["reason"].as_str().map(str::to_string),
        event_id: event.event_id.clone(),
    };
    state
        .populations
        .entry(result.gate_id.clone())
        .or_default()
        .push(result);
}

fn mark_stale(state: &mut ReplayState, event: &Event) {
    for population_id in string_array(&event.payload["affected_population_ids"]) {
        if let Some((gate, version)) = population_id.rsplit_once(':') {
            if let Some(list) = state.populations.get_mut(gate) {
                if let Ok(version) = version.parse::<u32>() {
                    if let Some(item) = list.iter_mut().find(|item| item.version == version) {
                        item.stale = true;
                        item.reason = Some(
                            event
                                .payload
                                .get("reason")
                                .and_then(|v| v.as_str())
                                .unwrap_or("upstream dependency changed")
                                .to_string(),
                        );
                    }
                }
            }
        }
    }
}

fn add_judgment(state: &mut ReplayState, event: &Event) {
    let target = event.payload["target_id"].as_str().unwrap_or("target").to_string();
    let judgment = Judgment {
        decision_id: event.payload["decision_id"].as_str().unwrap_or("decision").to_string(),
        accepted: event.payload["accepted"].as_bool().unwrap_or(false),
        rationale: event.payload["rationale"].as_str().unwrap_or_default().to_string(),
        event_id: event.event_id.clone(),
        conflicted: event.payload["conflicted"].as_bool().unwrap_or(false),
    };
    state.judgments.entry(target).or_default().push(judgment);
}

pub fn latest<T>(items: &[T], id: &str, id_fn: impl Fn(&T) -> String) -> Option<&T> {
    items.iter().filter(|item| id_fn(item) == id).last()
}

pub fn flow_events(state: &ReplayState) -> Vec<FlowEvent> {
    state
        .raw_versions
        .last()
        .and_then(|raw| raw.payload.get("events"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

pub fn latest_json<'a>(items: &'a [MaterialVersion], id: &str) -> Option<&'a MaterialVersion> {
    latest(items, id, |item| item.id.clone())
}

pub fn transform_event_values(
    flow_events: &[FlowEvent],
    transform: &MaterialVersion,
    channels: &[String],
) -> Vec<TransformedPoint> {
    let kind = transform.payload["kind"].as_str().unwrap_or("linear");
    let scale = transform.payload["scale"].as_f64().unwrap_or(1.0);
    let cofactor = transform.payload["cofactor"].as_f64().unwrap_or(1.0);
    flow_events
        .iter()
        .map(|event| {
            let mut values = BTreeMap::new();
            for channel in channels {
                let raw = event
                    .channels
                    .get(channel)
                    .and_then(|item| item.value)
                    .unwrap_or(f64::NAN);
                let value = if kind == "asinh" && raw.is_finite() {
                    (raw / cofactor).asinh()
                } else if raw.is_finite() {
                    raw * scale
                } else {
                    f64::NAN
                };
                values.insert(channel.clone(), value);
            }
            TransformedPoint {
                event_id: event.id.clone(),
                values,
            }
        })
        .collect()
}

pub fn compensate(
    flow_events: &[FlowEvent],
    matrix_payload: &serde_json::Value,
    channels: &[String],
) -> Result<Vec<TransformedPoint>, MatrixReportLike> {
    let matrix = matrix_from_payload(matrix_payload);
    let report = invert_two_by_two(&matrix, NEAR_SINGULAR_THRESHOLD);
    if report.inverse.is_none() {
        return Err(MatrixReportLike {
            reason: report.reason,
            determinant: report.determinant,
            reciprocal_condition: report.reciprocal_condition,
            singular: report.singular,
            near_singular: report.near_singular,
        });
    }
    Ok(flow_events
        .iter()
        .map(|event| {
            let raw = channels
                .iter()
                .map(|channel| event.channels.get(channel).and_then(|v| v.value).unwrap_or(0.0))
                .collect::<Vec<_>>();
            let corrected = apply_inverse(&matrix, &raw).unwrap_or_default();
            let mut values = BTreeMap::new();
            for (index, channel) in channels.iter().enumerate() {
                values.insert(channel.clone(), corrected[index]);
            }
            TransformedPoint {
                event_id: event.id.clone(),
                values,
            }
        })
        .collect())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MatrixReportLike {
    pub reason: String,
    pub determinant: f64,
    pub reciprocal_condition: f64,
    pub singular: bool,
    pub near_singular: bool,
}

pub fn gates(state: &ReplayState) -> Vec<GateState> {
    state
        .gate_relations
        .iter()
        .map(|relation| {
            let gate_id = relation.payload["gate_id"].as_str().unwrap_or("gate");
            let polygon = latest_gate_polygon(state, gate_id).unwrap_or(relation);
            gate_from_material(polygon, relation)
        })
        .collect()
}

pub fn latest_gate_polygon<'a>(
    state: &'a ReplayState,
    gate_id: &str,
) -> Option<&'a MaterialVersion> {
    state.gate_polygons.get(gate_id).and_then(|items| items.last())
}

fn gate_from_material(polygon: &MaterialVersion, relation: &MaterialVersion) -> GateState {
    GateState {
        id: polygon.payload["gate_id"].as_str().unwrap_or("gate").to_string(),
        version: polygon.payload["version"].as_u64().unwrap_or(1) as u32,
        name: polygon.payload["name"].as_str().unwrap_or("gate").to_string(),
        parent_gate_id: relation.payload["parent_gate_id"].as_str().map(str::to_string),
        x_channel: polygon.payload["x_channel"].as_str().unwrap_or("FL1-A").to_string(),
        y_channel: polygon.payload["y_channel"].as_str().unwrap_or("FL2-A").to_string(),
        transform_id: polygon.payload["transform_id"]
            .as_str()
            .unwrap_or("transform")
            .to_string(),
        polygon: polygon.payload["polygon"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|point| {
                [
                    point[0].as_f64().unwrap_or(0.0),
                    point[1].as_f64().unwrap_or(0.0),
                ]
            })
            .collect(),
        boundary_policy: polygon.payload["boundary_policy"]
            .as_str()
            .unwrap_or("include")
            .to_string(),
        event_id: polygon.event_id.clone(),
        relation_event_id: relation.event_id.clone(),
        relation_version: relation.payload["version"].as_u64().unwrap_or(1) as u32,
    }
}

pub fn population_for_gate(
    gate: &GateState,
    points: &[TransformedPoint],
    parent_event_ids: Option<&BTreeSet<String>>,
    transform_version: u32,
    previous_version: u32,
) -> PopulationResult {
    let include_boundary = gate.boundary_policy == "include";
    let denominator_points: Vec<_> = points
        .iter()
        .filter(|point| {
            let finite = point.values.get(&gate.x_channel).is_some_and(f64::is_finite)
                && point.values.get(&gate.y_channel).is_some_and(f64::is_finite);
            finite
                && parent_event_ids
                    .as_ref()
                    .is_none_or(|parent| parent.contains(&point.event_id))
        })
        .collect();
    let denominator_ids = denominator_points
        .iter()
        .map(|point| point.event_id.clone())
        .collect::<Vec<_>>();
    let included = denominator_points
        .iter()
        .filter(|point| {
            point_in_polygon(
                [
                    point.values[&gate.x_channel],
                    point.values[&gate.y_channel],
                ],
                &gate.polygon,
                include_boundary,
            )
        })
        .map(|point| point.event_id.clone())
        .collect::<Vec<_>>();
    let denominator = denominator_ids.len();
    let numerator = included.len();
    let percent = if denominator == 0 {
        0.0
    } else {
        numerator as f64 * 100.0 / denominator as f64
    };
    PopulationResult {
        id: format!("{}:{}", gate.id, previous_version + 1),
        version: previous_version + 1,
        gate_id: gate.id.clone(),
        parent_population_id: gate.parent_gate_id.as_ref().map(|id| format!("{id}:latest")),
        parent_gate_version: None,
        transform_version,
        included_event_ids: included,
        denominator_event_ids: denominator_ids,
        numerator,
        denominator,
        percent,
        percent_unit: "percent-of-pinned-parent-population".to_string(),
        boundary_policy: gate.boundary_policy.clone(),
        candidates: Vec::new(),
        valid: true,
        stale: false,
        reason: None,
        event_id: String::new(),
    }
}

pub fn stale_payload(reason: &str, affected: Vec<String>, sources: Vec<SourceRef>) -> serde_json::Value {
    json!({
        "id": "stale",
        "reason": reason,
        "affected_population_ids": affected,
        "sources": sources,
        "propagation": "downstream-only",
        "overwrite": false
    })
}

pub fn string_array(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}
