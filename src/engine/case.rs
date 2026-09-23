//! Offline domain case pack seeder. Builds a fully versioned baseline and the
//! three review branches, then derives results deterministically.

use super::engine::Engine;
use super::fcs::FcsFile;
use super::model::*;
use rusqlite::Connection;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

const CASE_DIR: &str = "cases/flow-boundary-chamber";

pub const BASELINE: &str = "branch-baseline";
pub const SINGULAR: &str = "branch-singular";
pub const REVISION: &str = "branch-revision-transform";
pub const RENAME: &str = "branch-rename";

fn sha1(bytes: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Baseline spillover matrix (FITC/PE block; FSC/SSC identity).
/// Measured = S * true. Well-conditioned.
pub fn baseline_spillover() -> SpilloverMeta {
    SpilloverMeta {
        source: "sst-fitc.fcs + sst-pe.fcs $PnS spillover estimates".to_string(),
        channels: vec![
            "FSC-A".to_string(),
            "SSC-A".to_string(),
            "FITC-A".to_string(),
            "PE-A".to_string(),
        ],
        matrix: vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.10],
            vec![0.0, 0.0, 0.02, 1.0],
        ],
        basis: "fraction of source-detector signal observed in detector row (ADC counts)".to_string(),
    }
}

/// Near-singular variant: FITC/PE columns nearly identical -> rcond < 1e-3.
pub fn singular_spillover() -> SpilloverMeta {
    let mut s = baseline_spillover();
    s.source = "branch-only revision: SST cross-talk re-estimated".to_string();
    s.matrix[2][3] = 0.999_5;
    s.matrix[3][2] = 0.999_9;
    s
}

fn default_transform() -> TransformParams {
    TransformParams {
        channels: vec!["FITC-A".to_string(), "PE-A".to_string()],
        t: 262_144.0,
        w: 0.5,
        m: 4.5,
        a: 0.0,
        basis: "logicle-family bi-exponential; ADC counts -> dimensionless decades".to_string(),
    }
}

fn revised_transform() -> TransformParams {
    let mut t = default_transform();
    t.w = 0.8;
    t.basis = "revised logicle W=0.8; ADC counts -> dimensionless decades".to_string();
    t
}

fn fcs_to_batch(file_name: &str, bytes: &[u8], fcs: &FcsFile, role: &str) -> RawBatch {
    let mut values = Vec::new();
    let mut diagnostics = Vec::new();
    for (ei, row) in fcs.events.iter().enumerate() {
        let mut vr = Vec::new();
        for (ci, v) in row.iter().enumerate() {
            let unit = fcs.channels[ci]
                .unit
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            vr.push(RawValue {
                token: v.token.clone(),
                numeric: v.numeric,
                unit,
            });
            if let Some(msg) = &v.diagnostic {
                diagnostics.push(RawDiagnostic {
                    event_index: ei,
                    channel_index: ci,
                    channel: fcs.channels[ci].n.clone(),
                    token: v.token.clone(),
                    message: msg.clone(),
                });
            }
        }
        values.push(vr);
    }
    RawBatch {
        file_name: file_name.to_string(),
        sha1: sha1(bytes),
        fcs_version: fcs.version.clone(),
        role: role.to_string(),
        channel_names: fcs.channels.iter().map(|c| c.n.clone()).collect(),
        channel_units: fcs.channels.iter().map(|c| c.unit.clone()).collect(),
        event_count: fcs.events.len(),
        values,
        diagnostics,
    }
}

fn insert_raw(
    engine: &Engine,
    conn: &Connection,
    branch: &str,
    file_name: &str,
    bytes: &[u8],
    role: &str,
) -> Result<RawBatch, String> {
    let fcs = FcsFile::from_bytes(bytes).map_err(|e| format!("{file_name}: {e}"))?;
    let batch = fcs_to_batch(file_name, bytes, &fcs, role);
    let id = engine.insert_authored_tx(
        conn,
        branch,
        "raw_events_imported",
        "raw#sample",
        &serde_json::to_value(&batch).unwrap(),
    )?;
    engine.put_raw_material(conn, &batch.sha1, file_name, bytes, id)?;
    Ok(batch)
}

fn insert_control(
    engine: &Engine,
    conn: &Connection,
    branch: &str,
    file_name: &str,
    bytes: &[u8],
    role: &str,
) -> Result<(), String> {
    let fcs = FcsFile::from_bytes(bytes).map_err(|e| format!("{file_name}: {e}"))?;
    let batch = fcs_to_batch(file_name, bytes, &fcs, role);
    let node = format!(
        "raw#{}",
        file_name.trim_end_matches(".fcs").replace('.', "-")
    );
    let id = engine.insert_authored_tx(
        conn,
        branch,
        "raw_events_imported",
        &node,
        &serde_json::to_value(&batch).unwrap(),
    )?;
    engine.put_raw_material(conn, &batch.sha1, file_name, bytes, id)?;
    Ok(())
}

fn gates() -> Vec<GatePolygon> {
    vec![
        GatePolygon {
            gate_id: "gate#cells".to_string(),
            label: "Cells".to_string(),
            x_channel: "FSC-A".to_string(),
            y_channel: "SSC-A".to_string(),
            coordinate_basis: "raw:ADC counts".to_string(),
            transform_version: None,
            vertices: vec![
                [40000.0, 0.0],
                [130000.0, 0.0],
                [130000.0, 80000.0],
                [40000.0, 80000.0],
            ],
        },
        GatePolygon {
            gate_id: "gate#lymph".to_string(),
            label: "Lymphocytes".to_string(),
            x_channel: "FSC-A".to_string(),
            y_channel: "SSC-A".to_string(),
            coordinate_basis: "raw:ADC counts".to_string(),
            transform_version: None,
            vertices: vec![
                [80000.0, 10000.0],
                [110000.0, 10000.0],
                [110000.0, 25000.0],
                [80000.0, 25000.0],
            ],
        },
        GatePolygon {
            gate_id: "gate#cd3".to_string(),
            label: "CD3+".to_string(),
            x_channel: "FITC-A".to_string(),
            y_channel: "PE-A".to_string(),
            coordinate_basis: "logicle:dimensionless".to_string(),
            transform_version: Some("transform#main".to_string()),
            vertices: vec![
                [1.5, 0.5],
                [2.5, 0.5],
                [2.5, 1.5],
                [1.5, 1.5],
            ],
        },
        GatePolygon {
            gate_id: "gate#cd4".to_string(),
            label: "CD4+".to_string(),
            x_channel: "FITC-A".to_string(),
            y_channel: "PE-A".to_string(),
            coordinate_basis: "logicle:dimensionless".to_string(),
            transform_version: Some("transform#main".to_string()),
            vertices: vec![
                [1.5, 0.5],
                [2.5, 0.5],
                [2.5, 0.94528],
                [1.5, 0.94528],
            ],
        },
        GatePolygon {
            gate_id: "gate#cd8".to_string(),
            label: "CD8+".to_string(),
            x_channel: "FITC-A".to_string(),
            y_channel: "PE-A".to_string(),
            coordinate_basis: "logicle:dimensionless".to_string(),
            transform_version: Some("transform#main".to_string()),
            vertices: vec![
                [1.5, 0.97516],
                [2.5, 0.97516],
                [2.5, 1.5],
                [1.5, 1.5],
            ],
        },
    ]
}

fn edges() -> GateEdges {
    GateEdges {
        edges: vec![
            GateEdge {
                child_gate: "gate#cells".to_string(),
                parent_gate: None,
            },
            GateEdge {
                child_gate: "gate#lymph".to_string(),
                parent_gate: Some("gate#cells".to_string()),
            },
            GateEdge {
                child_gate: "gate#cd3".to_string(),
                parent_gate: Some("gate#lymph".to_string()),
            },
            GateEdge {
                child_gate: "gate#cd4".to_string(),
                parent_gate: Some("gate#cd3".to_string()),
            },
            GateEdge {
                child_gate: "gate#cd8".to_string(),
                parent_gate: Some("gate#cd3".to_string()),
            },
        ],
    }
}

fn read_case_file(name: &str) -> Vec<u8> {
    std::fs::read(format!("{CASE_DIR}/{name}"))
        .unwrap_or_else(|e| panic!("missing case file {name}: {e}"))
}

/// Seed baseline + derived populations + branches. Idempotent: no-op when the
/// baseline branch already has events.
pub fn seed_if_empty(engine: &Engine) -> Result<(), String> {
    if !engine.list_branches().is_empty() {
        return Ok(());
    }
    engine.ensure_branch(BASELINE, "Baseline (导入基线)")?;

    let sample_bytes = read_case_file("sample.fcs");
    let fitc_bytes = read_case_file("sst-fitc.fcs");
    let pe_bytes = read_case_file("sst-pe.fcs");
    let unst_bytes = read_case_file("sst-unstained.fcs");

    {
        let mut c = engine.db.lock_conn();
        let tx = c.transaction().map_err(|e| e.to_string())?;
        build_baseline(
            engine,
            &tx,
            &sample_bytes,
            &fitc_bytes,
            &pe_bytes,
            &unst_bytes,
        )?;
        tx.commit().map_err(|e| e.to_string())?;
    }

    // Branches fork from the completed baseline before any branch edits.
    let base_head = engine.head_seq(BASELINE)?;
    engine.fork_branch(SINGULAR, "分支：近奇异补偿", BASELINE, Some(base_head))?;
    engine.fork_branch(REVISION, "分支：修订变换参数", BASELINE, Some(base_head))?;
    engine.fork_branch(RENAME, "分支：门嵌套改名", BASELINE, Some(base_head))?;

    apply_branch_edits(engine)?;
    Ok(())
}

fn build_baseline(
    engine: &Engine,
    conn: &Connection,
    sample: &[u8],
    fitc: &[u8],
    pe: &[u8],
    unst: &[u8],
) -> Result<(), String> {
    let b = BASELINE;
    insert_raw(engine, conn, b, "sample.fcs", sample, "sample")?;
    insert_control(engine, conn, b, "sst-fitc.fcs", fitc, "single_stain:FITC-A")?;
    insert_control(engine, conn, b, "sst-pe.fcs", pe, "single_stain:PE-A")?;
    insert_control(engine, conn, b, "sst-unstained.fcs", unst, "single_stain:none")?;

    engine.insert_authored_tx(
        conn,
        b,
        "spillover_versioned",
        "spill#main",
        &serde_json::to_value(baseline_spillover()).unwrap(),
    )?;
    engine.insert_authored_tx(
        conn,
        b,
        "transform_versioned",
        "transform#main",
        &serde_json::to_value(default_transform()).unwrap(),
    )?;
    for g in gates() {
        engine.insert_authored_tx(
            conn,
            b,
            "gate_versioned",
            &g.gate_id,
            &serde_json::to_value(&g).unwrap(),
        )?;
    }
    engine.insert_authored_tx(
        conn,
        b,
        "edges_versioned",
        "edges#main",
        &serde_json::to_value(edges()).unwrap(),
    )?;
    derive_all(engine, conn, b)?;
    Ok(())
}

/// Recompute compensation -> compensated events -> populations in dependency
/// order using internal helpers inside one transaction.
fn derive_all(engine: &Engine, conn: &Connection, branch: &str) -> Result<(), String> {
    let req_comp = engine.insert_request_tx(conn, branch, "compensation_requested")?;
    let produced = engine.produce_compensation(
        conn,
        branch,
        req_comp,
        &super::engine::ActionInput {
            branch_id: branch.to_string(),
            kind: "compensation_requested".to_string(),
            node_id: None,
            payload: Value::Null,
            parent_event: None,
            base_hash: None,
            chosen_candidate_id: None,
        },
    )?;
    if produced.is_none() {
        // near-singular branch: cannot derive further; candidates persisted.
        return Ok(());
    }
    engine.do_recompute_compensated(conn, branch, req_comp)?;
    for g in gates_order() {
        let req = engine.insert_request_tx(conn, branch, "population_requested")?;
        engine.do_recompute_population(conn, branch, g, req)?;
    }
    Ok(())
}

fn gates_order() -> [&'static str; 5] {
    [
        "gate#cells",
        "gate#lymph",
        "gate#cd3",
        "gate#cd4",
        "gate#cd8",
    ]
}

fn apply_branch_edits(engine: &Engine) -> Result<(), String> {
    // 1) Near-singular: revise only the spillover direct source, then request
    //    compensation. Inversion is withheld; ridge candidates are saved.
    let base = engine.head_hash(BASELINE)?;
    engine.apply(&super::engine::ActionInput {
        branch_id: SINGULAR.to_string(),
        kind: "spillover_revised".to_string(),
        node_id: Some("spill#main".to_string()),
        payload: serde_json::to_value(singular_spillover()).unwrap(),
        parent_event: None,
        base_hash: Some(base),
        chosen_candidate_id: None,
    })?;
    engine.apply(&super::engine::ActionInput {
        branch_id: SINGULAR.to_string(),
        kind: "compensation_requested".to_string(),
        node_id: None,
        payload: json!({}),
        parent_event: None,
        base_hash: None,
        chosen_candidate_id: None,
    })?;

    // 2) Transform revision: only fluorescence gates/populations go stale;
    //    FSC/SSC populations remain valid (local invalidation).
    let base = engine.head_hash(BASELINE)?;
    engine.apply(&super::engine::ActionInput {
        branch_id: REVISION.to_string(),
        kind: "transform_revised".to_string(),
        node_id: Some("transform#main".to_string()),
        payload: serde_json::to_value(revised_transform()).unwrap(),
        parent_event: None,
        base_hash: Some(base),
        chosen_candidate_id: None,
    })?;

    // 3) Rename within a nested gate: identity and lineage unchanged.
    let base = engine.head_hash(BASELINE)?;
    let renamed = GatePolygon {
        label: "CD3 positive (renamed)".to_string(),
        ..gates().into_iter().find(|g| g.gate_id == "gate#cd3").unwrap()
    };
    engine.apply(&super::engine::ActionInput {
        branch_id: RENAME.to_string(),
        kind: "gate_renamed".to_string(),
        node_id: Some("gate#cd3".to_string()),
        payload: serde_json::to_value(&renamed).unwrap(),
        parent_event: None,
        base_hash: Some(base),
        chosen_candidate_id: None,
    })?;
    Ok(())
}
