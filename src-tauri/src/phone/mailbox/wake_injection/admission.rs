//! Payload formatting and prewrite admission for peer wake injection.
use super::super::handoff::resolve_remote_pty_command;
use super::super::notice::{
    format_wake_content, ResolvedWakeSpawnPlan, WakeContent, WakeDeliveryOrigin,
    ERR_INVALID_PEER_TEXT,
};
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

/// #4309 - gate for rendered peer wake text. The fixed leading `\n` and trailing
/// `\n\r` are framing; the rest is data. The whole block stays within the bound.
pub(crate) fn checked_peer_payload(payload: String) -> Result<String, String> {
    let data = payload
        .strip_prefix('\n')
        .and_then(|rest| rest.strip_suffix("\n\r"));
    let verdict = match data {
        None => Err("invalid_text:framing".to_string()),
        Some(_) if payload.len() > crate::pty::backend::PTY_INPUT_MAX_BYTES => {
            Err("payload_too_large".to_string())
        }
        Some(data) => {
            crate::pty::inject::validate_pty_input_text(data).map_err(|error| error.to_string())
        }
    };
    verdict
        .map(|()| payload)
        .map_err(|reason| format!("{ERR_INVALID_PEER_TEXT}: {reason}"))
}
/// #4309 - true when the written body depends on the recipient backend: only a
/// container recipient inlines a FilesystemPoller file notification.
pub(crate) fn body_depends_on_backend(msg: &OutboxMessage, origin: WakeDeliveryOrigin) -> bool {
    msg.command.is_none()
        && origin == WakeDeliveryOrigin::FilesystemPoller
        && crate::phone::messaging::parse_file_notification(&msg.body).is_some()
}
/// #4309 - true when a session created from `plan` runs in a container.
pub(crate) fn plan_is_container(plan: &ResolvedWakeSpawnPlan) -> bool {
    plan.configured_spawn.as_ref().is_some_and(|spawn| {
        crate::pty::backend::SessionBackendKind::from(&spawn.backend)
            == crate::pty::backend::SessionBackendKind::ContainerTransport
    })
}
/// #4309 - pre-actuation gate for the exact peer text a wake writes when its
/// standard body is `body`. Production peer wakes inject interactively
/// (`inject_wake_into_pty` passes `true`), so response markers never render.
pub(crate) fn admit_peer_text(
    msg: &OutboxMessage,
    body: &str,
    origin: WakeDeliveryOrigin,
    comanaged: bool,
) -> Result<(), String> {
    let payload = match &msg.command {
        Some(_) if msg.body.is_empty() => return Ok(()),
        Some(_) => crate::phone::messaging::format_pty_wrap(&msg.from, &msg.body),
        None => standard_payload(msg, body, false, origin, comanaged),
    };
    checked_peer_payload(payload).map(drop)
}
