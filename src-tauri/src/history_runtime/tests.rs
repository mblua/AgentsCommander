use std::sync::Mutex;

use super::*;
use crate::context_history::config::ConfigStatus;

const WD: &str = "/p/proj/.ac/room-1-x/__agent_dev";

#[derive(Default)]
struct Shared {
    enabled: AtomicBool,
    busy_calls: AtomicU64,
    calls: AtomicU64,
    stored: Mutex<Vec<HistoryRecord>>,
}

struct FakeEnv(Arc<Shared>);

struct FakeWriter(Arc<Shared>);

impl HistoryAppend for FakeWriter {
    fn append(
        &mut self,
        records: &[HistoryRecord],
        _now: i64,
    ) -> Result<AppendOutcome, StoreError> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        if self.0.busy_calls.load(Ordering::SeqCst) > 0 {
            self.0.busy_calls.fetch_sub(1, Ordering::SeqCst);
            return Err(StoreError::RetryableBusy);
        }
        self.0.stored.lock().unwrap().extend_from_slice(records);
        Ok(AppendOutcome {
            inserted: records.len(),
            evicted: 0,
            full_retries: 0,
        })
    }
}

impl ProjectEnvironment for FakeEnv {
    fn load_config(&self, _ac_root: &Path) -> LoadedHistoryConfig {
        LoadedHistoryConfig {
            config: HistoryConfig {
                enabled: self.0.enabled.load(Ordering::SeqCst),
                retention_days: 30,
            },
            status: ConfigStatus::Loaded,
        }
    }
    fn authoritative_ac_root(&self, _ac_root: &Path) -> bool {
        true
    }
    fn teams(&self, _project_path: &Path) -> Vec<TeamMembers> {
        Vec::new()
    }
    fn open_writer(
        &self,
        _ac_root: &Path,
        _config: &HistoryConfig,
    ) -> Result<Box<dyn HistoryAppend>, StoreError> {
        Ok(Box::new(FakeWriter(Arc::clone(&self.0))))
    }
}

struct FakeSessions;

impl SessionLaunchSource for FakeSessions {
    fn launch(&self, session_id: Uuid) -> BoxFuture<'_, Option<SessionLaunch>> {
        Box::pin(async move {
            Some(SessionLaunch {
                session_id,
                working_directory: WD.to_string(),
                shell: "claude".to_string(),
                effective_shell_args: Some(vec!["--model".to_string(), "opus".to_string()]),
                is_root_agent: false,
            })
        })
    }
}

fn start(
    shared: &Arc<Shared>,
) -> (
    HistoryIngress,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let token = CancellationToken::new();
    let env = Arc::new(FakeEnv(Arc::clone(shared)));
    let (ingress, actor) = HistoryRuntime::start(Arc::new(FakeSessions), env, token.clone());
    (ingress, token, tokio::spawn(actor))
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn enabled_capture_writes_changes_with_scope_and_launch_metadata() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    let (ingress, token, task) = start(&shared);
    let id = Uuid::new_v4();
    for percent in [20, 20, 30] {
        ingress.offer(id, ObservedValue::Reading(percent));
    }
    ingress.offer(id, ObservedValue::SessionOver);
    settle().await;
    token.cancel();
    task.await.unwrap();
    let stored = shared.stored.lock().unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!(stored[0].agent_key, "dev");
    assert_eq!(stored[0].model.as_deref(), Some("opus"));
    assert_eq!(
        stored[2].boundary_reason,
        Some(crate::context_history::BoundaryReason::Lifecycle)
    );
}

#[tokio::test]
async fn disabled_capture_writes_nothing() {
    let shared = Arc::new(Shared::default());
    let (ingress, token, task) = start(&shared);
    ingress.offer(Uuid::new_v4(), ObservedValue::Reading(40));
    settle().await;
    token.cancel();
    task.await.unwrap();
    assert!(shared.stored.lock().unwrap().is_empty());
    assert_eq!(shared.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn busy_batch_is_retried_and_committed_once() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    shared.busy_calls.store(3, Ordering::SeqCst);
    let (ingress, token, task) = start(&shared);
    ingress.offer(Uuid::new_v4(), ObservedValue::Reading(40));
    tokio::time::sleep(Duration::from_millis(800)).await;
    token.cancel();
    task.await.unwrap();
    assert_eq!(shared.stored.lock().unwrap().len(), 1);
    assert_eq!(shared.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn persistent_busy_is_dropped_after_bounded_calls() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    shared.busy_calls.store(100, Ordering::SeqCst);
    let (ingress, token, task) = start(&shared);
    ingress.offer(Uuid::new_v4(), ObservedValue::Reading(40));
    tokio::time::sleep(Duration::from_millis(1_600)).await;
    token.cancel();
    task.await.unwrap();
    assert!(shared.stored.lock().unwrap().is_empty());
    assert_eq!(
        shared.calls.load(Ordering::SeqCst),
        u64::from(BUSY_MAX_CALLS)
    );
}

#[tokio::test]
async fn full_mailbox_drops_and_signals_a_gap_without_blocking() {
    let (sender, _receiver) = mpsc::channel(1);
    let ingress = HistoryIngress {
        sender,
        sequence: Arc::new(AtomicU64::new(0)),
        dropped: Arc::new(AtomicU64::new(0)),
        gap: Arc::new(AtomicBool::new(false)),
    };
    for _ in 0..3 {
        ingress.offer(Uuid::nil(), ObservedValue::Unavailable);
    }
    assert_eq!(ingress.dropped.load(Ordering::Relaxed), 2);
    assert!(ingress.gap.load(Ordering::Relaxed));
}

#[test]
fn pending_reservation_is_capped() {
    let writer = ProjectWriter {
        sender: std_mpsc::channel().0,
        pending: Arc::new(AtomicUsize::new(PENDING_CAPACITY - 1)),
        overflowed: Arc::new(AtomicBool::new(false)),
    };
    assert_eq!(writer.reserve(5), 1);
    assert_eq!(writer.reserve(5), 0);
}

#[tokio::test]
async fn shutdown_flush_is_bounded() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    shared.busy_calls.store(u64::MAX / 2, Ordering::SeqCst);
    let (ingress, token, task) = start(&shared);
    ingress.offer(Uuid::new_v4(), ObservedValue::Reading(40));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let began = Instant::now();
    token.cancel();
    task.await.unwrap();
    assert!(began.elapsed() < SHUTDOWN_FLUSH + Duration::from_millis(500));
}

#[test]
fn sidebar_scopes_work_without_capture() {
    let shared = Arc::new(Shared::default());
    let env = FakeEnv(shared);
    let launch = |wd: &str, root| SessionLaunch {
        session_id: Uuid::new_v4(),
        working_directory: wd.to_string(),
        shell: "claude".to_string(),
        effective_shell_args: None,
        is_root_agent: root,
    };
    let scopes = snapshot_sidebar_scopes(&[launch(WD, false), launch(WD, true)], &env);
    assert!(matches!(scopes[0].1, ScopeResolution::Scoped(_)));
    assert_eq!(scopes[1].1, ScopeResolution::Excluded("root_agent"));
}

fn kinds(
    stored: &[HistoryRecord],
) -> Vec<(Option<u8>, Option<crate::context_history::BoundaryReason>)> {
    stored
        .iter()
        .map(|record| (record.percent, record.boundary_reason))
        .collect()
}

#[tokio::test]
async fn restart_closes_the_old_session_with_a_null_lifecycle_boundary() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    let (ingress, token, task) = start(&shared);
    let old = Uuid::new_v4();
    let intent = Uuid::new_v4();
    ingress.offer(old, ObservedValue::Reading(40));
    assert_eq!(
        ingress.restart_boundary(old, intent).await,
        RestartAck::Acked
    );
    // Retry of the same intent and late samples of the old instance write nothing.
    assert_eq!(
        ingress.restart_boundary(old, intent).await,
        RestartAck::Acked
    );
    ingress.offer(old, ObservedValue::Reading(40));
    ingress.offer(old, ObservedValue::Reading(55));
    ingress.offer(old, ObservedValue::SessionOver);
    // The replacement is an independent instance with its own first positive.
    let new = Uuid::new_v4();
    ingress.offer(new, ObservedValue::Reading(40));
    settle().await;
    token.cancel();
    task.await.unwrap();
    let stored = shared.stored.lock().unwrap();
    let lifecycle = Some(crate::context_history::BoundaryReason::Lifecycle);
    assert_eq!(
        kinds(&stored),
        vec![(Some(40), None), (None, lifecycle), (Some(40), None)]
    );
    assert_eq!(stored[1].runtime_session_id, old);
    assert_eq!(stored[1].model.as_deref(), Some("opus"));
    assert_eq!(stored[2].runtime_session_id, new);
    assert_ne!(stored[2].logical_session_id, stored[0].logical_session_id);
}

#[tokio::test]
async fn restart_after_an_observed_zero_does_not_close_twice() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    let (ingress, token, task) = start(&shared);
    let old = Uuid::new_v4();
    ingress.offer(old, ObservedValue::Reading(40));
    ingress.offer(old, ObservedValue::Reading(0));
    ingress.restart_boundary(old, Uuid::new_v4()).await;
    settle().await;
    token.cancel();
    task.await.unwrap();
    let zero = Some(crate::context_history::BoundaryReason::ObservedZero);
    assert_eq!(
        kinds(&shared.stored.lock().unwrap()),
        vec![(Some(40), None), (Some(0), zero)]
    );
}

#[tokio::test]
async fn restart_of_an_unobserved_or_disabled_instance_writes_nothing() {
    let shared = Arc::new(Shared::default());
    let (ingress, token, task) = start(&shared);
    let disabled = Uuid::new_v4();
    ingress.offer(disabled, ObservedValue::Reading(40));
    assert_eq!(
        ingress.restart_boundary(disabled, Uuid::new_v4()).await,
        RestartAck::Acked
    );
    let unseen = Uuid::new_v4();
    assert_eq!(
        ingress.restart_boundary(unseen, Uuid::new_v4()).await,
        RestartAck::Acked
    );
    settle().await;
    token.cancel();
    task.await.unwrap();
    assert!(shared.stored.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_teardown_reopens_the_old_instance_as_a_partial_segment() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    let (ingress, token, task) = start(&shared);
    let old = Uuid::new_v4();
    let intent = Uuid::new_v4();
    ingress.offer(old, ObservedValue::Reading(40));
    ingress.restart_boundary(old, intent).await;
    ingress.restart_teardown_failed(old, intent);
    ingress.offer(old, ObservedValue::Reading(40));
    settle().await;
    token.cancel();
    task.await.unwrap();
    let stored = shared.stored.lock().unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!(stored[2].runtime_session_id, old);
    assert!(stored[2].partial_start);
    assert!(!stored[2].continuity_known);
    assert_eq!(
        stored[2].initial_kind,
        crate::context_history::InitialKind::Unknown
    );
    assert_ne!(stored[2].logical_session_id, stored[0].logical_session_id);
}

#[tokio::test]
async fn restart_boundary_never_waits_past_its_budget() {
    // Nobody drains this mailbox: the ack times out and restart goes on.
    let (ingress, _receiver) = HistoryIngress::for_test(4);
    let began = Instant::now();
    let ack = ingress
        .restart_boundary(Uuid::new_v4(), Uuid::new_v4())
        .await;
    assert_eq!(ack, RestartAck::TimedOut);
    assert!(began.elapsed() < RESTART_ACK + Duration::from_millis(200));
    // A full mailbox refuses at once; a closed one reports closed.
    let (full, receiver) = HistoryIngress::for_test(1);
    full.offer(Uuid::nil(), ObservedValue::Unavailable);
    assert_eq!(
        full.restart_boundary(Uuid::nil(), Uuid::nil()).await,
        RestartAck::Busy
    );
    drop(receiver);
    assert_eq!(
        full.restart_boundary(Uuid::nil(), Uuid::nil()).await,
        RestartAck::Closed
    );
}
