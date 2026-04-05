use std::sync::{Arc, Mutex};
use tauri::Manager;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::pty::transcript::{InjectReason, TranscriptWriter};
use crate::session::manager::SessionManager;

/// Returns true when the given shell command requires a separate Enter keystroke
/// to submit pasted input. Coding agents (Claude, Codex, etc.) all need explicit
/// Enter after a text block paste. Plain shells (bash, powershell) don't go through
/// this path — they're filtered out before reaching inject_text_into_session.
///
/// The shell may be a bare name ("claude") or a full path
/// ("C:\Users\...\.claude\local\claude.exe"), so we extract the filename stem
/// before matching.
fn needs_explicit_enter(shell: &str) -> bool {
    let stem = std::path::Path::new(shell.trim())
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(shell.trim())
        .to_lowercase();
    stem.starts_with("codex") || stem.starts_with("claude")
}

/// Inject a text block into a session's PTY stdin.
///
/// - `submit = false` → passive injection (token refresh).
///   Text is written as-is. No Enter is ever sent, regardless of agent type.
/// - `submit = true` → active injection (init prompt, message delivery, Telegram input).
///   For agents that require explicit Enter (Claude, Codex), a `\r` is sent
///   as a separate write after a 1500 ms delay to let the agent finish
///   processing the pasted text block.
///
/// This is the ONLY function that should be used for text-block injection.
/// Direct keystrokes from xterm.js (single chars, Ctrl sequences) bypass this
/// and call PtyManager::write() directly via the pty_write Tauri command.
pub async fn inject_text_into_session(
    app: &tauri::AppHandle,
    session_id: Uuid,
    text: &str,
    submit: bool,
    inject_reason: InjectReason,
    inject_sender: Option<String>,
) -> Result<(), String> {
    // Resolve shell without holding any lock across an await point
    let shell = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let result = mgr.get_shell(session_id).await;
        drop(mgr);
        result
    };

    let send_enter = submit && shell.as_deref().map(needs_explicit_enter).unwrap_or(false);
    log::info!("[inject] session={} submit={} shell={:?} send_enter={}", session_id, submit, shell, send_enter);

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
        log::info!("[inject] PTY write OK session={} bytes={}", session_id, text.len());
    }

    // Record transcript
    {
        let transcript = app.state::<TranscriptWriter>();
        transcript.record_inject(session_id, text.as_bytes(), inject_reason, inject_sender, submit);
    }

    // Agent CLIs (Claude, Codex): send Enter as a separate write after a delay.
    // The delay must be long enough for the agent to finish processing the pasted
    // text block and exit any internal "paste detection" mode.
    if send_enter {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        log::info!("[inject] sending Enter for session {}", session_id);
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
        pty_mgr
            .lock()
            .map_err(|_| "PtyManager lock poisoned".to_string())?
            .write(session_id, b"\r")
            .map_err(|e| format!("PTY Enter write failed: {}", e))?;

        let transcript = app.state::<TranscriptWriter>();
        transcript.record_inject(session_id, b"\r", InjectReason::EnterKeystroke, None, true);
    }

    Ok(())
}
