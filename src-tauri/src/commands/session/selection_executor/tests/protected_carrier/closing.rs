//! Causal reproduction and nonauthoritative diagnostics; no recovery assertions.

use super::{complete_isolated, run_isolated, PollLog, PollLogger, ProtectedFixture};
use crate::commands::session::destroy::{reconcile_response_closes, DestroyCompletion};
use crate::config::sessions_persistence::{
    begin_carrier_replacement, observe_discard_veto, poison_discard_slot_for_test,
    retained_carrier_for_replacement, DiscardVetoObservation, DiscardVetoReason,
};
use crate::pty::backend::PtyBackend;
use crate::session::manager::acquire_carrier_mutation_permit;
use crate::session::selection::response_close::ResponseEffectOwner;
use crate::session::selection::{DestroyRequest, DestructionSource};
use crate::session::session::SessionStatus;
use futures::FutureExt;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;
use uuid::Uuid;

const PREFIX: &str = "commands::session::selection_executor::tests::protected_carrier::closing::";

fn owners(f: &ProtectedFixture) -> Vec<Arc<ResponseEffectOwner>> {
    f.native
        .app
        .state::<crate::app_state::AppOwners>()
        .response_effects
        .snapshot()
        .unwrap()
}

fn owner(f: &ProtectedFixture) -> Arc<ResponseEffectOwner> {
    let owners = owners(f);
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].registration().member.session_id, f.native.source);
    owners[0].clone()
}

fn force(f: &ProtectedFixture) {
    f.native
        .app
        .state::<crate::app_state::AppOwners>()
        .response_effects
        .force_next_settlement_for_test(f.native.source);
}

fn logger() -> PollLog {
    let log = Arc::new(Mutex::new(Vec::new()));
    log::set_logger(Box::leak(Box::new(PollLogger(log.clone())))).unwrap();
    log::set_max_level(log::LevelFilter::Debug);
    log
}

fn records(log: &PollLog, stage: &str) -> Vec<BTreeMap<String, String>> {
    log.lock()
        .unwrap()
        .iter()
        .map(|line| {
            line.split_whitespace()
                .filter_map(|field| {
                    field
                        .split_once('=')
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .filter(|record| record.get("stage").is_some_and(|value| value == stage))
        .collect()
}

fn exact(record: &BTreeMap<String, String>, key: &str, value: impl ToString) {
    assert_eq!(
        record.get(key),
        Some(&value.to_string()),
        "{key}: {record:?}"
    );
}

async fn veto(f: &ProtectedFixture) -> Vec<DiscardVetoObservation> {
    let n = &f.native;
    let manager = n.manager.read().await.clone();
    let before = manager.get_session(n.source).await.unwrap();
    let kills = n.backend.kill_count(n.source);
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
    assert_eq!(
        outcome.failed,
        vec![(n.source, "response_close_deferred".to_owned())]
    );
    assert!(outcome.destroyed_ids.is_empty());
    assert!(outcome.retained_exited_ids.is_empty());
    let after = manager.get_session(n.source).await.unwrap();
    assert_eq!(after.id, before.id);
    assert_eq!(after.token, before.token);
    assert_eq!(after.status, before.status);
    assert_eq!(n.backend.kill_count(n.source), kills);
    let permit = acquire_carrier_mutation_permit().await;
    observe_discard_veto(&permit, n.source)
}

async fn closing(f: &ProtectedFixture, terminal: bool) -> Arc<ResponseEffectOwner> {
    let owner = owner(f);
    assert_eq!(owner.diagnostic_terminal(), Some(terminal));
    let observed = veto(f).await;
    assert_eq!(observed.len(), 1);
    let observed = &observed[0];
    assert_eq!(observed.reason, DiscardVetoReason::ProtectionClosing);
    assert_eq!(observed.phase, Some("Closing"));
    assert_eq!(observed.session, f.native.source);
    assert_eq!(observed.protected_session, Some(f.native.source));
    assert_eq!(observed.target.as_ref(), Some(&f.carrier.target));
    assert_eq!(observed.protection_nonce, Some(f.carrier.nonce));
    assert_eq!(observed.token, Some(f.carrier.token));
    assert!(observed.epoch.is_some());
    assert!(observed.version.is_some());
    assert!(Arc::ptr_eq(&owner, &self::owner(f)));
    assert!(crate::session::selection::response_close_registration(f.native.source).is_some());
    owner
}

async fn dormant(f: &ProtectedFixture) {
    let n = &f.native;
    assert_eq!(n.backend.kill_count(n.source), 1);
    assert!(!n.backend.has_session(n.source));
    assert!(owners(f).is_empty());
    assert!(crate::session::selection::response_close_registration(n.source).is_none());
    let manager = n.manager.read().await.clone();
    assert_eq!(
        manager.get_session(n.source).await.unwrap().status,
        SessionStatus::Exited(0)
    );
    let permit = acquire_carrier_mutation_permit().await;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            retained_carrier_for_replacement(
                &permit,
                &f.carrier.directory,
                &f.carrier.target,
                n.source,
                current,
            )
            .expect("normal settlement minted real retained Dormant history");
        })
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3248_terminal_settlement_rejection() {
    let name = format!("{PREFIX}issue_3248_terminal_settlement_rejection");
    run_isolated(&name, async {
        let f = ProtectedFixture::new(false).await;
        force(&f);
        f.retire().await;
        let n = &f.native;
        assert!(!n.backend.has_session(n.source));
        assert_eq!(
            n.manager
                .read()
                .await
                .get_session(n.source)
                .await
                .unwrap()
                .status,
            SessionStatus::Exited(0)
        );
        let owner = closing(&f, true).await;
        let binding = owner.ticket().committed_binding();
        reconcile_response_closes(n.app.handle()).await;
        let retry = closing(&f, true).await;
        assert!(Arc::ptr_eq(&owner, &retry));
        assert!(retry.ticket().committed_binding() == binding);
        assert_eq!(n.backend.kill_count(n.source), 1);
        n.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3248_running_effect_veto() {
    let name = format!("{PREFIX}issue_3248_running_effect_veto");
    run_isolated(&name, async {
        tokio::task::LocalSet::new()
            .run_until(async {
                let f = std::rc::Rc::new(ProtectedFixture::new(false).await);
                let n = &f.native;
                let (entered, release) = n.backend.gate_kill(n.source);
                // A nonmatching session cannot consume the one-shot gate.
                n.backend.kill(Uuid::new_v4()).unwrap();
                let producer_fixture = f.clone();
                let producer = tokio::task::spawn_local(async move {
                    producer_fixture.retire().await;
                });
                let assertions = std::panic::AssertUnwindSafe(async {
                    tokio::time::timeout(Duration::from_secs(10), entered)
                        .await
                        .unwrap()
                        .unwrap();
                    assert!(!producer.is_finished());
                    assert!(n.backend.has_session(n.source));
                    assert_eq!(n.backend.kill_count(n.source), 0);
                    let owner = closing(&f, false).await;
                    assert_eq!(owner.diagnostic_terminal(), Some(false));
                    assert!(!producer.is_finished());
                })
                .catch_unwind()
                .await;
                let released = release.send(());
                let joined = tokio::time::timeout(Duration::from_secs(40), producer).await;
                // Release and join the actual producer even if an assertion unwinds.
                n.coordinator.close_and_join().await;
                released.unwrap();
                joined.unwrap().unwrap();
                if let Err(panic) = assertions {
                    std::panic::resume_unwind(panic);
                }
                dormant(&f).await;
                complete_isolated(&name);
            })
            .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3248_fault_free_control() {
    let name = format!("{PREFIX}issue_3248_fault_free_control");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        f.retire().await;
        dormant(&f).await;
        assert!(records(&log, "settlement").is_empty());
        assert!(records(&log, "discard_veto").is_empty());
        reconcile_response_closes(f.native.app.handle()).await;
        assert_eq!(f.native.backend.kill_count(f.native.source), 1);
        f.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3248_diagnostic_reasons() {
    if std::env::var("AC_P17_CARRIER_CHILD").is_err() {
        tokio::task::spawn_blocking(diagnostic_preparing_control)
            .await
            .unwrap();
    }
    let name = format!("{PREFIX}issue_3248_diagnostic_reasons");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        f.retire().await;
        let owner = closing(&f, true).await;
        let binding = owner.ticket().committed_binding();
        for stage in ["settlement", "discard_veto"] {
            let records = records(&log, stage);
            assert!(!records.is_empty(), "missing {stage}");
            for record in records {
                exact(
                    &record,
                    "reason",
                    if stage == "settlement" {
                        "test_forced_settlement"
                    } else {
                        "protection_closing"
                    },
                );
                exact(&record, "session", f.native.source);
                exact(&record, "target", &f.carrier.target);
                exact(&record, "token", f.carrier.token);
                exact(&record, "protection_nonce", f.carrier.nonce);
                exact(&record, "close_nonce", binding.close_nonce);
                exact(&record, "generation", binding.member.generation);
                if stage == "discard_veto" {
                    exact(&record, "phase", "Closing");
                    exact(&record, "protected_session", f.native.source);
                    exact(&record, "owner_reason", "observed");
                    exact(&record, "recorded_terminal", true);
                }
            }
        }
        f.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_3248_diagnostic_unreadable() {
    if std::env::var("AC_P17_CARRIER_CHILD").is_err() {
        tokio::task::spawn_blocking(diagnostic_slot_unreadable_control)
            .await
            .unwrap();
    }
    let name = format!("{PREFIX}issue_3248_diagnostic_unreadable");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        f.retire().await;
        let owner = owner(&f);
        let binding = owner.ticket().committed_binding();
        owner.poison_record_for_test();
        log.lock().unwrap().clear();
        let observed = veto(&f).await;
        assert_eq!(observed[0].reason, DiscardVetoReason::ProtectionClosing);
        let records = records(&log, "discard_veto");
        assert_eq!(records.len(), 1);
        let record = &records[0];
        exact(record, "reason", "protection_closing");
        exact(record, "owner_reason", "owner_record_unreadable");
        exact(record, "recorded_terminal", "unknown");
        exact(record, "session", f.native.source);
        exact(record, "target", &f.carrier.target);
        exact(record, "token", f.carrier.token);
        exact(record, "protection_nonce", f.carrier.nonce);
        exact(record, "close_nonce", binding.close_nonce);
        exact(record, "generation", binding.member.generation);
        assert!(Arc::ptr_eq(&owner, &self::owner(&f)));
        assert_eq!(f.native.backend.kill_count(f.native.source), 1);
        f.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn diagnostic_preparing_control() {
    let name = format!("{PREFIX}diagnostic_preparing_control");
    run_isolated(&name, async {
        let log = logger();
        // The independent control reaches real Dormant before establishing Preparing.
        let p = ProtectedFixture::new(false).await;
        p.retire().await;
        dormant(&p).await;
        let manager = p.native.manager.read().await.clone();
        let permit = acquire_carrier_mutation_permit().await;
        let owner_nonce = Uuid::new_v4();
        let replacement = Uuid::new_v4();
        let carrier = manager
            .with_current_carrier_snapshot(&permit, |current| {
                let carrier = retained_carrier_for_replacement(
                    &permit,
                    &p.carrier.directory,
                    &p.carrier.target,
                    p.native.source,
                    current,
                )
                .unwrap();
                assert!(begin_carrier_replacement(
                    &permit,
                    &carrier,
                    owner_nonce,
                    replacement,
                    current,
                ));
                carrier
            })
            .await;
        let before = observe_discard_veto(&permit, p.native.source);
        drop(permit);
        log.lock().unwrap().clear();
        assert_eq!(veto(&p).await, before);
        let records = records(&log, "discard_veto");
        assert_eq!(records.len(), 1);
        let record = &records[0];
        exact(record, "reason", "protection_preparing");
        exact(record, "phase", "Preparing");
        exact(record, "session", p.native.source);
        exact(record, "protected_session", p.native.source);
        exact(record, "target", &carrier.target);
        exact(record, "token", carrier.token);
        exact(record, "protection_nonce", carrier.nonce);
        exact(record, "owner_nonce", owner_nonce);
        exact(record, "previous", p.native.source);
        exact(record, "replacement", replacement);
        exact(record, "close_nonce", "unknown");
        exact(record, "generation", "unknown");
        let permit = acquire_carrier_mutation_permit().await;
        assert_eq!(observe_discard_veto(&permit, p.native.source), before);
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                assert!(
                    retained_carrier_for_replacement(
                        &permit,
                        &carrier.directory,
                        &carrier.target,
                        p.native.source,
                        current,
                    )
                    .is_none(),
                    "Preparing must remain in place"
                );
            })
            .await;
        drop(permit);
        p.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn diagnostic_slot_unreadable_control() {
    let name = format!("{PREFIX}diagnostic_slot_unreadable_control");
    run_isolated(&name, async {
        let log = logger();
        let f = ProtectedFixture::new(false).await;
        force(&f);
        f.retire().await;
        // Slot poison is independently owned; lower unreadability cannot infer a phase.
        poison_discard_slot_for_test(&f.carrier);
        log.lock().unwrap().clear();
        let observed = veto(&f).await;
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].reason, DiscardVetoReason::SlotLockUnreadable);
        assert_eq!(observed[0].target, None);
        assert_eq!(observed[0].phase, None);
        assert_eq!(observed[0].protection_nonce, None);
        let records = self::records(&log, "discard_veto");
        assert_eq!(records.len(), 1);
        exact(&records[0], "reason", "slot_lock_unreadable");
        exact(&records[0], "session", f.native.source);
        for field in [
            "target",
            "phase",
            "protection_nonce",
            "token",
            "close_nonce",
            "generation",
            "protected_session",
            "epoch",
            "version",
            "recorded_terminal",
        ] {
            exact(&records[0], field, "unknown");
        }
        assert_eq!(f.native.backend.kill_count(f.native.source), 1);
        f.native.coordinator.close_and_join().await;
        complete_isolated(&name);
    })
    .await;
}
