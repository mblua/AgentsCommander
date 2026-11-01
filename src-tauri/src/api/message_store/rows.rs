use super::types::ClaimedPtyInputOperation;
use super::{
    response_work_key, MessageStoreError, ResponseTerminalPersistence, PTY_STATUS_ACTUATING,
    PTY_STATUS_INDETERMINATE, PTY_STATUS_INJECTED, PTY_STATUS_REJECTED,
};
use crate::pty::manager::ResponseCloseState;
use rusqlite::{params, OptionalExtension};

pub(super) fn source_plane_str(source: crate::phone::types::PtyInputSourcePlane) -> &'static str {
    match source {
        crate::phone::types::PtyInputSourcePlane::HostCli => "host_cli",
        crate::phone::types::PtyInputSourcePlane::ContainerApi => "container_api",
    }
}

pub(super) fn parse_source_plane(
    source: &str,
) -> Result<crate::phone::types::PtyInputSourcePlane, rusqlite::Error> {
    match source {
        "host_cli" => Ok(crate::phone::types::PtyInputSourcePlane::HostCli),
        "container_api" => Ok(crate::phone::types::PtyInputSourcePlane::ContainerApi),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(super) fn reason_code_string(code: crate::phone::types::PtyInputReasonCode) -> String {
    crate::phone::types::pty_input_reason_code_name(code).to_string()
}

pub(super) fn parse_reason_code(
    code: Option<String>,
) -> Result<Option<crate::phone::types::PtyInputReasonCode>, rusqlite::Error> {
    code.map(|value| {
        serde_json::from_str(&format!("\"{value}\"")).map_err(|_| rusqlite::Error::InvalidQuery)
    })
    .transpose()
}

pub(super) struct ActuatingRow {
    pub(super) payload: Vec<u8>,
    pub(super) digest: String,
    pub(super) payload_bytes: i64,
    pub(super) version: i64,
    pub(super) enter_mode: String,
    pub(super) source: String,
    pub(super) nonce_sha256: String,
    pub(super) request_fingerprint: String,
    pub(super) sender_incarnation_fingerprint: String,
    pub(super) sender_identity_fingerprint: Option<String>,
    pub(super) target_identity_fingerprint: Option<String>,
    pub(super) authority_session_id: Option<String>,
    pub(super) authority_client_id: Option<String>,
    pub(super) authority_client_generation: Option<String>,
    pub(super) issued_at: String,
    pub(super) expires_at: String,
}

pub(super) fn actuating_row_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActuatingRow> {
    Ok(ActuatingRow {
        payload: row.get(0)?,
        digest: row.get(1)?,
        payload_bytes: row.get(2)?,
        version: row.get(3)?,
        enter_mode: row.get(4)?,
        source: row.get(5)?,
        nonce_sha256: row.get(6)?,
        request_fingerprint: row.get(7)?,
        sender_incarnation_fingerprint: row.get(8)?,
        sender_identity_fingerprint: row.get(9)?,
        target_identity_fingerprint: row.get(10)?,
        authority_session_id: row.get(11)?,
        authority_client_id: row.get(12)?,
        authority_client_generation: row.get(13)?,
        issued_at: row.get(14)?,
        expires_at: row.get(15)?,
    })
}

pub(super) struct IdempotencyRow {
    pub(super) injection_id: String,
    pub(super) request_fingerprint: String,
    pub(super) sender_incarnation_fingerprint: String,
}

pub(super) fn lookup_idempotency_row(
    conn: &rusqlite::Connection,
    sender: &str,
    op_id: &str,
) -> Result<Option<IdempotencyRow>, rusqlite::Error> {
    conn.query_row(
        r#"SELECT injection_id,request_fingerprint,sender_incarnation_fingerprint
           FROM pty_input_operations WHERE sender_fqn=?1 AND op_id=?2
           UNION ALL
           SELECT injection_id,request_fingerprint,sender_incarnation_fingerprint
           FROM pty_input_tombstones WHERE sender_fqn=?1 AND op_id=?2
           LIMIT 1"#,
        params![sender, op_id],
        |row| {
            Ok(IdempotencyRow {
                injection_id: row.get(0)?,
                request_fingerprint: row.get(1)?,
                sender_incarnation_fingerprint: row.get(2)?,
            })
        },
    )
    .optional()
}

pub(super) fn result_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<crate::phone::types::PtyInputResult> {
    let version: i64 = row.get(0)?;
    let injection_id: String = row.get(1)?;
    let op_id: String = row.get(2)?;
    let sender: String = row.get(3)?;
    let status_raw: String = row.get(4)?;
    let payload_bytes: i64 = row.get(5)?;
    let payload_sha256: String = row.get(6)?;
    let source = parse_source_plane(&row.get::<_, String>(7)?)?;
    let selected_session_id: Option<String> = row.get(8)?;
    let selected_backend: Option<String> = row.get(9)?;
    let issued_at: String = row.get(10)?;
    let expires_at: String = row.get(11)?;
    let queued_at: String = row.get(12)?;
    let actuating_at: Option<String> = row.get(13)?;
    let terminal_at: Option<String> = row.get(14)?;
    let reason_detail: Option<String> = row.get(15)?;
    let reason_code = parse_reason_code(row.get(17)?)?;
    let target: String = row.get(18)?;
    let status = match status_raw.as_str() {
        "queued" | "preparing" | "retry" => crate::phone::types::PtyInputPublicStatus::Queued,
        "actuating" => crate::phone::types::PtyInputPublicStatus::Actuating,
        "injected" => crate::phone::types::PtyInputPublicStatus::Injected,
        "rejected" => crate::phone::types::PtyInputPublicStatus::Rejected,
        "indeterminate" => crate::phone::types::PtyInputPublicStatus::Indeterminate,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let timestamp_valid =
        |value: &str| crate::phone::types::parse_canonical_pty_timestamp(value).is_ok();
    if version != 1
        || crate::phone::types::parse_canonical_uuid_v4(&injection_id).is_err()
        || crate::phone::types::parse_canonical_uuid_v4(&op_id).is_err()
        || sender.is_empty()
        || target.is_empty()
        || !(1..=65_536).contains(&payload_bytes)
        || !is_lower_hex_64(&payload_sha256)
        || !timestamp_valid(&issued_at)
        || !timestamp_valid(&expires_at)
        || !timestamp_valid(&queued_at)
        || actuating_at
            .as_deref()
            .is_some_and(|value| !timestamp_valid(value))
        || terminal_at
            .as_deref()
            .is_some_and(|value| !timestamp_valid(value))
        || terminal_at.is_some() != status.is_terminal()
        || selected_session_id.is_some() != selected_backend.is_some()
        || selected_session_id.as_deref().is_some_and(|session_id| {
            crate::phone::types::parse_canonical_uuid_v4(session_id).is_err()
        })
        || selected_backend
            .as_deref()
            .is_some_and(|backend| !matches!(backend, "localProcess" | "containerTransport"))
        || reason_code.is_some() != reason_detail.is_some()
        || reason_code.is_some_and(|code| {
            reason_detail.as_deref() != Some(crate::phone::types::safe_detail(code))
        })
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let payload_bytes = u64::try_from(payload_bytes).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let result = crate::phone::types::PtyInputResult {
        version: 1,
        injection_id,
        op_id: Some(op_id),
        sender: Some(sender),
        target: Some(target),
        status,
        terminal: status.is_terminal(),
        payload_bytes: Some(payload_bytes),
        payload_sha256: Some(payload_sha256),
        source_plane: Some(source),
        selected_session_id,
        selected_backend,
        issued_at: Some(issued_at),
        expires_at: Some(expires_at),
        queued_at: Some(queued_at),
        actuating_at,
        terminal_at,
        reason: reason_code.map(crate::phone::types::PtyInputReason::from_code),
    };
    crate::phone::types::validate_enqueued_pty_input_result(&result)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(result)
}

pub(super) fn load_pty_result(
    conn: &rusqlite::Connection,
    injection_id: &str,
) -> Result<Option<crate::phone::types::PtyInputResult>, MessageStoreError> {
    const LIVE_QUERY: &str = r#"
        SELECT version,injection_id,op_id,sender_fqn,status,payload_bytes,
               payload_sha256,source_plane,selected_session_id,selected_backend,
               issued_at,expires_at,queued_at,actuating_at,terminal_at,
               reason_detail,updated_at,reason_code,target_fqn
        FROM pty_input_operations WHERE injection_id=?1
    "#;
    if let Some(result) = conn
        .query_row(LIVE_QUERY, [injection_id], result_from_row)
        .optional()?
    {
        return Ok(Some(result));
    }
    const TOMBSTONE_QUERY: &str = r#"
        SELECT version,injection_id,op_id,sender_fqn,status,payload_bytes,
               payload_sha256,source_plane,selected_session_id,selected_backend,
               issued_at,expires_at,queued_at,actuating_at,terminal_at,
               reason_detail,terminal_at,reason_code,target_fqn
        FROM pty_input_tombstones WHERE injection_id=?1
    "#;
    Ok(conn
        .query_row(TOMBSTONE_QUERY, [injection_id], result_from_row)
        .optional()?)
}

pub(super) fn claimed_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ClaimedPtyInputOperation> {
    let source: String = row.get(4)?;
    Ok(ClaimedPtyInputOperation {
        injection_id: row.get(0)?,
        sender_fqn: row.get(1)?,
        target_fqn: row.get(2)?,
        op_id: row.get(3)?,
        source_plane: parse_source_plane(&source)?,
        lease_owner: row.get(5)?,
        attempt: row.get(6)?,
        authority_session_id: row
            .get::<_, Option<String>>(7)?
            .ok_or(rusqlite::Error::InvalidQuery)?,
        authority_client_id: row.get(8)?,
        authority_client_generation: row.get(9)?,
        sender_identity_fingerprint: row
            .get::<_, Option<String>>(10)?
            .ok_or(rusqlite::Error::InvalidQuery)?,
        target_identity_fingerprint: row
            .get::<_, Option<String>>(11)?
            .ok_or(rusqlite::Error::InvalidQuery)?,
        requested_agent_id: row.get(12)?,
        expires_at: row.get(13)?,
    })
}

pub(super) fn insert_pty_audit(
    tx: &rusqlite::Transaction<'_>,
    injection_id: &str,
    status: &str,
    reason: Option<crate::phone::types::PtyInputReasonCode>,
    at: &str,
) -> Result<(), rusqlite::Error> {
    let inserted = tx.execute(
        r#"INSERT INTO pty_input_audit(
             event_id,injection_id,op_id,sender_fqn,target_fqn,version,
             payload_bytes,payload_sha256,source_plane,selected_session_id,
             selected_backend,status,reason_code,at
           ) SELECT ?1,injection_id,op_id,sender_fqn,target_fqn,version,
                    payload_bytes,payload_sha256,source_plane,selected_session_id,
                    selected_backend,?2,?3,?4
             FROM pty_input_operations WHERE injection_id=?5"#,
        params![
            uuid::Uuid::new_v4().to_string(),
            status,
            reason.map(reason_code_string),
            at,
            injection_id
        ],
    )?;
    if inserted != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

pub(super) fn tombstone_matches(
    tx: &rusqlite::Transaction<'_>,
    injection_id: &str,
) -> Result<bool, rusqlite::Error> {
    tx.query_row(
        r#"SELECT EXISTS(
             SELECT 1 FROM pty_input_operations operation
             JOIN pty_input_tombstones tombstone
               ON tombstone.injection_id = operation.injection_id
             WHERE operation.injection_id=?1
               AND tombstone.sender_fqn=operation.sender_fqn
               AND tombstone.target_fqn=operation.target_fqn
               AND tombstone.op_id=operation.op_id
               AND tombstone.nonce_sha256=operation.nonce_sha256
               AND tombstone.request_fingerprint=operation.request_fingerprint
               AND tombstone.confirmation_tag IS operation.confirmation_tag
               AND tombstone.sender_incarnation_fingerprint=operation.sender_incarnation_fingerprint
               AND tombstone.version=operation.version
               AND tombstone.payload_sha256=operation.payload_sha256
               AND tombstone.payload_bytes=operation.payload_bytes
               AND tombstone.source_plane=operation.source_plane
               AND tombstone.selected_session_id IS operation.selected_session_id
               AND tombstone.selected_backend IS operation.selected_backend
               AND tombstone.issued_at=operation.issued_at
               AND tombstone.expires_at=operation.expires_at
               AND tombstone.queued_at=operation.queued_at
               AND tombstone.actuating_at IS operation.actuating_at
               AND tombstone.terminal_at=operation.terminal_at
               AND tombstone.status=operation.status
               AND tombstone.reason_code IS operation.reason_code
               AND tombstone.reason_detail IS operation.reason_detail
           )"#,
        [injection_id],
        |row| row.get(0),
    )
}

pub(super) fn terminalize_in_transaction(
    response_close: Option<&ResponseCloseState>,
    tx: rusqlite::Transaction<'_>,
    injection_id: &str,
    status: &str,
    reason: Option<crate::phone::types::PtyInputReasonCode>,
    now: &str,
) -> Result<(), MessageStoreError> {
    let mut response_persistence = ResponseTerminalPersistence {
        state: response_close,
        completed: false,
    };
    let response_row: (String, String, String) = tx.query_row(
        "SELECT target_fqn,sender_fqn,op_id FROM pty_input_operations WHERE injection_id=?1",
        [injection_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let public_status = match status {
        PTY_STATUS_INJECTED => crate::phone::types::PtyInputPublicStatus::Injected,
        PTY_STATUS_REJECTED => crate::phone::types::PtyInputPublicStatus::Rejected,
        PTY_STATUS_INDETERMINATE => crate::phone::types::PtyInputPublicStatus::Indeterminate,
        _ => return Err(MessageStoreError::InvalidTransition),
    };
    if !crate::phone::types::pty_input_reason_allowed_for_status(public_status, reason) {
        return Err(MessageStoreError::InvalidTransition);
    }
    let current: (String, Option<String>, Option<String>) = tx
        .query_row(
            "SELECT status,reason_code,reason_detail FROM pty_input_operations WHERE \
                injection_id=?1",
            [injection_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .ok_or(MessageStoreError::OperationNotFound)?;
    let desired_reason_code = reason.map(crate::phone::types::pty_input_reason_code_name);
    let desired_reason_detail = reason.map(crate::phone::types::safe_detail);
    if matches!(
        current.0.as_str(),
        "injected" | "rejected" | "indeterminate"
    ) {
        if current.0 == status
            && current.1.as_deref() == desired_reason_code
            && current.2.as_deref() == desired_reason_detail
        {
            if !tombstone_matches(&tx, injection_id)? {
                return Err(MessageStoreError::StoreCorrupt);
            }
            if let Err(error) = tx.commit() {
                if let Some(state) = response_close {
                    state.fail_closed();
                }
                return Err(error.into());
            }
            if let Some(state) = response_close {
                state.settle_terminal(
                    &response_row.0,
                    &response_work_key("pty", &response_row.1, &response_row.2),
                    std::time::Instant::now(),
                );
            }
            response_persistence.completed = true;
            return Ok(());
        }
        return Err(MessageStoreError::InvalidTransition);
    }
    let allowed = if status == PTY_STATUS_REJECTED {
        matches!(current.0.as_str(), "queued" | "preparing" | "retry")
    } else {
        current.0 == PTY_STATUS_ACTUATING
    };
    if !allowed {
        return Err(MessageStoreError::InvalidTransition);
    }
    let reason_code = desired_reason_code;
    let reason_detail = desired_reason_detail;
    let changed = tx.execute(
        r#"UPDATE pty_input_operations SET status=?1,terminal_at=?2,updated_at=?2,
               payload=NULL,requested_agent_id=NULL,sender_identity_fingerprint=NULL,
               target_identity_fingerprint=NULL,authority_session_id=NULL,
               authority_client_id=NULL,authority_client_generation=NULL,
               lease_owner=NULL,lease_until=NULL,reason_code=?3,reason_detail=?4
           WHERE injection_id=?5 AND status=?6"#,
        params![
            status,
            now,
            reason_code,
            reason_detail,
            injection_id,
            current.0
        ],
    )?;
    if changed != 1 {
        return Err(MessageStoreError::InvalidTransition);
    }
    insert_pty_audit(&tx, injection_id, status, reason, now)?;
    insert_pty_tombstone(&tx, injection_id)?;
    if !tombstone_matches(&tx, injection_id)? {
        return Err(MessageStoreError::StoreCorrupt);
    }
    if let Err(error) = tx.commit() {
        if let Some(state) = response_close {
            state.fail_closed();
        }
        return Err(error.into());
    }
    if let Some(state) = response_close {
        state.settle_terminal(
            &response_row.0,
            &response_work_key("pty", &response_row.1, &response_row.2),
            std::time::Instant::now(),
        );
    }
    response_persistence.completed = true;
    Ok(())
}

pub fn retry_backoff_seconds(attempt: i64) -> i64 {
    let exponent = attempt.saturating_sub(1).min(5) as u32;
    5_i64.saturating_mul(2_i64.saturating_pow(exponent))
}

pub(super) fn insert_audit(
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

pub(super) fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn insert_pty_tombstone(
    tx: &rusqlite::Transaction<'_>,
    injection_id: &str,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        r#"INSERT INTO pty_input_tombstones(
             injection_id,sender_fqn,target_fqn,op_id,nonce_sha256,
             request_fingerprint,confirmation_tag,sender_incarnation_fingerprint,
             version,payload_sha256,payload_bytes,source_plane,selected_session_id,
             selected_backend,issued_at,expires_at,queued_at,actuating_at,
             terminal_at,status,reason_code,reason_detail
           ) SELECT injection_id,sender_fqn,target_fqn,op_id,nonce_sha256,
                    request_fingerprint,confirmation_tag,sender_incarnation_fingerprint,
                    version,payload_sha256,payload_bytes,source_plane,selected_session_id,
                    selected_backend,issued_at,expires_at,queued_at,actuating_at,
                    terminal_at,status,reason_code,reason_detail
             FROM pty_input_operations WHERE injection_id=?1
           ON CONFLICT(injection_id) DO NOTHING"#,
        [injection_id],
    )?;
    Ok(())
}
