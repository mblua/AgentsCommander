use std::fmt;
use std::sync::{Arc, Mutex};

use tauri::Manager;
use uuid::Uuid;

use crate::pty::backend::PTY_INPUT_MAX_BYTES;
use crate::pty::manager::{PtyInputPermit, PtyManager, PtyRouteWriteGuard};
use crate::session::manager::SessionManager;
use crate::session::session::{Session, SessionStatus};

/// Stable, payload-free validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyInputTextErrorKind {
    Empty,
    TooLarge,
    ForbiddenScalar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyInputTextError {
    pub kind: PtyInputTextErrorKind,
    pub byte_offset: usize,
    pub scalar_offset: usize,
    pub code_point: Option<u32>,
}

impl fmt::Display for PtyInputTextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            PtyInputTextErrorKind::Empty => formatter.write_str("invalid_text:empty"),
            PtyInputTextErrorKind::TooLarge => formatter.write_str("payload_too_large"),
            PtyInputTextErrorKind::ForbiddenScalar => write!(
                formatter,
                "invalid_text:byte={}:scalar={}:codepoint=U+{:04X}",
                self.byte_offset,
                self.scalar_offset,
                self.code_point.unwrap_or_default()
            ),
        }
    }
}

impl std::error::Error for PtyInputTextError {}

/// #1157 - the forbidden-scalar set of [`validate_pty_input_text`], extracted so
/// the injected-message sanitizer (`config::injected_messages::sanitize`) strips
/// exactly what this validator rejects. Deriving both from one predicate is what
/// keeps the two from drifting; there must be no second copy of this list.
///
/// Covers C0 except `\t` and `\n`, DEL and C1, and the whole bidi and separator
/// class (U+061C, U+200E, U+200F, U+2028, U+2029, U+202A-U+202E, U+2066-U+2069).
pub(crate) fn is_forbidden_pty_scalar(ch: char) -> bool {
    matches!(
        ch,
        '\u{0000}'..='\u{0008}'
            | '\u{000b}'
            | '\u{000c}'
            | '\u{000d}'
            | '\u{000e}'..='\u{001f}'
            | '\u{007f}'..='\u{009f}'
            | '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

/// The single authoritative in-process exact-text validator.
///
/// Accepted text is returned unchanged by callers. This function performs no
/// trimming, normalization, line-ending conversion, or wrapping.
pub fn validate_pty_input_text(text: &str) -> Result<(), PtyInputTextError> {
    if text.is_empty() {
        return Err(PtyInputTextError {
            kind: PtyInputTextErrorKind::Empty,
            byte_offset: 0,
            scalar_offset: 0,
            code_point: None,
        });
    }
    if text.len() > PTY_INPUT_MAX_BYTES {
        return Err(PtyInputTextError {
            kind: PtyInputTextErrorKind::TooLarge,
            byte_offset: PTY_INPUT_MAX_BYTES,
            scalar_offset: text
                .char_indices()
                .take_while(|(offset, _)| *offset < PTY_INPUT_MAX_BYTES)
                .count(),
            code_point: None,
        });
    }

    for (scalar_offset, (byte_offset, ch)) in text.char_indices().enumerate() {
        if is_forbidden_pty_scalar(ch) {
            return Err(PtyInputTextError {
                kind: PtyInputTextErrorKind::ForbiddenScalar,
                byte_offset,
                scalar_offset,
                code_point: Some(ch as u32),
            });
        }
    }
    Ok(())
}

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
    if stem.starts_with("claude")
        || stem.starts_with("codex")
        || matches!(stem.as_str(), "agy" | "antigravity")
    {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSubmitOutcome {
    TextWriteFailed,
    RequiredEnterFailed,
    Submitted { redundant_enter_failed: bool },
}

/// Perform the synchronous, linearized first write and consume the non-Send
/// lifecycle guard before any await can occur.
pub fn write_exact_agent_input_first(route_guard: PtyRouteWriteGuard<'_>, bytes: &[u8]) -> bool {
    route_guard.write(bytes).is_ok()
}

/// Finish an exact submission while the same per-session input permit remains
/// held. Backend error strings are deliberately discarded at the phase seam.
pub async fn submit_exact_agent_input_with_permit(
    permit: &PtyInputPermit,
    text_write_succeeded: bool,
) -> AgentSubmitOutcome {
    if !text_write_succeeded {
        return AgentSubmitOutcome::TextWriteFailed;
    }
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    if PtyManager::write_with_permit(permit, b"\r").is_err() {
        return AgentSubmitOutcome::RequiredEnterFailed;
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    AgentSubmitOutcome::Submitted {
        redundant_enter_failed: PtyManager::write_with_permit(permit, b"\r").is_err(),
    }
}

/// Inject a text block into a session's PTY stdin.
///
/// Direct Claude, Codex, Antigravity, Cursor agent, and exact-stem Pi shells receive
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
            "Session {} is a root session, not a supported orchestrator agent",
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

    // Acquire the per-session input permit before running the pre-write closure,
    // so the closure and the first checked write happen together at the real
    // serialized write boundary (plan 7.5). The permit is held across the whole
    // text-then-Enter seam, which gives legacy/logical injection writer
    // serialization (invariant 3) without changing its public byte sequence.
    let pty_manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let permit = PtyManager::acquire_input_writer(&pty_manager, session_id)
        .await
        .map_err(|error| error.to_string())?;

    if let Some(menu_guard) = app.try_state::<Arc<crate::pty::menu_guard::MenuGuard>>() {
        if menu_guard.is_blocked(session_id) {
            return Err(format!(
                "{}: session {} is blocked by interactive menu",
                crate::pty::menu_guard::ERR_MENU_GUARD_DEFERRED,
                session_id
            ));
        }
    }

    // Deliberately synchronous and immediately adjacent to the serialized write
    // boundary. Callers of the supported variant perform their final
    // filesystem/config guard here.
    pre_write_check(session.as_ref())?;

    // Write the text block through the held permit.
    PtyManager::write_with_permit(&permit, text.as_bytes()).map_err(|error| {
        log::error!(
            "[inject] PTY write FAILED session={}: {}",
            session_id,
            error
        );
        format!("PTY write failed: {}", error)
    })?;
    log::info!(
        "[inject] PTY write OK session={} bytes={}",
        session_id,
        text.len()
    );
    crate::commands::pty::mark_successful_pty_write_busy(app, session_id, text.len()).await;

    // Supported interactive agent CLIs receive two staggered Enters. The
    // second is nonfatal because the first may already have submitted the text.
    if send_enter {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        log::info!("[inject] sending Enter (1/2) for session {}", session_id);
        PtyManager::write_with_permit(&permit, b"\r")
            .map_err(|error| format!("PTY Enter (1/2) write failed: {}", error))?;

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        log::info!("[inject] sending Enter (2/2) for session {}", session_id);
        if let Err(error) = PtyManager::write_with_permit(&permit, b"\r") {
            log::warn!(
                "[inject] Enter (2/2) failed for session {} (non-fatal): {}",
                session_id,
                error
            );
        }
    }

    // #1682 - an injected text block is a message to the agent, submitted on
    // every branch but R8, so it arms this session and the busy->idle edges
    // that follow stamp `tooling.lastAgentMessageAt`. Single funnel for every
    // injection path (inter-agent wake, Loop delivery, the self-clear, self-switch
    // and self-restart resume prompts, internal system notices, Telegram inject),
    // so no caller needs its own site. Placed here rather than beside the
    // `mark_successful_pty_write_busy` call above so an early `Err` return
    // leaves the session unarmed. NOT "an undelivered message never arms": R8.
    {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let manager = session_mgr.read().await.clone();
        manager.arm_agent_turn(session_id).await;
        // #1682 - the text write above marked this session through
        // `mark_successful_pty_write_busy` (`:388`); this clear cancels that mark.
        // It keys on ARMING, not on proven delivery: R8 arms and clears with
        // nothing submitted. Self-cancelling here, so no caller needs its own site.
        crate::commands::pty::clear_control_write_mark(app, session_id);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
    use chrono::Utc;
    use std::collections::VecDeque;

    #[derive(serde::Deserialize)]
    struct Fixture {
        name: String,
        text: String,
        valid: bool,
    }

    #[test]
    fn shared_validation_fixture_is_authoritative() {
        let rows: Vec<Fixture> = serde_json::from_str(include_str!(
            "../../../crates/session-bridge/tests/fixtures/pty_input_validation.json"
        ))
        .unwrap();
        for row in rows {
            assert_eq!(
                validate_pty_input_text(&row.text).is_ok(),
                row.valid,
                "fixture {}",
                row.name
            );
        }
    }

    #[test]
    fn validator_rejects_every_forbidden_control_and_bidi_scalar() {
        let mut forbidden: Vec<u32> = (0x00..=0x1f)
            .filter(|code| !matches!(code, 0x09 | 0x0a))
            .chain(0x7f..=0x9f)
            .collect();
        forbidden.extend([
            0x061c, 0x200e, 0x200f, 0x2028, 0x2029, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066,
            0x2067, 0x2068, 0x2069,
        ]);
        for code in forbidden {
            let scalar = char::from_u32(code).expect("valid scalar");
            let text = format!("a{scalar}b");
            assert!(
                validate_pty_input_text(&text).is_err(),
                "U+{code:04X} must reject"
            );
        }
        for accepted in [" \t\n ", "-x; $(echo) | cat", "héllo 世界"] {
            assert!(validate_pty_input_text(accepted).is_ok(), "{accepted:?}");
        }
    }

    #[test]
    fn validator_enforces_utf8_byte_boundary() {
        assert!(validate_pty_input_text(&"x".repeat(PTY_INPUT_MAX_BYTES)).is_ok());
        let error = validate_pty_input_text(&"x".repeat(PTY_INPUT_MAX_BYTES + 1)).unwrap_err();
        assert_eq!(error.kind, PtyInputTextErrorKind::TooLarge);
        assert!(validate_pty_input_text(&"é".repeat(PTY_INPUT_MAX_BYTES / 2)).is_ok());
        assert!(
            validate_pty_input_text(&format!("{}é", "x".repeat(PTY_INPUT_MAX_BYTES - 1))).is_err()
        );
    }

    #[test]
    fn validator_reports_offset_without_preview() {
        let error = validate_pty_input_text("ok\u{001b}bad").unwrap_err();
        assert_eq!(error.byte_offset, 2);
        assert_eq!(error.scalar_offset, 2);
        assert_eq!(error.code_point, Some(0x1b));
        assert!(!error.to_string().contains("bad"));
    }

    #[test]
    fn agent_clis_require_explicit_enter() {
        for shell in [
            "codex",
            "codex.exe",
            "C:\\Users\\maria\\.codex\\codex.exe",
            "/usr/local/bin/claude",
            "agy",
            "agy.exe",
            "C:\\tools\\agy.cmd",
            "antigravity",
            "agent.exe",
        ] {
            assert!(needs_explicit_enter(shell), "shell={shell:?}");
        }
    }

    #[test]
    fn plain_shells_do_not_require_explicit_enter() {
        for shell in ["bash", "powershell.exe", "cmd.exe", "agentctl", ""] {
            assert!(!needs_explicit_enter(shell), "shell={shell:?}");
        }
    }

    struct ScriptedBackend {
        outcomes: Mutex<VecDeque<Result<(), ()>>>,
        calls: Mutex<Vec<Vec<u8>>>,
    }

    impl ScriptedBackend {
        fn new(outcomes: Vec<Result<(), ()>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl crate::pty::backend::PtyBackend for ScriptedBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            _spec: crate::pty::backend::BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async { Ok(()) })
        }

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            _id: Uuid,
            data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
            self.calls.lock().unwrap().push(data.to_vec());
            match self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(())) {
                Ok(()) => Ok(()),
                Err(()) => Err(crate::errors::AppError::PtyError(
                    "scripted write failure".to_string(),
                )),
            }
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

    async fn scripted_exact_submission(
        outcomes: Vec<Result<(), ()>>,
    ) -> (AgentSubmitOutcome, Vec<Vec<u8>>) {
        let id = Uuid::new_v4();
        let backend = Arc::new(ScriptedBackend::new(outcomes));
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        manager
            .lock()
            .unwrap()
            .try_record_route(id, crate::pty::backend::SessionBackendKind::LocalProcess)
            .unwrap();
        let permit = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        let route = PtyManager::lock_route_for_write(&permit).unwrap();
        let text_succeeded = write_exact_agent_input_first(route, b"exact text");
        let outcome = submit_exact_agent_input_with_permit(&permit, text_succeeded).await;
        let calls = backend.calls.lock().unwrap().clone();
        (outcome, calls)
    }

    #[tokio::test]
    async fn a_waiting_user_write_cannot_splice_between_text_and_enters() {
        let id = Uuid::new_v4();
        let backend = Arc::new(ScriptedBackend::new(vec![Ok(()); 4]));
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        manager
            .lock()
            .unwrap()
            .try_record_route(id, crate::pty::backend::SessionBackendKind::LocalProcess)
            .unwrap();
        let privileged = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        let waiting_manager = Arc::clone(&manager);
        let user = tokio::spawn(async move {
            let permit = PtyManager::acquire_input_writer(&waiting_manager, id)
                .await
                .unwrap();
            PtyManager::write_with_permit(&permit, b"user").unwrap();
        });
        tokio::task::yield_now().await;
        let route = PtyManager::lock_route_for_write(&privileged).unwrap();
        assert!(write_exact_agent_input_first(route, b"exact text"));
        let outcome = submit_exact_agent_input_with_permit(&privileged, true).await;
        assert_eq!(
            outcome,
            AgentSubmitOutcome::Submitted {
                redundant_enter_failed: false
            }
        );
        assert!(!user.is_finished());
        drop(privileged);
        user.await.unwrap();
        assert_eq!(
            backend.calls.lock().unwrap().as_slice(),
            [
                b"exact text".to_vec(),
                b"\r".to_vec(),
                b"\r".to_vec(),
                b"user".to_vec(),
            ]
        );
    }

    #[tokio::test]
    async fn exact_submission_phase_outcomes_and_backend_calls_are_pinned() {
        let (text_failed, calls) = scripted_exact_submission(vec![Err(())]).await;
        assert_eq!(text_failed, AgentSubmitOutcome::TextWriteFailed);
        assert_eq!(calls, vec![b"exact text".to_vec()]);

        let (required_failed, calls) = scripted_exact_submission(vec![Ok(()), Err(())]).await;
        assert_eq!(required_failed, AgentSubmitOutcome::RequiredEnterFailed);
        assert_eq!(calls, vec![b"exact text".to_vec(), b"\r".to_vec()]);

        let (redundant_failed, calls) =
            scripted_exact_submission(vec![Ok(()), Ok(()), Err(())]).await;
        assert_eq!(
            redundant_failed,
            AgentSubmitOutcome::Submitted {
                redundant_enter_failed: true
            }
        );
        assert_eq!(
            calls,
            vec![b"exact text".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
        );

        let (submitted, calls) = scripted_exact_submission(vec![Ok(()), Ok(()), Ok(())]).await;
        assert_eq!(
            submitted,
            AgentSubmitOutcome::Submitted {
                redundant_enter_failed: false
            }
        );
        assert_eq!(
            calls,
            vec![b"exact text".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
        );
    }

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
            agent_turn_armed: false,
            token: Uuid::new_v4(),
            agent_kind: None,
            requested_profile: None,
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            effective_codex_home: None,
            resolved_claude_projects_dir: None,
            profile_content_hash: None,
            trusted_configured_spawn: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
            context_percent: None,
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
            assert!(
                supports_self_handoff_switch(shell),
                "Pi switch source: {shell:?}"
            );
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
            "agy-proxy",
            "agyctl",
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
            "codex-proxy.exe",
            "agy",
            "antigravity.exe",
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

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            id: Uuid,
            data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
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

    #[tokio::test]
    async fn test_injection_blocked_when_menu_guard_active() {
        let session_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = session_manager
            .read()
            .await
            .create_session(
                "claude".to_string(),
                Vec::new(),
                "C:\\test".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let id = session.id;

        let backend = Arc::new(RecordingBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(id, SessionBackendKind::LocalProcess);

        let menu_guard = Arc::new(crate::pty::menu_guard::MenuGuard::new());
        let entries = vec![crate::config::settings::BlockingMenuEntry::Valid(
            crate::config::settings::BlockingMenuConfig {
                pattern: "Do you trust".to_string(),
                notification: "trust dialog".to_string(),
                enabled: true,
                captured_against: None,
            },
        )];
        let eval = menu_guard.evaluate_logical_rows(
            id,
            &[crate::pty::watchers::frame::LogicalRow {
                text: "Do you trust the authors of this file?".to_string(),
                start: 0,
                end: 0,
            }],
            &entries,
        );
        assert!(eval.is_blocked);
        assert!(menu_guard.is_blocked(id));

        let app = tauri::test::mock_builder()
            .manage(session_manager)
            .manage(pty)
            .manage(menu_guard)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();

        let err = inject_text_into_session(app.handle(), id, "echo hello")
            .await
            .unwrap_err();

        assert!(crate::pty::menu_guard::is_menu_guard_deferred_error(&err));
        assert!(err.contains(&id.to_string()));
        assert!(backend.writes.lock().unwrap().is_empty());
    }
}
