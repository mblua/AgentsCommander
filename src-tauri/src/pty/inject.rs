use std::sync::{Arc, Mutex};
use tauri::Manager;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::{Session, SessionStatus};

/// Returns true when the given shell command requires a separate Enter keystroke
/// to submit pasted input. Coding agents (Claude, Codex, Gemini, Cursor agent)
/// need explicit Enter after a text block paste. Plain shells (bash, powershell)
/// don't go through this path; they're filtered out before reaching
/// inject_text_into_session.
///
/// The shell may be a bare name ("claude") or a full path
/// ("C:\Users\...\.claude\local\claude.exe"), so we extract the filename stem
/// before matching.
pub(crate) fn needs_explicit_enter(shell: &str) -> bool {
    let stem = std::path::Path::new(shell.trim())
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(shell.trim())
        .to_lowercase();
    stem.starts_with("codex")
        || stem.starts_with("claude")
        || stem.starts_with("gemini")
        || stem == "agent"
}

/// Inject a text block into a session's PTY stdin.
///
/// For agents that require explicit Enter (Claude, Codex, Gemini, Cursor agent),
/// `\r` is sent twice, at 1500 ms and 2000 ms after the text write, as a
/// reliability measure against Enter not registering on the first attempt. For
/// plain shells (bash, powershell), no Enter is sent (the caller's text already
/// controls submission).
///
/// This is the ONLY function that should be used for text-block injection.
/// Direct keystrokes from xterm.js (single chars, Ctrl sequences) bypass this
/// and call PtyManager::write() directly via the pty_write Tauri command.
pub async fn inject_text_into_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
) -> Result<(), String> {
    inject_text_into_session_with_pre_write_check(app, session_id, text, || Ok(())).await
}

pub(crate) async fn inject_text_into_session_with_pre_write_check<R, F>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    pre_write_check: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    F: FnOnce() -> Result<(), String>,
{
    inject_text_into_session_impl(app, session_id, text, move |_| pre_write_check()).await
}

/// Canonical injector for trusted internal notices. Unlike the peer-compatible variant,
/// this rejects a missing, root, exited, agentless, or plain-shell recipient immediately
/// before the text write and gives the caller that resolved session snapshot for its final
/// canonical-path and authorization check.
fn validate_supported_agent_session(session: &Session, session_id: Uuid) -> Result<(), String> {
    if session.is_root_agent {
        return Err(format!(
            "Session {} is a root session, not a supported coordinator agent",
            session_id
        ));
    }
    if matches!(session.status, SessionStatus::Exited(_)) {
        return Err(format!("Session {} exited before injection", session_id));
    }
    if session.agent_id.is_none() {
        return Err(format!(
            "Session {} has no configured coding-agent identity",
            session_id
        ));
    }
    if !needs_explicit_enter(&session.shell) {
        return Err(format!(
            "Session {} shell '{}' is not a supported coding-agent CLI",
            session_id, session.shell
        ));
    }
    Ok(())
}

pub(crate) async fn inject_text_into_supported_agent_session_with_pre_write_check<R, F>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    pre_write_check: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    F: FnOnce(&Session) -> Result<(), String>,
{
    inject_text_into_session_impl(app, session_id, text, move |session| {
        let session = session.ok_or_else(|| {
            format!(
                "Session {} is missing before supported-agent injection",
                session_id
            )
        })?;
        validate_supported_agent_session(session, session_id)?;
        pre_write_check(session)
    })
    .await
}

async fn inject_text_into_session_impl<R, F>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
    pre_write_check: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    F: FnOnce(Option<&Session>) -> Result<(), String>,
{
    // Resolve one public snapshot without retaining a manager guard across an await.
    let session = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let manager = session_mgr.read().await.clone();
        manager.get_session(session_id).await
    };
    let shell = session.as_ref().map(|session| session.shell.clone());
    let send_enter = shell.as_deref().map(needs_explicit_enter).unwrap_or(false);
    log::info!(
        "[inject] session={} shell={:?} send_enter={}",
        session_id,
        shell,
        send_enter
    );

    // Deliberately synchronous and immediately adjacent to the PTY-owner lock. Callers of
    // the supported variant perform their final filesystem/config guard here.
    pre_write_check(session.as_ref())?;

    {
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
        pty_mgr
            .lock()
            .map_err(|_| "PtyManager lock poisoned".to_string())?
            .write(session_id, text.as_bytes())
            .map_err(|e| {
                log::error!("[inject] PTY write FAILED session={}: {}", session_id, e);
                format!("PTY write failed: {}", e)
            })?;
        log::info!(
            "[inject] PTY write OK session={} bytes={}",
            session_id,
            text.len()
        );
    }

    // Keep the established text/write/double-Enter algorithm shared by both variants.
    if send_enter {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        log::info!("[inject] sending Enter (1/2) for session {}", session_id);
        {
            let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
            pty_mgr
                .lock()
                .map_err(|_| "PtyManager lock poisoned".to_string())?
                .write(session_id, b"\r")
                .map_err(|e| format!("PTY Enter (1/2) write failed: {}", e))?;
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        log::info!("[inject] sending Enter (2/2) for session {}", session_id);
        {
            let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
            match pty_mgr
                .lock()
                .map_err(|_| "PtyManager lock poisoned".to_string())
                .and_then(|mgr| mgr.write(session_id, b"\r").map_err(|e| e.to_string()))
            {
                Ok(()) => {}
                Err(e) => log::warn!(
                    "[inject] Enter (2/2) failed for session {} (non-fatal): {}",
                    session_id,
                    e
                ),
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{needs_explicit_enter, validate_supported_agent_session};
    use crate::pty::backend::SessionBackendKind;
    use crate::session::session::{Session, SessionStatus};
    use chrono::Utc;
    use uuid::Uuid;

    fn supported_session() -> Session {
        Session {
            id: Uuid::new_v4(),
            name: "coordinator".to_string(),
            shell: "codex".to_string(),
            shell_args: Vec::new(),
            backend_kind: SessionBackendKind::LocalProcess,
            effective_shell_args: None,
            created_at: Utc::now(),
            working_directory: "C:/replica".to_string(),
            status: SessionStatus::Running,
            waiting_for_input: false,
            communication: None,
            pending_review: false,
            last_prompt: None,
            agent_id: Some("codex-profile".to_string()),
            agent_label: Some("Codex".to_string()),
            git_repos: Vec::new(),
            is_coordinator: true,
            is_root_agent: false,
            git_repos_gen: 0,
            token: Uuid::new_v4(),
            agent_kind: None,
            requested_profile: None,
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            effective_codex_home: None,
            resolved_claude_projects_dir: None,
            profile_content_hash: None,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
        }
    }

    #[test]
    fn supported_agent_final_snapshot_rejects_unsafe_recipient_records() {
        let valid = supported_session();
        assert!(validate_supported_agent_session(&valid, valid.id).is_ok());

        let mut root = supported_session();
        root.is_root_agent = true;
        assert!(validate_supported_agent_session(&root, root.id).is_err());

        let mut exited = supported_session();
        exited.status = SessionStatus::Exited(0);
        assert!(validate_supported_agent_session(&exited, exited.id).is_err());

        let mut agentless = supported_session();
        agentless.agent_id = None;
        assert!(validate_supported_agent_session(&agentless, agentless.id).is_err());

        let mut shell = supported_session();
        shell.shell = "pwsh".to_string();
        assert!(validate_supported_agent_session(&shell, shell.id).is_err());
    }

    #[test]
    fn agent_clis_require_explicit_enter() {
        for shell in [
            "codex",
            "codex.exe",
            "codex.cmd",
            "Codex",
            "CODEX",
            "C:\\Users\\maria\\.codex\\codex.exe",
            "/usr/local/bin/codex",
            "claude",
            "claude.exe",
            "C:\\Users\\maria\\.claude\\local\\claude.exe",
            "gemini",
            "gemini.exe",
            "agent",
            "agent.exe",
            "agent.cmd",
            "C:\\Users\\maria\\AppData\\Local\\Programs\\Cursor\\agent.exe",
            "  agent  ",
            "  codex  ", // leading/trailing whitespace tolerated
        ] {
            assert!(
                needs_explicit_enter(shell),
                "expected true for shell={:?}",
                shell
            );
        }
    }

    #[test]
    fn plain_shells_do_not_require_explicit_enter() {
        for shell in [
            "bash",
            "powershell.exe",
            "pwsh",
            "pwsh.exe",
            "cmd.exe",
            "zsh",
            "agentctl",
            "agentic",
            "",
            "   ",
        ] {
            assert!(
                !needs_explicit_enter(shell),
                "expected false for shell={:?}",
                shell
            );
        }
    }
}
