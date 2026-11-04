//! Payload formatting and prewrite admission for peer wake injection.
use super::super::handoff::resolve_remote_pty_command;
use super::super::notice::{format_wake_content, WakeContent, WakeDeliveryOrigin};
use super::super::pty_claim::response_diag_debug_only;
use crate::phone::types::OutboxMessage;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use std::path::Path;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub(crate) async fn logical_session_snapshot(
    session_mgr: &tokio::sync::RwLock<SessionManager>,
    session_id: Uuid,
    command: &str,
) -> Result<(String, bool), String> {
    let mgr = session_mgr.read().await;
    let sessions = mgr.list_sessions().await;
    match sessions
        .iter()
        .find(|session| session.id == session_id.to_string())
    {
        Some(session) => Ok((session.shell.clone(), session.waiting_for_input)),
        None => Err(format!(
            "Session not found: {session_id} - cannot execute logical remote command '{command}'"
        )),
    }
}

pub(crate) fn standard_payload(
    msg: &OutboxMessage,
    body: &str,
    use_markers: bool,
    origin: WakeDeliveryOrigin,
    comanaged: bool,
) -> String {
    match (use_markers, msg.request_id.as_ref()) {
        (true, Some(rid)) if comanaged => {
            let display_from =
                crate::phone::messaging::compose_sender_for_comanaged_origin(&msg.from);
            crate::phone::messaging::format_pty_wrap_with_markers(&display_from, body, rid)
        }
        (true, Some(rid)) => {
            crate::phone::messaging::format_pty_wrap_with_markers(&msg.from, body, rid)
        }
        _ => format_wake_content(WakeContent::Peer {
            from: &msg.from,
            body,
            origin,
            comanaged,
        }),
    }
}

pub(crate) fn standard_prewrite_admission(
    msg: &OutboxMessage,
    session: &crate::session::session::Session,
    current: &crate::session::manager::CarrierSnapshot,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    owned_replacement: Option<&crate::session::selection::OwnedReplacementContinuation>,
) -> Result<(), String> {
    response_diag_local!(
        "prewrite",
        "callback_reached",
        Uuid::parse_str(&msg.id).ok()
    );
    if owned_replacement.is_none()
        && crate::config::sessions_persistence::unconfirmed_replacement_work_waits(
            &msg.to, session.id,
        )
    {
        response_diag_local!(
            "prewrite",
            "owned_replacement_wait",
            Uuid::parse_str(&msg.id).ok()
        );
        return Err(crate::pty::inject::OWNED_WORK_SCOPE_WAIT.into());
    }
    if owned_replacement
        .is_some_and(|continuation| !continuation.pre_write_work_matches(current, pty_mgr))
    {
        response_diag_local!(
            "prewrite",
            "owned_replacement_mismatch",
            Uuid::parse_str(&msg.id).ok()
        );
        return Err("replacement work binding requires recapture".into());
    }
    Ok(())
}

pub(crate) fn logical_prewrite_admission(
    msg: &OutboxMessage,
    session: &crate::session::session::Session,
    current: &crate::session::manager::CarrierSnapshot,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    owned_replacement: Option<&crate::session::selection::OwnedReplacementContinuation>,
    source: Option<&Path>,
    resolved: super::super::notice::ResolvedRemotePtyCommand,
) -> Result<(), String> {
    if owned_replacement.is_none()
        && crate::config::sessions_persistence::unconfirmed_replacement_work_waits(
            &msg.to, session.id,
        )
    {
        return Err(crate::pty::inject::OWNED_WORK_SCOPE_WAIT.into());
    }
    if owned_replacement
        .is_some_and(|continuation| !continuation.pre_write_work_matches(current, pty_mgr))
    {
        return Err("replacement work binding requires recapture".into());
    }
    if let Some(source) = source {
        if msg.command.as_deref() != Some("clear")
            || resolve_remote_pty_command(&session.shell, "clear")? != resolved
        {
            return Err("codex_clear_execution_error: provider changed".into());
        }
        super::super::clear_attempt::persist_attempt(source, msg, session.id)
            .map_err(|error| format!("codex_clear_execution_error: {error}"))?;
    }
    Ok(())
}

pub(crate) fn response_attempt(
    response: Option<&super::super::pty_claim::ResponseWakeWork>,
    session_id: Uuid,
    prepared: bool,
) -> Option<Arc<super::super::pty_claim::ResponseCandidateAttempt>> {
    let attempt = response.and_then(|work| match work.attempt.lock() {
        Ok(attempt) => attempt.clone(),
        Err(_) => {
            response_diag_local!("inject", "attempt_lock_failure", work.diagnostic_message_id);
            None
        }
    });
    response_diag_debug_only(|| {
        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag \
            site={} outcome=observed stage=inject_presence id=unknown member={} prepared={} \
            attempt={} work={}",
            line!(),
            session_id,
            prepared,
            attempt.is_some(),
            response.is_some()
        );
    });
    attempt
}

pub(crate) fn log_standard_payload(session_id: Uuid, msg: &OutboxMessage, payload: &str) {
    // SECURITY: this `first_100` log MUST NOT see credential values.
    // Credentials are env-only and must never be routed through PTY payloads.
    // Keep this log limited to standard message payloads.
    log::debug!(
        "[mailbox] Injecting into PTY session={} msg={} payload_len={} first_100={:?}",
        session_id,
        msg.id,
        payload.len(),
        payload.chars().take(100).collect::<String>()
    );
}
