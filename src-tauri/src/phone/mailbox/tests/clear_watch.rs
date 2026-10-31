use super::CodexClearScannerFixture;
use crate::errors::AppError;
use crate::filesystem::phone_mailbox::MailboxFaultGuard;
use crate::phone::mailbox::clear_attempt::{persist_attempt, probe_attempt};
use crate::phone::mailbox::handoff::{prune_retry_tracker, wake_claim_path, WakeHandoff};
use crate::phone::mailbox::poller::MailboxPoller;
use crate::phone::types::OutboxMessage;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

#[derive(Default)]
struct RecordingBackend {
    writes: Mutex<Vec<Vec<u8>>>,
    spawns: Mutex<usize>,
}

impl PtyBackend for RecordingBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn spawn(&self, _: BackendSpawnSpec) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        *self.spawns.lock().unwrap() += 1;
        Box::pin(async { Ok(()) })
    }
    fn write(
        &self,
        _: &crate::pty::manager::BackendWriteAuthority,
        _: Uuid,
        bytes: &[u8],
    ) -> Result<(), AppError> {
        self.writes.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }
    fn resize(&self, _: Uuid, _: u16, _: u16) -> Result<(), AppError> {
        Ok(())
    }
    fn kill(&self, _: Uuid) -> Result<(), AppError> {
        Ok(())
    }
    fn has_session(&self, _: Uuid) -> bool {
        true
    }
    fn get_screen_snapshot(&self, _: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }
    fn get_pty_size(&self, _: Uuid) -> Option<(u16, u16)> {
        None
    }
    fn get_screen_rows(&self, _: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }
    fn register_response_watcher(&self, _: Uuid, _: String, _: PathBuf) {}
    fn terminate_job_for_session(&self, _: Uuid) -> bool {
        false
    }
    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

fn message(id: &str) -> OutboxMessage {
    serde_json::from_value(serde_json::json!({
        "id": id, "from": "proj-a:wg-1-dev-team/tech-lead",
        "to": "proj-a:wg-1-dev-team/dev-rust", "body": "", "mode": "wake",
        "timestamp": "2026-10-30T00:00:00Z", "command": "clear", "token": "secret"
    }))
    .unwrap()
}

fn save(outbox: &Path, msg: &OutboxMessage) -> PathBuf {
    let path = outbox.join(format!("{}.json", msg.id));
    std::fs::write(&path, serde_json::to_vec(msg).unwrap()).unwrap();
    path
}

fn project(root: &Path) -> PathBuf {
    let ac = root.join("proj-a/.ac");
    for name in ["_agent_dev-rust", "_agent_tech-lead", "_team_dev-team"] {
        std::fs::create_dir_all(ac.join(name)).unwrap();
    }
    std::fs::write(
        ac.join("_team_dev-team/config.json"),
        concat!(
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"#,
            r#""coordinator":"../_agent_tech-lead"}"#,
        ),
    )
    .unwrap();
    let room = ac.join("wg-1-dev-team");
    for name in ["dev-rust", "tech-lead"] {
        let replica = room.join(format!("__agent_{name}"));
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::write(
            replica.join("config.json"),
            format!(r#"{{"identity":"../../_agent_{name}"}}"#,),
        )
        .unwrap();
    }
    room.join("__agent_dev-rust")
}

async fn finish(poller: &mut MailboxPoller) {
    tokio::time::timeout(Duration::from_secs(60), async {
        while poller.wake_workers.join_next().await.is_some() {}
        poller.drain_wake_outcomes().await;
    })
    .await
    .unwrap();
    assert!(poller.live_claims.is_empty());
    assert_eq!(poller.wake_lanes.len(), 0);
    assert_eq!(poller.wake_lanes.observing_count(), 0);
}

#[tokio::test]
async fn issue_3371_worker_observers_release_both_caps_and_keep_target_exclusion() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = CodexClearScannerFixture::new(temp.path(), Arc::new(RecordingBackend::default()));
    let mut poller = fixture.poller();
    let observers = fixture.observe_deliveries(&mut poller, true);
    for slot in 0..8 {
        let mut msg = message(&format!("observer-{slot}"));
        msg.to = format!("observer-{slot}");
        let source = save(temp.path(), &msg);
        assert!(matches!(
            fixture.claim(&mut poller, &source, msg),
            WakeHandoff::Spawned
        ));
    }
    let sources = tokio::time::timeout(Duration::from_secs(5), observers.wait_for(8))
        .await
        .unwrap();
    assert!(sources
        .iter()
        .all(|path| path.extension().unwrap() == "in-flight"));
    assert_eq!(poller.wake_lanes.observing_count(), 8);
    let mut same = message("same-recipient");
    same.to = "observer-0".into();
    let source = save(temp.path(), &same);
    assert!(matches!(
        fixture.claim(&mut poller, &source, same),
        WakeHandoff::LaneBusy
    ));
    let active = fixture.observe_deliveries(&mut poller, false);
    for slot in 0..8 {
        let mut msg = message(&format!("active-{slot}"));
        msg.to = format!("active-{slot}");
        let source = save(temp.path(), &msg);
        assert!(matches!(
            fixture.claim(&mut poller, &source, msg),
            WakeHandoff::Spawned
        ));
    }
    tokio::time::timeout(Duration::from_secs(5), active.wait_for(8))
        .await
        .unwrap();
    assert_eq!(poller.wake_workers.len(), 16);
    assert_eq!(poller.live_claims.len(), 16);
    let msg = message("cap-rejection");
    let source = save(temp.path(), &msg);
    assert!(matches!(
        fixture.claim(&mut poller, &source, msg),
        WakeHandoff::LaneBusy
    ));
    assert!(source.exists());
    observers.release(8);
    active.release(8);
    finish(&mut poller).await;
    assert_eq!(fixture.recorded_effect_counts(), [0; 5]);
}

#[tokio::test]
async fn issue_3371_pre_t2_cancel_and_panic_release_worker_claims() {
    for demote in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let fixture =
            CodexClearScannerFixture::new(temp.path(), Arc::new(RecordingBackend::default()));
        let target = project(temp.path());
        fixture.configure_project(temp.path()).await;
        let id = fixture.seed_session(&target, false).await;
        for panic in [false, true] {
            let mut poller = fixture.poller();
            let probe = fixture.observe_deliveries(&mut poller, demote);
            let msg = message(if panic { "panic" } else { "cancel" });
            let source = save(temp.path(), &msg);
            if panic {
                fixture.panic_on_inject(id);
            }
            assert!(matches!(
                fixture.claim(&mut poller, &source, msg),
                WakeHandoff::Spawned
            ));
            tokio::time::timeout(Duration::from_secs(5), probe.wait_for(1))
                .await
                .unwrap();
            assert_eq!(poller.wake_lanes.observing_count(), usize::from(demote));
            if panic {
                probe.release(1);
            } else {
                poller.wake_workers.abort_all();
            }
            finish(&mut poller).await;
            assert!(source.exists());
            assert!(!wake_claim_path(&source).exists());
            assert_eq!(poller.retry_tracker[&source].attempt_count, 1);
        }
    }
}

#[tokio::test]
async fn issue_3371_terminal_reason_worker_drain_prune_and_source_defaults() {
    for reason in [
        "codex_clear_failed: old transcript",
        "codex_clear_execution_error: lost route",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let backend = Arc::new(RecordingBackend::default());
        let fixture = CodexClearScannerFixture::new(temp.path(), backend.clone());
        let target = project(temp.path());
        fixture.configure_project(temp.path()).await;
        fixture.seed_session(&target, false).await;
        fixture.injection_error(reason);
        let mut poller = fixture.poller();
        assert!(poller.codex_clear_source.is_none());
        assert!(poller.clear_reservation.is_none());
        let msg = message("terminal");
        let source = save(temp.path(), &msg);
        assert!(matches!(
            fixture.claim(&mut poller, &source, msg),
            WakeHandoff::Spawned
        ));
        finish(&mut poller).await;
        assert_eq!(
            std::fs::read(temp.path().join("rejected/terminal.reason.txt")).unwrap(),
            reason.as_bytes()
        );
        let rejected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(temp.path().join("rejected/terminal.json")).unwrap(),
        )
        .unwrap();
        assert!(rejected.get("token").is_none_or(serde_json::Value::is_null));
        assert!(!source.exists());
        assert!(!wake_claim_path(&source).exists());
        assert!(!temp.path().join("delivered/terminal.json").exists());
        assert_eq!(poller.retry_tracker[&source].attempt_count, 1);
        prune_retry_tracker(&mut poller.retry_tracker, &poller.live_claims, None);
        assert!(poller.retry_tracker.is_empty());
        assert_eq!(fixture.recorded_effect_counts()[1], 1);
        assert_eq!(fixture.recorded_effect_counts()[4], 0);
        assert!(backend.writes.lock().unwrap().is_empty());
        assert_eq!(*backend.spawns.lock().unwrap(), 0);
        assert!(poller.delivery_poller().codex_clear_source.is_none());
        assert!(MailboxPoller::delivery_only_for_tests()
            .codex_clear_source
            .is_none());
    }
}

#[tokio::test]
async fn issue_3371_inline_comanaged_and_spawn_fallback_publish_exact_reason() {
    for (comanaged, session) in [
        (true, Some(false)),
        (false, Some(false)),
        (false, None),
        (false, Some(true)),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let fixture =
            CodexClearScannerFixture::new(temp.path(), Arc::new(RecordingBackend::default()));
        let target = project(temp.path());
        fixture.configure_project(temp.path()).await;
        if let Some(exited) = session {
            fixture.seed_session(&target, exited).await;
        }
        let poller = fixture.poller();
        let msg = message("inline");
        let source = save(temp.path(), &msg);
        let reason = "codex_clear_failed: exact stable reason";
        fixture.injection_error(reason);
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(5),
                fixture.inline(&poller, &source, msg, comanaged)
            )
            .await
            .unwrap(),
            Err(reason.into())
        );
        assert_eq!(
            std::fs::read(temp.path().join("rejected/inline.reason.txt")).unwrap(),
            reason.as_bytes()
        );
        assert!(!source.exists());
        assert!(poller.codex_clear_source.is_none());
        let effects = fixture.recorded_effect_counts();
        assert_eq!(effects[0], usize::from(session != Some(false)));
        assert_eq!(effects[1], 1);
        assert_eq!(effects[4], 0);
    }
}

#[tokio::test]
async fn issue_3371_reason_survives_json_failure_scanner_and_restart_without_retyping() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = CodexClearScannerFixture::new(temp.path(), Arc::new(RecordingBackend::default()));
    let target = project(temp.path());
    fixture.configure_project(temp.path()).await;
    fixture.install_authentication("secret");
    fixture.seed_session(&target, false).await;
    let reason = "codex_clear_failed: reason survived";
    let msg = message("partial");
    let source = save(temp.path(), &msg);
    persist_attempt(&source, &msg, Uuid::nil()).unwrap();
    std::fs::create_dir_all(temp.path().join("rejected/partial.json")).unwrap();
    fixture.injection_error(reason);
    let mut poller = fixture.poller();
    assert!(matches!(
        fixture.claim(&mut poller, &source, msg),
        WakeHandoff::Spawned
    ));
    finish(&mut poller).await;
    let reason_path = temp.path().join("rejected/partial.reason.txt");
    assert_eq!(std::fs::read(&reason_path).unwrap(), reason.as_bytes());
    assert!(probe_attempt(&std::fs::read_to_string(&source).unwrap())
        .unwrap()
        .is_some());
    let before = fixture.recorded_effect_counts();
    fixture.poll(&mut poller).await.unwrap();
    let mut restarted = fixture.poller();
    fixture.poll(&mut restarted).await.unwrap();
    assert_eq!(fixture.recorded_effect_counts(), before);
    assert_eq!(std::fs::read(&reason_path).unwrap(), reason.as_bytes());
    std::fs::remove_dir(temp.path().join("rejected/partial.json")).unwrap();
    fixture.poll(&mut restarted).await.unwrap();
    assert!(!source.exists());
    assert_eq!(fixture.recorded_effect_counts(), before);
    assert_eq!(std::fs::read(&reason_path).unwrap(), reason.as_bytes());
}

#[tokio::test]
async fn issue_3371_pathless_and_unrelated_errors_keep_legacy_release() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = CodexClearScannerFixture::new(temp.path(), Arc::new(RecordingBackend::default()));
    let target = project(temp.path());
    fixture.configure_project(temp.path()).await;
    fixture.seed_session(&target, false).await;
    let poller = fixture.poller();
    let msg = message("legacy");
    assert!(!poller.is_terminal_clear_error(&msg, "codex_clear_failed: old"));
    let mut bound = poller.delivery_poller();
    let source = save(temp.path(), &msg);
    bound.codex_clear_source = Some(wake_claim_path(&source));
    assert!(!bound.is_terminal_clear_error(&msg, "prefix codex_clear_failed: old"));
    let mut ordinary = msg.clone();
    ordinary.command = None;
    assert!(!bound.is_terminal_clear_error(&ordinary, "codex_clear_failed: old"));
    std::fs::rename(&source, wake_claim_path(&source)).unwrap();
    fixture.injection_error("transient legacy error");
    assert_eq!(
        fixture
            .deliver_claimed(
                &bound,
                &wake_claim_path(&source),
                &source,
                &crate::phone::mailbox::poller::ValidatedWake {
                    message: msg,
                    response: None
                },
            )
            .await,
        Err("transient legacy error".into())
    );
    assert!(source.exists());
    assert!(!temp.path().join("rejected").exists());
}

#[tokio::test]
async fn issue_3371_owned_replacement_remains_source_free_and_retained_on_injection_error() {
    const NAME: &str = concat!(
        "phone::mailbox::tests::clear_watch::",
        "issue_3371_owned_replacement_remains_source_free_and_retained_on_injection_error"
    );
    const CHILD: &str = "AC_3371_OWNED_CLEAR_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            CodexClearScannerFixture::owned_clear_exclusion(temp).await,
            (true, 1, true, 1)
        );
        return;
    }
    let config = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([NAME, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("AGENTSCOMMANDER_CONFIG_DIR", config.path())
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", config.path())
        .env("RUST_MIN_STACK", "33554432")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().unwrap();
    use tokio::io::AsyncReadExt;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let err = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.unwrap();
        bytes
    });
    let status = tokio::time::timeout(Duration::from_secs(45), child.wait()).await;
    let timed_out = status.is_err();
    if timed_out {
        child.kill().await.unwrap();
    }
    let output = std::process::Output {
        status: if let Ok(status) = status {
            status.unwrap()
        } else {
            child.wait().await.unwrap()
        },
        stdout: out.await.unwrap(),
        stderr: err.await.unwrap(),
    };
    assert!(!timed_out, "owned clear child timed out: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed;"));
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

const KNOWN: &str = "codex_clear_failed: provider reported fresh-session startup failure";
const INCOMPLETE: &str = "codex_clear_execution_error: observation incomplete";
const PREFIX: &str = "Failed to start a fresh session through the app server:";

type ClearBackend = super::MailboxMockPtyBackend;

async fn activated(
    seed: Option<bool>,
) -> (
    tempfile::TempDir,
    PathBuf,
    Arc<ClearBackend>,
    CodexClearScannerFixture,
    Option<Uuid>,
) {
    let temp = tempfile::tempdir().unwrap();
    let target = project(temp.path());
    let backend = Arc::new(ClearBackend::default());
    backend
        .prepare_clear
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let fixture = CodexClearScannerFixture::new(temp.path(), backend.clone());
    fixture.configure_project(temp.path()).await;
    fixture.install_authentication("secret");
    let session = match seed {
        Some(false) => Some(fixture.real_session(&target, "codex").await),
        Some(true) => Some(fixture.seed_session(&target, true).await),
        None => None,
    };
    (temp, target, backend, fixture, session)
}

async fn submitted(backend: &ClearBackend, fixture: &CodexClearScannerFixture) -> Uuid {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let ready = backend
                .enter_times
                .lock()
                .unwrap()
                .iter()
                .find(|(_, times)| times.len() == 2)
                .map(|(id, _)| *id);
            if let Some(id) = ready {
                if fixture.submission_state(id).await.0 {
                    return id;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual second CR and T2 accounting")
}

fn failure_rows(backend: &ClearBackend, id: Uuid) {
    backend
        .rows
        .lock()
        .unwrap()
        .insert(id, Some(vec![PREFIX.into()]));
}

fn assert_terminal(outbox: &Path, id: &str, reason: &str) {
    assert_eq!(
        std::fs::read_to_string(outbox.join(format!("rejected/{id}.reason.txt"))).unwrap(),
        reason
    );
    assert!(!outbox.join(format!("delivered/{id}.json")).exists());
}

#[tokio::test]
async fn issue_3371_activated_known_failure_exact_reason_no_success_or_followup() {
    let (temp, _, backend, fixture, session) = activated(Some(false)).await;
    let mut poller = fixture.poller();
    let mut msg = message("positive");
    msg.body = "must never run".into();
    let source = save(temp.path(), &msg);
    assert!(matches!(
        fixture.claim(&mut poller, &source, msg),
        WakeHandoff::Spawned
    ));
    let id = submitted(&backend, &fixture).await;
    assert_eq!(Some(id), session);
    assert_eq!(fixture.submission_state(id).await, (true, false, false));
    assert!(
        probe_attempt(&std::fs::read_to_string(wake_claim_path(&source)).unwrap())
            .unwrap()
            .is_some()
    );
    assert_eq!(poller.wake_lanes.observing_count(), 1);
    assert_eq!(fixture.recorded_effect_counts()[4], 0);
    fixture.input_progress(id).await;
    failure_rows(&backend, id);
    finish(&mut poller).await;
    assert_terminal(temp.path(), "positive", KNOWN);
    assert!(!fixture.submission_state(id).await.1);
    assert_eq!(fixture.recorded_effect_counts()[4], 0);
    assert_eq!(
        backend
            .writes_for(id)
            .iter()
            .filter(|bytes| bytes.as_slice() == b"/clear")
            .count(),
        1
    );
    assert!(!backend
        .writes_for(id)
        .iter()
        .any(|bytes| String::from_utf8_lossy(bytes).contains("must never run")));
    assert!(*backend.captures.lock().unwrap().get(&id).unwrap() >= 4);
    let before = fixture.recorded_effect_counts();
    fixture.poll(&mut poller).await.unwrap();
    fixture.poll(&mut fixture.poller()).await.unwrap();
    assert_eq!(fixture.recorded_effect_counts(), before);
}

#[tokio::test]
async fn issue_3371_activated_baseline_loss_and_route_replacement_fail_closed() {
    for baseline in [true, false] {
        let (temp, _, backend, fixture, session) = activated(Some(false)).await;
        let id = session.unwrap();
        if baseline {
            backend.rows.lock().unwrap().insert(id, None);
        }
        let mut poller = fixture.poller();
        let msg = message(if baseline { "baseline" } else { "replacement" });
        let source = save(temp.path(), &msg);
        assert!(matches!(
            fixture.claim(&mut poller, &source, msg.clone()),
            WakeHandoff::Spawned
        ));
        if !baseline {
            assert_eq!(submitted(&backend, &fixture).await, id);
            fixture.remove_route(id);
        }
        finish(&mut poller).await;
        assert_terminal(temp.path(), &msg.id, INCOMPLETE);
        assert_eq!(fixture.recorded_effect_counts()[4], 0);
        assert!(!fixture.submission_state(id).await.1);
    }
}

#[tokio::test]
async fn issue_3371_fresh_and_resumed_real_watch_and_publication_only_recovery() {
    for seed in [None, Some(true)] {
        for healthy in [false, true] {
            let (temp, _, backend, fixture, old) = activated(seed).await;
            let mut poller = fixture.poller();
            let msg = message("fallback");
            let source = save(temp.path(), &msg);
            assert!(matches!(
                fixture.claim(&mut poller, &source, msg),
                WakeHandoff::Spawned
            ));
            let id = submitted(&backend, &fixture).await;
            assert_ne!(Some(id), old);
            assert!(backend.live.lock().unwrap().contains(&id));
            assert_eq!(poller.wake_lanes.observing_count(), 1);
            assert_eq!(fixture.recorded_effect_counts()[0], 1);
            if !healthy {
                failure_rows(&backend, id);
            }
            finish(&mut poller).await;
            if healthy {
                assert!(temp.path().join("delivered/fallback.json").exists());
                assert!(fixture.submission_state(id).await.1);
                assert_eq!(fixture.recorded_effect_counts()[4], 1);
            } else {
                assert_terminal(temp.path(), "fallback", KNOWN);
            }
            let before = fixture.recorded_effect_counts();
            fixture.poll(&mut poller).await.unwrap();
            fixture.poll(&mut fixture.poller()).await.unwrap();
            assert_eq!(fixture.recorded_effect_counts(), before);
            assert_eq!(
                backend.writes_for(id),
                vec![b"/clear".to_vec(), vec![13], vec![13]]
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn issue_3371_activated_reason_failure_then_restart_never_spawns_or_types_twice() {
    for seed in [Some(false), None, Some(true)] {
        for json_failure in [false, true] {
            let (temp, _, backend, fixture, _) = activated(seed).await;
            let mut poller = fixture.poller();
            let msg = message("publication");
            let source = save(temp.path(), &msg);
            let blocked = temp.path().join(if json_failure {
                "rejected/publication.json"
            } else {
                "rejected/publication.reason.txt"
            });
            if json_failure {
                std::fs::create_dir_all(blocked.parent().unwrap()).unwrap();
            } else {
                std::fs::create_dir_all(&blocked).unwrap();
            }
            assert!(matches!(
                fixture.claim(&mut poller, &source, msg),
                WakeHandoff::Spawned
            ));
            let id = submitted(&backend, &fixture).await;
            failure_rows(&backend, id);
            let fault = json_failure.then(|| MailboxFaultGuard::install_document_write(&blocked));
            finish(&mut poller).await;
            assert!(fault.as_ref().is_none_or(|fault| fault.consumed()));
            drop(fault);
            if json_failure {
                assert!(source.exists() && !blocked.exists());
                assert_eq!(
                    std::fs::read_to_string(blocked.with_extension("reason.txt")).unwrap(),
                    KNOWN
                );
            }
            assert!(!wake_claim_path(&source).exists());
            assert!(probe_attempt(&std::fs::read_to_string(&source).unwrap())
                .unwrap()
                .is_some());
            let before = fixture.recorded_effect_counts();
            let fault = json_failure.then(|| MailboxFaultGuard::install_document_write(&blocked));
            fixture.poll(&mut poller).await.unwrap();
            assert!(fault.as_ref().is_none_or(|fault| fault.consumed()));
            drop(fault);
            if json_failure {
                assert!(source.exists() && !blocked.exists());
                assert_eq!(
                    std::fs::read_to_string(blocked.with_extension("reason.txt")).unwrap(),
                    KNOWN
                );
            }
            let mut restart = fixture.poller();
            std::fs::rename(&source, wake_claim_path(&source)).unwrap();
            fixture.poll(&mut restart).await.unwrap();
            assert!(source.exists() && !wake_claim_path(&source).exists());
            let fault = json_failure.then(|| MailboxFaultGuard::install_document_write(&blocked));
            fixture.poll(&mut restart).await.unwrap();
            assert!(fault.as_ref().is_none_or(|fault| fault.consumed()));
            drop(fault);
            assert_eq!(fixture.recorded_effect_counts(), before);
            if json_failure {
                assert!(source.exists() && !blocked.exists());
                assert!(source.exists());
                assert_eq!(
                    std::fs::read_to_string(blocked.with_extension("reason.txt")).unwrap(),
                    KNOWN
                );
            } else {
                std::fs::remove_dir(&blocked).unwrap();
            }
            fixture.poll(&mut restart).await.unwrap();
            assert!(!source.exists());
            assert_eq!(fixture.recorded_effect_counts(), before);
            assert_eq!(
                backend.writes_for(id),
                vec![b"/clear".to_vec(), vec![13], vec![13]]
            );
            if json_failure {
                assert_terminal(temp.path(), "publication", KNOWN);
            } else {
                assert_terminal(
                    temp.path(),
                    "publication",
                    crate::phone::mailbox::clear_attempt::REPLAY_REASON,
                );
            }
        }
    }
}

#[tokio::test]
async fn issue_3371_actual_mailbox_same_recipient_waits_for_healthy_and_failed_verdict() {
    for healthy in [false, true] {
        let (temp, _, backend, fixture, session) = activated(Some(false)).await;
        let id = session.unwrap();
        let mut poller = fixture.poller();
        let clear = message("lane-clear");
        save(temp.path(), &clear);
        fixture.poll(&mut poller).await.unwrap();
        assert_eq!(submitted(&backend, &fixture).await, id);
        let mut task = message("lane-task");
        task.command = None;
        task.body = "task X".into();
        let source = save(temp.path(), &task);
        for _ in 0..4 {
            fixture.poll(&mut poller).await.unwrap();
            poller.drain_wake_outcomes().await;
            assert!(source.exists());
            assert_eq!(fixture.recorded_effect_counts()[1], 1);
            assert_eq!(fixture.recorded_effect_counts()[4], 0);
        }
        if !healthy {
            failure_rows(&backend, id);
        }
        finish(&mut poller).await;
        fixture.mark_ready(id).await;
        fixture.poll(&mut poller).await.unwrap();
        finish(&mut poller).await;
        assert!(temp.path().join("delivered/lane-task.json").exists());
        assert_eq!(fixture.recorded_effect_counts()[1], 2);
        assert_eq!(
            fixture.recorded_effect_counts()[4],
            1 + usize::from(healthy)
        );
        assert_eq!(
            backend
                .writes_for(id)
                .iter()
                .filter(|bytes| String::from_utf8_lossy(bytes).contains("task X"))
                .count(),
            1
        );
    }
}

fn additional_recipient(root: &Path, name: &str) -> PathBuf {
    let ac = root.join("proj-a/.ac");
    let replica = ac.join(format!("wg-1-dev-team/__agent_{name}"));
    std::fs::create_dir_all(ac.join(format!("_agent_{name}"))).unwrap();
    std::fs::create_dir_all(&replica).unwrap();
    std::fs::write(
        replica.join("config.json"),
        format!(r#"{{"identity":"../../_agent_{name}"}}"#),
    )
    .unwrap();
    replica
}

#[tokio::test]
async fn issue_3371_actual_nine_observers_allow_other_codex_and_non_codex_delivery() {
    let (temp, _, backend, fixture, _) = activated(None).await;
    let mut poller = fixture.poller();
    let mut ids = Vec::new();
    let members: Vec<_> = (0..11)
        .map(|slot| format!("../_agent_member-{slot}"))
        .collect();
    std::fs::write(
        temp.path().join("proj-a/.ac/_team_dev-team/config.json"),
        serde_json::to_vec(&serde_json::json!({"agents": members})).unwrap(),
    )
    .unwrap();
    for slot in 0..11 {
        let target = additional_recipient(temp.path(), &format!("member-{slot}"));
        ids.push(
            fixture
                .real_session(&target, if slot == 10 { "claude" } else { "codex" })
                .await,
        );
    }
    for (slot, id) in ids.iter().take(9).enumerate() {
        let mut msg = message(&format!("clear-{slot}"));
        msg.to = format!("proj-a:wg-1-dev-team/member-{slot}");
        save(temp.path(), &msg);
        fixture.poll(&mut poller).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while poller.wake_lanes.observing_count() != slot + 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(fixture.submission_state(*id).await, (true, false, false));
        if slot == 0 {
            let mut msg = message("below-cap");
            msg.to = "proj-a:wg-1-dev-team/member-9".into();
            msg.command = None;
            msg.body = "ordinary below cap".into();
            save(temp.path(), &msg);
            fixture.poll(&mut poller).await.unwrap();
            await_delivered(&fixture, &mut poller, temp.path(), "below-cap").await;
            assert_eq!(poller.wake_lanes.observing_count(), 1);
        }
    }
    assert!(poller.wake_workers.len() >= 9);
    assert!(poller.live_claims.len() >= 9);
    for slot in [9, 10] {
        fixture.mark_ready(ids[slot]).await;
        let mut msg = message(&format!("ordinary-{slot}"));
        msg.to = format!("proj-a:wg-1-dev-team/member-{slot}");
        msg.command = None;
        msg.body = "ordinary at cap".into();
        save(temp.path(), &msg);
        fixture.poll(&mut poller).await.unwrap();
        await_delivered(&fixture, &mut poller, temp.path(), &msg.id).await;
        assert_eq!(poller.wake_lanes.observing_count(), 9);
    }
    for id in &ids[..9] {
        failure_rows(&backend, *id);
    }
    finish(&mut poller).await;
    for slot in 0..9 {
        assert_terminal(temp.path(), &format!("clear-{slot}"), KNOWN);
    }
}

async fn await_delivered(
    _fixture: &CodexClearScannerFixture,
    poller: &mut MailboxPoller,
    root: &Path,
    id: &str,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !root.join(format!("delivered/{id}.json")).exists() {
            poller.drain_wake_outcomes().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("ordinary delivery before releasing any clear observation");
}

#[tokio::test]
async fn issue_3371_activated_receipt_before_remove_failure_replays_cleanup_only() {
    for healthy in [false, true] {
        let (temp, _, backend, fixture, _) = activated(Some(false)).await;
        let mut poller = fixture.poller();
        let msg = message("remove-failure");
        let source = save(temp.path(), &msg);
        fixture.claim(&mut poller, &source, msg);
        let id = submitted(&backend, &fixture).await;
        let claim = wake_claim_path(&source);
        let marked = std::fs::read(&claim).unwrap();
        std::fs::remove_file(&claim).unwrap();
        std::fs::create_dir(&claim).unwrap();
        if !healthy {
            failure_rows(&backend, id);
        }
        finish(&mut poller).await;
        let archive = if healthy { "delivered" } else { "rejected" };
        let receipt = temp.path().join(format!("{archive}/remove-failure.json"));
        let bytes = std::fs::read(&receipt).unwrap();
        assert!(
            source.is_dir(),
            "remove failed and worker restored claim directory"
        );
        std::fs::remove_dir(&source).unwrap();
        std::fs::write(&source, marked).unwrap();
        let before = fixture.recorded_effect_counts();
        fixture.poll(&mut fixture.poller()).await.unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read(receipt).unwrap(), bytes);
        assert_eq!(fixture.recorded_effect_counts(), before);
        assert_eq!(
            backend.writes_for(id),
            vec![b"/clear".to_vec(), vec![13], vec![13]]
        );
        if !healthy {
            assert_terminal(temp.path(), "remove-failure", KNOWN);
        }
    }
}

#[tokio::test]
async fn issue_3371_old_failure_baseline_and_reappearance_are_distinct() {
    for reappear in [false, true] {
        let (temp, _, backend, fixture, session) = activated(Some(false)).await;
        let id = session.unwrap();
        failure_rows(&backend, id);
        let mut poller = fixture.poller();
        let msg = message("old-prefix");
        let source = save(temp.path(), &msg);
        fixture.claim(&mut poller, &source, msg);
        submitted(&backend, &fixture).await;
        if reappear {
            backend.rows.lock().unwrap().insert(id, Some(Vec::new()));
            let captures = *backend.captures.lock().unwrap().get(&id).unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                while *backend.captures.lock().unwrap().get(&id).unwrap() == captures {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            failure_rows(&backend, id);
        }
        finish(&mut poller).await;
        if reappear {
            assert_terminal(temp.path(), "old-prefix", KNOWN);
        } else {
            assert!(temp.path().join("delivered/old-prefix.json").exists());
        }
    }
}

#[tokio::test]
async fn issue_3371_healthy_watch_body_runs_once_after_verdict_and_delivered_failure_fences() {
    for blocked in [false, true] {
        let (temp, _, backend, fixture, _) = activated(Some(false)).await;
        let mut poller = fixture.poller();
        let mut msg = message("healthy-body");
        if !blocked {
            msg.body = "followup exactly once".into();
        }
        let source = save(temp.path(), &msg);
        let blocker = temp.path().join("delivered/healthy-body.json");
        if blocked {
            std::fs::create_dir_all(&blocker).unwrap();
        }
        let started = std::time::Instant::now();
        fixture.claim(&mut poller, &source, msg);
        let id = submitted(&backend, &fixture).await;
        eprintln!("native admission through T2: {:?}", started.elapsed());
        assert_eq!(fixture.recorded_effect_counts()[4], 0);
        assert_eq!(backend.writes_for(id).len(), 3);
        finish(&mut poller).await;
        assert_eq!(fixture.recorded_effect_counts()[4], 1);
        if blocked {
            assert!(source.is_file());
            std::fs::remove_dir(&blocker).unwrap();
            fixture.poll(&mut fixture.poller()).await.unwrap();
            assert_terminal(
                temp.path(),
                "healthy-body",
                crate::phone::mailbox::clear_attempt::REPLAY_REASON,
            );
            assert_eq!(backend.writes_for(id).len(), 3);
        } else {
            assert!(blocker.is_file());
            assert!(fixture.submission_state(id).await.1);
            fixture.mark_ready(id).await;
            tokio::time::timeout(Duration::from_secs(5), async {
                while backend.writes_for(id).len() < 6 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(backend.writes_for(id).iter().filter(|bytes|
                String::from_utf8_lossy(bytes).contains("followup exactly once")).count(), 1);
        }
    }
}
