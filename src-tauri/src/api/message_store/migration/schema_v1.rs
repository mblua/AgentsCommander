use super::MessageStoreError;
use chrono::Utc;
use rusqlite::Transaction;

pub(super) fn apply(tx: &Transaction<'_>) -> Result<(), MessageStoreError> {
    tx.execute_batch(
        r#"
                CREATE TABLE IF NOT EXISTS messages(
                    message_id TEXT PRIMARY KEY,
                    sender_fqn TEXT NOT NULL,
                    target_fqn TEXT NOT NULL,
                    op_id TEXT NOT NULL,
                    content_type TEXT NOT NULL,
                    body TEXT NOT NULL,
                    body_sha256 TEXT NOT NULL,
                    body_bytes INTEGER NOT NULL,
                    source_plane TEXT NOT NULL,
                    source_ref TEXT NULL,
                    status TEXT NOT NULL,
                    attempt INTEGER NOT NULL DEFAULT 0,
                    next_attempt_at TEXT NOT NULL,
                    lease_owner TEXT NULL,
                    lease_until TEXT NULL,
                    created_at TEXT NOT NULL,
                    delivered_at TEXT NULL,
                    last_error TEXT NULL,
                    UNIQUE(sender_fqn, op_id)
                );
                CREATE INDEX IF NOT EXISTS idx_messages_due
                    ON messages(status, next_attempt_at, lease_until);
                CREATE TABLE IF NOT EXISTS message_audit(
                    event_id TEXT PRIMARY KEY,
                    message_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    detail TEXT NULL,
                    at TEXT NOT NULL,
                    FOREIGN KEY(message_id) REFERENCES messages(message_id) ON DELETE CASCADE
                );
                "#,
    )?;
    tx.execute(
        "INSERT INTO api_message_schema(version, applied_at) VALUES(1, ?1)",
        [Utc::now().to_rfc3339()],
    )?;
    Ok(())
}
