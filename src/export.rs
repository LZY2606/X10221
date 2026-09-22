//! Export/import the full archive. After delete-database + re-import, stable
//! identities, event ordering and provenance links are byte-identical.

use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub fn export_all(conn: &Connection) -> Value {
    let objects: Vec<Value> = conn
        .prepare("SELECT id,version,branch,kind,status,payload,created_seq,note FROM objects ORDER BY created_seq, id, version")
        .unwrap()
        .query_map([], |r| {
            let payload: String = r.get(5)?;
            Ok(json!({
                "id": r.get::<_,String>(0)?, "version": r.get::<_,i64>(1)?,
                "branch": r.get::<_,String>(2)?, "kind": r.get::<_,String>(3)?,
                "status": r.get::<_,String>(4)?,
                "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                "created_seq": r.get::<_,i64>(6)?, "note": r.get::<_,String>(7)?,
            }))
        })
        .unwrap()
        .flatten()
        .collect();

    let events: Vec<Value> = conn
        .prepare("SELECT seq,id,ts,branch,kind,payload FROM events ORDER BY seq")
        .unwrap()
        .query_map([], |r| {
            let payload: String = r.get(5)?;
            Ok(json!({
                "seq": r.get::<_,i64>(0)?, "id": r.get::<_,String>(1)?,
                "ts": r.get::<_,String>(2)?, "branch": r.get::<_,String>(3)?,
                "kind": r.get::<_,String>(4)?,
                "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
            }))
        })
        .unwrap()
        .flatten()
        .collect();

    let blobs: Vec<Value> = conn
        .prepare("SELECT id,filename,checksum,bytes FROM blobs ORDER BY id")
        .unwrap()
        .query_map([], |r| {
            let bytes: Vec<u8> = r.get(3)?;
            Ok(json!({
                "id": r.get::<_,String>(0)?, "filename": r.get::<_,String>(1)?,
                "checksum": r.get::<_,String>(2)?, "bytes_hex": hex_lower(&bytes),
            }))
        })
        .unwrap()
        .flatten()
        .collect();

    let branches: Vec<Value> = conn
        .prepare("SELECT name,head_seq,fork_from,fork_seq FROM branches ORDER BY name")
        .unwrap()
        .query_map([], |r| {
            Ok(json!({
                "name": r.get::<_,String>(0)?, "head_seq": r.get::<_,i64>(1)?,
                "fork_from": r.get::<_,Option<String>>(2)?, "fork_seq": r.get::<_,Option<i64>>(3)?,
            }))
        })
        .unwrap()
        .flatten()
        .collect();

    let diagnostics: Vec<Value> = conn
        .prepare("SELECT id,branch,object_id,object_version,event_seq,severity,message,payload FROM diagnostics ORDER BY rowid")
        .unwrap()
        .query_map([], |r| {
            let payload: String = r.get(7)?;
            Ok(json!({
                "id": r.get::<_,String>(0)?, "branch": r.get::<_,String>(1)?,
                "object_id": r.get::<_,String>(2)?, "object_version": r.get::<_,i64>(3)?,
                "event_seq": r.get::<_,Option<i64>>(4)?, "severity": r.get::<_,String>(5)?,
                "message": r.get::<_,String>(6)?,
                "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
            }))
        })
        .unwrap()
        .flatten()
        .collect();

    json!({
        "format": "flowline-archive/v1",
        "objects": objects, "events": events, "blobs": blobs,
        "branches": branches, "diagnostics": diagnostics,
    })
}

pub fn reset(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "DELETE FROM objects; DELETE FROM events; DELETE FROM blobs;
         DELETE FROM branches; DELETE FROM diagnostics;
         DELETE FROM sqlite_sequence WHERE name='events';",
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

pub fn import_all(conn: &Connection, archive: &Value) -> Result<(), String> {
    if archive["format"].as_str() != Some("flowline-archive/v1") {
        return Err("unsupported archive format".into());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    reset(&tx)?;
    for b in archive["blobs"].as_array().ok_or("bad blobs")? {
        let bytes = unhex(b["bytes_hex"].as_str().ok_or("bad bytes_hex")?)?;
        tx.execute(
            "INSERT INTO blobs(id,filename,checksum,bytes) VALUES (?1,?2,?3,?4)",
            params![
                b["id"].as_str().ok_or("bad blob id")?,
                b["filename"].as_str().ok_or("bad filename")?,
                b["checksum"].as_str().ok_or("bad checksum")?,
                bytes
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    for e in archive["events"].as_array().ok_or("bad events")? {
        tx.execute(
            "INSERT INTO events(seq,id,ts,branch,kind,payload) VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                e["seq"].as_i64().ok_or("bad seq")?,
                e["id"].as_str().ok_or("bad event id")?,
                e["ts"].as_str().ok_or("bad ts")?,
                e["branch"].as_str().ok_or("bad branch")?,
                e["kind"].as_str().ok_or("bad kind")?,
                e["payload"].to_string()
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    for o in archive["objects"].as_array().ok_or("bad objects")? {
        tx.execute(
            "INSERT INTO objects(id,version,branch,kind,status,payload,created_seq,note)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                o["id"].as_str().ok_or("bad id")?,
                o["version"].as_i64().ok_or("bad version")?,
                o["branch"].as_str().ok_or("bad branch")?,
                o["kind"].as_str().ok_or("bad kind")?,
                o["status"].as_str().unwrap_or("current"),
                o["payload"].to_string(),
                o["created_seq"].as_i64().ok_or("bad created_seq")?,
                o["note"].as_str().unwrap_or("")
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    for br in archive["branches"].as_array().ok_or("bad branches")? {
        tx.execute(
            "INSERT INTO branches(name,head_seq,fork_from,fork_seq) VALUES (?1,?2,?3,?4)",
            params![
                br["name"].as_str().ok_or("bad branch name")?,
                br["head_seq"].as_i64().unwrap_or(0),
                br["fork_from"].as_str(),
                br["fork_seq"].as_i64()
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    for d in archive["diagnostics"].as_array().ok_or("bad diagnostics")? {
        tx.execute(
            "INSERT INTO diagnostics(id,branch,object_id,object_version,event_seq,severity,message,payload)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                d["id"].as_str().ok_or("bad diag id")?,
                d["branch"].as_str().ok_or("bad branch")?,
                d["object_id"].as_str().ok_or("bad object id")?,
                d["object_version"].as_i64().ok_or("bad version")?,
                d["event_seq"].as_i64(),
                d["severity"].as_str().ok_or("bad severity")?,
                d["message"].as_str().ok_or("bad message")?,
                d["payload"].to_string()
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

pub fn hex_lower(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 0xf) as usize] as char);
    }
    s
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return Err("odd hex length".into());
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    while i < b.len() {
        let hi = (b[i] as char).to_digit(16).ok_or("bad hex")?;
        let lo = (b[i + 1] as char).to_digit(16).ok_or("bad hex")?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Ok(out)
}
