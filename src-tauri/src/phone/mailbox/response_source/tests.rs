use super::*;
use crate::pty::manager::ResponseCloseState;
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
    assert!(MailboxPoller::delivery_only()
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
    let poller = MailboxPoller::delivery_only();
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
    let poller = MailboxPoller::delivery_only();
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
    assert!(MailboxPoller::delivery_only()
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
    let poller = MailboxPoller::delivery_only();
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
        let poller = MailboxPoller::delivery_only();
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
