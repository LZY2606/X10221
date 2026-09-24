//! 复核视图：通道散点（补偿前后）、门树、群体结论与分母来源。
//! 视图是派生物：可指定事件序号，恢复当时的依赖版本与可见结论。

use crate::derive::*;
use crate::model::*;
use crate::store::{Store, StoreError};
use serde_json::{json, Value};

pub fn build_view(store: &Store, draft_id: &str, at: Option<i64>) -> Result<Value, StoreError> {
    let draft = store
        .get_draft(draft_id)
        .ok_or_else(|| StoreError::Msg("draft not found".into()))?;
    let head = at.unwrap_or_else(|| draft["head_event"].as_i64().unwrap_or(0));
    let versions = store.artifact_versions_at(head);

    let get = |kind: ArtifactKind| -> Option<(String, i64, Value)> {
        versions
            .iter()
            .find(|(k, _, _, _)| *k == kind)
            .map(|(_, id, seq, p)| (id.clone(), *seq, p.clone()))
    };

    let mut scatter = json!({"points": [], "compensation_applied": false});
    let mut gates_json = json!([]);
    let mut tree_json = json!([]);
    let mut version_json = json!([]);

    for (kind, id, seq, _) in &versions {
        version_json.as_array_mut().unwrap().push(json!({
            "kind": kind.as_str(), "artifact_id": id, "version_seq": seq,
        }));
    }

    if let (
        Some((_, raw_seq, raw_p)),
        Some((_, comp_seq, comp_p)),
        Some((_, tf_seq, tf_p)),
    ) = (
        get(ArtifactKind::RawChannels),
        get(ArtifactKind::CompensationMatrix),
        get(ArtifactKind::TransformParams),
    ) {
        let raw: RawChannelsPayload = serde_json::from_value(raw_p).unwrap_or(RawChannelsPayload {
            channels: vec![],
            detector: DetectorMeta { linear_max: 0.0, unit: "RFU".into() },
            events: vec![],
        });
        let comp: Option<CompMatrixPayload> = serde_json::from_value(comp_p).ok();
        let tf: Option<TransformParamsPayload> = serde_json::from_value(tf_p).ok();
        let norm = normalize_events(&raw);
        let inversion = comp.as_ref().map(|c| invert_matrix(&c.matrix));
        let applied = matches!(inversion, Some(Ok(_)));
        let compensated: Vec<std::collections::BTreeMap<String, f64>> = match (&inversion, &comp) {
            (Some(Ok(inv)), Some(c)) => compensate(&norm, &c.order, inv),
            _ => norm.iter().map(|e| e.values.clone()).collect(),
        };
        let mut points = Vec::new();
        for (i, ev) in norm.iter().enumerate() {
            let mut scratch = Vec::new();
            let pre = tf
                .as_ref()
                .map(|t| transform(&ev.values, t, &ev.id, &mut scratch))
                .unwrap_or_default();
            let mut scratch2 = Vec::new();
            let post = tf
                .as_ref()
                .map(|t| transform(&compensated[i], t, &ev.id, &mut scratch2))
                .unwrap_or_default();
            points.push(json!({"id": ev.id, "pre": pre, "post": post}));
        }
        scatter = json!({
            "points": points,
            "compensation_applied": applied,
            "raw_version": raw_seq,
            "comp_version": comp_seq,
            "transform_version": tf_seq,
            "compensation_note": if applied { Value::Null } else { json!("补偿矩阵不可用或近奇异，散点显示未补偿值") },
        });
    }

    if let Some((_, _, poly_p)) = get(ArtifactKind::GatePolygon) {
        if let Ok(poly) = serde_json::from_value::<GatePolygonPayload>(poly_p) {
            gates_json = json!(poly.gates);
        }
    }
    if let Some((_, _, hier_p)) = get(ArtifactKind::GateHierarchy) {
        if let Ok(hier) = serde_json::from_value::<GateHierarchyPayload>(hier_p) {
            tree_json = json!(hier.nodes);
        }
    }

    // 群体结论：每个值都带稳定身份（候选 id）与分母来源。
    let cands = store.candidates_of(draft_id);
    let visible: Vec<Value> = cands
        .iter()
        .filter(|c| c.event_seq <= head || at.is_none())
        .map(|c| {
            json!({
                "id": c.id,
                "population": c.population,
                "label": c.label,
                "status": c.status,
                "value": c.value,
                "denominator": c.denominator,
                "provenance": c.provenance,
                "event_seq": c.event_seq,
            })
        })
        .collect();

    let diagnostics: Vec<Value> = store
        .diagnostics()
        .into_iter()
        .filter(|d| d.event_seq <= head || at.is_none())
        .map(|d| json!({
            "id": d.id, "object_id": d.object_id, "severity": d.severity,
            "code": d.code, "message": d.message, "event_seq": d.event_seq,
        }))
        .collect();

    Ok(json!({
        "draft": draft,
        "at_event": head,
        "versions": version_json,
        "scatter": scatter,
        "gates": gates_json,
        "tree": tree_json,
        "populations": visible,
        "diagnostics": diagnostics,
    }))
}
