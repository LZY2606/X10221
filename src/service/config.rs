use crate::store::{Repo, StoreResult};
use rusqlite::params;

impl Repo {
    pub fn set_config(&mut self, branch_id: &str, key: &str, value: &str) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO branch_config(branch_id,key,value) VALUES(?1,?2,?3)
             ON CONFLICT(branch_id,key) DO UPDATE SET value=excluded.value",
            params![branch_id, key, value],
        )?;
        Ok(())
    }

    pub fn get_config(&self, branch_id: &str, key: &str) -> StoreResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM branch_config WHERE branch_id=?1 AND key=?2",
                params![branch_id, key],
                |r| r.get::<_, String>(0),
            )
            .optional()
    }
}

use rusqlite::OptionalExtension;
