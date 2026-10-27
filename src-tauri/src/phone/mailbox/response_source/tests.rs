use super::*;
use crate::config::settings::SettingsState;
use crate::pty::manager::{PtyManager, ResponseCloseState};
use crate::session::manager::SessionManager;
use futures::FutureExt;

#[derive(Default)]
struct AdmissionState {
    tracker: Option<ResponseCloseState>,
    accesses: Mutex<Vec<&'static str>>,
}

impl PtyAuthorityState for AdmissionState {
    fn pty_settings(&self) -> &SettingsState {
        self.accesses.lock().unwrap().push("settings");
        panic!("settings sentinel");
    }

    fn pty_sessions(&self) -> &Arc<tokio::sync::RwLock<SessionManager>> {
        panic!("unexpected sessions lookup");
    }

    fn pty_routes(&self) -> &Arc<Mutex<PtyManager>> {
        panic!("unexpected routes lookup");
    }
}

impl ResponseAdmissionState for AdmissionState {
    fn response_tracker(&self) -> Option<&ResponseCloseState> {
        self.accesses.lock().unwrap().push("tracker");
        self.tracker.as_ref()
    }
}

fn message() -> OutboxMessage {
    serde_json::from_value(serde_json::json!({
        "id": Uuid::new_v4().to_string(),
        "token": Uuid::new_v4().to_string(),
        "from": "project:room-22/coordinator",
        "to": "project:room-22/member",
        "body": "body",
        "timestamp": "2026-10-26T00:00:00Z"
    }))
    .unwrap()
}

async fn admitted(poller: &MailboxPoller, state: &AdmissionState) -> ResponseWakeWork {
    let work = poller
        .admit_response_source(state, Path::new("source.json"), &message(), false, "body")
        .await
        .unwrap();
    state.accesses.lock().unwrap().clear();
    work
}

#[tokio::test]
async fn prepare_rejects_missing_work_without_observations() {
    let state = AdmissionState::default();
    assert!(MailboxPoller::delivery_only_for_tests()
        .prepare_response_request(&state, &message(), Uuid::new_v4(), false, None)
        .await
        .is_none());
    assert!(state.accesses.lock().unwrap().is_empty());
}

#[tokio::test]
async fn prepare_rejects_excluded_classes_rooms_and_tokens_before_settings() {
    let state = AdmissionState {
        tracker: Some(ResponseCloseState::default()),
        ..Default::default()
    };
    let poller = MailboxPoller::delivery_only_for_tests();
    let work = admitted(&poller, &state).await;
    for case in 0..7 {
        let mut msg = message();
        match case {
            0 => {}
            1 => msg.command = Some("clear".into()),
            2 => msg.action = Some("action".into()),
            3 => msg.get_output = true,
            4 => msg.to = "project:room-23/member".into(),
            5 => msg.token = None,
            6 => msg.token = Some("malformed".into()),
            _ => unreachable!(),
        }
        assert!(poller
            .prepare_response_request(&state, &msg, Uuid::new_v4(), case == 0, Some(&work))
            .await
            .is_none());
        assert!(state.accesses.lock().unwrap().is_empty(), "case {case}");
    }
}

#[tokio::test]
async fn valid_prepare_reaches_settings_after_rejection_checks() {
    let state = AdmissionState {
        tracker: Some(ResponseCloseState::default()),
        ..Default::default()
    };
    let poller = MailboxPoller::delivery_only_for_tests();
    let work = admitted(&poller, &state).await;
    let msg = message();
    let result = std::panic::AssertUnwindSafe(poller.prepare_response_request(
        &state,
        &msg,
        Uuid::new_v4(),
        false,
        Some(&work),
    ))
    .catch_unwind()
    .await;
    let panic = result.err().expect("settings getter must be reached");
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"settings sentinel"));
    assert_eq!(*state.accesses.lock().unwrap(), ["settings"]);
}
#[tokio::test]
async fn absent_tracker_stops_admission_without_other_observations() {
    let state = AdmissionState::default();
    assert!(MailboxPoller::delivery_only_for_tests()
        .admit_response_source(&state, Path::new("source.json"), &message(), true, "body")
        .await
        .is_none());
    assert_eq!(*state.accesses.lock().unwrap(), ["tracker"]);
}

#[tokio::test]
async fn unauthenticated_admission_and_identical_replay_only_observe_tracker() {
    let tracker = ResponseCloseState::default();
    let state = AdmissionState {
        tracker: Some(tracker.clone()),
        ..Default::default()
    };
    let poller = MailboxPoller::delivery_only_for_tests();
    let path = Path::new("source.json");
    let msg = message();
    let first = poller
        .admit_response_source(&state, path, &msg, false, "body")
        .await
        .unwrap();
    assert_eq!(*state.accesses.lock().unwrap(), ["tracker"]);
    assert!(first.state.same_state(&tracker));
    assert_eq!(first.digest, crate::phone::types::sha256_hex(b"body"));
    let replay = poller
        .admit_response_source(&state, path, &msg, true, "body")
        .await
        .unwrap();
    assert_eq!(*state.accesses.lock().unwrap(), ["tracker", "tracker"]);
    assert!(replay.state.same_state(&tracker));
    assert!(Arc::ptr_eq(&first.attempt, &replay.attempt));
    assert_eq!(first.key, replay.key);
    assert_eq!(first.digest, replay.digest);
    assert!(replay.return_context.is_none());
    let mut reservation = tracker.reserve_work(&msg.to, &first.key).unwrap();
    assert!(!reservation.accept_durable(&first.digest).unwrap());
    assert!(tracker.is_healthy());
}

#[tokio::test]
async fn conflicting_replays_fail_closed_before_other_observations() {
    for change_target in [false, true] {
        let tracker = ResponseCloseState::default();
        let state = AdmissionState {
            tracker: Some(tracker.clone()),
            ..Default::default()
        };
        let poller = MailboxPoller::delivery_only_for_tests();
        let path = Path::new("source.json");
        let mut msg = message();
        let original = poller
            .admit_response_source(&state, path, &msg, false, "body")
            .await
            .unwrap();
        let content = if change_target {
            msg.to = "project:room-22/other".into();
            "body"
        } else {
            "changed"
        };
        assert!(poller
            .admit_response_source(&state, path, &msg, true, content)
            .await
            .is_none());
        assert_eq!(*state.accesses.lock().unwrap(), ["tracker", "tracker"]);
        assert!(!tracker.is_healthy());
        let sources = poller.response_sources.lock().unwrap();
        assert_eq!(sources[path].digest, original.digest);
        assert!(Arc::ptr_eq(&sources[path].attempt, &original.attempt));
    }
}

#[derive(Default)]
struct CompletionState {
    clones: Arc<std::sync::atomic::AtomicUsize>,
}

impl Clone for CompletionState {
    fn clone(&self) -> Self {
        self.clones
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self {
            clones: Arc::clone(&self.clones),
        }
    }
}

impl PtyAuthorityState for CompletionState {
    fn pty_settings(&self) -> &SettingsState {
        panic!("unexpected completion settings lookup");
    }

    fn pty_sessions(&self) -> &Arc<tokio::sync::RwLock<SessionManager>> {
        panic!("unexpected completion sessions lookup");
    }

    fn pty_routes(&self) -> &Arc<Mutex<PtyManager>> {
        panic!("unexpected completion routes lookup");
    }
}

impl ResponseCompletionState for CompletionState {
    fn response_selection(&self) -> Option<&crate::session::selection::SelectionCoordinator> {
        panic!("unexpected completion selection lookup");
    }
}

#[tokio::test]
async fn missing_completion_entry_does_not_clone_or_observe_state() {
    let state = CompletionState::default();
    MailboxPoller::delivery_only_for_tests()
        .complete_response_delivery(&state, Path::new("missing.json"))
        .await;
    assert_eq!(state.clones.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn missing_completion_attempt_returns_before_observations() {
    let state = CompletionState::default();
    let poller = MailboxPoller::delivery_only_for_tests();
    let admission = AdmissionState {
        tracker: Some(ResponseCloseState::default()),
        ..Default::default()
    };
    let work = admitted(&poller, &admission).await;
    assert!(work.attempt.lock().unwrap().is_none());
    MailboxPoller::complete_response_request(&state, &work).await;
    assert_eq!(state.clones.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn missing_completion_candidate_returns_before_observations() {
    let state = CompletionState::default();
    let poller = MailboxPoller::delivery_only_for_tests();
    let admission = AdmissionState {
        tracker: Some(ResponseCloseState::default()),
        ..Default::default()
    };
    let work = admitted(&poller, &admission).await;
    *work.attempt.lock().unwrap() = Some(Arc::new(
        super::super::pty_claim::ResponseCandidateAttempt {
            state: work.state.clone(),
            candidate: Mutex::new(None),
            armed: std::sync::atomic::AtomicBool::new(true),
        },
    ));
    MailboxPoller::complete_response_request(&state, &work).await;
    assert_eq!(state.clones.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ordinary_completion_clones_once_settles_and_removes_only_its_origin() {
    let state = CompletionState::default();
    let tracker = ResponseCloseState::default();
    let admission = AdmissionState {
        tracker: Some(tracker.clone()),
        ..Default::default()
    };
    let poller = MailboxPoller::delivery_only_for_tests();
    let work = admitted(&poller, &admission).await;
    let other_path = Path::new("other.json");
    let other = poller
        .admit_response_source(&admission, other_path, &message(), false, "other")
        .await
        .unwrap();
    poller
        .complete_response_delivery(&state, Path::new("source.json"))
        .await;
    assert_eq!(state.clones.load(std::sync::atomic::Ordering::SeqCst), 1);
    let sources = poller.response_sources.lock().unwrap();
    assert_eq!(sources.len(), 1);
    assert!(Arc::ptr_eq(&sources[other_path].attempt, &other.attempt));
    drop(sources);
    let mut reservation = tracker.reserve_work(&work.target, &work.key).unwrap();
    assert!(!reservation.accept_durable(&work.digest).unwrap());
    assert!(tracker.is_healthy());
}
