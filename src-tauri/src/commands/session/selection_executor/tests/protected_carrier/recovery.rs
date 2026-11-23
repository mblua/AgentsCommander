//! Recovery and negative controls.

use super::super::native_fixture::NativeExitMode;
use super::closing::{exact, force, logger, owner, owners, records};
use super::{complete_isolated, run_isolated, PollLog, ProtectedFixture};
use crate::commands::session::destroy::DestroyCompletion;
use crate::config::sessions_persistence::{observe_discard_veto, DiscardVetoReason};
use crate::phone::mailbox::{force_close, TeardownOutcome};
use crate::session::manager::acquire_carrier_mutation_permit;
use crate::session::selection::response_close::{DiscardHold, ResponseEffectOwner, StopLane};
use crate::session::selection::{
    response_close_registration, CriticalAdmissionOutcome, DestroyRequest, DestructionSource,
};
use crate::telegram::bridge::{ReaderDestSender, ReaderTask};
use crate::telegram::manager::{ReaderConsumer, ReaderEntry, TelegramBridgeState};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Manager;
use tokio_util::sync::CancellationToken;

const PREFIX: &str = "commands::session::selection_executor::tests::protected_carrier::recovery::";

/// The production `purge-room`/`close-session` discard path.
async fn discard_via_mailbox(f: &ProtectedFixture) -> TeardownOutcome {
    let n = &f.native;
    force_close(
        n.manager.read().await.clone(),
        n.coordinator.clone(),
        n.source,
        Arc::new(AtomicBool::new(false)),
    )
    .await
}

fn row_exists(f: &ProtectedFixture) -> bool {
    let id = f.native.source.to_string();
    crate::config::sessions_persistence::load_sessions_raw_from_dir_for_test(&f.carrier.directory)
        .iter()
        .any(|row| row.id.as_deref() == Some(id.as_str()))
}

/// Every hold leaves protection, tracker close, owner and registration in place.
async fn still_held(f: &ProtectedFixture) {
    let n = &f.native;
    assert!(owners(f)
        .iter()
        .any(|owner| owner.registration().member.session_id == n.source));
    assert!(response_close_registration(n.source).is_some());
    let permit = acquire_carrier_mutation_permit().await;
    let observed = observe_discard_veto(&permit, n.source);
    drop(permit);
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].reason, DiscardVetoReason::ProtectionClosing);
    assert_eq!(observed[0].protection_nonce, Some(f.carrier.nonce));
    assert!(row_exists(f));
}

async fn held(f: &ProtectedFixture, log: &PollLog, code: &str) {
    let outcome = discard_via_mailbox(f).await;
    let TeardownOutcome::Failed(reason) = &outcome else {
        panic!("discard must hold: {outcome:?}");
    };
    assert!(reason.contains("response_close_deferred"), "{reason}");
    let recovery = records(log, "discard_recovery");
    let newest = recovery.last().expect("missing discard_recovery");
    exact(newest, "outcome", "held");
    exact(newest, "reason", code);
    exact(newest, "session", f.native.source);
    still_held(f).await;
}

/// Polls the real verdict until `done`; the 10 s cap only fails the test.
async fn poll_until(
    owner: &ResponseEffectOwner,
    done: impl Fn(Result<(), DiscardHold>) -> bool,
) -> Result<(), DiscardHold> {
    let started = Instant::now();
    loop {
        let verdict = owner.discard_quiescence();
        if done(verdict) {
            return verdict;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "quiescence poll cap: {verdict:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn set_mode(f: &ProtectedFixture, mode: NativeExitMode) {
    f.native.backend.set_native_exit(f.native.source, mode);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3317_quiescent_closing_discard_recovers() {
    let name = format!("{PREFIX}issue_3317_quiescent_closing_discard_recovers");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        set_mode(&f, NativeExitMode::Observed);
        f.retire().await;
        let n = &f.native;
        let owner = owner(&f);
        assert_eq!(owner.discard_quiescence(), Ok(()));
        let binding = owner.ticket().committed_binding();
        drop(owner);
        // v5: a configured Root Agent dir lets the destroy prove this is not the Root Agent.
        std::fs::create_dir_all(
            crate::config::config_dir()
                .unwrap()
                .join(crate::config::ROOT_AGENT_DIR_NAME),
        )
        .unwrap();
        let vetoes = records(&log, "discard_veto").len();
        assert_eq!(discard_via_mailbox(&f).await, TeardownOutcome::Closed);
        assert!(owners(&f).is_empty());
        assert!(response_close_registration(n.source).is_none());
        let permit = acquire_carrier_mutation_permit().await;
        let observed = observe_discard_veto(&permit, n.source);
        drop(permit);
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].reason, DiscardVetoReason::BlockerUnavailable);
        let manager = n.manager.read().await.clone();
        assert!(manager.get_session(n.source).await.is_none());
        assert!(!row_exists(&f));
        let tracker = n.app.state::<crate::pty::manager::ResponseCloseState>();
        assert!(tracker.is_healthy());
        assert!(!tracker.target_is_closing(&f.carrier.target));
        assert!(tracker.generation(n.source).is_none());
        let retired: Vec<_> = records(&log, "discard_recovery")
            .into_iter()
            .filter(|record| record.get("outcome").is_some_and(|v| v == "retired"))
            .collect();
        assert_eq!(retired.len(), 1);
        exact(&retired[0], "session", n.source);
        exact(&retired[0], "close_nonce", binding.close_nonce);
        exact(&retired[0], "generation", binding.member.generation);
        assert_eq!(records(&log, "discard_veto").len(), vetoes);
        n.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3317_manual_close_discard_recovers() {
    let name = format!("{PREFIX}issue_3317_manual_close_discard_recovers");
    run_isolated(&name, async {
        let _log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        set_mode(&f, NativeExitMode::Observed);
        f.retire().await;
        let n = &f.native;
        // v5: a configured Root Agent dir lets the destroy prove this is not the Root Agent.
        std::fs::create_dir_all(
            crate::config::config_dir()
                .unwrap()
                .join(crate::config::ROOT_AGENT_DIR_NAME),
        )
        .unwrap();
        let completion = crate::commands::session::execute_destroy_transaction_common(
            &n.transaction(),
            DestroyRequest {
                ids: vec![n.source],
                source: DestructionSource::ManualClose,
                force_destroy_root: false,
            },
            None,
        )
        .await
        .unwrap();
        let DestroyCompletion::Legacy(outcome) = completion else {
            panic!("must finish the full legacy transaction");
        };
        assert_eq!(outcome.destroyed_ids, vec![n.source]);
        assert!(outcome.failed.is_empty());
        assert!(outcome.retained_exited_ids.is_empty());
        assert!(owners(&f).is_empty());
        assert!(response_close_registration(n.source).is_none());
        n.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3317_running_reaper_keeps_veto() {
    let name = format!("{PREFIX}issue_3317_running_reaper_keeps_veto");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        let (gate, reaping) = std::sync::mpsc::channel();
        set_mode(&f, NativeExitMode::Reaping(reaping));
        f.retire().await;
        held(&f, &log, "native_exit_running").await;
        gate.send(()).unwrap();
        assert_eq!(poll_until(&owner(&f), |v| v.is_ok()).await, Ok(()));
        assert_eq!(discard_via_mailbox(&f).await, TeardownOutcome::Closed);
        f.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3317_failed_reap_is_ambiguous() {
    let name = format!("{PREFIX}issue_3317_failed_reap_is_ambiguous");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        set_mode(&f, NativeExitMode::ReapFails);
        f.retire().await;
        let verdict = poll_until(&owner(&f), |v| v != Err(DiscardHold::NativeExitRunning)).await;
        assert_eq!(verdict, Err(DiscardHold::NativeExitAmbiguous));
        held(&f, &log, "native_exit_ambiguous").await;
        f.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3317_missing_exit_evidence_keeps_veto() {
    let name = format!("{PREFIX}issue_3317_missing_exit_evidence_keeps_veto");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        f.retire().await;
        held(&f, &log, "native_exit_missing").await;
        f.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3317_running_bridge_stop_keeps_veto() {
    let name = format!("{PREFIX}issue_3317_running_bridge_stop_keeps_veto");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        set_mode(&f, NativeExitMode::Observed);
        let n = &f.native;
        let (gate, gate_rx) = std::sync::mpsc::channel::<()>();
        {
            let tg = n.app.state::<TelegramBridgeState>();
            let mut tg = tg.lock().await;
            tg.insert_test_bridge(n.source, "bot-3317");
            // A blocking task cannot be aborted, so the stop stays running past its deadline.
            tg.insert_test_bridge_task_for_test(
                n.source,
                tokio::task::spawn_blocking(move || {
                    let _ = gate_rx.recv();
                }),
            );
        }
        f.retire().await;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert_eq!(
            owner(&f).discard_quiescence(),
            Err(DiscardHold::TelegramRunning)
        );
        held(&f, &log, "telegram_running").await;
        gate.send(()).unwrap();
        assert_eq!(poll_until(&owner(&f), |v| v.is_ok()).await, Ok(()));
        assert_eq!(discard_via_mailbox(&f).await, TeardownOutcome::Closed);
        n.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3317_running_reader_stop_keeps_veto() {
    let name = format!("{PREFIX}issue_3317_running_reader_stop_keeps_veto");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        set_mode(&f, NativeExitMode::Observed);
        let n = &f.native;
        let (gate, gate_rx) = std::sync::mpsc::channel::<()>();
        {
            let tg = n.app.state::<TelegramBridgeState>();
            let mut tg = tg.lock().await;
            let reader_id = tg.next_reader_id();
            tg.reader_install(
                n.source,
                ReaderEntry::new(
                    reader_id,
                    ReaderTask {
                        cancel: CancellationToken::new(),
                        tasks: vec![tokio::task::spawn_blocking(move || {
                            let _ = gate_rx.recv();
                        })],
                        dest: ReaderDestSender::Claude(tokio::sync::watch::channel(None).0),
                        frontier: Arc::new(std::sync::Mutex::new(None)),
                    },
                ),
                ReaderConsumer::Bot,
            );
        }
        f.retire().await;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert_eq!(
            owner(&f).discard_quiescence(),
            Err(DiscardHold::ReadersRunning)
        );
        held(&f, &log, "readers_running").await;
        gate.send(()).unwrap();
        assert_eq!(poll_until(&owner(&f), |v| v.is_ok()).await, Ok(()));
        assert_eq!(discard_via_mailbox(&f).await, TeardownOutcome::Closed);
        n.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3317_recorder_running_task_holds() {
    let name = format!("{PREFIX}issue_3317_recorder_running_task_holds");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        set_mode(&f, NativeExitMode::Observed);
        f.retire().await;
        let owner = owner(&f);
        assert_eq!(owner.discard_quiescence(), Ok(()));
        let pending = || Ok(Some(tokio::spawn(std::future::pending::<bool>())));
        owner.clear_task_lane_for_test(StopLane::Telegram);
        owner
            .record_task_stop(StopLane::Telegram, pending())
            .unwrap();
        assert_eq!(
            owner.discard_quiescence(),
            Err(DiscardHold::TelegramRunning)
        );
        owner.clear_task_lane_for_test(StopLane::Telegram);
        owner
            .record_task_stop(StopLane::Telegram, Ok(None))
            .unwrap();
        owner.clear_task_lane_for_test(StopLane::Readers);
        owner
            .record_task_stop(StopLane::Readers, pending())
            .unwrap();
        assert_eq!(owner.discard_quiescence(), Err(DiscardHold::ReadersRunning));
        held(&f, &log, "readers_running").await;
        f.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3317_ambiguous_reader_stop_keeps_veto() {
    let name = format!("{PREFIX}issue_3317_ambiguous_reader_stop_keeps_veto");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        set_mode(&f, NativeExitMode::Observed);
        f.retire().await;
        let owner = owner(&f);
        owner.clear_task_lane_for_test(StopLane::Readers);
        owner
            .record_task_stop(StopLane::Readers, Ok(Some(tokio::spawn(async { false }))))
            .unwrap();
        let verdict = poll_until(&owner, |v| v != Err(DiscardHold::ReadersRunning)).await;
        assert_eq!(verdict, Err(DiscardHold::ReadersAmbiguous));
        held(&f, &log, "readers_ambiguous").await;
        f.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3317_poisoned_owner_keeps_veto() {
    let name = format!("{PREFIX}issue_3317_poisoned_owner_keeps_veto");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        set_mode(&f, NativeExitMode::Observed);
        f.retire().await;
        owner(&f).poison_record_for_test();
        held(&f, &log, "owner_record_unreadable").await;
        f.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3317_background_discard_keeps_veto() {
    let name = format!("{PREFIX}issue_3317_background_discard_keeps_veto");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        set_mode(&f, NativeExitMode::Observed);
        f.retire().await;
        let n = &f.native;
        let CriticalAdmissionOutcome::Completed(outcome) =
            n.coordinator.background_destroy(n.source).await.unwrap()
        else {
            panic!("background destroy must complete");
        };
        assert_eq!(
            outcome.failed,
            vec![(n.source, "response_close_deferred".to_owned())]
        );
        let recovery = records(&log, "discard_recovery");
        let newest = recovery.last().expect("missing discard_recovery");
        exact(newest, "outcome", "held");
        exact(newest, "reason", "not_explicit_discard");
        exact(newest, "session", n.source);
        still_held(&f).await;
        n.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}
