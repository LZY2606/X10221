//! Typed readers over the versioned store. Nothing here ever updates or
//! deletes historical rows; new states are new versions.

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRow {
    pub id: String,
    pub version: i64,
    pub draft: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub parent_version: Option<i64>,
    pub x_channel: String,
    pub y_channel: String,
    pub basis: String,
    pub unit: String,
    pub polygon: Vec<[f64; 2]>,
    pub seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixRow {
    pub id: String,
    pub version: i64,
    pub fluorochromes: Vec<String>,
    pub detectors: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub condition_number: Option<f64>,
    pub status: String,
    pub seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformRow {
    pub id: String,
    pub version: i64,
    pub channel: String,
    pub transform: crate::transform::Transform,
    pub basis: String,
    pub unit: String,
    pub seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedResult {
    pub id: String,
    pub branch_id: String,
    pub draft: String,
    pub gate_id: String,
    pub gate_version: i64,
    pub count: i64,
    pub denominator_gate_id: Option<String>,
    pub denominator_gate_version: Option<i64>,
    pub denominator_count: i64,
    pub percent: Option<f64>,
    pub deps: serde_json::Value,
    pub status: String,
    pub stale_reason: Option<String>,
    pub boundary_events: Vec<i64>,
    pub diagnostics: Vec<serde_json::Value>,
    pub seq: i64,
}

fn gate_from_row(row: &rusqlite::Row) -> rusqlite::Result<GateRow> {
    let polygon: String = row.get("polygon")?;
    Ok(GateRow {
        id: row.get("id")?,
        version: row.get("version")?,
        draft: row.get("draft")?,
        name: row.get("name")?,
        parent_id: row.get("parent_id")?,
        parent_version: row.get("parent_version")?,
        x_channel: row.get("x_channel")?,
        y_channel: row.get("y_channel")?,
        basis: row.get("basis")?,
        unit: row.get("unit")?,
        polygon: serde_json::from_str(&polygon).unwrap_or_default(),
        seq: row.get("seq")?,
    })
}

const GATE_COLS: &str =
    "id, version, draft, name, parent_id, parent_version, x_channel, y_channel, basis, unit, polygon, seq";

/// Latest version of every gate in a draft, optionally as of an event seq.
pub fn gates_as_of(conn: &Connection, draft: &str, seq: Option<i64>) -> Result<Vec<GateRow>> {
    let sql = format!(
        "SELECT {GATE_COLS} FROM gates g
         WHERE draft = ?1
           AND (?2 IS NULL OR seq <= ?2)
           AND version = (SELECT MAX(version) FROM gates
                          WHERE id = g.id AND draft = g.draft
                            AND (?2 IS NULL OR seq <= ?2))
         ORDER BY id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params![draft, seq], gate_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn gate_version(conn: &Connection, draft: &str, id: &str, version: i64) -> Result<Option<GateRow>> {
    let sql = format!(
        "SELECT {GATE_COLS} FROM gates WHERE id = ?1 AND version = ?2 AND draft = ?3"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map(rusqlite::params![id, version, draft], gate_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.pop())
}

fn matrix_from_row(row: &rusqlite::Row) -> rusqlite::Result<MatrixRow> {
    Ok(MatrixRow {
        id: row.get("id")?,
        version: row.get("version")?,
        fluorochromes: serde_json::from_str(&row.get::<_, String>("fluorochromes")?)
            .unwrap_or_default(),
        detectors: serde_json::from_str(&row.get::<_, String>("detectors")?).unwrap_or_default(),
        matrix: serde_json::from_str(&row.get::<_, String>("matrix")?).unwrap_or_default(),
        condition_number: row.get("condition_number")?,
        status: row.get("status")?,
        seq: row.get("seq")?,
    })
}

/// Latest accepted (status = ok) matrix, optionally as of an event seq.
pub fn current_matrix(conn: &Connection, seq: Option<i64>) -> Result<Option<MatrixRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, version, fluorochromes, detectors, matrix, condition_number, status, seq
         FROM comp_matrices
         WHERE status = 'ok' AND (?1 IS NULL OR seq <= ?1)
         ORDER BY version DESC LIMIT 1",
    )?;
    let mut rows = stmt
        .query_map(rusqlite::params![seq], matrix_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.pop())
}

pub fn all_matrices(conn: &Connection) -> Result<Vec<MatrixRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, version, fluorochromes, detectors, matrix, condition_number, status, seq
         FROM comp_matrices ORDER BY id, version",
    )?;
    let rows = stmt
        .query_map([], matrix_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Latest transform per channel, optionally as of an event seq.
pub fn transforms_as_of(
    conn: &Connection,
    seq: Option<i64>,
) -> Result<HashMap<String, TransformRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, version, channel, kind, params, basis, unit, seq FROM transform_params t
         WHERE (?1 IS NULL OR seq <= ?1)
           AND version = (SELECT MAX(version) FROM transform_params
                          WHERE id = t.id AND channel = t.channel
                            AND (?1 IS NULL OR seq <= ?1))
         ORDER BY channel",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![seq], |row| {
            let kind: String = row.get("kind")?;
            let params: String = row.get("params")?;
            let mut v: serde_json::Value = serde_json::from_str(&params).unwrap_or_default();
            v.as_object_mut()
                .map(|o| o.insert("kind".into(), serde_json::Value::String(kind.clone())));
            Ok(TransformRow {
                id: row.get("id")?,
                version: row.get("version")?,
                channel: row.get("channel")?,
                transform: serde_json::from_value(v).unwrap_or(crate::transform::Transform::Linear),
                basis: row.get("basis")?,
                unit: row.get("unit")?,
                seq: row.get("seq")?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().map(|t| (t.channel.clone(), t)).collect())
}

fn result_from_row(row: &rusqlite::Row) -> rusqlite::Result<DerivedResult> {
    Ok(DerivedResult {
        id: row.get("id")?,
        branch_id: row.get("branch_id")?,
        draft: row.get("draft")?,
        gate_id: row.get("gate_id")?,
        gate_version: row.get("gate_version")?,
        count: row.get("count")?,
        denominator_gate_id: row.get("denominator_gate_id")?,
        denominator_gate_version: row.get("denominator_gate_version")?,
        denominator_count: row.get("denominator_count")?,
        percent: row.get("percent")?,
        deps: serde_json::from_str(&row.get::<_, String>("deps")?).unwrap_or_default(),
        status: row.get("status")?,
        stale_reason: row.get("stale_reason")?,
        boundary_events: serde_json::from_str(&row.get::<_, String>("boundary_events")?)
            .unwrap_or_default(),
        diagnostics: serde_json::from_str(&row.get::<_, String>("diagnostics")?)
            .unwrap_or_default(),
        seq: row.get("seq")?,
    })
}

const RESULT_COLS: &str = "id, branch_id, draft, gate_id, gate_version, count, \
    denominator_gate_id, denominator_gate_version, denominator_count, percent, \
    deps, status, stale_reason, boundary_events, diagnostics, seq";

/// Latest result per gate of a draft on a branch, optionally as of a seq.
pub fn results_as_of(
    conn: &Connection,
    branch: &str,
    draft: &str,
    seq: Option<i64>,
) -> Result<Vec<DerivedResult>> {
    let sql = format!(
        "SELECT {RESULT_COLS} FROM derived_results r
         WHERE branch_id = ?1 AND draft = ?2
           AND (?3 IS NULL OR seq <= ?3)
           AND seq = (SELECT MAX(seq) FROM derived_results
                      WHERE branch_id = r.branch_id AND draft = r.draft
                        AND gate_id = r.gate_id AND (?3 IS NULL OR seq <= ?3))
         ORDER BY gate_id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params![branch, draft, seq], result_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn current_results(conn: &Connection, branch: &str) -> Result<Vec<DerivedResult>> {
    let sql = format!(
        "SELECT {RESULT_COLS} FROM derived_results WHERE branch_id = ?1 AND status = 'current'"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params![branch], result_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn branch_version(conn: &Connection, branch: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT version FROM branches WHERE id = ?1")?;
    let mut rows = stmt
        .query_map(rusqlite::params![branch], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(rows.pop())
}

pub fn bump_branch_version(conn: &Connection, branch: &str) -> Result<i64> {
    conn.execute(
        "UPDATE branches SET version = version + 1 WHERE id = ?1",
        rusqlite::params![branch],
    )?;
    Ok(branch_version(conn, branch)?.unwrap_or(0))
}
