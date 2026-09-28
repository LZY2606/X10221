//! Domain model for the flow-cytometry compensation / gating lineage review tool.
//!
//! Sources and interpretations are stored separately: raw channel values,
//! spillover metadata, compensation matrices, transform parameters, gate
//! polygons and parent/child relations each get their own immutable,
//! independently versioned artifact documents.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CASE_BUNDLE_ID: &str = "flow-boundary-chamber";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseBundle {
    pub schema: String,
    pub case_id: String,
    pub title: String,
    pub domain: String,
    pub panel: Panel,
    pub artifacts: BTreeMap<String, ArtifactInit>,
    pub steps: Vec<ReplayStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Panel {
    pub channels: Vec<Channel>,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    pub marker: String,
    pub detector: String,
    pub unit: String,
    pub space: String,
}

/// Initial immutable version of one artifact kind, with the raw source bytes
/// kept separately from the interpreted document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactInit {
    pub kind: ArtifactKind,
    pub version: u32,
    pub source: String,
    pub source_encoding: String,
    pub doc: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Events,
    Spillover,
    Transform,
    Gates,
    Relations,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Events => "events",
            ArtifactKind::Spillover => "spillover",
            ArtifactKind::Transform => "transform",
            ArtifactKind::Gates => "gates",
            ArtifactKind::Relations => "relations",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "events" => ArtifactKind::Events,
            "spillover" => ArtifactKind::Spillover,
            "transform" => ArtifactKind::Transform,
            "gates" => ArtifactKind::Gates,
            "relations" => ArtifactKind::Relations,
            _ => return None,
        })
    }
}

// ---------- interpreted documents ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsDoc {
    pub events: Vec<EventRaw>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRaw {
    pub id: String,
    pub index: u32,
    pub values: BTreeMap<String, RawValue>,
}

/// A raw channel value keeps its original representation. A value that cannot
/// be converted to a number stays in place with a diagnostic; it is never
/// silently dropped just to make plotting possible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawValue {
    pub repr: String,
    pub unit: String,
    pub numeric: Option<f64>,
    pub convertible: bool,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverDoc {
    pub channels: Vec<String>,
    /// Square spillover matrix S (rows = detector, columns = fluorophore),
    /// unit: fraction of source signal.
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    pub method: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformDoc {
    pub params: BTreeMap<String, TransformParam>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformParam {
    pub method: String,
    pub unit: String,
    pub basis: String,
    #[serde(default)]
    pub t: f64,
    #[serde(default)]
    pub m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatesDoc {
    pub gates: Vec<GateDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateDef {
    pub id: String,
    pub label: String,
    pub space: GateSpace,
    pub x_channel: String,
    pub y_channel: String,
    pub kind: String,
    pub vertices: Vec<[f64; 2]>,
    /// Point exactly on an edge is included (half-open boundary convention).
    pub boundary_rule: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum GateSpace {
    Raw,
    Compensated,
    Transformed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationsDoc {
    pub root: String,
    /// Edges point parent gate id -> child gate id.
    pub edges: Vec<RelationEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationEdge {
    pub parent: String,
    pub child: String,
}

// ---------- replay script ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayStep {
    pub seq: u32,
    pub trigger: String,
    pub description: String,
    pub action: ReplayAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplayAction {
    ImportInitial,
    PublishArtifact {
        kind: ArtifactKind,
        version: u32,
        reason: String,
    },
    Adjudicate {
        target: String,
        decision: String,
        reason: String,
    },
    Fork {
        branch: String,
        reason: String,
    },
    RenameGate {
        gate_id: String,
        new_label: String,
    },
    ReviseGate {
        gate_id: String,
        version: u32,
    },
    ConcurrentCommit {
        branch: String,
    },
}
