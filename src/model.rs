//! Machine-readable event vocabulary.
//!
//! Every reviewer-visible fact is an append-only event. Events reference
//! exact upstream versions (`Ref { id, version }`), so historical conclusions
//! can be reconstructed without consulting the newest computation.

use serde::{Deserialize, Serialize};

/// Pointer to a specific versioned object.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RefV {
    pub id: String,
    pub version: u32,
}

impl RefV {
    pub fn new(id: impl Into<String>, version: u32) -> Self {
        RefV { id: id.into(), version }
    }
    pub fn stable(&self) -> String {
        format!("{}@v{}", self.id, self.version)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChannelDefV {
    pub code: String,
    pub long_name: String,
    pub unit: String,
    pub coordinate_basis: String,
    pub range: f64,
    pub bits: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CompStatus {
    Invertible,
    NearSingular,
    Singular,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CandidateV {
    pub id: String,
    pub label: String,
    pub formula: String,
    pub inverse: Vec<Vec<f64>>,
    pub residual_norm: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GatePointV {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GateSpecV {
    pub coordinate_basis: String,
    pub x_channel: String,
    pub y_channel: String,
    pub transform: RefV,
    pub vertices: Vec<GatePointV>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PercentageCandidateV {
    pub id: String,
    pub interpretation: String,
    pub count: u32,
    pub denominator: u32,
    pub percent: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PopulationStatV {
    pub parent_result_id: Option<String>,
    pub parent_label: String,
    pub parent_gate_version: Option<RefV>,
    pub candidates: Vec<PercentageCandidateV>,
    pub boundary_events: Vec<u32>,
    pub non_finite_events: Vec<u32>,
    pub ambiguous: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StaleItem {
    pub result_id: String,
    pub reason: String,
    pub causal_event_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EventPayload {
    CaseInitialized {
        case_id: String,
        title: String,
    },
    SourceImported {
        source_id: String,
        version: u32,
        kind: String,
        filename: String,
        sha256: String,
        size: usize,
        raw_nonfinite: u32,
    },
    SourceReimported {
        source_id: String,
        version: u32,
        filename: String,
        sha256: String,
        size: usize,
        raw_nonfinite: u32,
        changed_channels: Vec<String>,
    },
    ChannelDefsRegistered {
        source: RefV,
        channels: Vec<ChannelDefV>,
    },
    SpilloverRegistered {
        spill_id: String,
        version: u32,
        source: RefV,
        labels: Vec<String>,
        matrix: Vec<Vec<f64>>,
    },
    CompDerived {
        comp_id: String,
        version: u32,
        spill: RefV,
        status: CompStatus,
        rcond: f64,
        min_rel_pivot: f64,
        inverse: Option<Vec<Vec<f64>>>,
        candidates: Vec<CandidateV>,
    },
    CompAdopted {
        comp: RefV,
        candidate_id: Option<String>,
    },
    CompRejected {
        comp: RefV,
        reason: String,
    },
    TransformRegistered {
        transform_id: String,
        version: u32,
        basis: String,
        cofactor: f64,
        input_unit: String,
    },
    GateDrafted {
        gate_id: String,
        version: u32,
        name: String,
        parent_gate_id: Option<String>,
        spec: GateSpecV,
    },
    GateAdopted {
        gate: RefV,
    },
    GateRejected {
        gate: RefV,
        reason: String,
    },
    GateRenamed {
        gate_id: String,
        version: u32,
        old_name: String,
        new_name: String,
    },
    GateRevised {
        gate_id: String,
        version: u32,
        spec: GateSpecV,
    },
    PopulationDerived {
        result_id: String,
        gate: RefV,
        parent_result_id: Option<String>,
        denominator: u32,
        denom_source: String,
        source: RefV,
        comp: Option<RefV>,
        transform: RefV,
        stat: PopulationStatV,
    },
    BoundaryResolved {
        result_id: String,
        gate: RefV,
        chosen_candidate_id: String,
        interpretation: String,
    },
    ResultsMarkedStale {
        items: Vec<StaleItem>,
        scope: String,
    },
    BranchCreated {
        branch_id: String,
        name: String,
        parent_branch_id: String,
        fork_event_id: String,
    },
    DecisionUndone {
        undone_event_id: String,
        reverted_event_ids: Vec<String>,
    },
    ConflictRejected {
        branch: String,
        base_tip_seq: i64,
        actual_tip_seq: i64,
        attempted_kind: String,
        detail: String,
    },
}

impl EventPayload {
    pub fn kind(&self) -> &'static str {
        match self {
            EventPayload::CaseInitialized { .. } => "case-initialized",
            EventPayload::SourceImported { .. } => "source-imported",
            EventPayload::SourceReimported { .. } => "source-reimported",
            EventPayload::ChannelDefsRegistered { .. } => "channel-defs-registered",
            EventPayload::SpilloverRegistered { .. } => "spillover-registered",
            EventPayload::CompDerived { .. } => "comp-derived",
            EventPayload::CompAdopted { .. } => "comp-adopted",
            EventPayload::CompRejected { .. } => "comp-rejected",
            EventPayload::TransformRegistered { .. } => "transform-registered",
            EventPayload::GateDrafted { .. } => "gate-drafted",
            EventPayload::GateAdopted { .. } => "gate-adopted",
            EventPayload::GateRejected { .. } => "gate-rejected",
            EventPayload::GateRenamed { .. } => "gate-renamed",
            EventPayload::GateRevised { .. } => "gate-revised",
            EventPayload::PopulationDerived { .. } => "population-derived",
            EventPayload::BoundaryResolved { .. } => "boundary-resolved",
            EventPayload::ResultsMarkedStale { .. } => "results-marked-stale",
            EventPayload::BranchCreated { .. } => "branch-created",
            EventPayload::DecisionUndone { .. } => "decision-undone",
            EventPayload::ConflictRejected { .. } => "conflict-rejected",
        }
    }

    /// Stable identity of the displayed value this event establishes or acts on.
    /// The UI attaches exactly this string to every displayed value.
    pub fn subject_id(&self) -> String {
        match self {
            EventPayload::CaseInitialized { case_id, .. } => case_id.clone(),
            EventPayload::SourceImported { source_id, version, .. }
            | EventPayload::SourceReimported { source_id, version, .. } => {
                format!("{source_id}@v{version}")
            }
            EventPayload::ChannelDefsRegistered { source, .. } => {
                format!("channels@{}", source.stable())
            }
            EventPayload::SpilloverRegistered { spill_id, version, .. } => {
                format!("{spill_id}@v{version}")
            }
            EventPayload::CompDerived { comp_id, version, .. } => {
                format!("{comp_id}@v{version}")
            }
            EventPayload::CompAdopted { comp, .. } | EventPayload::CompRejected { comp, .. } => {
                comp.stable()
            }
            EventPayload::TransformRegistered { transform_id, version, .. } => {
                format!("{transform_id}@v{version}")
            }
            EventPayload::GateDrafted { gate_id, version, .. }
            | EventPayload::GateRenamed { gate_id, version, .. }
            | EventPayload::GateRevised { gate_id, version, .. } => {
                format!("{gate_id}@v{version}")
            }
            EventPayload::GateAdopted { gate } | EventPayload::GateRejected { gate, .. } => {
                gate.stable()
            }
            EventPayload::PopulationDerived { result_id, .. }
            | EventPayload::BoundaryResolved { result_id, .. } => result_id.clone(),
            EventPayload::ResultsMarkedStale { .. } => "stale-scope".to_string(),
            EventPayload::BranchCreated { branch_id, .. } => branch_id.clone(),
            EventPayload::DecisionUndone { undone_event_id, .. } => undone_event_id.clone(),
            EventPayload::ConflictRejected { branch, .. } => format!("conflict@{branch}"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredEvent {
    pub seq: i64,
    pub branch_seq: i64,
    pub event_id: String,
    pub parent_event_id: Option<String>,
    pub branch_id: String,
    pub actor: String,
    pub clock: String,
    pub payload: EventPayload,
    pub undone: bool,
}
