use super::*;
use std::collections::VecDeque;

mod lifecycle;
mod occurrence;
mod payload;
mod state;

type LivenessHook = Arc<dyn Fn(Uuid) + Send + Sync>;

/// What the scripted backend paints on the next read.
pub(super) enum Painted {
    Frame(Vec<String>, Vec<bool>, u16),
    Unchanged,
    Missing,
    Gone,
    /// #2879 - the last frame again under its SAME stamp, answered as a real backend
    /// would: `Unchanged` to a reader that already saw that stamp, the frame otherwise.
    Same,
}

/// The rows of a frame with their wrap flags and cursor row: what a repaint replays.
type PaintedRows = (Vec<String>, Vec<bool>, u16);

/// A `PtyBackend` whose only real method is the watcher seam. Every other method is a
/// stub, which is exactly the point: the engine never calls one.
#[derive(Default)]
pub(super) struct ScriptedBackend {
    script: Mutex<HashMap<Uuid, VecDeque<Painted>>>,
    last: Mutex<HashMap<Uuid, PaintedRows>>,
    sequence: Mutex<HashMap<Uuid, u64>>,
    pub(super) reads: AtomicUsize,
    /// #2879 - the `seen` stamp of every read, in order.
    pub(super) seen: Mutex<Vec<Option<FrameStamp>>>,
}

impl ScriptedBackend {
    /// Paint a screen with the cursor parked on row 0, so a row arriving by scroll is
    /// evaluated on arrival. `paint_with_cursor` is for the tests that are ABOUT the
    /// cursor rule.
    pub(super) fn paint(&self, id: Uuid, rows: &[&str]) {
        self.paint_with_cursor(id, rows, 0);
    }

    pub(super) fn paint_with_cursor(&self, id: Uuid, rows: &[&str], cursor_row: u16) {
        self.push(
            id,
            Painted::Frame(
                rows.iter().map(|r| r.to_string()).collect(),
                vec![false; rows.len()],
                cursor_row,
            ),
        );
    }

    pub(super) fn push(&self, id: Uuid, painted: Painted) {
        self.script
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .push_back(painted);
    }
}

impl PtyBackend for ScriptedBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn spawn(
        &self,
        _spec: crate::pty::backend::BackendSpawnSpec,
    ) -> BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async { Ok(()) })
    }
    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        _id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        unreachable!("the engine must never be able to write to a PTY")
    }
    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        unreachable!("the engine must never be able to resize a PTY")
    }
    fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
        unreachable!("the engine must never be able to kill a session")
    }
    fn has_session(&self, _id: Uuid) -> bool {
        true
    }
    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }
    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
    }
    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        unreachable!("the engine reads through the #1171 seam, never the 5 s one")
    }
    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: std::path::PathBuf,
    ) {
    }
    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }
    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }

    fn screen_rows_since(&self, id: Uuid, seen: Option<FrameStamp>) -> ScreenRowsSince {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.seen.lock().unwrap().push(seen);
        let next = self
            .script
            .lock()
            .unwrap()
            .get_mut(&id)
            .and_then(|queue| queue.pop_front());
        let (rows, wrapped, cursor_row) = match next {
            Some(Painted::Missing) => return ScreenRowsSince::Missing,
            Some(Painted::Gone) => return ScreenRowsSince::Gone,
            Some(Painted::Unchanged) => return ScreenRowsSince::Unchanged,
            Some(Painted::Same) => {
                let (rows, wrapped, cursor_row) = self
                    .last
                    .lock()
                    .unwrap()
                    .get(&id)
                    .cloned()
                    .unwrap_or_default();
                let stamp = FrameStamp {
                    sequence: self.sequence.lock().unwrap().get(&id).copied().unwrap_or(0),
                    rows: rows.len() as u16,
                    cols: 120,
                };
                if seen == Some(stamp) {
                    return ScreenRowsSince::Unchanged;
                }
                return ScreenRowsSince::Frame(ScreenFrame {
                    rows,
                    wrapped,
                    cursor_row,
                    stamp: Some(stamp),
                });
            }
            Some(Painted::Frame(rows, wrapped, cursor_row)) => {
                self.last
                    .lock()
                    .unwrap()
                    .insert(id, (rows.clone(), wrapped.clone(), cursor_row));
                (rows, wrapped, cursor_row)
            }
            // A script that ran out repaints the same screen with a NEW sequence, which is
            // what a session with a live spinner does several times per second.
            None => self
                .last
                .lock()
                .unwrap()
                .get(&id)
                .cloned()
                .unwrap_or_default(),
        };
        let mut sequences = self.sequence.lock().unwrap();
        let sequence = sequences.entry(id).or_insert(0);
        *sequence += 1;
        ScreenRowsSince::Frame(ScreenFrame {
            rows: rows.clone(),
            wrapped,
            cursor_row,
            stamp: Some(FrameStamp {
                sequence: *sequence,
                rows: rows.len() as u16,
                cols: 120,
            }),
        })
    }
}

pub(super) struct FakeBackends {
    pub(super) backend: Arc<ScriptedBackend>,
    pub(super) liveness: Mutex<HashMap<Uuid, ContextSessionLiveness>>,
    pub(super) resolutions: AtomicUsize,
    pub(super) liveness_calls: AtomicUsize,
    pub(super) on_liveness: Mutex<Option<LivenessHook>>,
}

impl WatcherBackendSource for FakeBackends {
    fn reader_for(&self, _id: Uuid) -> Option<SessionFrameReader> {
        self.resolutions.fetch_add(1, Ordering::Relaxed);
        Some(SessionFrameReader::new(
            Arc::clone(&self.backend) as Arc<dyn PtyBackend>
        ))
    }

    fn liveness(&self, id: Uuid) -> ContextSessionLiveness {
        self.liveness_calls.fetch_add(1, Ordering::Relaxed);
        let hook = self.on_liveness.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(id);
        }
        self.liveness
            .lock()
            .unwrap()
            .get(&id)
            .copied()
            .unwrap_or(ContextSessionLiveness::Live)
    }
}

#[derive(Default)]
pub(super) struct FakePatterns(
    pub(super) Mutex<HashMap<String, AgentResolution>>,
    pub(super) std::sync::atomic::AtomicBool,
);

impl WatcherPatternSource for FakePatterns {
    fn resolve(&self) -> BoxFuture<'_, Option<HashMap<String, AgentResolution>>> {
        Box::pin(async move {
            if self.1.load(Ordering::Relaxed) {
                None
            } else {
                Some(self.0.lock().unwrap().clone())
            }
        })
    }
}

#[derive(Default)]
pub(super) struct FakeSink(Mutex<Vec<WatcherMatchBatch>>);

impl WatcherEventSink for FakeSink {
    fn emit(&self, batch: WatcherMatchBatch) {
        self.0.lock().unwrap().push(batch);
    }
}

pub(super) struct Harness {
    pub(super) engine: Arc<WatcherEngine>,
    pub(super) backend: Arc<ScriptedBackend>,
    pub(super) backends: Arc<FakeBackends>,
    pub(super) patterns: Arc<FakePatterns>,
    pub(super) sink: Arc<FakeSink>,
    pub(super) history: history::WatcherHistoryState,
}

impl Harness {
    pub(super) fn new() -> Self {
        Self::build(None)
    }

    /// #2879 - the same harness with an internal activity factory.
    pub(super) fn with_activity_factory(factory: ActivityFactory) -> Self {
        Self::build(Some(factory))
    }

    fn build(factory: Option<ActivityFactory>) -> Self {
        let backend = Arc::new(ScriptedBackend::default());
        let backends = Arc::new(FakeBackends {
            backend: Arc::clone(&backend),
            liveness: Mutex::new(HashMap::new()),
            resolutions: AtomicUsize::new(0),
            liveness_calls: AtomicUsize::new(0),
            on_liveness: Mutex::new(None),
        });
        let patterns = Arc::new(FakePatterns::default());
        let sink = Arc::new(FakeSink::default());
        let history: history::WatcherHistoryState = Arc::new(history::WatcherHistory::default());
        let (backends_dyn, patterns_dyn, sink_dyn) = (
            Arc::clone(&backends) as Arc<dyn WatcherBackendSource>,
            Arc::clone(&patterns) as Arc<dyn WatcherPatternSource>,
            Arc::clone(&sink) as Arc<dyn WatcherEventSink>,
        );
        let engine = match factory {
            None => WatcherEngine::new(backends_dyn, patterns_dyn, sink_dyn, Arc::clone(&history)),
            factory => WatcherEngine::new_with_activity_factory(
                backends_dyn,
                patterns_dyn,
                sink_dyn,
                Arc::clone(&history),
                factory,
            ),
        };
        Self {
            engine,
            backend,
            backends,
            patterns,
            sink,
            history,
        }
    }

    pub(super) fn configure(&self, agent_id: &str, watchers: Vec<ResolvedWatcher>) {
        self.patterns.0.lock().unwrap().insert(
            agent_id.to_string(),
            AgentResolution {
                running: watchers.into_iter().map(Arc::new).collect(),
                over_budget: Vec::new(),
            },
        );
    }

    pub(super) async fn ticks(&self, count: usize) {
        for _ in 0..count {
            self.engine.tick().await;
        }
    }

    pub(super) fn emitted(&self) -> Vec<WatcherMatchPayload> {
        self.sink
            .0
            .lock()
            .unwrap()
            .iter()
            .flat_map(|batch| batch.matches.clone())
            .collect()
    }

    pub(super) fn batches(&self) -> usize {
        self.sink.0.lock().unwrap().len()
    }
}

pub(super) fn state_watcher(id: &str, pattern: &str) -> ResolvedWatcher {
    ResolvedWatcher {
        id: id.to_string(),
        mode: WatcherMode::State,
        pattern: pattern.to_string(),
        dedupe: WatcherDedupe::Row,
        dedupe_window_ms: 2000,
    }
}

pub(super) fn as_refs(rows: &[String]) -> Vec<&str> {
    rows.iter().map(|row| row.as_str()).collect()
}

pub(super) fn occurrence_watcher(
    id: &str,
    pattern: &str,
    dedupe: WatcherDedupe,
    window_ms: u64,
) -> ResolvedWatcher {
    ResolvedWatcher {
        id: id.to_string(),
        mode: WatcherMode::Occurrence,
        pattern: pattern.to_string(),
        dedupe,
        dedupe_window_ms: window_ms,
    }
}
