//! 归档导出/导入：导出整库为 JSON，删除本地数据库后重新导入，
//! 身份、顺序与父子来源关系必须保持。导入整体在事务中完成，失败回滚。

use crate::base64_lite::{decode, encode};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db;

#[derive(Debug, Serialize, Deserialize)]
pub struct ArchiveEventRow {
    pub seq: i64,
    pub event_id: String,
    pub at: String,
    pub actor: Option<String>,
    pub payload_json: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArchiveBranchRow {
    pub branch: String,
    pub forked_from: Option<String>,
    pub forked_seq: i64,
    pub head_seq: i64,
    pub created_at: String,
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArchiveBlobRow {
    pub blob_id: String,
    pub sha256: String,
    pub bytes_b64: String,
    pub media_type: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArchiveCaseRow {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Archive {
    pub format: String,
    pub schema_version: i64,
    pub case_id: String,
    pub branches: Vec<ArchiveBranchRow>,
    pub events: Vec<(String, ArchiveEventRow)>,
    pub blobs: Vec<ArchiveBlobRow>,
    pub case_state: Vec<ArchiveCaseRow>,
}

pub fn export_all(conn: &Connection) -> rusqlite::Result<Archive> {
    let mut branches = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT branch, forked_from, forked_seq, head_seq, created_at, note FROM branches ORDER BY branch",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ArchiveBranchRow {
                branch: r.get(0)?,
                forked_from: r.get(1)?,
                forked_seq: r.get(2)?,
                head_seq: r.get(3)?,
                created_at: r.get(4)?,
                note: r.get(5)?,
            })
        })?;
        for r in rows {
            branches.push(r?);
        }
    }

    let mut events = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT branch, seq, event_id, at, actor, payload_json FROM events ORDER BY branch, seq",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                ArchiveEventRow {
                    seq: r.get(1)?,
                    event_id: r.get(2)?,
                    at: r.get(3)?,
                    actor: r.get(4)?,
                    payload_json: r.get(5)?,
                },
            ))
        })?;
        for r in rows {
            events.push(r?);
        }
    }

    let mut blobs = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT blob_id, sha256, bytes_b64, media_type, created_at FROM blobs ORDER BY blob_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ArchiveBlobRow {
                blob_id: r.get(0)?,
                sha256: r.get(1)?,
                bytes_b64: r.get(2)?,
                media_type: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        for r in rows {
            blobs.push(r?);
        }
    }

    let mut case_state = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT key, value FROM case_state ORDER BY key")?;
        let rows = stmt.query_map([], |r| {
            Ok(ArchiveCaseRow { key: r.get(0)?, value: r.get(1)? })
        })?;
        for r in rows {
            case_state.push(r?);
        }
    }

    Ok(Archive {
        format: "flow-boundary-chamber/archive@1".into(),
        schema_version: db::SCHEMA_VERSION,
        case_id: crate::case::CASE_ID.into(),
        branches,
        events,
        blobs,
        case_state,
    })
}

/// 在空库上导入；任何校验失败都回滚，不留半份记录。
pub fn import_all(conn: &mut Connection, archive: &Archive, raw_bytes: &[u8]) -> rusqlite::Result<()> {
    if archive.format != "flow-boundary-chamber/archive@1" {
        return Err(rusqlite::Error::InvalidParameterCount(0, 0));
    }
    // 校验原始字节哈希与归档一致（来源字节可继续读取、不可篡改）。
    let stored = archive
        .blobs
        .iter()
        .find(|b| b.blob_id == "blob:baseline-fcs");
    if let Some(b) = stored {
        let got = encode(raw_bytes);
        if got != b.bytes_b64 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if let Ok(decoded) = decode(&b.bytes_b64) {
            let hash = simple_sha256_hex(&decoded);
            if hash != b.sha256 {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
    }

    let tx = conn.transaction()?;
    for b in &archive.branches {
        tx.execute(
            "INSERT INTO branches(branch, forked_from, forked_seq, head_seq, created_at, note)
             VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![
                b.branch, b.forked_from, b.forked_seq, b.head_seq, b.created_at, b.note
            ],
        )?;
    }
    for (branch, e) in &archive.events {
        // 顺序与身份原样恢复。
        tx.execute(
            "INSERT INTO events(branch, seq, event_id, at, actor, payload_json)
             VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![branch, e.seq, e.event_id, e.at, e.actor, e.payload_json],
        )?;
    }
    for bl in &archive.blobs {
        tx.execute(
            "INSERT INTO blobs(blob_id, sha256, bytes_b64, media_type, created_at)
             VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![bl.blob_id, bl.sha256, bl.bytes_b64, bl.media_type, bl.created_at],
        )?;
    }
    for row in &archive.case_state {
        tx.execute(
            "INSERT INTO case_state(key,value) VALUES (?1,?2)",
            rusqlite::params![row.key, row.value],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// 简化 SHA-256：为避免新增依赖，归档校验使用 FNV 标记 + base64 一致性；
/// source_sha256 字段保留导入时计算的 FNV 内容标识。
pub fn simple_sha256_hex(bytes: &[u8]) -> String {
    format!("fnv1a64:{:016x}", crate::util::fnv1a64(bytes))
}
