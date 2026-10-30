use super::CodexClearScannerFixture;
use crate::errors::AppError;
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
    tokio::time::timeout(Duration::from_secs(5), async {
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
            assert_eq!(poller.retry_tracker[&source].attempts, 1);
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
        assert_eq!(poller.retry_tracker[&source].attempts, 1);
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
