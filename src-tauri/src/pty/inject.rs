use std::sync::{Arc, Mutex};
use tauri::Manager;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

/// Returns true when the given shell command requires a separate Enter keystroke
/// to submit pasted input. Coding agents (Claude, Codex, etc.) all need explicit
/// Enter after a text block paste. Plain shells (bash, powershell) don't go through
/// this path — they're filtered out before reaching inject_text_into_session.
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
    stem.starts_with("codex") || stem.starts_with("claude") || stem.starts_with("gemini")
}

/// Inject a text block into a session's PTY stdin.
///
/// For agents that require explicit Enter (Claude, Codex, Gemini), `\r` is
/// sent twice — at 1500 ms and 2000 ms after the text write — as a reliability
/// measure against Enter not registering on the first attempt. For plain shells
/// (bash, powershell), no Enter is sent (the caller's text already controls
/// submission).
///
/// This is the ONLY function that should be used for text-block injection.
/// Direct keystrokes from xterm.js (single chars, Ctrl sequences) bypass this
/// and call PtyManager::write() directly via the pty_write Tauri command.
pub async fn inject_text_into_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
    text: &str,
) -> Result<(), String> {
    // Resolve shell without holding any lock across an await point
    let shell = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let result = mgr.get_shell(session_id).await;
        drop(mgr);
        result
    };

    let send_enter = shell.as_deref().map(needs_explicit_enter).unwrap_or(false);
    log::info!(
        "[inject] session={} shell={:?} send_enter={}",
        session_id,
        shell,
        send_enter
    );

    // Write the text block
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

    // Agent CLIs (Claude, Codex): send Enter twice with staggered delays.
    // Sometimes a single \r doesn't register (race with paste-detection mode).
    // The second \r is a safety net — if the first worked, the agent is already
    // processing and an extra Enter on empty input is harmless.
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
    use super::needs_explicit_enter;

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
