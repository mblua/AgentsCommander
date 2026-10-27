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
