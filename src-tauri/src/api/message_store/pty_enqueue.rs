use super::rows::{
    claimed_from_row, insert_pty_audit, is_lower_hex_64, load_pty_result, lookup_idempotency_row,
    reason_code_string, source_plane_str,
};
use super::sha256_hex;
use super::types::{
    ClaimedPtyInputOperation, DuePtyInputCandidate, HostPtyInputRejectionRequest,
    PtyInputEnqueueRequest, PtyInputEnqueueResult,
};
use super::{
    response_work_key, MessageStore, MessageStoreError, PTY_INPUT_MAX_NONTERMINAL_BYTES,
    PTY_INPUT_MAX_NONTERMINAL_GLOBAL, PTY_INPUT_MAX_NONTERMINAL_PER_SENDER, PTY_STATUS_PREPARING,
    PTY_STATUS_QUEUED,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

impl MessageStore {
    pub fn enqueue_pty_input(
        &self,
        req: PtyInputEnqueueRequest,
    ) -> Result<PtyInputEnqueueResult, MessageStoreError> {
        let (issued_at, source_plane) = validate_pty_enqueue_request(&req)?;
        let payload_sha256 = sha256_hex(&req.payload);
        let payload_bytes =
            i64::try_from(req.payload.len()).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let response_key = response_work_key("pty", &req.sender_fqn, &req.op_id);
        let mut response_admission = self.reserve_response_work(&req.target_fqn, &response_key);
        let wall_now = Utc::now();
        for _ in 0..8 {
            let (_, scanned) = self.expire_pty_input_for_admission_page(wall_now)?;
            if scanned < 64 {
                break;
            }
        }
        // Host envelopes allow a small future skew. Queue at the issued instant
        // in that case so the row remains temporally coherent and is not due
        // before its authenticated issue time.
        let queued_at = wall_now.max(issued_at);
        let now = crate::phone::types::canonical_pty_timestamp(queued_at);

        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some(existing) = lookup_idempotency_row(&tx, &req.sender_fqn, &req.op_id)? {
            if existing.request_fingerprint != req.request_fingerprint
                || existing.sender_incarnation_fingerprint != req.sender_incarnation_fingerprint
            {
                return Err(MessageStoreError::IdempotencyConflict);
            }
            let result = load_pty_result(&tx, &existing.injection_id)?
                .ok_or(MessageStoreError::StoreCorrupt)?;
            tx.commit()?;
            if !result.terminal {
                self.accept_response_work(&mut response_admission, &req.request_fingerprint);
            }
            return Ok(PtyInputEnqueueResult {
                result,
                duplicate: true,
            });
        }

        let (sender_count, global_count, aggregate_bytes): (i64, i64, i64) = tx.query_row(
            r#"
            SELECT
                COALESCE(SUM(CASE WHEN sender_fqn = ?1 THEN 1 ELSE 0 END), 0),
                COUNT(*),
                COALESCE(SUM(payload_bytes), 0)
            FROM pty_input_operations
            WHERE status IN ('queued','preparing','retry','actuating')
            "#,
            [&req.sender_fqn],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if sender_count >= PTY_INPUT_MAX_NONTERMINAL_PER_SENDER
            || global_count >= PTY_INPUT_MAX_NONTERMINAL_GLOBAL
            || aggregate_bytes.saturating_add(payload_bytes) > PTY_INPUT_MAX_NONTERMINAL_BYTES
        {
            return Err(MessageStoreError::CapacityExceeded);
        }

        let collision: bool = tx.query_row(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM pty_input_operations
                WHERE injection_id = ?1 OR (sender_fqn = ?2 AND nonce_sha256 = ?3)
                UNION ALL
                SELECT 1 FROM pty_input_tombstones
                WHERE injection_id = ?1 OR (sender_fqn = ?2 AND nonce_sha256 = ?3)
            )
            "#,
            params![req.injection_id, req.sender_fqn, req.nonce_sha256],
            |row| row.get(0),
        )?;
        if collision {
            return Err(MessageStoreError::IdempotencyConflict);
        }

        tx.execute(
            r#"
            INSERT INTO pty_input_operations(
                injection_id, sender_fqn, target_fqn, op_id, nonce_sha256,
                request_fingerprint, confirmation_tag, version, enter_mode,
                requested_agent_id, payload, payload_sha256, payload_bytes,
                source_plane, sender_incarnation_fingerprint,
                sender_identity_fingerprint, target_identity_fingerprint,
                authority_session_id, authority_client_id,
                authority_client_generation, status, attempt, next_attempt_at,
                issued_at, expires_at, queued_at, updated_at
            ) VALUES(
                ?1,?2,?3,?4,?5,?6,?7,1,'agent-submit',?8,?9,?10,?11,
                ?12,?13,?14,?15,?16,?17,?18,'queued',0,?19,?20,?21,?19,?19
            )
            "#,
            params![
                req.injection_id,
                req.sender_fqn,
                req.target_fqn,
                req.op_id,
                req.nonce_sha256,
                req.request_fingerprint,
                req.confirmation_tag,
                req.requested_agent_id,
                req.payload,
                payload_sha256,
                payload_bytes,
                source_plane,
                req.sender_incarnation_fingerprint,
                req.sender_identity_fingerprint,
                req.target_identity_fingerprint,
                req.authority_session_id,
                req.authority_client_id,
                req.authority_client_generation,
                now,
                req.issued_at,
                req.expires_at,
            ],
        )?;
        insert_pty_audit(&tx, &req.injection_id, PTY_STATUS_QUEUED, None, &now)?;
        let result =
            load_pty_result(&tx, &req.injection_id)?.ok_or(MessageStoreError::StoreCorrupt)?;
        tx.commit()?;
        self.accept_response_work(&mut response_admission, &req.request_fingerprint);
        Ok(PtyInputEnqueueResult {
            result,
            duplicate: false,
        })
    }

    pub fn record_host_pty_input_rejection(
        &self,
        req: HostPtyInputRejectionRequest,
        now: DateTime<Utc>,
    ) -> Result<crate::phone::types::PtyInputResult, MessageStoreError> {
        use crate::phone::types::{
            parse_canonical_pty_timestamp, parse_canonical_uuid_v4,
            pty_input_reason_allowed_for_status, PtyInputPublicStatus,
        };
        parse_canonical_uuid_v4(&req.injection_id).map_err(|_| MessageStoreError::StoreCorrupt)?;
        parse_canonical_uuid_v4(&req.op_id).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let issued = parse_canonical_pty_timestamp(&req.issued_at)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let expires = parse_canonical_pty_timestamp(&req.expires_at)
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        if req.injection_id != req.op_id
            || expires - issued != chrono::Duration::minutes(10)
            || req.sender_fqn.is_empty()
            || req.sender_fqn.chars().any(char::is_control)
            || req.target_fqn.is_empty()
            || req.target_fqn.chars().any(char::is_control)
            || !is_lower_hex_64(&req.nonce_sha256)
            || !is_lower_hex_64(&req.request_fingerprint)
            || !is_lower_hex_64(&req.confirmation_tag)
            || !is_lower_hex_64(&req.sender_incarnation_fingerprint)
            || !is_lower_hex_64(&req.payload_sha256)
            || !(1..=crate::pty::backend::PTY_INPUT_MAX_BYTES as u64).contains(&req.payload_bytes)
            || !pty_input_reason_allowed_for_status(
                PtyInputPublicStatus::Rejected,
                Some(req.reason),
            )
        {
            return Err(MessageStoreError::StoreCorrupt);
        }
        let Some(_operation_lock) = self.try_operation_lock(&req.injection_id)? else {
            return Err(MessageStoreError::InvalidTransition);
        };
        let queued_at = req.issued_at.clone();
        let terminal_at = crate::phone::types::canonical_pty_timestamp(now.max(issued));
        let payload_bytes =
            i64::try_from(req.payload_bytes).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some(existing) = lookup_idempotency_row(&tx, &req.sender_fqn, &req.op_id)? {
            if existing.injection_id == req.injection_id
                && existing.request_fingerprint == req.request_fingerprint
                && existing.sender_incarnation_fingerprint == req.sender_incarnation_fingerprint
            {
                let result = load_pty_result(&tx, &existing.injection_id)?
                    .ok_or(MessageStoreError::StoreCorrupt)?;
                if result.status == PtyInputPublicStatus::Rejected
                    && result.reason.as_ref().map(|reason| reason.code) == Some(req.reason)
                {
                    tx.commit()?;
                    return Ok(result);
                }
            }
            return Err(MessageStoreError::IdempotencyConflict);
        }
        let collision: bool = tx.query_row(
            r#"SELECT EXISTS(
                 SELECT 1 FROM pty_input_operations
                 WHERE injection_id=?1 OR (sender_fqn=?2 AND nonce_sha256=?3)
                 UNION ALL
                 SELECT 1 FROM pty_input_tombstones
                 WHERE injection_id=?1 OR (sender_fqn=?2 AND nonce_sha256=?3)
               )"#,
            params![req.injection_id, req.sender_fqn, req.nonce_sha256],
            |row| row.get(0),
        )?;
        if collision {
            return Err(MessageStoreError::IdempotencyConflict);
        }
        tx.execute(
            r#"INSERT INTO pty_input_tombstones(
                 injection_id,sender_fqn,target_fqn,op_id,nonce_sha256,
                 request_fingerprint,confirmation_tag,sender_incarnation_fingerprint,
                 version,payload_sha256,payload_bytes,source_plane,
                 selected_session_id,selected_backend,issued_at,expires_at,queued_at,
                 actuating_at,terminal_at,status,reason_code,reason_detail
               ) VALUES(
                 ?1,?2,?3,?4,?5,?6,?7,?8,1,?9,?10,'host_cli',
                 NULL,NULL,?11,?12,?13,NULL,?14,'rejected',?15,?16
               )"#,
            params![
                req.injection_id,
                req.sender_fqn,
                req.target_fqn,
                req.op_id,
                req.nonce_sha256,
                req.request_fingerprint,
                req.confirmation_tag,
                req.sender_incarnation_fingerprint,
                req.payload_sha256,
                payload_bytes,
                req.issued_at,
                req.expires_at,
                queued_at,
                terminal_at,
                reason_code_string(req.reason),
                crate::phone::types::safe_detail(req.reason),
            ],
        )?;
        let result =
            load_pty_result(&tx, &req.injection_id)?.ok_or(MessageStoreError::StoreCorrupt)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn due_pty_input_ids(
        &self,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<String>, MessageStoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let now = crate::phone::types::canonical_pty_timestamp(now);
        let mut statement = conn.prepare(
            r#"SELECT injection_id FROM pty_input_operations
               WHERE source_plane=?1
                 AND ((status IN ('queued','retry') AND next_attempt_at <= ?2)
                   OR (status='preparing' AND lease_until <= ?2))
                 AND attempt < 5
               ORDER BY next_attempt_at,queued_at,injection_id LIMIT ?3"#,
        )?;
        let limit = i64::try_from(limit.min(64)).map_err(|_| MessageStoreError::StoreCorrupt)?;
        let rows = statement
            .query_map(params![source_plane_str(source_plane), now, limit], |row| {
                row.get::<_, String>(0)
            })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub(crate) fn due_container_pty_input_candidates_fair(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<DuePtyInputCandidate>, MessageStoreError> {
        let query_time = now;
        let now = crate::phone::types::canonical_pty_timestamp(now);
        let cursor = self
            .maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .due_container
            .clone();
        let rows = {
            let conn = self
                .conn
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mut statement = conn.prepare(
                r#"SELECT injection_id,target_fqn,next_attempt_at,queued_at
                   FROM pty_input_operations
                   WHERE source_plane='container_api'
                     AND ((status IN ('queued','retry') AND next_attempt_at <= ?1)
                       OR (status='preparing' AND lease_until <= ?1))
                     AND attempt < 5
                     AND (?2 IS NULL OR next_attempt_at > ?2
                       OR (next_attempt_at = ?2 AND queued_at > ?3)
                       OR (next_attempt_at = ?2 AND queued_at = ?3 AND injection_id > ?4))
                   ORDER BY next_attempt_at,queued_at,injection_id LIMIT ?5"#,
            )?;
            let (cursor_next, cursor_queued, cursor_id) = cursor
                .as_ref()
                .map(|(next, queued, id)| {
                    (
                        Some(next.as_str()),
                        Some(queued.as_str()),
                        Some(id.as_str()),
                    )
                })
                .unwrap_or((None, None, None));
            let limit =
                i64::try_from(limit.min(64)).map_err(|_| MessageStoreError::StoreCorrupt)?;
            let mapped = statement.query_map(
                params![now, cursor_next, cursor_queued, cursor_id, limit],
                |row| {
                    Ok((
                        DuePtyInputCandidate {
                            injection_id: row.get(0)?,
                            target_fqn: row.get(1)?,
                        },
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        if rows.is_empty() && cursor.is_some() {
            self.maintenance_cursors
                .lock()
                .map_err(|_| MessageStoreError::StoreCorrupt)?
                .due_container = None;
            return self.due_container_pty_input_candidates_fair(query_time, limit);
        }
        self.maintenance_cursors
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?
            .due_container = rows.last().map(|(candidate, next, queued)| {
            (next.clone(), queued.clone(), candidate.injection_id.clone())
        });
        Ok(rows
            .into_iter()
            .map(|(candidate, _, _)| candidate)
            .collect())
    }

    pub fn claim_pty_input(
        &self,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        exact_injection_id: Option<&str>,
        lease_owner: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimedPtyInputOperation>, MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let lease_until =
            crate::phone::types::canonical_pty_timestamp(now + chrono::Duration::seconds(120));
        let source = source_plane_str(source_plane);
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate = if let Some(injection_id) = exact_injection_id {
            tx.query_row(
                r#"SELECT injection_id FROM pty_input_operations
                   WHERE injection_id=?1 AND source_plane=?2 AND attempt < 5
                     AND expires_at > ?3
                     AND ((status IN ('queued','retry') AND next_attempt_at <= ?3)
                       OR (status='preparing' AND lease_until <= ?3))"#,
                params![injection_id, source, now_s],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        } else {
            tx.query_row(
                r#"SELECT injection_id FROM pty_input_operations
                   WHERE source_plane=?1 AND attempt < 5
                     AND expires_at > ?2
                     AND ((status IN ('queued','retry') AND next_attempt_at <= ?2)
                       OR (status='preparing' AND lease_until <= ?2))
                   ORDER BY next_attempt_at, queued_at, injection_id LIMIT 1"#,
                params![source, now_s],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        };
        let Some(injection_id) = candidate else {
            tx.commit()?;
            return Ok(None);
        };
        let changed = tx.execute(
            r#"UPDATE pty_input_operations
               SET status='preparing', attempt=attempt+1, lease_owner=?1,
                   lease_until=MIN(?2,expires_at), preparing_at=?3, updated_at=?3
               WHERE injection_id=?4 AND attempt < 5 AND expires_at > ?3
                 AND ((status IN ('queued','retry') AND next_attempt_at <= ?3)
                   OR (status='preparing' AND lease_until <= ?3))"#,
            params![lease_owner, lease_until, now_s, injection_id],
        )?;
        if changed != 1 {
            tx.commit()?;
            return Ok(None);
        }
        insert_pty_audit(&tx, &injection_id, PTY_STATUS_PREPARING, None, &now_s)?;
        let claimed = tx.query_row(
            r#"SELECT injection_id,sender_fqn,target_fqn,op_id,source_plane,
                      lease_owner,attempt,authority_session_id,authority_client_id,
                      authority_client_generation,sender_identity_fingerprint,
                      target_identity_fingerprint,requested_agent_id,expires_at
               FROM pty_input_operations WHERE injection_id=?1"#,
            [&injection_id],
            claimed_from_row,
        )?;
        tx.commit()?;
        Ok(Some(claimed))
    }

    pub fn renew_pty_input_lease(
        &self,
        injection_id: &str,
        lease_owner: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, MessageStoreError> {
        let now_s = crate::phone::types::canonical_pty_timestamp(now);
        let lease_until =
            crate::phone::types::canonical_pty_timestamp(now + chrono::Duration::seconds(120));
        let conn = self
            .conn
            .lock()
            .map_err(|_| MessageStoreError::StoreCorrupt)?;
        let changed = conn.execute(
            r#"UPDATE pty_input_operations
               SET lease_until=MIN(?1,expires_at),updated_at=?2
               WHERE injection_id=?3 AND status='preparing'
                 AND lease_owner=?4 AND lease_until > ?2 AND expires_at > ?2"#,
            params![lease_until, now_s, injection_id, lease_owner],
        )?;
        Ok(changed == 1)
    }
}

fn validate_pty_enqueue_request(
    req: &PtyInputEnqueueRequest,
) -> Result<(DateTime<Utc>, &'static str), MessageStoreError> {
    use crate::phone::types::{
        parse_canonical_pty_timestamp, parse_canonical_uuid_v4, PtyInputSourcePlane,
    };
    parse_canonical_uuid_v4(&req.injection_id).map_err(|_| MessageStoreError::StoreCorrupt)?;
    parse_canonical_uuid_v4(&req.op_id).map_err(|_| MessageStoreError::StoreCorrupt)?;
    let issued_at = parse_canonical_pty_timestamp(&req.issued_at)
        .map_err(|_| MessageStoreError::StoreCorrupt)?;
    let expires_at = parse_canonical_pty_timestamp(&req.expires_at)
        .map_err(|_| MessageStoreError::StoreCorrupt)?;
    if expires_at - issued_at != chrono::Duration::minutes(10) {
        return Err(MessageStoreError::StoreCorrupt);
    }
    let text = std::str::from_utf8(&req.payload).map_err(|_| MessageStoreError::StoreCorrupt)?;
    crate::pty::inject::validate_pty_input_text(text)
        .map_err(|_| MessageStoreError::StoreCorrupt)?;
    let source_plane = match req.source_plane {
        PtyInputSourcePlane::HostCli => "host_cli",
        PtyInputSourcePlane::ContainerApi => "container_api",
    };
    if !is_lower_hex_64(&req.nonce_sha256)
        || !is_lower_hex_64(&req.request_fingerprint)
        || !is_lower_hex_64(&req.sender_incarnation_fingerprint)
        || !is_lower_hex_64(&req.sender_identity_fingerprint)
        || !is_lower_hex_64(&req.target_identity_fingerprint)
        || crate::phone::types::parse_canonical_uuid_v4(&req.authority_session_id).is_err()
    {
        return Err(MessageStoreError::StoreCorrupt);
    }
    match req.source_plane {
        PtyInputSourcePlane::HostCli
            if req.confirmation_tag.as_deref().is_some_and(is_lower_hex_64)
                && req.authority_client_id.is_none()
                && req.authority_client_generation.is_none()
                && req.injection_id == req.op_id => {}
        PtyInputSourcePlane::ContainerApi
            if req.confirmation_tag.is_none()
                && req
                    .authority_client_id
                    .as_deref()
                    .is_some_and(|id| !id.is_empty())
                && req
                    .authority_client_generation
                    .as_deref()
                    .is_some_and(|generation| {
                        crate::phone::types::parse_canonical_uuid_v4(generation).is_ok()
                    }) => {}
        _ => return Err(MessageStoreError::StoreCorrupt),
    }
    Ok((issued_at, source_plane))
}
