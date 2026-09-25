//! 离线案例包：编译期内嵌真实 FCS/领域 JSON；导入是单事务，失败整体回滚。
use crate::comp::{Matrix, Transform};
use crate::derive::{self, dep, parse_tree, TreeInfo};
use crate::domain::{Diag, Qty};
use crate::fcs;
use crate::polygon::Gate;
use crate::store::{FileRow, FcsParamRow, Store};
use rusqlite::Connection;
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub static CASE_DIR: include_dir::Dir = include_dir::include_dir!("cases/flow-boundary-chamber");

#[derive(Debug, Clone)]
pub struct Material {
    pub path: String,
    pub bytes: Vec<u8>,
}

pub fn embedded_materials() -> Vec<Material> {
    CASE_DIR
        .files()
        .iter()
        .map(|f| Material { path: f.path().to_string_lossy().replace('\\', "/"), bytes: f.contents().to_vec() })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub case_id: String,
    pub title: String,
    pub files: ManifestFiles,
}

#[derive(Debug, Deserialize)]
pub struct ManifestFiles {
    pub main_events: String,
    pub single_stain_controls: Vec<String>,
    pub spillover: String,
    pub matrix_v1: String,
    pub matrix_near_singular: String,
    pub transform_v1: String,
    pub gates: Vec<String>,
    pub tree_v1: String,
    pub revisions: Vec<String>,
}

pub struct CaseData {
    pub case_id: String,
    pub title: String,
    pub main_file: String,
    pub matrix: Matrix,
    pub xform: Transform,
    pub gates: Vec<Gate>,
    pub tree: TreeInfo,
}

fn sha(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// 事务式导入；任一步失败回滚，绝不留半份记录。
pub fn import_casepack(store: &Store, materials: &[Material]) -> anyhow::Result<CaseData> {
    let by_path = |p: &str| -> anyhow::Result<&[u8]> {
        materials
            .iter()
            .find(|m| m.path.ends_with(p))
            .map(|m| m.bytes.as_slice())
            .ok_or_else(|| anyhow::anyhow!("案例包缺少文件 {}", p))
    };
    let manifest: Manifest = serde_json::from_slice(by_path("manifest.json")?)?;
    let matrix: Matrix = serde_json::from_slice(by_path(&manifest.files.matrix_v1)?)?;
    let xform: Transform = serde_json::from_slice(by_path(&manifest.files.transform_v1)?)?;
    let mut gates = Vec::new();
    for g in &manifest.files.gates {
        gates.push(serde_json::from_slice::<Gate>(by_path(g)?)?);
    }
    let tree_json: serde_json::Value = serde_json::from_slice(by_path(&manifest.files.tree_v1)?)?;
    let tree = parse_tree(&tree_json);

    let tx = store.conn.unchecked_transaction()?;
    let result = import_inner(&store.conn, store, materials, &manifest, &matrix, &xform, &gates, &tree_json, &tree);
    match result {
        Ok(data) => {
            tx.commit()?;
            Ok(data)
        }
        Err(e) => {
            tx.rollback()?;
            Err(e)
        }
    }
}

fn import_inner(
    store: &Store,
    materials: &[Material],
    manifest: &Manifest,
    matrix: &Matrix,
    xform: &Transform,
    gates: &[Gate],
    tree_json: &serde_json::Value,
    tree: &TreeInfo,
) -> anyhow::Result<CaseData> {
    let by_path = |p: &str| -> anyhow::Result<&[u8]> {
        materials
            .iter()
            .find(|m| m.path.ends_with(p))
            .map(|m| m.bytes.as_slice())
            .ok_or_else(|| anyhow::anyhow!("案例包缺少文件 {}", p))
    };

    let start_evt = store.record_event(
        "case.import.started",
        &serde_json::json!({"case_id": manifest.case_id, "title": manifest.title, "atomic": true}),
    )?;

    // 1) 原始文件 + 事件（不做任何不可逆清洗；无法转换的值留在原位置并附诊断）
    let mut file_specs: Vec<(String, &str, &str)> = vec![("file:fcs:sample-main".into(), "fcs", &manifest.files.main_events)];
    for (i, p) in manifest.files.single_stain_controls.iter().enumerate() {
        let id = if p.contains("fl1") { "file:fcs:control-fl1" } else { "file:fcs:control-fl2" };
        let _ = i;
        file_specs.push((id.into(), "fcs", p.as_str()));
    }
    let mut main_file = String::new();
    for (file_id, kind, path) in &file_specs {
        let bytes = by_path(path)?.to_vec();
        let parsed = fcs::parse(file_id, &bytes)?;
        let evt = store.record_event(
            "file.imported",
            &serde_json::json!({
                "file_id": file_id, "name": path, "sha256": sha(&bytes),
                "kind": kind, "n_events": parsed.events.len(),
                "unit_note": "原始 counts 表示保留；派生层才做规范化"
            }),
        )?;
        store.insert_file(&FileRow {
            file_id: file_id.clone(),
            kind: kind.to_string(),
            name: path.to_string(),
            sha256: sha(&bytes),
            bytes: bytes.clone(),
            imported_event: evt,
        })?;
        for p in &parsed.params {
            store.insert_fcs_params(&FcsParamRow {
                file_id: file_id.clone(),
                index: p.index as i64,
                name: p.name.clone(),
                stain: p.stain.clone(),
                range: p.range,
                gain: p.gain.clone(),
                amp: p.amp.clone(),
                bits: p.bits,
            })?;
        }
        for ev in &parsed.events {
            store.insert_raw_event(ev)?;
            for d in &ev.diagnostics {
                let did = format!("diag:{}:{}", ev.event_id, d.code);
                store.insert_diagnostic(&crate::store::DiagRow {
                    diag_id: did,
                    object_id: ev.event_id.clone(),
                    diag: d.clone(),
                    event: evt,
                })?;
            }
        }
        if *file_id == "file:fcs:sample-main" {
            main_file = file_id.clone();
        }
    }

    // 2) spillover 元数据（与矩阵分开版本化）
    let spill: serde_json::Value = serde_json::from_slice(by_path(&manifest.files.spillover)?)?;
    let sp_evt = store.record_event("spillover.versioned", &serde_json::json!({
        "spillover_id": "spillover:main", "version": 1,
        "unit": spill["unit"], "basis": spill["basis"],
        "controls": spill["controls"],
    }))?;
    store.put_version(crate::store::KIND_SPILLOVER, "spillover:main", 1, &spill, sp_evt)?;

    // 3) 补偿矩阵 v1 / 变换 v1 / 门 v1 / 树 v1
    let m_evt = store.record_event("matrix.versioned", &serde_json::to_value(matrix)?, )?;
    store.put_version(crate::store::KIND_MATRIX, &matrix.matrix_id, 1, &serde_json::to_value(matrix)?, m_evt)?;
    let t_evt = store.record_event("transform.versioned", &serde_json::to_value(xform)?, )?;
    store.put_version(crate::store::KIND_TRANSFORM, &xform.transform_id, 1, &serde_json::to_value(xform)?, t_evt)?;
    for g in gates {
        let e = store.record_event("gate.versioned", &serde_json::to_value(g)?, )?;
        store.put_version(crate::store::KIND_GATE, &g.gate_id, 1, &serde_json::to_value(g)?, e)?;
    }
    // 根门（全集，无多边形）
    let root_json = serde_json::json!({
        "gate_id": "gate:root", "version": 1, "name": "All Events",
        "x_channel": "", "y_channel": "", "vertices": [],
        "basis": "root gate; includes every event of file:fcs:sample-main"
    });
    let r_evt = store.record_event("gate.versioned", &root_json)?;
    store.put_version(crate::store::KIND_GATE, "gate:root", 1, &root_json, r_evt)?;
    let tr_evt = store.record_event("tree.versioned", tree_json)?;
    store.put_version(crate::store::KIND_TREE, &tree.tree_id, 1, tree_json, tr_evt)?;

    // 4) 派生：补偿 → 变换 → 门成员 → 群体统计（main 分支）
    let branch_evt = store.record_event(
        "branch.created",
        &serde_json::json!({"branch_id": "branch:main", "parent": serde_json::Value::Null, "base_event": start_evt}),
    )?;
    let _ = branch_evt;
    let ws = crate::state::WorldState::default();

    let comp_row = derive::compute_compensated(store, &main_file, matrix, dep("matrix", &matrix.matrix_id, 1), start_evt)?;
    let comp_evt = store.record_event("derived.computed", &serde_json::json!({
        "derived_id": comp_row.derived_id, "version": comp_row.version,
        "status": comp_row.status, "deps": comp_row.deps, "kind": comp_row.kind,
    }))?;
    let mut comp_row = comp_row;
    comp_row.event = comp_evt;
    store.insert_derived(&comp_row)?;
    assert_eq!(comp_row.status, "ready", "基线矩阵必须良态");

    let xf_row = derive::compute_transformed(
        store, &main_file, xform, dep("transform", &xform.transform_id, 1),
        dep("derived", &comp_row.derived_id, comp_row.version), comp_evt, "branch:main", &ws,
    )?;
    let xf_evt = store.record_event("derived.computed", &serde_json::json!({
        "derived_id": xf_row.derived_id, "version": xf_row.version,
        "status": xf_row.status, "deps": xf_row.deps, "kind": xf_row.kind,
    }))?;
    let mut xf_row = xf_row;
    xf_row.event = xf_evt;
    store.insert_derived(&xf_row)?;

    for node in &tree.nodes {
        if node.gate_id == tree.root {
            continue;
        }
        let gate = gates.iter().find(|g| g.gate_id == node.gate_id)
            .ok_or_else(|| anyhow::anyhow!("树引用了未导入的门 {}", node.gate_id))?;
        let mem = derive::compute_gate_membership(
            store, gate,
            dep("gate", &gate.gate_id, 1),
            dep("derived", &xf_row.derived_id, xf_row.version),
            dep("tree", &tree.tree_id, 1),
            xf_evt,
        )?;
        let e = store.record_event("derived.computed", &serde_json::json!({
            "derived_id": mem.derived_id, "version": mem.version,
            "status": mem.status, "deps": mem.deps, "kind": mem.kind,
            "boundary_rows": mem.payload["boundary_rows"],
        }))?;
        let mut mem = mem;
        mem.event = e;
        store.insert_derived(&mem)?;
    }

    let stats = derive::compute_popstats(store, &ws, "branch:main", tree,
        dep("derived", &xf_row.derived_id, xf_row.version), xf_evt)?;
    for (st, pin) in &stats {
        let e = store.record_event("popstat.computed", &serde_json::json!({
            "stat_id": st.stat_id, "version": st.version, "gate_id": st.gate_id,
            "gate_version": st.gate_version, "count": st.count, "denom_count": st.denom_count,
            "pct": st.pct, "status": st.status, "parent_stat_id": st.parent_stat_id,
            "parent_stat_version": st.parent_stat_version, "deps": st.deps,
            "denominator_judgment": pin,
        }))?;
        let mut st = st.clone();
        st.event = e;
        store.insert_popstat(&st)?;
    }

    store.record_event("case.imported", &serde_json::json!({
        "case_id": manifest.case_id, "branch_id": "branch:main",
        "separately_versioned": ["raw_channels", "spillover", "matrix", "transform", "gates", "tree"],
    }))?;
    Ok(CaseData {
        case_id: manifest.case_id.clone(),
        title: manifest.title.clone(),
        main_file,
        matrix: matrix.clone(),
        xform: xform.clone(),
        gates: gates.to_vec(),
        tree: tree.clone(),
    })
}

/// 未使用的 Qty/Diag 引用（保留：该模块语义上负责单位/诊断的导入保证）。
fn _qty_ref(_q: Qty, _d: Diag) {}

/// 供测试：直接从磁盘目录构造 Material 列表。
pub fn materials_from_disk(dir: &std::path::Path) -> anyhow::Result<Vec<Material>> {
    let mut out = Vec::new();
    fn walk(base: &std::path::Path, out: &mut Vec<Material>) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(base)? {
            let p = entry?.path();
            if p.is_dir() {
                walk(&p, out)?;
            } else {
                let rel = p.strip_prefix(base)?.to_string_lossy().replace('\\', "/");
                out.push(Material { path: rel, bytes: std::fs::read(&p)? });
            }
        }
        Ok(())
    }
    walk(dir, &mut out)?;
    Ok(out)
}
