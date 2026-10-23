use super::rows::{
    actuating_row_from_row, insert_pty_audit, is_lower_hex_64, load_pty_result, reason_code_string,
    retry_backoff_seconds, sha256_hex, terminalize_in_transaction, tombstone_matches,
};
use super::{
    MessageStore, MessageStoreError, PTY_STATUS_ACTUATING, PTY_STATUS_INDETERMINATE,
    PTY_STATUS_INJECTED, PTY_STATUS_PREPARING, PTY_STATUS_REJECTED, PTY_STATUS_RETRY,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

impl MessageStore {
    pub fn begin_pty_actuating(
        &self,
        injection_id: &str,
        lease_owner: &str,
        selected_session_id: &str,
        selected_backend: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<u8>, MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row = tx
            .query_row(
                r#"SELECT payload,payload_sha256,payload_bytes,version,enter_mode,source_plane,
                          nonce_sha256,request_fingerprint,sender_incarnation_fingerprint,
                          sender_identity_fingerprint,target_identity_fingerprint,
                          authority_session_id,authority_client_id,authority_client_generation,
                          issued_at,expires_at
                   FROM pty_input_operations
                   WHERE injection_id=?1 AND status='preparing' AND lease_owner=?2
                     AND lease_until > ?3 AND expires_at > ?3"#,
                params![injection_id, lease_owner, now_s],
                actuating_row_from_row,
            )
            .optional()?
            .ok_or(MessageStoreError::InvalidTransition)?;
        let actual_payload_bytes =
            i64::try_from(row.payload.len()).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let issued = crate::phone::types::parse_canonical_pty_timestamp(&row.issued_at)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let expires = crate::phone::types::parse_canonical_pty_timestamp(&row.expires_at)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let authority_valid = match row.source.as_str() {
            "host_cli" => {
                row.authority_client_id.is_none() && row.authority_client_generation.is_none()
            }
            "container_api" => {
                row.authority_client_id
                    .as_deref()
                    .is_some_and(|id| !id.is_empty())
                    && row
                        .authority_client_generation
                        .as_deref()
                        .is_some_and(|generation| {
                            crate::phone::types::parse_canonical_uuid_v4(generation).is_ok()
                        })
            }
            _ => false,
        };
        if row.version != 1
            || row.enter_mode != "agent-submit"
            || !authority_valid
            || actual_payload_bytes != row.payload_bytes
            || row.digest != sha256_hex(&row.payload)
            || !is_lower_hex_64(&row.digest)
            || !is_lower_hex_64(&row.nonce_sha256)
            || !is_lower_hex_64(&row.request_fingerprint)
            || !is_lower_hex_64(&row.sender_incarnation_fingerprint)
            || !row
                .sender_identity_fingerprint
                .as_deref()
                .is_some_and(is_lower_hex_64)
            || !row
                .target_identity_fingerprint
                .as_deref()
                .is_some_and(is_lower_hex_64)
            || !row
                .authority_session_id
                .as_deref()
                .is_some_and(|session_id| {
                    crate::phone::types::parse_canonical_uuid_v4(session_id).is_ok()
                })
            || expires - issued != chrono::Duration::minutes(10)
            || crate::phone::types::parse_canonical_uuid_v4(selected_session_id).is_err()
            || !matches!(selected_backend, "localProcess" | "containerTransport")
        {
            return Err(MessageStoreError::StoreCorrupt);
        }
        let payload = row.payload;
        let text = std::str::from_utf8(&payload).map_err(|_| MessageStoreError::StoreCorrupt)?;
        crate::pty::inject::validate_pty_input_text(text)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let changed = tx.execute(
            r#"UPDATE pty_input_operations
               SET status='actuating',actuating_at=?1,updated_at=?1,
                   selected_session_id=?2,selected_backend=?3,payload=NULL,
                   requested_agent_id=NULL,sender_identity_fingerprint=NULL,
                   target_identity_fingerprint=NULL,authority_session_id=NULL,
                   authority_client_id=NULL,authority_client_generation=NULL,
                   lease_owner=NULL,lease_until=NULL,reason_code=NULL,reason_detail=NULL
               WHERE injection_id=?4 AND status='preparing' AND lease_owner=?5"#,
            params![
                now_s,
                selected_session_id,
                selected_backend,
                injection_id,
                lease_owner
            ],
        )?;
        if changed != 1 {
            return Err(MessageStoreError::InvalidTransition);
        }
        insert_pty_audit(&tx, injection_id, PTY_STATUS_ACTUATING, None, &now_s)?;
        #[cfg(test)]
        {
            let inject_ambiguous = {
                let mut faults = self
                    .test_faults
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                std::mem::take(&mut faults.actuation_commit_ambiguous)
            };
            if inject_ambiguous {
                tx.commit()?;
                return Err(MessageStoreError::ActuationCommitAmbiguous);
            }
        }
        match tx.commit() {
            Ok(()) => Ok(payload),
            Err(error) => {
                let status = conn
                    .query_row(
                        "SELECT status FROM pty_input_operations WHERE injection_id=?1",
                        [injection_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional();
                match status {
                    Ok(Some(status)) if status == PTY_STATUS_ACTUATING => {
                        Err(MessageStoreError::ActuationCommitAmbiguous)
                    }
                    Ok(Some(status)) if status == PTY_STATUS_PREPARING => {
                        Err(MessageStoreError::Sqlite(error))
                    }
                    Ok(Some(_)) => Err(MessageStoreError::StoreCorrupt),
                    Ok(None) => Err(MessageStoreError::OperationNotFound),
                    Err(_) => Err(MessageStoreError::StoreCorrupt),
                }
            }
        }
    }

    pub fn retry_pty_input(
        &self,
        injection_id: &str,
        lease_owner: &str,
        code: crate::phone::types::PtyInputReasonCode,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let attempt: i64 = tx.query_row(
            "SELECT attempt FROM pty_input_operations WHERE injection_id=?1 AND status='preparing' \
                AND lease_owner=?2",
            params![injection_id, lease_owner],
            |row| row.get(0),
        )?;
        if attempt >= 5 {
            return terminalize_in_transaction(
                self.response_close.get(),
                tx,
                injection_id,
                PTY_STATUS_REJECTED,
                Some(code),
                &now_s,
            );
        }
        let delay = retry_backoff_seconds(attempt).min(40);
        let next =
            crate::phone::types::canonical_pty_timestamp(now + chrono::Duration::seconds(delay));
        let changed = tx.execute(
            r#"UPDATE pty_input_operations SET status='retry',next_attempt_at=?1,
                   lease_owner=NULL,lease_until=NULL,updated_at=?2,
                   reason_code=?3,reason_detail=?4
               WHERE injection_id=?5 AND status='preparing' AND lease_owner=?6"#,
            params![
                next,
                now_s,
                reason_code_string(code),
                crate::phone::types::safe_detail(code),
                injection_id,
                lease_owner
            ],
        )?;
        if changed != 1 {
            return Err(MessageStoreError::InvalidTransition);
        }
        insert_pty_audit(&tx, injection_id, PTY_STATUS_RETRY, Some(code), &now_s)?;
        tx.commit()?;
        Ok(())
    }

    pub fn terminalize_pty_input(
        &self,
        injection_id: &str,
        status: crate::phone::types::PtyInputPublicStatus,
        reason: Option<crate::phone::types::PtyInputReasonCode>,
        now: DateTime<Utc>,
    ) -> Result<crate::phone::types::PtyInputResult, MessageStoreError> {
        let status = match status {
            crate::phone::types::PtyInputPublicStatus::Injected => PTY_STATUS_INJECTED,
            crate::phone::types::PtyInputPublicStatus::Rejected => PTY_STATUS_REJECTED,
            crate::phone::types::PtyInputPublicStatus::Indeterminate => PTY_STATUS_INDETERMINATE,
            _ => return Err(MessageStoreError::InvalidTransition),
        };
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        terminalize_in_transaction(
            self.response_close.get(),
            tx,
            injection_id,
            status,
            reason,
            &now_s,
        )?;
        drop(conn);
        self.query_pty_input_by_injection(injection_id)?
            .ok_or(MessageStoreError::StoreCorrupt)
    }

    pub fn query_pty_input_by_injection(
        &self,
        injection_id: &str,
    ) -> Result<Option<crate::phone::types::PtyInputResult>, MessageStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        load_pty_result(&conn, injection_id)
    }

    pub fn host_confirmation_tag(
        &self,
        injection_id: &str,
    ) -> Result<Option<String>, MessageStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        Ok(conn
            .query_row(
                r#"SELECT confirmation_tag FROM pty_input_operations WHERE injection_id=?1
                   UNION ALL
                   SELECT confirmation_tag FROM pty_input_tombstones WHERE injection_id=?1
                   LIMIT 1"#,
                [injection_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    pub fn mark_host_artifact(
        &self,
        injection_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MessageStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let changed = conn.execute(
            "UPDATE pty_input_operations SET host_artifact_at=?1 WHERE injection_id=?2 AND \
                source_plane='host_cli' AND status IN ('injected','rejected','indeterminate')",
            params![
                crate::phone::types::canonical_pty_timestamp(now),
                injection_id
            ],
        )?;
        if changed == 1 {
            return Ok(());
        }
        let compacted: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pty_input_tombstones WHERE injection_id=?1 AND \
                source_plane='host_cli')",
            [injection_id],
            |row| row.get(0),
        )?;
        if compacted {
            Ok(())
        } else {
            Err(MessageStoreError::InvalidTransition)
        }
    }

    pub fn query_pty_input(
        &self,
        sender_fqn: &str,
        op_id: &str,
        sender_incarnation_fingerprint: &str,
    ) -> Result<Option<crate::phone::types::PtyInputResult>, MessageStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let id = conn
            .query_row(
                r#"SELECT injection_id,sender_incarnation_fingerprint
                   FROM pty_input_operations WHERE sender_fqn=?1 AND op_id=?2
                   UNION ALL
                   SELECT injection_id,sender_incarnation_fingerprint
                   FROM pty_input_tombstones WHERE sender_fqn=?1 AND op_id=?2
                   LIMIT 1"#,
                params![sender_fqn, op_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((id, incarnation)) = id else {
            return Ok(None);
        };
        if incarnation != sender_incarnation_fingerprint {
            return Ok(None);
        }
        load_pty_result(&conn, &id)
    }

    pub fn compact_pty_terminal_before(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<usize, MessageStoreError> {
        let cutoff_time = cutoff;
        let cutoff = crate::phone::types::canonical_pty_timestamp(cutoff);
        let limit = i64::try_from(limit.min(64)).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let cursor = self
            .maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .compact_before
            .clone();
        let candidates = {
            let conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mut statement = conn.prepare(
                r#"SELECT injection_id,terminal_at FROM pty_input_operations
                   WHERE status IN ('injected','rejected','indeterminate')
                     AND terminal_at < ?1
                     AND (?2 IS NULL OR terminal_at > ?2
                       OR (terminal_at = ?2 AND injection_id > ?3))
                   ORDER BY terminal_at,injection_id LIMIT ?4"#,
            )?;
            let (cursor_at, cursor_id) = cursor
                .as_ref()
                .map(|(at, id)| (Some(at.as_str()), Some(id.as_str())))
                .unwrap_or((None, None));
            let rows = statement
                .query_map(params![cutoff, cursor_at, cursor_id, limit], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if candidates.is_empty() && cursor.is_some() {
            self.maintenance_cursors
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?
                .compact_before = None;
            return self.compact_pty_terminal_before(cutoff_time, limit as usize);
        }
        self.maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .compact_before = candidates
            .last()
            .map(|(id, terminal_at)| (terminal_at.clone(), id.clone()));
        let mut deleted = 0;
        for (injection_id, _) in candidates {
            let Some(_operation_lock) = self.try_operation_lock(&injection_id)? else {
                continue;
            };
            let mut conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let eligible: bool = tx.query_row(
                r#"SELECT EXISTS(SELECT 1 FROM pty_input_operations
                   WHERE injection_id=?1 AND status IN ('injected','rejected','indeterminate')
                     AND terminal_at < ?2)"#,
                params![injection_id, cutoff],
                |row| row.get(0),
            )?;
            if !eligible {
                tx.commit()?;
                continue;
            }
            if !tombstone_matches(&tx, &injection_id)? {
                return Err(MessageStoreError::StoreCorrupt);
            }
            let changed = tx.execute(
                r#"DELETE FROM pty_input_operations
                   WHERE injection_id=?1 AND status IN ('injected','rejected','indeterminate')
                     AND terminal_at < ?2"#,
                params![injection_id, cutoff],
            )?;
            if changed != 1 {
                return Err(MessageStoreError::InvalidTransition);
            }
            tx.commit()?;
            deleted += 1;
        }
        Ok(deleted)
    }

    pub fn compact_pty_terminal_maintenance(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<usize, MessageStoreError> {
        let maintenance_time = now;
        let normal_cutoff =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::days(7));
        let unclaimed_cutoff =
            crate::phone::types::canonical_pty_timestamp(now - chrono::Duration::days(30));
        let limit = i64::try_from(limit.min(64)).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let cursor = self
            .maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .compact_maintenance
            .clone();
        let candidates = {
            let conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mut statement = conn.prepare(
                r#"SELECT injection_id,terminal_at FROM pty_input_operations
                   WHERE status IN ('injected','rejected','indeterminate')
                     AND ((source_plane!='host_cli' AND terminal_at < ?1)
                       OR (source_plane='host_cli' AND host_artifact_at IS NOT NULL AND terminal_at
                           < ?1)
                       OR (source_plane='host_cli' AND host_artifact_at IS NULL AND terminal_at <
                           ?2))
                     AND (?3 IS NULL OR terminal_at > ?3
                       OR (terminal_at = ?3 AND injection_id > ?4))
                   ORDER BY terminal_at,injection_id LIMIT ?5"#,
            )?;
            let (cursor_at, cursor_id) = cursor
                .as_ref()
                .map(|(at, id)| (Some(at.as_str()), Some(id.as_str())))
                .unwrap_or((None, None));
            let rows = statement.query_map(
                params![normal_cutoff, unclaimed_cutoff, cursor_at, cursor_id, limit],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if candidates.is_empty() && cursor.is_some() {
            self.maintenance_cursors
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?
                .compact_maintenance = None;
            return self.compact_pty_terminal_maintenance(maintenance_time, limit as usize);
        }
        self.maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .compact_maintenance = candidates
            .last()
            .map(|(id, terminal_at)| (terminal_at.clone(), id.clone()));
        let mut compacted = 0;
        for (injection_id, _) in candidates {
            let Some(_operation_lock) = self.try_operation_lock(&injection_id)? else {
                continue;
            };
            let mut conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if !tombstone_matches(&tx, &injection_id)? {
                return Err(MessageStoreError::StoreCorrupt);
            }
            let deleted = tx.execute(
                r#"DELETE FROM pty_input_operations
                   WHERE injection_id=?1 AND status IN ('injected','rejected','indeterminate')
                     AND ((source_plane!='host_cli' AND terminal_at < ?2)
                       OR (source_plane='host_cli' AND host_artifact_at IS NOT NULL AND terminal_at
                           < ?2)
                       OR (source_plane='host_cli' AND host_artifact_at IS NULL AND terminal_at <
                           ?3))"#,
                params![injection_id, normal_cutoff, unclaimed_cutoff],
            )?;
            if deleted != 1 {
                return Err(MessageStoreError::InvalidTransition);
            }
            tx.commit()?;
            compacted += 1;
        }
        Ok(compacted)
    }
}
