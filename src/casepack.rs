//! 仓库自带的离线案例包「流式边界室：补偿矩阵、门控系谱与细胞群不确定性」。
//! 材料在编译期内嵌，不依赖网络或通用表格：FCS 原始字节按字节保留，
//! 六类对象分开版本化，replay 实际经过六类边界情形。

use crate::actions::{
    self, Adjudication, CommitConflict, NewArtifact,
};
use crate::fcs;
use crate::model::{ArtifactKind, EventKind};
use crate::store::Store;
use base64::Engine;
use serde_json::{json, Value};

pub const CASE_JSON: &str = include_str!("../cases/flow-boundary-chamber/case.json");
pub const FCS_BYTES: &[u8] = include_bytes!("../cases/flow-boundary-chamber/events.fcs");

pub fn case() -> Value {
    serde_json::from_str(CASE_JSON).expect("case.json 非法")
}

/// 初始导入：六类对象分开版本化；全部在一个事务（多次提交各自事务，
/// 单次导入失败不会有半成品 —— 见 `load_case` 的整体回滚）。
pub fn load_case(store: &mut Store, actor: &str) -> Result<Vec<String>, String> {
    if !store.load_events().map_err(|e| e.0)?.is_empty() {
        return Err("数据库非空；请先导出并清空再载入案例".into());
    }
    let c = case();
    let fcs = fcs::parse(FCS_BYTES).map_err(|e| format!("FCS 解析失败：{e}（导入回滚）"))?;

    let raw_b64 = base64::engine::general_purpose::STANDARD.encode(FCS_BYTES);

    let mut channel_values = Vec::new();
    for (ei, row) in fcs.events.iter().enumerate() {
        let vals: Vec<Value> = row
            .iter()
            .zip(fcs.params.iter())
            .map(|(v, p)| {
                json!({
                    "channel": p.name,
                    "raw_value": *v as f64,
                    "raw_repr": format!("{v}"),
                    "range": p.range,
                    "amplification": p.amplification,
                    "unit": channel_unit(&c, &p.name),
                    "basis": "linear",
                })
            })
            .collect();
        channel_values.push(json!({"event_index": ei, "values": vals}));
    }

    let raw_payload = json!({
        "source_file": "cases/flow-boundary-chamber/events.fcs",
        "format": "FCS3.0",
        "total_events": fcs.n_events,
        "text_segment": fcs.raw_text,
        "channels": c["channels"],
        "events": channel_values,
    });

    // 逐个提交；任何失败都向上抛，调用方可 wipe 后重试，不会产生半份有效状态。
    commit(store, NewArtifact {
        id: "raw-channels".into(), kind: ArtifactKind::RawChannels,
        branch: "main".into(), payload: raw_payload,
        raw_b64: Some(raw_b64), base_version: Some(0),
    }, actor)?;
    commit(store, NewArtifact {
        id: "spillover-meta".into(), kind: ArtifactKind::SpilloverMeta,
        branch: "main".into(), payload: c["spillover_meta"].clone(),
        raw_b64: None, base_version: Some(0),
    }, actor)?;
    commit(store, NewArtifact {
        id: "comp-matrix".into(), kind: ArtifactKind::CompMatrix,
        branch: "main".into(),
        payload: json!({
            "channels": c["comp_matrix"]["channels"],
            "matrix": c["comp_matrix"]["matrix"],
            "unit": c["comp_matrix"]["unit"],
            "basis": c["comp_matrix"]["basis"],
            "note": c["comp_matrix"]["note"],
        }),
        raw_b64: None, base_version: Some(0),
    }, actor)?;
    commit(store, NewArtifact {
        id: "transform-params".into(), kind: ArtifactKind::TransformParams,
        branch: "main".into(), payload: c["transform_params"].clone(),
        raw_b64: None, base_version: Some(0),
    }, actor)?;

    let mut gate_ids = Vec::new();
    for g in c["gates"].as_array().unwrap() {
        let gid = g["gate_id"].as_str().unwrap();
        gate_ids.push(gid.to_string());
        commit(store, NewArtifact {
            id: format!("gate:{gid}"), kind: ArtifactKind::GatePolygon,
            branch: "main".into(), payload: g.clone(),
            raw_b64: None, base_version: Some(0),
        }, actor)?;
    }
    commit(store, NewArtifact {
        id: "gate-hierarchy".into(), kind: ArtifactKind::GateHierarchy,
        branch: "main".into(),
        payload: json!({
            "root": c["hierarchy"]["root"],
            "edges": c["hierarchy"]["edges"],
            "order": ["ALL", "P1", "P3", "P2"],
            "names": {"ALL": "ALL", "P1": "Lymphocytes", "P3": "SSC-hi", "P2": "Dye-positive"},
        }),
        raw_b64: None, base_version: Some(0),
    }, actor)?;

    let ev = store
        .append_event(EventKind::CaseImported, "main", actor, json!({
            "case": {
                "case_id": c["case_id"],
                "title": c["title"],
                "narrative": c["narrative"],
            },
            "artifacts": ["raw-channels","spillover-meta","comp-matrix","transform-params",
                          "gate:P1","gate:P3","gate:P2","gate-hierarchy"],
            "separately_versioned": true,
            "script": c["replay_script"],
        }))
        .map_err(|e| e.0)?;
    Ok(vec![ev.id])
}

fn channel_unit(c: &Value, name: &str) -> Value {
    c["channels"]
        .as_array()
        .and_then(|a| a.iter().find(|d| d["name"] == name).map(|d| d["unit"].clone()))
        .unwrap_or(Value::Null)
}

fn commit(store: &mut Store, a: NewArtifact, actor: &str) -> Result<(), String> {
    actions::commit_artifact(store, a, actor).map(|_| ()).map_err(|cf: CommitConflict| {
        format!("初始导入冲突：{} v{} vs v{}", cf.artifact, cf.base_version, cf.head_version)
    })
}

/// 按案例脚本重放，返回每一步产生的事件/判定摘要。
pub fn replay(store: &mut Store, actor: &str) -> Result<Vec<Value>, String> {
    let c = case();
    let mut log = Vec::new();

    // 防止重复重放：仅在案例刚导入后运行
    let kinds: Vec<String> = store
        .load_events()
        .map_err(|e| e.0)?
        .iter()
        .map(|e| e.kind.as_str().to_string())
        .collect();
    if kinds.iter().filter(|k| k.as_str() == "derivation_computed").count() > 0 {
        return Err("案例已经重放；请清空数据库后重新载入".into());
    }

    // step 2 矩阵奇异
    let ev = actions::derive_compensation(store, "main", actor).map_err(|e| e.0)?;
    log.push(json!({"step": "derive_compensation", "event": ev.id,
                    "result": ev.payload.get("diagnostic").map(|d| d["code"].clone())
                        .unwrap_or(json!("derived"))}));

    // step 3 人工裁定：采纳 λ=1e-3（cand-tikhonov-2），其余候选未采纳但留档
    let diag_comp = open_diagnostic(store, "NearSingularCompMatrix")?;
    let evs = actions::adjudicate(store, "main", actor, Adjudication {
        diagnostic_id: diag_comp.clone(),
        action: "accept".into(),
        candidate_id: Some("cand-tikhonov-2".into()),
        params: None,
        rationale: "近奇异矩阵不可静默求逆；选择 λ=1e-3 Tikhonov 候选并登记来源".into(),
    }).map_err(|e| e.0)?;
    log.push(json!({"step": "adjudicate_compensation",
                    "diagnostic": diag_comp,
                    "events": evs.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
                    "adopted": "cand-tikhonov-2",
                    "not_adopted": ["cand-tikhonov-1", "cand-tikhonov-3"]}));

    // step 4 补偿派生在裁定中已完成（含极端信号/不可转换通道诊断）
    log.push(json!({"step": "derive_events", "result": "comp-events",
                    "diagnostics": ["ExtremeSignal", "UnconvertibleChannel"]}));

    // step 5 门控派生：触发边界点诊断（P1 有两个边界事件）
    let evs = actions::derive_populations(store, "main", actor).map_err(|e| e.0)?;
    log.push(json!({"step": "derive_populations",
                    "events": evs.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
                    "result": "BoundaryMembership candidates retained"}));

    // step 6 边界裁定：两个边界点计入门内
    let open_diags = open_diagnostics(store, "BoundaryMembership");
    for d in &open_diags {
        actions::adjudicate(store, "main", actor, Adjudication {
            diagnostic_id: d.clone(),
            action: "accept".into(),
            candidate_id: Some("cand-include".into()),
            params: None,
            rationale: "边界点经审阅确认为门内，保留候选 cand-exclude 于档案".into(),
        }).map_err(|e| e.0)?;
    }
    log.push(json!({"step": "adjudicate_boundary", "diagnostics": open_diags,
                    "adopted": "cand-include", "not_adopted": ["cand-exclude"]}));

    // step 7 参数修订：cofactor 150 → 400
    let mut trans2 = c["transform_params"].clone();
    if let Some(obj) = trans2.as_object_mut() {
        obj.insert("channels".into(), c["transform_params_v2"]["channels"].clone());
        obj.insert("note".into(), c["transform_params_v2"]["note"].clone());
    }
    let ev = actions::commit_artifact(store, NewArtifact {
        id: "transform-params".into(), kind: ArtifactKind::TransformParams,
        branch: "main".into(), payload: trans2, raw_b64: None,
        base_version: Some(1),
    }, actor).map_err(|e| conflict_msg(&e))?;
    log.push(json!({"step": "revise_transform", "event": ev.id,
                    "invalidation": ev.payload}));
    // 参数修订后重算补偿与种群 → 新派生版本，旧版本过期留存
    actions::derive_compensation(store, "main", actor).map_err(|e| e.0)?;
    actions::derive_populations(store, "main", actor).map_err(|e| e.0)?;

    // step 8 局部源数据改动：仅 spillover 元数据 v2
    let mut spill2 = c["spillover_meta"].clone();
    if let Some(o) = spill2.as_object_mut() {
        if let Some(p) = c["spillover_meta_v2"]["patch"].as_object() {
            for (k, v) in p {
                o.insert(k.clone(), v.clone());
            }
        }
        o.insert("note".into(), c["spillover_meta_v2"]["note"].clone());
    }
    let ev = actions::commit_artifact(store, NewArtifact {
        id: "spillover-meta".into(), kind: ArtifactKind::SpilloverMeta,
        branch: "main".into(), payload: spill2, raw_b64: None,
        base_version: Some(1),
    }, actor).map_err(conflict_msg)?;
    log.push(json!({"step": "local_source_change", "event": ev.id,
                    "scope": "comp-events/populations only; gate artifacts untouched"}));
    actions::derive_compensation(store, "main", actor).map_err(|e| e.0)?;
    actions::derive_populations(store, "main", actor).map_err(|e| e.0)?;

    // step 9 门嵌套改名：层级 v2，只改名不动身份与父子关系
    let mut hier2 = current_payload(store, "gate-hierarchy", "main")?;
    let names = hier2["names"].as_object_mut().unwrap();
    for (g, nm) in c["hierarchy_v2_rename"]["renames"].as_object().unwrap() {
        names.insert(g.clone(), nm.clone());
    }
    hier2.as_object_mut().unwrap().insert(
        "note".into(),
        c["hierarchy_v2_rename"]["note"].clone(),
    );
    let ev = actions::commit_artifact(store, NewArtifact {
        id: "gate-hierarchy".into(), kind: ArtifactKind::GateHierarchy,
        branch: "main".into(), payload: hier2, raw_b64: None,
        base_version: Some(1),
    }, actor).map_err(conflict_msg)?;
    log.push(json!({"step": "rename_gate", "event": ev.id,
                    "renamed": c["hierarchy_v2_rename"]["renames"],
                    "identity": "P2 stable; parent P1 unchanged"}));
    actions::derive_populations(store, "main", actor).map_err(|e| e.0)?;

    // step 10 同版本并发修改：第二个窗口仍基于 v1 保存
    let attempted = json!({"names": {"ALL": "ALL", "P1": "Lymphocytes",
                                     "P3": "SSC-hi", "P2": "Dye-positive (window-B draft)"},
                           "order": ["ALL","P1","P3","P2"],
                           "edges": [["ALL","P1"],["ALL","P3"],["P1","P2"]]});
    let cf = match actions::commit_artifact(store, NewArtifact {
        id: "gate-hierarchy".into(), kind: ArtifactKind::GateHierarchy,
        branch: "main".into(), payload: attempted.clone(), raw_b64: None,
        base_version: Some(1),
    }, actor) {
        Ok(_) => return Err("预期发生并发冲突，但提交成功".into()),
        Err(cf) => cf,
    };
    actions::record_conflict(store, "main", actor, &cf).map_err(|e| e.0)?;
    log.push(json!({"step": "concurrent_conflict", "code": cf.code,
                    "base_version": cf.base_version, "head_version": cf.head_version,
                    "diff_paths": cf.diff.iter()
                        .map(|d| d["path"].clone()).collect::<Vec<_>>()}));

    // step 11 冲突解决：在新分支 review-B 上保留双方
    actions::create_branch(store, "review-B", "main", actor).map_err(|e| e.0)?;
    // 在 review-B 上提交窗口 B 的草案（基于该分支当前 head v2）
    actions::commit_artifact(store, NewArtifact {
        id: "gate-hierarchy".into(), kind: ArtifactKind::GateHierarchy,
        branch: "review-B".into(), payload: attempted, raw_b64: None,
        base_version: Some(2),
    }, actor).map_err(conflict_msg)?;
    actions::derive_populations(store, "review-B", actor).map_err(|e| e.0)?;
    log.push(json!({"step": "resolve_conflict", "branch": "review-B",
                    "resolution": "both drafts retained on separate branches"}));

    // step 12 最终复核：main 分支以当前全部 head 重算
    actions::derive_compensation(store, "main", actor).map_err(|e| e.0)?;
    let evs = actions::derive_populations(store, "main", actor).map_err(|e| e.0)?;
    log.push(json!({"step": "final_rederive",
                    "events": evs.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
                    "rule": "denominator-pinned-to-parent-population-version"}));

    Ok(log)
}

fn open_diagnostic(store: &mut Store, code: &str) -> Result<String, String> {
    let st = crate::engine::fold(&store.load_events().map_err(|e| e.0)?);
    st.diagnostics
        .values()
        .find(|d| d.code == code && matches!(d.status, crate::model::DiagStatus::Open))
        .map(|d| d.id.clone())
        .ok_or_else(|| format!("未找到开放诊断 {code}"))
}

fn open_diagnostics(store: &mut Store, code: &str) -> Vec<String> {
    let st = crate::engine::fold(&store.load_events().unwrap_or_default());
    st.diagnostics
        .values()
        .filter(|d| d.code == code && matches!(d.status, crate::model::DiagStatus::Open))
        .map(|d| d.id.clone())
        .collect()
}

fn current_payload(store: &mut Store, id: &str, branch: &str) -> Result<Value, String> {
    let st = crate::engine::fold(&store.load_events().map_err(|e| e.0)?);
    Ok(st.head(id, branch)
        .ok_or_else(|| format!("缺少 {id}"))?
        .payload
        .clone())
}

fn conflict_msg(cf: &CommitConflict) -> String {
    format!("版本冲突：{} base=v{} 当前=v{}", cf.artifact, cf.base_version, cf.head_version)
}
