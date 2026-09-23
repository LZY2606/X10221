use crate::case;
use crate::domain;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::sync::Mutex;

pub const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_initial", include_str!("../migrations/0001_initial.sql")),
    ("0002_ui_review", include_str!("../migrations/0002_ui_review.sql")),
];

#[derive(Clone)]
pub struct AppState {
    pub conn: std::sync::Arc<Mutex<Connection>>,
}

pub fn now_string() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix:{seconds}")
}

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    for (version, sql) in MIGRATIONS {
        let applied: Option<String> = conn
            .query_row("SELECT version FROM schema_migrations WHERE version = ?1", params![version], |row| row.get(0))
            .optional()?;
        if applied.is_none() {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES (?1)", params![version])?;
            tx.commit()?;
        }
    }
    Ok(())
}

fn refs_object(conn: &Connection, branch_id: &str) -> rusqlite::Result<Map<String, Value>> {
    let refs_json: String = conn.query_row("SELECT base_refs_json FROM branches WHERE branch_id = ?1", params![branch_id], |row| row.get(0))?;
    let refs: Value = serde_json::from_str(&refs_json).unwrap_or_else(|_| json!({}));
    Ok(refs.as_object().cloned().unwrap_or_default())
}

fn set_refs(conn: &Connection, branch_id: &str, refs: &Map<String, Value>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE branches SET base_refs_json = ?2 WHERE branch_id = ?1",
        params![branch_id, serde_json::to_string(&Value::Object(refs.clone())).unwrap()],
    )?;
    Ok(())
}

fn next_seq(conn: &Connection, branch_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE branch_id = ?1",
        params![branch_id],
        |row| row.get(0),
    )
}

fn insert_candidate(conn: &Connection, candidate_id: &str, kind: &str, branch_id: &str, event_id: &str, object_id: &str, refs: &Value, value: &Value, label: &str, ordinal: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO candidates(candidate_id, candidate_kind, branch_id, event_id, object_stable_id, refs_json, value_json, ordinal, display_label)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            candidate_id,
            kind,
            branch_id,
            event_id,
            object_id,
            serde_json::to_string(refs).unwrap(),
            serde_json::to_string(value).unwrap(),
            ordinal,
            label
        ],
    )?;
    Ok(())
}

fn insert_status(conn: &Connection, candidate_id: &str, event_id: &str, branch_id: &str, status: &str, reason: &str, scope: &Value, ordinal: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO candidate_statuses(candidate_id, event_id, branch_id, status, reason_code, affected_scope_json, ordinal, ui_visible)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
        params![candidate_id, event_id, branch_id, status, reason, serde_json::to_string(scope).unwrap(), ordinal],
    )?;
    Ok(())
}

fn add_event(
    conn: &Connection,
    branch_id: &str,
    kind: &str,
    code: &str,
    refs: Value,
    payload: Value,
    changed_head: bool,
) -> rusqlite::Result<String> {
    let seq = next_seq(conn, branch_id)?;
    let head: Option<String> = conn.query_row("SELECT head_event_id FROM branches WHERE branch_id = ?1", params![branch_id], |row| row.get(0))?;
    let event_id = format!("evt-{branch_id}-{seq:04}");
    conn.execute(
        "INSERT INTO events(event_id, branch_id, seq, parent_event_id, kind, code, refs_json, payload_json, snapshot_json, changed_head, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '', ?10, ?11)",
        params![
            event_id,
            branch_id,
            seq,
            head,
            kind,
            code,
            serde_json::to_string(&refs).unwrap(),
            serde_json::to_string(&payload).unwrap(),
            if changed_head { 1 } else { 0 },
            now_string()
        ],
    )?;
    if changed_head {
        conn.execute(
            "UPDATE branches SET head_event_id = ?2, head_seq = ?3 WHERE branch_id = ?1",
            params![branch_id, event_id, seq],
        )?;
    }
    let snapshot = build_snapshot(conn, branch_id)?;
    conn.execute(
        "UPDATE events SET snapshot_json = ?2 WHERE event_id = ?1",
        params![event_id, serde_json::to_string(&snapshot).unwrap()],
    )?;
    Ok(event_id)
}

fn insert_source(conn: &Connection, branch_id: &str, kind: &str, path: &str, version: &str, content: &Value) -> rusqlite::Result<String> {
    let stable_id = content
        .get("stable_id")
        .and_then(Value::as_str)
        .unwrap_or(kind)
        .to_string();
    let ordinal: i64 = conn.query_row("SELECT COUNT(*) + 1 FROM sources", [], |row| row.get(0))?;
    let event_id = add_event(
        conn,
        branch_id,
        "source",
        "SOURCE_VERSION_RECORDED",
        json!({kind: version}),
        json!({
            "kind": kind,
            "path": path,
            "version": version,
            "stable_id": stable_id,
            "content_hash": domain::stable_hash(content),
            "machine_readable": true
        }),
        true,
    )?;
    conn.execute(
        "INSERT INTO sources(stable_id, version, kind, path, content_json, content_hash, created_event_id, ordinal)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            stable_id,
            version,
            kind,
            path,
            serde_json::to_string(content).unwrap(),
            domain::stable_hash(content),
            event_id,
            ordinal
        ],
    )?;
    refresh_event_snapshot(conn, &event_id)?;
    Ok(event_id)
}

fn refresh_event_snapshot(conn: &Connection, event_id: &str) -> rusqlite::Result<()> {
    let branch_id: String = conn.query_row("SELECT branch_id FROM events WHERE event_id = ?1", params![event_id], |row| row.get(0))?;
    let snapshot = build_snapshot(conn, &branch_id)?;
    conn.execute(
        "UPDATE events SET snapshot_json = ?2 WHERE event_id = ?1",
        params![event_id, serde_json::to_string(&snapshot).unwrap()],
    )?;
    Ok(())
}

fn current_refs(conn: &Connection, branch_id: &str) -> rusqlite::Result<Value> {
    Ok(Value::Object(refs_object(conn, branch_id)?))
}

fn source_content(conn: &Connection, kind: &str, version: &str) -> rusqlite::Result<Value> {
    let json_text: String = conn.query_row(
        "SELECT content_json FROM sources WHERE kind = ?1 AND version = ?2",
        params![kind, version],
        |row| row.get(0),
    )?;
    Ok(serde_json::from_str(&json_text).unwrap_or(Value::Null))
}

fn current_source(conn: &Connection, refs: &Value, kind: &str) -> rusqlite::Result<Value> {
    let version = refs
        .get(kind)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("refs contain {kind}"));
    source_content(conn, kind, version)
}

fn mark_old_stale(
    conn: &Connection,
    branch_id: &str,
    event_id: &str,
    object_ids: &[String],
    reason: &str,
    scope: &Value,
) -> rusqlite::Result<usize> {
    let mut count = 0;
    for object_id in object_ids {
        let mut stmt = conn.prepare(
            "SELECT cs.candidate_id, COALESCE(cs2.status, 'proposed') AS latest_status
             FROM candidates c
             JOIN events e ON c.event_id = e.event_id
             LEFT JOIN candidate_statuses cs2 ON cs2.candidate_id = c.candidate_id AND cs2.event_id = (
                 SELECT s.event_id FROM candidate_statuses s
                 JOIN events se ON s.event_id = se.event_id
                 WHERE s.candidate_id = c.candidate_id AND se.branch_id = ?2
                 ORDER BY se.seq DESC, s.ordinal DESC LIMIT 1
             )
             LEFT JOIN candidate_statuses cs ON cs.candidate_id = c.candidate_id
             WHERE c.object_stable_id = ?1 AND e.branch_id = ?2 AND c.event_id != ?3",
        )?;
        let rows: Vec<(String, String)> = stmt
            .query_map(params![object_id, branch_id, event_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (index, (candidate_id, status)) in rows.into_iter().enumerate() {
            if status == "stale" {
                continue;
            }
            insert_status(conn, &candidate_id, event_id, branch_id, "stale", reason, scope, index as i64)?;
            count += 1;
        }
    }
    Ok(count)
}

fn materialize_derived(conn: &Connection, branch_id: &str, code: &str, trigger: Value) -> rusqlite::Result<String> {
    let refs = current_refs(conn, branch_id)?;
    let fcs_value = current_source(conn, &refs, "fcs_events")?;
    let spill = current_source(conn, &refs, "spillover_metadata")?;
    let matrix_value = current_source(conn, &refs, "compensation_matrix")?;
    let transform_value = current_source(conn, &refs, "transform_parameters")?;
    let gates_value = current_source(conn, &refs, "gate_polygons")?;
    let lineage_value = current_source(conn, &refs, "gate_parent_child")?;
    let fcs: domain::FcsFile = serde_json::from_value(fcs_value).expect("fcs source");
    let matrix: domain::CompensationMatrix = serde_json::from_value(matrix_value).expect("matrix source");
    let transform: domain::TransformParams = serde_json::from_value(transform_value).expect("transform source");
    let gates: domain::GateSet = serde_json::from_value(gates_value).expect("gates source");
    let lineage: domain::Lineage = serde_json::from_value(lineage_value).expect("lineage source");
    let analysis = domain::analyze_matrix(&matrix);
    let epsilon = *transform.parameters.get("boundary_epsilon").unwrap_or(&0.001);
    let payload = json!({
        "trigger": trigger,
        "matrix_version": matrix.version,
        "matrix_verdict": format!("{:?}", analysis.verdict).to_ascii_lowercase(),
        "determinant": analysis.determinant,
        "rcond_estimate": analysis.rcond_estimate,
        "diagnostics": analysis.diagnostics,
        "spillover_metadata_version": refs.get("spillover_metadata"),
        "machine_readable": true
    });
    let event_id = add_event(conn, branch_id, "derivation", code, refs.clone(), payload, true)?;
    if analysis.verdict != domain::MatrixVerdict::Invertible {
        return Ok(event_id);
    }

    let points = domain::derive_points(&fcs, &matrix, &transform, &analysis);
    let memberships = domain::gate_memberships(&points, &gates, epsilon);
    let populations = domain::population_candidates(&fcs, &lineage, &gates, &memberships, &refs);

    for (index, point) in points.iter().enumerate() {
        let raw_values: std::collections::BTreeMap<_, _> = fcs
            .events
            .iter()
            .find(|e| e.event_id == point.event_id)
            .map(|e| e.values.iter().map(|(k, v)| (k.clone(), json!({"raw_repr": v.raw_repr, "value": v.value, "unit": v.unit, "diagnostics": v.diagnostics}))).collect())
            .unwrap_or_default();
        let value = json!({
            "stable_id": format!("scatter-{}", point.event_id),
            "event_id": point.event_id,
            "x": point.x,
            "y": point.y,
            "compensated": point.compensated,
            "raw_values": raw_values,
            "coordinate_basis": point.coordinate_basis,
            "unit": point.unit,
            "diagnostics": point.diagnostics
        });
        let candidate_id = format!("cand-derived-{}-{}", point.event_id, &domain::stable_hash(&json!({"refs": refs, "value": value}))[..12]);
        insert_candidate(conn, &candidate_id, "derived_point", branch_id, &event_id, &format!("event-{}", point.event_id), &refs, &value, &format!("Scatter {}", point.event_id), index as i64)?;
        insert_status(conn, &candidate_id, &event_id, branch_id, "proposed", "derived-coordinate", &json!({"object": point.event_id}), index as i64)?;

        for channel in &matrix.detector_order {
            let raw = raw_values.get(channel).and_then(|v| v.get("value")).cloned().unwrap_or(Value::Null);
            let comp = point.compensated.get(channel).cloned().unwrap_or(Value::Null);
            let diff = json!({
                "stable_id": format!("diff-{}-{}", point.event_id, channel),
                "event_id": point.event_id,
                "channel": channel,
                "before_compensation": raw,
                "after_compensation": comp,
                "before_unit": "ADC-count",
                "after_unit": "ADC-count",
                "transformed_unit": "dimensionless asinh coordinate",
                "coordinate_basis": matrix.coordinate_basis,
                "diagnostics": point.diagnostics
            });
            let diff_id = format!("cand-diff-{}-{}-{}", point.event_id, channel, &domain::stable_hash(&json!({"refs": refs, "value": diff}))[..12]);
            insert_candidate(conn, &diff_id, "compensation_diff", branch_id, &event_id, &format!("diff-{}-{}", point.event_id, channel), &refs, &diff, &format!("Diff {point} {channel}"), index as i64)?;
            insert_status(conn, &diff_id, &event_id, branch_id, "proposed", "compensation-difference", &json!({"object": format!("diff-{}-{}", point.event_id, channel)}), index as i64)?;
        }
    }

    for (index, gate) in gates.gates.iter().enumerate() {
        let rows: Vec<_> = memberships.iter().filter(|m| m.gate_id == gate.gate_id).collect();
        let value = json!({
            "stable_id": format!("gate-result-{}", gate.gate_id),
            "gate_id": gate.gate_id,
            "gate_version": gates.version,
            "name": gate.name,
            "inside_events": rows.iter().filter(|m| m.inside && !m.ambiguous).map(|m| m.event_id.clone()).collect::<Vec<_>>(),
            "boundary_events": rows.iter().filter(|m| m.boundary).map(|m| m.event_id.clone()).collect::<Vec<_>>(),
            "unknown_events": rows.iter().filter(|m| m.ambiguous).map(|m| m.event_id.clone()).collect::<Vec<_>>(),
            "coordinate_basis": gates.coordinate_basis,
            "coordinate_unit": gate.coordinate_unit,
            "polygon": gate.polygon,
            "refs": refs
        });
        let id = format!("cand-gate-result-{}-{}", gate.gate_id, &domain::stable_hash(&value)[..12]);
        insert_candidate(conn, &id, "gate_result", branch_id, &event_id, &gate.stable_id, &refs, &value, &gate.name, index as i64)?;
        insert_status(conn, &id, &event_id, branch_id, "proposed", "gate-membership-candidates", &json!({"object": gate.stable_id}), index as i64)?;
    }

    for (index, population) in populations.iter().enumerate() {
        let value = serde_json::to_value(population).unwrap();
        insert_candidate(conn, &population.candidate_id, "population", branch_id, &event_id, &population.population_id, &refs, &value, &population.population_id, index as i64)?;
        let reason = if population.diagnostics.is_empty() { "deterministic-population" } else { "all-legal-candidates-retained" };
        insert_status(conn, &population.candidate_id, &event_id, branch_id, "proposed", reason, &json!({"object": population.population_id}), index as i64)?;
    }
    refresh_event_snapshot(conn, &event_id)?;
    Ok(event_id)
}

pub fn build_snapshot(conn: &Connection, branch_id: &str) -> rusqlite::Result<Value> {
    let (head_event_id, head_seq, refs_json): (Option<String>, i64, String) = conn.query_row(
        "SELECT head_event_id, head_seq, base_refs_json FROM branches WHERE branch_id = ?1",
        params![branch_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let refs: Value = serde_json::from_str(&refs_json).unwrap_or_else(|_| json!({}));

    let mut sources_stmt = conn.prepare(
        "SELECT stable_id, version, kind, path, content_json, content_hash, created_event_id, ordinal
         FROM sources ORDER BY ordinal",
    )?;
    let sources: Vec<Value> = sources_stmt
        .query_map([], |row| {
            let content_json: String = row.get(4)?;
            Ok(json!({
                "stable_id": row.get::<_, String>(0)?,
                "version": row.get::<_, String>(1)?,
                "kind": row.get::<_, String>(2)?,
                "path": row.get::<_, String>(3)?,
                "content": serde_json::from_str::<Value>(&content_json).unwrap_or(Value::Null),
                "content_hash": row.get::<_, String>(5)?,
                "created_event_id": row.get::<_, String>(6)?,
                "ordinal": row.get::<_, i64>(7)?
            }))
        })?
        .collect::<Result<_, _>>()?;

    let mut latest = std::collections::BTreeMap::<String, Value>::new();
    let mut stmt = conn.prepare(
        "SELECT c.candidate_id, c.candidate_kind, c.object_stable_id, c.refs_json, c.value_json, c.display_label,
                se.seq, s.status, s.reason_code, s.affected_scope_json, s.ordinal
         FROM candidate_statuses s
         JOIN candidates c ON c.candidate_id = s.candidate_id
         JOIN events se ON se.event_id = s.event_id
         WHERE s.branch_id = ?1
         ORDER BY se.seq DESC, s.ordinal DESC",
    )?;
    let rows = stmt.query_map(params![branch_id], |row| {
        let refs: String = row.get(3)?;
        let value: String = row.get(4)?;
        let scope: String = row.get(9)?;
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            serde_json::from_str::<Value>(&refs).unwrap_or(Value::Null),
            serde_json::from_str::<Value>(&value).unwrap_or(Value::Null),
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            serde_json::from_str::<Value>(&scope).unwrap_or(Value::Null),
            row.get::<_, i64>(10)?,
        ))
    })?;
    for row in rows {
        let (id, kind, object_id, candidate_refs, value, label, seq, status, reason, scope, ordinal) = row?;
        if !latest.contains_key(&id) {
            latest.insert(
                id.clone(),
                json!({
                    "stable_id": id,
                    "candidate_id": id,
                    "candidate_kind": kind,
                    "object_stable_id": object_id,
                    "refs": candidate_refs,
                    "value": value,
                    "display_label": label,
                    "status": status,
                    "reason_code": reason,
                    "affected_scope": scope,
                    "status_seq": seq,
                    "ordinal": ordinal
                }),
            );
        }
    }
    let mut candidates: Vec<Value> = latest.into_values().collect();
    candidates.sort_by_key(|item| (item["candidate_kind"].as_str().unwrap_or("").to_string(), item["ordinal"].as_i64().unwrap_or(0), item["candidate_id"].as_str().unwrap_or("").to_string()));

    let gates: Value = if refs.get("gate_polygons").and_then(Value::as_str).is_some() {
        current_source(conn, &refs, "gate_polygons")?
    } else {
        Value::Null
    };
    let lineage: Value = if refs.get("gate_parent_child").and_then(Value::as_str).is_some() {
        current_source(conn, &refs, "gate_parent_child")?
    } else {
        Value::Null
    };

    let mut events_stmt = conn.prepare(
        "SELECT event_id, parent_event_id, kind, code, refs_json, payload_json, changed_head, seq, created_at
         FROM events WHERE branch_id = ?1 ORDER BY seq",
    )?;
    let events: Vec<Value> = events_stmt
        .query_map(params![branch_id], |row| {
            let refs: String = row.get(4)?;
            let payload: String = row.get(5)?;
            Ok(json!({
                "event_id": row.get::<_, String>(0)?,
                "parent_event_id": row.get::<_, Option<String>>(1)?,
                "kind": row.get::<_, String>(2)?,
                "code": row.get::<_, String>(3)?,
                "refs": serde_json::from_str::<Value>(&refs).unwrap_or(Value::Null),
                "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                "changed_head": row.get::<_, i64>(6)? == 1,
                "seq": row.get::<_, i64>(7)?,
                "created_at": row.get::<_, String>(8)?
            }))
        })?
        .collect::<Result<_, _>>()?;

    let mut decisions_stmt = conn.prepare(
        "SELECT d.decision_id, d.candidate_id, d.action, d.rationale, d.expected_head_event_id, d.created_at,
                c.candidate_kind, c.object_stable_id, c.value_json
         FROM decisions d JOIN candidates c ON c.candidate_id = d.candidate_id
         WHERE d.branch_id = ?1 ORDER BY d.rowid",
    )?;
    let decisions: Vec<Value> = decisions_stmt
        .query_map(params![branch_id], |row| {
            let value: String = row.get(8)?;
            Ok(json!({
                "decision_id": row.get::<_, String>(0)?,
                "candidate_id": row.get::<_, String>(1)?,
                "action": row.get::<_, String>(2)?,
                "rationale": row.get::<_, String>(3)?,
                "expected_head_event_id": row.get::<_, String>(4)?,
                "created_at": row.get::<_, String>(5)?,
                "candidate_kind": row.get::<_, String>(6)?,
                "object_stable_id": row.get::<_, String>(7)?,
                "candidate_value": serde_json::from_str::<Value>(&value).unwrap_or(Value::Null)
            }))
        })?
        .collect::<Result<_, _>>()?;

    Ok(json!({
        "branch_id": branch_id,
        "head_event_id": head_event_id,
        "head_seq": head_seq,
        "refs": refs,
        "sources": sources,
        "scatter_points": candidates.iter().filter(|c| c["candidate_kind"] == "derived_point").cloned().collect::<Vec<_>>(),
        "compensation_diffs": candidates.iter().filter(|c| c["candidate_kind"] == "compensation_diff").cloned().collect::<Vec<_>>(),
        "gate_results": candidates.iter().filter(|c| c["candidate_kind"] == "gate_result").cloned().collect::<Vec<_>>(),
        "populations": candidates.iter().filter(|c| c["candidate_kind"] == "population").cloned().collect::<Vec<_>>(),
        "candidates": candidates,
        "gate_polygons": gates,
        "lineage": lineage,
        "events": events,
        "decisions": decisions
    }))
}

#[derive(Debug)]
pub struct ConflictError {
    pub code: String,
    pub message: String,
    pub diff: Value,
}

pub enum OpError {
    Sql(rusqlite::Error),
    Conflict(ConflictError),
    Invalid(String),
}

impl From<rusqlite::Error> for OpError {
    fn from(error: rusqlite::Error) -> Self {
        OpError::Sql(error)
    }
}

fn derive_stale_objects() -> Vec<String> {
    let mut objects = vec![];
    for event in ["E01", "E02", "E03", "E04", "E05", "E06"] {
        objects.push(format!("event-{event}"));
        for channel in ["FITC-A", "PE-A"] {
            objects.push(format!("diff-{event}-{channel}"));
        }
    }
    objects.extend(["gate-lym", "gate-sing", "gate-cd3", "gate-cd4"].map(str::to_string));
    objects.extend(["POP-LYM", "POP-SING", "POP-CD3", "POP-CD4"].map(str::to_string));
    objects
}

impl AppState {
    pub fn with_connection(conn: Connection) -> Self {
        Self { conn: std::sync::Arc::new(Mutex::new(conn)) }
    }

    pub fn load_or_import_case(&self) -> Result<Value, OpError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let loaded: Option<String> = tx
            .query_row("SELECT value FROM archive_meta WHERE key = 'case_id'", [], |row| row.get(0))
            .optional()?;
        if loaded.as_deref() == Some(case::CASE_ID) {
            return Ok(build_snapshot(&tx, "baseline")?);
        }
        import_case_tx(&tx)?;
        tx.commit()?;
        Ok(self.snapshot("baseline")?)
    }

    pub fn snapshot(&self, branch_id: &str) -> Result<Value, OpError> {
        let conn = self.conn.lock().unwrap();
        Ok(build_snapshot(&conn, branch_id)?)
    }

    pub fn event_snapshot(&self, event_id: &str) -> Result<Value, OpError> {
        let conn = self.conn.lock().unwrap();
        let snapshot: String = conn.query_row("SELECT snapshot_json FROM events WHERE event_id = ?1", params![event_id], |row| row.get(0))?;
        Ok(serde_json::from_str(&snapshot).unwrap_or_else(|_| json!({})))
    }

    pub fn branches(&self) -> Result<Vec<Value>, OpError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT branch_id, parent_branch_id, parent_event_id, head_event_id, head_seq, base_refs_json, created_at
             FROM branches ORDER BY created_at, branch_id",
        )?;
        let rows = stmt.query_map([], |row| {
            let refs: String = row.get(5)?;
            Ok(json!({
                "branch_id": row.get::<_, String>(0)?,
                "parent_branch_id": row.get::<_, Option<String>>(1)?,
                "parent_event_id": row.get::<_, Option<String>>(2)?,
                "head_event_id": row.get::<_, Option<String>>(3)?,
                "head_seq": row.get::<_, i64>(4)?,
                "refs": serde_json::from_str::<Value>(&refs).unwrap_or(Value::Null),
                "created_at": row.get::<_, String>(6)?
            }))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn matrix_singular(&self, branch_id: &str) -> Result<Value, OpError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut refs = refs_object(&tx, branch_id)?;
        let matrix = json!({
            "stable_id": "compensation-matrix-v2-singular",
            "version": "matrix-v2-singular",
            "detector_order": ["FITC-A", "PE-A"],
            "rows": [[1.0, 0.25], [4.0, 1.0]],
            "unit": "dimensionless spillover fraction",
            "coordinate_basis": "M * compensated_signal = raw_signal",
            "source_metadata": "spill-v1"
        });
        refs.insert("compensation_matrix".to_string(), json!("matrix-v2-singular"));
        set_refs(&tx, branch_id, &refs)?;
        insert_source(&tx, branch_id, "compensation_matrix", "cases/flow-boundary-chamber/compensation-v2-singular.json", "matrix-v2-singular", &matrix)?;
        let event_id = materialize_derived(&tx, branch_id, "COMPENSATION_MATRIX_SINGULAR", json!({"scenario": "matrix-singular", "direct_source": "compensation_matrix"}))?;
        let scope = json!({"direct_source": "compensation_matrix", "matrix_version": "matrix-v2-singular", "affected": derive_stale_objects(), "preserved": true});
        let count = mark_old_stale(&tx, branch_id, &event_id, &derive_stale_objects(), "upstream-matrix-singular-stale", &scope)?;
        let judgment_payload = json!({
            "code": "NEAR_OR_EXACT_SINGULAR_COMPENSATION_MATRIX_NOT_INVERTED",
            "machine_readable": true,
            "matrix_version": "matrix-v2-singular",
            "rule": "detector spillover matrix must pass determinant/rcond gate before inverse",
            "stale_candidate_count": count,
            "affected_scope": scope
        });
        let judgment_event = add_event(&tx, branch_id, "judgment", "COMPENSATION_MATRIX_SINGULAR", Value::Object(refs.clone()), judgment_payload, true)?;
        refresh_event_snapshot(&tx, &judgment_event)?;
        tx.commit()?;
        Ok(self.snapshot(branch_id)?)
    }

    pub fn source_local_revision(&self, branch_id: &str) -> Result<Value, OpError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut refs = refs_object(&tx, branch_id)?;
        let mut fcs: Value = current_source(&tx, &Value::Object(refs.clone()), "fcs_events")?;
        fcs["events"][1]["values"]["FITC-A"]["raw_repr"] = json!("190");
        fcs["events"][1]["values"]["FITC-A"]["value"] = json!(190.0);
        fcs["events"][1]["values"]["FITC-A"]["diagnostics"]
            .as_array_mut()
            .unwrap()
            .push(json!("local-source-revision-events-v2"));
        fcs["version"] = json!("events-v2");
        refs.insert("fcs_events".to_string(), json!("events-v2"));
        set_refs(&tx, branch_id, &refs)?;
        insert_source(&tx, branch_id, "fcs_events", "cases/flow-boundary-chamber/fcs-events-v2.json", "events-v2", &fcs)?;
        let event_id = materialize_derived(&tx, branch_id, "SOURCE_REVISION_LOCAL_DEPENDENCY", json!({"scenario": "extreme-signal-and-local-source-revision", "direct_source": "fcs_events", "changed_events": ["E02"]}))?;
        let affected = ["event-E02", "diff-E02-FITC-A", "diff-E02-PE-A", "gate-cd3", "gate-cd4", "POP-CD3", "POP-CD4"].map(str::to_string);
        let unaffected = ["POP-LYM", "POP-SING", "gate-lym", "gate-sing"].map(str::to_string);
        let scope = json!({"direct_source": "fcs_events", "changed_events": ["E02"], "affected": affected, "unaffected": unaffected, "preserved_without_global_replacement": true});
        mark_old_stale(&tx, branch_id, &event_id, &affected, "local-source-dependency-stale", &scope)?;
        refresh_event_snapshot(&tx, &event_id)?;
        tx.commit()?;
        Ok(self.snapshot(branch_id)?)
    }

    pub fn gate_geometry_revision(&self, branch_id: &str) -> Result<Value, OpError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut refs = refs_object(&tx, branch_id)?;
        let mut gates: Value = current_source(&tx, &Value::Object(refs.clone()), "gate_polygons")?;
        gates["version"] = json!("gates-v2");
        gates["gates"][2]["polygon"] = json!([[0.40, -0.1], [1.0, -0.1], [1.0, 0.25], [0.40, 0.25]]);
        refs.insert("gate_polygons".to_string(), json!("gates-v2"));
        set_refs(&tx, branch_id, &refs)?;
        insert_source(&tx, branch_id, "gate_polygons", "cases/flow-boundary-chamber/gates-v2.json", "gates-v2", &gates)?;
        let event_id = materialize_derived(&tx, branch_id, "UPSTREAM_GATE_REVISION_STALE", json!({"scenario": "boundary-point-and-updated-gate-geometry", "direct_source": "gate_polygons", "gate": "G-CD3"}))?;
        let affected = ["gate-cd3", "gate-cd4", "POP-CD3", "POP-CD4"].map(str::to_string);
        let scope = json!({"direct_source": "gate_polygons", "gate": "G-CD3", "affected": affected, "denominator_pin": "POP-CD4 percentages remain pinned to old POP-CD3 candidate until reviewed"});
        mark_old_stale(&tx, branch_id, &event_id, &affected, "upstream-gate-revision-stale", &scope)?;
        refresh_event_snapshot(&tx, &event_id)?;
        tx.commit()?;
        Ok(self.snapshot(branch_id)?)
    }

    pub fn gate_nested_rename(&self, branch_id: &str) -> Result<Value, OpError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut refs = refs_object(&tx, branch_id)?;
        let mut gates: Value = current_source(&tx, &Value::Object(refs.clone()), "gate_polygons")?;
        gates["version"] = json!("gates-v3-renamed");
        gates["gates"][2]["name"] = json!("CD3 T cells (reviewed)");
        refs.insert("gate_polygons".to_string(), json!("gates-v3-renamed"));
        set_refs(&tx, branch_id, &refs)?;
        insert_source(&tx, branch_id, "gate_polygons", "cases/flow-boundary-chamber/gates-v3-renamed.json", "gates-v3-renamed", &gates)?;
        let event_id = materialize_derived(&tx, branch_id, "GATE_NESTED_RENAME", json!({"scenario": "nested-gate-rename", "stable_gate_id": "G-CD3", "stable_edge_id": "EDGE-CD3", "changed": "display-name-only", "invalidated": []}))?;
        refresh_event_snapshot(&tx, &event_id)?;
        tx.commit()?;
        Ok(self.snapshot(branch_id)?)
    }

    pub fn create_draft(&self, branch_id: &str, draft_id: &str) -> Result<Value, OpError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let (parent_event, parent_seq, refs_json): (Option<String>, i64, String) = tx.query_row(
            "SELECT head_event_id, head_seq, base_refs_json FROM branches WHERE branch_id = ?1",
            params![branch_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        tx.execute(
            "INSERT INTO branches(branch_id, parent_branch_id, parent_event_id, base_refs_json, head_event_id, head_seq, created_at)
             VALUES (?1, ?2, ?3, ?4, NULL, 0, ?5)",
            params![draft_id, branch_id, parent_event, refs_json, now_string()],
        )?;
        let refs: Value = serde_json::from_str(&refs_json).unwrap_or_else(|_| json!({}));
        let event_id = add_event(
            &tx,
            draft_id,
            "draft",
            "DRAFT_OPENED_FOR_SIDE_BY_SIDE_REVIEW",
            refs,
            json!({"parent_branch_id": branch_id, "parent_event_id": parent_event, "stable_identities_copied": false, "content_addressed_candidates_shared": true}),
            true,
        )?;
        let mut stmt = tx.prepare(
            "SELECT candidate_id, status, reason_code, affected_scope_json, ordinal FROM candidate_statuses
             WHERE branch_id = ?1 AND event_id = (
                 SELECT s.event_id FROM candidate_statuses s JOIN events e ON e.event_id = s.event_id
                 WHERE s.branch_id = ?1 ORDER BY e.seq DESC, s.ordinal DESC LIMIT 1
             )",
        )?;
        let rows: Vec<(String, String, String, String, i64)> = stmt
            .query_map(params![branch_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?
            .collect::<Result<_, _>>()?;
        for (candidate_id, status, reason, scope, ordinal) in rows {
            insert_status(&tx, &candidate_id, &event_id, draft_id, &status, &reason, &serde_json::from_str::<Value>(&scope).unwrap_or(Value::Null), ordinal)?;
        }
        refresh_event_snapshot(&tx, &event_id)?;
        tx.execute("INSERT INTO review_drafts(draft_id, branch_id, expected_head_event_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![draft_id, draft_id, event_id, now_string()])?;
        tx.commit()?;
        Ok(self.snapshot(draft_id)?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn decide(&self, branch_id: &str, expected_head_event_id: &str, candidate_id: &str, action: &str, rationale: &str, draft_label: &str) -> Result<Value, OpError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let head: Option<String> = tx.query_row("SELECT head_event_id FROM branches WHERE branch_id = ?1", params![branch_id], |row| row.get(0))?;
        if head.as_deref() != Some(expected_head_event_id) {
            let refs = current_refs(&tx, branch_id)?;
            let payload = json!({
                "code": "CONCURRENT_VERSION_CONFLICT",
                "machine_readable": true,
                "draft_label": draft_label,
                "expected_head_event_id": expected_head_event_id,
                "actual_head_event_id": head,
                "candidate_id": candidate_id,
                "diff": {
                    "expected_head": expected_head_event_id,
                    "committed_head": head,
                    "requested_candidate": candidate_id
                }
            });
            add_event(&tx, branch_id, "conflict", "CONCURRENT_VERSION_CONFLICT", refs, payload.clone(), false)?;
            tx.commit()?;
            return Err(OpError::Conflict(ConflictError {
                code: "CONCURRENT_VERSION_CONFLICT".to_string(),
                message: "同一基础版本已有先提交裁定；请刷新并查看双方差异".to_string(),
                diff: payload,
            }));
        }

        let previous_status: String = tx
            .query_row(
                "SELECT status FROM candidate_statuses s JOIN events e ON e.event_id = s.event_id
                 WHERE s.candidate_id = ?1 AND s.branch_id = ?2 ORDER BY e.seq DESC, s.ordinal DESC LIMIT 1",
                params![candidate_id, branch_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "proposed".to_string());
        let refs = current_refs(&tx, branch_id)?;
        let code = match action {
            "accept" => "DECISION_ACCEPTED",
            "reject" => "DECISION_REJECTED",
            "adjust" => "DECISION_PARAMETER_ADJUSTED",
            _ => "DECISION_RECORDED",
        };
        let payload = json!({
            "machine_readable": true,
            "candidate_id": candidate_id,
            "action": action,
            "rationale": rationale,
            "previous_status": previous_status,
            "unadopted_candidates_preserved": true,
            "raw_sources_immutable": true,
            "draft_label": draft_label
        });
        let event_id = add_event(&tx, branch_id, "decision", code, refs.clone(), payload, true)?;
        let next_status = match action {
            "accept" => "accepted",
            "reject" => "rejected",
            "adjust" => "adjusted",
            _ => "recorded",
        };
        insert_status(&tx, candidate_id, &event_id, branch_id, next_status, code, &json!({"decision_event_id": event_id}), 0)?;
        let decision_id = format!("dec-{branch_id}-{}", &event_id[4..]);
        tx.execute(
            "INSERT INTO decisions(decision_id, branch_id, event_id, candidate_id, action, rationale, expected_head_event_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![decision_id, branch_id, event_id, candidate_id, action, rationale, expected_head_event_id, now_string()],
        )?;
        refresh_event_snapshot(&tx, &event_id)?;
        tx.commit()?;
        Ok(self.snapshot(branch_id)?)
    }

    pub fn undo_last_decision(&self, branch_id: &str) -> Result<Value, OpError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let (event_id, candidate_id, action): (String, String, String) = tx.query_row(
            "SELECT d.event_id, d.candidate_id, d.action FROM decisions d
             WHERE d.branch_id = ?1 ORDER BY d.rowid DESC LIMIT 1",
            params![branch_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let previous_status: String = tx
            .query_row(
                "SELECT payload_json FROM events WHERE event_id = ?1",
                params![event_id],
                |row| row.get::<_, String>(0)),
            .ok()
            .and_then(|json_text| serde_json::from_str::<Value>(&json_text).ok())
            .and_then(|payload| payload.get("previous_status").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| "proposed".to_string());
        let refs = current_refs(&tx, branch_id)?;
        let undo_event = add_event(
            &tx,
            branch_id,
            "undo",
            "DECISION_UNDONE",
            refs,
            json!({"undone_event_id": event_id, "candidate_id": candidate_id, "action": action, "restored_status": previous_status, "raw_sources_immutable": true}),
            true,
        )?;
        insert_status(&tx, &candidate_id, &undo_event, branch_id, &previous_status, "DECISION_UNDONE", &json!({"restored_from": event_id}), 0)?;
        refresh_event_snapshot(&tx, &undo_event)?;
        tx.commit()?;
        Ok(self.snapshot(branch_id)?)
    }
}

fn import_case_tx(conn: &Connection) -> rusqlite::Result<()> {
    let offline = case::load_case();
    conn.execute(
        "INSERT INTO branches(branch_id, parent_branch_id, parent_event_id, base_refs_json, head_event_id, head_seq, created_at)
         VALUES ('baseline', NULL, NULL, '{}', NULL, 0, ?1)",
        params![now_string()],
    )?;
    set_refs(conn, "baseline", case::initial_refs().as_object().unwrap())?;
    let materials = [
        ("fcs_events", "cases/flow-boundary-chamber/fcs-events.json", "events-v1", serde_json::to_value(&offline.fcs).unwrap()),
        ("spillover_metadata", "cases/flow-boundary-chamber/spillover.json", "spill-v1", offline.spillover),
        ("compensation_matrix", "cases/flow-boundary-chamber/compensation-v1.json", "matrix-v1", serde_json::to_value(&offline.matrix).unwrap()),
        ("transform_parameters", "cases/flow-boundary-chamber/transform-v1.json", "transform-v1", serde_json::to_value(&offline.transform).unwrap()),
        ("gate_polygons", "cases/flow-boundary-chamber/gates-v1.json", "gates-v1", serde_json::to_value(&offline.gates).unwrap()),
        ("gate_parent_child", "cases/flow-boundary-chamber/lineage-v1.json", "lineage-v1", serde_json::to_value(&offline.lineage).unwrap()),
    ];
    for (kind, path, version, content) in materials {
        insert_source(conn, "baseline", kind, path, version, &content)?;
    }
    let derived_event = materialize_derived(
        conn,
        "baseline",
        "BASELINE_DERIVATION",
        json!({"scenario": "initial-import", "atomic_batch": true, "irreversible_cleaning": false}),
    )?;
    let judgments = json!({
        "machine_readable": true,
        "derived_event_id": derived_event,
        "judgments": [
            {"code": "EXTREME_SIGNAL_PRESERVED", "events": ["E06"], "rule": "unconvertible OVERFLOW remains in raw channel value with diagnostics"},
            {"code": "BOUNDARY_POINT_INCLUSIVE_RULE_RECORDED", "events": ["E04"], "rule": "closed polygon edge is inclusive within transform boundary epsilon"},
            {"code": "AMBIGUOUS_CANDIDATES_RETAINED", "events": ["E06"], "rule": "all legal parent/child gate outcomes are stored"},
            {"code": "POPULATION_DENOMINATOR_PINNED_TO_PARENT_VERSION", "populations": ["POP-CD3", "POP-CD4"], "rule": "percent candidate stores exact parent candidate id"},
            {"code": "NEAR_SINGULAR_COMPENSATION_MATRIX_MAY_NOT_BE_SILENTLY_INVERTED", "threshold_determinant": domain::NEAR_SINGULAR_DETERMINANT, "threshold_rcond": domain::NEAR_SINGULAR_RCOND}
        ]
    });
    let judgment_event = add_event(conn, "baseline", "judgment", "BASELINE_FLOW_BOUNDARY_JUDGMENTS", case::initial_refs(), judgments, true)?;
    refresh_event_snapshot(conn, &judgment_event)?;
    conn.execute("INSERT INTO archive_meta(key, value) VALUES ('case_id', ?1), ('case_title', ?2), ('format', 'flow-boundary-chamber-archive-v1')",
        params![case::CASE_ID, case::CASE_TITLE])?;
    Ok(())
}

impl AppState {
    pub fn export_archive(&self) -> Result<Value, OpError> {
        let conn = self.conn.lock().unwrap();
        let table = |sql: &'static str| -> rusqlite::Result<Vec<Vec<Value>>> {
            let mut stmt = conn.prepare(sql)?;
            let count = stmt.column_count();
            let rows = stmt.query_map([], |row| {
                let mut out = Vec::new();
                for index in 0..count {
                    let value = match row.get_ref(index)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(v) => json!(v),
                        rusqlite::types::ValueRef::Real(v) => json!(v),
                        rusqlite::types::ValueRef::Text(v) => Value::String(String::from_utf8_lossy(v).to_string()),
                        rusqlite::types::ValueRef::Blob(v) => json!(v),
                    };
                    out.push(value);
                }
                Ok(out)
            })?;
            rows.collect()
        };
        Ok(json!({
            "archive_format": "flow-boundary-chamber-archive-v1",
            "exported_at": now_string(),
            "tables": {
                "archive_meta": table("SELECT key, value FROM archive_meta ORDER BY key")?,
                "sources": table("SELECT stable_id, version, kind, path, content_json, content_hash, created_event_id, ordinal FROM sources ORDER BY ordinal, stable_id, version")?,
                "branches": table("SELECT branch_id, parent_branch_id, parent_event_id, base_refs_json, head_event_id, head_seq, created_at FROM branches ORDER BY created_at, branch_id")?,
                "events": table("SELECT event_id, branch_id, seq, parent_event_id, kind, code, refs_json, payload_json, snapshot_json, changed_head, created_at FROM events ORDER BY branch_id, seq")?,
                "candidates": table("SELECT candidate_id, candidate_kind, branch_id, event_id, object_stable_id, refs_json, value_json, ordinal, display_label FROM candidates ORDER BY branch_id, ordinal, candidate_id")?,
                "candidate_statuses": table("SELECT candidate_id, event_id, branch_id, status, reason_code, affected_scope_json, ordinal, ui_visible FROM candidate_statuses ORDER BY branch_id, event_id, ordinal, candidate_id")?,
                "decisions": table("SELECT decision_id, branch_id, event_id, candidate_id, action, rationale, expected_head_event_id, created_at FROM decisions ORDER BY branch_id, rowid")?,
                "review_drafts": table("SELECT draft_id, branch_id, expected_head_event_id, created_at FROM review_drafts ORDER BY created_at, draft_id")?
            }
        }))
    }

    pub fn import_archive(&self, archive: &Value) -> Result<Value, OpError> {
        if archive.get("archive_format").and_then(Value::as_str) != Some("flow-boundary-chamber-archive-v1") {
            return Err(OpError::Invalid("archive format is not flow-boundary-chamber-archive-v1".to_string()));
        }
        let tables = archive.get("tables").and_then(Value::as_object).ok_or(rusqlite::Error::InvalidQuery)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for table_name in [
            "review_drafts", "decisions", "candidate_statuses", "candidates", "events", "branches", "sources", "archive_meta",
        ] {
            tx.execute(format!("DELETE FROM {table_name}").as_str(), [])?;
        }
        let insert_rows = |conn: &Connection, name: &str, sql: &str, columns: usize| -> rusqlite::Result<()> {
            let rows = tables
                .get(name)
                .and_then(Value::as_array)
                .ok_or(rusqlite::Error::InvalidQuery)?;
            for row in rows {
                let values = row.as_array().ok_or(rusqlite::Error::InvalidQuery)?;
                if values.len() != columns {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                let params: Vec<&dyn rusqlite::ToSql> = values
                    .iter()
                    .map(|value| -> Box<dyn rusqlite::ToSql> {
                        match value {
                            Value::Null => Box::new(rusqlite::types::Null),
                            Value::Bool(v) => Box::new(if *v { 1 } else { 0 }),
                            Value::Number(v) => {
                                if let Some(i) = v.as_i64() {
                                    Box::new(i)
                                } else {
                                    Box::new(v.as_f64().unwrap_or(0.0))
                                }
                            }
                            Value::String(v) => Box::new(v.clone()),
                            _ => Box::new(value.to_string()),
                        }
                    })
                    .collect();
                conn.execute(sql, params.as_slice())?;
            }
            Ok(())
        };
        insert_rows(&tx, "archive_meta", "INSERT INTO archive_meta(key, value) VALUES (?1, ?2)", 2).map_err(|_| OpError::Invalid("invalid archive_meta rows".to_string()))?;
        insert_rows(&tx, "sources", "INSERT INTO sources(stable_id, version, kind, path, content_json, content_hash, created_event_id, ordinal) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)", 8)?;
        insert_rows(&tx, "branches", "INSERT INTO branches(branch_id, parent_branch_id, parent_event_id, base_refs_json, head_event_id, head_seq, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", 7)?;
        insert_rows(&tx, "events", "INSERT INTO events(event_id, branch_id, seq, parent_event_id, kind, code, refs_json, payload_json, snapshot_json, changed_head, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)", 11)?;
        insert_rows(&tx, "candidates", "INSERT INTO candidates(candidate_id, candidate_kind, branch_id, event_id, object_stable_id, refs_json, value_json, ordinal, display_label) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)", 9)?;
        insert_rows(&tx, "candidate_statuses", "INSERT INTO candidate_statuses(candidate_id, event_id, branch_id, status, reason_code, affected_scope_json, ordinal, ui_visible) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)", 8)?;
        insert_rows(&tx, "decisions", "INSERT INTO decisions(decision_id, branch_id, event_id, candidate_id, action, rationale, expected_head_event_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)", 8)?;
        insert_rows(&tx, "review_drafts", "INSERT INTO review_drafts(draft_id, branch_id, expected_head_event_id, created_at) VALUES (?1, ?2, ?3, ?4)", 4)?;
        tx.commit()?;
        drop(conn);
        Ok(self.snapshot("baseline")?)
    }
}
