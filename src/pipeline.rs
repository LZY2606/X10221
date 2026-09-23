//! Derived-layer computation: raw channel values -> compensation -> transform
//! -> gate membership -> population counts with pinned denominators.
//! Every result records its exact dependency versions.

use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde_json::json;
use std::collections::{HashMap, HashSet};

use crate::comp;
use crate::gating::{self, Membership};
use crate::store::{self, GateRow};
use crate::transform;

pub const RANGE_MAX: f64 = 262144.0;

#[derive(Debug, Clone)]
pub struct ChannelValues {
    pub source_id: String,
    pub version: i64,
    /// events[event_index] -> per-channel (value, diagnostic)
    pub events: Vec<HashMap<String, (Option<f64>, Option<String>)>>,
}

pub fn load_channel_values(conn: &Connection, source_id: &str) -> Result<ChannelValues> {
    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM channel_values WHERE source_id = ?1",
        rusqlite::params![source_id],
        |r| r.get(0),
    )?;
    load_channel_values_version(conn, source_id, version)
}

pub fn load_channel_values_version(
    conn: &Connection,
    source_id: &str,
    version: i64,
) -> Result<ChannelValues> {
    let mut stmt = conn.prepare(
        "SELECT event_index, channel, value, diagnostic FROM channel_values
         WHERE source_id = ?1 AND version = ?2 ORDER BY event_index, channel",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![source_id, version], |r| {
            Ok((
                r.get::<_, i64>(0)? as usize,
                r.get::<_, String>(1)?,
                r.get::<_, Option<f64>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut events: Vec<HashMap<String, (Option<f64>, Option<String>)>> = Vec::new();
    for (idx, ch, val, diag) in rows {
        if idx >= events.len() {
            events.resize(idx + 1, HashMap::new());
        }
        events[idx].insert(ch, (val, diag));
    }
    Ok(ChannelValues { source_id: source_id.into(), version, events })
}

/// Coordinate of one event in a gate's basis.
fn gate_coordinate(
    event: &HashMap<String, (Option<f64>, Option<String>)>,
    channel: &str,
    basis: &str,
    matrix_inv: Option<&Vec<Vec<f64>>>,
    detectors: &[String],
    transforms: &HashMap<String, store::TransformRow>,
    comp_cache: &mut HashMap<usize, Vec<f64>>,
    event_idx: usize,
) -> (Option<f64>, Option<String>) {
    let (raw, diag) = match event.get(channel) {
        Some((v, d)) => (*v, d.clone()),
        None => (None, None),
    };
    let raw = match raw {
        Some(v) => v,
        None => {
            return (None, Some(diag.unwrap_or_else(|| "missing_channel".into())));
        }
    };
    if let Some(d) = diag {
        if d == "not_a_number" {
            return (None, Some(d));
        }
    }
    if basis == "raw_detector" {
        return (Some(raw), None);
    }
    // compensated + transformed basis
    let det_pos = match detectors.iter().position(|d| d == channel) {
        Some(p) => p,
        None => return (None, Some(format!("channel {channel} not covered by matrix"))),
    };
    let inv = match matrix_inv {
        Some(m) => m,
        None => return (None, Some("no_accepted_compensation_matrix".into())),
    };
    let compensated = comp_cache.entry(event_idx).or_insert_with(|| {
        let observed: Vec<f64> = detectors
            .iter()
            .map(|d| event.get(d).and_then(|(v, _)| *v).unwrap_or(f64::NAN))
            .collect();
        comp::apply_inverse(inv, &observed)
    });
    let value = compensated[det_pos];
    if value.is_nan() {
        return (None, Some("not_a_number".into()));
    }
    let t = transforms.get(channel).map(|t| &t.transform);
    let t = t.cloned().unwrap_or(transform::Transform::Linear);
    let (derived, diag2) = transform::derive_value(value, &t, RANGE_MAX);
    (derived, diag2.map(|d| d.code))
}

/// Topological gate order: parents before children.
fn topo_order(gates: &[GateRow]) -> Vec<GateRow> {
    let mut remaining: Vec<GateRow> = gates.to_vec();
    let mut done: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|g| {
            let ready = match &g.parent_id {
                None => true,
                Some(p) => done.contains(p),
            };
            if ready {
                done.insert(g.id.clone());
                out.push(g.clone());
            }
            !ready
        });
        if remaining.len() == before {
            out.append(&mut remaining); // cycle guard
        }
    }
    out
}

/// Recompute every gate of a draft on a branch. Inserts new derived_results
/// rows (status 'current'); previously current rows for the same gate become
/// 'superseded' — their content is never altered or deleted.
pub fn compute_draft(
    conn: &Connection,
    branch: &str,
    draft: &str,
    actor: Option<&str>,
) -> Result<Vec<String>> {
    let seq = crate::db::next_event_seq(conn)?;
    let gates = store::gates_as_of(conn, draft, None)?;
    if gates.is_empty() {
        return Err(anyhow!("draft '{draft}' has no gates"));
    }
    let matrix = store::current_matrix(conn, None)?;
    let (matrix_inv, matrix_ref) = match &matrix {
        Some(m) => (
            Some(comp::inverse(&m.matrix).ok_or_else(|| anyhow!("accepted matrix not invertible"))?),
            Some(format!("{}@{}", m.id, m.version)),
        ),
        None => (None, None),
    };
    let detectors: Vec<String> = matrix
        .as_ref()
        .map(|m| m.detectors.clone())
        .unwrap_or_default();
    let transforms = store::transforms_as_of(conn, None)?;
    let sample = sample_source(conn)?;
    let values = load_channel_values(conn, &sample)?;

    // Re-pin: if a gate's pinned parent version is behind the parent's
    // latest version, create a new gate version with the pin moved forward.
    // This is an explicit, machine-readable event - never a silent rewrite.
    let mut repinned: Vec<serde_json::Value> = Vec::new();
    for gate in &gates {
        if let Some(pid) = &gate.parent_id {
            let latest_parent = gates
                .iter()
                .filter(|g| &g.id == pid)
                .map(|g| g.version)
                .max()
                .unwrap_or(0);
            if gate.parent_version != Some(latest_parent) && latest_parent > 0 {
                let new_version = gate.version + 1;
                conn.execute(
                    "INSERT INTO gates(id, version, draft, name, parent_id, parent_version,
                                       x_channel, y_channel, basis, unit, polygon, seq, created_at)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                    rusqlite::params![
                        gate.id, new_version, gate.draft, gate.name, pid, latest_parent,
                        gate.x_channel, gate.y_channel, gate.basis, gate.unit,
                        serde_json::to_string(&gate.polygon)?, seq,
                        chrono::Utc::now().to_rfc3339()
                    ],
                )?;
                repinned.push(json!({
                    "gate_id": gate.id,
                    "from": format!("{}@{}", gate.id, gate.version),
                    "to": format!("{}@{}", gate.id, new_version),
                    "parent_pin": format!("{}@{}", pid, latest_parent),
                }));
            }
        }
    }
    if !repinned.is_empty() {
        crate::db::emit_event(conn, branch, "gates.repinned", actor, &json!({"repinned": repinned}))?;
    }
    let gates = store::gates_as_of(conn, draft, None)?;
    let ordered = topo_order(&gates);
    // membership sets per gate id (this compute pass)
    let mut members: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut inserted = Vec::new();

    for gate in &ordered {
        let mut comp_cache: HashMap<usize, Vec<f64>> = HashMap::new();
        let mut member_set: HashSet<usize> = HashSet::new();
        let mut boundary: Vec<i64> = Vec::new();
        let mut diag_counts: HashMap<String, i64> = HashMap::new();

        // parent membership uses the PINNED parent version's member set.
        let parent_members: Option<HashSet<usize>> = match &gate.parent_id {
            None => None,
            Some(pid) => Some(members.get(pid).cloned().unwrap_or_default()),
        };

        for (idx, event) in values.events.iter().enumerate() {
            if let Some(pm) = &parent_members {
                if !pm.contains(&idx) {
                    continue;
                }
            }
            let (x, dx) = gate_coordinate(
                event, &gate.x_channel, &gate.basis, matrix_inv.as_ref(),
                &detectors, &transforms, &mut comp_cache, idx,
            );
            let (y, dy) = gate_coordinate(
                event, &gate.y_channel, &gate.basis, matrix_inv.as_ref(),
                &detectors, &transforms, &mut comp_cache, idx,
            );
            for d in [dx, dy].into_iter().flatten() {
                *diag_counts.entry(d).or_insert(0) += 1;
            }
            let (Some(x), Some(y)) = (x, y) else { continue };
            match gating::classify(x, y, &gate.polygon) {
                Membership::Inside => {
                    member_set.insert(idx);
                }
                Membership::Boundary => {
                    member_set.insert(idx);
                    boundary.push(idx as i64);
                }
                Membership::Outside => {}
            }
        }
        counts.insert(gate.id.clone(), member_set.len());
        members.insert(gate.id.clone(), member_set);

        // denominator pinned to the parent population VERSION recorded on the gate
        let (den_id, den_ver, den_count) = match (&gate.parent_id, gate.parent_version) {
            (Some(pid), Some(pv)) => {
                let pc = *counts.get(pid).unwrap_or(&0);
                (Some(pid.clone()), Some(pv), pc as i64)
            }
            _ => (None, None, values.events.len() as i64),
        };
        let count = *counts.get(&gate.id).unwrap_or(&0) as i64;
        let percent = if den_count > 0 {
            Some(100.0 * count as f64 / den_count as f64)
        } else {
            None
        };

        // dependency closure: self + ancestors at their pinned versions
        let mut dep_gates = serde_json::Map::new();
        dep_gates.insert(gate.id.clone(), json!(gate.version));
        let mut cursor = gate.clone();
        while let Some(pid) = cursor.parent_id.clone() {
            let pv = cursor.parent_version.unwrap_or(0);
            dep_gates.insert(pid.clone(), json!(pv));
            match store::gate_version(conn, draft, &pid, pv)? {
                Some(parent) => cursor = parent,
                None => break,
            }
        }
        let dep_transforms: serde_json::Map<String, serde_json::Value> = [
            (&gate.x_channel, transforms.get(&gate.x_channel)),
            (&gate.y_channel, transforms.get(&gate.y_channel)),
        ]
        .into_iter()
        .filter_map(|(ch, t)| t.map(|t| (ch.clone(), json!(t.version))))
        .collect();
        let mut dep_channels = serde_json::Map::new();
        dep_channels.insert(
            format!("{}:{}", values.source_id, gate.x_channel),
            json!(values.version),
        );
        dep_channels.insert(
            format!("{}:{}", values.source_id, gate.y_channel),
            json!(values.version),
        );
        let uses_matrix = gate.basis != "raw_detector";
        let deps = json!({
            "gates": dep_gates,
            "transforms": dep_transforms,
            "matrix": if uses_matrix { matrix_ref.clone() } else { None },
            "channel_values": dep_channels,
        });
        let diagnostics: Vec<serde_json::Value> = diag_counts
            .iter()
            .map(|(code, n)| json!({"code": code, "events": n}))
            .collect();

        conn.execute(
            "UPDATE derived_results SET status = 'superseded'
             WHERE branch_id = ?1 AND draft = ?2 AND gate_id = ?3 AND status = 'current'",
            rusqlite::params![branch, draft, gate.id],
        )?;
        let result_id = format!("res-{}", uuid::Uuid::new_v4().simple());
        conn.execute(
            "INSERT INTO derived_results(
                id, branch_id, draft, gate_id, gate_version, count,
                denominator_gate_id, denominator_gate_version, denominator_count,
                percent, deps, status, stale_reason, boundary_events, diagnostics,
                seq, created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'current',NULL,?12,?13,?14,?15)",
            rusqlite::params![
                result_id, branch, draft, gate.id, gate.version, count,
                den_id, den_ver, den_count, percent,
                serde_json::to_string(&deps)?,
                serde_json::to_string(&boundary)?,
                serde_json::to_string(&diagnostics)?,
                seq,
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        inserted.push(result_id);
    }
    crate::db::emit_event(
        conn,
        branch,
        "pipeline.computed",
        actor,
        &json!({"draft": draft, "results": inserted, "seq_ref": seq}),
    )?;
    Ok(inserted)
}

pub fn sample_source(conn: &Connection) -> Result<String> {
    let mut stmt =
        conn.prepare("SELECT id FROM sources WHERE kind = 'fcs_events' ORDER BY id LIMIT 1")?;
    let mut rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.pop()
        .ok_or_else(|| anyhow!("no fcs_events source imported"))
}
