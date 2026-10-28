use super::*;

#[test]
fn request_file_parses_ready_and_inflight_names() {
    let id = Uuid::new_v4().to_string();
    let ready = RequestFile::from_path(Path::new(&format!("{id}.json"))).unwrap();
    assert_eq!(ready.request_id, id);
    matches!(ready.kind, RequestFileKind::Ready);

    let inflight = RequestFile::from_path(Path::new(&format!("{id}.inflight.json"))).unwrap();
    assert_eq!(inflight.request_id, id);
    matches!(inflight.kind, RequestFileKind::Inflight);

    assert!(RequestFile::from_path(Path::new("not-a-uuid.json")).is_none());
    assert!(RequestFile::from_path(Path::new(&format!("{id}.tmp"))).is_none());
}

#[test]
fn request_expiry_uses_optional_deadline() {
    let mut request = sample_request(Uuid::new_v4().to_string(), "target");
    request.expires_at_unix_ms = None;
    assert!(!request_expired(&request, 100));

    request.expires_at_unix_ms = Some(99);
    assert!(request_expired(&request, 100));

    request.expires_at_unix_ms = Some(101);
    assert!(!request_expired(&request, 100));
}

#[test]
fn complete_rejects_unknown_request_id() {
    let tmp = tempfile::tempdir().unwrap();
    let state = UiAutomationState::new(true, tmp.path().to_path_buf());
    let response = UiAutomationResponse::minimal_error(
        &Uuid::new_v4().to_string(),
        "main",
        UiAutomationAction::Query,
        "target",
        "frontend_error",
        "frontend failed",
    );

    assert_eq!(
        state.complete("main", response).unwrap_err(),
        "unknown_request_id"
    );
}

#[test]
fn frontend_ready_registers_dynamic_caller_label() {
    let tmp = tempfile::tempdir().unwrap();
    let state = UiAutomationState::new(true, tmp.path().to_path_buf());
    state.initialize_files().unwrap();

    state
        .mark_frontend_ready("resource-monitor", Some("resource-monitor"))
        .unwrap();

    let raw = fs::read_to_string(&state.inner.session_path).unwrap();
    let session: UiAutomationSession = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        session.window_labels,
        vec!["main".to_string(), "resource-monitor".to_string()]
    );
    assert_eq!(
        session.ready_window_labels,
        vec!["resource-monitor".to_string()]
    );
}

#[test]
fn live_window_sync_prunes_closed_dynamic_windows() {
    let tmp = tempfile::tempdir().unwrap();
    let state = UiAutomationState::new(true, tmp.path().to_path_buf());
    state.initialize_files().unwrap();
    state
        .mark_frontend_ready("resource-monitor", Some("resource-monitor"))
        .unwrap();

    state
        .sync_live_window_labels(vec!["main".to_string()])
        .unwrap();

    let raw = fs::read_to_string(&state.inner.session_path).unwrap();
    let session: UiAutomationSession = serde_json::from_str(&raw).unwrap();
    assert_eq!(session.window_labels, vec!["main".to_string()]);
    assert!(session.ready_window_labels.is_empty());
}

#[test]
fn complete_writes_completion_mismatch_response() {
    let tmp = tempfile::tempdir().unwrap();
    let state = UiAutomationState::new(true, tmp.path().to_path_buf());
    fs::create_dir_all(state.requests_dir()).unwrap();
    fs::create_dir_all(state.responses_dir()).unwrap();

    let request_id = Uuid::new_v4().to_string();
    let request = sample_request(request_id.clone(), "expected");
    let response_path = state.response_path(&request_id);
    let inflight_path = state.inflight_path(&request_id);
    fs::write(&inflight_path, "{}").unwrap();
    state.inner.pending.lock().unwrap().insert(
        request_id.clone(),
        PendingRequest {
            request,
            response_path: response_path.clone(),
            inflight_path: inflight_path.clone(),
        },
    );

    let result = UiAutomationResponse {
        ok: true,
        request_id,
        window: "main".to_string(),
        action: UiAutomationAction::Query,
        selector: "different".to_string(),
        target: Some(json!({ "testId": "different" })),
        error: None,
        message: None,
        available: None,
        diagnostics: None,
        available_windows: None,
        timeout_ms: None,
        phase: None,
    };

    state.complete("main", result).unwrap();

    let raw = fs::read_to_string(response_path).unwrap();
    let written: UiAutomationResponse = serde_json::from_str(&raw).unwrap();
    assert!(!written.ok);
    assert_eq!(written.error.as_deref(), Some("completion_mismatch"));
    assert!(!inflight_path.exists());
}

#[test]
fn expire_pending_requests_writes_timeout_response() {
    let tmp = tempfile::tempdir().unwrap();
    let state = UiAutomationState::new(true, tmp.path().to_path_buf());
    fs::create_dir_all(state.requests_dir()).unwrap();
    fs::create_dir_all(state.responses_dir()).unwrap();

    let request_id = Uuid::new_v4().to_string();
    let mut request = sample_request(request_id.clone(), "missing.target");
    request.expires_at_unix_ms = Some(now_unix_ms() - 1);
    let response_path = state.response_path(&request_id);
    let inflight_path = state.inflight_path(&request_id);
    fs::write(&inflight_path, "{}").unwrap();
    state.inner.pending.lock().unwrap().insert(
        request_id,
        PendingRequest {
            request,
            response_path: response_path.clone(),
            inflight_path: inflight_path.clone(),
        },
    );

    state.expire_pending_requests();

    let raw = fs::read_to_string(response_path).unwrap();
    let written: UiAutomationResponse = serde_json::from_str(&raw).unwrap();
    assert!(!written.ok);
    assert_eq!(written.error.as_deref(), Some("timeout"));
    assert!(!inflight_path.exists());
    assert!(state.inner.pending.lock().unwrap().is_empty());
}

#[test]
fn session_read_retries_transient_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(SESSION_FILE);
    let writer_path = path.clone();
    let count = std::rc::Rc::new(std::cell::Cell::new(0));
    let observed = count.clone();
    let guard = SessionReadHookGuard::install(path.clone(), move || {
        observed.set(observed.get() + 1);
        write_json_atomic_new(&writer_path, &serde_json::json!({"ok": true})).unwrap_or_else(
            |error| panic!("publish session fixture {}: {error}", writer_path.display()),
        );
    });

    let result = read_session_file_with_retry(&path);
    assert!(
        result.is_ok(),
        "session reader must recover after observed NotFound: {result:?}"
    );
    assert_eq!(result.unwrap(), "{\"ok\":true}");
    assert_eq!(count.get(), 1);
    assert!(!path.with_extension("tmp").exists());
    drop(guard);
    tmp.close().expect("remove transient session fixture");
}

#[test]
fn session_read_not_found_exhausts_attempts() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(SESSION_FILE);
    let error = read_session_file_with_retry(&path).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    tmp.close().expect("remove absent session fixture");
}

#[test]
fn session_read_invalid_utf8_is_not_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(SESSION_FILE);
    fs::write(&path, [0xff]).unwrap();
    let count = std::rc::Rc::new(std::cell::Cell::new(0));
    let observed = count.clone();
    let guard = SessionReadHookGuard::install(path.clone(), move || {
        observed.set(observed.get() + 1);
    });
    let error = read_session_file_with_retry(&path).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(count.get(), 0);
    drop(guard);
    tmp.close().expect("remove invalid UTF-8 session fixture");
}

#[test]
fn session_read_empty_success_is_not_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(SESSION_FILE);
    fs::write(&path, "").unwrap();
    let count = std::rc::Rc::new(std::cell::Cell::new(0));
    let observed = count.clone();
    let guard = SessionReadHookGuard::install(path.clone(), move || {
        observed.set(observed.get() + 1);
    });
    assert_eq!(read_session_file_with_retry(&path).unwrap(), "");
    assert_eq!(count.get(), 0);
    drop(guard);
    tmp.close().expect("remove empty session fixture");
}

#[test]
fn initialization_failure_can_disable_state() {
    let tmp = tempfile::tempdir().unwrap();
    let state = UiAutomationState::new(true, tmp.path().to_path_buf());
    fs::create_dir_all(&state.inner.automation_dir).unwrap();
    fs::write(state.requests_dir(), "not a directory").unwrap();

    assert!(state.initialize_files().is_err());
    state.mark_unavailable();
    assert!(!state.enabled());
}

#[test]
fn cleanup_stale_automation_files_removes_request_and_response_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let automation_dir = tmp.path().join(UI_AUTOMATION_DIR);
    let requests_dir = automation_dir.join(REQUESTS_DIR);
    let responses_dir = automation_dir.join(RESPONSES_DIR);
    fs::create_dir_all(&requests_dir).unwrap();
    fs::create_dir_all(&responses_dir).unwrap();
    fs::write(requests_dir.join("stale.json"), "{}").unwrap();
    fs::write(responses_dir.join("stale.json"), "{}").unwrap();

    cleanup_stale_automation_files(&automation_dir).unwrap();

    assert!(!requests_dir.exists());
    assert!(!responses_dir.exists());
}

#[test]
fn timeout_phase_uses_ready_window_labels_for_inflight() {
    let tmp = tempfile::tempdir().unwrap();
    let request_path = tmp.path().join("request.json");
    let inflight_path = tmp.path().join("request.inflight.json");
    let session_path = tmp.path().join(SESSION_FILE);
    fs::write(&request_path, "{}").unwrap();
    assert_eq!(
        timeout_phase(&request_path, &inflight_path, &session_path, "main"),
        "awaiting_gui_poller"
    );

    fs::remove_file(&request_path).unwrap();
    fs::write(&inflight_path, "{}").unwrap();
    fs::write(
        &session_path,
        serde_json::to_string(&UiAutomationSession {
            pid: std::process::id(),
            token: "token".to_string(),
            exe_path: current_exe_path_string(),
            config_dir: tmp.path().to_string_lossy().into_owned(),
            window_labels: vec!["main".to_string()],
            ready_window_labels: vec![],
            started_at_unix_ms: 1,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        timeout_phase(&request_path, &inflight_path, &session_path, "main"),
        "awaiting_frontend_ready"
    );

    fs::write(
        &session_path,
        serde_json::to_string(&UiAutomationSession {
            pid: std::process::id(),
            token: "token".to_string(),
            exe_path: current_exe_path_string(),
            config_dir: tmp.path().to_string_lossy().into_owned(),
            window_labels: vec!["main".to_string()],
            ready_window_labels: vec!["main".to_string()],
            started_at_unix_ms: 1,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        timeout_phase(&request_path, &inflight_path, &session_path, "main"),
        "awaiting_frontend_response"
    );
}

#[test]
fn issue_3355_atomic_new_stage_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("request.json");
    let stage = path.with_extension("tmp");
    fs::write(&stage, "stage sentinel").unwrap();
    let error = write_json_atomic_new(&path, &json!({"ok": true})).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&stage).unwrap(), "stage sentinel");
    assert!(!path.exists());
    fs::write(&path, "destination sentinel").unwrap();
    let error = write_json_atomic_new(&path, &json!({"ok": true})).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(
        error.to_string(),
        format!("destination already exists: {}", path.display())
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "destination sentinel");
    assert_eq!(fs::read_to_string(stage).unwrap(), "stage sentinel");
}

#[test]
fn issue_3355_atomic_new_and_replace_preserve_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("request.json");
    let value = json!({"text": "unicode λ", "ok": true});
    write_json_atomic_new(&path, &value).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        serde_json::to_vec(&value).unwrap()
    );
    assert!(!path.with_extension("tmp").exists());
    fs::write(
        path.with_extension("tmp"),
        "stale stage longer than replacement",
    )
    .unwrap();
    let replacement = json!({"n": 1});
    super::super::files::write_json_atomic_replace(&path, &replacement).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        serde_json::to_vec(&replacement).unwrap()
    );
    assert!(!path.with_extension("tmp").exists());
}

#[test]
fn issue_3355_atomic_serialize_error_keeps_stage() {
    struct Failing;
    impl serde::Serialize for Failing {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional serialize failure"))
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("request.json");
    fs::write(&path, "original").unwrap();
    let error = super::super::files::write_json_atomic_replace(&path, &Failing).unwrap_err();
    assert!(error.to_string().contains("intentional serialize failure"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    assert_eq!(
        fs::read(path.with_extension("tmp")).unwrap(),
        Vec::<u8>::new()
    );
}

#[test]
fn issue_3355_retry_contract() {
    use super::super::files::retry_fs;
    let mut attempts = 0;
    retry_fs(|| {
        attempts += 1;
        if attempts == 8 {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "transient"))
        }
    })
    .unwrap();
    assert_eq!(attempts, 8);
    attempts = 0;
    let error = retry_fs(|| {
        attempts += 1;
        Err(io::Error::new(io::ErrorKind::InvalidInput, "permanent"))
    })
    .unwrap_err();
    assert_eq!(attempts, 1);
    assert_eq!(error.to_string(), "permanent");
    attempts = 0;
    let error = retry_fs(|| {
        attempts += 1;
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("attempt{attempts}"),
        ))
    })
    .unwrap_err();
    assert_eq!(attempts, 8);
    assert_eq!(error.to_string(), "attempt8");
}

#[test]
fn issue_3355_automation_ports_preserve_native_results() {
    use crate::filesystem::{directories as dirs, documents as docs};
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("text");
    let error = |e: io::Error| (e.kind(), e.raw_os_error(), e.to_string());
    for text in ["", " \u{feff}λ\n", "plain"] {
        fs::write(&path, text).unwrap();
        assert_eq!(
            docs::read_ui_automation_text(&path).unwrap(),
            fs::read_to_string(&path).unwrap()
        );
    }
    fs::write(&path, [0xff]).unwrap();
    for bad in [
        path.clone(),
        tmp.path().join("missing"),
        tmp.path().to_path_buf(),
    ] {
        assert_eq!(
            error(docs::read_ui_automation_text(&bad).unwrap_err()),
            error(fs::read_to_string(&bad).unwrap_err())
        );
    }
    let directory = tmp.path().join("nested").join("directory");
    dirs::ensure_ui_automation_directory(&directory).unwrap();
    assert!(directory.is_dir());
    for p in [&directory, &path, &tmp.path().join("missing")] {
        assert_eq!(dirs::ui_automation_path_exists(p), p.exists());
    }
    assert_eq!(
        error(dirs::ensure_ui_automation_directory(&path).unwrap_err()),
        error(fs::create_dir_all(&path).unwrap_err())
    );
    let absent = tmp.path().join("absent");
    let destination = tmp.path().join("renamed");
    assert_eq!(
        error(docs::rename_ui_automation_file(&absent, &destination).unwrap_err()),
        error(fs::rename(&absent, &destination).unwrap_err())
    );
    assert_eq!(
        error(docs::remove_ui_automation_file(&absent).unwrap_err()),
        error(fs::remove_file(&absent).unwrap_err())
    );
    super::super::files::retry_remove_file(&absent).unwrap();
    super::super::files::retry_remove_dir_all(&absent).unwrap();
    docs::rename_ui_automation_file(&path, &destination).unwrap();
    assert!(!path.exists());
    assert_eq!(fs::read(&destination).unwrap(), vec![0xff]);
    docs::remove_ui_automation_file(&destination).unwrap();
    assert!(!destination.exists());
    let resolved = directory.canonicalize().unwrap();
    assert!(resolved.starts_with(tmp.path().canonicalize().unwrap()));
    dirs::remove_ui_automation_directory(&resolved).unwrap();
    assert!(!directory.exists());
}
