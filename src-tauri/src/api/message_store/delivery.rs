use super::rows::{insert_audit, retry_backoff_seconds, sha256_hex};
use super::types::{EnqueueRequest, EnqueueResult, LeasedMessage};
use super::{
    response_work_key, MessageStore, MessageStoreError, ResponseTerminalPersistence,
    INLINE_BODY_MAX_BYTES, STATUS_DELIVERED, STATUS_DELIVERING, STATUS_POISONED, STATUS_QUEUED,
    STATUS_RETRY,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use std::time::Duration;

impl MessageStore {
    pub fn enqueue(&self, req: EnqueueRequest) -> Result<EnqueueResult, MessageStoreError> {
        if req.body.len() > INLINE_BODY_MAX_BYTES {
            return Err(MessageStoreError::BodyTooLarge);
        }

        let response_key = response_work_key("db", &req.sender_fqn, &req.op_id);
        let mut response_admission = self.reserve_response_work(&req.target_fqn, &response_key);
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
        if result.target_fqn != req.target_fqn {
            response_admission = self.reserve_response_work(&result.target_fqn, &response_key);
        }
        let accepted_digest: String = tx.query_row(
            "SELECT body_sha256 FROM messages WHERE message_id=?1",
            [&result.message_id],
            |row| row.get(0),
        )?;
        if inserted > 0 {
            insert_audit(&tx, &result.message_id, STATUS_QUEUED, None, &now)?;
        }
        tx.commit()?;
        if matches!(
            result.status.as_str(),
            STATUS_QUEUED | STATUS_RETRY | STATUS_DELIVERING
        ) {
            self.accept_response_work(&mut response_admission, &accepted_digest);
        }
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
                SELECT message_id, target_fqn
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
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let mut leased = Vec::with_capacity(ids.len());
        for (id, target) in ids {
            if self.response_target_is_closing(&target) {
                continue;
            }
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
        let mut response_persistence = ResponseTerminalPersistence {
            state: self.response_close.get(),
            completed: false,
        };
        let now_s = now.to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let response_row: Option<(String, String, String)> = tx
            .query_row(
                "SELECT target_fqn,sender_fqn,op_id FROM messages WHERE message_id=?1",
                [message_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
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
        if let Err(error) = tx.commit() {
            if let Some(state) = self.response_close.get() {
                state.fail_closed();
            }
            return Err(error.into());
        }

        if let Some(row) = response_row {
            self.settle_response_row(&row.0, &response_work_key("db", &row.1, &row.2));
        }

        response_persistence.completed = true;
        Ok(())
    }

    pub fn mark_delivery_failed(
        &self,
        message_id: &str,
        error: &str,
        now: DateTime<Utc>,
        max_attempts: i64,
    ) -> Result<String, MessageStoreError> {
        let mut response_persistence = ResponseTerminalPersistence {
            state: self.response_close.get(),
            completed: false,
        };
        let now_s = now.to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let response_row: Option<(String, String, String)> = tx
            .query_row(
                "SELECT target_fqn,sender_fqn,op_id FROM messages WHERE message_id=?1",
                [message_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
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
        if let Err(error) = tx.commit() {
            if let Some(state) = self.response_close.get() {
                state.fail_closed();
            }
            return Err(error.into());
        }
        if next_status == STATUS_POISONED {
            if let Some(row) = response_row {
                self.settle_response_row(&row.0, &response_work_key("db", &row.1, &row.2));
            }
        }
        response_persistence.completed = true;
        Ok(next_status.to_string())
    }

    pub fn release_delivery_lease(
        &self,
        message_id: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let now_s = now.to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            UPDATE messages
            SET status = ?1, next_attempt_at = ?2,
                lease_owner = NULL, lease_until = NULL, last_error = ?3
            WHERE message_id = ?4
            "#,
            params![STATUS_QUEUED, now_s, reason, message_id],
        )?;
        insert_audit(
            &tx,
            message_id,
            "lease-released-deferred",
            Some(reason),
            &now_s,
        )?;
        tx.commit()?;
        Ok(())
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

    pub async fn release_delivery_lease_offloaded(
        &self,
        message_id: String,
        reason: String,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.release_delivery_lease(&message_id, &reason, now))
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
}
