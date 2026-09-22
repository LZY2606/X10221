//! 存储层操作：所有写入都在调用方提供的事务中完成，失败即整体回滚。
use crate::model::Material;
use crate::store::RowEvent;
use rusqlite::{params, Transaction};
use serde_json::Value;

pub fn branch_exists(t: &Transaction, name: &str) -> bool {
    t.query_row("SELECT 1 FROM branches WHERE name=?1", [name], |_| Ok(()))
        .is_ok()
}

pub fn create_branch(
    t: &Transaction,
    name: &str,
    title: &str,
    parent: Option<&str>,
    fork_seq: Option<i64>,
    created_seq: i64,
) -> rusqlite::Result<()> {
    t.execute(
        "INSERT INTO branches(name,title,parent,fork_seq,created_seq) VALUES(?1,?2,?3,?4,?5)",
        params![name, title, parent, fork_seq, created_seq],
    )?;
    Ok(())
}

pub fn branch_tip(t: &Transaction, name: &str) -> rusqlite::Result<i64> {
    t.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM events WHERE branch=?1",
        [name],
        |r| r.get(0),
    )
}

pub fn insert_material(t: &Transaction, m: &Material) -> rusqlite::Result<()> {
    let raw = serde_json::to_string(&m.raw).unwrap_or_else(|_| "null".into());
    t.execute(
        "INSERT INTO materials(kind,id,version,title,raw_json,sha256,source_file)
         VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(kind,id,version) DO UPDATE SET
           title=excluded.title, raw_json=excluded.raw_json, sha256=excluded.sha256,
           source_file=excluded.source_file",
        params![
            m.kind,
            m.id,
            m.version as i64,
            m.title,
            raw,
            crate::store::sha256_hex(raw.as_bytes()),
            m.source_file
        ],
    )?;
    Ok(())
}

pub fn material_raw(
    t: &Transaction,
    kind: &str,
    id: &str,
    version: u32,
) -> rusqlite::Result<Option<Value>> {
    let s: Option<String> = t
        .query_row(
            "SELECT raw_json FROM materials WHERE kind=?1 AND id=?2 AND version=?3",
            params![kind, id, version as i64],
            |r| r.get(0),
        )
        .optional()?;
    Ok(s.map(|s| serde_json::from_str(&s).unwrap_or(Value::Null)))
}

pub fn material_sha(t: &Transaction, kind: &str, id: &str, version: u32) -> Option<String> {
    t.query_row(
        "SELECT sha256 FROM materials WHERE kind=?1 AND id=?2 AND version=?3",
        params![kind, id, version as i64],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

#[allow(clippy::too_many_arguments)]
pub fn insert_event(
    t: &Transaction,
    seq: i64,
    branch: &str,
    kind: &str,
    subject: &str,
    payload: &Value,
    basis: &Value,
    basis_rules: &str,
    mat: Option<(&str, &str, u32)>,
    origin: Option<(i64, &str)>,
) -> rusqlite::Result<()> {
    let p = serde_json::to_string(payload).unwrap_or_else(|_| "null".into());
    let b = serde_json::to_string(basis).unwrap_or_else(|_| "null".into());
    t.execute(
        "INSERT INTO events(seq,branch,kind,subject,payload,basis,basis_rules,
           mat_kind,mat_id,mat_version,origin_seq,origin_branch)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            seq,
            branch,
            kind,
            subject,
            p,
            b,
            basis_rules,
            mat.map(|x| x.0),
            mat.map(|x| x.1),
            mat.map(|x| x.2 as i64),
            origin.map(|x| x.0),
            origin.map(|x| x.1),
        ],
    )?;
    Ok(())
}

pub fn list_events_until(
    t: &Transaction,
    branch: &str,
    seq: Option<i64>,
) -> rusqlite::Result<Vec<RowEvent>> {
    let mut sql = String::from(
        "SELECT seq,kind,subject,payload,basis,basis_rules,
                mat_kind,mat_id,mat_version,origin_seq,origin_branch
         FROM events WHERE branch=?1",
    );
    if seq.is_some() {
        sql.push_str(" AND seq<=?2");
    }
    sql.push_str(" ORDER BY seq ASC");
    let mut stmt = t.prepare(&sql)?;
    let rows = if let Some(s) = seq {
        stmt.query_map(params![branch, s], row_event)?
    } else {
        stmt.query_map(params![branch], row_event)?
    };
    rows.collect()
}

fn row_event(r: &rusqlite::Row) -> rusqlite::Result<RowEvent> {
    let p: String = r.get(3)?;
    let b: String = r.get(4)?;
    Ok(RowEvent {
        seq: r.get(0)?,
        kind: r.get(1)?,
        subject: r.get(2)?,
        payload: serde_json::from_str(&p).unwrap_or(Value::Null),
        basis: serde_json::from_str(&b).unwrap_or(Value::Null),
        basis_rules: r.get(5)?,
        mat_kind: r.get(6)?,
        mat_id: r.get(7)?,
        mat_version: r.get(8)?,
        origin_seq: r.get(9)?,
        origin_branch: r.get(10)?,
    })
}

trait OptionalExt<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}
impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
