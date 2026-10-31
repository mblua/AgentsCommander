mod schema_v1;
mod schema_v2;
mod schema_v3;

use super::{MessageStore, MessageStoreError};

impl MessageStore {
    pub(super) fn migrate(&self) -> Result<(), MessageStoreError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        let tx = conn.transaction()?;
        tx.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS api_message_schema(
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );
            "#,
        )?;
        let current: Option<i64> =
            tx.query_row("SELECT MAX(version) FROM api_message_schema", [], |row| {
                row.get::<_, Option<i64>>(0)
            })?;
        let mut schema_version = current.unwrap_or(0);
        if schema_version > 3 {
            return Err(MessageStoreError::StoreCorrupt);
        }
        if schema_version < 1 {
            schema_v1::apply(&tx)?;
            schema_version = 1;
        }
        if schema_version < 2 {
            schema_v2::apply(&tx)?;
            schema_version = 2;
        }
        if schema_version < 3 {
            schema_v3::apply(&tx)?;
            let _ = schema_version;
        }
        tx.commit()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    }
}
