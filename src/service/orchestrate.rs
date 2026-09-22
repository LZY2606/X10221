//! 用例编排：把“来源材料 + 解释版本 + 人工裁定”串成可追溯叙事。

use super::pipeline::{current_refs, CurrentRefs};
use crate::domain::{
    EntityKind, EntityVersion, EventPayload, GateLink, LineageSet, PolygonGate,
};
use crate::fcs::spill::SpillMatrix;
use crate::service::derive::{spill_from_version, transform_from_version};
use crate::store::ids::now_ms;
use crate::store::results::GateResultRow;
use crate::store::{err, Repo, StoreResult};
use rusqlite::OptionalExtension;
use serde_json::json;

/// 内嵌离线案例包（build.rs 生成 FCS；JSON 为检入文件）。
pub struct CaseFile {
    pub path: &'static str,
    pub bytes: &'static [u8],
}

pub fn case_files() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("baseline/main_tube.fcs", include_bytes!("../../cases/flow/baseline/main_tube.fcs")),
        ("baseline/sst_fitc.fcs", include_bytes!("../../cases/flow/baseline/sst_fitc.fcs")),
        ("baseline/sst_pe.fcs", include_bytes!("../../cases/flow/baseline/sst_pe.fcs")),
        ("baseline/sst_percp.fcs", include_bytes!("../../cases/flow/baseline/sst_percp.fcs")),
        ("baseline/spillover.v1.json", include_bytes!("../../cases/flow/baseline/spillover.v1.json")),
        ("baseline/transforms.v1.json", include_bytes!("../../cases/flow/baseline/transforms.v1.json")),
        ("baseline/gates.lymph.json", include_bytes!("../../cases/flow/baseline/gates.lymph.json")),
        ("baseline/lineage.v1.json", include_bytes!("../../cases/flow/baseline/lineage.v1.json")),
        ("revisions/transforms.v2.json", include_bytes!("../../cases/flow/revisions/transforms.v2.json")),
        ("revisions/spillover.v2_near_singular.json", include_bytes!("../../cases/flow/revisions/spillover.v2_near_singular.json")),
        ("revisions/spillover.v3_singular.json", include_bytes!("../../cases/flow/revisions/spillover.v3_singular.json")),
        ("revisions/gates.v2_rename.json", include_bytes!("../../cases/flow/revisions/gates.v2_rename.json")),
        ("revisions/main_tube_near_singular.fcs", include_bytes!("../../cases/flow/revisions/main_tube_near_singular.fcs")),
        ("revisions/main_tube_revised.fcs", include_bytes!("../../cases/flow/revisions/main_tube_revised.fcs")),
    ]
}

pub fn read_case(path: &str) -> Option<&'static [u8]> {
    case_files().into_iter().find(|(p, _)| *p == path).map(|(_, b)| b)
}

#[derive(Debug, Clone)]
pub struct ImportedCase {
    pub branch_id: String,
    pub material_ids: Vec<String>,
}

/// 加载基线案例到一个新分支：全部材料与 4 类解释在单个事务中，
/// 任何一份材料失败则整体回滚（不留半份记录）。
pub fn load_baseline(repo: &mut Repo, branch_name: &str) -> StoreResult<ImportedCase> {
    let branch = repo.create_branch(branch_name)?;
    let tx = repo.conn.transaction()?;
    let batch = format!("batch_{}", now_ms());
    Repo::append_event_tx(&tx, &branch.id, "importer", &EventPayload::ImportBegan {
        batch_id: batch.clone(),
    })?;

    let mut material_ids = Vec::new();
    let mut add = |tx: &rusqlite::Transaction,
                   path: &str,
                   filename: &str,
                   kind: &str,
                   label: &str|
     -> StoreResult<String> {
        let bytes = read_case(path)
            .ok_or_else(|| err("CASE_FILE_MISSING", path))?
            .to_vec();
        let (mat, _new) = Repo::insert_material_tx(tx, filename, kind, &bytes, label)?;
        // 保存 FCS 解析诊断（不丢弃）。
        if kind == "fcs" {
            let tube = crate::service::parse_tube(&bytes);
            for (i, d) in tube.diagnostics.iter().enumerate() {
                Repo::insert_diagnostic_tx(tx, &mat.id, i as i64, &d.code, &d.detail)?;
            }
        }
        material_ids.push(mat.id.clone());
        Ok(mat.id)
    };

    let main_id = add(&tx, "baseline/main_tube.fcs", "main_tube.fcs", "fcs", "主管（基线）")?;
    add(&tx, "baseline/sst_fitc.fcs", "sst_fitc.fcs", "fcs", "单染 FITC")?;
    add(&tx, "baseline/sst_pe.fcs", "sst_pe.fcs", "fcs", "单染 PE")?;
    add(&tx, "baseline/sst_percp.fcs", "sst_percp.fcs", "fcs", "单染 PerCP")?;
    let spill_mat = add(&tx, "baseline/spillover.v1.json", "spillover.v1.json", "spillover", "补偿矩阵 v1")?;
    let trans_mat = add(&tx, "baseline/transforms.v1.json", "transforms.v1.json", "transforms", "变换参数 v1")?;
    let gates_mat = add(&tx, "baseline/gates.lymph.json", "gates.lymph.json", "gates", "门草案 v1")?;
    let lineage_mat = add(&tx, "baseline/lineage.v1.json", "lineage.v1.json", "lineage", "父子关系 v1")?;

    // 4 类解释分别版本化。
    let spill_val: serde_json::Value =
        serde_json::from_slice(read_case("baseline/spillover.v1.json").unwrap()).unwrap();
    let spill_v = Repo::add_version_tx(&tx, &branch.id, super::versions::ProposedVersion {
        kind: EntityKind::Spillover,
        entity_id: "spill_main",
        data: &spill_val,
        source_material_id: Some(&spill_mat),
        note: "基线补偿矩阵（来自单染 $SPILLOVER）",
        expected_version: 0,
        actor: "importer",
    })?;

    let trans_doc: serde_json::Value =
        serde_json::from_slice(read_case("baseline/transforms.v1.json").unwrap()).unwrap();
    // gates 草案中 transform_version_id 是占位；版本化时解析 params（去掉 schema 包装）。
    let transform_params = trans_doc.get("params").cloned().unwrap_or(serde_json::Value::Null);
    let trans_v = Repo::add_version_tx(&tx, &branch.id, super::versions::ProposedVersion {
        kind: EntityKind::Transform,
        entity_id: "xform_main",
        data: &transform_params,
        source_material_id: Some(&trans_mat),
        note: "基线变换参数 asinh cofactor=150",
        expected_version: 0,
        actor: "importer",
    })?;

    let gates_doc: serde_json::Value =
        serde_json::from_slice(read_case("baseline/gates.lymph.json").unwrap()).unwrap();
    let gates_arr = gates_doc.get("gates").cloned().unwrap_or(json!([]));
    let gates: Vec<PolygonGate> = serde_json::from_value(gates_arr.clone()).unwrap();
    for g in &gates {
        let mut data = serde_json::to_value(g).unwrap();
        // 占位 transform 引用替换为实际版本 id。
        if let Some(b) = data.pointer_mut("/basis") {
            if b.get("transform_version_id").and_then(|v| v.as_str()) == Some("RESOLVED_AT_IMPORT") {
                b["transform_version_id"] = json!(trans_v.id);
            }
        }
        Repo::add_version_tx(&tx, &branch.id, super::versions::ProposedVersion {
            kind: EntityKind::Gate,
            entity_id: &g.gate_id,
            data: &data,
            source_material_id: Some(&gates_mat),
            note: &format!("基线条目门 {}", g.name),
            expected_version: 0,
            actor: "importer",
        })?;
    }

    let lineage_doc: serde_json::Value =
        serde_json::from_slice(read_case("baseline/lineage.v1.json").unwrap()).unwrap();
    let lineage_data = lineage_doc.get("links").cloned().unwrap_or(json!([]));
    let lineage_v = Repo::add_version_tx(&tx, &branch.id, super::versions::ProposedVersion {
        kind: EntityKind::Lineage,
        entity_id: "tree_main",
        data: &lineage_data,
        source_material_id: Some(&lineage_mat),
        note: "基线父子关系",
        expected_version: 0,
        actor: "importer",
    })?;

    tx.execute(
        "INSERT INTO branch_config(branch_id,key,value) VALUES(?1,'main_tube',?2)",
        rusqlite::params![branch.id, main_id],
    )?;

    // 首次群体计算（当前引用即刚写入版本），在同一事务内读取未提交材料。
    let gates_vers: Vec<EntityVersion> = gates
        .iter()
        .map(|g| {
            let entity_id = g.gate_id.clone();
            tx.query_row(
                "SELECT id,kind,entity_id,version,data,source_material_id,branch_id,parent_version_id,created_at,note
                 FROM entity_versions
                 WHERE branch_id=?1 AND kind='gate' AND entity_id=?2
                 ORDER BY version DESC LIMIT 1",
                rusqlite::params![branch.id, entity_id],
                |r| {
                    let data_s: String = r.get(4)?;
                    Ok(EntityVersion {
                        id: r.get(0)?,
                        kind: EntityKind::Gate,
                        entity_id: r.get(2)?,
                        version: r.get(3)?,
                        data: serde_json::from_str(&data_s).unwrap(),
                        source_material_id: r.get(5)?,
                        branch_id: r.get(6)?,
                        parent_version_id: r.get(7)?,
                        created_at: r.get(8)?,
                        note: r.get(9)?,
                    })
                },
            ).unwrap()
        })
        .collect();
    let refs = CurrentRefs {
        spill: spill_v,
        transform: trans_v,
        gates: gates_vers,
        lineage: lineage_v,
    };
    super::pipeline::compute_in_tx(&tx, &branch.id, &refs, &main_id, "importer")?;

    Repo::append_event_tx(&tx, &branch.id, "importer", &EventPayload::ImportCommitted {
        batch_id: batch,
        material_ids: material_ids.clone(),
    })?;
    tx.commit()?;
    Ok(ImportedCase {
        branch_id: branch.id,
        material_ids,
    })
}
