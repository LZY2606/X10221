//! SQLite storage: append-only event log with branches, plus package archive.
//!
//! All writes are transactional; a failed import rolls the whole batch back.

use crate::domain::{Branch, Event, EventPayload};
use rusqlite::{params, Connection, OptionalExtension};

pub const MIGRATIONS: &[&str] = &[
    // v1: initial schema
    "CREATE TABLE IF NOT EXISTS schema_meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS branches (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        parent_branch TEXT,
        forked_from_event TEXT,
        head_event TEXT
    );
    CREATE TABLE IF NOT EXISTS events (
        id TEXT PRIMARY KEY,
        seq INTEGER NOT NULL,
        branch_id TEXT NOT NULL,
        parent_id TEXT,
        base_event TEXT,
        actor TEXT NOT NULL,
        created_ms INTEGER NOT NULL,
        payload_json TEXT NOT NULL,
        UNIQUE(branch_id, seq)
    );
    CREATE INDEX IF NOT EXISTS idx_events_branch ON events(branch_id, seq);
    CREATE TABLE IF NOT EXISTS package_archive (
        id TEXT PRIMARY KEY,
        payload_json TEXT NOT NULL
    );",
    // v2: provenance / stable display identity index survives export/import
    "CREATE TABLE IF NOT EXISTS display_identities (
        identity TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        logical_ref TEXT NOT NULL,
        born_event TEXT NOT NULL
    );
    INSERT INTO schema_meta(key, value) VALUES
      ('archive_identity_version', '2')
    ON CONFLICT(key) DO NOTHING;",
];

pub fn connect(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

pub fn current_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_meta'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .and_then(|exists| {
        if exists == 0 {
            Ok(0)
        } else {
            conn.query_row(
                "SELECT CAST(value AS INTEGER) FROM schema_meta WHERE key='user_version'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .or(Ok(0))
        }
    })
}

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()>> {
    let tx = conn.transaction()?;
    let v: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_meta'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if v == 0 {
        tx.execute_batch(MIGRATIONS[0])?;
        tx.execute_batch(MIGRATIONS[1])?;
        tx.execute(
            "INSERT INTO schema_meta(key,value) VALUES('user_version','2')",
            [],
        )?;
    } else {
        let cur: i64 = tx
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM schema_meta WHERE key='user_version'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        for (idx, sql) in MIGRATIONS.iter().enumerate() {
            let step = idx as i64 + 1;
            if step > cur {
                tx.execute_batch(sql)?;
            }
        }
        tx.execute(
            "INSERT INTO schema_meta(key,value) VALUES('user_version','2')
             ON CONFLICT(key) DO UPDATE SET value='2'",
            [],
        )?;
    }
    tx.commit()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredBranch {
    #[serde(flatten)]
    pub branch: Branch,
    pub head_event: Option<String>,
}

pub fn list_branches(conn: &Connection) -> rusqlite::Result<Vec<StoredBranch>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, parent_branch, forked_from_event, head_event FROM branches ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(StoredBranch {
            branch: Branch {
                id: r.get(0)?,
                name: r.get(1)?,
                parent_branch: r.get(2)?,
                forked_from_event: r.get(3)?,
            },
            head_event: r.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn get_branch(conn: &Connection, id: &str) -> rusqlite::Result<Option<StoredBranch>> {
    conn.query_row(
        "SELECT id, name, parent_branch, forked_from_event, head_event FROM branches WHERE id=?",
        params![id],
        |r| {
            Ok(StoredBranch {
                branch: Branch {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parent_branch: r.get(2)?,
                    forked_from_event: r.get(3)?,
                },
                head_event: r.get(4)?,
            })
        },
    )
    .optional()
}

pub fn next_seq(conn: &Connection, branch_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq)+1, 0) FROM events WHERE branch_id=?",
        params![branch_id],
        |r| r.get(0),
    )
}

/// Append an event with optimistic concurrency control. When `base_event` is
/// provided it must equal the branch head, otherwise a conflict error is
/// returned for the caller to render both sides.
pub struct Conflict {
    pub current_head: Option<String>,
    pub attempted_base: Option<String>,
}

pub fn append_event(
    conn: &mut Connection,
    event: &Event,
) -> Result<(), Conflict> {
    let tx = conn.transaction().map_err(|_| Conflict {
        current_head: None,
        attempted_base: event.base_event.clone(),
    })?;
    let current_head: Option<String> = tx
        .query_row(
            "SELECT head_event FROM branches WHERE id=?",
            params![event.branch_id],
            |r| r.get(0),
        )
        .map_err(|_| Conflict {
            current_head: None,
            attempted_base: event.base_event.clone(),
        })?;
    if let Some(base) = &event.base_event {
        if current_head.as_deref() != Some(base.as_str()) {
            return Err(Conflict {
                current_head,
                attempted_base: event.base_event.clone(),
            });
        }
    }
    let seq = next_seq(&tx, &event.branch_id).map_err(|_| Conflict {
        current_head: current_head.clone(),
        attempted_base: event.base_event.clone(),
    })?;
    let payload = serde_json::to_string(&event.payload).unwrap_or_default();
    tx.execute(
        "INSERT INTO events(id, seq, branch_id, parent_id, base_event, actor, created_ms, payload_json)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            event.id,
            seq,
            event.branch_id,
            event.parent_id,
            event.base_event,
            event.actor,
            event.created_ms,
            payload
        ],
    ).map_err(|_| Conflict {
        current_head: current_head.clone(),
        attempted_base: event.base_event.clone(),
    })?;
    tx.execute(
        "UPDATE branches SET head_event=?1 WHERE id=?2",
        params![event.id, event.branch_id],
    ).map_err(|_| Conflict {
        current_head: current_head.clone(),
        attempted_base: event.base_event.clone(),
    })?;
    tx.commit().map_err(|_| Conflict {
        current_head,
        attempted_base: event.base_event.clone(),
    })
}

pub fn branch_events(conn: &Connection, branch_id: &str) -> rusqlite::Result<Vec<Event>> {
    let mut stmt = conn.prepare(
        "SELECT id, seq, branch_id, parent_id, base_event, actor, created_ms, payload_json
         FROM events WHERE branch_id=? ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![branch_id], |r| {
        let payload_json: String = r.get(7)?;
        let payload: EventPayload = serde_json::from_str(&payload_json).unwrap_or_else(|_| {
            EventPayload::PackageImported {
                package: serde_json::from_str("{}").unwrap_or_else(|_| {
                    panic!("corrupt payload for event {}", r.get::<_, String>(0).unwrap_or_default())
                }),
            }
        });
        Ok(Event {
            id: r.get(0)?,
            seq: r.get(1)?,
            branch_id: r.get(2)?,
            parent_id: r.get(3)?,
            base_event: r.get(4)?,
            actor: r.get(5)?,
            created_ms: r.get(6)?,
            payload,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn create_branch(
    conn: &mut Connection,
    branch: &Branch,
    head_event: Option<String>,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO branches(id,name,parent_branch,forked_from_event,head_event)
         VALUES(?1,?2,?3,?4,?5)",
        params![
            branch.id,
            branch.name,
            branch.parent_branch,
            branch.forked_from_event,
            head_event
        ],
    )?;
    // copy ancestor events so the fork is independently replayable
    if let Some(src) = branch.parent_branch.as_deref() {
        tx.execute(
            "INSERT INTO events(id, seq, branch_id, parent_id, base_event, actor, created_ms, payload_json)
             SELECT id, seq, ?2, parent_id, base_event, actor, created_ms, payload_json
             FROM events WHERE branch_id=?1",
            params![src, branch.id],
        )?;
    }
    tx.commit()
}

pub fn save_archive(conn: &mut Connection, id: &str, payload_json: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO package_archive(id,payload_json) VALUES(?1,?2)
         ON CONFLICT(id) DO UPDATE SET payload_json=excluded.payload_json",
        params![id, payload_json],
    )?;
    Ok(())
}

pub fn load_archive(conn: &Connection, id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT payload_json FROM package_archive WHERE id=?",
        params![id],
        |r| r.get::<_, String>(0),
    )
    .optional()
}

pub fn archive_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM package_archive", [], |r| r.get(0))
}
