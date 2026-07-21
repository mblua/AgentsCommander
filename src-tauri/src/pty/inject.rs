use std::sync::{Arc, Mutex};
use tauri::Manager;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::{Session, SessionStatus};

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
        PtyInjectionProfile::Established | PtyInjectionProfile::Cursor | PtyInjectionProfile::Pi
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
    inject_text_into_session_impl(app, session_id, text, move |session| {
        session.ok_or_else(|| format!("Session not found: {}", session_id))?;
        pre_write_check()
    })
    .await
}

/// Canonical injector for trusted internal notices. This adds root, exited,
/// agentless, and plain-shell rejection and gives the caller the resolved
/// session snapshot for its final canonical-path and authorization check.
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
        supports_auto_self_maintenance, supports_self_handoff_switch,
        validate_supported_agent_session, LogicalPtyCommand,
    };
    use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
    use crate::pty::manager::PtyManager;
    use crate::session::manager::SessionManager;
    use crate::session::session::{Session, SessionStatus};
    use chrono::Utc;
    use std::sync::{Arc, Mutex};
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
            assert!(supports_self_handoff_switch(shell), "Pi switch source: {shell:?}");
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
