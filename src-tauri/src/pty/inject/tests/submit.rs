use super::*;

#[derive(serde::Deserialize)]
struct Fixture {
    name: String,
    text: String,
    valid: bool,
}

#[test]
fn shared_validation_fixture_is_authoritative() {
    let rows: Vec<Fixture> = serde_json::from_str(include_str!(
        "../../../../../crates/session-bridge/tests/fixtures/pty_input_validation.json"
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
    assert!(validate_pty_input_text(&format!("{}é", "x".repeat(PTY_INPUT_MAX_BYTES - 1))).is_err());
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
        "hermes",
        "hermes.exe",
        "hermes.cmd",
        "hermes.ps1",
        "HERMES",
        "  hermes  ",
        "/usr/local/bin/hermes",
        "opencode",
        "opencode.exe",
        "opencode.cmd",
        "opencode.ps1",
        "OPENCODE",
        "  opencode  ",
        "/usr/local/bin/opencode",
        "grok",
        "grok.exe",
        "grok.cmd",
        "grok.ps1",
        "GROK",
        "  grok  ",
        "/usr/local/bin/grok",
    ] {
        assert!(needs_explicit_enter(shell), "shell={shell:?}");
    }
}

#[cfg(windows)]
#[test]
fn explicit_submit_windows_native_paths() {
    // `shell_file_stem` relies on `std::path::Path::file_stem`, which does
    // not treat `\` as a separator off Windows, so these native path shapes
    // are asserted only where that behaviour holds.
    for shell in [
        r"C:\Tools\hermes.exe",
        r"\\server\share\hermes.cmd",
        r"\\?\C:\Tools\hermes.exe",
        r"C:\Tools\opencode.exe",
        r"\\server\share\opencode.cmd",
        r"\\?\C:\Tools\opencode.exe",
        r"C:\Tools\grok.exe",
        r"\\server\share\grok.cmd",
        r"\\?\C:\Tools\grok.exe",
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

    let (redundant_failed, calls) = scripted_exact_submission(vec![Ok(()), Ok(()), Err(())]).await;
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
