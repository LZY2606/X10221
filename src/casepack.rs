//! The bundled offline case pack "流式边界室". Raw file bytes are preserved
//! verbatim so the import layer can store the original representation.

use serde_json::Value;

pub struct Artifact {
    pub raw: String,
    pub json: Value,
}

pub struct CasePack {
    pub manifest: Artifact,
    pub fcs_events: Artifact,
    pub spillover_meta: Artifact,
    pub comp_matrix: Artifact,
    pub comp_matrix_near_singular: Artifact,
    pub transform_params: Artifact,
    pub gating_draft: Artifact,
    pub extreme_events: Artifact,
    pub scenarios: Artifact,
}

fn artifact(raw: &str) -> Result<Artifact, String> {
    let json: Value = serde_json::from_str(raw).map_err(|e| format!("casepack parse: {e}"))?;
    Ok(Artifact { raw: raw.to_string(), json })
}

pub fn load_embedded() -> Result<CasePack, String> {
    Ok(CasePack {
        manifest: artifact(include_str!("../casepack/manifest.json"))?,
        fcs_events: artifact(include_str!("../casepack/fcs_events.json"))?,
        spillover_meta: artifact(include_str!("../casepack/single_stain_controls.json"))?,
        comp_matrix: artifact(include_str!("../casepack/compensation_matrix.json"))?,
        comp_matrix_near_singular: artifact(include_str!(
            "../casepack/compensation_matrix_near_singular.json"
        ))?,
        transform_params: artifact(include_str!("../casepack/transform_params.json"))?,
        gating_draft: artifact(include_str!("../casepack/gating_draft.json"))?,
        extreme_events: artifact(include_str!("../casepack/extreme_events.json"))?,
        scenarios: artifact(include_str!("../casepack/scenarios.json"))?,
    })
}
