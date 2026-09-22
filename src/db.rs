use rusqlite::{params, Connection};
use std::sync::Mutex;

pub const SCHEMA_VERSION: i64 = 2;
pub const NEAR_SINGULAR_RCOND: f64 = 1e-8;

pub struct Db(pub Mutex<Connection>);

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", 1)?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let db = Db(Mutex::new(conn));
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", 1)?;
        let db = Db(Mutex::new(conn));
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        let conn = &mut *self.0.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _schema_migrations (
               version INTEGER PRIMARY KEY,
               name TEXT NOT NULL,
               applied_at TEXT NOT NULL
             );",
        )?;
        let applied: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM _schema_migrations",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let files: Vec<(i64, &str, &str)> = vec![
            (1, "0001_init", include_str!("migrations/0001_init.sql")),
            (2, "0002_events_actor", include_str!("migrations/0002_events_actor.sql")),
        ];
        for (ver, name, sql) in files {
            if ver <= applied {
                continue;
            }
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO _schema_migrations(version,name,applied_at) VALUES(?1,?2,?3)",
                params![ver, name, now_iso()],
            )?;
            tx.commit()?;
        }
        let cur: i64 = conn
            .query_row("SELECT COALESCE(MAX(version),0) FROM _schema_migrations", [], |r| {
                r.get(0)
            })?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(key,value) VALUES('schema_version',?1)",
            params![cur.to_string()],
        )?;
        Ok(())
    }
}

pub fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_to_current_version() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        let v: i64 = conn
            .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| {
                r.get::<_, String>(0).unwrap().parse().unwrap()
            })
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM _schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        let has_actor: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('events') WHERE name='actor'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_actor, 1);
    }

    #[test]
    fn idempotent_migration() {
        let db = Db::open_in_memory().unwrap();
        drop(db);
    }
}
