//! Bundled offline domain case pack. Not a generic spreadsheet: the fixture is
//! a real (synthetic) FCS 3.1 acquisition with flow-specific spillover,
//! compensation and gate lineage semantics.

use crate::domain::{
    ChannelMeta, CompMatrixV, EventSetV, GateNode, GateV, LineageV, SpilloverMetaV, TransformV,
    UnitSpec, CASE_ID,
};
use crate::fcs::parse_fcs;
use crate::linalg::{self, MatrixVerdict};
use crate::store::build::eval::{self, EvalContext};
use crate::store::build::replay;
use crate::store::Store;
use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const MANIFEST_JSON: &str = include_str!("../casepack/manifest.json");
pub const FCS_BYTES: &[u8] = include_bytes!("../casepack/boundary-chamber.fcs");
pub const FCS_NAME: &str = "boundary-chamber.fcs";
pub const CASE_SPILL_KEYWORD: &str = "$SPILL";
pub const NEAR_SINGULAR_THRESHOLD: f64 = 1e-6;
pub const RIDGE_LAMBDA: f64 = 0.01;

pub fn fcs_sha256() -> String {
    hex::encode(Sha256::digest(FCS_BYTES))
}

pub fn manifest() -> Value {
    serde_json::from_str(MANIFEST_JSON).expect("committed manifest.json must parse")
}

pub fn eventset_from_pack() -> EventSetV {
    let parsed = parse_fcs(FCS_BYTES).expect("committed FCS must parse");
    let m = manifest();
    let channels: Vec<ChannelMeta> =
        serde_json::from_value(m["channels"].clone()).expect("channels");
    let values = parsed
        .events
        .iter()
        .map(|row| row.iter().map(|v| crate::jsonnum::Jf64(*v)).collect())
        .collect();
    EventSetV {
        source_file: FCS_NAME.to_string(),
        sha256: parsed.sha256,
        media_type: "application/vnd.isac.fcs".to_string(),
        channels,
        values,
        diagnostics: parsed.diagnostics,
    }
}

pub fn spillover_from_pack() -> SpilloverMetaV {
    let m = manifest();
    SpilloverMetaV {
        keyword: m["spillover"]["keyword"].as_str().unwrap().to_string(),
        detectors: serde_json::from_value(m["spillover"]["detectors"].clone()).unwrap(),
        fluorophores: serde_json::from_value(m["spillover"]["fluorophores"].clone()).unwrap(),
        matrix: serde_json::from_value(m["spillover"]["matrix"].clone()).unwrap(),
        unit: serde_json::from_value(m["spillover"]["unit"].clone()).unwrap(),
        raw_representation: m["spillover"]["raw_representation"].as_str().unwrap().to_string(),
    }
}

pub fn transforms_from_pack() -> Vec<TransformV> {
    serde_json::from_value(manifest()["transforms"].clone()).unwrap()
}

pub fn gates_from_pack() -> Vec<GateV> {
    serde_json::from_value(manifest()["gates"].clone()).unwrap()
}

pub fn lineage_from_pack() -> LineageV {
    LineageV {
        nodes: serde_json::from_value(manifest()["lineage"].clone()).unwrap(),
    }
}

pub fn comp_required_gates() -> Vec<String> {
    serde_json::from_value(manifest()["comp_required_gates"].clone()).unwrap()
}

pub fn verdict() -> MatrixVerdict {
    let s = spillover_from_pack();
    linalg::verdict(&s.matrix, NEAR_SINGULAR_THRESHOLD)
        .expect("pack spillover is square and finite")
}

/// Seed the baseline: all six source kinds separately versioned, rule
/// judgement events, blocked populations (no silent inversion) and the two
/// legal compensation candidates. Runs as one atomic import: any failure
/// rolls the whole case back.
pub fn seed_baseline(store: &mut Store) -> rusqlite::Result<i64> {
    store.clear_case(CASE_ID)?;

    let tx_blob_sha = fcs_sha256();
    store.put_blob(&tx_blob_sha, FCS_BYTES, "application/vnd.isac.fcs")?;

    let mut seq = 0;
    let commit = |store: &mut Store, seq: &mut i64, kind: &str, actor: &str, payload: Value| {
        let out = store.commit_event(
            CASE_ID,
            crate::domain::MAIN_BRANCH,
            *seq,
            Some(*seq),
            kind,
            actor,
            &payload,
        )?;
        *seq = out.head_seq;
        Ok::<_, rusqlite::Error>(())
    };

    commit(store, &mut seq, "case_opened", "system", json!({
        "case_id": CASE_ID,
        "title": manifest()["title"],
    }))?;

    commit(store, &mut seq, "rules_registered", "system", json!({
        "rule_codes": [
            "R-COMP-NEARSING-NO-INVERT",
            "R-PCT-PINNED-PARENT-VERSION",
            "R-UPSTREAM-REVISION-STALE-NO-OVERWRITE",
            "R-MULTI-CANDIDATE-EXPLICIT-ADJUDICATION",
            "R-NO-IRREVERSIBLE-IMPORT-CLEANING"
        ],
        "thresholds": {
            "near_singular_rcond": NEAR_SINGULAR_THRESHOLD,
            "ridge_lambda": RIDGE_LAMBDA,
            "boundary_epsilon": 1e-9
        }
    }))?;

    commit(store, &mut seq, "eventset_imported", "flow-platform", json!({
        "resource_id": eval::ACQ,
        "version": 1,
        "body": eventset_from_pack(),
        "raw_blob_sha256": tx_blob_sha,
        "source_kind": "FCS 3.1 acquisition",
    }))?;

    // Single-stain controls reference raw event indexes, never rewritten values.
    commit(store, &mut seq, "single_stains_imported", "flow-platform", json!({
        "resource_id": "eventset:single-stains",
        "version": 1,
        "body": {
            "controls": manifest()["single_stain_controls"],
            "eventset_id": eval::ACQ,
            "eventset_version": 1
        }
    }))?;

    commit(store, &mut seq, "spillover_registered", "flow-platform", json!({
        "resource_id": "spillover:instrument",
        "version": 1,
        "body": spillover_from_pack(),
    }))?;

    // Machine-readable judgement: near-singular -> inversion blocked.
    let v = verdict();
    commit(store, &mut seq, "rule_judged", "system", json!({
        "rule_code": "R-COMP-NEARSING-NO-INVERT",
        "subject_kind": "spillover",
        "subject_id": "spillover:instrument",
        "subject_version": 1,
        "status": "blocked",
        "evidence": {
            "condition_number_1norm": v.condition_number_1norm,
            "reciprocal_condition": v.reciprocal_condition,
            "threshold": v.threshold
        },
        "decision": "inverse_not_applied; legal candidates retained for adjudication"
    }))?;

    commit(store, &mut seq, "transform_registered", "reviewer", json!({
        "transform_id": eval::TRANSFORM,
        "version": 1,
        "body": { "transforms": transforms_from_pack() }
    }))?;
    commit(store, &mut seq, "transform_registered", "reviewer", json!({
        "transform_id": "transform:raw-scatter",
        "version": 1,
        "body": manifest()["raw_scatter_transform"]
    }))?;

    for g in gates_from_pack() {
        commit(store, &mut seq, "gate_registered", "reviewer", json!({
            "gate_id": gate_id_for(&g.name),
            "version": 1,
            "body": g
        }))?;
    }
    commit(store, &mut seq, "lineage_registered", "reviewer", json!({
        "resource_id": eval::LINEAGE,
        "version": 1,
        "body": lineage_from_pack()
    }))?;

    // Two legal, retained candidates for the unresolvable compensation decision.
    let ridge = linalg::ridge_unmixing(&spillover_from_pack().matrix, RIDGE_LAMBDA)
        .expect("ridge unmixing exists");
    commit(store, &mut seq, "candidate_proposed", "system", json!({
        "candidate_id": "cand:comp-block-v1",
        "scope": "compensation",
        "option_key": "block",
        "rationale": "Do not unmix: keep raw fluorescence and mark populations blocked until controls are reviewed.",
        "payload": {"action": "leave_compensated_uncomputed"}
    }))?;
    commit(store, &mut seq, "candidate_proposed", "system", json!({
        "candidate_id": "cand:comp-ridge-v1",
        "scope": "compensation",
        "option_key": "ridge_regularize",
        "rationale": "Ridge-regularised unmixing (S^T S + lambda I)^-1 S^T with explicit lambda; bias is disclosed, not hidden.",
        "payload": {"lambda": RIDGE_LAMBDA, "unmixing": ridge}
    }))?;

    // Initial evaluation: scatter gate computes; fluorescence gates blocked.
    evaluate_and_commit(store, &mut seq, "system", "baseline evaluation after import")?;
    Ok(seq)
}

/// Build comp matrix body from the accepted ridge candidate.
pub fn ridge_comp_matrix_v2() -> CompMatrixV {
    let s = spillover_from_pack();
    let unmixing = linalg::ridge_unmixing(&s.matrix, RIDGE_LAMBDA).unwrap();
    CompMatrixV {
        detectors: s.detectors,
        unmixing,
        derived_from_spillover_id: "spillover:instrument".to_string(),
        derived_from_spillover_version: 1,
        derivation: format!("ridge-regularised pseudoinverse lambda={RIDGE_LAMBDA}"),
        near_singular_threshold: NEAR_SINGULAR_THRESHOLD,
        condition_number_1norm: Some(verdict().condition_number_1norm),
        unit: UnitSpec {
            scale: "dimensionless unmixing coefficients".to_string(),
            basis: "applied to ADC-count fluorescence vector".to_string(),
        },
    }
}

/// Recompute populations against current resources and commit a new
/// `populations_evaluated` event with pinned dependencies.
pub fn evaluate_and_commit(
    store: &mut Store,
    seq: &mut i64,
    actor: &str,
    note: &str,
) -> rusqlite::Result<()> {
    let events = store.all_events(CASE_ID)?;
    let state = replay(&events);
    let ctx = EvalContext {
        comp_required: comp_required_gates().into_iter().collect(),
    };
    let results = eval::evaluate(&state, &ctx);
    let body = results_to_payload(&results);
    let out = store.commit_event(
        CASE_ID,
        crate::domain::MAIN_BRANCH,
        *seq,
        Some(*seq),
        "populations_evaluated",
        actor,
        &json!({"note": note, "results": body}),
    )?;
    *seq = out.head_seq;
    Ok(())
}

fn results_to_payload(results: &[crate::domain::PopulationResult]) -> Value {
    let arr: Vec<Value> = results
        .iter()
        .map(|r| {
            let mut deps = vec![
                json!({"kind": "eventset", "resource_id": eval::ACQ, "version": 1}),
                json!({"kind": "transform", "resource_id": r.transform_id, "version": r.transform_version}),
                json!({"kind": "gate", "resource_id": r.gate_id, "version": r.gate_version}),
            ];
            if let (Some(pid), Some(pv)) = (&r.parent_result_id, r.parent_result_version) {
                deps.push(json!({"kind": "parent_result", "resource_id": pid, "version": pv}));
            }
            json!({
                "status": r.status,
                "gate_id": r.gate_id,
                "gate_version": r.gate_version,
                "parent_result_id": r.parent_result_id,
                "parent_result_version": r.parent_result_version,
                "transform_id": r.transform_id,
                "transform_version": r.transform_version,
                "comp_candidate_id": r.comp_candidate_id,
                "denominator_scope": r.denominator_scope,
                "denominator_event_count": r.denominator_event_count,
                "numerator_event_count": r.numerator_event_count,
                "percent": r.percent,
                "member_event_indexes": r.member_event_indexes,
                "diagnostics": r.diagnostics,
                "dependencies": deps
            })
        })
        .collect();
    json!(arr)
}

fn gate_id_for(name: &str) -> String {
    match name {
        "Lymphocytes" => "gate:lymphocytes",
        "CD3+ T cells" => "gate:cd3",
        "CD4+ T cells" => "gate:cd4",
        "CD8+ T cells" => "gate:cd8",
        "CD19+ B cells" => "gate:cd19",
        other => panic!("unexpected gate name {other}"),
    }
    .to_string()
}

/// Nodes in a stable parent-before-child order for invalidation expansion.
pub fn lineage_nodes() -> Vec<GateNode> {
    lineage_from_pack().nodes
}

#[allow(dead_code)]
fn blob_anchor(conn: &rusqlite::Connection) {
    let _ = conn.query_row("SELECT 1 FROM raw_blob WHERE sha256=?1", params!["x"], |_| Ok(()));
}
