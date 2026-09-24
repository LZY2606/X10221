//! 存储操作：事件追加、版本实体、失效传播、裁定分支、并发冲突、导出导入。
use crate::domain;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Entity {
    pub id: String,
    pub kind: String,
    pub version: i64,
    pub branch: String,
    pub payload: Value,
    pub supersedes: Option<String>,
    pub event_seq: i64,
}

#[derive(Debug, Serialize)]
pub struct Conflict {
    pub error: String,
    pub id: String,
    pub expected_version: i64,
    pub current_version: i64,
    pub your_payload: Value,
    pub current_payload: Value,
}

pub fn append_event(conn: &Connection, kind: &str, payload: &Value) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO events(kind, payload) VALUES (?1, ?2)",
        params![kind, payload.to_string()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn put_entity(
    conn: &Connection,
    id: &str,
    kind: &str,
    version: i64,
    branch: &str,
    payload: &Value,
    supersedes: Option<&str>,
    event_seq: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO entities(id,kind,version,branch,payload,supersedes,event_seq)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![id, kind, version, branch, payload.to_string(), supersedes, event_seq],
    )?;
    Ok(())
}

pub fn latest_version(conn: &Connection, id: &str, branch: &str) -> rusqlite::Result<Option<(i64, Value)>> {
    let mut stmt = conn.prepare(
        "SELECT version, payload FROM entities WHERE id=?1 AND branch=?2
         ORDER BY version DESC LIMIT 1",
    )?;
    let mut rows = stmt.query(params![id, branch])?;
    if let Some(row) = rows.next()? {
        let v: i64 = row.get(0)?;
        let p: String = row.get(1)?;
        Ok(Some((v, serde_json::from_str(&p).unwrap_or(Value::Null))))
    } else {
        Ok(None)
    }
}

pub fn entity_at(conn: &Connection, id: &str, version: i64, branch: &str) -> rusqlite::Result<Option<Value>> {
    let mut stmt = conn.prepare(
        "SELECT payload FROM entities WHERE id=?1 AND version=?2 AND branch=?3",
    )?;
    let mut rows = stmt.query(params![id, version, branch])?;
    if let Some(row) = rows.next()? {
        let p: String = row.get(0)?;
        Ok(Some(serde_json::from_str(&p).unwrap_or(Value::Null)))
    } else {
        Ok(None)
    }
}

/// 乐观并发写：base_version 必须等于当前最新版本，否则返回冲突及双方内容。
pub fn revise_entity(
    conn: &Connection,
    id: &str,
    kind: &str,
    branch: &str,
    base_version: i64,
    payload: &Value,
) -> Result<i64, Conflict> {
    let cur = latest_version(conn, id, branch).map_err(|e| Conflict {
        error: e.to_string(),
        id: id.into(),
        expected_version: base_version,
        current_version: -1,
        your_payload: payload.clone(),
        current_payload: Value::Null,
    })?;
    let (cur_v, cur_p) = cur.unwrap_or((0, Value::Null));
    if cur_v != base_version {
        let seq = append_event(
            conn,
            "conflict.detected",
            &json!({"id": id, "expected": base_version, "current": cur_v}),
        )
        .map_err(|e| Conflict {
            error: e.to_string(),
            id: id.into(),
            expected_version: base_version,
            current_version: cur_v,
            your_payload: payload.clone(),
            current_payload: cur_p.clone(),
        })?;
        let _ = seq;
        return Err(Conflict {
            error: "version_conflict".into(),
            id: id.into(),
            expected_version: base_version,
            current_version: cur_v,
            your_payload: payload.clone(),
            current_payload: cur_p,
        });
    }
    let seq = append_event(
        conn,
        &format!("{kind}.revised"),
        &json!({"id": id, "from": cur_v, "to": cur_v + 1, "branch": branch}),
    )
    .map_err(|e| Conflict {
        error: e.to_string(),
        id: id.into(),
        expected_version: base_version,
        current_version: cur_v,
        your_payload: payload.clone(),
        current_payload: cur_p.clone(),
    })?;
    put_entity(
        conn,
        id,
        kind,
        cur_v + 1,
        branch,
        payload,
        Some(&format!("{id}@{cur_v}")),
        seq,
    )
    .map_err(|e| Conflict {
        error: e.to_string(),
        id: id.into(),
        expected_version: base_version,
        current_version: cur_v,
        your_payload: payload.clone(),
        current_payload: cur_p,
    })?;
    Ok(cur_v + 1)
}

/// 门修订后的失效传播：标记下游群体 stale，不覆盖其数值。
pub fn propagate_stale(conn: &Connection, gate_id: &str, branch: &str) -> rusqlite::Result<Vec<String>> {
    // 找到引用该门任一版本的群体
    let mut stmt = conn.prepare(
        "SELECT pop_id, parent_pop_ref FROM populations WHERE branch=?1",
    )?;
    let rows: Vec<(String, Option<String>)> = stmt
        .query_map(params![branch], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt2 = conn.prepare(
        "SELECT pop_id FROM populations WHERE branch=?1 AND gate_ref LIKE ?2",
    )?;
    let direct: Vec<String> = stmt2
        .query_map(params![branch, format!("{gate_id}@%")], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    // 依赖边：pop -> parent_pop（parent_pop_ref 形如 pop:xxx@n，取 id 部分）
    let edges: Vec<(String, String)> = rows
        .iter()
        .filter_map(|(c, p)| {
            p.as_ref().map(|pr| (c.clone(), pr.split('@').next().unwrap_or("").to_string()))
        })
        .collect();
    let scope = domain::stale_scope(&direct, &edges);
    for pop in &scope {
        conn.execute(
            "UPDATE populations SET status='stale' WHERE pop_id=?1 AND branch=?2",
            params![pop, branch],
        )?;
    }
    if !scope.is_empty() {
        append_event(
            conn,
            "population.staled",
            &json!({"gate": gate_id, "stale": scope, "branch": branch}),
        )?;
    }
    Ok(scope)
}

pub fn record_decision(
    conn: &Connection,
    decision_id: &str,
    subject: &str,
    verdict: &str,
    payload: &Value,
) -> rusqlite::Result<()> {
    let prev_head: Option<String> = conn
        .query_row("SELECT head FROM heads WHERE subject=?1", params![subject], |r| r.get(0))
        .ok();
    let seq = append_event(
        conn,
        "decision.made",
        &json!({"decision_id": decision_id, "subject": subject, "verdict": verdict, "payload": payload}),
    )?;
    conn.execute(
        "INSERT INTO decisions(decision_id,subject,verdict,payload,prev_head,event_seq)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![decision_id, subject, verdict, payload.to_string(), prev_head, seq],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO heads(subject, head) VALUES (?1, ?2)",
        params![subject, decision_id],
    )?;
    Ok(())
}

pub fn undo_decision(conn: &Connection, decision_id: &str) -> rusqlite::Result<bool> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT subject, prev_head FROM decisions WHERE decision_id=?1 AND undone=0",
            params![decision_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((subject, prev_head)) = row else { return Ok(false) };
    conn.execute(
        "UPDATE decisions SET undone=1 WHERE decision_id=?1",
        params![decision_id],
    )?;
    match prev_head {
        Some(h) => {
            conn.execute("INSERT OR REPLACE INTO heads(subject, head) VALUES (?1,?2)", params![subject, h])?;
        }
        None => {
            conn.execute("DELETE FROM heads WHERE subject=?1", params![subject])?;
        }
    }
    append_event(
        conn,
        "decision.undone",
        &json!({"decision_id": decision_id, "subject": subject}),
    )?;
    Ok(true)
}

/// 全量导出：保持身份、顺序与来源关系。
pub fn export_all(conn: &Connection) -> rusqlite::Result<Value> {
    let dump = |sql: &str| -> rusqlite::Result<Vec<Value>> {
        let mut stmt = conn.prepare(sql)?;
        let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let rows = stmt.query_map([], |row| {
            let mut m = serde_json::Map::new();
            for (i, c) in cols.iter().enumerate() {
                let v: rusqlite::types::Value = row.get(i)?;
                m.insert(
                    c.clone(),
                    match v {
                        rusqlite::types::Value::Null => Value::Null,
                        rusqlite::types::Value::Integer(n) => json!(n),
                        rusqlite::types::Value::Real(f) => json!(f),
                        rusqlite::types::Value::Text(s) => Value::Text(s),
                        rusqlite::types::Value::Blob(b) => json!(base64ish(&b)),
                    },
                );
            }
            Ok(Value::Object(m))
        })?;
        rows.collect()
    };
    Ok(json!({
        "format": "flow_provenance/export/v1",
        "events": dump("SELECT * FROM events ORDER BY seq")?,
        "entities": dump("SELECT * FROM entities ORDER BY id, version, branch")?,
        "populations": dump("SELECT * FROM populations ORDER BY pop_id, branch")?,
        "candidates": dump("SELECT * FROM candidates ORDER BY cand_id, branch")?,
        "decisions": dump("SELECT * FROM decisions ORDER BY decision_id")?,
        "diagnostics": dump("SELECT * FROM diagnostics ORDER BY diag_id")?,
        "heads": dump("SELECT * FROM heads ORDER BY subject")?,
    }))
}

fn base64ish(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 导入到当前（应为全新）库，保持 seq/身份/顺序。
pub fn import_all(conn: &Connection, data: &Value) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    let insert_rows = |tx: &Connection, table: &str, rows: &Vec<Value>| -> rusqlite::Result<()> {
        for r in rows {
            let obj = r.as_object().ok_or_else(|| {
                rusqlite::Error::ToSqlConversionFailure("bad row".into())
            })?;
            let cols: Vec<&String> = obj.keys().collect();
            let sql = format!(
                "INSERT INTO {table} ({}) VALUES ({})",
                cols.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(","),
                cols.iter().map(|_| "?").collect::<Vec<_>>().join(",")
            );
            let vals: Vec<rusqlite::types::Value> = cols
                .iter()
                .map(|c| match &obj[*c] {
                    Value::Null => rusqlite::types::Value::Null,
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            rusqlite::types::Value::Integer(i)
                        } else {
                            rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0))
                        }
                    }
                    other => rusqlite::types::Value::Text(match other {
                        Value::String(s) => s.clone(),
                        o => o.to_string(),
                    }),
                })
                .collect();
            tx.execute(&sql, rusqlite::params_from_iter(vals))?;
        }
        Ok(())
    };
    for table in ["events", "entities", "populations", "candidates", "decisions", "diagnostics", "heads"] {
        if let Some(rows) = data.get(table).and_then(|v| v.as_array()) {
            insert_rows(&tx, table, rows)?;
        }
    }
    tx.commit()?;
    Ok(())
}
