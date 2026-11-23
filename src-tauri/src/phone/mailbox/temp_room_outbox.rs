//! Temp-room mailbox routes: a temp room talks only with the Root Agent
//! and verified Room orchestrators, both directions.

use super::outbox_auth::outbox_route_project_paths;
use super::poller::{MailboxPoller, OutboxStage};
use super::pty_observation::PtyAuthorityState;
use crate::config::{root_agent, teams, temp_rooms};
use crate::phone::messaging;
use crate::phone::types::OutboxMessage;
use std::path::Path;

pub(crate) fn validate_temp_room_sender_route(
    to: &str,
    project_paths: &[String],
    is_master: bool,
    saw_session_token: bool,
) -> Result<(), &'static str> {
    if !is_master && !saw_session_token {
        return Err("Temporary Room sender requires its live session token");
    }
    if root_agent::is_root_agent_target(to)
        || teams::verified_wg_coordinator_target(to, project_paths).is_some()
    {
        return Ok(());
    }
    Err("Temporary Rooms can only message the Root Agent and verified Room orchestrator replicas")
}

/// No master input: the master token never bypasses this rule.
pub(crate) fn validate_to_temp_room_route(
    from: &str,
    project_paths: &[String],
) -> Result<(), &'static str> {
    if teams::verified_wg_coordinator_target(from, project_paths).is_none() {
        return Err(
            "Only the Root Agent and verified Room orchestrator replicas may message a \
            Temporary Room",
        );
    }
    Ok(())
}

/// The payload is a file notification for a file directly in
/// `<base>/temp-room-trNN/messaging/` of the sending room.
pub(crate) fn validate_temp_room_sender_payload(
    msg: &OutboxMessage,
    base: &Path,
) -> Result<(), String> {
    if msg.command.is_some() {
        return Err(
            "Temporary Room messages must use --send; remote commands are not allowed".into(),
        );
    }
    if msg.action.is_some() {
        return Err(
            "Temporary Room messages must use --send; action messages are not allowed".into(),
        );
    }
    let room = temp_rooms::temp_room_name_for_address(&msg.from)
        .ok_or_else(|| "Temporary Room sender address is invalid".to_string())?;

    let notification_path = messaging::parse_file_notification(&msg.body).ok_or_else(|| {
        "Temporary Room messages must be canonical file notifications".to_string()
    })?;
    let filename = messaging::notification_filename(notification_path).ok_or_else(|| {
        "Temporary Room file notification must point to a Markdown file".to_string()
    })?;
    if Path::new(notification_path)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        != Some(filename)
    {
        return Err("Temporary Room file notification must point to a Markdown file".into());
    }
    let notification_path = Path::new(notification_path);
    if !notification_path.is_absolute() {
        return Err("Temporary Room file notification must use an absolute path".into());
    }
    let parent = notification_path.parent().ok_or_else(|| {
        "Temporary Room file notification target has no parent directory".to_string()
    })?;
    let canon_file = messaging::resolve_existing_message(parent, filename)
        .map_err(|e| format!("Temporary Room file notification target is not readable: {e}"))?;

    let own_messaging_dir = base.join(&room).join(messaging::MESSAGING_DIR_NAME);
    let canon_own = messaging::resolve_existing_message(&own_messaging_dir, filename).ok();
    if canon_own.as_deref() != Some(canon_file.as_path()) {
        return Err(format!(
            "Temporary Room file notification must point inside {room}/messaging"
        ));
    }
    Ok(())
}

impl MailboxPoller {
    pub(super) async fn authorize_temp_room_sender_outbox<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        path: &Path,
        msg: &OutboxMessage,
        outbox_project: &Option<String>,
        authentication: (bool, bool),
    ) -> OutboxStage {
        let (is_master, saw_session_token) = authentication;
        let paths = outbox_route_project_paths(app, outbox_project).await;
        if let Err(reason) =
            validate_temp_room_sender_route(&msg.to, &paths, is_master, saw_session_token)
        {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(path, msg, reason).await,
            ));
        }
        let payload = match crate::config::config_dir() {
            Some(base) => validate_temp_room_sender_payload(msg, &base),
            None => Err("Could not resolve app config directory".to_string()),
        };
        if let Err(reason) = payload {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(path, msg, &reason).await,
            ));
        }
        log::debug!(
            "[mailbox] Temporary Room routing check passed: '{}' -> '{}'",
            msg.from,
            msg.to
        );
        Ok(std::ops::ControlFlow::Continue(()))
    }

    pub(super) async fn authorize_temp_room_recipient_outbox<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        path: &Path,
        msg: &OutboxMessage,
        outbox_project: &Option<String>,
    ) -> OutboxStage {
        let paths = outbox_route_project_paths(app, outbox_project).await;
        if let Err(reason) = validate_to_temp_room_route(&msg.from, &paths) {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(path, msg, reason).await,
            ));
        }
        log::debug!(
            "[mailbox] To-Temporary-Room routing check passed: '{}' -> '{}'",
            msg.from,
            msg.to
        );
        Ok(std::ops::ControlFlow::Continue(()))
    }
}
