use std::sync::Mutex;

use futures::future::BoxFuture;

use super::*;
use crate::context_history::config::{ConfigStatus, LoadedHistoryConfig};
use crate::context_history::observation::TeamMembers;
use crate::context_history::store::{AppendOutcome, HistoryWriter};

impl HistoryIngress {
    /// An ingress whose mailbox the test drains itself.
    pub(crate) fn for_test(capacity: usize) -> (Self, mpsc::Receiver<IngressMessage>) {
        let (sender, receiver) = mpsc::channel(capacity);
        let ingress = Self {
            sender,
            sequence: Arc::new(AtomicU64::new(0)),
            dropped: Arc::new(AtomicU64::new(0)),
            gap: Arc::new(AtomicBool::new(false)),
            last_utc_ms: Arc::new(AtomicI64::new(i64::MIN)),
            clock_back: Arc::new(AtomicBool::new(false)),
        };
        (ingress, receiver)
    }
}

impl RestartNotice {
    pub(crate) fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub(crate) fn closes(&self) -> bool {
        self.phase == RestartPhase::Closing
    }

    pub(crate) fn acknowledge(mut self) {
        if let Some(ack) = self.ack.take() {
            let _ = ack.send(());
        }
    }
}

const WD: &str = "/p/proj/.ac/room-1-x/__agent_dev";

#[derive(Default)]
struct Shared {
    enabled: AtomicBool,
    busy_calls: AtomicU64,
    calls: AtomicU64,
    /// Team discoveries: one per resolved session.
    teams: AtomicU64,
    /// While set, the writer thread blocks inside `append`.
    hold: AtomicBool,
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
        while self.0.hold.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
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
        self.0.teams.fetch_add(1, Ordering::SeqCst);
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

/// Every session is present and live unless marked `absent`.
#[derive(Default)]
struct FakeSessions {
    absent: Mutex<HashSet<Uuid>>,
    roots: Mutex<HashSet<Uuid>>,
}

impl FakeSessions {
    fn fact(&self, session_id: Uuid) -> Option<SessionLaunch> {
        (!self.absent.lock().unwrap().contains(&session_id)).then(|| SessionLaunch {
            session_id,
            working_directory: WD.to_string(),
            shell: "claude".to_string(),
            effective_shell_args: Some(vec!["--model".to_string(), "opus".to_string()]),
            is_root_agent: self.roots.lock().unwrap().contains(&session_id),
            exited: false,
        })
    }
}

impl SessionLaunchSource for FakeSessions {
    fn launches<'a>(
        &'a self,
        ids: &'a [Uuid],
    ) -> BoxFuture<'a, Option<Vec<Option<SessionLaunch>>>> {
        Box::pin(async move { Some(ids.iter().map(|id| self.fact(*id)).collect()) })
    }

    fn live_ids<'a>(&'a self, ids: &'a [Uuid]) -> BoxFuture<'a, Option<HashSet<Uuid>>> {
        Box::pin(async move {
            let absent = self.absent.lock().unwrap();
            Some(
                ids.iter()
                    .filter(|id| !absent.contains(id))
                    .copied()
                    .collect(),
            )
        })
    }
}

type Started = (
    HistoryIngress,
    CancellationToken,
    tokio::task::JoinHandle<()>,
);

fn start_with(shared: &Arc<Shared>, sessions: Arc<dyn SessionLaunchSource>) -> Started {
    let token = CancellationToken::new();
    let env = Arc::new(FakeEnv(Arc::clone(shared)));
    let scopes = Arc::new(HistoryScopeState::new(sessions, env));
    let (ingress, actor) = HistoryRuntime::start(scopes, token.clone());
    (ingress, token, tokio::spawn(actor))
}

fn start(shared: &Arc<Shared>) -> Started {
    start_with(shared, Arc::new(FakeSessions::default()))
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
    tokio::time::sleep(BUSY_MIN_SPAN + Duration::from_millis(1_000)).await;
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
    let (ingress, _receiver) = HistoryIngress::for_test(1);
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

#[test]
fn a_backward_utc_jump_is_flagged_once_and_never_corrected() {
    let (ingress, _receiver) = HistoryIngress::for_test(1);
    ingress.last_utc_ms.store(i64::MAX, Ordering::Relaxed);
    let first = ingress.next_stamp();
    assert!(
        first.observed_at_utc_ms < i64::MAX,
        "the clock reading is kept as is"
    );
    assert!(ingress.clock_back.load(Ordering::Relaxed));
    let second = ingress.next_stamp();
    assert!(second.sequence > first.sequence, "only the sequence orders");
    assert!(ingress.clock_back.load(Ordering::Relaxed));
    // Once the clock passes the last stamp again, a new jump is logged again.
    ingress.last_utc_ms.store(0, Ordering::Relaxed);
    ingress.next_stamp();
    assert!(!ingress.clock_back.load(Ordering::Relaxed));
}

/// The real store under a project-shaped temporary AC root.
struct DiskEnv {
    ac_root: std::path::PathBuf,
}

impl ProjectEnvironment for DiskEnv {
    fn load_config(&self, _ac_root: &Path) -> LoadedHistoryConfig {
        LoadedHistoryConfig {
            config: HistoryConfig {
                enabled: true,
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
        config: &HistoryConfig,
    ) -> Result<Box<dyn HistoryAppend>, StoreError> {
        Ok(Box::new(HistoryWriter::open(&self.ac_root, config)?))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_five_second_reader_delays_the_batch_which_then_commits_once() {
    use crate::context_history::store::{history_database_path, read_history, ReadAttempt};
    use rusqlite::{Connection, OpenFlags};

    let tmp = tempfile::tempdir().unwrap();
    let ac_root = tmp.path().join("proj").join(".ac");
    std::fs::create_dir_all(&ac_root).unwrap();
    let config = HistoryConfig {
        enabled: true,
        retention_days: 30,
    };
    // Create the database, then hold a shared lock on it for 5 s.
    drop(HistoryWriter::open(&ac_root, &config).unwrap());
    let path = history_database_path(&ac_root);
    let reader = std::thread::spawn(move || {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        conn.execute_batch("BEGIN").unwrap();
        let _: i64 = conn
            .query_row("SELECT count(*) FROM records", [], |row| row.get(0))
            .unwrap();
        std::thread::sleep(Duration::from_secs(5));
        conn.execute_batch("ROLLBACK").unwrap();
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let token = CancellationToken::new();
    let env = Arc::new(DiskEnv {
        ac_root: ac_root.clone(),
    });
    let scopes = HistoryScopeState::new(Arc::new(FakeSessions::default()), env);
    let (ingress, actor) = HistoryRuntime::start(Arc::new(scopes), token.clone());
    let task = tokio::spawn(actor);
    ingress.offer(Uuid::new_v4(), ObservedValue::Reading(40));
    tokio::task::spawn_blocking(move || reader.join().unwrap())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    token.cancel();
    task.await.unwrap();
    let stored = read_history(&ac_root, ReadAttempt::ReadOnly).unwrap();
    assert_eq!(
        stored.len(),
        1,
        "committed exactly once after the reader left"
    );
}

use std::path::PathBuf;

use tauri::Manager as _;

// ---- #3532 P14: live-state pruning, pending sessions and composition ----------------------

/// Wait until `count` records are stored, at most ten seconds.
async fn stored_at_least(shared: &Shared, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while shared.stored.lock().unwrap().len() < count && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Offer one positive sample per id in mailbox-sized chunks.
async fn fill(ingress: &HistoryIngress, shared: &Shared, ids: &[Uuid]) {
    let mut expected = shared.stored.lock().unwrap().len();
    for chunk in ids.chunks(512) {
        for id in chunk {
            ingress.offer(*id, ObservedValue::Reading(10));
        }
        expected += chunk.len();
        stored_at_least(shared, expected).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn absent_sessions_free_live_state_with_a_gap_and_a_live_one_is_admitted() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    let sessions = Arc::new(FakeSessions::default());
    let (ingress, token, task) = start_with(&shared, sessions.clone());
    let roots: Vec<Uuid> = (0..96).map(|_| Uuid::new_v4()).collect();
    sessions.roots.lock().unwrap().extend(roots.iter().copied());
    let scoped: Vec<Uuid> = (0..LIVE_STATE_CAPACITY - roots.len())
        .map(|_| Uuid::new_v4())
        .collect();
    fill(&ingress, &shared, &scoped).await;
    // Excluded entries, seeded while visible; they never write.
    for id in &roots {
        ingress.offer(*id, ObservedValue::Reading(10));
    }
    settle().await;
    assert_eq!(shared.stored.lock().unwrap().len(), scoped.len());
    // Hold the writer so these changes are still queued when pruning runs.
    shared.hold.store(true, Ordering::SeqCst);
    for id in &scoped[..100] {
        ingress.offer(*id, ObservedValue::Reading(20));
    }
    let gone = &scoped[..3000];
    let mut absent = sessions.absent.lock().unwrap().clone();
    absent.extend(gone.iter().chain(&roots).copied());
    *sessions.absent.lock().unwrap() = absent;
    let fresh = Uuid::new_v4();
    ingress.offer(fresh, ObservedValue::Reading(55));
    settle().await;
    shared.hold.store(false, Ordering::SeqCst);
    let expected = scoped.len() + 100 + gone.len() + 1;
    stored_at_least(&shared, expected).await;
    token.cancel();
    task.await.unwrap();
    let stored = shared.stored.lock().unwrap();
    assert_eq!(
        stored.len(),
        expected,
        "queued records drained, nothing extra"
    );
    let (firsts, rest) = stored.split_at(scoped.len());
    let changes = rest
        .iter()
        .filter(|record| record.percent == Some(20))
        .count();
    assert_eq!(changes, 100, "records queued before pruning are kept");
    let gap = Some(crate::context_history::BoundaryReason::ObservationGap);
    let gaps: Vec<&HistoryRecord> = rest
        .iter()
        .filter(|record| record.boundary_reason == gap)
        .collect();
    assert_eq!(gaps.len(), gone.len(), "one gap per open absent session");
    for record in &gaps {
        assert_eq!(record.percent, None, "no fabricated value");
        assert!(gone.contains(&record.runtime_session_id));
        let first = firsts
            .iter()
            .find(|first| first.runtime_session_id == record.runtime_session_id)
            .expect("first record of the session");
        assert_eq!(record.logical_session_id, first.logical_session_id);
    }
    assert!(stored
        .iter()
        .all(|record| !roots.contains(&record.runtime_session_id)));
    let admitted: Vec<Option<u8>> = rest
        .iter()
        .filter(|record| record.runtime_session_id == fresh)
        .map(|record| record.percent)
        .collect();
    assert_eq!(admitted, [Some(55)]);
}

#[test]
fn saturation_warns_once_per_episode() {
    let mut saturated = false;
    let warned: Vec<bool> = (0..3)
        .map(|_| scopes::first_in_episode(&mut saturated))
        .collect();
    assert_eq!(warned, [true, false, false]);
    saturated = false; // an admission ends the episode
    assert!(scopes::first_in_episode(&mut saturated));
}

#[tokio::test]
async fn a_cached_scope_of_a_vanished_session_is_not_tracked() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    let sessions = Arc::new(FakeSessions::default());
    let env = Arc::new(FakeEnv(Arc::clone(&shared)));
    let scopes = Arc::new(HistoryScopeState::new(sessions.clone(), env));
    let token = CancellationToken::new();
    let (ingress, actor) = HistoryRuntime::start(Arc::clone(&scopes), token.clone());
    let task = tokio::spawn(actor);
    let id = Uuid::new_v4();
    assert!(matches!(scopes.resolve(id).await, ScopeLookup::Resolved(_)));
    sessions.absent.lock().unwrap().insert(id);
    ingress.offer(id, ObservedValue::Reading(30));
    settle().await;
    assert!(shared.stored.lock().unwrap().is_empty(), "no late capture");
    sessions.absent.lock().unwrap().remove(&id);
    ingress.offer(id, ObservedValue::Reading(45));
    stored_at_least(&shared, 1).await;
    token.cancel();
    task.await.unwrap();
    let values: Vec<Option<u8>> = shared
        .stored
        .lock()
        .unwrap()
        .iter()
        .map(|record| record.percent)
        .collect();
    assert_eq!(values, [Some(45)], "not memoized as excluded");
}

// ---- Real manager and HistorySessions -----------------------------------------------------

type SharedManager = Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>;

fn real_manager() -> (SharedManager, Arc<crate::pty_wiring::HistorySessions>) {
    let manager = Arc::new(tokio::sync::RwLock::new(
        crate::session::manager::SessionManager::new(),
    ));
    let sessions = Arc::new(crate::pty_wiring::HistorySessions {
        session_mgr: Arc::clone(&manager),
    });
    (manager, sessions)
}

async fn running(manager: &SharedManager, working_directory: &str) -> Uuid {
    let backend = crate::pty::backend::SessionBackendKind::LocalProcess;
    let session = manager
        .read()
        .await
        .create_session(
            "claude".to_string(),
            Vec::new(),
            working_directory.to_string(),
            None,
            None,
            Vec::new(),
            false,
            backend,
        )
        .await
        .expect("create session");
    session.id
}

#[tokio::test]
async fn real_batches_refuse_malformed_requests() {
    let (manager, sessions) = real_manager();
    let id = running(&manager, WD).await;
    let ids = |count: usize| -> Vec<Uuid> { (0..count).map(|_| Uuid::new_v4()).collect() };
    assert!(sessions.launches(&[]).await.is_none());
    assert!(sessions.launches(&ids(101)).await.is_none());
    assert!(sessions.launches(&[id, id]).await.is_none());
    assert_eq!(
        sessions.launches(&ids(100)).await.map(|all| all.len()),
        Some(100)
    );
    assert_eq!(sessions.live_ids(&[]).await, Some(HashSet::new()));
    let mut most = ids(4095);
    most.push(id);
    assert_eq!(sessions.live_ids(&most).await, Some(HashSet::from([id])));
    assert!(sessions.live_ids(&ids(4097)).await.is_none());
    assert!(sessions.live_ids(&[id, id]).await.is_none());
}

#[tokio::test]
async fn real_facts_show_exited_removed_and_root_sessions() {
    let (manager, sessions) = real_manager();
    let exited = running(&manager, WD).await;
    let removed = running(&manager, WD).await;
    let root = running(&manager, WD).await;
    manager.read().await.mark_exited(exited, 0).await;
    manager.read().await.destroy_session(removed).await.unwrap();
    manager.read().await.set_is_root_agent(root, true).await;
    let facts = sessions.launches(&[exited, removed, root]).await.unwrap();
    assert!(facts[0].as_ref().is_some_and(|launch| launch.exited));
    assert!(facts[1].is_none(), "removed is absent");
    assert!(facts[2].as_ref().is_some_and(|launch| launch.is_root_agent));
    let live = sessions.live_ids(&[exited, removed, root]).await.unwrap();
    assert_eq!(
        live,
        HashSet::from([root]),
        "exited is present but not live"
    );
    let shared = Arc::new(Shared::default());
    let env = Arc::new(FakeEnv(Arc::clone(&shared)));
    let scopes = HistoryScopeState::new(sessions, env);
    let project = [PathBuf::from("/p/proj")];
    let view = scopes
        .snapshot_sidebar_scopes(&project, &[exited, removed, root])
        .await
        .unwrap();
    assert!(view.iter().all(|(_, scope)| scope.is_none()), "all unknown");
    assert_eq!(shared.teams.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn real_mixed_local_and_foreign_sessions_are_refused_before_teams() {
    let (manager, sessions) = real_manager();
    let local = running(&manager, WD).await;
    let foreign = running(&manager, "/q/other/.ac/room-1-x/__agent_dev").await;
    let shared = Arc::new(Shared::default());
    let env = Arc::new(FakeEnv(Arc::clone(&shared)));
    let scopes = HistoryScopeState::new(sessions, env);
    let project = [PathBuf::from("/p/proj")];
    let refused = scopes
        .snapshot_sidebar_scopes(&project, &[local, foreign])
        .await;
    assert_eq!(
        refused,
        Err(scopes::SidebarScopeError::ForeignSession(foreign))
    );
    assert_eq!(shared.teams.load(Ordering::SeqCst), 0);
    // A Root of another project is foreign too; only a local Root is unknown.
    let root = running(&manager, "/q/other/.ac/room-1-x/__agent_root").await;
    manager.read().await.set_is_root_agent(root, true).await;
    let refused = scopes
        .snapshot_sidebar_scopes(&project, &[local, root])
        .await;
    assert_eq!(
        refused,
        Err(scopes::SidebarScopeError::ForeignSession(root))
    );
    assert_eq!(shared.teams.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_deferred_restart_replacement_is_captured_only_once_finished() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    let (manager, sessions) = real_manager();
    let old = running(&manager, WD).await;
    let env = Arc::new(FakeEnv(Arc::clone(&shared)));
    let scopes = Arc::new(HistoryScopeState::new(sessions, env));
    let token = CancellationToken::new();
    let (ingress, actor) = HistoryRuntime::start(Arc::clone(&scopes), token.clone());
    let task = tokio::spawn(actor);
    ingress.offer(old, ObservedValue::Reading(40));
    stored_at_least(&shared, 1).await;
    assert_eq!(
        ingress.restart_boundary(old, Uuid::new_v4()).await,
        RestartAck::Acked
    );
    // The replacement waits in pending_create until restart settles.
    let new = manager
        .read()
        .await
        .insert_pending_session_for_test(WD.to_string())
        .await;
    ingress.offer(new, ObservedValue::Reading(30));
    settle().await;
    let project = [PathBuf::from("/p/proj")];
    let pending = scopes.snapshot_sidebar_scopes(&project, &[new]).await;
    assert!(pending.unwrap()[0].1.is_none(), "pending is unknown");
    assert_eq!(
        shared.stored.lock().unwrap().len(),
        2,
        "pending writes nothing"
    );
    manager
        .read()
        .await
        .finish_pending_create_for_test(new)
        .await;
    ingress.offer(new, ObservedValue::Reading(45));
    stored_at_least(&shared, 3).await;
    let known = scopes
        .snapshot_sidebar_scopes(&project, &[new])
        .await
        .unwrap();
    assert!(known[0].1.is_some(), "known once settled");
    token.cancel();
    task.await.unwrap();
    let stored = shared.stored.lock().unwrap();
    let lifecycle = Some(crate::context_history::BoundaryReason::Lifecycle);
    assert_eq!(
        kinds(&stored),
        vec![(Some(40), None), (None, lifecycle), (Some(45), None)]
    );
    assert_eq!(stored[2].runtime_session_id, new);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_state_full_of_live_sessions_admits_nothing_until_one_leaves() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    let sessions = Arc::new(FakeSessions::default());
    let (ingress, token, task) = start_with(&shared, sessions.clone());
    let filled: Vec<Uuid> = (0..LIVE_STATE_CAPACITY).map(|_| Uuid::new_v4()).collect();
    fill(&ingress, &shared, &filled).await;
    let fresh = Uuid::new_v4();
    for percent in [20, 30] {
        ingress.offer(fresh, ObservedValue::Reading(percent));
    }
    settle().await;
    assert_eq!(shared.stored.lock().unwrap().len(), LIVE_STATE_CAPACITY);
    sessions.absent.lock().unwrap().insert(filled[0]);
    ingress.offer(fresh, ObservedValue::Reading(40));
    stored_at_least(&shared, LIVE_STATE_CAPACITY + 2).await;
    token.cancel();
    task.await.unwrap();
    let stored = shared.stored.lock().unwrap();
    let fresh_values: Vec<Option<u8>> = stored
        .iter()
        .filter(|record| record.runtime_session_id == fresh)
        .map(|record| record.percent)
        .collect();
    assert_eq!(
        fresh_values,
        [Some(40)],
        "admitted after one left, no eviction"
    );
}

#[tokio::test]
async fn a_pending_session_is_captured_once_finished_without_any_memo() {
    let shared = Arc::new(Shared::default());
    shared.enabled.store(true, Ordering::SeqCst);
    let manager = Arc::new(tokio::sync::RwLock::new(
        crate::session::manager::SessionManager::new(),
    ));
    let id = manager
        .read()
        .await
        .insert_pending_session_for_test(WD.to_string())
        .await;
    let sessions = Arc::new(crate::pty_wiring::HistorySessions {
        session_mgr: Arc::clone(&manager),
    });
    let env = Arc::new(FakeEnv(Arc::clone(&shared)));
    let scopes = Arc::new(HistoryScopeState::new(sessions, env));
    let token = CancellationToken::new();
    let (ingress, actor) = HistoryRuntime::start(Arc::clone(&scopes), token.clone());
    let task = tokio::spawn(actor);
    let project = [PathBuf::from("/p/proj")];
    ingress.offer(id, ObservedValue::Reading(30));
    settle().await;
    assert!(
        shared.stored.lock().unwrap().is_empty(),
        "pending writes nothing"
    );
    let pending = scopes.snapshot_sidebar_scopes(&project, &[id]).await;
    assert!(pending.unwrap()[0].1.is_none(), "pending is unknown");
    manager
        .read()
        .await
        .finish_pending_create_for_test(id)
        .await;
    ingress.offer(id, ObservedValue::Reading(40));
    stored_at_least(&shared, 1).await;
    let known = scopes
        .snapshot_sidebar_scopes(&project, &[id])
        .await
        .unwrap();
    assert!(known[0].1.is_some(), "known once finished");
    token.cancel();
    task.await.unwrap();
    let stored = shared.stored.lock().unwrap();
    let values: Vec<Option<u8>> = stored.iter().map(|record| record.percent).collect();
    assert_eq!(values, [Some(40)]);
}

#[test]
fn context_history_state_is_registered_once_by_the_lib_helper() {
    let app = tauri::test::mock_app();
    let shared = Arc::new(Shared::default());
    let scopes = |shared: &Arc<Shared>| {
        let env = Arc::new(FakeEnv(Arc::clone(shared)));
        Arc::new(HistoryScopeState::new(
            Arc::new(FakeSessions::default()),
            env,
        ))
    };
    let (ingress, _receiver) = HistoryIngress::for_test(4);
    let first = scopes(&shared);
    crate::register_context_history_state(&app, ingress.clone(), Arc::clone(&first))
        .expect("first registration");
    let (other, _other_receiver) = HistoryIngress::for_test(4);
    let duplicate = crate::register_context_history_state(&app, other, scopes(&shared));
    assert!(duplicate.is_err(), "a duplicate is refused");
    let managed = app.state::<Arc<HistoryScopeState>>();
    assert!(Arc::ptr_eq(managed.inner(), &first));
    assert!(app
        .state::<HistoryIngress>()
        .sender
        .same_channel(&ingress.sender));

    let partial = tauri::test::mock_app();
    partial.manage(ingress.clone());
    let refused = crate::register_context_history_state(&partial, ingress, scopes(&shared));
    assert!(refused.is_err());
    assert!(partial.try_state::<Arc<HistoryScopeState>>().is_none());
}

#[test]
fn session_observers_register_the_state_once_before_the_scraper_gets_the_ingress() {
    let setup = include_str!("../app_run/setup.rs");
    let call = "crate::register_context_history_state(";
    assert_eq!(setup.matches(call).count(), 1);
    let registered = setup.find(call).unwrap();
    let observers = setup.find("fn start_session_observers(").unwrap();
    let handed = setup.find("history: Some(history_ingress)").unwrap();
    assert!(observers < registered && registered < handed);
}
