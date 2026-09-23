//! Dependency-driven invalidation. When an upstream artefact changes, only
//! derived results whose recorded dependency set actually references it are
//! marked stale. Nothing is recomputed or overwritten behind the scenes.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::json;

fn mark_stale_where(
    conn: &Connection,
    branch: &str,
    reason: &str,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> Result<Vec<String>> {
    let current = crate::store::current_results(conn, branch)?;
    let mut marked = Vec::new();
    for r in current {
        if predicate(&r.deps) {
            conn.execute(
                "UPDATE derived_results SET status = 'stale', stale_reason = ?1
                 WHERE id = ?2",
                rusqlite::params![reason, r.id],
            )?;
            marked.push(r.id);
        }
    }
    Ok(marked)
}

/// A gate was revised: results depending on that gate (itself or descendants,
/// all captured in the dependency closure) become stale.
pub fn invalidate_for_gate(
    conn: &Connection,
    branch: &str,
    gate_id: &str,
    new_version: i64,
    actor: Option<&str>,
) -> Result<Vec<String>> {
    let reason = format!("gate {gate_id} revised to v{new_version}");
    let marked = mark_stale_where(conn, branch, &reason, |deps| {
        deps.get("gates")
            .and_then(|g| g.get(gate_id))
            .is_some()
    })?;
    crate::db::emit_event(
        conn,
        branch,
        "lineage.stale_marked",
        actor,
        &json!({
            "trigger": "gate_revised",
            "gate_id": gate_id,
            "new_version": new_version,
            "stale_results": marked,
            "rule": "upstream revision marks downstream stale; results are never overwritten",
        }),
    )?;
    Ok(marked)
}

pub fn invalidate_for_transform(
    conn: &Connection,
    branch: &str,
    channel: &str,
    new_version: i64,
    actor: Option<&str>,
) -> Result<Vec<String>> {
    let reason = format!("transform for {channel} revised to v{new_version}");
    let marked = mark_stale_where(conn, branch, &reason, |deps| {
        deps.get("transforms")
            .and_then(|t| t.get(channel))
            .is_some()
    })?;
    crate::db::emit_event(
        conn,
        branch,
        "lineage.stale_marked",
        actor,
        &json!({
            "trigger": "transform_revised",
            "channel": channel,
            "new_version": new_version,
            "stale_results": marked,
        }),
    )?;
    Ok(marked)
}

pub fn invalidate_for_matrix(
    conn: &Connection,
    branch: &str,
    matrix_ref: &str,
    actor: Option<&str>,
) -> Result<Vec<String>> {
    let reason = format!("compensation matrix changed to {matrix_ref}");
    let marked = mark_stale_where(conn, branch, &reason, |deps| {
        deps.get("matrix").and_then(|m| m.as_str()).is_some()
    })?;
    crate::db::emit_event(
        conn,
        branch,
        "lineage.stale_marked",
        actor,
        &json!({
            "trigger": "matrix_changed",
            "matrix": matrix_ref,
            "stale_results": marked,
        }),
    )?;
    Ok(marked)
}

/// A source amendment touches specific channels only; invalidation is
/// limited to results whose dependency set names those channels.
pub fn invalidate_for_source_channels(
    conn: &Connection,
    branch: &str,
    source_id: &str,
    channels: &[String],
    new_version: i64,
    actor: Option<&str>,
) -> Result<Vec<String>> {
    let reason = format!(
        "source {source_id} channels [{}] amended to v{new_version}",
        channels.join(", ")
    );
    let keys: Vec<String> = channels
        .iter()
        .map(|c| format!("{source_id}:{c}"))
        .collect();
    let marked = mark_stale_where(conn, branch, &reason, |deps| {
        deps.get("channel_values")
            .and_then(|cv| cv.as_object())
            .map(|obj| keys.iter().any(|k| obj.contains_key(k)))
            .unwrap_or(false)
    })?;
    crate::db::emit_event(
        conn,
        branch,
        "lineage.stale_marked",
        actor,
        &json!({
            "trigger": "source_amended",
            "source_id": source_id,
            "channels": channels,
            "new_version": new_version,
            "stale_results": marked,
            "rule": "local invalidation: only results depending on amended channels",
        }),
    )?;
    Ok(marked)
}
