use std::fmt;
use std::sync::{Arc, Mutex};

use tauri::Manager;
use uuid::Uuid;

use crate::pty::backend::PTY_INPUT_MAX_BYTES;
use crate::pty::manager::{PtyInputPermit, PtyManager, PtyRouteWriteGuard};
use crate::session::manager::SessionManager;

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
        let forbidden = matches!(
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
        );
        if forbidden {
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

/// Returns true when the legacy shell command requires explicit Enter.
pub(crate) fn needs_explicit_enter(shell: &str) -> bool {
    let normalized = shell.trim().replace('\\', "/");
    let token = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    let stem = token
        .strip_suffix(".exe")
        .or_else(|| token.strip_suffix(".cmd"))
        .unwrap_or(token)
        .to_ascii_lowercase();
    stem.starts_with("codex")
        || stem.starts_with("claude")
        || stem.starts_with("gemini")
        || stem == "agent"
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

/// Inject a legacy text block. The public behavior remains text followed by
/// staggered Enter writes for known coding-agent shells.
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
    let shell = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let result = mgr.get_shell(session_id).await;
        drop(mgr);
        result
    };
    let send_enter = shell.as_deref().map(needs_explicit_enter).unwrap_or(false);

    let pty_manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let permit = PtyManager::acquire_input_writer(&pty_manager, session_id)
        .await
        .map_err(|error| error.to_string())?;

    // The stale-delivery closure runs at the serialized first-write boundary.
    pre_write_check()?;
    PtyManager::write_with_permit(&permit, text.as_bytes()).map_err(|error| error.to_string())?;
    crate::commands::pty::mark_successful_pty_write_busy(app, session_id, text.len()).await;

    if send_enter {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        PtyManager::write_with_permit(&permit, b"\r").map_err(|error| error.to_string())?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if PtyManager::write_with_permit(&permit, b"\r").is_err() {
            log::warn!(
                "[inject] redundant Enter failed session={} code=redundant_enter_failed",
                session_id
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
            "gemini.cmd",
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

        fn write(&self, _id: Uuid, data: &[u8]) -> Result<(), crate::errors::AppError> {
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
}
