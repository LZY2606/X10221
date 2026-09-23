use crate::domain::{
    CompensationMatrix, FcsFile, GateSet, Lineage, TransformParams,
};
use serde_json::Value;

pub const CASE_ID: &str = "flow-boundary-chamber-v1";
pub const CASE_TITLE: &str = "流式边界室：补偿矩阵、门控系谱与细胞群不确定性";

pub const FCS_EVENTS_JSON: &str = include_str!("../cases/flow-boundary-chamber/fcs-events.json");
pub const SPILLOVER_JSON: &str = include_str!("../cases/flow-boundary-chamber/spillover.json");
pub const COMPENSATION_V1_JSON: &str = include_str!("../cases/flow-boundary-chamber/compensation-v1.json");
pub const TRANSFORM_V1_JSON: &str = include_str!("../cases/flow-boundary-chamber/transform-v1.json");
pub const GATES_V1_JSON: &str = include_str!("../cases/flow-boundary-chamber/gates-v1.json");
pub const LINEAGE_V1_JSON: &str = include_str!("../cases/flow-boundary-chamber/lineage-v1.json");
pub const MANIFEST_JSON: &str = include_str!("../cases/flow-boundary-chamber/manifest.json");

pub struct OfflineCase {
    pub manifest: Value,
    pub fcs: FcsFile,
    pub spillover: Value,
    pub matrix: CompensationMatrix,
    pub transform: TransformParams,
    pub gates: GateSet,
    pub lineage: Lineage,
}

pub fn load_case() -> OfflineCase {
    OfflineCase {
        manifest: serde_json::from_str(MANIFEST_JSON).expect("embedded manifest"),
        fcs: serde_json::from_str(FCS_EVENTS_JSON).expect("embedded FCS events"),
        spillover: serde_json::from_str(SPILLOVER_JSON).expect("embedded spillover metadata"),
        matrix: serde_json::from_str(COMPENSATION_V1_JSON).expect("embedded compensation matrix"),
        transform: serde_json::from_str(TRANSFORM_V1_JSON).expect("embedded transform parameters"),
        gates: serde_json::from_str(GATES_V1_JSON).expect("embedded gate polygons"),
        lineage: serde_json::from_str(LINEAGE_V1_JSON).expect("embedded lineage"),
    }
}

pub fn initial_refs() -> Value {
    serde_json::json!({
        "fcs_events": "events-v1",
        "spillover_metadata": "spill-v1",
        "compensation_matrix": "matrix-v1",
        "transform_parameters": "transform-v1",
        "gate_polygons": "gates-v1",
        "gate_parent_child": "lineage-v1"
    })
}
