//! Offline case pack loader: "流式边界室：补偿矩阵、门控系谱与细胞群不确定性".
//! Case materials are domain artifacts (real FCS 3.0 files + spillover /
//! matrix / transform / gate JSON), never generic tables. A failed import is
//! fully rolled back — no half records are ever left behind.

use crate::fcs;
use crate::pipeline::{self, PipeError};
use crate::store;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn default_case_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("cases/flow-boundary-lab")
}

/// Parse everything BEFORE touching the database; then persist in one
/// transaction so any failure rolls back the whole import.
pub fn load_baseline(conn: &Connection, case_dir: &Path) -> Result<Value, String> {
    let manifest_path = case_dir.join("manifest.json");
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("read {}: {e}", manifest_path.display()))?,
    )
    .map_err(|e| e.to_string())?;
    let base = &manifest["baseline"];

    // ---- pre-parse stage (no DB writes yet) ----
    struct FcsInput {
        sample_id: String,
        filename: String,
        bytes: Vec<u8>,
        parsed: fcs::FcsFile,
    }
    let mut fcs_inputs = Vec::new();
    for spec in base["fcs"].as_array().ok_or("manifest.baseline.fcs missing")? {
        let filename = spec["file"].as_str().ok_or("fcs.file missing")?;
        let bytes = std::fs::read(case_dir.join("data").join(filename))
            .map_err(|e| format!("read {filename}: {e}"))?;
        let parsed = match fcs::parse(&bytes) {
            Ok(f) => f,
            Err(e) => {
                emit_rollback_note(conn, "fcs.parse", filename, &e)?;
                return Err(e);
            }
        };
        fcs_inputs.push(FcsInput {
            sample_id: spec["id"].as_str().ok_or("fcs.id missing")?.to_string(),
            filename: filename.to_string(),
            bytes,
            parsed,
        });
    }
    let read_json = |rel: &str| -> Result<Value, String> {
        let p = case_dir.join(rel);
        serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| format!("{rel}: {e}"))?)
            .map_err(|e| format!("parse {rel}: {e}"))
    };
    let spillover = read_json(base["spillover_file"].as_str().ok_or("spillover_file")?)?;
    let matrix = read_json(base["matrix_file"].as_str().ok_or("matrix_file")?)?;
    let transform = read_json(base["transform_file"].as_str().ok_or("transform_file")?)?;
    let gates_json = read_json(base["gates_file"].as_str().ok_or("gates_file")?)?;
    let hierarchy = read_json(base["hierarchy_file"].as_str().ok_or("hierarchy_file")?)?;

    // ---- persistence stage (single transaction; failure => full rollback) ----
    {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        store::ensure_branch(&tx, "main", None).ok();
        for inp in &fcs_inputs {
            let (blob_id, checksum) = store::store_blob(&tx, &inp.filename, &inp.bytes)?;
            let payload = json!({
                "source_file": inp.filename,
                "blob": {"id": blob_id, "checksum": checksum},
                "fcs_version": inp.parsed.version,
                "text_keywords": inp.parsed.text,
                "params": inp.parsed.params,
                "events": inp.parsed.events,
                "unit": "ADC channels",
                "basis": "raw DATA segment values as acquired (no cleaning)",
            });
            store::put_object(&tx, "main", &inp.sample_id, "raw_channels", payload,
                &format!("imported {}", inp.filename))?;
        }
        store::put_object(&tx, "main", "spillover:main", "spillover", spillover,
            "single-stain spillover metadata")?;
        let mid = matrix["id"].as_str().unwrap_or("matrix:main").to_string();
        store::put_object(&tx, "main", &mid, "comp_matrix", matrix,
            "baseline compensation matrix")?;
        let tid = transform["id"].as_str().unwrap_or("transform:default").to_string();
        store::put_object(&tx, "main", &tid, "transform", transform,
            "asinh display transforms")?;
        for g in gates_json["gates"].as_array().ok_or("gates missing")? {
            let gid = g["id"].as_str().ok_or("gate id missing")?.to_string();
            store::put_object(&tx, "main", &gid, "gate_polygon", g.clone(),
                g["note"].as_str().unwrap_or("gate polygon"))?;
        }
        let hid = hierarchy["id"].as_str().unwrap_or("hierarchy:main").to_string();
        store::put_object(&tx, "main", &hid, "gate_hierarchy", hierarchy,
            "gate parent-child relations")?;

        // pipeline on the main sample
        let sample = base["sample"].as_str().ok_or("baseline.sample missing")?;
        pipeline::apply_compensation(&tx, "main", sample, &mid, Some(1)).map_err(pipe_err)?;
        pipeline::apply_transforms(&tx, "main", sample, &tid).map_err(pipe_err)?;
        pipeline::run_gating(&tx, "main", sample, Some(&hid)).map_err(pipe_err)?;

        store::emit(&tx, "main", "case.baseline_loaded", json!({
            "title": manifest["title"],
            "case_dir": case_dir.display().to_string(),
            "samples": fcs_inputs.iter().map(|f| f.sample_id.clone()).collect::<Vec<_>>(),
            "matrix_id": mid,
        }))?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(json!({"status": "loaded", "manifest": manifest["title"]}))
}

fn pipe_err(e: PipeError) -> String {
    match e {
        PipeError::Db(s) => s,
        PipeError::Rejected(v) => format!("rejected: {v}"),
    }
}

fn emit_rollback_note(conn: &Connection, stage: &str, target: &str, err: &str) -> Result<(), String> {
    store::emit(
        conn,
        "main",
        "import.rolled_back",
        json!({"stage": stage, "target": target, "error": err,
               "rule": "import failure rolls back this import only; no half records remain"}),
    )?;
    Ok(())
}

/// Named replay scenarios. Each leaves input fragment, judgment status and
/// affected objects in the event log.
pub fn scenario(conn: &Connection, name: &str, case_dir: &Path) -> Result<Value, String> {
    match name {
        "singular_matrix" => scenario_singular(conn, case_dir),
        "boundary_points" => scenario_boundary(conn),
        "extreme_signal" => scenario_extreme(conn),
        "rename_nested_gate" => scenario_rename(conn, case_dir),
        "upstream_revision" => scenario_upstream(conn, case_dir),
        other => Err(format!("unknown scenario {other}")),
    }
}

fn scenario_singular(conn: &Connection, case_dir: &Path) -> Result<Value, String> {
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(
        case_dir.join("manifest.json"),
    )?)
    .map_err(|e| e.to_string())?;
    let rel = manifest["scenarios"]["singular_matrix"]["file"].as_str().ok_or("scenario file")?;
    let matrix: Value = serde_json::from_str(&std::fs::read_to_string(case_dir.join(rel))?)
        .map_err(|e| e.to_string())?;
    let mid = matrix["id"].as_str().ok_or("matrix id")?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let obj = store::put_object(&tx, "main", mid, "comp_matrix", matrix.clone(),
        "near-singular candidate matrix")?;
    store::emit(&tx, "main", "case.scenario.started",
        json!({"scenario": "singular_matrix", "matrix_id": mid, "matrix_version": obj.version}))?;
    let sample = manifest["baseline"]["sample"].as_str().unwrap_or("sample_events");
    let res = pipeline::apply_compensation(&tx, "main", sample, mid, Some(obj.version));
    tx.commit().map_err(|e| e.to_string())?;
    match res {
        Ok(_) => Err("near-singular matrix was compensated — the rule must block this".into()),
        Err(PipeError::Rejected(v)) => Ok(json!({"scenario": "singular_matrix", "status": "rejected", "evidence": v})),
        Err(PipeError::Db(e)) => Err(e),
    }
}

fn scenario_boundary(conn: &Connection) -> Result<Value, String> {
    let evs: Vec<Value> = store::list_events(conn, Some("main"))
        .into_iter()
        .filter(|e| e.kind == "gate.boundary_candidates")
        .map(|e| json!({"seq": e.seq, "id": e.id, "payload": e.payload}))
        .collect();
    Ok(json!({"scenario": "boundary_points", "boundary_events": evs,
              "instruction": "adjudicate via POST /api/adjudicate with an event seq and choice inside|outside"}))
}

fn scenario_extreme(conn: &Connection) -> Result<Value, String> {
    let tid = store::list_latest_by_kind(conn, "main", "transform")
        .first()
        .map(|o| o.id.clone())
        .ok_or_else(|| "no transform object".to_string())?;
    // re-running transforms is idempotent w.r.t. events (deduped); it proves
    // the extreme raw values are preserved in place with diagnostics.
    let disp = pipeline::apply_transforms(conn, "main", "sample_events", &tid)
        .map_err(pipe_err)?;
    Ok(json!({"scenario": "extreme_signal", "display_object": {"id": disp.id, "version": disp.version},
              "flags": disp.payload["flags"]}))
}

fn scenario_rename(conn: &Connection, case_dir: &Path) -> Result<Value, String> {
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(
        case_dir.join("manifest.json"),
    )?)
    .map_err(|e| e.to_string())?;
    let gate_id = manifest["scenarios"]["rename_nested_gate"]["gate_id"].as_str().ok_or("gate_id")?;
    let new_name = manifest["scenarios"]["rename_nested_gate"]["new_name"].as_str().ok_or("new_name")?;
    let g = pipeline::rename_gate(conn, "main", gate_id, new_name)?;
    Ok(json!({"scenario": "rename_nested_gate", "gate_id": gate_id, "new_version": g.version, "new_name": new_name}))
}

fn scenario_upstream(conn: &Connection, case_dir: &Path) -> Result<Value, String> {
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(
        case_dir.join("manifest.json"),
    )?)
    .map_err(|e| e.to_string())?;
    let spec = &manifest["scenarios"]["upstream_revision"];
    let gate_id = spec["gate_id"].as_str().ok_or("gate_id")?;
    let rel = spec["file"].as_str().ok_or("file")?;
    let revised: Value = serde_json::from_str(&std::fs::read_to_string(case_dir.join(rel))?)
        .map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let g = store::put_object(&tx, "main", gate_id, "gate_polygon", revised,
        "upstream gate polygon revised")?;
    store::emit(&tx, "main", "case.scenario.started",
        json!({"scenario": "upstream_revision", "gate_id": gate_id, "gate_version": g.version}))?;
    let affected = pipeline::mark_stale(&tx, "main", gate_id, g.version, "polygon_revised")?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(json!({"scenario": "upstream_revision", "gate_id": gate_id,
              "new_version": g.version, "affected_populations": affected,
              "rule": "downstream results marked stale, old versions retained"}))
}

/// Create a draft branch forking from `from`: copies the latest object versions
/// so the draft is independently editable while provenance stays linked.
pub fn fork_branch(conn: &Connection, name: &str, from: &str) -> Result<Value, String> {
    if name == from {
        return Err("draft name must differ from source branch".into());
    }
    if store::list_branches(conn).iter().any(|(n, _)| n == name) {
        return Err(format!("branch {name} already exists"));
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let fork_seq = store::branch_head(&tx, from);
    let mut copied = 0;
    let latest: Vec<(_, i64, String, String, String, String, i64, String)> = {
        let mut out = Vec::new();
        for kind in ["raw_channels","spillover","comp_matrix","transform","gate_polygon",
                     "gate_hierarchy","derived_compensated","derived_display","population_result"] {
            for o in store::list_latest_by_kind(&tx, from, kind) {
                out.push((o.id, o.version, from.to_string(), o.kind, o.status,
                          o.payload.to_string(), o.created_seq, o.note));
            }
        }
        out
    };
    for (id, version, _from, kind, status, payload, created_seq, note) in &latest {
        tx.execute(
            "INSERT INTO objects(id,version,branch,kind,status,payload,created_seq,note)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![id, version, name, kind, status, payload, created_seq, note],
        )
        .map_err(|e| e.to_string())?;
        copied += 1;
    }
    store::ensure_branch(&tx, name, Some((from.to_string(), fork_seq)))?;
    let ev = store::emit(&tx, name, "branch.forked",
        json!({"from": from, "fork_seq": fork_seq, "copied_objects": copied}))?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(json!({"branch": name, "from": from, "fork_seq": fork_seq,
              "copied_objects": copied, "head": ev.seq}))
}
