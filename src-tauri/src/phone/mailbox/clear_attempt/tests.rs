use super::*;
use crate::errors::AppError;
use crate::phone::mailbox::handoff::{release_wake_claim, wake_claim_path};
use crate::phone::mailbox::poller::{deliver_claimed_wake, MailboxPoller, ValidatedWake};
use crate::phone::mailbox::pty_claim::MailboxTestHooks;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
use crate::pty::manager::PtyManager;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::Manager;

fn envelope(id: &str) -> Value {
    serde_json::json!({
        "id": id, "from": "sender", "to": "target", "body": "", "mode": "wake",
        "timestamp": "2026-10-30T00:00:00Z", "command": "clear", "token": "secret",
        "unknown": {"preserve": [1, 2, 3]}
    })
}

fn message(id: &str) -> OutboxMessage {
    serde_json::from_value(envelope(id)).unwrap()
}

fn save(path: &Path, document: &Value) {
    std::fs::write(path, serde_json::to_vec(document).unwrap()).unwrap();
}

fn mark(path: &Path, id: &str) {
    save(path, &envelope(id));
    persist_attempt(path, &message(id), Uuid::nil()).unwrap();
}

#[test]
fn issue_3371_schema_absent_valid_and_corrupt() {
    assert_eq!(probe_attempt(&envelope("m").to_string()).unwrap(), None);
    let marker = serde_json::json!({
        "version": 1, "messageId": "m", "sessionId": Uuid::nil()
    });
    let mut document = envelope("m");
    document[MEMBER] = marker.clone();
    assert_eq!(
        probe_attempt(&document.to_string())
            .unwrap()
            .unwrap()
            .version,
        1
    );
    for bad in [
        Value::Null,
        Value::Bool(true),
        serde_json::json!({}),
        serde_json::json!({"version": 2, "messageId": "m", "sessionId": Uuid::nil()}),
        serde_json::json!({"version": 1, "messageId": "other", "sessionId": Uuid::nil()}),
        serde_json::json!({"version": 1, "messageId": "m", "sessionId": "not-a-uuid"}),
    ] {
        document[MEMBER] = bad;
        assert!(probe_attempt(&document.to_string()).is_err());
    }
    assert!(probe_attempt("{").is_err());
}

#[test]
fn issue_3371_persistence_preserves_envelope_and_rejects_reentry() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("dotted.message.json.in-flight");
    let original = envelope("dotted.message");
    save(&path, &original);
    persist_attempt(&path, &message("dotted.message"), Uuid::nil()).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let mut marked: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(probe_attempt(std::str::from_utf8(&bytes).unwrap())
        .unwrap()
        .is_some());
    marked.as_object_mut().unwrap().remove(MEMBER);
    assert_eq!(marked, original);
    assert!(persist_attempt(&path, &message("dotted.message"), Uuid::new_v4()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn issue_3371_source_binding_and_dangerous_ids_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("m.json");
    save(&path, &envelope("m"));
    let bytes = std::fs::read(&path).unwrap();
    for id in [
        "different",
        "../escape",
        "a/b",
        r"a\b",
        "C:escape",
        ".",
        "..",
    ] {
        assert!(persist_attempt(&path, &message(id), Uuid::nil()).is_err());
    }
    for field in ["to", "command"] {
        let mut changed = envelope("m");
        changed[field] = Value::String("different".into());
        let authorized = serde_json::from_value(changed).unwrap();
        assert!(persist_attempt(&path, &authorized, Uuid::nil()).is_err());
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn issue_3371_bom_matches_bounded_classifier() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("m.json");
    let text = envelope("m").to_string();
    let mut encodings = vec![
        text.as_bytes().to_vec(),
        [b"\xef\xbb\xbf", text.as_bytes()].concat(),
    ];
    for little in [true, false] {
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in text.encode_utf16() {
            bytes.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        encodings.push(bytes);
    }
    for bytes in encodings {
        std::fs::write(&path, bytes).unwrap();
        persist_attempt(&path, &message("m"), Uuid::nil()).unwrap();
        assert!(probe_attempt(&std::fs::read_to_string(&path).unwrap())
            .unwrap()
            .is_some());
    }
    for bytes in [vec![0xff, 0xfe, 1], vec![0xfe, 0xff, 0xd8, 0], vec![0xff]] {
        std::fs::write(&path, &bytes).unwrap();
        assert!(persist_attempt(&path, &message("m"), Uuid::nil()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn issue_3371_envelope_exact_limit_and_over_limit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("m.json");
    let mut expected = envelope("m");
    expected[MEMBER] = serde_json::to_value(ClearAttempt {
        version: 1,
        message_id: "m".into(),
        session_id: Uuid::nil(),
    })
    .unwrap();
    let padding = PTY_INPUT_HOST_ENVELOPE_MAX_BYTES - serde_json::to_vec(&expected).unwrap().len();
    for over in [0, 1] {
        let mut original = envelope("m");
        original["body"] = Value::String("a".repeat(padding + over));
        save(&path, &original);
        let before = std::fs::read(&path).unwrap();
        let result = persist_attempt(&path, &message("m"), Uuid::nil());
        if over == 0 {
            result.unwrap();
            assert_eq!(
                std::fs::read(&path).unwrap().len(),
                PTY_INPUT_HOST_ENVELOPE_MAX_BYTES
            );
        } else {
            assert!(matches!(result, Err(AttemptError::InvalidDocument)));
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}

#[derive(Default)]
struct RecordingBackend {
    writes: Mutex<Vec<Vec<u8>>>,
    spawns: Mutex<usize>,
}

impl PtyBackend for RecordingBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn spawn(
        &self,
        _spec: BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        *self.spawns.lock().unwrap() += 1;
        Box::pin(async { Ok(()) })
    }
    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        _id: Uuid,
        data: &[u8],
    ) -> Result<(), AppError> {
        self.writes.lock().unwrap().push(data.to_vec());
        Ok(())
    }
    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
        Ok(())
    }
    fn kill(&self, _id: Uuid) -> Result<(), AppError> {
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
    fn register_response_watcher(&self, _id: Uuid, _request: String, _dir: PathBuf) {}
    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }
    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

#[tokio::test]
async fn issue_3371_storage_faults_gate_recording_continuation() {
    use mailbox_disk::{MailboxFaultGuard, MailboxFaultPoint};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("m.json.in-flight");
    let unrelated = temp.path().join(".foreign.codex-clear-stage");
    std::fs::write(&unrelated, b"foreign").unwrap();
    let backend = Arc::new(RecordingBackend::default());
    let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    let id = Uuid::new_v4();
    manager
        .lock()
        .unwrap()
        .try_record_route(id, SessionBackendKind::LocalProcess)
        .unwrap();
    let permit = PtyManager::acquire_input_writer(&manager, id)
        .await
        .unwrap();
    for point in [
        Some(MailboxFaultPoint::StageSync),
        Some(MailboxFaultPoint::Replace),
        None,
    ] {
        save(&path, &envelope("m"));
        let before = std::fs::read(&path).unwrap();
        let guard = point.map(MailboxFaultGuard::install);
        let result = persist_attempt(&path, &message("m"), id);
        if let Some(guard) = guard {
            assert!(guard.consumed());
            drop(guard);
            assert!(result.is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert!(backend.writes.lock().unwrap().is_empty());
        } else {
            result.unwrap();
            PtyManager::write_with_permit(&permit, b"/clear").unwrap();
            PtyManager::write_with_permit(&permit, b"\r").unwrap();
            assert_eq!(
                *backend.writes.lock().unwrap(),
                vec![b"/clear".to_vec(), b"\r".to_vec()]
            );
        }
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"foreign");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
    }
}

#[test]
fn issue_3371_fault_guard_restores_on_unwind_and_rejects_nesting() {
    use mailbox_disk::{MailboxFaultGuard, MailboxFaultPoint};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("m.json");
    let outcome = std::panic::catch_unwind(|| {
        let _guard = MailboxFaultGuard::install(MailboxFaultPoint::Replace);
        let _nested = MailboxFaultGuard::install(MailboxFaultPoint::StageSync);
    });
    assert!(outcome.is_err());
    mark(&path, "m");
}

struct ScannerFixture {
    _temp: tempfile::TempDir,
    outbox: PathBuf,
    app: tauri::App<tauri::test::MockRuntime>,
    backend: Arc<RecordingBackend>,
    hooks: MailboxTestHooks,
}

impl ScannerFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::path_utils::normalize_windows_verbatim_path_buf(
            &std::fs::canonicalize(temp.path()).unwrap(),
        );
        let outbox = root.join("outbox");
        std::fs::create_dir(&outbox).unwrap();
        let owners = crate::app_state::AppOwners::new();
        let sessions = crate::session::manager::SessionManager::with_activity_coalescer(
            owners.activity.clone(),
        );
        let backend = Arc::new(RecordingBackend::default());
        let app = tauri::test::mock_builder()
            .manage(owners)
            .manage(crate::config::settings::new_settings_state(
                Default::default(),
            ))
            .manage(Arc::new(tokio::sync::RwLock::new(sessions)))
            .manage(Arc::new(Mutex::new(PtyManager::new_for_test(
                backend.clone(),
            ))))
            .manage(crate::AppOutbox::new(outbox.to_string_lossy().into_owned()))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        Self {
            _temp: temp,
            outbox,
            app,
            backend,
            hooks: MailboxTestHooks::default(),
        }
    }

    fn poller(&self) -> MailboxPoller {
        MailboxPoller::new_with_test_hooks(self.hooks.clone())
    }

    fn assert_no_actuation(&self, poller: &MailboxPoller) {
        assert!(self.backend.writes.lock().unwrap().is_empty());
        assert_eq!(*self.backend.spawns.lock().unwrap(), 0);
        assert!(self.hooks.spawn_calls.lock().unwrap().is_empty());
        assert!(self.hooks.inject_calls.lock().unwrap().is_empty());
        assert!(self.hooks.events.lock().unwrap().is_empty());
        assert!(self.hooks.internal_bookkeeping.lock().unwrap().is_empty());
        assert!(poller.live_claims.is_empty());
        assert!(poller.wake_workers.is_empty());
    }
}

#[tokio::test]
async fn issue_3371_crash_before_first_text_and_malformed_markers_never_execute() {
    let fixture = ScannerFixture::new();
    for (index, marker) in [Value::Null, serde_json::json!({"version": 99})]
        .into_iter()
        .enumerate()
    {
        let id = format!("corrupt-{index}");
        let path = fixture.outbox.join(format!("{id}.json"));
        let mut document = envelope(&id);
        document[MEMBER] = marker;
        save(&path, &document);
    }
    let path = fixture.outbox.join("crash-before-text.json");
    mark(&path, "crash-before-text");
    let mut poller = fixture.poller();
    poller.poll(fixture.app.handle()).await.unwrap();
    fixture.assert_no_actuation(&poller);
    for id in ["corrupt-0", "corrupt-1", "crash-before-text"] {
        assert!(!fixture.outbox.join(format!("{id}.json")).exists());
        let archive = fixture.outbox.join("rejected").join(format!("{id}.json"));
        let archived: Value = serde_json::from_slice(&std::fs::read(archive).unwrap()).unwrap();
        assert!(archived.get("token").is_none());
        assert_eq!(
            std::fs::read_to_string(
                fixture
                    .outbox
                    .join("rejected")
                    .join(format!("{id}.reason.txt"))
            )
            .unwrap(),
            REPLAY_REASON
        );
    }
}

#[tokio::test]
async fn issue_3371_rejection_failure_retries_only_publication_across_restart() {
    for after_reason in [false, true] {
        let fixture = ScannerFixture::new();
        let path = fixture.outbox.join("m.json");
        mark(&path, "m");
        let original = std::fs::read(&path).unwrap();
        let rejected = fixture.outbox.join("rejected");
        std::fs::create_dir(&rejected).unwrap();
        let blocker = rejected.join(if after_reason {
            "m.json"
        } else {
            "m.reason.txt"
        });
        std::fs::create_dir(&blocker).unwrap();
        let mut poller = fixture.poller();
        if after_reason {
            assert!(poller
                .reject_message(&path, &message("m"), REPLAY_REASON)
                .await
                .is_err());
            assert_eq!(
                std::fs::read_to_string(rejected.join("m.reason.txt")).unwrap(),
                REPLAY_REASON
            );
        } else {
            poller.poll(fixture.app.handle()).await.unwrap();
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
        fixture.assert_no_actuation(&poller);
        std::fs::remove_dir(&blocker).unwrap();
        let reason = "codex_clear_execution_error: retained live failure";
        std::fs::write(rejected.join("m.reason.txt"), reason).unwrap();
        let mut restarted = fixture.poller();
        restarted.poll(fixture.app.handle()).await.unwrap();
        fixture.assert_no_actuation(&restarted);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(rejected.join("m.reason.txt")).unwrap(),
            reason
        );
    }
}

#[tokio::test]
async fn issue_3371_payload_receipts_win_with_dotted_leftover_source() {
    for receipt in ["delivered", "rejected"] {
        let fixture = ScannerFixture::new();
        let path = fixture.outbox.join("dotted.id.json");
        mark(&path, "dotted.id");
        let directory = fixture.outbox.join(receipt);
        std::fs::create_dir(&directory).unwrap();
        let receipt_path = directory.join("dotted.id.json");
        std::fs::write(&receipt_path, b"receipt-preserved").unwrap();
        let mut poller = fixture.poller();
        poller.poll(fixture.app.handle()).await.unwrap();
        assert!(!path.exists());
        assert_eq!(std::fs::read(&receipt_path).unwrap(), b"receipt-preserved");
        assert!(!fixture
            .outbox
            .join("rejected/dotted.id.reason.txt")
            .exists());
        fixture.assert_no_actuation(&poller);
    }
}

#[tokio::test]
async fn issue_3371_worker_restore_drain_and_restart_preserve_marker() {
    for recovery in ["worker", "drain", "restart"] {
        let fixture = ScannerFixture::new();
        let path = fixture.outbox.join("m.json");
        let claim = wake_claim_path(&path);
        mark(&claim, "m");
        let before = std::fs::read(&claim).unwrap();
        let mut poller = fixture.poller();
        match recovery {
            "worker" => {
                let validated = ValidatedWake {
                    message: message("m"),
                    response: None,
                };
                assert!(deliver_claimed_wake(
                    &poller,
                    fixture.app.handle(),
                    &claim,
                    &path,
                    &validated
                )
                .await
                .is_err());
            }
            "drain" => {
                poller.live_claims.insert(claim.clone());
                poller
                    .wake_outcomes_tx
                    .send((path.clone(), Err("delivery interrupted".into())))
                    .unwrap();
                poller.drain_wake_outcomes().await;
            }
            _ => poller.poll(fixture.app.handle()).await.unwrap(),
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        poller.poll(fixture.app.handle()).await.unwrap();
        assert!(!path.exists());
        fixture.assert_no_actuation(&poller);
    }
}

#[tokio::test]
async fn issue_3371_nonplain_ids_keep_legacy_raw_archive_order_and_token() {
    let fixture = ScannerFixture::new();
    let path = fixture.outbox.join("safe-source.json");
    let mut document = envelope("../not-an-archive-path");
    document[MEMBER] = Value::Null;
    save(&path, &document);
    let rejected = fixture.outbox.join("rejected");
    std::fs::create_dir(&rejected).unwrap();
    let reason = rejected.join("safe-source.reason.txt");
    // A reason-only failure is probed before raw publication, so simulate the
    // legacy rename-first failure directly, then run scanner recovery.
    std::fs::create_dir(&reason).unwrap();
    let poller = fixture.poller();
    assert!(poller
        .reject_message(&path, &message("../not-an-archive-path"), REPLAY_REASON)
        .await
        .is_err());
    assert!(!path.exists());
    let archived: Value =
        serde_json::from_slice(&std::fs::read(rejected.join("safe-source.json")).unwrap()).unwrap();
    assert_eq!(archived["token"], "secret");
    assert!(reason.is_dir());
    std::fs::remove_dir(&reason).unwrap();
    let mut restarted = fixture.poller();
    restarted.poll(fixture.app.handle()).await.unwrap();
    assert!(!reason.exists());
    fixture.assert_no_actuation(&restarted);
}

#[tokio::test]
async fn issue_3371_delivered_publication_failure_before_receipt_restores_fence() {
    let fixture = ScannerFixture::new();
    let origin = fixture.outbox.join("m.json");
    let claim = wake_claim_path(&origin);
    mark(&claim, "m");
    let bytes = std::fs::read(&claim).unwrap();
    std::fs::create_dir(fixture.outbox.join("delivered")).unwrap();
    let blocker = fixture.outbox.join("delivered/m.json");
    std::fs::create_dir(&blocker).unwrap();
    let mut poller = fixture.poller();
    assert!(poller
        .move_to_delivered(fixture.app.handle(), &claim, &message("m"))
        .await
        .is_err());
    assert_eq!(std::fs::read(&claim).unwrap(), bytes);
    std::fs::remove_dir(&blocker).unwrap();
    release_wake_claim(&claim, &origin);
    poller.poll(fixture.app.handle()).await.unwrap();
    assert!(!origin.exists());
    assert!(fixture.outbox.join("rejected/m.json").is_file());
    fixture.assert_no_actuation(&poller);
}

#[cfg(windows)]
#[tokio::test]
async fn issue_3371_receipt_written_source_remove_failure_retries_cleanup_only() {
    use std::os::windows::fs::OpenOptionsExt;
    for receipt in ["delivered", "rejected"] {
        let fixture = ScannerFixture::new();
        let origin = fixture.outbox.join("m.json");
        mark(&origin, "m");
        let original = std::fs::read(&origin).unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&origin)
            .unwrap();
        let mut poller = fixture.poller();
        let result = if receipt == "delivered" {
            poller
                .move_to_delivered(fixture.app.handle(), &origin, &message("m"))
                .await
        } else {
            poller
                .reject_message(&origin, &message("m"), "live clear error")
                .await
        };
        assert!(result.is_err());
        assert_eq!(std::fs::read(&origin).unwrap(), original);
        let receipt_path = fixture.outbox.join(receipt).join("m.json");
        let receipt_bytes = std::fs::read(&receipt_path).unwrap();
        poller.poll(fixture.app.handle()).await.unwrap();
        assert_eq!(std::fs::read(&origin).unwrap(), original);
        drop(file);
        let mut restarted = fixture.poller();
        restarted.poll(fixture.app.handle()).await.unwrap();
        assert!(!origin.exists());
        assert_eq!(std::fs::read(&receipt_path).unwrap(), receipt_bytes);
        fixture.assert_no_actuation(&restarted);
    }
}

#[tokio::test]
async fn issue_3371_unmarked_ordinary_message_retains_existing_validation() {
    let fixture = ScannerFixture::new();
    fixture.app.manage(crate::MasterToken::new("secret".into()));
    let path = fixture.outbox.join("ordinary.json");
    let mut document = envelope("ordinary");
    document.as_object_mut().unwrap().remove("command");
    document["mode"] = Value::String("ordinary".into());
    save(&path, &document);
    let mut poller = fixture.poller();
    poller.poll(fixture.app.handle()).await.unwrap();
    assert!(!path.exists());
    let reason =
        std::fs::read_to_string(fixture.outbox.join("rejected/ordinary.reason.txt")).unwrap();
    assert!(reason.contains("Unsupported delivery mode"), "{reason}");
    assert!(!reason.contains("codex_clear_execution_error"));
    fixture.assert_no_actuation(&poller);
}

#[tokio::test]
async fn issue_3371_malformed_payload_uses_raw_rejection_without_execution() {
    let fixture = ScannerFixture::new();
    let path = fixture.outbox.join("broken.json");
    save(&path, &serde_json::json!({"id": "broken", MEMBER: null}));
    let mut poller = fixture.poller();
    poller.poll(fixture.app.handle()).await.unwrap();
    assert!(!path.exists());
    assert_eq!(
        std::fs::read_to_string(fixture.outbox.join("rejected/broken.reason.txt")).unwrap(),
        REPLAY_REASON
    );
    fixture.assert_no_actuation(&poller);
}

#[tokio::test]
async fn issue_3371_crash_stage_has_no_scanner_authority() {
    let fixture = ScannerFixture::new();
    let stage = fixture.outbox.join(".m.json.abandoned.codex-clear-stage");
    save(&stage, &envelope("m"));
    let before = std::fs::read(&stage).unwrap();
    let mut poller = fixture.poller();
    poller.poll(fixture.app.handle()).await.unwrap();
    assert_eq!(std::fs::read(&stage).unwrap(), before);
    assert!(!fixture.outbox.join("rejected").exists());
    fixture.assert_no_actuation(&poller);
}
