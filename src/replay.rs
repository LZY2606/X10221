//! Event-sourced lineage state.
//!
//! State is reconstructed by replaying immutable branch events. Revisions
//! supersede references but never overwrite objects or old conclusions: stale
//! population results are marked with a reason and kept for history.

use crate::engine;
use crate::types::*;
use anyhow::{anyhow, bail, Result};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn as_str(v: &serde_json::Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

fn as_obj<T: serde::de::DeserializeOwned>(v: &serde_json::Value) -> T {
    serde_json::from_value(v.clone()).expect("event payload matches schema")
}

/// Effective references for one branch at one point in time.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub raw: Option<(String, RawEvents)>,
    pub spill_meta: BTreeMap<String, SpilloverMeta>,
    pub matrices: BTreeMap<String, CompensationMatrix>,
    pub matrix_candidate_sets: BTreeMap<String, MatrixCandidateSet>,
    pub transforms: BTreeMap<String, TransformParams>,
    pub gates: BTreeMap<String, Gate>,
    pub gate_versions: BTreeMap<String, String>,
    pub links: BTreeMap<String, GateLink>,
    pub link_versions: BTreeMap<String, String>,
    /// gate_stable_id -> gate_version -> BoundaryCandidateSet
    pub boundary_sets: BTreeMap<String, BTreeMap<String, BoundaryCandidateSet>>,
    pub comp_adj: Option<CompAdjudication>,
    pub comps: BTreeMap<String, CompensatedEvents>,
    pub boundary_adj: BTreeMap<String, BoundaryAdjudication>,
    /// gate stable id -> result (kept even when stale)
    pub pops: BTreeMap<String, PopResult>,
    pub stale: BTreeMap<String, StaleReason>,
    pub history: Vec<String>,
}

impl State {
    pub fn replay(events: &[EventRecord]) -> Result<State> {
        let mut state = State::default();
        for ev in events {
            state.apply(ev)?;
        }
        Ok(state)
    }

    pub fn effective_transforms(&self) -> BTreeMap<String, TransformParams> {
        let mut out = BTreeMap::new();
        for version in self.transform_versions.values() {
            if let Some(t) = self.transforms.get(version) {
                out.insert(version.clone(), t.clone());
            }
        }
        out
    }

    pub fn comp_events(&self) -> Option<&CompensatedEvents> {
        self.comp_adj.as_ref().and_then(|a| self.comps.get(&a.compensated_version))
    }

    fn mark_stale(&mut self, reason: StaleReason) {
        for id in reason.affected() {
            self.stale.insert(id.clone(), reason.clone());
        }
    }

    fn gate_transform_version(&self, gate: &Gate) -> String {
        self.transforms
            .values()
            .find(|t| {
                (t.channel == gate.x_channel || t.channel == gate.y_channel)
            })
            .map(|t| t.version_id())
            .unwrap_or_default()
    }

    fn apply(&mut self, ev: &EventRecord) -> Result<()> {
        let p = &ev.payload;
        self.history.push(ev.id.clone());
        match ev.kind.as_str() {
            "CASE_OPENED" => {}
            "RAW_REGISTERED" => {
                let raw: RawEvents = as_obj(&p["raw"]);
                self.raw = Some((crate::hash::content_id("raw", &raw), raw));
            }
            "SPILLOVER_META_REGISTERED" => {
                let meta: SpilloverMeta = as_obj(&p["meta"]);
                self.spill_meta
                    .insert(crate::hash::content_id("spillmeta", &meta), meta);
            }
            "MATRIX_REGISTERED" => {
                let matrix: CompensationMatrix = as_obj(&p["matrix"]);
                let version = matrix.version_id();
                self.matrices.insert(version.clone(), matrix.clone());
                let set = engine::matrix_candidates(&version, &matrix)?;
                self.matrix_candidate_sets
                    .insert(version.clone(), set);
                // A new matrix supersedes any prior compensation downstream.
                if let Some(prev) = self.comp_adj.clone() {
                    if prev.matrix_version != version {
                        let affects = self.pop_subtree_all();
                        self.mark_stale(StaleReason::CompensationSuperseded {
                            old_version: prev.compensated_version.clone(),
                            new_version: None,
                            affects,
                        });
                        self.comp_adj = None;
                    }
                }
            }
            "TRANSFORM_REGISTERED" => {
                let t: TransformParams = as_obj(&p["transform"]);
                self.transforms.insert(t.version_id(), t);
            }
            "TRANSFORM_REVISED" => {
                let old_version = as_str(p, "old_version");
                let t: TransformParams = as_obj(&p["transform"]);
                let new_version = t.version_id();
                self.transforms.insert(new_version.clone(), t.clone());
                // Re-key effective transforms by channel.
                let channel = t.channel.clone();
                self.transform_versions.retain(|_, v| *v != old_version);
                self.transform_versions.insert(channel, new_version.clone());
                let mut using = BTreeSet::new();
                for gate in self.gates.values() {
                    if gate.x_channel == channel || gate.y_channel == channel {
                        using.insert(gate.stable_id.clone());
                    }
                }
                let affects = engine::blast_radius(&self.live_links(), &using);
                self.mark_stale(StaleReason::TransformRevision {
                    channel,
                    old_version,
                    new_version,
                    affects: affects.into_iter().collect(),
                });
            }
            "GATE_REGISTERED" => {
                let gate: Gate = as_obj(&p["gate"]);
                let version = gate.version_id();
                self.gates.insert(version, gate.clone());
                self.gate_versions.insert(gate.stable_id.clone(), version);
            }
            "LINK_REGISTERED" => {
                let link: GateLink = as_obj(&p["link"]);
                let version = link.version_id();
                self.links.insert(version, link.clone());
                self.link_versions.insert(link.child_stable_id.clone(), version);
            }
            "GATE_REVISED" => {
                let old_version = as_str(p, "old_version");
                let gate: Gate = as_obj(&p["gate"]);
                let new_version = gate.version_id();
                let geometry_changed =
                    p["geometry_changed"].as_bool().unwrap_or(true);
                self.gates.insert(new_version.clone(), gate.clone());
                self.gate_versions
                    .insert(gate.stable_id.clone(), new_version.clone());
                // Boundary adjudication of the old polygon is withdrawn.
                self.boundary_adj.remove(&gate.stable_id);
                if geometry_changed {
                    let affects = engine::blast_radius(
                        &self.live_links(),
                        &BTreeSet::from([gate.stable_id.clone()]),
                    );
                    self.mark_stale(StaleReason::GateRevision {
                        stable_id: gate.stable_id.clone(),
                        old_version,
                        new_version: new_version.clone(),
                        affects: affects.into_iter().collect(),
                    });
                }
            }
            "LINK_REVISED" => {
                let old_version = as_str(p, "old_version");
                let link: GateLink = as_obj(&p["link"]);
                let new_version = link.version_id();
                self.links.insert(new_version.clone(), link.clone());
                let affects = engine::blast_radius(
                    &self.live_links(),
                    &BTreeSet::from([link.child_stable_id.clone()]),
                );
                self.link_versions
                    .insert(link.child_stable_id.clone(), new_version.clone());
                self.mark_stale(StaleReason::LinkRevision {
                    old_version,
                    new_version,
                    affects: affects.into_iter().collect(),
                });
            }
            "BOUNDARY_DETECTED" => {
                let set: BoundaryCandidateSet = as_obj(p);
                self.boundary_sets
                    .entry(set.gate_version.clone())
                    .or_default()
                    .insert(set.gate_version.clone(), set);
            }
            "COMPENSATION_ADJUDICATED" => {
                let adj: CompAdjudication = as_obj(p);
                if !self.comps.contains_key(&adj.compensated_version) {
                    bail!("adjudication references missing compensated events");
                }
                self.comp_adj = Some(adj);
                // Clear compensation-related staleness; populations must be
                // (re)computed explicitly rather than silently replaced.
            }
            "COMPENSATION_ADJUDICATION_WITHDRAWN" => {
                let prev = self
                    .comp_adj
                    .clone()
                    .ok_or_else(|| anyhow!("cannot withdraw: no active compensation"))?;
                let affects = self.pop_subtree_all();
                self.mark_stale(StaleReason::AdjudicationWithdrawn {
                    scope: format!("matrix:{}", prev.matrix_version),
                    affects,
                });
                self.comp_adj = None;
            }
            "COMPENSATED_REGISTERED" => {
                let comp: CompensatedEvents = as_obj(&p["compensated"]);
                self.comps.insert(comp.version_id(), comp);
            }
            "BOUNDARY_ADJUDICATED" => {
                let adj: BoundaryAdjudication = as_obj(p);
                self.boundary_adj
                    .insert(as_str(p, "gate_stable_id"), adj);
            }
            "BOUNDARY_ADJUDICATION_WITHDRAWN" => {
                let stable_id = as_str(p, "gate_stable_id");
                let prev = self
                    .boundary_adj
                    .remove(&stable_id)
                    .ok_or_else(|| anyhow!("no boundary adjudication to withdraw"))?;
                let affects = engine::blast_radius(
                    &self.live_links(),
                    &BTreeSet::from([stable_id.clone()]),
                );
                self.mark_stale(StaleReason::AdjudicationWithdrawn {
                    scope: format!("boundary:{}", prev.gate_version),
                    affects: affects.into_iter().collect(),
                });
            }
            "POPULATION_COMPUTED" => {
                let pop: PopResult = as_obj(p);
                self.stale.remove(&pop.gate_stable_id);
                self.pops.insert(pop.gate_stable_id.clone(), pop);
            }
            "CONFLICT_RECORDED" => {}
            "BRANCH_OPENED" | "RULING_UNDONE" => {}
            other => bail!("unknown event kind {}", other),
        }
        Ok(())
    }
}
