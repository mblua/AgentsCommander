//! Durable DB-backed send queue for the control-plane API.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::params;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const DB_FILENAME: &str = "api-message-bus.sqlite3";
pub const INLINE_BODY_MAX_BYTES: usize = 256 * 1024;
pub const DEFAULT_CONTENT_TYPE: &str = "text/markdown";
pub const STATUS_QUEUED: &str = "queued";
pub const STATUS_DELIVERING: &str = "delivering";
pub const STATUS_RETRY: &str = "retry";
pub const STATUS_DELIVERED: &str = "delivered";
pub const STATUS_POISONED: &str = "poisoned";

#[derive(Debug, Error)]
pub enum MessageStoreError {
    #[error("config_dir is unavailable")]
    MissingConfigDir,
    #[error("message body exceeds 256 KiB")]
    BodyTooLarge,
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("blocking task failed: {0}")]
    BlockingTask(String),
}

#[derive(Clone)]
pub struct MessageStore {
    path: PathBuf,
    conn: Arc<Mutex<rusqlite::Connection>>,
}

#[derive(Debug, Clone)]
pub struct EnqueueRequest {
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub content_type: String,
    pub body: String,
    pub source_plane: String,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueResult {
    pub message_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub status: String,
    pub duplicate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeasedMessage {
    pub message_id: String,
    pub sender_fqn: String,
    pub target_fqn: String,
    pub op_id: String,
    pub content_type: String,
    pub body: String,
    pub attempt: i64,
}

impl MessageStore {
    pub fn at_config_dir() -> Result<Self, MessageStoreError> {
        let dir = crate::config::config_dir().ok_or(MessageStoreError::MissingConfigDir)?;
        Self::open(dir.join(DB_FILENAME))
    }

    pub fn open(path: PathBuf) -> Result<Self, MessageStoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = rusqlite::Connection::open(&path)?;
        conn.busy_timeout(Duration::from_millis(5000))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let _: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let store = Self {
            path,
            conn: Arc::new(Mutex::new(conn)),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn migrate(&self) -> Result<(), MessageStoreError> {
        let mut conn = self.conn.lock().unwrap();
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
        if current.unwrap_or(0) < 1 {
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
        }
        tx.commit()?;
        Ok(())
    }

    pub fn enqueue(&self, req: EnqueueRequest) -> Result<EnqueueResult, MessageStoreError> {
        if req.body.len() > INLINE_BODY_MAX_BYTES {
            return Err(MessageStoreError::BodyTooLarge);
        }

        let now = Utc::now().to_rfc3339();
        let message_id = uuid::Uuid::new_v4().to_string();
        let body_sha256 = sha256_hex(req.body.as_bytes());
        let body_bytes = req.body.len() as i64;

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let inserted = tx.execute(
            r#"
            INSERT OR IGNORE INTO messages(
                message_id, sender_fqn, target_fqn, op_id, content_type, body,
                body_sha256, body_bytes, source_plane, source_ref, status,
                attempt, next_attempt_at, created_at
            )
            VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12, ?13)
            "#,
            params![
                message_id,
                req.sender_fqn,
                req.target_fqn,
                req.op_id,
                req.content_type,
                req.body,
                body_sha256,
                body_bytes,
                req.source_plane,
                req.source_ref,
                STATUS_QUEUED,
                now,
                now,
            ],
        )?;

        let result = tx.query_row(
            r#"
            SELECT message_id, sender_fqn, target_fqn, op_id, status
            FROM messages
            WHERE sender_fqn = ?1 AND op_id = ?2
            "#,
            params![req.sender_fqn, req.op_id],
            |row| {
                Ok(EnqueueResult {
                    message_id: row.get(0)?,
                    sender_fqn: row.get(1)?,
                    target_fqn: row.get(2)?,
                    op_id: row.get(3)?,
                    status: row.get(4)?,
                    duplicate: inserted == 0,
                })
            },
        )?;
        if inserted > 0 {
            insert_audit(&tx, &result.message_id, STATUS_QUEUED, None, &now)?;
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn lease_due(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        lease_for: Duration,
        lease_owner: &str,
    ) -> Result<Vec<LeasedMessage>, MessageStoreError> {
        let now_s = now.to_rfc3339();
        let lease_until =
            (now + chrono::Duration::from_std(lease_for).unwrap_or_default()).to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let ids = {
            let mut stmt = tx.prepare(
                r#"
                SELECT message_id
                FROM messages
                WHERE (
                    status IN (?1, ?2) AND next_attempt_at <= ?3
                ) OR (
                    status = ?4 AND lease_until IS NOT NULL AND lease_until <= ?3
                )
                ORDER BY created_at ASC
                LIMIT ?5
                "#,
            )?;
            let rows = stmt.query_map(
                params![
                    STATUS_QUEUED,
                    STATUS_RETRY,
                    &now_s,
                    STATUS_DELIVERING,
                    limit as i64
                ],
                |row| row.get::<_, String>(0),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let mut leased = Vec::with_capacity(ids.len());
        for id in ids {
            let changed = tx.execute(
                r#"
                UPDATE messages
                SET status = ?1, lease_owner = ?2, lease_until = ?3, last_error = NULL
                WHERE message_id = ?4
                  AND (
                    (status IN (?5, ?6) AND next_attempt_at <= ?7)
                    OR (status = ?8 AND lease_until IS NOT NULL AND lease_until <= ?7)
                  )
                "#,
                params![
                    STATUS_DELIVERING,
                    lease_owner,
                    &lease_until,
                    &id,
                    STATUS_QUEUED,
                    STATUS_RETRY,
                    &now_s,
                    STATUS_DELIVERING,
                ],
            )?;
            if changed == 0 {
                continue;
            }
            let msg = tx.query_row(
                r#"
                SELECT message_id, sender_fqn, target_fqn, op_id, content_type, body, attempt
                FROM messages
                WHERE message_id = ?1
                "#,
                [id],
                |row| {
                    Ok(LeasedMessage {
                        message_id: row.get(0)?,
                        sender_fqn: row.get(1)?,
                        target_fqn: row.get(2)?,
                        op_id: row.get(3)?,
                        content_type: row.get(4)?,
                        body: row.get(5)?,
                        attempt: row.get(6)?,
                    })
                },
            )?;
            leased.push(msg);
        }
        tx.commit()?;
        Ok(leased)
    }

    pub fn mark_delivered(
        &self,
        message_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let now_s = now.to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            UPDATE messages
            SET status = ?1, delivered_at = ?2, lease_owner = NULL, lease_until = NULL,
                last_error = NULL
            WHERE message_id = ?3
            "#,
            params![STATUS_DELIVERED, now_s, message_id],
        )?;
        insert_audit(&tx, message_id, STATUS_DELIVERED, None, &now_s)?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_delivery_failed(
        &self,
        message_id: &str,
        error: &str,
        now: DateTime<Utc>,
        max_attempts: i64,
    ) -> Result<String, MessageStoreError> {
        let now_s = now.to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let current_attempt: i64 = tx.query_row(
            "SELECT attempt FROM messages WHERE message_id = ?1",
            [message_id],
            |row| row.get(0),
        )?;
        let next_attempt = current_attempt + 1;
        let next_status = if next_attempt >= max_attempts {
            STATUS_POISONED
        } else {
            STATUS_RETRY
        };
        let backoff = retry_backoff_seconds(next_attempt);
        let next_attempt_at = (now + chrono::Duration::seconds(backoff)).to_rfc3339();
        tx.execute(
            r#"
            UPDATE messages
            SET status = ?1, attempt = ?2, next_attempt_at = ?3,
                lease_owner = NULL, lease_until = NULL, last_error = ?4
            WHERE message_id = ?5
            "#,
            params![
                next_status,
                next_attempt,
                next_attempt_at,
                error,
                message_id
            ],
        )?;
        insert_audit(&tx, message_id, next_status, Some(error), &now_s)?;
        tx.commit()?;
        Ok(next_status.to_string())
    }

    pub fn reap_terminal_before(&self, cutoff: DateTime<Utc>) -> Result<usize, MessageStoreError> {
        let cutoff_s = cutoff.to_rfc3339();
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute(
            r#"
            DELETE FROM messages
            WHERE (
                status = ?1 AND delivered_at IS NOT NULL AND delivered_at < ?2
            ) OR (
                status = ?3 AND next_attempt_at < ?2
            )
            "#,
            params![STATUS_DELIVERED, cutoff_s, STATUS_POISONED],
        )?;
        Ok(deleted)
    }

    pub async fn enqueue_offloaded(
        &self,
        req: EnqueueRequest,
    ) -> Result<EnqueueResult, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.enqueue(req))
            .await
            .map_err(|e| MessageStoreError::BlockingTask(e.to_string()))?
    }

    pub async fn lease_due_offloaded(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        lease_for: Duration,
        lease_owner: String,
    ) -> Result<Vec<LeasedMessage>, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.lease_due(now, limit, lease_for, &lease_owner))
            .await
            .map_err(|e| MessageStoreError::BlockingTask(e.to_string()))?
    }

    pub async fn mark_delivered_offloaded(
        &self,
        message_id: String,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.mark_delivered(&message_id, now))
            .await
            .map_err(|e| MessageStoreError::BlockingTask(e.to_string()))?
    }

    pub async fn mark_delivery_failed_offloaded(
        &self,
        message_id: String,
        error: String,
        now: DateTime<Utc>,
        max_attempts: i64,
    ) -> Result<String, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.mark_delivery_failed(&message_id, &error, now, max_attempts)
        })
        .await
        .map_err(|e| MessageStoreError::BlockingTask(e.to_string()))?
    }

    pub async fn reap_terminal_before_offloaded(
        &self,
        cutoff: DateTime<Utc>,
    ) -> Result<usize, MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.reap_terminal_before(cutoff))
            .await
            .map_err(|e| MessageStoreError::BlockingTask(e.to_string()))?
    }

    #[cfg(test)]
    fn count_by_status(&self, status: &str) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE status = ?1",
                [status],
                |row| row.get(0),
            )
            .unwrap()
    }
}

pub fn retry_backoff_seconds(attempt: i64) -> i64 {
    let exponent = attempt.saturating_sub(1).min(5) as u32;
    5_i64.saturating_mul(2_i64.saturating_pow(exponent))
}

fn insert_audit(
    tx: &rusqlite::Transaction<'_>,
    message_id: &str,
    status: &str,
    detail: Option<&str>,
    at: &str,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        r#"
        INSERT INTO message_audit(event_id, message_id, status, detail, at)
        VALUES(?1, ?2, ?3, ?4, ?5)
        "#,
        params![
            uuid::Uuid::new_v4().to_string(),
            message_id,
            status,
            detail,
            at
        ],
    )?;
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> MessageStore {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(DB_FILENAME);
        let store = MessageStore::open(path).unwrap();
        std::mem::forget(dir);
        store
    }

    fn request(op: &str, body: &str) -> EnqueueRequest {
        EnqueueRequest {
            sender_fqn: "proj:wg-1/a".into(),
            target_fqn: "proj:wg-1/b".into(),
            op_id: op.into(),
            content_type: DEFAULT_CONTENT_TYPE.into(),
            body: body.into(),
            source_plane: "api_inline".into(),
            source_ref: None,
        }
    }

    #[test]
    fn migration_creates_schema_and_enqueue_writes_row() {
        let store = store();
        let result = store.enqueue(request("op1", "hello")).unwrap();

        assert!(!result.message_id.is_empty());
        assert_eq!(result.status, STATUS_QUEUED);
        assert_eq!(store.count_by_status(STATUS_QUEUED), 1);
    }

    #[test]
    fn duplicate_sender_op_id_returns_existing_row() {
        let store = store();
        let first = store.enqueue(request("op1", "hello")).unwrap();
        let mut changed = request("op1", "changed");
        changed.target_fqn = "proj:wg-1/other".into();
        let second = store.enqueue(changed).unwrap();

        assert_eq!(first.message_id, second.message_id);
        assert_eq!(first.target_fqn, second.target_fqn);
        assert!(second.duplicate);
        assert_eq!(store.count_by_status(STATUS_QUEUED), 1);
    }

    #[test]
    fn lease_due_hides_active_lease_until_expiry_then_redelivers() {
        let store = store();
        let first = store.enqueue(request("op1", "hello")).unwrap();
        let now = Utc::now();

        let leased = store
            .lease_due(now, 10, Duration::from_secs(30), "worker")
            .unwrap();
        assert_eq!(leased.len(), 1);
        assert_eq!(leased[0].message_id, first.message_id);

        let none = store
            .lease_due(
                now + chrono::Duration::seconds(1),
                10,
                Duration::from_secs(30),
                "worker",
            )
            .unwrap();
        assert!(none.is_empty());

        let again = store
            .lease_due(
                now + chrono::Duration::seconds(31),
                10,
                Duration::from_secs(30),
                "worker",
            )
            .unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].message_id, first.message_id);
    }

    #[test]
    fn delivered_retry_poison_and_reaper_transitions() {
        let store = store();
        let delivered = store.enqueue(request("op1", "ok")).unwrap();
        let retry = store.enqueue(request("op2", "retry")).unwrap();
        let now = Utc::now();

        store.mark_delivered(&delivered.message_id, now).unwrap();
        assert_eq!(store.count_by_status(STATUS_DELIVERED), 1);

        let status = store
            .mark_delivery_failed(&retry.message_id, "boom", now, 2)
            .unwrap();
        assert_eq!(status, STATUS_RETRY);
        let status = store
            .mark_delivery_failed(&retry.message_id, "boom again", now, 2)
            .unwrap();
        assert_eq!(status, STATUS_POISONED);

        let deleted = store
            .reap_terminal_before(now + chrono::Duration::days(2))
            .unwrap();
        assert_eq!(deleted, 2);
    }

    #[test]
    fn oversize_body_rejected() {
        let store = store();
        let too_large = "x".repeat(INLINE_BODY_MAX_BYTES + 1);
        let err = store.enqueue(request("op1", &too_large)).unwrap_err();
        assert!(matches!(err, MessageStoreError::BodyTooLarge));
    }

    #[test]
    fn backoff_is_bounded_exponential() {
        assert_eq!(retry_backoff_seconds(1), 5);
        assert_eq!(retry_backoff_seconds(2), 10);
        assert_eq!(retry_backoff_seconds(6), 160);
        assert_eq!(retry_backoff_seconds(100), 160);
    }
}
