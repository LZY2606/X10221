//! 离线案例包加载：导入失败只回滚本次变更（单事务），不留半份记录。
//! 原始表示整体保留在 store payload 中；规范化发生在派生层。

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::db;

pub const CASE_DIR: &str = "cases/flow-boundary-chamber";

fn read_case_file(name: &str) -> Result<Value, String> {
    let path = format!("{}/{}", CASE_DIR, name);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {}", path, e))?;
    serde_json::from_str(&text).map_err(|e| format!("解析 {} 失败: {}", path, e))
}

/// 基线导入：六个存储各自版本化为 v1。门控草案拆成 gate_polygons 与 gate_relations 两个存储。
pub fn import_baseline(conn: &mut Connection, branch: &str) -> Result<Value, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let result = (|| -> Result<Value, String> {
        let fcs = read_case_file("fcs_events.json")?;
        let controls = read_case_file("single_stain_controls.json")?;
        let spill = read_case_file("spillover.json")?;
        let comp = read_case_file("comp_matrix.json")?;
        let xform = read_case_file("transforms.json")?;
        let draft = read_case_file("gating_draft.json")?;

        let mut imported = Vec::new();
        let mut put = |store: &str, id: &str, payload: &Value| -> Result<(), String> {
            let v = db::put_version(&tx, store, id, None, payload, branch).map_err(|e| e.to_string())?;
            imported.push(json!({"store": store, "id": id, "version": v}));
            Ok(())
        };
        put("raw_channels", fcs["id"].as_str().unwrap(), &fcs)?;
        put("raw_channels", controls["id"].as_str().unwrap(), &controls)?;
        put("spillover", spill["id"].as_str().unwrap(), &spill)?;
        put("comp_matrix", comp["id"].as_str().unwrap(), &comp)?;
        put("transform_params", xform["id"].as_str().unwrap(), &xform)?;
        put(
            "gate_polygons",
            draft["id"].as_str().unwrap(),
            &json!({"kind": "gate_polygons", "id": draft["id"], "gates": draft["gates"]}),
        )?;
        put(
            "gate_relations",
            "rels:draft-v1",
            &json!({"kind": "gate_relations", "id": "rels:draft-v1", "relations": draft["relations"]}),
        )?;
        db::emit_event(&tx, "case.baseline_imported", branch, json!({
            "case": "flow-boundary-chamber",
            "stores": imported,
            "note": "原始通道值、spillover 元数据、补偿矩阵、变换参数、门多边形、父子关系分开版本化"
        }))
        .map_err(|e| e.to_string())?;
        Ok(json!({"status": "imported", "stores": imported}))
    })();
    match result {
        Ok(v) => {
            tx.commit().map_err(|e| e.to_string())?;
            Ok(v)
        }
        Err(e) => {
            let _ = tx.rollback();
            Err(format!("导入失败已回滚本次变更: {}", e))
        }
    }
}
