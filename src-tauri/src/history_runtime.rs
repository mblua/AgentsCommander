//! Context-history runtime (#3532 P02): bounded capture of context changes.
//!
//! The scraper's sample bridge offers every outcome to [`HistoryIngress`],
//! which stamps UTC time and a monotonic sequence and `try_send`s into a
//! bounded mailbox; it never blocks, awaits or touches disk. One actor owns
//! the per-instance transition state, resolves each session's scope once, and
//! hands records to one serial writer thread per project. No session or PTY
//! lock is held during history I/O, and no `AppHandle` reaches this module:
//! `lib`/`setup` compose it and own the shutdown hook.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::context_history::config::{load_history_config, HistoryConfig, LoadedHistoryConfig};
use crate::context_history::observation::{
    parse_history_layout, parse_launch_metadata, resolve_history_scope, InstanceContext,
    InstanceState, ObservedValue, ScopeInput, ScopeResolution, Stamp, TeamMembers,
};
use crate::context_history::store::{AppendOutcome, HistoryWriter, StoreError, MAX_APPEND_BATCH};
use crate::context_history::HistoryRecord;

/// Bounded ingress mailbox between the scraper bridge and the actor.
pub(crate) const MAILBOX_CAPACITY: usize = 4096;
/// Maximum tracked instances (scoped or excluded) at once.
pub(crate) const LIVE_STATE_CAPACITY: usize = 4096;
/// Maximum records queued, in flight or retrying for one project.
pub(crate) const PENDING_CAPACITY: usize = 4096;
/// How often `.ac/heatmap.json` is re-read; configuration, not sampling.
pub(crate) const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// A busy batch is retried in FIFO order for at least this long...
pub(crate) const BUSY_MIN_SPAN: Duration = Duration::from_secs(1);
/// ...in at most this many append calls, the first included.
pub(crate) const BUSY_MAX_CALLS: u32 = 10;
/// Shutdown flush budget; whatever is left afterwards is dropped and logged.
pub(crate) const SHUTDOWN_FLUSH: Duration = Duration::from_secs(2);

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

/// Nonblocking, fail-soft entry point used by the scraper's sample bridge.
#[derive(Clone)]
pub(crate) struct HistoryIngress {
    sender: mpsc::Sender<StampedSample>,
    sequence: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    /// Set when samples were lost; the actor turns it into gap boundaries.
    gap: Arc<AtomicBool>,
}

impl HistoryIngress {
    /// Stamp and offer one sample. A full mailbox drops it, counts it and
    /// signals a discontinuity; it never waits.
    pub(crate) fn offer(&self, session_id: Uuid, value: ObservedValue) {
        let stamp = Stamp {
            observed_at_utc_ms: now_utc_ms(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
        };
        let sample = StampedSample {
            session_id,
            value,
            stamp,
        };
        // A closed mailbox means the actor already stopped: the sample is moot.
        if let Err(mpsc::error::TrySendError::Full(_)) = self.sender.try_send(sample) {
            let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if !self.gap.swap(true, Ordering::Relaxed) {
                log::warn!("[context-history] mailbox full; sample dropped (dropped={dropped})");
            }
        }
    }

    fn next_stamp(&self) -> Stamp {
        Stamp {
            observed_at_utc_ms: now_utc_ms(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
        }
    }
}

// ---- Collaborators -----------------------------------------------------------------------

/// The launch facts of one live session, copied out of the session manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionLaunch {
    pub(crate) session_id: Uuid,
    pub(crate) working_directory: String,
    pub(crate) shell: String,
    pub(crate) effective_shell_args: Option<Vec<String>>,
    pub(crate) is_root_agent: bool,
}

/// Where the runtime reads a session's launch facts. The implementation copies
/// them and releases its lock before returning.
pub(crate) trait SessionLaunchSource: Send + Sync {
    fn launch(&self, session_id: Uuid) -> BoxFuture<'_, Option<SessionLaunch>>;
}

/// Project-level reads: configuration, team snapshot, AC-root authority and
/// the writer. Every method may touch disk and runs off the async workers.
pub(crate) trait ProjectEnvironment: Send + Sync + 'static {
    fn load_config(&self, ac_root: &Path) -> LoadedHistoryConfig;
    fn authoritative_ac_root(&self, ac_root: &Path) -> bool;
    fn teams(&self, project_path: &Path) -> Vec<TeamMembers>;
    fn open_writer(
        &self,
        ac_root: &Path,
        config: &HistoryConfig,
    ) -> Result<Box<dyn HistoryAppend>, StoreError>;
}

/// The append half of the store, boxed so tests can script outcomes.
pub(crate) trait HistoryAppend: Send {
    fn append(
        &mut self,
        records: &[HistoryRecord],
        now_utc_ms: i64,
    ) -> Result<AppendOutcome, StoreError>;
}

impl HistoryAppend for HistoryWriter {
    fn append(
        &mut self,
        records: &[HistoryRecord],
        now_utc_ms: i64,
    ) -> Result<AppendOutcome, StoreError> {
        HistoryWriter::append(self, records, now_utc_ms)
    }
}

/// Production environment: the real config owner, team discovery and store.
pub(crate) struct DiskProjectEnvironment;

impl ProjectEnvironment for DiskProjectEnvironment {
    fn load_config(&self, ac_root: &Path) -> LoadedHistoryConfig {
        load_history_config(ac_root)
    }

    fn authoritative_ac_root(&self, ac_root: &Path) -> bool {
        crate::config::ac_root::ensure_authoritative_ac_root(ac_root).is_ok()
    }

    fn teams(&self, project_path: &Path) -> Vec<TeamMembers> {
        let mut teams = Vec::new();
        crate::config::teams::discover_teams_in_project_filtered(project_path, &mut teams, |_| {
            true
        });
        teams
            .into_iter()
            .map(|team| TeamMembers {
                team_id: team.name,
                member_paths: team
                    .agent_paths
                    .into_iter()
                    .flatten()
                    .chain(team.coordinator_path)
                    .filter_map(|path| path.to_str().map(str::to_string))
                    .collect(),
            })
            .collect()
    }

    fn open_writer(
        &self,
        ac_root: &Path,
        config: &HistoryConfig,
    ) -> Result<Box<dyn HistoryAppend>, StoreError> {
        Ok(Box::new(HistoryWriter::open(ac_root, config)?))
    }
}

fn is_root_session(launch: &SessionLaunch) -> bool {
    launch.is_root_agent
        || crate::config::root_agent::is_root_agent_dir_name(&launch.working_directory)
        || crate::config::root_agent::is_root_agent_path(&launch.working_directory)
}

/// Resolve the scope of one session from an already-copied launch snapshot.
fn resolve_launch(launch: &SessionLaunch, env: &dyn ProjectEnvironment) -> ScopeResolution {
    let is_root_agent = is_root_session(launch);
    let (authoritative_ac_root, teams) = match parse_history_layout(&launch.working_directory) {
        Ok(layout) if !is_root_agent => (
            env.authoritative_ac_root(Path::new(&layout.ac_root)),
            env.teams(Path::new(&layout.project_path)),
        ),
        _ => (false, Vec::new()),
    };
    resolve_history_scope(&ScopeInput {
        working_directory: &launch.working_directory,
        is_root_agent,
        authoritative_ac_root,
        teams: &teams,
    })
}

/// The scopes the sidebar shows, from the same resolver capture uses. Works
/// whether or not capture is enabled and never reads history or private files.
// Consumed by the history IPC (#3532 P05); only tests call it until then.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn snapshot_sidebar_scopes(
    sessions: &[SessionLaunch],
    env: &dyn ProjectEnvironment,
) -> Vec<(Uuid, ScopeResolution)> {
    sessions
        .iter()
        .map(|launch| (launch.session_id, resolve_launch(launch, env)))
        .collect()
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
        sessions: Arc<dyn SessionLaunchSource>,
        env: Arc<dyn ProjectEnvironment>,
        shutdown: CancellationToken,
    ) -> (HistoryIngress, impl Future<Output = ()> + Send + 'static) {
        let (sender, receiver) = mpsc::channel(MAILBOX_CAPACITY);
        let ingress = HistoryIngress {
            sender,
            sequence: Arc::new(AtomicU64::new(0)),
            dropped: Arc::new(AtomicU64::new(0)),
            gap: Arc::new(AtomicBool::new(false)),
        };
        let actor = Actor {
            sessions,
            env,
            ingress: ingress.clone(),
            app_run_id: Uuid::new_v4(),
            projects: HashMap::new(),
            tracked: HashMap::new(),
            next_epoch: 1,
        };
        (ingress, actor.run(receiver, shutdown))
    }
}

struct Actor {
    sessions: Arc<dyn SessionLaunchSource>,
    env: Arc<dyn ProjectEnvironment>,
    ingress: HistoryIngress,
    app_run_id: Uuid,
    projects: HashMap<String, Project>,
    tracked: HashMap<Uuid, Tracked>,
    next_epoch: u64,
}

impl Actor {
    async fn run(
        mut self,
        mut receiver: mpsc::Receiver<StampedSample>,
        shutdown: CancellationToken,
    ) {
        let mut poll = tokio::time::interval(CONFIG_POLL_INTERVAL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                sample = receiver.recv() => match sample {
                    Some(sample) => self.handle(sample).await,
                    None => break,
                },
                _ = poll.tick() => self.poll_configs().await,
            }
        }
        receiver.close();
        self.flush().await;
    }

    async fn handle(&mut self, sample: StampedSample) {
        if self.ingress.gap.swap(false, Ordering::Relaxed) {
            self.mark_gap(None);
        }
        self.mark_overflowed_projects();
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

    async fn track(&mut self, session_id: Uuid) {
        if self.tracked.len() >= LIVE_STATE_CAPACITY {
            log::warn!("[context-history] live-state capacity reached; session not tracked");
            return;
        }
        let Some(launch) = self.sessions.launch(session_id).await else {
            self.tracked.insert(session_id, Tracked::Excluded);
            return;
        };
        let env = Arc::clone(&self.env);
        let resolved = tokio::task::spawn_blocking(move || {
            let scope = resolve_launch(&launch, env.as_ref());
            let args = launch.effective_shell_args.as_deref();
            (scope, parse_launch_metadata(&launch.shell, args))
        })
        .await;
        let Ok((ScopeResolution::Scoped(scope), launch)) = resolved else {
            self.tracked.insert(session_id, Tracked::Excluded);
            return;
        };
        let ac_root = scope.layout.ac_root.clone();
        self.ensure_project(&ac_root).await;
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
        self.tracked.insert(session_id, tracked);
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
