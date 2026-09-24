//! 离线案例包加载：事务性导入，失败只回滚本次变更，不留半份记录。
//! 导入只保存原始表示；派生层（诊断、门控分类、群体百分比）单独计算并记录事件。
use crate::domain;
use crate::store;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::Path;

pub struct LoadOutcome {
    pub populations: Vec<Value>,
    pub diagnostics: Vec<Value>,
    pub boundary_candidates: Vec<Value>,
}

fn read_json(dir: &Path, name: &str) -> Result<Value, String> {
    let p = dir.join(name);
    let s = std::fs::read_to_string(&p).map_err(|e| format!("读取 {} 失败: {e}", p.display()))?;
    serde_json::from_str(&s).map_err(|e| format!("解析 {} 失败: {e}", p.display()))
}

pub fn load_case(conn: &Connection, dir: &Path, branch: &str) -> Result<LoadOutcome, String> {
    let manifest = read_json(dir, "manifest.json")?;
    let detector_max = manifest["detector_max"].as_f64().unwrap_or(262144.0);
    let files = manifest["files"].clone();
    let fcs = read_json(dir, files["fcs_events"].as_str().unwrap_or("fcs_events.json"))?;
    let spill = read_json(dir, files["spillover"].as_str().unwrap_or("spillover.json"))?;
    let comp = read_json(dir, files["comp_matrix"].as_str().unwrap_or("comp_matrix.json"))?;
    let xform = read_json(dir, files["transforms"].as_str().unwrap_or("transforms.json"))?;
    let gates = read_json(dir, files["gates"].as_str().unwrap_or("gates.json"))?;
    let hier = read_json(dir, files["hierarchy"].as_str().unwrap_or("hierarchy.json"))?;

    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;

    // 1) 六类材料分开版本化，各自 version=1，互不影响。
    let seq = store::append_event(&tx, "case.imported", &json!({
        "case_id": manifest["case_id"], "title": manifest["title"], "branch": branch,
    }))
    .map_err(|e| e.to_string())?;
    let put = |id: &str, kind: &str, payload: &Value| -> Result<(), String> {
        store::put_entity(&tx, id, kind, 1, branch, payload, None, seq).map_err(|e| e.to_string())
    };
    put(fcs["id"].as_str().unwrap_or("raw:fcs"), "raw_events", &fcs)?;
    put(spill["id"].as_str().unwrap_or("spill:controls"), "spillover", &spill)?;
    for m in comp["matrices"].as_array().cloned().unwrap_or_default() {
        put(m["id"].as_str().unwrap_or("comp:?"), "comp_matrix", &m)?;
    }
    put(xform["id"].as_str().unwrap_or("xform:fl"), "transform", &xform)?;
    for g in gates["gates"].as_array().cloned().unwrap_or_default() {
        put(g["id"].as_str().unwrap_or("gate:?"), "gate", &g)?;
    }
    put(hier["id"].as_str().unwrap_or("tree:gating"), "hierarchy", &hier)?;

    // 2) 派生层：极端信号诊断（原始值留在原位置）。
    let mut diagnostics = Vec::new();
    let events = fcs["events"].as_array().cloned().unwrap_or_default();
    for ev in &events {
        let idx = ev["idx"].as_i64().unwrap_or(0);
        for (ch, raw) in ev.as_object().cloned().unwrap_or_default() {
            if ch == "idx" {
                continue;
            }
            let raw_s = raw.as_str().unwrap_or("").to_string();
            let check = domain::check_signal(&raw_s, detector_max);
            if let Some(msg) = &check.diagnostic {
                let diag_id = format!("diag:evt{idx}:{ch}");
                let sev = if msg.starts_with("signal_out_of_range") { "warn" } else { "error" };
                let eseq = store::append_event(&tx, "diagnostic.flagged", &json!({
                    "diag_id": diag_id, "event_idx": idx, "channel": ch, "message": msg,
                }))
                .map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO diagnostics(diag_id,subject,severity,message,raw_value,event_seq)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    rusqlite::params![diag_id, format!("evt:{idx}"), sev, msg, raw_s, eseq],
                )
                .map_err(|e| e.to_string())?;
                diagnostics.push(json!({"diag_id": diag_id, "channel": ch, "message": msg, "raw": raw_s}));
            }
        }
    }

    // 3) 门控分类 + 群体百分比（分母钉住父群版本）。边界点产生 include/exclude 双候选。
    let relations = hier["relations"].as_array().cloned().unwrap_or_default();
    let gate_list = gates["gates"].as_array().cloned().unwrap_or_default();
    let mut boundary_candidates = Vec::new();
    let mut populations = Vec::new();
    // 自根向下计算；父群成员集合用于分母。
    let mut membership: std::collections::HashMap<String, Vec<i64>> = Default::default();
    let all_idx: Vec<i64> = events.iter().map(|e| e["idx"].as_i64().unwrap_or(0)).collect();
    membership.insert("root".to_string(), all_idx.clone());

    // 按层级排序：父先子后（案例包内层级浅，拓扑一次即可）
    let mut ordered: Vec<Value> = Vec::new();
    let mut remaining = gate_list.clone();
    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|g| {
            let gid = g["id"].as_str().unwrap_or("");
            let parent = relations
                .iter()
                .find(|r| r["child"].as_str() == Some(gid))
                .and_then(|r| r["parent"].as_str().map(|s| s.to_string()));
            let ready = match &parent {
                None => true,
                Some(p) => ordered.iter().any(|o| o["id"].as_str() == Some(p.as_str())),
            };
            if ready {
                ordered.push(g.clone());
            }
            !ready
        });
        if remaining.len() == before {
            return Err("hierarchy 存在环，无法拓扑排序".into());
        }
    }

    for g in &ordered {
        let gid = g["id"].as_str().unwrap_or("").to_string();
        let axes: Vec<String> = g["axes"].as_array().cloned().unwrap_or_default()
            .iter().filter_map(|a| a.as_str().map(|s| s.to_string())).collect();
        let poly: Vec<(f64, f64)> = g["polygon"].as_array().cloned().unwrap_or_default()
            .iter()
            .filter_map(|p| {
                let a = p.as_array()?;
                Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
            })
            .collect();
        let parent_gate = relations
            .iter()
            .find(|r| r["child"].as_str() == Some(gid.as_str()))
            .and_then(|r| r["parent"].as_str().map(|s| s.to_string()));
        let parent_pop = parent_gate
            .as_ref()
            .map(|pg| format!("pop:{}", pg.trim_start_matches("gate:")))
            .unwrap_or_else(|| "root".to_string());
        let parent_members = membership.get(&parent_pop).cloned().unwrap_or_else(|| all_idx.clone());

        let mut members = Vec::new();
        for idx in &parent_members {
            let ev = events.iter().find(|e| e["idx"].as_i64() == Some(*idx)).cloned().unwrap_or(Value::Null);
            let x = ev[axes[0].as_str()].as_str().and_then(|s| s.parse::<f64>().ok());
            let y = ev[axes[1].as_str()].as_str().and_then(|s| s.parse::<f64>().ok());
            let (Some(x), Some(y)) = (x, y) else { continue }; // 无法转换的值不参与分类，留在原位置
            match domain::classify_point(&poly, x, y) {
                domain::GateHit::Inside => members.push(*idx),
                domain::GateHit::Outside => {}
                domain::GateHit::OnBoundary => {
                    // 无法唯一决定：保留 include/exclude 两个合法候选。
                    let subj = format!("gate:{gid}:evt{idx}");
                    for verdict in ["include", "exclude"] {
                        let cid = format!("cand:{}:{verdict}", subj.replace(':', "_"));
                        let eseq = store::append_event(&tx, "candidate.recorded", &json!({
                            "cand_id": cid, "subject": subj, "verdict_option": verdict,
                        }))
                        .map_err(|e| e.to_string())?;
                        tx.execute(
                            "INSERT INTO candidates(cand_id,subject,payload,status,branch,event_seq)
                             VALUES (?1,?2,?3,'pending',?4,?5)",
                            rusqlite::params![cid, subj, json!({"option": verdict, "event_idx": idx, "gate": gid}).to_string(), branch, eseq],
                        )
                        .map_err(|e| e.to_string())?;
                        boundary_candidates.push(json!({"cand_id": cid, "option": verdict, "event_idx": idx}));
                    }
                }
            }
        }
        membership.insert(format!("pop:{}", gid.trim_start_matches("gate:")), members.clone());

        // 分母钉住父群版本：denominator_ref 显式引用父群体与其门版本。
        let parent_gate_ver: i64 = match &parent_gate {
            Some(pg) => store::latest_version(&tx, pg, branch)
                .map_err(|e| e.to_string())?
                .map(|(v, _)| v)
                .unwrap_or(1),
            None => 0,
        };
        let denominator_ref = match &parent_pop[..] {
            "root" => "raw:fcs-run-0417@1".to_string(),
            pp => format!("{pp}#{}@{}", parent_gate.clone().unwrap_or_default(), parent_gate_ver),
        };
        let pct = domain::percent_of(members.len(), parent_members.len());
        let pop_id = format!("pop:{}", gid.trim_start_matches("gate:"));
        let gate_ref = format!("{gid}@1");
        let eseq = store::append_event(&tx, "population.computed", &json!({
            "pop_id": pop_id, "gate_ref": gate_ref, "parent_pop_ref": parent_pop,
            "denominator_ref": denominator_ref, "percent": pct, "members": members,
        }))
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO populations(pop_id,gate_ref,parent_pop_ref,percent,denominator_ref,status,branch,event_seq)
             VALUES (?1,?2,?3,?4,?5,'current',?6,?7)",
            rusqlite::params![
                pop_id,
                gate_ref,
                if parent_pop == "root" { None } else { Some(parent_pop.clone()) },
                pct,
                denominator_ref,
                branch,
                eseq
            ],
        )
        .map_err(|e| e.to_string())?;
        populations.push(json!({
            "pop_id": pop_id, "gate_ref": gate_ref, "percent": pct,
            "denominator_ref": denominator_ref, "members": members,
        }));
    }

    tx.commit().map_err(|e| e.to_string())?;
    Ok(LoadOutcome { populations, diagnostics, boundary_candidates })
}
