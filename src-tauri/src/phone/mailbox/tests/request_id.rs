use super::*;
use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use crate::pty::manager::PtyManager;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

const V4: &str = "550e8400-e29b-41d4-a716-446655440000";
const V1: &str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
const ACTIONS: [Option<&str>; 8] = [
    Some("self-handoff-and-clear"),
    Some("self-clear"),
    Some("self-handoff-and-switch"),
    Some("self-handoff-and-restart"),
    Some("raise-hand"),
    Some("purge-wg"),
    Some("close-session"),
    None,
];

// Observe the actual backend boundary, including watcher registration, rather than
// relying on the existing mock's no-op watcher implementation.
#[derive(Default)]
struct CorrelationBackend {
    inner: MailboxMockPtyBackend,
    watchers: Mutex<Vec<(Uuid, String, PathBuf)>>,
}

impl PtyBackend for CorrelationBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        self.inner.spawn(spec)
    }

    fn write(
        &self,
        authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        data: &[u8],
    ) -> Result<(), AppError> {
        self.inner.write(authority, id, data)
    }

    fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
        self.inner.resize(id, cols, rows)
    }

    fn kill(&self, id: Uuid) -> Result<(), AppError> {
        self.inner.kill(id)
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.inner.has_session(id)
    }

    fn get_screen_snapshot(&self, id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        self.inner.get_screen_snapshot(id)
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.inner.get_pty_size(id)
    }

    fn get_screen_rows(&self, id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        self.inner.get_screen_rows(id)
    }

    fn register_response_watcher(&self, id: Uuid, request_id: String, response_dir: PathBuf) {
        self.watchers
            .lock()
            .unwrap()
            .push((id, request_id, response_dir));
    }

    fn terminate_job_for_session(&self, id: Uuid) -> bool {
        self.inner.terminate_job_for_session(id)
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        self.inner.kill_all_jobs()
    }
}

fn observe_backend(fixture: &MailboxFixture) -> Arc<CorrelationBackend> {
    let backend = Arc::new(CorrelationBackend::default());
    let manager = fixture.app.state::<Arc<Mutex<PtyManager>>>();
    *manager.lock().unwrap() = PtyManager::new_for_test(backend.clone());
    backend
}

async fn scan_sender(fixture: &MailboxFixture) {
    fixture
        .app
        .state::<SettingsState>()
        .write()
        .await
        .project_paths
        .push(fixture.sender_cwd.to_string_lossy().to_string());
}

fn source_path(fixture: &MailboxFixture, master: bool, id: &str) -> PathBuf {
    let dir = if master {
        PathBuf::from(fixture.app.state::<AppOutbox>().path())
    } else {
        fixture
            .sender_cwd
            .join(crate::config::agent_local_dir_name())
            .join("outbox")
    };
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{id}.json"))
}

fn save(path: &Path, message: &OutboxMessage) -> Vec<u8> {
    let bytes = serde_json::to_vec_pretty(message).unwrap();
    std::fs::write(path, &bytes).unwrap();
    bytes
}

fn local(cwd: &Path) -> PathBuf {
    cwd.join(crate::config::agent_local_dir_name())
}

async fn assert_no_effects(
    fixture: &MailboxFixture,
    poller: &MailboxPoller,
    hooks: &MailboxTestHooks,
    backend: &CorrelationBackend,
    sender: Uuid,
    target: Uuid,
) {
    assert_no_spawn_or_destroy_events(hooks);
    assert!(hooks.spawn_calls.lock().unwrap().is_empty());
    assert!(hooks.destroy_calls.lock().unwrap().is_empty());
    assert!(hooks.inject_calls.lock().unwrap().is_empty());
    assert!(hooks.settle_calls.lock().unwrap().is_empty());
    assert!(hooks.events.lock().unwrap().is_empty());
    assert!(backend.inner.writes.lock().unwrap().is_empty());
    assert!(backend.watchers.lock().unwrap().is_empty());
    assert_eq!(
        backend
            .inner
            .kill_attempts
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(pending_self_clear_len(fixture.app.handle()).await, 0);
    assert!(poller.live_claims.is_empty());
    assert!(poller.wake_workers.is_empty());
    assert!(poller.response_sources.lock().unwrap().is_empty());
    for cwd in [&fixture.sender_cwd, &fixture.target_cwd] {
        assert!(!local(cwd).join("responses").exists());
        assert!(!local(cwd).join("outbox/delivered").exists());
        assert!(!cwd.join("self-clear").exists());
    }
    assert!(!PathBuf::from(fixture.app.state::<AppOutbox>().path())
        .parent()
        .unwrap()
        .join("responses")
        .exists());
    let sessions = fixture
        .app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let manager = sessions.read().await;
    for id in [sender, target] {
        assert!(manager.get_session(id).await.is_some());
    }
    assert_eq!(
        std::fs::read(fixture.sender_cwd.join("SELF-HANDOFF.md")).unwrap(),
        b"resume notes for the test"
    );
}

#[tokio::test]
async fn noncanonical_ids_retry_ten_times_before_rejection_without_action_effects() {
    let invalid = [
        "../escape",
        r"..\escape",
        "C:/escape",
        r"C:\escape",
        r"\\server\share\escape",
        "/escape",
        "",
        "legacy-request-id",
        "550E8400-E29B-41D4-A716-446655440000",
        "550e8400e29b41d4a716446655440000",
        "{550e8400-e29b-41d4-a716-446655440000}",
        "urn:uuid:550e8400-e29b-41d4-a716-446655440000",
    ];
    for master in [false, true] {
        for action in ACTIONS {
            let fixture = make_mailbox_fixture();
            let app = app_handle(&fixture.app);
            scan_sender(&fixture).await;
            let backend = observe_backend(&fixture);
            let (sender, token) =
                seed_raise_hand_session(&app, &fixture.sender_cwd, true, SessionStatus::Running)
                    .await;
            let target = add_mailbox_session_with_shell(
                &app,
                &fixture.target_cwd,
                "target",
                "codex",
                SessionStatus::Idle,
            )
            .await;
            seed_self_handoff(&fixture.sender_cwd);
            let hooks = MailboxTestHooks::default();
            hooks.pty_presence.lock().unwrap().insert(target, true);
            let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());
            for raw in invalid {
                // msg.id is always safe; request_id is the only adversarial selector.
                let id = Uuid::new_v4().to_string();
                let path = source_path(&fixture, master, &id);
                let mut msg = wake_message_to_target();
                msg.id = id.clone();
                msg.token = Some(token.to_string());
                msg.request_id = Some(raw.into());
                msg.action = action.map(str::to_string);
                msg.get_output = action.is_none();
                msg.target = Some(CANONICAL_WAKE_TO.into());
                msg.force = Some(true);
                if action.is_some() {
                    msg.to.clear();
                }
                let bytes = save(&path, &msg);
                let rejected = path
                    .parent()
                    .unwrap()
                    .join("rejected")
                    .join(format!("{id}.json"));
                let reason = rejected.with_extension("reason.txt");
                for attempt in 1..=MAX_DELIVERY_ATTEMPTS {
                    // Supplemental exact-error observation uses the production content entry.
                    // poll() alone owns attempt accounting and the rejection lifecycle.
                    let mut deferred = None;
                    let origin = if master {
                        OutboxOrigin::Master
                    } else {
                        OutboxOrigin::Replica
                    };
                    let error = poller
                        .process_message_content(
                            &app,
                            &path,
                            origin,
                            std::str::from_utf8(&bytes).unwrap(),
                            Some(&mut deferred),
                        )
                        .await
                        .unwrap_err();
                    assert_eq!(error, "invalid_request_id");
                    assert!(deferred.is_none());
                    tokio::time::timeout(Duration::from_secs(10), poller.poll(&app))
                        .await
                        .unwrap()
                        .unwrap();
                    poller.settle_wake_workers().await;
                    assert_no_effects(&fixture, &poller, &hooks, &backend, sender, target).await;
                    if attempt < MAX_DELIVERY_ATTEMPTS {
                        let state = poller.retry_tracker.get(&path).expect("counted retry");
                        assert_eq!(state.attempt_count, attempt);
                        assert!(state.logged);
                        assert_eq!(std::fs::read(&path).unwrap(), bytes);
                        assert!(!rejected.exists());
                        assert!(!reason.exists());
                    } else {
                        assert!(!path.exists());
                        assert!(!poller.retry_tracker.contains_key(&path));
                        assert_eq!(
                            std::fs::read_to_string(&reason).unwrap(),
                            "Undeliverable after 10 attempts. Last error: invalid_request_id"
                        );
                        let copy: OutboxMessage =
                            serde_json::from_slice(&std::fs::read(&rejected).unwrap()).unwrap();
                        let mut expected = serde_json::to_value(&msg).unwrap();
                        expected.as_object_mut().unwrap().remove("token");
                        assert_eq!(serde_json::to_value(copy).unwrap(), expected);
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn invalid_id_precedes_authentication_even_with_a_malformed_token() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    scan_sender(&fixture).await;
    let path = source_path(&fixture, false, "auth-order");
    let mut msg = wake_message_to_target();
    msg.id = "auth-order".into();
    msg.token = Some("malformed-token".into());
    msg.request_id = Some("../escape".into());
    save(&path, &msg);
    let mut poller = MailboxPoller::default();
    for attempt in 1..=MAX_DELIVERY_ATTEMPTS {
        poller.poll(&app).await.unwrap();
        if attempt < MAX_DELIVERY_ATTEMPTS {
            assert_eq!(
                poller.retry_tracker.get(&path).unwrap().attempt_count,
                attempt
            );
            assert!(path.exists());
        }
    }
    assert_eq!(
        std::fs::read_to_string(
            path.parent()
                .unwrap()
                .join("rejected/auth-order.reason.txt"),
        )
        .unwrap(),
        "Undeliverable after 10 attempts. Last error: invalid_request_id"
    );
}

#[tokio::test]
async fn canonical_v4_and_v1_reach_authenticated_action_and_preserve_response_basename() {
    for master in [false, true] {
        for rid in [V4, V1] {
            let fixture = make_mailbox_fixture();
            let app = app_handle(&fixture.app);
            scan_sender(&fixture).await;
            // A specialist returns a real not_visible action response without global UI writes.
            let (sid, token) =
                seed_raise_hand_session(&app, &fixture.sender_cwd, false, SessionStatus::Running)
                    .await;
            let id = Uuid::new_v4().to_string();
            let (replica_path, msg) =
                build_raise_hand_message(&fixture.sender_cwd, &id, rid, Some(token.to_string()));
            let path = if master {
                let path = source_path(&fixture, true, &id);
                std::fs::rename(&replica_path, &path).unwrap();
                path
            } else {
                replica_path
            };
            let mut poller = MailboxPoller::default();
            poller.poll(&app).await.unwrap();
            assert!(!path.exists());
            let response = read_response_json(&fixture.sender_cwd, rid).unwrap();
            assert_eq!(response["action"], "raise-hand");
            assert_eq!(response["status"], "not_visible");
            assert_eq!(response["session_id"], sid.to_string());
            let delivered: OutboxMessage = serde_json::from_slice(
                &std::fs::read(
                    path.parent()
                        .unwrap()
                        .join("delivered")
                        .join(format!("{id}.json")),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(delivered.request_id, msg.request_id);
            assert_eq!(delivered.request_id.as_deref(), Some(rid));
            assert!(poller.retry_tracker.is_empty());
            if master {
                assert!(!path
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("responses")
                    .exists());
            }
        }
    }
}

#[tokio::test]
async fn absent_id_keeps_fire_and_forget_wake_and_plain_clear_requirement() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    scan_sender(&fixture).await;
    let hooks = MailboxTestHooks::default();
    let path = source_path(&fixture, false, "fire-and-forget");
    let mut msg = wake_message_to_target();
    msg.id = "fire-and-forget".into();
    msg.token = Some(MAILBOX_MASTER_TOKEN.into());
    msg.request_id = None;
    save(&path, &msg);
    let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    poller.poll(&app).await.unwrap();
    poller.settle_wake_workers().await;
    assert_eq!(hooks.spawn_calls.lock().unwrap().len(), 1);
    assert!(path
        .parent()
        .unwrap()
        .join("delivered/fire-and-forget.json")
        .is_file());
    assert!(!local(&fixture.sender_cwd).join("responses").exists());

    let (_, token) =
        seed_raise_hand_session(&app, &fixture.sender_cwd, false, SessionStatus::Running).await;
    let path = source_path(&fixture, false, "plain-clear-no-id");
    msg.id = "plain-clear-no-id".into();
    msg.token = Some(token.to_string());
    msg.action = Some("self-clear".into());
    msg.to.clear();
    save(&path, &msg);
    poller.poll(&app).await.unwrap();
    assert!(!path.exists());
    let reason = std::fs::read_to_string(
        path.parent()
            .unwrap()
            .join("rejected/plain-clear-no-id.reason.txt"),
    )
    .unwrap();
    assert_eq!(reason, "self-clear requires request_id UUID");
    assert!(!reason.contains("invalid_request_id"));
    assert_eq!(pending_self_clear_len(&app).await, 0);
}

#[tokio::test]
async fn malformed_json_keeps_existing_parse_error_and_raw_rejection() {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    scan_sender(&fixture).await;
    let path = source_path(&fixture, false, "malformed");
    // Valid JSON with a malformed OutboxMessage retains the legacy deserialize retry path.
    let bytes = br#"{"request_id":7}"#;
    std::fs::write(&path, bytes).unwrap();
    let mut poller = MailboxPoller::default();
    for attempt in 1..=MAX_DELIVERY_ATTEMPTS {
        poller.poll(&app).await.unwrap();
        if attempt < MAX_DELIVERY_ATTEMPTS {
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(
                poller.retry_tracker.get(&path).unwrap().attempt_count,
                attempt
            );
        }
    }
    assert!(!path.exists());
    let reason =
        std::fs::read_to_string(path.parent().unwrap().join("rejected/malformed.reason.txt"))
            .unwrap();
    assert!(reason.contains("10 attempts"));
    assert!(reason.contains("Failed to parse outbox message:"));
    assert!(!reason.contains("invalid_request_id"));

    // Syntactically invalid JSON is rejected by the preexisting document classifier.
    let path = source_path(&fixture, false, "invalid-json");
    std::fs::write(&path, b"{").unwrap();
    poller.poll(&app).await.unwrap();
    assert!(!path.exists());
    assert!(poller.retry_tracker.is_empty());
    let rejected = path.parent().unwrap().join("rejected");
    let reasons: Vec<String> = std::fs::read_dir(rejected)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "txt"))
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect();
    assert!(reasons
        .iter()
        .any(|reason| reason.starts_with("invalid_envelope:")));
}

// Include directories as well as bytes so an escaped write or extra archive is visible.
fn delivered_inventory(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    fn visit(root: &Path, dir: &Path, rows: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let directory = path.is_dir();
            rows.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                (!directory).then(|| std::fs::read(&path).unwrap()),
            ));
            if directory {
                visit(root, &path, rows);
            }
        }
    }
    let mut rows = Vec::new();
    visit(root, root, &mut rows);
    rows.sort();
    rows
}

fn delivered_work_state() -> crate::pty::manager::ResponseCloseState {
    use crate::pty::manager::ResponseClosePublisher;
    let state = crate::pty::manager::ResponseCloseState::default();
    for publisher in [
        ResponseClosePublisher::Manager,
        ResponseClosePublisher::Pty,
        ResponseClosePublisher::RawOutput,
        ResponseClosePublisher::Detector,
        ResponseClosePublisher::Menu,
        ResponseClosePublisher::Hold,
    ] {
        state.bind_publisher(publisher);
    }
    state.mark_work_bootstrapped();
    state.try_enable_readiness().unwrap();
    state.apply_settings(true, 30, std::time::Instant::now());
    state
}

fn assert_delivered_work_terminal(
    state: &crate::pty::manager::ResponseCloseState,
    target: &str,
    terminal: bool,
) {
    // The API checks terminal work before looking up a route. With no routes,
    // Conflict proves terminal dedupe; Unknown proves the work is still pending.
    let identity = crate::pty::manager::ResponseRouteIdentity {
        target: target.into(),
        session_id: Uuid::new_v4(),
        token: Uuid::new_v4(),
        generation: 1,
        carrier_version: 1,
        backend: crate::pty::backend_kind::SessionBackendKind::LocalProcess,
    };
    let result = state.reserve_policy_candidate(
        identity.clone(),
        identity,
        "delivery-work".into(),
        "digest".into(),
        std::time::Instant::now(),
    );
    let error = result.err().expect("no route was registered");
    let expected = if terminal {
        crate::pty::manager::ResponseCloseError::Conflict
    } else {
        crate::pty::manager::ResponseCloseError::Unknown
    };
    assert_eq!(error, expected);
}

fn assert_claimed_invalid_delivery_receipt(
    outbox: &Path,
    delivered: &Path,
    origin: &Path,
    claimed: bool,
    valid: bool,
    failure: u8,
) {
    if claimed && !valid && failure == 0 {
        assert!(!outbox.join("rejected/src.json").exists());
        assert_eq!(
            super::super::handoff::wake_claim_has_settled_receipt(origin),
            Ok(true)
        );
        assert!(delivered.join("src.json").is_file());
        assert!(!delivered.join("src.json.in-flight").exists());
    }
}

async fn delivered_case(raw: &str, valid: bool, claimed: bool, failure: u8) {
    let fixture = make_mailbox_fixture();
    let app = app_handle(&fixture.app);
    let sessions = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let coordinator = crate::session::selection::SelectionCoordinator::new(
        Arc::clone(sessions.inner()),
        tokio_util::sync::CancellationToken::new(),
    );
    assert!(app.manage(coordinator));
    // Two parents below TempDir contain every traversal in this table, even on pin.
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    let outbox = root.join("nested/outbox");
    let delivered = outbox.join("delivered");
    std::fs::create_dir_all(&delivered).unwrap();
    std::fs::write(root.join("sentinel"), b"outside unchanged").unwrap();
    let absolute = root.join("absolute-escape");
    let id = match raw {
        "@absolute" => absolute.to_string_lossy().into_owned(),
        "@verbatim" => std::fs::canonicalize(root)
            .unwrap()
            .join("verbatim-escape")
            .to_string_lossy()
            .into_owned(),
        _ => raw.into(),
    };
    let origin = outbox.join("src.json");
    let source = if claimed {
        super::super::handoff::wake_claim_path(&origin)
    } else {
        origin.clone()
    };
    let mut msg = wake_message_to_target();
    msg.id = id.clone();
    msg.token = Some("delivery-secret".into());
    let bytes = serde_json::to_vec_pretty(&msg).unwrap();
    if failure != 2 {
        save(&source, &msg);
    }
    let archive = if valid {
        delivered.join(format!("{id}.json"))
    } else {
        delivered.join("src.json")
    };
    if failure == 1 {
        std::fs::create_dir(&archive).unwrap();
    }
    let mut expected = delivered_inventory(root);
    let poller = MailboxPoller::delivery_only_for_tests();
    let state = delivered_work_state();
    let mut admission = state.reserve_work(&msg.to, "delivery-work").unwrap();
    assert!(admission.accept_durable("digest").unwrap());
    drop(admission);
    poller.response_sources.lock().unwrap().insert(
        origin.clone(),
        ResponseWakeWork {
            diagnostic_message_id: None,
            state: state.clone(),
            target: msg.to.clone(),
            key: "delivery-work".into(),
            digest: "digest".into(),
            attempt: Arc::default(),
            return_context: None,
            return_routes: None,
            delivery_context: None,
        },
    );
    assert_delivered_work_terminal(&state, &msg.to, false);
    let result = poller.move_to_delivered(&app, &source, &msg).await;
    if failure == 1 {
        assert!(result
            .unwrap_err()
            .starts_with("Failed to write delivered file:"));
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
    } else {
        let mut stripped = msg.clone();
        stripped.token = None;
        let archived_bytes = serde_json::to_vec_pretty(&stripped).unwrap();
        expected.push((
            archive.strip_prefix(root).unwrap().to_path_buf(),
            Some(archived_bytes.clone()),
        ));
        if failure == 2 {
            assert!(result
                .unwrap_err()
                .starts_with("Failed to remove outbox file:"));
        } else {
            assert_eq!(result, Ok(()), "id={id:?}, claimed={claimed}");
            expected.retain(|(path, _)| path != source.strip_prefix(root).unwrap());
            assert!(!source.exists());
        }
        assert_eq!(std::fs::read(&archive).unwrap(), archived_bytes);
        let archived: OutboxMessage =
            serde_json::from_slice(&std::fs::read(&archive).unwrap()).unwrap();
        assert!(archived.token.is_none());
        assert_eq!(archived.id, id);
        assert_eq!(
            serde_json::to_value(archived).unwrap(),
            serde_json::to_value(stripped).unwrap()
        );
        assert_claimed_invalid_delivery_receipt(
            &outbox, &delivered, &origin, claimed, valid, failure,
        );
    }
    expected.sort();
    assert_eq!(
        delivered_inventory(root),
        expected,
        "id={id:?}, claimed={claimed}"
    );
    assert_eq!(
        poller
            .response_sources
            .lock()
            .unwrap()
            .contains_key(&origin),
        failure != 0
    );
    assert_delivered_work_terminal(&state, &msg.to, failure == 0);
    app.state::<crate::session::selection::SelectionCoordinator>()
        .close_and_join()
        .await;
}

#[tokio::test]
async fn move_to_delivered_hostile_ids_use_origin_basename() {
    for claimed in [false, true] {
        for id in [
            "",
            ".",
            "..",
            "../escape",
            "nested/slash",
            r"nested\backslash",
            "\0",
            "@absolute",
            "@verbatim",
        ] {
            delivered_case(id, false, claimed, 0).await;
        }
    }
}

#[tokio::test]
async fn move_to_delivered_claimed_invalid_id_has_settled_receipt() {
    delivered_case("../escape", false, true, 0).await;
}

#[tokio::test]
async fn move_to_delivered_valid_ids_keep_exact_archive_and_complete_work() {
    for claimed in [false, true] {
        for id in [
            V4,
            "msg-sr-origin-switch",
            "embedded.dot",
            "雪",
            "with space",
            "tail.",
            "tail ",
            "\u{7f}",
            "\u{85}",
        ] {
            delivered_case(id, true, claimed, 0).await;
        }
        for id in ["a".repeat(244), "雪".repeat(81) + "a"] {
            delivered_case(&id, true, claimed, 0).await;
        }
    }
}

#[tokio::test]
async fn move_to_delivered_write_error_keeps_source_and_work() {
    for claimed in [false, true] {
        for (id, valid) in [("safe-id", true), ("../escape", false)] {
            delivered_case(id, valid, claimed, 1).await;
        }
    }
}

#[tokio::test]
async fn move_to_delivered_remove_error_keeps_archive_and_work() {
    for claimed in [false, true] {
        for (id, valid) in [("safe-id", true), ("../escape", false)] {
            delivered_case(id, valid, claimed, 2).await;
        }
    }
}

#[tokio::test]
async fn move_to_delivered_windows_invalid_ids_use_origin_basename() {
    let ids = ['<', '>', '"', '|', '?', '*']
        .into_iter()
        .chain('\u{1}'..='\u{1f}')
        .map(|c| format!("a{c}b"))
        .chain(["a".repeat(245), "a".repeat(251), "雪".repeat(81) + "ab"]);
    for id in ids {
        for claimed in [false, true] {
            delivered_case(&id, false, claimed, 0).await;
        }
    }
}

#[tokio::test]
async fn move_to_delivered_windows_invalid_id_is_injected_once() {
    for id in ["a?b".into(), "a\u{1}b".into(), "a".repeat(251)] {
        let fixture = make_mailbox_fixture();
        let app = app_handle(&fixture.app);
        let sessions = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let coordinator = crate::session::selection::SelectionCoordinator::new(
            Arc::clone(sessions.inner()),
            tokio_util::sync::CancellationToken::new(),
        );
        assert!(app.manage(coordinator));
        let session =
            add_mailbox_session(&app, &fixture.target_cwd, "live", SessionStatus::Idle, None).await;
        let hooks = MailboxTestHooks::default();
        hooks.pty_presence.lock().unwrap().insert(session, true);
        hooks.inject_results.lock().unwrap().push_back(Ok(()));
        hooks
            .consumption_results
            .lock()
            .unwrap()
            .push_back(ConsumptionVerdict::Observed);
        let mut poller = MailboxPoller::new_with_test_hooks(hooks.clone());
        let temp = tempfile::TempDir::new().unwrap();
        let origin = temp.path().join("src.json");
        let claim = super::super::handoff::wake_claim_path(&origin);
        let mut msg = wake_message_to_target();
        msg.id = id.clone();
        msg.token = Some("delivery-secret".into());
        save(&origin, &msg);
        assert!(matches!(
            poller.claim_and_spawn_wake(&app, &origin, msg.clone()),
            WakeHandoff::Spawned
        ));
        let joined = tokio::time::timeout(Duration::from_secs(10), async {
            let mut joined = 0;
            while let Some(result) = poller.wake_workers.join_next().await {
                assert!(result.is_ok(), "worker failed: {result:?}");
                joined += 1;
            }
            joined
        })
        .await
        .expect("wake worker timed out");
        assert_eq!(joined, 1);
        poller.drain_wake_outcomes().await;
        assert_eq!(*hooks.inject_calls.lock().unwrap(), vec![session]);
        assert!(!origin.exists(), "id={id:?}");
        assert!(!claim.exists());
        let archived: OutboxMessage =
            serde_json::from_slice(&std::fs::read(temp.path().join("delivered/src.json")).unwrap())
                .unwrap();
        assert_eq!(archived.id, id);
        assert!(archived.token.is_none());
        assert_eq!(
            super::super::handoff::wake_claim_has_settled_receipt(&origin),
            Ok(true)
        );
        assert!(poller.retry_tracker.is_empty());
        assert!(matches!(
            poller.claim_and_spawn_wake(&app, &origin, msg),
            WakeHandoff::LaneBusy
        ));
        assert!(poller.wake_workers.is_empty());
        assert_eq!(*hooks.inject_calls.lock().unwrap(), vec![session]);
        app.state::<crate::session::selection::SelectionCoordinator>()
            .close_and_join()
            .await;
    }
}
