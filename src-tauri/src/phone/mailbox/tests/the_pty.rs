use super::*;

/// #1682 - the privileged exact-agent-input plane bypasses `mark_busy`, so
/// the boundary transaction itself is the arming site for it.
#[tokio::test]
async fn the_pty_input_boundary_arms_the_session() {
    use crate::pty::backend::SessionBackendKind;
    use crate::session::profile::IdleTuning;

    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle().clone();
    let sender_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "coordinator",
        SessionStatus::Running,
        None,
    )
    .await;
    let target_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "member",
        SessionStatus::Idle,
        None,
    )
    .await;
    let paths = vec![fixture._temp.path().to_string_lossy().to_string()];
    let route = crate::config::teams::verify_pty_input_route(
        &fixture.sender_cwd,
        false,
        CANONICAL_WAKE_TO,
        &paths,
    )
    .expect("verified route");
    let target_cwd_identity =
        crate::path_identity::verify_directory(&fixture.target_cwd).expect("target identity");
    let sender_cwd_identity =
        crate::path_identity::verify_directory(&fixture.sender_cwd).expect("sender identity");
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    pty.lock()
        .unwrap()
        .record_route_with_identities(
            target_id,
            SessionBackendKind::LocalProcess,
            Some(target_cwd_identity),
            Some(route.target.replica_identity.clone()),
        )
        .expect("record target route");
    pty.lock()
        .unwrap()
        .record_route_with_identities(
            sender_id,
            SessionBackendKind::LocalProcess,
            Some(sender_cwd_identity),
            Some(route.sender.replica_identity.clone()),
        )
        .expect("record sender route");
    let authority_route =
        PtyManager::authority_route_proof(&pty, sender_id).expect("sender authority proof");
    let permit = PtyManager::acquire_input_writer(&pty, target_id)
        .await
        .expect("target permit");
    let idle = app
        .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
        .inner()
        .clone();
    idle.register_session(target_id, IdleTuning::DEFAULT);
    idle.set_pty_input_ready_for_test(target_id);
    let sessions = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let settings = app.state::<SettingsState>().inner().clone();
    fn current_recipe(session: &crate::session::session::Session, settings: &AppSettings) -> bool {
        session_has_current_pty_submission_provenance(&SessionInfo::from(session), settings)
    }

    assert!(
        !sessions
            .get_session(target_id)
            .await
            .unwrap()
            .agent_turn_armed
    );

    let boundary_guard = sessions
        .prepare_pty_input_boundary(
            target_id,
            &route.target,
            SessionBackendKind::LocalProcess,
            sender_id,
            &route.sender,
            SessionBackendKind::LocalProcess,
            &authority_route,
            &permit,
            &idle,
            &settings,
            current_recipe,
            || true,
        )
        .await
        .expect("the boundary must succeed");
    drop(boundary_guard);

    assert!(
        sessions
            .get_session(target_id)
            .await
            .unwrap()
            .agent_turn_armed
    );
}

/// #1682 - the single injection funnel behind the inter-agent wake, so one
/// arming site there covers every injection caller (round-1 blocker B1).
#[tokio::test]
async fn an_injected_text_block_arms_the_session() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let session_id = add_mailbox_session_with_shell(
        &app,
        &fixture.target_cwd,
        "pi-live",
        "pi",
        SessionStatus::Idle,
    )
    .await;
    register_mock_pty_route(&app, session_id);
    let sessions = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();

    assert!(
        !sessions
            .get_session(session_id)
            .await
            .unwrap()
            .agent_turn_armed
    );

    crate::pty::inject::inject_text_into_session(&app, session_id, "arbitrary payload")
        .await
        .unwrap();

    assert!(
        sessions
            .get_session(session_id)
            .await
            .unwrap()
            .agent_turn_armed
    );
}

#[tokio::test]
async fn final_pty_boundary_linearizes_wrapper_trust_against_concurrent_settings_mutation() {
    use crate::pty::backend::SessionBackendKind;
    use crate::pty::idle_detector::PtyInputBoundaryFailure;
    use crate::session::profile::IdleTuning;

    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle().clone();
    let sender_id = add_mailbox_session(
        &app,
        &fixture.sender_cwd,
        "coordinator",
        SessionStatus::Running,
        None,
    )
    .await;
    let target_id = add_mailbox_session(
        &app,
        &fixture.target_cwd,
        "member",
        SessionStatus::Idle,
        None,
    )
    .await;
    let paths = vec![fixture._temp.path().to_string_lossy().to_string()];
    let route = crate::config::teams::verify_pty_input_route(
        &fixture.sender_cwd,
        false,
        CANONICAL_WAKE_TO,
        &paths,
    )
    .expect("verified route");
    let target_cwd_identity =
        crate::path_identity::verify_directory(&fixture.target_cwd).expect("target identity");
    let sender_cwd_identity =
        crate::path_identity::verify_directory(&fixture.sender_cwd).expect("sender identity");
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    pty.lock()
        .unwrap()
        .record_route_with_identities(
            target_id,
            SessionBackendKind::LocalProcess,
            Some(target_cwd_identity),
            Some(route.target.replica_identity.clone()),
        )
        .expect("record target route");
    pty.lock()
        .unwrap()
        .record_route_with_identities(
            sender_id,
            SessionBackendKind::LocalProcess,
            Some(sender_cwd_identity),
            Some(route.sender.replica_identity.clone()),
        )
        .expect("record sender route");
    let authority_route =
        PtyManager::authority_route_proof(&pty, sender_id).expect("sender authority proof");
    let permit = PtyManager::acquire_input_writer(&pty, target_id)
        .await
        .expect("target permit");
    let idle = app
        .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
        .inner()
        .clone();
    idle.register_session(target_id, IdleTuning::DEFAULT);
    idle.set_pty_input_ready_for_test(target_id);
    let sessions = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let settings = app.state::<SettingsState>().inner().clone();
    fn current_recipe(session: &crate::session::session::Session, settings: &AppSettings) -> bool {
        session_has_current_pty_submission_provenance(&SessionInfo::from(session), settings)
    }
    // A wrapper whose configured provenance has just been revoked: the boundary
    // must re-validate the current recipe rather than trust a stale boolean.
    fn revoked_recipe(_: &crate::session::session::Session, _: &AppSettings) -> bool {
        false
    }

    // A concurrent settings mutation holds the write lock during the exact
    // window between validation and the first write. The no-await
    // linearization must fail closed instead of proceeding on a possibly
    // revoked wrapper.
    let settings_write = Arc::clone(&settings).write_owned().await;
    let blocked = sessions
        .prepare_pty_input_boundary(
            target_id,
            &route.target,
            SessionBackendKind::LocalProcess,
            sender_id,
            &route.sender,
            SessionBackendKind::LocalProcess,
            &authority_route,
            &permit,
            &idle,
            &settings,
            current_recipe,
            || true,
        )
        .await;
    assert!(
        matches!(&blocked, Err(PtyInputBoundaryFailure::RouteUnavailable)),
        "a settings write in flight must fail the boundary closed"
    );
    drop(blocked);
    drop(settings_write);

    // With no write in flight but a revoked current recipe, the boundary
    // rejects rather than accepting the stale wrapper trust.
    let stale = sessions
        .prepare_pty_input_boundary(
            target_id,
            &route.target,
            SessionBackendKind::LocalProcess,
            sender_id,
            &route.sender,
            SessionBackendKind::LocalProcess,
            &authority_route,
            &permit,
            &idle,
            &settings,
            revoked_recipe,
            || true,
        )
        .await;
    assert!(
        matches!(&stale, Err(PtyInputBoundaryFailure::Busy)),
        "a wrapper whose current recipe no longer holds must be rejected"
    );
    drop(stale);

    // Isolation control: neither rejection above mutated session state, so the
    // otherwise-valid boundary still succeeds once the race conditions clear.
    let ok = sessions
        .prepare_pty_input_boundary(
            target_id,
            &route.target,
            SessionBackendKind::LocalProcess,
            sender_id,
            &route.sender,
            SessionBackendKind::LocalProcess,
            &authority_route,
            &permit,
            &idle,
            &settings,
            current_recipe,
            || true,
        )
        .await;
    assert!(
        ok.is_ok(),
        "the boundary must otherwise succeed, isolating the two race rejections"
    );
    drop(ok);
}

#[test]
fn maximum_host_request_can_be_atomically_replaced_with_a_marker() {
    let temp = tempfile::tempdir().unwrap();
    let injection_id = Uuid::new_v4().to_string();
    let path = temp.path().join(format!("{injection_id}.json"));
    std::fs::write(&path, vec![b'x'; crate::pty::backend::PTY_INPUT_MAX_BYTES]).unwrap();
    let source_identity = crate::path_identity::read_bounded_regular(
        &path,
        crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
    )
    .unwrap()
    .1;
    MailboxPoller::default()
        .replace_host_request_with_marker(&path, &source_identity, &injection_id, &injection_id)
        .unwrap();
    let (bytes, _) = crate::path_identity::read_bounded_regular(
        &path,
        crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES,
    )
    .unwrap();
    let marker: crate::phone::types::PtyInputQueueMarker = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(marker.injection_id, injection_id);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        serde_json::json!({
            "kind": "pty-input-marker",
            "version": 1,
            "injectionId": injection_id,
            "opId": marker.op_id,
        })
    );
}

#[tokio::test]
async fn unavailable_store_rejects_a_valid_host_request_with_its_correlated_id() {
    use crate::phone::types::{
        canonical_pty_timestamp, PtyInputEnterMode, PtyInputHostEnvelope, PtyInputPublicStatus,
        PtyInputReasonCode, PtyInputWirePayload,
    };

    let fixture = make_mailbox_fixture();
    let outbox = fixture
        .sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let injection_id = Uuid::new_v4().to_string();
    let issued = chrono::Utc::now();
    let issued_at = canonical_pty_timestamp(issued);
    let envelope = PtyInputHostEnvelope {
        id: injection_id.clone(),
        token: Uuid::new_v4().to_string(),
        from: CANONICAL_WAKE_FROM.to_string(),
        to: CANONICAL_WAKE_TO.to_string(),
        body: String::new(),
        mode: "wake".to_string(),
        get_output: false,
        preferred_agent: String::new(),
        priority: "normal".to_string(),
        timestamp: issued_at.clone(),
        action: "pty-input".to_string(),
        pty_input: PtyInputWirePayload {
            version: crate::phone::types::PTY_INPUT_VERSION,
            text: "correlated store rejection".to_string(),
            enter: PtyInputEnterMode::AgentSubmit,
            injection_id: injection_id.clone(),
            op_id: injection_id.clone(),
            issued_at,
            expires_at: canonical_pty_timestamp(issued + chrono::Duration::minutes(10)),
            nonce: Uuid::new_v4().to_string(),
            agent_id: None,
        },
    };
    let path = outbox.join(format!("{injection_id}.json"));
    let bytes = serde_json::to_vec(&envelope).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let source_identity = crate::path_identity::read_bounded_regular(
        &path,
        crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
    )
    .unwrap()
    .1;

    MailboxPoller::default()
        .process_pty_input_file(fixture.app.handle(), &path, false, &bytes, &source_identity)
        .await;

    assert!(!path.exists());
    let artifact_path = outbox.join("rejected").join(format!("{injection_id}.json"));
    let artifact: crate::phone::types::PtyInputHostArtifact =
        serde_json::from_slice(&std::fs::read(artifact_path).unwrap()).unwrap();
    assert_eq!(artifact.result.injection_id, injection_id);
    assert_eq!(artifact.result.status, PtyInputPublicStatus::Rejected);
    assert_eq!(
        artifact.result.reason.map(|reason| reason.code),
        Some(PtyInputReasonCode::StoreCorrupt)
    );
    assert_eq!(artifact.confirmation_tag.len(), 64);
}

#[tokio::test]
async fn live_sender_token_host_request_authenticates_and_injects_end_to_end() {
    use crate::phone::types::{
        canonical_pty_timestamp, PtyInputEnterMode, PtyInputHostArtifact, PtyInputHostEnvelope,
        PtyInputPublicStatus, PtyInputWirePayload,
    };

    // The real authenticated host path: a live sender session whose actual
    // session token bears the request, a managed ready store, and an
    // idle-ready target. This proves ingress authenticates through
    // find_unique_live_by_token and drives the engine to a real injection,
    // rather than a random token against an unavailable store.
    let fixture = make_mailbox_fixture();
    let app = fixture.app.handle().clone();
    let (_route, sender_id, target_id) =
        seed_shared_pty_engine_sessions(&fixture, SessionStatus::Idle).await;
    let store = Arc::new(
        crate::api::message_store::MessageStore::open(
            fixture._temp.path().join("authenticated-host.sqlite3"),
        )
        .unwrap(),
    );
    app.manage(crate::api::message_store::MessageStoreState::ready(
        Arc::clone(&store),
    ));

    // Read the live sender session's real token; the daemon authenticates the
    // request only if it round-trips to a unique live session.
    let sender_token = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .list_sessions()
        .await
        .into_iter()
        .find(|session| session.id == sender_id.to_string())
        .expect("live sender session")
        .token;

    let outbox = fixture
        .sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let injection_id = Uuid::new_v4().to_string();
    let issued = chrono::Utc::now();
    let issued_at = canonical_pty_timestamp(issued);
    let text = "authenticated host injection";
    let envelope = PtyInputHostEnvelope {
        id: injection_id.clone(),
        token: sender_token,
        from: CANONICAL_WAKE_FROM.to_string(),
        to: CANONICAL_WAKE_TO.to_string(),
        body: String::new(),
        mode: "wake".to_string(),
        get_output: false,
        preferred_agent: String::new(),
        priority: "normal".to_string(),
        timestamp: issued_at.clone(),
        action: "pty-input".to_string(),
        pty_input: PtyInputWirePayload {
            version: crate::phone::types::PTY_INPUT_VERSION,
            text: text.to_string(),
            enter: PtyInputEnterMode::AgentSubmit,
            injection_id: injection_id.clone(),
            op_id: injection_id.clone(),
            issued_at,
            expires_at: canonical_pty_timestamp(issued + chrono::Duration::minutes(10)),
            nonce: Uuid::new_v4().to_string(),
            agent_id: None,
        },
    };
    let path = outbox.join(format!("{injection_id}.json"));
    let bytes = serde_json::to_vec(&envelope).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let source_identity = crate::path_identity::read_bounded_regular(
        &path,
        crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
    )
    .unwrap()
    .1;

    MailboxPoller::default()
        .process_pty_input_file(fixture.app.handle(), &path, false, &bytes, &source_identity)
        .await;

    // The raw request was consumed and a terminal injected artifact published.
    assert!(!path.exists());
    let delivered = outbox
        .join("delivered")
        .join(format!("{injection_id}.json"));
    let artifact: PtyInputHostArtifact =
        serde_json::from_slice(&std::fs::read(&delivered).unwrap()).unwrap();
    assert_eq!(artifact.result.injection_id, injection_id);
    assert_eq!(artifact.result.status, PtyInputPublicStatus::Injected);

    // The store row and the target backend both reflect the exact injection.
    let row = store
        .query_pty_input_by_injection(&injection_id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, PtyInputPublicStatus::Injected);
    assert_eq!(
        shared_engine_writes(&app, target_id),
        vec![text.as_bytes().to_vec(), b"\r".to_vec(), b"\r".to_vec()]
    );
}

#[tokio::test]
async fn host_terminal_artifact_is_source_correlated_and_idempotently_repairable() {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        let outer = tempfile::TempDir::new().unwrap();
        let mut parent = crate::path_utils::normalize_windows_verbatim_path_buf(
            &std::fs::canonicalize(outer.path()).unwrap(),
        );
        let suffix = PathBuf::from(".tmpXXXXXX/proj-a/.ac/wg-1-dev-team/__agent_tech-lead")
            .join(crate::config::agent_local_dir_name())
            .join("outbox/rejected")
            .join(format!("{}.json", Uuid::nil()));
        while parent.join(&suffix).as_os_str().encode_wide().count() <= 260 {
            let remaining = 261 - parent.join(&suffix).as_os_str().encode_wide().count();
            parent = parent.join("p".repeat(remaining.saturating_sub(1).clamp(1, 60)));
        }
        std::fs::create_dir_all(&parent).unwrap();
        let fixture = make_mailbox_fixture_in(tempfile::TempDir::new_in(parent).unwrap());
        assert_host_terminal_artifact_repair(&fixture, outer.path(), true).await;
    }
    let fixture = make_mailbox_fixture();
    assert_host_terminal_artifact_repair(&fixture, fixture._temp.path(), false).await;
}

async fn assert_host_terminal_artifact_repair(
    fixture: &MailboxFixture,
    database_root: &Path,
    expect_long_path: bool,
) {
    use crate::phone::types::{
        canonical_pty_timestamp, PtyInputPublicStatus, PtyInputReasonCode, PtyInputSourcePlane,
    };

    let outbox = fixture
        .sender_cwd
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let paths = vec![fixture._temp.path().to_string_lossy().to_string()];
    let route = crate::config::teams::verify_pty_input_route(
        &fixture.sender_cwd,
        false,
        CANONICAL_WAKE_TO,
        &paths,
    )
    .unwrap();
    let store =
        crate::api::message_store::MessageStore::open(database_root.join("host-artifact.sqlite3"))
            .unwrap();
    let injection_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let request_fingerprint = "a".repeat(64);
    let confirmation_tag = "b".repeat(64);
    store
        .enqueue_pty_input(crate::api::message_store::PtyInputEnqueueRequest {
            injection_id: injection_id.clone(),
            sender_fqn: route.sender.canonical_fqn,
            target_fqn: route.target.canonical_fqn,
            op_id: injection_id.clone(),
            nonce_sha256: "c".repeat(64),
            request_fingerprint: request_fingerprint.clone(),
            confirmation_tag: Some(confirmation_tag.clone()),
            requested_agent_id: None,
            payload: b"artifact exact text".to_vec(),
            source_plane: PtyInputSourcePlane::HostCli,
            sender_incarnation_fingerprint: route.sender.incarnation_fingerprint,
            sender_identity_fingerprint: route.sender.authority_fingerprint,
            target_identity_fingerprint: route.target.authority_fingerprint,
            authority_session_id: Uuid::new_v4().to_string(),
            authority_client_id: None,
            authority_client_generation: None,
            issued_at: canonical_pty_timestamp(now),
            expires_at: canonical_pty_timestamp(now + chrono::Duration::minutes(10)),
        })
        .unwrap();
    store
        .terminalize_pty_input(
            &injection_id,
            PtyInputPublicStatus::Rejected,
            Some(PtyInputReasonCode::Busy),
            now + chrono::Duration::seconds(1),
        )
        .unwrap();

    let poller = MailboxPoller::default();
    let marker_path = outbox.join(format!("{injection_id}.json"));
    let artifact_path = outbox.join("rejected").join(format!("{injection_id}.json"));
    #[cfg(windows)]
    let marker_setup_path = std::fs::canonicalize(marker_path.parent().unwrap())
        .unwrap()
        .join(marker_path.file_name().unwrap());
    #[cfg(not(windows))]
    let marker_setup_path = marker_path.clone();
    #[cfg(windows)]
    if expect_long_path {
        use std::os::windows::ffi::OsStrExt;
        assert!(!marker_path.to_string_lossy().starts_with(r"\\?\"));
        assert!(!artifact_path.to_string_lossy().starts_with(r"\\?\"));
        assert!(artifact_path.as_os_str().encode_wide().count() > 260);
        eprintln!(
            "marker={}, setup={}, artifact={} UTF-16 units",
            marker_path.as_os_str().encode_wide().count(),
            marker_setup_path.as_os_str().encode_wide().count(),
            artifact_path.as_os_str().encode_wide().count()
        );
    }
    std::fs::write(&marker_path, b"source envelope").unwrap();
    let source_identity = crate::path_identity::read_bounded_regular(
        &marker_path,
        crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
    )
    .unwrap()
    .1;
    poller
        .replace_host_request_with_marker(
            &marker_setup_path,
            &source_identity,
            &injection_id,
            &injection_id,
        )
        .unwrap();
    poller
        .materialize_host_terminal_artifact(&marker_path, &store, &injection_id)
        .await
        .unwrap();
    assert!(!marker_path.exists());
    eprintln!("first materialization completed; marker removed");
    #[cfg(windows)]
    if expect_long_path {
        use std::os::windows::ffi::OsStrExt;
        let length = artifact_path.as_os_str().encode_wide().count();
        eprintln!("forced-long ordinary artifact UTF-16 length: {length}");
        assert!(length > 260, "artifact must exceed MAX_PATH");
    }
    #[cfg(not(windows))]
    let _ = expect_long_path;
    let artifact: crate::phone::types::PtyInputHostArtifact = serde_json::from_slice(
        &crate::path_identity::read_bounded_regular(
            &artifact_path,
            crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES,
        )
        .unwrap()
        .0,
    )
    .unwrap();
    assert_eq!(artifact.confirmation_tag, confirmation_tag);
    assert_eq!(artifact.result.injection_id, injection_id);

    // Simulate a crash after artifact publication but before marker cleanup.
    std::fs::write(&marker_path, b"retained source envelope").unwrap();
    let source_identity = crate::path_identity::read_bounded_regular(
        &marker_path,
        crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
    )
    .unwrap()
    .1;
    poller
        .replace_host_request_with_marker(
            &marker_setup_path,
            &source_identity,
            &injection_id,
            &injection_id,
        )
        .unwrap();
    #[cfg(windows)]
    if expect_long_path {
        use std::os::windows::ffi::OsStrExt;
        assert!(artifact_path.as_os_str().encode_wide().count() > 260);
    }
    eprintln!("second marker preparation completed; starting repair");
    poller
        .materialize_host_terminal_artifact(&marker_path, &store, &injection_id)
        .await
        .unwrap();
    assert!(!marker_path.exists());

    let repaired: crate::phone::types::PtyInputHostArtifact = serde_json::from_slice(
        &crate::path_identity::read_bounded_regular(
            &artifact_path,
            crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES,
        )
        .unwrap()
        .0,
    )
    .unwrap();
    assert_eq!(repaired.result.injection_id, injection_id);
    assert_eq!(repaired.confirmation_tag, confirmation_tag);

    std::fs::write(&marker_path, b"tampered source envelope").unwrap();
    let source_identity = crate::path_identity::read_bounded_regular(
        &marker_path,
        crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
    )
    .unwrap()
    .1;
    poller
        .replace_host_request_with_marker(
            &marker_setup_path,
            &source_identity,
            &injection_id,
            &injection_id,
        )
        .unwrap();
    let mut marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&marker_path).unwrap()).unwrap();
    marker["unexpectedField"] = serde_json::Value::String("tampered".to_string());
    std::fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
    assert!(poller
        .materialize_host_terminal_artifact(&marker_path, &store, &injection_id)
        .await
        .is_err());
    assert!(marker_path.exists());
}
