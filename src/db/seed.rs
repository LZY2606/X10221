use super::*;
use crate::domain;
use serde_json::{json, Value};

const BRANCHES: [(&str, &str, &str); 6] = [
    ("main", "主基线：完整来源与已采纳派生", "baseline accepted lineage and unadopted candidates"),
    ("draft-rename", "草案：门嵌套改名", "same polygon, renamed nested gate identity"),
    ("draft-matrix-revision", "草案：近奇异补偿矩阵", "near-singular matrix cannot be silently inverted"),
    ("draft-upstream-revision", "草案：修订上游 Live 门", "downstream results become stale and are not overwritten"),
    ("draft-transform-revision", "草案：修订荧光变换参数", "only transform-dependent derivations are invalidated"),
    ("draft-source-change", "草案：局部源数据改动", "only locally dependent derivations are invalidated"),
];

pub fn seed_case(tx: &rusqlite::Transaction<'_>) -> DbResult<()> {
    let files = domain::case_files();
    let parsed: Vec<Value> = files.iter().map(|file| serde_json::from_str(&file.content).unwrap()).collect();
    let _manifest = &parsed[0];
    let raw = parsed[1].clone();
    let spillover = parsed[2].clone();
    let matrices = parsed[3].clone();
    let transforms = parsed[4].clone();
    let gates = parsed[5].clone();
    let links = parsed[6].clone();

    for (branch_id, name, description) in BRANCHES {
        let root_id = format!("E-{branch_id}-001");
        insert_branch(tx, branch_id, name, description, &root_id)?;
        for file in &files {
            tx.execute(
                "INSERT INTO file_evidence (branch_id, path, role, content, sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![branch_id, file.path, file.role, file.content, file.sha256],
            )?;
        }

        insert_event(
            tx,
            branch_id,
            1,
            &root_id,
            None,
            "case.imported",
            "adopted",
            "flow-platform-fixture",
            &json!({
                "schema": "flow-audit/event/v1",
                "case_id": CASE_ID,
                "atomic_import": true,
                "materials": files.iter().map(|file| json!({
                    "path": file.path,
                    "role": file.role,
                    "sha256": file.sha256
                })).collect::<Vec<_>>(),
                "rollback_policy": "failed import rolls back the entire transaction and leaves no partial records"
            }),
        )?;

        let mut seq = 1;
        let source_items = [
            ("raw-events.json", "fcs-events", "SRC-RAW-V1", &raw),
            ("spillover.json", "spillover-metadata", "SRC-SPILL-V1", &spillover),
            ("compensation-matrices.json", "compensation-matrices", "SRC-MATRICES-V1", &matrices),
            ("transform-parameters.json", "transform-parameters", "SRC-TRANSFORM-V1", &transforms),
            ("gates.json", "gate-polygons", "SRC-GATES-V1", &gates),
            ("parent-links.json", "gate-parent-links", "SRC-LINKS-V1", &links),
        ];
        for (path, kind, id, payload) in source_items {
            seq += 1;
            let event_id = format!("E-{branch_id}-{seq:03}");
            let sha = files.iter().find(|file| file.path == path).unwrap().sha256.clone();
            insert_record(
                tx,
                branch_id,
                seq,
                id,
                id,
                kind,
                "v1",
                "imported",
                format!("source:{kind}"),
                None,
                &event_id,
                payload,
                Some(path),
                Some(sha),
            )?;
            insert_event(
                tx,
                branch_id,
                seq,
                &event_id,
                Some(&previous_event(tx, branch_id, seq)?),
                "source.versioned",
                "adopted",
                "flow-platform-fixture",
                &json!({
                    "schema": "flow-audit/event/v1",
                    "record_id": id,
                    "kind": kind,
                    "source_path": path,
                    "source_sha256": sha,
                    "interpretation": "source and interpretation are stored separately; raw representation is retained"
                }),
            )?;
        }

        seed_baseline_derivations(tx, branch_id, &raw, &matrices, &transforms, &gates, &links, &mut seq)?;
        match branch_id {
            "draft-rename" => seed_rename(tx, branch_id, &mut seq)?,
            "draft-matrix-revision" => seed_matrix_block(tx, branch_id, &mut seq)?,
            "draft-upstream-revision" => seed_upstream_revision(tx, branch_id, &mut seq)?,
            "draft-transform-revision" => seed_transform_revision(tx, branch_id, &mut seq)?,
            "draft-source-change" => seed_source_change(tx, branch_id, &mut seq)?,
            _ => {}
        }
    }
    Ok(())
}

fn previous_event(tx: &rusqlite::Transaction<'_>, branch: &str, seq: i64) -> DbResult<String> {
    tx.query_row(
        "SELECT id FROM audit_events WHERE branch_id = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT 1",
        params![branch, seq],
        |row| row.get(0),
    ).map_err(Into::into)
}

#[allow(clippy::too_many_arguments)]
fn seed_baseline_derivations(
    tx: &rusqlite::Transaction<'_>,
    branch: &str,
    raw: &Value,
    matrices: &Value,
    transforms: &Value,
    gates: &Value,
    links: &Value,
    seq: &mut i64,
) -> DbResult<()> {
    let matrix_list = matrices["matrices"].as_array().unwrap();
    for matrix in matrix_list {
        *seq += 1;
        let matrix_id = matrix["matrix_id"].as_str().unwrap();
        let is_near_singular = matrix_id == "M-NEAR-SINGULAR-V1";
        let status = if is_near_singular { "unadopted-near-singular-blocked" } else { "accepted" };
        let inverse = matrix_values(matrix)
            .and_then(|values| invert_spillover(&values, matrix["singularity_threshold"].as_f64().unwrap_or(1e-12)));
        let diagnostics = match &inverse {
            Ok(_) if !is_near_singular => vec![],
            Err(message) => vec![json!({
                "code": "NEAR_SINGULAR_MATRIX_NOT_INVERTED",
                "severity": "error",
                "rule_id": "RULE-COMP-001",
                "message": message
            })],
            Ok(_) => vec![json!({"code": "UNEXPECTED_INVERTIBLE_CANDIDATE", "severity": "warning"})],
        };
        let payload = json!({
            "matrix_id": matrix_id,
            "source_matrix": matrix,
            "inverse": inverse.ok(),
            "diagnostics": diagnostics,
            "decision": if is_near_singular { "retained_as_unadopted_candidate" } else { "accepted" }
        });
        add_derived(
            tx, branch, seq,
            &format!("R-{matrix_id}"),
            "compensation-matrix-result",
            "v1", status,
            &format!("compensation:{matrix_id}"),
            Some("SRC-MATRICES-V1"),
            "matrix.compensation.evaluated",
            payload,
        )?;
    }

    let stable_matrix = matrix_list.iter().find(|matrix| matrix["matrix_id"] == "M-STABLE-V1").unwrap();
    let inverse = invert_spillover(
        &matrix_values(stable_matrix).unwrap(),
        stable_matrix["singularity_threshold"].as_f64().unwrap(),
    ).unwrap();
    let channels = stable_matrix_columns(matrices);
    let compensated = compensate_events(raw, &channels, &inverse)?;

    *seq += 1;
    add_derived(
        tx, branch, seq,
        "R-COMPENSATED-EVENTS-V1",
        "compensated-events",
        "v1", "accepted",
        "compensated-events:M-STABLE-V1:SRC-RAW-V1",
        Some("R-M-STABLE-V1"),
        "compensation.events.produced",
        compensated.clone(),
    )?;

    let transformed = transform_events(raw, &compensated, transforms)?;
    *seq += 1;
    add_derived(
        tx, branch, seq,
        "R-TRANSFORMED-EVENTS-V1",
        "transformed-events",
        "v1", "accepted",
        "transformed-events:TR-V1:R-COMPENSATED-EVENTS-V1",
        Some("R-COMPENSATED-EVENTS-V1"),
        "transform.events.produced",
        transformed.clone(),
    )?;

    let gate_values = gates["gates"].as_array().unwrap();
    let link_values = links["links"].as_array().unwrap();
    for gate in gate_values {
        let gate_id = gate["gate_id"].as_str().unwrap();
        if gate_id == "G-RENAMED-NESTED-V2" {
            continue;
        }
        let link = link_values.iter().find(|link| link["gate_id"] == gate_id).unwrap();
        let parent_result = link["parent_gate_result_id"].as_str();
        let parent_members = parent_members_for(parent_result, raw, &compensated, &transformed, gate_values);
        let result = gate_result_payload(gate, raw, &compensated, &transformed, parent_result, parent_members)?;
        *seq += 1;
        let result_id = format!("R-{gate_id}");
        add_derived(
            tx, branch, seq,
            &result_id,
            "gate-result",
            "v1", "accepted",
            &format!("gate-result:{gate_id}"),
            Some(parent_result.unwrap_or("R-COMPENSATED-EVENTS-V1")),
            "gate.evaluated",
            result,
        )?;
    }
    Ok(())
}

fn add_derived(
    tx: &rusqlite::Transaction<'_>,
    branch: &str,
    seq: &mut i64,
    id: &str,
    kind: &str,
    version: &str,
    status: &str,
    dependency_key: &str,
    parent: Option<&str>,
    event_type: &str,
    payload: Value,
) -> DbResult<()> {
    let event_id = format!("E-{branch}-{*seq:03}");
    insert_record(
        tx, branch, *seq, id, id, kind, version, status, dependency_key, parent, &event_id, &payload, None, None,
    )?;
    insert_event(
        tx, branch, *seq, &event_id, Some(&previous_event(tx, branch, *seq)?), event_type, status, "derivation-engine",
        &json!({
            "schema": "flow-audit/event/v1",
            "record_id": id,
            "stable_identity": id,
            "status": status,
            "dependency_key": dependency_key,
            "parent_record_id": parent
        }),
    )
}

fn matrix_values(matrix: &Value) -> Option<Vec<Vec<f64>>> {
    matrix["values"]
        .as_array()?
        .iter()
        .map(|row| row.as_array().map(|values| values.iter().filter_map(|value| value.as_f64()).collect()))
        .collect()
}

fn stable_matrix_columns(matrices: &Value) -> Vec<String> {
    matrices["basis"]["matrix_order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect()
}

fn raw_number(event: &Value, channel: &str) -> Option<f64> {
    event["values"][channel]["value"].as_f64()
}

fn compensate_events(raw: &Value, channels: &[String], inverse: &[Vec<f64>]) -> DbResult<Value> {
    let mut items = Vec::new();
    for event in raw["events"].as_array().unwrap() {
        let vector: Vec<Option<f64>> = channels.iter().map(|channel| raw_number(event, channel)).collect();
        let mut compensated = std::collections::BTreeMap::new();
        let mut diagnostics = event["diagnostics"].as_array().cloned().unwrap_or_default();
        for (index, channel) in channels.iter().enumerate() {
            let original = vector[index];
            let mut value = None;
            if vector.iter().all(Option::is_some) {
                let mut sum = 0.0;
                for (column, input) in vector.iter().enumerate() {
                    sum += inverse[index][column] * input.unwrap();
                }
                value = Some(sum);
            } else if original.is_none() {
                diagnostics.push(json!({
                    "code": "UNCONVERTIBLE_VALUE_RETAINED",
                    "severity": "error",
                    "channel": channel,
                    "message": "raw value remains in place; compensation is not imputed"
                }));
            } else {
                diagnostics.push(json!({
                    "code": "COMPENSATION_SKIPPED_MISSING_PEER",
                    "severity": "warning",
                    "channel": channel,
                    "message": "channel not compensated because another detector in the matrix has no numeric source value"
                }));
                value = original;
            }
            compensated.insert(
                channel.clone(),
                json!({
                    "raw": event["values"][channel]["raw"],
                    "raw_unit": event["values"][channel]["unit"],
                    "compensated_value": value,
                    "compensated_unit": "compensated detector event value"
                }),
            );
        }
        items.push(json!({
            "event_id": event["event_id"],
            "acquisition_index": event["acquisition_index"],
            "channels": compensated,
            "diagnostics": diagnostics
        }));
    }
    Ok(json!({
        "matrix_id": "M-STABLE-V1",
        "matrix_order": channels,
        "events": items
    }))
}

fn transform_events(raw: &Value, compensated: &Value, transforms: &Value) -> DbResult<Value> {
    let mut items = Vec::new();
    for event in compensated["events"].as_array().unwrap() {
        let mut coordinates = std::collections::BTreeMap::new();
        let raw_event = raw["events"].as_array().unwrap().iter()
            .find(|item| item["event_id"] == event["event_id"])
            .unwrap();
        for parameter in transforms["parameters"].as_array().unwrap() {
            let method = parameter["method"].as_str().unwrap();
            let cofactor = parameter["cofactor"].as_f64();
            for channel in parameter["applies_to"].as_array().unwrap() {
                let channel = channel.as_str().unwrap();
                let source = event["channels"].get(channel);
                let numeric = source
                    .and_then(|source| source["compensated_value"].as_f64())
                    .or_else(|| raw_number(raw_event, channel));
                let coordinate = if method == "asinh" {
                    numeric.map(|value| asinh_value(value, cofactor.unwrap()))
                } else {
                    numeric
                };
                let mut diagnostics = Vec::new();
                if let Some(value) = coordinate {
                    if !value.is_finite() {
                        diagnostics.push(json!({"code": "NON_FINITE_COORDINATE", "severity": "error"}));
                    }
                    if value > 8.0 {
                        diagnostics.push(json!({"code": "EXTREME_SIGNAL_OUTSIDE_NORMAL_DISPLAY_RANGE", "severity": "warning", "value": value}));
                    }
                } else {
                    diagnostics.push(json!({"code": "NO_DISPLAY_COORDINATE_RAW_RETAINED", "severity": "error", "channel": channel}));
                }
                coordinates.insert(channel.to_string(), json!({
                    "coordinate": coordinate,
                    "coordinate_unit": parameter["output_unit"],
                    "input_unit": parameter["input_unit"],
                    "diagnostics": diagnostics
                }));
            }
        }
        items.push(json!({
            "event_id": event["event_id"],
            "acquisition_index": event["acquisition_index"],
            "coordinates": coordinates,
            "diagnostics": event["diagnostics"]
        }));
    }
    Ok(json!({"events": items}))
}

fn event_transformed_coordinates(event: &Value) -> Vec<(String, Option<f64>)> {
    event["coordinates"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(channel, value)| (channel.clone(), value["coordinate"].as_f64()))
        .collect()
}

fn gate_members(gate_id: &str, gates: &[&Value], transformed: &Value) -> Vec<String> {
    let gate = gates.iter().find(|gate| gate["gate_id"] == gate_id).unwrap();
    transformed["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| {
            let event_id = event["event_id"].as_str().unwrap();
            gate_membership(event_id, &event_transformed_coordinates(event), gate).unwrap_or(false)
        })
        .map(|event| event["event_id"].as_str().unwrap().to_string())
        .collect()
}

fn parent_members_for(parent_result: Option<&str>, raw: &Value, _compensated: &Value, transformed: &Value, gates: &[&Value]) -> Vec<String> {
    match parent_result {
        None => raw["events"].as_array().unwrap().iter().map(|event| event["event_id"].as_str().unwrap().to_string()).collect(),
        Some("R-G-LIVE-V1") => gate_members("G-LIVE-V1", gates, transformed),
        Some("R-G-FITC-V1") => gate_members("G-FITC-V1", gates, transformed)
            .into_iter()
            .filter(|event| gate_members("G-LIVE-V1", gates, transformed).contains(event))
            .collect(),
        Some(other) => vec![format!("unmapped:{other}")],
    }
}

fn gate_result_payload(
    gate: &Value,
    raw: &Value,
    _compensated: &Value,
    transformed: &Value,
    parent_result: Option<&str>,
    parent_members: Vec<String>,
) -> DbResult<Value> {
    let gate_id = gate["gate_id"].as_str().unwrap();
    let mut members = Vec::new();
    let mut boundary_members = Vec::new();
    let mut unplottable = Vec::new();
    for event in transformed["events"].as_array().unwrap() {
        let event_id = event["event_id"].as_str().unwrap();
        if !parent_members.iter().any(|parent| parent == event_id) {
            continue;
        }
        match gate_membership(event_id, &event_transformed_coordinates(event), gate) {
            Ok(true) => members.push(event_id.to_string()),
            Ok(false) => {}
            Err(message) => unplottable.push(json!({"event_id": event_id, "diagnostic": message})),
        }
    }
    for raw_event in raw["events"].as_array().unwrap() {
        for diagnostic in raw_event["diagnostics"].as_array().unwrap_or(&Vec::new()) {
            if diagnostic["code"] == "BOUNDARY_POINT" && members.iter().any(|member| member == raw_event["event_id"].as_str().unwrap()) {
                boundary_members.push(raw_event["event_id"].as_str().unwrap().to_string());
            }
        }
    }
    let denominator_source = parent_version_key(parent_result);
    let denominator_count = parent_members.len();
    let numerator_count = members.len();
    let percentage = if denominator_count == 0 {
        None
    } else {
        Some(100.0 * numerator_count as f64 / denominator_count as f64)
    };
    Ok(json!({
        "gate_id": gate_id,
        "population_id": gate["population_id"],
        "label": gate["label"],
        "polygon_version": "v1",
        "x_channel": gate["x_channel"],
        "y_channel": gate["y_channel"],
        "coordinates": gate["coordinates"],
        "coordinate_unit": gate["coordinate_unit"],
        "parent_population_id": parent_population(gate_id),
        "denominator": {
            "source_record_id": denominator_source,
            "pinned_parent_version": parent_result.unwrap_or("P-ROOT@ALL"),
            "parent_event_ids": parent_members,
            "parent_count": denominator_count,
            "membership_hash": membership_hash(&parent_members)
        },
        "membership": {
            "event_ids": members,
            "count": numerator_count,
            "percentage_of_pinned_parent": percentage,
            "percentage_unit": "% of pinned parent gate version",
            "boundary_event_ids": boundary_members
        },
        "unplottable_events": unplottable,
        "rule_evidence": {
            "boundary_rule": "points on polygon edges are included using the fixed deterministic edge rule",
            "denominator_rule_id": "RULE-GATE-DENOM-001",
            "stale_rule_id": "RULE-LINEAGE-001"
        }
    }))
}

fn parent_population(gate_id: &str) -> &'static str {
    match gate_id {
        "G-LIVE-V1" => "P-ROOT",
        "G-FITC-V1" | "G-RENAMED-NESTED-V2" => "P-LIVE",
        "G-PE-SPREAD-V1" => "P-FITC-POS",
        _ => "P-UNKNOWN",
    }
}

fn add_event_and_reference(
    tx: &rusqlite::Transaction<'_>,
    branch: &str,
    seq: &mut i64,
    event_type: &str,
    status: &str,
    actor: &str,
    payload: Value,
) -> DbResult<String> {
    *seq += 1;
    let event_id = format!("E-{branch}-{*seq:03}");
    insert_event(
        tx,
        branch,
        *seq,
        &event_id,
        Some(&previous_event(tx, branch, *seq)?),
        event_type,
        status,
        actor,
        &payload,
    )?;
    Ok(event_id)
}

fn mark_stale(tx: &rusqlite::Transaction<'_>, branch: &str, seq: &mut i64, reason: &str, target_identities: &[&str], changed_source: &str) -> DbResult<()> {
    add_event_and_reference(
        tx,
        branch,
        seq,
        "derivation.stale-propagated",
        "stale",
        "dependency-engine",
        json!({
            "schema": "flow-audit/event/v1",
            "rule_id": "RULE-LINEAGE-001",
            "judgment": "upstream revision marks downstream results stale without overwriting them",
            "changed_direct_source": changed_source,
            "reason": reason,
            "invalidated_scope": target_identities,
            "preservation_policy": "old records remain readable at their creating event; no global replacement is performed"
        }),
    )
}

fn seed_rename(tx: &rusqlite::Transaction<'_>, branch: &str, seq: &mut i64) -> DbResult<()> {
    let event_id = add_event_and_reference(
        tx,
        branch,
        seq,
        "gate.nested-renamed",
        "accepted",
        "reviewer@example.test",
        json!({
            "schema": "flow-audit/event/v1",
            "rule_id": "RULE-LINEAGE-002",
            "old_gate_id": "G-FITC-V1",
            "new_gate_id": "G-RENAMED-NESTED-V2",
            "population_id": "P-FITC-POS",
            "polygon_changed": false,
            "lineage_judgment": "the nested gate display name is versioned separately from polygon geometry and parent links"
        }),
    )?;
    let gate: Value = serde_json::from_str(GATES)?;
    let renamed = gate["gates"].as_array().unwrap().iter()
        .find(|item| item["gate_id"] == "G-RENAMED-NESTED-V2")
        .unwrap()
        .clone();
    *seq += 1;
    insert_record(
        tx, branch, *seq, "G-RENAMED-NESTED-V2", "G-RENAMED-NESTED-V2", "gate-polygon", "v2", "accepted-candidate",
        "gate-polygon:G-RENAMED-NESTED-V2", Some("R-G-LIVE-V1"), &event_id, &renamed, Some("gates.json"), None,
    )?;
    Ok(())
}

fn seed_matrix_block(tx: &rusqlite::Transaction<'_>, branch: &str, seq: &mut i64) -> DbResult<()> {
    add_event_and_reference(
        tx,
        branch,
        seq,
        "compensation.near-singular-decision-replayed",
        "rejected",
        "reviewer@example.test",
        json!({
            "schema": "flow-audit/event/v1",
            "rule_id": "RULE-COMP-001",
            "matrix_identity": "M-NEAR-SINGULAR-V1",
            "determinant_approx": 1e-13,
            "singularity_threshold": 1e-12,
            "decision": "rejected",
            "judgment": "near-singular compensation matrix cannot be silently inverted",
            "adopted_record_unchanged": "R-M-STABLE-V1",
            "legal_candidates": ["keep M-STABLE-V1", "provide a newly measured spillover matrix"]
        }),
    )?;
    Ok(())
}

fn seed_upstream_revision(tx: &rusqlite::Transaction<'_>, branch: &str, seq: &mut i64) -> DbResult<()> {
    let event_id = add_event_and_reference(
        tx,
        branch,
        seq,
        "gate.upstream-revised",
        "accepted",
        "reviewer@example.test",
        json!({
            "schema": "flow-audit/event/v1",
            "rule_id": "RULE-LINEAGE-001",
            "revised_gate": "G-LIVE-V2",
            "old_gate_result": "R-G-LIVE-V1",
            "new_polygon": [[45,25],[60,25],[60,45],[45,45]],
            "excluded_boundary_event": "E08",
            "denominator_policy": "each child percentage remains pinned to its cited parent version until recalculated"
        }),
    )?;
    *seq += 1;
    insert_record(
        tx, branch, *seq, "G-LIVE-V2", "G-LIVE-V2", "gate-polygon", "v2", "accepted",
        "gate-polygon:G-LIVE-V2", Some("R-TRANSFORMED-EVENTS-V1"), &event_id,
        &json!({"gate_id":"G-LIVE-V2","population_id":"P-LIVE","coordinates":[[45,25],[60,25],[60,45],[45,45]]}),
        None, None,
    )?;
    mark_stale(
        tx,
        branch,
        seq,
        "parent gate G-LIVE changed",
        &["R-G-FITC-V1", "R-G-PE-SPREAD-V1"],
        "G-LIVE-V2",
    )
}

fn seed_transform_revision(tx: &rusqlite::Transaction<'_>, branch: &str, seq: &mut i64) -> DbResult<()> {
    let event_id = add_event_and_reference(
        tx,
        branch,
        seq,
        "transform.parameters-revised",
        "candidate",
        "reviewer@example.test",
        json!({
            "schema": "flow-audit/event/v1",
            "rule_id": "RULE-LINEAGE-001",
            "transform_identity": "TR-ASINH-FL-V2",
            "cofactor": 120.0,
            "changed_direct_source": "TR-ASINH-FL-V1",
            "invalidated_scope": ["R-TRANSFORMED-EVENTS-V1", "R-G-FITC-V1", "R-G-PE-SPREAD-V1"],
            "unaffected_scope": ["R-G-LIVE-V1", "R-COMPENSATED-EVENTS-V1"]
        }),
    )?;
    *seq += 1;
    insert_record(
        tx, branch, *seq, "TR-ASINH-FL-V2", "TR-ASINH-FL-V2", "transform-parameters", "v2", "pending-candidate",
        "transform-parameters:TR-ASINH-FL-V2", Some("R-COMPENSATED-EVENTS-V1"), &event_id,
        &json!({"transform_id":"TR-ASINH-FL-V2","method":"asinh","cofactor":120.0}),
        None, None,
    )?;
    mark_stale(
        tx,
        branch,
        seq,
        "fluorescence transform parameter changed",
        &["R-TRANSFORMED-EVENTS-V1", "R-G-FITC-V1", "R-G-PE-SPREAD-V1"],
        "TR-ASINH-FL-V2",
    )
}

fn seed_source_change(tx: &rusqlite::Transaction<'_>, branch: &str, seq: &mut i64) -> DbResult<()> {
    let event_id = add_event_and_reference(
        tx,
        branch,
        seq,
        "source.raw-events-locally-changed",
        "accepted",
        "flow-platform-fixture",
        json!({
            "schema": "flow-audit/event/v1",
            "new_source_record": "SRC-RAW-V2-LOCAL-E08",
            "direct_change": "E08 PE-A raw detector value",
            "invalidated_scope": ["R-COMPENSATED-EVENTS-V1", "R-TRANSFORMED-EVENTS-V1", "R-G-FITC-V1", "R-G-PE-SPREAD-V1"],
            "unaffected_scope": ["R-G-LIVE-V1"],
            "membership_changed": false,
            "judgment": "dependency graph limits invalidation to fluorescence-dependent objects; scatter gate remains valid"
        }),
    )?;
    *seq += 1;
    insert_record(
        tx, branch, *seq, "SRC-RAW-V2-LOCAL-E08", "SRC-RAW", "fcs-events", "v2", "accepted",
        "fcs-events:SRC-RAW", None, &event_id,
        &json!({"format":"flow-domain/raw-fcs-events/v2-patch","changed_event":"E08","channel":"PE-A","old_raw":"500","new_raw":"501","unit":"raw detector event value"}),
        Some("raw-events.json"), None,
    )?;
    mark_stale(
        tx,
        branch,
        seq,
        "E08 PE fluorescence source value changed",
        &["R-COMPENSATED-EVENTS-V1", "R-TRANSFORMED-EVENTS-V1", "R-G-FITC-V1", "R-G-PE-SPREAD-V1"],
        "SRC-RAW-V2-LOCAL-E08",
    )
}
