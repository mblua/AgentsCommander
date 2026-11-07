//! Context-history runtime (#3532 P02): bounded capture of context changes.
//!
//! The scraper's sample bridge offers every outcome to [`HistoryIngress`],
//! which stamps UTC time and a monotonic sequence and `try_send`s into a
//! bounded mailbox; it never blocks, awaits or touches disk. One actor owns
//! the per-instance transition state, resolves each session's scope once, and
//! hands records to one serial writer thread per project. No session or PTY
//! lock is held during history I/O, and no `AppHandle` reaches this module:
//! `lib`/`setup` compose it and own the shutdown hook.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::context_history::config::HistoryConfig;
use crate::context_history::observation::{
    InstanceContext, InstanceState, ObservedValue, ScopeResolution, Stamp,
};
use crate::context_history::store::{StoreError, MAX_APPEND_BATCH};
use crate::context_history::HistoryRecord;

pub(crate) mod project;
pub(crate) mod scopes;

pub(crate) use scopes::{
    DiskProjectEnvironment, HistoryAppend, HistoryScopeState, ProjectEnvironment, ScopeLookup,
    SessionLaunch, SessionLaunchSource,
};

/// Bounded ingress mailbox between the scraper bridge and the actor.
pub(crate) const MAILBOX_CAPACITY: usize = 4096;
/// Maximum tracked instances (scoped or excluded) at once.
pub(crate) const LIVE_STATE_CAPACITY: usize = 4096;
/// Maximum records queued, in flight or retrying for one project.
pub(crate) const PENDING_CAPACITY: usize = 4096;
/// How often `.ac/heatmap.json` is re-read; configuration, not sampling.
pub(crate) const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// A busy batch is retried in FIFO order for at least this long, so a reader
/// holding the database for 5 s still lets it commit once...
pub(crate) const BUSY_MIN_SPAN: Duration = Duration::from_secs(5);
/// ...in at most this many append calls, the first included.
pub(crate) const BUSY_MAX_CALLS: u32 = 10;
/// Shutdown flush budget; whatever is left afterwards is dropped and logged.
pub(crate) const SHUTDOWN_FLUSH: Duration = Duration::from_secs(2);
/// Restart waits at most this long for its boundary; lifecycle then goes on.
pub(crate) const RESTART_ACK: Duration = Duration::from_millis(250);

fn now_utc_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

// ---- Ingress -----------------------------------------------------------------------------

/// One observation stamped at ingress, before any queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StampedSample {
    pub(crate) session_id: Uuid,
    pub(crate) value: ObservedValue,
    pub(crate) stamp: Stamp,
}

/// What restart reports about one old instance (#3532 P03).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestartPhase {
    /// Teardown is about to start: close the old observation now.
    Closing,
    /// Teardown failed: the old instance may keep producing observations.
    TeardownFailed,
}

/// One restart notice, stamped at ingress like a sample.
#[derive(Debug)]
pub(crate) struct RestartNotice {
    session_id: Uuid,
    intent_id: Uuid,
    phase: RestartPhase,
    stamp: Stamp,
    ack: Option<oneshot::Sender<()>>,
}

/// One mailbox entry. Notices share the FIFO with samples, so everything
/// admitted before a restart is processed before its boundary.
#[derive(Debug)]
pub(crate) enum IngressMessage {
    Sample(StampedSample),
    Restart(RestartNotice),
}

/// How a restart boundary request ended. Every outcome lets restart go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartAck {
    Acked,
    Busy,
    TimedOut,
    Closed,
}

/// Nonblocking, fail-soft entry point used by the scraper's sample bridge.
#[derive(Clone)]
pub(crate) struct HistoryIngress {
    sender: mpsc::Sender<IngressMessage>,
    sequence: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    /// Set when samples were lost; the actor turns it into gap boundaries.
    gap: Arc<AtomicBool>,
    /// Latest UTC stamp issued, and whether a backward jump was already logged.
    last_utc_ms: Arc<AtomicI64>,
    clock_back: Arc<AtomicBool>,
}

impl HistoryIngress {
    /// Stamp and offer one sample. A full mailbox drops it, counts it and
    /// signals a discontinuity; it never waits.
    pub(crate) fn offer(&self, session_id: Uuid, value: ObservedValue) {
        let sample = StampedSample {
            session_id,
            value,
            stamp: self.next_stamp(),
        };
        // A closed mailbox means the actor already stopped: the sample is moot.
        let message = IngressMessage::Sample(sample);
        if let Err(mpsc::error::TrySendError::Full(_)) = self.sender.try_send(message) {
            let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if !self.gap.swap(true, Ordering::Relaxed) {
                log::warn!("[context-history] mailbox full; sample dropped (dropped={dropped})");
            }
        }
    }

    /// Close the old instance's observation before restart teardown. The
    /// record is a `lifecycle` boundary meaning "AC restart attempt", not a
    /// confirmed provider restart. Waits at most [`RESTART_ACK`]; a full
    /// mailbox, a closed actor or the timeout are logged and never fail the
    /// restart. A dropped wait leaves no task behind.
    pub(crate) async fn restart_boundary(&self, session_id: Uuid, intent_id: Uuid) -> RestartAck {
        let (ack, wait) = oneshot::channel();
        let notice = self.notice(session_id, intent_id, RestartPhase::Closing, Some(ack));
        if let Err(error) = self.sender.try_send(IngressMessage::Restart(notice)) {
            return Self::refused(&error);
        }
        match tokio::time::timeout(RESTART_ACK, wait).await {
            Ok(Ok(())) => RestartAck::Acked,
            Ok(Err(_)) => RestartAck::Closed,
            Err(_) => {
                log::warn!("[context-history] restart boundary not acknowledged in time");
                RestartAck::TimedOut
            }
        }
    }

    /// Teardown of a closed instance failed. Its later observations, if any,
    /// start a new segment of unknown start; they never join the replacement.
    pub(crate) fn restart_teardown_failed(&self, session_id: Uuid, intent_id: Uuid) {
        let notice = self.notice(session_id, intent_id, RestartPhase::TeardownFailed, None);
        if let Err(error) = self.sender.try_send(IngressMessage::Restart(notice)) {
            Self::refused(&error);
        }
    }

    fn notice(
        &self,
        session_id: Uuid,
        intent_id: Uuid,
        phase: RestartPhase,
        ack: Option<oneshot::Sender<()>>,
    ) -> RestartNotice {
        RestartNotice {
            session_id,
            intent_id,
            phase,
            stamp: self.next_stamp(),
            ack,
        }
    }

    fn refused(error: &mpsc::error::TrySendError<IngressMessage>) -> RestartAck {
        if let mpsc::error::TrySendError::Full(_) = error {
            log::warn!("[context-history] mailbox full; restart notice dropped");
            RestartAck::Busy
        } else {
            RestartAck::Closed
        }
    }

    /// Stamps keep the clock as read: a backward UTC jump is logged once per
    /// episode and never rejected or corrected; only `sequence` orders.
    fn next_stamp(&self) -> Stamp {
        let now = now_utc_ms();
        let last = self.last_utc_ms.fetch_max(now, Ordering::Relaxed);
        if now >= last {
            self.clock_back.store(false, Ordering::Relaxed);
        } else if !self.clock_back.swap(true, Ordering::Relaxed) {
            log::warn!("[context-history] UTC clock went back {} ms", last - now);
        }
        Stamp {
            observed_at_utc_ms: now,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
        }
    }
}

// ---- Project writer ----------------------------------------------------------------------

enum WorkerMessage {
    Records {
        epoch: u64,
        records: Vec<HistoryRecord>,
    },
    Config {
        epoch: u64,
        config: HistoryConfig,
    },
    Flush {
        deadline: Instant,
        done: oneshot::Sender<()>,
    },
}

struct ProjectWriter {
    sender: std_mpsc::Sender<WorkerMessage>,
    pending: Arc<AtomicUsize>,
    overflowed: Arc<AtomicBool>,
}

impl ProjectWriter {
    fn start(ac_root: String, env: Arc<dyn ProjectEnvironment>) -> Option<Self> {
        let (sender, receiver) = std_mpsc::channel();
        let pending = Arc::new(AtomicUsize::new(0));
        let worker = Worker {
            ac_root,
            env,
            receiver,
            pending: Arc::clone(&pending),
            epoch: 0,
            config: HistoryConfig::DISABLED,
            writer: None,
        };
        let spawned = std::thread::Builder::new()
            .name("context-history-writer".to_string())
            .spawn(move || worker.run());
        if let Err(error) = spawned {
            log::warn!("[context-history] writer thread unavailable: {error}");
            return None;
        }
        Some(Self {
            sender,
            pending,
            overflowed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Queue records, dropping the excess beyond [`PENDING_CAPACITY`].
    fn enqueue(&self, epoch: u64, mut records: Vec<HistoryRecord>) {
        let reserved = self.reserve(records.len());
        if reserved < records.len() {
            let lost = records.len() - reserved;
            records.truncate(reserved);
            if !self.overflowed.swap(true, Ordering::Relaxed) {
                log::warn!("[context-history] pending queue full; {lost} record(s) dropped");
            }
        }
        if records.is_empty() {
            return;
        }
        let count = records.len();
        if self
            .sender
            .send(WorkerMessage::Records { epoch, records })
            .is_err()
        {
            self.pending.fetch_sub(count, Ordering::Relaxed);
        }
    }

    fn reserve(&self, wanted: usize) -> usize {
        let mut current = self.pending.load(Ordering::Relaxed);
        loop {
            let granted = wanted.min(PENDING_CAPACITY.saturating_sub(current));
            match self.pending.compare_exchange(
                current,
                current + granted,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return granted,
                Err(actual) => current = actual,
            }
        }
    }

    fn configure(&self, epoch: u64, config: HistoryConfig) {
        let _ = self.sender.send(WorkerMessage::Config { epoch, config });
    }
}

struct Worker {
    ac_root: String,
    env: Arc<dyn ProjectEnvironment>,
    receiver: std_mpsc::Receiver<WorkerMessage>,
    pending: Arc<AtomicUsize>,
    epoch: u64,
    config: HistoryConfig,
    writer: Option<Box<dyn HistoryAppend>>,
}

impl Worker {
    fn run(mut self) {
        let mut queue: Vec<HistoryRecord> = Vec::new();
        while let Ok(message) = self.receiver.recv() {
            if let Some(done) = self.accept(message, &mut queue) {
                let _ = done.send(());
                return;
            }
            while let Ok(message) = self.receiver.try_recv() {
                if let Some(done) = self.accept(message, &mut queue) {
                    let _ = done.send(());
                    return;
                }
            }
            self.drain(&mut queue, None);
        }
    }

    /// Fold one message into the queue; a flush drains within its deadline
    /// and returns the completion handle.
    fn accept(
        &mut self,
        message: WorkerMessage,
        queue: &mut Vec<HistoryRecord>,
    ) -> Option<oneshot::Sender<()>> {
        match message {
            WorkerMessage::Records { epoch, records } if epoch == self.epoch => {
                queue.extend(records);
                None
            }
            WorkerMessage::Records { records, .. } => {
                self.release(records.len());
                None
            }
            WorkerMessage::Config { epoch, config } => {
                if epoch != self.epoch {
                    self.release(queue.len());
                    queue.clear();
                }
                if epoch != self.epoch || config != self.config {
                    self.writer = None;
                }
                self.epoch = epoch;
                self.config = config;
                None
            }
            WorkerMessage::Flush { deadline, done } => {
                while let Ok(message) = self.receiver.try_recv() {
                    if let WorkerMessage::Flush { done, .. } = message {
                        let _ = done.send(());
                    } else {
                        self.accept(message, queue);
                    }
                }
                self.drain(queue, Some(deadline));
                if !queue.is_empty() {
                    log::warn!(
                        "[context-history] shutdown flush budget spent; {} record(s) lost",
                        queue.len()
                    );
                    self.release(queue.len());
                    queue.clear();
                }
                Some(done)
            }
        }
    }

    fn release(&self, count: usize) {
        self.pending.fetch_sub(count, Ordering::Relaxed);
    }

    fn drain(&mut self, queue: &mut Vec<HistoryRecord>, deadline: Option<Instant>) {
        while !queue.is_empty() {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return;
            }
            let take = queue.len().min(MAX_APPEND_BATCH);
            let batch: Vec<HistoryRecord> = queue.drain(..take).collect();
            self.commit(&batch, deadline);
            self.release(batch.len());
        }
    }

    /// Append one batch: busy retries keep FIFO order for at least
    /// [`BUSY_MIN_SPAN`] in at most [`BUSY_MAX_CALLS`] calls; any other error,
    /// or exhausting them, drops the batch with a diagnostic.
    fn commit(&mut self, batch: &[HistoryRecord], deadline: Option<Instant>) {
        if !self.config.enabled {
            return;
        }
        let pause = BUSY_MIN_SPAN / (BUSY_MAX_CALLS - 1);
        for call in 1..=BUSY_MAX_CALLS {
            let result = match self.writer_mut() {
                Ok(writer) => writer.append(batch, now_utc_ms()),
                Err(error) => Err(error),
            };
            match result {
                Ok(_) => return,
                Err(StoreError::RetryableBusy) if call < BUSY_MAX_CALLS => {
                    let wake = Instant::now() + pause;
                    if deadline.is_some_and(|deadline| wake >= deadline) {
                        break;
                    }
                    std::thread::sleep(pause);
                }
                Err(error) => {
                    if !matches!(error, StoreError::RetryableBusy) {
                        self.writer = None;
                    }
                    log::warn!(
                        "[context-history] batch of {} dropped: {error:?}",
                        batch.len()
                    );
                    return;
                }
            }
        }
        log::warn!("[context-history] batch of {} dropped: busy", batch.len());
    }

    fn writer_mut(&mut self) -> Result<&mut Box<dyn HistoryAppend>, StoreError> {
        if self.writer.is_none() {
            let writer = self
                .env
                .open_writer(Path::new(&self.ac_root), &self.config)?;
            self.writer = Some(writer);
        }
        self.writer.as_mut().ok_or(StoreError::Disabled)
    }
}

// ---- Actor -------------------------------------------------------------------------------

struct Project {
    epoch: u64,
    config: HistoryConfig,
    writer: Option<ProjectWriter>,
}

enum Tracked {
    Excluded,
    Scoped {
        ac_root: String,
        context: Box<InstanceContext>,
        /// `None` while the project's capture is disabled.
        state: Option<InstanceState>,
        epoch: u64,
    },
}

/// Starts and owns the history actor.
pub(crate) struct HistoryRuntime;

impl HistoryRuntime {
    /// Build the actor and its ingress; the caller spawns the returned future
    /// on its async runtime. `shutdown` cancellation triggers a flush bounded
    /// by [`SHUTDOWN_FLUSH`].
    pub(crate) fn start(
        scopes: Arc<HistoryScopeState>,
        shutdown: CancellationToken,
    ) -> (HistoryIngress, impl Future<Output = ()> + Send + 'static) {
        let (sender, receiver) = mpsc::channel(MAILBOX_CAPACITY);
        let ingress = HistoryIngress {
            sender,
            sequence: Arc::new(AtomicU64::new(0)),
            dropped: Arc::new(AtomicU64::new(0)),
            gap: Arc::new(AtomicBool::new(false)),
            last_utc_ms: Arc::new(AtomicI64::new(i64::MIN)),
            clock_back: Arc::new(AtomicBool::new(false)),
        };
        let actor = Actor {
            env: Arc::clone(scopes.env()),
            scopes,
            saturated: false,
            ingress: ingress.clone(),
            app_run_id: Uuid::new_v4(),
            projects: HashMap::new(),
            tracked: HashMap::new(),
            restart_closed: HashSet::new(),
            restart_intents: HashSet::new(),
            next_epoch: 1,
        };
        (ingress, actor.run(receiver, shutdown))
    }
}

struct Actor {
    scopes: Arc<HistoryScopeState>,
    env: Arc<dyn ProjectEnvironment>,
    /// Live state is full of live sessions; warned once until an admission.
    saturated: bool,
    ingress: HistoryIngress,
    app_run_id: Uuid,
    projects: HashMap<String, Project>,
    tracked: HashMap<Uuid, Tracked>,
    /// Old instances closed by a restart: their late samples write nothing.
    restart_closed: HashSet<Uuid>,
    /// Restart intents already closed; a retry of one is a no-op.
    restart_intents: HashSet<Uuid>,
    next_epoch: u64,
}

impl Actor {
    async fn run(
        mut self,
        mut receiver: mpsc::Receiver<IngressMessage>,
        shutdown: CancellationToken,
    ) {
        let mut poll = tokio::time::interval(CONFIG_POLL_INTERVAL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                message = receiver.recv() => match message {
                    Some(IngressMessage::Sample(sample)) => self.handle(sample).await,
                    Some(IngressMessage::Restart(notice)) => self.restart(notice),
                    None => break,
                },
                _ = poll.tick() => self.poll_configs().await,
            }
        }
        receiver.close();
        self.flush().await;
    }

    fn note_losses(&mut self) {
        if self.ingress.gap.swap(false, Ordering::Relaxed) {
            self.mark_gap(None);
        }
        self.mark_overflowed_projects();
    }

    async fn handle(&mut self, sample: StampedSample) {
        self.note_losses();
        if sample.value == ObservedValue::SessionOver {
            self.scopes.retire(sample.session_id);
        }
        if self.restart_closed.contains(&sample.session_id) {
            if sample.value != ObservedValue::SessionOver {
                return;
            }
            self.restart_closed.remove(&sample.session_id);
        }
        if !self.tracked.contains_key(&sample.session_id) {
            if sample.value == ObservedValue::SessionOver {
                return;
            }
            self.track(sample.session_id).await;
        }
        let retire = sample.value == ObservedValue::SessionOver;
        let produced = match self.tracked.get_mut(&sample.session_id) {
            Some(Tracked::Scoped {
                ac_root,
                context,
                state: Some(state),
                epoch,
            }) => state
                .observe(context, sample.value, sample.stamp, &mut Uuid::new_v4)
                .map(|record| (ac_root.clone(), *epoch, record)),
            _ => None,
        };
        if retire {
            self.tracked.remove(&sample.session_id);
        }
        if let Some((ac_root, epoch, record)) = produced {
            self.send(&ac_root, epoch, vec![record]);
        }
    }

    /// Apply one restart notice, then acknowledge it. Never touches disk.
    fn restart(&mut self, mut notice: RestartNotice) {
        self.note_losses();
        match notice.phase {
            RestartPhase::Closing => {
                if self.restart_intents.len() >= LIVE_STATE_CAPACITY {
                    self.restart_intents.clear();
                }
                if self.restart_intents.insert(notice.intent_id) {
                    self.close_for_restart(&notice);
                }
            }
            RestartPhase::TeardownFailed => self.reopen_after_failed_teardown(&notice),
        }
        if let Some(ack) = notice.ack.take() {
            let _ = ack.send(());
        }
    }

    fn close_for_restart(&mut self, notice: &RestartNotice) {
        if self.restart_closed.len() >= LIVE_STATE_CAPACITY {
            log::warn!("[context-history] restart capacity reached; old instance not isolated");
        } else {
            self.restart_closed.insert(notice.session_id);
        }
        let produced = match self.tracked.get_mut(&notice.session_id) {
            Some(Tracked::Scoped {
                ac_root,
                context,
                state: Some(state),
                epoch,
            }) => state
                .observe(
                    context,
                    ObservedValue::SessionOver,
                    notice.stamp,
                    &mut Uuid::new_v4,
                )
                .map(|record| (ac_root.clone(), *epoch, record)),
            _ => None,
        };
        if let Some((ac_root, epoch, record)) = produced {
            self.send(&ac_root, epoch, vec![record]);
        }
    }

    /// The old instance survived: let it observe again as a new segment whose
    /// start is unknown, keeping its own runtime identity.
    fn reopen_after_failed_teardown(&mut self, notice: &RestartNotice) {
        if !self.restart_closed.remove(&notice.session_id) {
            return;
        }
        if let Some(Tracked::Scoped {
            context,
            state: Some(state),
            ..
        }) = self.tracked.get_mut(&notice.session_id)
        {
            let mut reopened = InstanceState::default();
            // No session is open, so this only marks the next start as lost.
            let _ = reopened.observe_gap(context, notice.stamp, &mut Uuid::new_v4);
            *state = reopened;
        }
    }

    async fn track(&mut self, session_id: Uuid) {
        if self.tracked.len() >= LIVE_STATE_CAPACITY {
            self.prune_tracked().await;
            if self.tracked.len() >= LIVE_STATE_CAPACITY {
                if scopes::first_in_episode(&mut self.saturated) {
                    log::warn!(
                        "[context-history] live-state capacity reached; session not tracked"
                    );
                }
                return;
            }
        }
        // Absent or pending: keep nothing, so the next sample reads it again.
        let ScopeLookup::Resolved(resolved) = self.scopes.resolve(session_id).await else {
            return;
        };
        let ScopeResolution::Scoped(scope) = resolved.scope.clone() else {
            self.admit(session_id, Tracked::Excluded);
            return;
        };
        let launch = resolved.launch.clone();
        let ac_root = scope.layout.ac_root.clone();
        self.ensure_project(&ac_root).await;
        if !self.is_live(session_id).await {
            return;
        }
        let project = self.projects.get(&ac_root);
        let epoch = project.map_or(0, |project| project.epoch);
        let enabled = project.is_some_and(|project| project.config.enabled);
        let context = InstanceContext {
            app_run_id: self.app_run_id,
            runtime_session_id: session_id,
            scope,
            launch,
        };
        let state = enabled.then(InstanceState::default);
        let tracked = Tracked::Scoped {
            ac_root,
            context: Box::new(context),
            state,
            epoch,
        };
        self.admit(session_id, tracked);
    }

    fn admit(&mut self, session_id: Uuid, tracked: Tracked) {
        self.tracked.insert(session_id, tracked);
        self.saturated = false;
    }

    async fn is_live(&self, session_id: Uuid) -> bool {
        let live = self.scopes.sessions().live_ids(&[session_id]).await;
        live.is_some_and(|live| live.contains(&session_id))
    }

    /// Retire tracked sessions the manager no longer shows (absent, pending or
    /// exited), never a live one. An open session first gets its gap boundary;
    /// queues, epochs and projects stay untouched.
    async fn prune_tracked(&mut self) {
        let candidates: Vec<Uuid> = self
            .tracked
            .keys()
            .take(LIVE_STATE_CAPACITY)
            .copied()
            .collect();
        let Some(live) = self.scopes.sessions().live_ids(&candidates).await else {
            return;
        };
        for session_id in candidates.into_iter().filter(|id| !live.contains(id)) {
            if let Some(Tracked::Scoped {
                ac_root,
                context,
                state: Some(state),
                epoch,
            }) = self.tracked.get_mut(&session_id)
            {
                let stamp = self.ingress.next_stamp();
                if let Some(record) = state.observe_gap(context, stamp, &mut Uuid::new_v4) {
                    let (ac_root, epoch) = (ac_root.clone(), *epoch);
                    self.send(&ac_root, epoch, vec![record]);
                }
            }
            self.tracked.remove(&session_id);
        }
    }

    async fn load_config(&self, ac_root: &str) -> HistoryConfig {
        let env = Arc::clone(&self.env);
        let root = ac_root.to_string();
        let loaded = tokio::task::spawn_blocking(move || env.load_config(Path::new(&root))).await;
        loaded.map_or(HistoryConfig::DISABLED, |loaded| loaded.config)
    }

    async fn ensure_project(&mut self, ac_root: &str) {
        if self.projects.contains_key(ac_root) {
            return;
        }
        let config = self.load_config(ac_root).await;
        let project = Project {
            epoch: 0,
            config: HistoryConfig::DISABLED,
            writer: None,
        };
        self.projects.insert(ac_root.to_string(), project);
        self.apply_config(ac_root, config);
    }

    async fn poll_configs(&mut self) {
        let roots: Vec<String> = self.projects.keys().cloned().collect();
        for ac_root in roots {
            let config = self.load_config(&ac_root).await;
            self.apply_config(&ac_root, config);
        }
    }

    /// Enable and disable start a new epoch: queued records and live state of
    /// the project are discarded, and later samples start a new segment.
    fn apply_config(&mut self, ac_root: &str, config: HistoryConfig) {
        let next_epoch = self.next_epoch;
        let env = Arc::clone(&self.env);
        let Some(project) = self.projects.get_mut(ac_root) else {
            return;
        };
        if project.config == config && project.epoch != 0 {
            return;
        }
        let toggled = project.config.enabled != config.enabled || project.epoch == 0;
        if toggled {
            project.epoch = next_epoch;
            self.next_epoch += 1;
        }
        project.config = config;
        if config.enabled && project.writer.is_none() {
            project.writer = ProjectWriter::start(ac_root.to_string(), env);
        }
        if let Some(writer) = &project.writer {
            writer.configure(project.epoch, config);
        }
        if !toggled {
            return;
        }
        let epoch = project.epoch;
        for tracked in self.tracked.values_mut() {
            if let Tracked::Scoped {
                ac_root: root,
                state,
                epoch: segment,
                ..
            } = tracked
            {
                if root == ac_root {
                    *state = config.enabled.then(InstanceState::default);
                    *segment = epoch;
                }
            }
        }
    }

    fn mark_overflowed_projects(&mut self) {
        let overflowed: Vec<String> = self
            .projects
            .iter()
            .filter(|(_, project)| {
                project
                    .writer
                    .as_ref()
                    .is_some_and(|writer| writer.overflowed.swap(false, Ordering::Relaxed))
            })
            .map(|(root, _)| root.clone())
            .collect();
        for root in overflowed {
            self.mark_gap(Some(&root));
        }
    }

    /// Turn lost observations into bounded gap signals for the affected scope.
    fn mark_gap(&mut self, only: Option<&str>) {
        let mut produced: HashMap<String, (u64, Vec<HistoryRecord>)> = HashMap::new();
        for tracked in self.tracked.values_mut() {
            let Tracked::Scoped {
                ac_root,
                context,
                state: Some(state),
                epoch,
            } = tracked
            else {
                continue;
            };
            if only.is_some_and(|only| only != ac_root) {
                continue;
            }
            let stamp = self.ingress.next_stamp();
            if let Some(record) = state.observe_gap(context, stamp, &mut Uuid::new_v4) {
                let entry = produced
                    .entry(ac_root.clone())
                    .or_insert((*epoch, Vec::new()));
                entry.1.push(record);
            }
        }
        for (ac_root, (epoch, records)) in produced {
            self.send(&ac_root, epoch, records);
        }
    }

    fn send(&self, ac_root: &str, epoch: u64, records: Vec<HistoryRecord>) {
        let Some(project) = self.projects.get(ac_root) else {
            return;
        };
        if !project.config.enabled || project.epoch != epoch {
            return;
        }
        if let Some(writer) = &project.writer {
            writer.enqueue(epoch, records);
        }
    }

    /// Flush every project writer within [`SHUTDOWN_FLUSH`]; never waits longer.
    async fn flush(&mut self) {
        let deadline = Instant::now() + SHUTDOWN_FLUSH;
        let mut waits = Vec::new();
        for project in self.projects.values() {
            let Some(writer) = &project.writer else {
                continue;
            };
            let (done, wait) = oneshot::channel();
            if writer
                .sender
                .send(WorkerMessage::Flush { deadline, done })
                .is_ok()
            {
                waits.push(wait);
            }
        }
        let budget = tokio::time::Instant::from_std(deadline);
        for wait in waits {
            if tokio::time::timeout_at(budget, wait).await.is_err() {
                log::warn!("[context-history] shutdown flush timed out; pending records lost");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
