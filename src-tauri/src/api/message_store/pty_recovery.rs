use super::locks::{try_stripe_lock_at, PtyInputStripeGuard};
use super::rows::{insert_pty_audit, reason_code_string};
use super::{
    MessageStore, MessageStoreError, PTY_STATUS_ACTUATING, PTY_STATUS_PREPARING, PTY_STATUS_RETRY,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, TransactionBehavior};
use std::collections::HashSet;
use std::time::Duration;

impl MessageStore {
    pub(super) fn recover_pty_input_startup(&self) -> Result<(), MessageStoreError> {
        let now = Utc::now();
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let mut cursor: Option<(String, String)> = None;
        loop {
            let candidates = {
                let conn = self
                    .conn
                    .lock()
                    .map_err(|_| MessageStoreError::StoreCorrupt)?;
                let mut statement = conn.prepare(
                    r#"SELECT injection_id,status,expires_at,attempt,updated_at
                       FROM pty_input_operations
                       WHERE (status IN ('preparing','actuating')
                              OR (status IN ('queued','retry') AND expires_at <= ?1))
                         AND (?2 IS NULL OR updated_at > ?2
                              OR (updated_at = ?2 AND injection_id > ?3))
                       ORDER BY updated_at,injection_id LIMIT 64"#,
                )?;
                let (cursor_at, cursor_id) = cursor
                    .as_ref()
                    .map(|(at, id)| (Some(at.as_str()), Some(id.as_str())))
                    .unwrap_or((None, None));
                let rows = statement.query_map(params![now_s, cursor_at, cursor_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            let candidate_count = candidates.len();
            for (injection_id, status, expires_at, attempt, _) in &candidates {
                let Some(_operation_lock) = self.try_operation_lock(injection_id)? else {
                    continue;
                };
                if status == PTY_STATUS_ACTUATING {
                    self.terminalize_pty_input(
                        injection_id,
                        crate::phone::types::PtyInputPublicStatus::Indeterminate,
                        Some(crate::phone::types::PtyInputReasonCode::DaemonRestartAfterActuation),
                        now,
                    )?;
                } else if expires_at <= &now_s {
                    self.terminalize_pty_input(
                        injection_id,
                        crate::phone::types::PtyInputPublicStatus::Rejected,
                        Some(crate::phone::types::PtyInputReasonCode::Expired),
                        now,
                    )?;
                } else if status == PTY_STATUS_PREPARING && *attempt >= 5 {
                    self.terminalize_pty_input(
                        injection_id,
                        crate::phone::types::PtyInputPublicStatus::Rejected,
                        Some(crate::phone::types::PtyInputReasonCode::LeaseLost),
                        now,
                    )?;
                } else if status == PTY_STATUS_PREPARING {
                    let mut conn = self
                        .conn
                        .lock()
                        .map_err(|_| MessageStoreError::StoreCorrupt)?;
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let changed = tx.execute(
                        r#"UPDATE pty_input_operations SET status='retry',lease_owner=NULL,
                             lease_until=NULL,next_attempt_at=?1,updated_at=?1,
                             reason_code=?2,reason_detail=?3
                           WHERE injection_id=?4 AND status='preparing' AND attempt < 5"#,
                        params![
                            now_s,
                            reason_code_string(crate::phone::types::PtyInputReasonCode::LeaseLost),
                            crate::phone::types::safe_detail(
                                crate::phone::types::PtyInputReasonCode::LeaseLost
                            ),
                            injection_id
                        ],
                    )?;
                    if changed != 1 {
                        return Err(MessageStoreError::InvalidTransition);
                    }
                    insert_pty_audit(
                        &tx,
                        injection_id,
                        PTY_STATUS_RETRY,
                        Some(crate::phone::types::PtyInputReasonCode::LeaseLost),
                        &now_s,
                    )?;
                    tx.commit()?;
                }
            }
            cursor = candidates
                .last()
                .map(|(id, _, _, _, updated_at)| (updated_at.clone(), id.clone()));
            if candidate_count < 64 {
                break;
            }
        }
        Ok(())
    }

    pub fn recover_pty_input_runtime(
        &self,
        active: &HashSet<String>,
        now: DateTime<Utc>,
    ) -> Result<usize, MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let orphan_cutoff =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::seconds(15));
        let cursor = self
            .maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .runtime_recovery
            .clone();
        let candidates = {
            let conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mut statement = conn.prepare(
                r#"SELECT injection_id,status,expires_at,attempt,lease_until,updated_at
                   FROM pty_input_operations
                   WHERE ((status='actuating' AND actuating_at < ?1)
                      OR (status IN ('queued','preparing','retry') AND expires_at <= ?2)
                      OR (status='preparing' AND lease_until <= ?2))
                     AND (?3 IS NULL OR updated_at > ?3
                       OR (updated_at = ?3 AND injection_id > ?4))
                   ORDER BY updated_at,injection_id LIMIT 64"#,
            )?;
            let (cursor_at, cursor_id) = cursor
                .as_ref()
                .map(|(at, id)| (Some(at.as_str()), Some(id.as_str())))
                .unwrap_or((None, None));
            let rows = statement.query_map(
                params![orphan_cutoff, now_s, cursor_at, cursor_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if candidates.is_empty() && cursor.is_some() {
            self.maintenance_cursors
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?
                .runtime_recovery = None;
            return self.recover_pty_input_runtime(active, now);
        }
        self.maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .runtime_recovery = candidates
            .last()
            .map(|(id, _, _, _, _, updated_at)| (updated_at.clone(), id.clone()));
        let mut recovered = 0;
        for (injection_id, status, expires_at, attempt, lease_until, _) in candidates {
            if active.contains(&injection_id) {
                continue;
            }
            let Some(_operation_lock) = self.try_operation_lock(&injection_id)? else {
                continue;
            };
            if expires_at <= now_s && status != PTY_STATUS_ACTUATING {
                self.terminalize_pty_input(
                    &injection_id,
                    crate::phone::types::PtyInputPublicStatus::Rejected,
                    Some(crate::phone::types::PtyInputReasonCode::Expired),
                    now,
                )?;
                recovered += 1;
            } else if status == PTY_STATUS_ACTUATING {
                self.terminalize_pty_input(
                    &injection_id,
                    crate::phone::types::PtyInputPublicStatus::Indeterminate,
                    Some(crate::phone::types::PtyInputReasonCode::RuntimeActuationOrphan),
                    now,
                )?;
                recovered += 1;
            } else if status == PTY_STATUS_PREPARING
                && lease_until
                    .as_deref()
                    .is_some_and(|lease| lease <= now_s.as_str())
            {
                if attempt >= 5 {
                    self.terminalize_pty_input(
                        &injection_id,
                        crate::phone::types::PtyInputPublicStatus::Rejected,
                        Some(crate::phone::types::PtyInputReasonCode::LeaseLost),
                        now,
                    )?;
                } else {
                    let mut conn = self
                        .conn
                        .lock()
                        .map_err(|_| MessageStoreError::StoreCorrupt)?;
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let changed = tx.execute(
                        r#"UPDATE pty_input_operations
                           SET status='retry',lease_owner=NULL,lease_until=NULL,
                               next_attempt_at=?1,updated_at=?1,reason_code=?2,reason_detail=?3
                           WHERE injection_id=?4 AND status='preparing' AND lease_until <= ?1"#,
                        params![
                            now_s,
                            reason_code_string(crate::phone::types::PtyInputReasonCode::LeaseLost),
                            crate::phone::types::safe_detail(
                                crate::phone::types::PtyInputReasonCode::LeaseLost
                            ),
                            injection_id
                        ],
                    )?;
                    if changed != 1 {
                        return Err(MessageStoreError::InvalidTransition);
                    }
                    insert_pty_audit(
                        &tx,
                        &injection_id,
                        PTY_STATUS_RETRY,
                        Some(crate::phone::types::PtyInputReasonCode::LeaseLost),
                        &now_s,
                    )?;
                    tx.commit()?;
                }
                recovered += 1;
            }
        }
        Ok(recovered)
    }

    pub fn try_operation_lock(
        &self,
        injection_id: &str,
    ) -> Result<Option<PtyInputStripeGuard>, MessageStoreError> {
        self.try_stripe_lock(injection_id, true)
    }

    pub fn try_target_lock(
        &self,
        target_fqn: &str,
    ) -> Result<Option<PtyInputStripeGuard>, MessageStoreError> {
        self.try_stripe_lock(target_fqn, false)
    }

    pub async fn acquire_target_lock(
        &self,
        target_fqn: &str,
    ) -> Result<PtyInputStripeGuard, MessageStoreError> {
        loop {
            if let Some(guard) = self.try_target_lock(target_fqn)? {
                return Ok(guard);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub(super) fn try_stripe_lock(
        &self,
        value: &str,
        operation: bool,
    ) -> Result<Option<PtyInputStripeGuard>, MessageStoreError> {
        let parent = self.path.parent().ok_or(MessageStoreError::UnsafePath)?;
        try_stripe_lock_at(parent, value, operation)
    }

    pub(super) fn expire_pty_input_for_admission_page(
        &self,
        now: DateTime<Utc>,
    ) -> Result<(usize, usize), MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let cursor = self
            .maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .admission_expiry
            .clone();
        let candidates = {
            let conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mut statement = conn.prepare(
                r#"SELECT injection_id,expires_at FROM pty_input_operations
                   WHERE status IN ('queued','preparing','retry') AND expires_at <= ?1
                     AND (?2 IS NULL OR expires_at > ?2
                       OR (expires_at = ?2 AND injection_id > ?3))
                   ORDER BY expires_at,injection_id LIMIT 64"#,
            )?;
            let (cursor_at, cursor_id) = cursor
                .as_ref()
                .map(|(at, id)| (Some(at.as_str()), Some(id.as_str())))
                .unwrap_or((None, None));
            let rows = statement.query_map(params![now_s, cursor_at, cursor_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if candidates.is_empty() && cursor.is_some() {
            self.maintenance_cursors
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?
                .admission_expiry = None;
            return self.expire_pty_input_for_admission_page(now);
        }
        self.maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .admission_expiry = candidates
            .last()
            .map(|(id, expires_at)| (expires_at.clone(), id.clone()));
        let scanned = candidates.len();
        let mut expired = 0;
        for (injection_id, _) in candidates {
            let Some(_operation_lock) = self.try_operation_lock(&injection_id)? else {
                continue;
            };
            self.terminalize_pty_input(
                &injection_id,
                crate::phone::types::PtyInputPublicStatus::Rejected,
                Some(crate::phone::types::PtyInputReasonCode::Expired),
                now,
            )?;
            expired += 1;
        }
        Ok((expired, scanned))
    }

    #[cfg(test)]
    pub(super) fn expire_pty_input_for_admission(
        &self,
        now: DateTime<Utc>,
    ) -> Result<usize, MessageStoreError> {
        self.expire_pty_input_for_admission_page(now)
            .map(|(expired, _)| expired)
    }
}
