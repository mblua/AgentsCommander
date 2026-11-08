//! Neutral observations exercise real routing and retry policy.

use super::*;
use crate::co_managed::route::{
    commit_co_managed_with_retries, gather_co_managed_preconditions, resolve_co_managed_route,
    CoManagedCommit,
};
use crate::co_managed::{CoManagedRoute, CoManagedSupervisorHandle};
use crate::config::co_managed::OffReason;
use crate::session::selection::task::SelectionTask;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;
use uuid::Uuid;

struct PolicyHost {
    sessions: Option<Arc<RwLock<SessionManager>>>,
    calls: Mutex<Vec<&'static str>>,
    states: Mutex<VecDeque<CoManagedState>>,
    observations: Mutex<Vec<EffectPreconditions>>,
    outcomes: Mutex<VecDeque<Result<(), AbstainReason>>>,
}

impl CoManagedEvents for PolicyHost {
    fn co_managed_event(&self, _: &'static str, _: Value) {
        panic!("preconditions and commit policy must not publish events");
    }
}

impl SelectionEvents for PolicyHost {
    fn selection_event(&self, _: &'static str, _: Value) {
        panic!("preconditions and commit policy must not publish selection events");
    }
}

impl CoManagedHost for PolicyHost {
    fn sessions_optional(&self) -> Option<&Arc<RwLock<SessionManager>>> {
        self.calls.lock().unwrap().push("sessions");
        self.sessions.as_ref()
    }
    fn sessions_required(&self) -> &Arc<RwLock<SessionManager>> {
        self.sessions.as_ref().expect("session manager present")
    }
    fn settings_required(&self) -> &SettingsState {
        panic!("precondition policy does not require settings");
    }
    fn input_optional(&self) -> Option<&SubstantiveInputState> {
        self.calls.lock().unwrap().push("input");
        None
    }
    fn registry_optional(&self) -> Option<&Arc<CaptureRegistry>> {
        panic!("precondition policy does not require the registry");
    }
    fn network_required(&self) -> &OutboundNetwork {
        panic!("precondition policy does not require the network");
    }
    fn tasks(&self) -> &dyn SelectionTasks {
        self
    }
    fn effective_state<'a>(
        &'a self,
        _: &'a Path,
        _: &'a str,
    ) -> BoxFuture<'a, Result<CoManagedState, String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push("effective");
            Ok(self
                .states
                .lock()
                .unwrap()
                .pop_front()
                .expect("fresh state"))
        })
    }
    fn commit_effect(
        &self,
        _: PathBuf,
        _: CaptureSlot,
        _: u64,
        key: ConsumptionKey,
        pre: EffectPreconditions,
        _: Arc<CaptureState>,
    ) -> BoxFuture<'static, Result<CommittedEffect, AbstainReason>> {
        self.calls.lock().unwrap().push("commit");
        self.observations.lock().unwrap().push(pre.clone());
        let outcome = self.outcomes.lock().unwrap().pop_front().expect("outcome");
        Box::pin(async move {
            outcome.map(|()| CommittedEffect {
                session_id: pre.session_id,
                key,
                budget_remaining: 2,
                kind: pre.kind,
                routable: true,
            })
        })
    }
}

impl SelectionTasks for PolicyHost {
    fn spawn(&self, _: BoxFuture<'static, ()>) -> Pin<Box<dyn SelectionTask>> {
        panic!("neutral policy proof must not schedule native work");
    }
}

fn host(present: bool, states: Vec<CoManagedState>) -> PolicyHost {
    PolicyHost {
        sessions: present.then(|| Arc::new(RwLock::new(SessionManager::new()))),
        calls: Mutex::new(Vec::new()),
        states: Mutex::new(states.into()),
        observations: Mutex::new(Vec::new()),
        outcomes: Mutex::new(VecDeque::new()),
    }
}

async fn session(host: &PolicyHost) -> Uuid {
    host.sessions_required()
        .read()
        .await
        .create_session(
            "sh".into(),
            Vec::new(),
            "C:/work".into(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap()
        .id
}

async fn assert_neutral_preconditions() {
    let missing = host(false, Vec::new());
    let id = Uuid::new_v4();
    let pre = gather_co_managed_preconditions(
        &missing,
        id,
        Path::new("unused-room"),
        crate::capture::state::EffectKind::Automatic,
    )
    .await;
    assert_eq!(pre.session_id, id.to_string());
    assert!(!pre.session_alive && !pre.unique_live_session_for_cwd);
    assert!(!pre.no_pending_user_input && !pre.effective_ready);
    assert_eq!(*missing.calls.lock().unwrap(), ["sessions"]);

    let present = host(true, vec![CoManagedState::Ready]);
    let id = session(&present).await;
    let pre = gather_co_managed_preconditions(
        &present,
        id,
        Path::new("unused-room"),
        crate::capture::state::EffectKind::Automatic,
    )
    .await;
    assert!(pre.session_alive && pre.unique_live_session_for_cwd);
    assert!(pre.no_pending_user_input, "input None permits the effect");
    assert!(pre.effective_ready);
    assert_eq!(
        *present.calls.lock().unwrap(),
        ["sessions", "input", "effective"]
    );
}

async fn assert_neutral_retries() {
    let host = host(
        true,
        vec![
            CoManagedState::Ready,
            CoManagedState::Off {
                reason: OffReason::RoomFlagOff,
            },
        ],
    );
    let id = session(&host).await;
    *host.outcomes.lock().unwrap() = [Err(AbstainReason::LockBusy), Ok(())].into();
    let record = crate::capture::record::CapturedRecord {
        session_id: id.to_string(),
        text: "candidate".into(),
        file: "candidate.jsonl".into(),
        epoch: 0,
        record_start: Some(0),
        reader_seq: 1,
        text_sha256: [7; 32],
        turn_id: None,
        provider: crate::capture::record::CaptureProvider::Claude,
        provider_final: false,
        turn_identified: false,
        origin: crate::capture::record::RecordOrigin::Live,
        observed_path: "candidate.jsonl".into(),
        observed_len: 1,
        observed_prefix: vec![1],
    };
    let key = ConsumptionKey::from_record(&record, 0);
    let (handle, _receiver) = CoManagedSupervisorHandle::new();
    let outcome = commit_co_managed_with_retries(
        &host,
        &handle,
        id,
        Path::new("unused-room"),
        &CaptureSlot::new(),
        0,
        &key,
        crate::capture::state::EffectKind::Automatic,
    )
    .await;
    match outcome {
        CoManagedCommit::Committed(effect) => {
            assert_eq!(effect.session_id, id.to_string());
            assert_eq!(effect.key, key);
            assert!(effect.routable);
        }
        _ => panic!("scripted retry must commit"),
    }
    assert_eq!(
        *host.calls.lock().unwrap(),
        [
            "sessions",
            "input",
            "effective",
            "commit",
            "sessions",
            "input",
            "effective",
            "commit"
        ]
    );
    let observations = host.observations.lock().unwrap();
    assert_eq!(observations.len(), 2);
    assert!(observations[0].effective_ready);
    assert!(
        !observations[1].effective_ready,
        "retry reads the current effective state"
    );
    assert!(observations.iter().all(|pre| pre.no_pending_user_input));
    assert!(observations[1].observed_at > observations[0].observed_at);
    assert!(host.states.lock().unwrap().is_empty());
    assert!(host.outcomes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn issue_3950_co_fake_policy() {
    assert_neutral_preconditions().await;
    assert_neutral_retries().await;
    let outcome = crate::capture::jev::ClassifyOutcome::Abstained {
        reason: "neutral-proof".into(),
    };
    let catalog = crate::capture::catalog::Catalog::missing();
    let route = resolve_co_managed_route(outcome, &catalog, "project/agent", &[], "candidate");
    match route {
        CoManagedRoute::User {
            reason,
            write_candidate_file,
        } => {
            assert_eq!(reason, "abstained: neutral-proof");
            assert!(write_candidate_file);
        }
        CoManagedRoute::Wake { .. } => panic!("abstention routes to the user"),
    }
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_co_fake_policy");
}
