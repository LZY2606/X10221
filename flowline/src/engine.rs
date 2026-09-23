//! Append-only event log + deterministic reducer.
//!
//! Source facts and human interpretation are separate.  Reading at any event
//! id reconstructs the exact material versions and visible conclusions at
//! that point; newer calculations never overwrite history.

use crate::linalg::{self, InvertStatus};
use crate::model::{AxisRef, CompCandidate, GateDef, Geometry, TransformParam};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn digest_id(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    h.update(b"flowline|v1|");
    for p in parts {
        h.update([0x1f]);
        h.update(p.as_bytes());
    }
    hex::encode(&h.finalize())[..16].to_string()
}

// ---------------------------------------------------------------- events

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    /// Whole case pack imported atomically; carries every material version.
    PackImported {
        case_id: String,
        title: String,
        versions: MaterialVersions,
    },
    /// One direct source revised; `scope` lists dependency ids invalidated.
    SourceRevised {
        kind: String,
        version_id: String,
        reason: String,
        scope: Vec<String>,
        body: serde_json::Value,
    },
    /// Localized raw patch to a single channel of a single event.
    SourcePatched {
        event_index: usize,
        channel: String,
        old_token: String,
        new_token: String,
        reason: String,
        scope: Vec<String>,
    },
    /// Reviewer decision on compensation candidates.
    CompensationDecided {
        spillover_version: String,
        /// adopted candidate id, or None = rejected
        adopted: Option<String>,
        rejected: Vec<String>,
        rationale: String,
    },
    /// Reviewer resolution of an ambiguous population denominator.
    DenominatorResolved {
        population_id: String,
        gate_version: String,
        chosen_parent_version: String,
        rejected_parent_versions: Vec<String>,
        rationale: String,
    },
    /// Manual accept/reject/adjust of a population result.
    PopulationVerdict {
        population_id: String,
        gate_version: String,
        verdict: String, // accepted | rejected | adjusted
        note: String,
    },
    /// Marker carried by the auto-derived evaluation trail (kept in the log
    /// when an explicit evaluation action is recorded; replay is idempotent).
    DerivedEvaluated {
        targets: Vec<String>,
        deterministic: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub id: String,
    pub branch: String,
    pub parent_event_id: Option<String>,
    pub payload: EventPayload,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MaterialVersions {
    pub events: String,
    pub spillover: String,
    pub compensation: String,
    pub transforms: String,
    pub gates: String,
}

// ---------------------------------------------------------------- derived

#[derive(Debug, Clone, Serialize)]
pub struct MemberResult {
    pub event_index: usize,
    pub event_id: String,
    pub included: bool,
    /// "in" | "out" | "boundary" | "undetermined"
    pub membership: String,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateResult {
    pub id: String,
    pub parent_version: Option<String>,
    pub count: usize,
    pub percent: Option<f64>,
    pub denominator_note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopulationResult {
    pub gate_id: String,
    pub gate_version: String,
    pub label: String,
    pub parent_gate: Option<String>,
    /// "valid" | "stale" | "ambiguous" | "blocked"
    pub status: String,
    pub stale_reason: Option<String>,
    pub chosen_candidate_id: Option<String>,
    pub candidates: Vec<CandidateResult>,
    pub members: Vec<MemberResult>,
    pub verdict: Option<String>,
    pub verdict_note: Option<String>,
    pub denominator_pin: Option<DenominatorPin>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DenominatorPin {
    pub parent_gate: String,
    pub parent_version: String,
    pub parent_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompensationEvaluation {
    pub spillover_version: String,
    pub candidate_id: String,
    pub matrix_kind: String,
    pub status: String, // adopted | rejected | blocked_ok | blocked_near_singular | blocked_singular | candidate
    pub reason: String,
    pub condition_estimate: Option<f64>,
    pub gain_matrix_preview: Option<Vec<Vec<f64>>>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct BranchHead {
    pub branch: String,
    pub head_event_id: Option<String>,
    pub base_version: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct State {
    pub branch: String,
    pub head_event_id: Option<String>,
    pub case_id: Option<String>,
    pub title: Option<String>,
    pub versions: Option<MaterialVersions>,
    pub events_version: Option<String>,
    pub spillover_version: Option<String>,
    pub compensation_version: Option<String>,
    pub transforms_version: Option<String>,
    pub gates_version: Option<String>,
    pub gates: Vec<GateDef>,
    pub transforms: Vec<TransformParam>,
    pub spillover_order: Vec<String>,
    pub spillover_matrix: Vec<Vec<f64>>,
    pub comp_candidates: Vec<CompCandidate>,
    pub comp_evaluations: Vec<CompensationEvaluation>,
    pub adopted_comp: Option<String>,
    pub populations: Vec<PopulationResult>,
    pub channel_values: std::collections::BTreeMap<String, ChannelSeries>,
    pub diagnostics: Vec<crate::fcs::Diagnostic>,
    pub event_meta: Vec<EventMeta>,
    pub stale_scopes: Vec<StaleScope>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleScope {
    pub reason: String,
    pub invalidated: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct EventMeta {
    pub event_id: String,
    pub kind: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelSeries {
    pub unit: Option<String>,
    pub basis: Option<String>,
    pub values: Vec<crate::fcs::RawValue>,
}

impl State {
    pub fn new(branch: &str) -> Self {
        State {
            branch: branch.to_string(),
            ..Default::default()
        }
    }
}
