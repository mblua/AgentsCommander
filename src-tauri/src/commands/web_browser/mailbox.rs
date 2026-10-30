//! Agent-facing web browser actions (#3367), delivered through the outbox like
//! `raise-hand`. Any live session may read the state; only orchestrators may
//! navigate or reload.

use std::path::Path;
use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};
use uuid::Uuid;

use super::WebBrowserSnapshot;
use crate::filesystem::phone_mailbox;
use crate::phone::mailbox::MailboxPoller;
use crate::phone::types::OutboxMessage;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;

pub(crate) const WEB_BROWSER_NAVIGATE_ACTION: &str = "web-browser-navigate";
pub(crate) const WEB_BROWSER_RELOAD_ACTION: &str = "web-browser-reload";
pub(crate) const WEB_BROWSER_STATUS_ACTION: &str = "web-browser-status";

pub(crate) fn is_web_browser_action(action: Option<&str>) -> bool {
    matches!(
        action,
        Some(WEB_BROWSER_NAVIGATE_ACTION | WEB_BROWSER_RELOAD_ACTION | WEB_BROWSER_STATUS_ACTION)
    )
}

/// Only the status read is open to every agent.
pub(crate) fn action_requires_orchestrator(action: &str) -> bool {
    action != WEB_BROWSER_STATUS_ACTION
}

pub(crate) fn response_json(action: &str, snapshot: &WebBrowserSnapshot) -> serde_json::Value {
    serde_json::json!({
        "action": action,
        "status": "ok",
        "url": snapshot.url,
        "title": snapshot.title,
        "loading": snapshot.loading,
        "visible": snapshot.visible,
    })
}

async fn caller_session<R: Runtime>(
    app: &AppHandle<R>,
    msg: &OutboxMessage,
) -> Option<SessionInfo> {
    let token = msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok())?;
    let mgr = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = session_mgr.read().await;
        guard.clone()
    };
    mgr.find_by_token(token).await
}

impl MailboxPoller {
    pub(crate) async fn handle_web_browser<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let action = msg.action.clone().unwrap_or_default();
        let Some(session) = caller_session(app, msg).await else {
            let reason =
                format!("{action} requires a valid session token; restart or respawn the session");
            return self.reject_message(path, msg, &reason).await;
        };
        if action_requires_orchestrator(&action) && !session.is_coordinator {
            let reason = format!("{action}: only room orchestrators may control the web browser");
            return self.reject_message(path, msg, &reason).await;
        }
        let result = match action.as_str() {
            WEB_BROWSER_NAVIGATE_ACTION => super::agent_navigate(app, &msg.body),
            WEB_BROWSER_RELOAD_ACTION => super::reload(app),
            _ => Ok(super::snapshot(app)),
        };
        match result {
            Ok(snapshot) => {
                let response = response_json(&action, &snapshot);
                self.write_web_browser_response(app, path, msg, &response, is_app_outbox)
                    .await
            }
            Err(e) => {
                self.reject_message(path, msg, &format!("{action}: {e}"))
                    .await
            }
        }
    }

    async fn write_web_browser_response<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        response: &serde_json::Value,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let (Some(rid), Ok(json)) = (&msg.request_id, serde_json::to_string_pretty(response)) {
            let mut dirs = Vec::new();
            if !is_app_outbox {
                dirs.extend(
                    path.parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses")),
                );
            }
            if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                let ac_dir = std::path::PathBuf::from(sender_path)
                    .join(crate::config::agent_local_dir_name());
                dirs.push(ac_dir.join("responses"));
            }
            for dir in dirs {
                let _ = phone_mailbox::ensure_mailbox_archive_directory(&dir);
                let file = dir.join(format!("{rid}.json"));
                let _ = phone_mailbox::write_mailbox_response(&file, json.as_bytes());
            }
        }
        self.move_to_delivered(app, path, msg).await
    }
}
