use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::Value;
use std::sync::Mutex;

#[derive(Debug)]
pub struct ConflictError {
    pub expected_head: Option<String>,
    pub actual_head: Option<String>,
}

#[derive(Debug)]
pub enum DbError {
    Sql(rusqlite::Error),
    Conflict(ConflictError),
    NotFound(String),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Sql(error) => write!(formatter, "{error}"),
            DbError::Conflict(error) => write!(
                formatter,
                "expected head {:?}, actual head {:?}",
                error.expected_head, error.actual_head
            ),
            DbError::NotFound(id) => write!(formatter, "artifact not found: {id}"),
        }
    }
}

impl std::error::Error for DbError {}

impl From<rusqlite::Error> for DbError {
    fn from(error: rusqlite::Error) -> Self {
        DbError::Sql(error)
    }
}

pub struct Database {
    connection: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &str) -> Result<Self, DbError> {
        let mut connection = Connection::open(path)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn in_memory() -> Result<Self, DbError> {
        let mut connection = Connection::open_in_memory()?;
        connection.pragma_update(None, "foreign_keys", true)?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn transaction<T, F>(&self, action: F) -> Result<T, DbError>
    where
        F: FnOnce(&Transaction<'_>) -> Result<T, DbError>,
    {
        let mut connection = self.connection.lock().expect("database mutex poisoned");
        let transaction = connection.transaction()?;
        let result = action(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn query<T, F>(&self, action: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> Result<T, DbError>,
    {
        let connection = self.connection.lock().expect("database mutex poisoned");
        action(&connection)
    }
}

pub fn insert_artifact(
    transaction: &Transaction<'_>,
    id: &str,
    kind: &str,
    version: &str,
    fingerprint: &str,
    payload: &Value,
) -> Result<(), DbError> {
    transaction.execute(
        "INSERT INTO artifacts (id, kind, version, fingerprint, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, kind, version, fingerprint, serde_json::to_string(payload)?],
    )?;
    Ok(())
}

pub fn artifact_exists(transaction: &Transaction<'_>, id: &str) -> Result<bool, DbError> {
    Ok(transaction
        .query_row("SELECT 1 FROM artifacts WHERE id = ?1", rusqlite::params![id], |_| Ok(()))
        .optional()?
        .is_some())
}

pub fn get_artifact(connection: &Connection, id: &str) -> Result<Value, DbError> {
    connection
        .query_row(
            "SELECT payload FROM artifacts WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                let payload: String = row.get(0)?;
                Ok(payload)
            },
        )
        .optional()?
        .ok_or_else(|| DbError::NotFound(id.to_string()))
        .and_then(|payload| Ok(serde_json::from_str(&payload)?))
}

pub fn all_artifacts(connection: &Connection) -> Result<Vec<Value>, DbError> {
    let mut statement = connection.prepare(
        "SELECT payload FROM artifacts ORDER BY kind, version, id",
    )?;
    let rows = statement.query_map([], |row| {
        let payload: String = row.get(0)?;
        Ok(payload)
    })?;
    let mut artifacts = Vec::new();
    for row in rows {
        artifacts.push(serde_json::from_str(&row?)?);
    }
    Ok(artifacts)
}

pub fn append_event(
    transaction: &Transaction<'_>,
    event_id: &str,
    branch: &str,
    event_type: &str,
    payload: &Value,
    refs: &Value,
    expected_head: Option<&str>,
) -> Result<i64, DbError> {
    let actual_head: Option<String> = transaction
        .query_row(
            "SELECT head_event_id FROM branch_heads WHERE branch = ?1",
            rusqlite::params![branch],
            |row| row.get(0),
        )
        .optional()?;
    match (expected_head, actual_head.as_deref()) {
        (None, Some(head)) => {
            return Err(DbError::Conflict(ConflictError {
                expected_head: None,
                actual_head: Some(head.to_string()),
            }))
        }
        (Some(expected), actual) if Some(expected) != actual => {
            return Err(DbError::Conflict(ConflictError {
                expected_head: Some(expected.to_string()),
                actual_head: actual.map(str::to_string),
            }))
        }
        _ => {}
    }
    let branch_seq: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(branch_seq), 0) + 1 FROM events WHERE branch = ?1",
        rusqlite::params![branch],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO events (event_id, branch, branch_seq, event_type, payload, refs, prev_event_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            event_id,
            branch,
            branch_seq,
            event_type,
            serde_json::to_string(payload)?,
            serde_json::to_string(refs)?,
            actual_head
        ],
    )?;
    let seq = transaction.last_insert_rowid();
    transaction.execute(
        "INSERT INTO branch_heads(branch, head_event_id, head_seq) VALUES (?1, ?2, ?3)
         ON CONFLICT(branch) DO UPDATE SET head_event_id = excluded.head_event_id, head_seq = excluded.head_seq",
        rusqlite::params![branch, event_id, branch_seq],
    )?;
    Ok(seq)
}

pub fn head_event(connection: &Connection, branch: &str) -> Result<Option<String>, DbError> {
    Ok(connection
        .query_row(
            "SELECT head_event_id FROM branch_heads WHERE branch = ?1",
            rusqlite::params![branch],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn list_events(connection: &Connection) -> Result<Vec<Value>, DbError> {
    let mut statement = connection.prepare(
        "SELECT event_id, branch, branch_seq, event_type, payload, refs, prev_event_id, created_at
         FROM events ORDER BY seq",
    )?;
    let rows = statement.query_map([], |row| {
        let payload: String = row.get(4)?;
        let refs: String = row.get(5)?;
        Ok(serde_json::json!({
            "event_id": row.get::<_, String>(0)?,
            "branch": row.get::<_, String>(1)?,
            "branch_seq": row.get::<_, i64>(2)?,
            "event_type": row.get::<_, String>(3)?,
            "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
            "refs": serde_json::from_str::<Value>(&refs).unwrap_or(Value::Null),
            "prev_event_id": row.get::<_, Option<String>>(6)?,
            "created_at": row.get::<_, String>(7)?,
        }))
    })?;
    let mut events = Vec::new();
    for row in rows {
        events.push(row?);
    }
    Ok(events)
}

pub fn list_heads(connection: &Connection) -> Result<Value, DbError> {
    let mut statement =
        connection.prepare("SELECT branch, head_event_id, head_seq FROM branch_heads ORDER BY branch")?;
    let rows = statement.query_map([], |row| {
        Ok(serde_json::json!({
            "branch": row.get::<_, String>(0)?,
            "head_event_id": row.get::<_, String>(1)?,
            "head_seq": row.get::<_, i64>(2)?,
        }))
    })?;
    let mut heads = Vec::new();
    for row in rows {
        heads.push(row?);
    }
    Ok(Value::Array(heads))
}

pub fn record_checkpoint(
    transaction: &Transaction<'_>,
    case_id: &str,
    source_fingerprint: &str,
    event_count: i64,
) -> Result<(), DbError> {
    transaction.execute(
        "INSERT INTO import_checkpoints (case_id, source_fingerprint, event_count)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(case_id) DO UPDATE SET
           source_fingerprint = excluded.source_fingerprint,
           event_count = excluded.event_count,
           imported_at = CURRENT_TIMESTAMP",
        rusqlite::params![case_id, source_fingerprint, event_count],
    )?;
    Ok(())
}

pub fn checkpoint_count(connection: &Connection) -> Result<i64, DbError> {
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM import_checkpoints",
        [],
        |row| row.get(0),
    )?)
}

pub fn reset_branch_head(transaction: &Transaction<'_>, branch: &str, event_id: Option<&str>) -> Result<(), DbError> {
    if let Some(event_id) = event_id {
        let (branch_seq, prev): (i64, Option<String>) = transaction.query_row(
            "SELECT branch_seq, prev_event_id FROM events WHERE branch = ?1 AND event_id = ?2",
            rusqlite::params![branch, event_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        transaction.execute(
            "INSERT INTO branch_heads(branch, head_event_id, head_seq) VALUES (?1, ?2, ?3)
             ON CONFLICT(branch) DO UPDATE SET head_event_id = excluded.head_event_id, head_seq = excluded.head_seq",
            rusqlite::params![branch, event_id, branch_seq],
        )?;
        let _ = prev;
    } else {
        transaction.execute("DELETE FROM branch_heads WHERE branch = ?1", rusqlite::params![branch])?;
    }
    Ok(())
}

fn migrate(connection: &mut Connection) -> Result<(), DbError> {
    let version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 0 {
        connection.execute_batch(
            r#"
            CREATE TABLE artifacts (
              id TEXT PRIMARY KEY,
              kind TEXT NOT NULL,
              version TEXT NOT NULL,
              payload TEXT NOT NULL
            );
            CREATE TABLE events (
              seq INTEGER PRIMARY KEY AUTOINCREMENT,
              event_id TEXT NOT NULL UNIQUE,
              branch TEXT NOT NULL,
              branch_seq INTEGER NOT NULL,
              event_type TEXT NOT NULL,
              payload TEXT NOT NULL,
              prev_event_id TEXT,
              created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE branch_heads (
              branch TEXT PRIMARY KEY,
              head_event_id TEXT NOT NULL,
              head_seq INTEGER NOT NULL
            );
            "#,
        )?;
        connection.pragma_update(None, "user_version", 1)?;
    }
    let version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 1 {
        connection.execute_batch(
            r#"
            ALTER TABLE artifacts ADD COLUMN fingerprint TEXT NOT NULL DEFAULT '';
            ALTER TABLE events ADD COLUMN refs TEXT NOT NULL DEFAULT '[]';
            "#,
        )?;
        connection.pragma_update(None, "user_version", 2)?;
    }
    let version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 2 {
        connection.execute_batch(
            r#"
            CREATE TABLE import_checkpoints (
              case_id TEXT PRIMARY KEY,
              source_fingerprint TEXT NOT NULL,
              event_count INTEGER NOT NULL,
              imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )?;
        connection.pragma_update(None, "user_version", 3)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_version_one_and_preserves_rows() -> Result<(), DbError> {
        let connection = &Connection::open_in_memory()?;
        connection.execute_batch(
            r#"
            PRAGMA user_version = 1;
            CREATE TABLE artifacts (id TEXT PRIMARY KEY, kind TEXT NOT NULL, version TEXT NOT NULL, payload TEXT NOT NULL);
            CREATE TABLE events (seq INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT NOT NULL UNIQUE, branch TEXT NOT NULL, branch_seq INTEGER NOT NULL, event_type TEXT NOT NULL, payload TEXT NOT NULL, prev_event_id TEXT, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
            CREATE TABLE branch_heads (branch TEXT PRIMARY KEY, head_event_id TEXT NOT NULL, head_seq INTEGER NOT NULL);
            INSERT INTO artifacts (id, kind, version, payload) VALUES ('old', 'raw', 'raw-v1', '{"ok":true}');
            "#,
        )?;
        migrate(connection)?;
        let payload = get_artifact(connection, "old")?;
        assert_eq!(payload["ok"], true);
        assert!(checkpoint_count(connection)? >= 0);
        Ok(())
    }
}
