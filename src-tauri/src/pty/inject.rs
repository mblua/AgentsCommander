use std::sync::{Arc, Mutex};
use tauri::Manager;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PtyInjectionProfile {
    Established,
    Cursor,
    Pi,
    Unsupported,
}

fn shell_file_stem(shell: &str) -> String {
    std::path::Path::new(shell.trim())
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(shell.trim())
        .to_lowercase()
}

fn pty_injection_profile(shell: &str) -> PtyInjectionProfile {
    let stem = shell_file_stem(shell);
    if stem.starts_with("claude") || stem.starts_with("codex") || stem.starts_with("gemini") {
        PtyInjectionProfile::Established
    } else if stem == "agent" {
        PtyInjectionProfile::Cursor
    } else if stem == "pi" {
        PtyInjectionProfile::Pi
    } else {
        PtyInjectionProfile::Unsupported
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogicalPtyCommand {
    Clear,
    Compact,
}

impl LogicalPtyCommand {
    pub(crate) fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "clear" => Some(Self::Clear),
            "compact" => Some(Self::Compact),
            _ => None,
        }
    }

    pub(crate) fn creates_fresh_boundary(self) -> bool {
        matches!(self, Self::Clear)
    }
}

/// Returns true when the direct shell command uses the canonical delayed Enter
/// sequence for pasted text blocks. Classification is lexical: it uses only the
/// trimmed shell file stem and does not inspect shell arguments or wrapper
/// contents.
pub(crate) fn needs_explicit_enter(shell: &str) -> bool {
    !matches!(
        pty_injection_profile(shell),
        PtyInjectionProfile::Unsupported
    )
}

/// Resolve a logical PTY action to provider text for a directly launched shell.
pub(crate) fn resolve_logical_command_text(
    shell: &str,
    command: LogicalPtyCommand,
) -> Option<&'static str> {
    match (pty_injection_profile(shell), command) {
        (
            PtyInjectionProfile::Established | PtyInjectionProfile::Cursor,
            LogicalPtyCommand::Clear,
        ) => Some("/clear"),
        (
            PtyInjectionProfile::Established | PtyInjectionProfile::Cursor,
            LogicalPtyCommand::Compact,
        ) => Some("/compact"),
        (PtyInjectionProfile::Pi, LogicalPtyCommand::Clear) => Some("/new"),
        _ => None,
    }
}

pub(crate) fn supports_auto_self_maintenance(shell: &str) -> bool {
    matches!(
        pty_injection_profile(shell),
        PtyInjectionProfile::Established | PtyInjectionProfile::Pi
    )
}

pub(crate) fn supports_self_handoff_switch(shell: &str) -> bool {
    matches!(
        pty_injection_profile(shell),
        PtyInjectionProfile::Established | PtyInjectionProfile::Cursor
    )
}

/// Inject a text block into a session's PTY stdin.
///
/// Direct Claude, Codex, Gemini, Cursor agent, and exact-stem Pi shells receive
/// `\r` twice, at 1500 ms and 2000 ms after the text write, as a reliability
/// measure against Enter not registering on the first attempt. Plain shells do
/// not receive an added Enter.
///
/// This is the ONLY function that should be used for text-block injection.
/// Direct keystrokes from xterm.js bypass this and call PtyManager::write()
/// directly via the pty_write Tauri command.
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
    // Resolve shell without holding any lock across an await point.
    let shell = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let result = mgr.get_shell(session_id).await;
        drop(mgr);
        result
    };
    let shell = shell.ok_or_else(|| format!("Session not found: {}", session_id))?;

    let send_enter = needs_explicit_enter(&shell);
    log::info!(
        "[inject] session={} shell={:?} send_enter={}",
        session_id,
        shell,
        send_enter
    );

    pre_write_check()?;

    // Write the text block.
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

    // Supported interactive agent CLIs receive two staggered Enters. The
    // second is nonfatal because the first may already have submitted the text.
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
    use super::{
        inject_text_into_session, needs_explicit_enter, resolve_logical_command_text,
        supports_auto_self_maintenance, supports_self_handoff_switch, LogicalPtyCommand,
    };
    use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
    use crate::pty::manager::PtyManager;
    use crate::session::manager::SessionManager;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    #[test]
    fn direct_shell_capability_matrix() {
        let pi_positive = [
            "pi",
            "PI",
            "pi.exe",
            "Pi.CMD",
            "pi.ps1",
            r"C:\Tools\pi.exe",
            r"\\server\share\pi.cmd",
            r"\\?\C:\Tools\pi.exe",
            "/usr/local/bin/pi",
            "  pi  ",
        ];
        for shell in pi_positive {
            assert!(needs_explicit_enter(shell), "Pi positive: {shell:?}");
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
                Some("/new"),
                "Pi clear mapping: {shell:?}"
            );
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
                None,
                "Pi compact remains unsupported: {shell:?}"
            );
            assert!(supports_auto_self_maintenance(shell));
            assert!(!supports_self_handoff_switch(shell));
        }

        let unsupported = [
            "pip",
            "pipx",
            "ping",
            "pixel",
            "pi-agent",
            "pi2",
            "pi-claude",
            r"C:\pi\runner.exe",
            "cmd.exe",
            "pwsh",
            "",
            "   ",
            "bash",
        ];
        for shell in unsupported {
            assert!(!needs_explicit_enter(shell), "unsupported: {shell:?}");
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
                None
            );
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
                None
            );
            assert!(!supports_auto_self_maintenance(shell));
            assert!(!supports_self_handoff_switch(shell));
        }

        for shell in [
            "claude",
            "claude-pi",
            "codex-wrapper.cmd",
            "gemini-proxy.exe",
        ] {
            assert!(needs_explicit_enter(shell));
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
                Some("/clear")
            );
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
                Some("/compact")
            );
            assert!(supports_auto_self_maintenance(shell));
            assert!(supports_self_handoff_switch(shell));
        }

        for shell in ["agent", "agent.exe", r"C:\Cursor\agent.cmd"] {
            assert!(needs_explicit_enter(shell));
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
                Some("/clear")
            );
            assert_eq!(
                resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
                Some("/compact")
            );
            assert!(!supports_auto_self_maintenance(shell));
            assert!(supports_self_handoff_switch(shell));
        }
        assert!(!needs_explicit_enter("agentctl"));
        assert!(!needs_explicit_enter("agentic"));
    }

    #[test]
    fn logical_command_parser_and_boundary_matrix() {
        assert_eq!(
            LogicalPtyCommand::from_wire_value("clear"),
            Some(LogicalPtyCommand::Clear)
        );
        assert_eq!(
            LogicalPtyCommand::from_wire_value("compact"),
            Some(LogicalPtyCommand::Compact)
        );
        for value in ["Clear", "COMPACT", "", "new"] {
            assert_eq!(LogicalPtyCommand::from_wire_value(value), None);
        }
        assert!(LogicalPtyCommand::Clear.creates_fresh_boundary());
        assert!(!LogicalPtyCommand::Compact.creates_fresh_boundary());
    }

    #[derive(Default)]
    struct RecordingBackend {
        writes: Mutex<Vec<(Uuid, Vec<u8>)>>,
    }

    impl PtyBackend for RecordingBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            _spec: BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async { Ok(()) })
        }

        fn write(&self, id: Uuid, data: &[u8]) -> Result<(), crate::errors::AppError> {
            self.writes.lock().unwrap().push((id, data.to_vec()));
            Ok(())
        }

        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
            Ok(())
        }

        fn has_session(&self, _id: Uuid) -> bool {
            true
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    #[tokio::test]
    async fn missing_session_record_with_lingering_route_writes_nothing() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(id, SessionBackendKind::LocalProcess);
        let app = tauri::test::mock_builder()
            .manage(Arc::new(tokio::sync::RwLock::new(SessionManager::new())))
            .manage(pty)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();

        let err = inject_text_into_session(app.handle(), id, "/new")
            .await
            .unwrap_err();

        assert_eq!(err, format!("Session not found: {id}"));
        assert!(backend.writes.lock().unwrap().is_empty());
    }
}
