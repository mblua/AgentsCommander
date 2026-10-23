use super::*;
use std::collections::VecDeque;

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

/// 9.3.25 and 9.3.26 - **the zero-cost promise, and the lock promise, together.**
///
/// With a session registered and no watcher reaching its agent, the tick returns before
/// touching the session: no read, so no `screen_parsers` lock and no row allocation. And
/// the backend `Arc` is resolved exactly ONCE, at registration, so the tick path never
/// goes through `PtyManager` or the route registry however many times it runs.
#[tokio::test]
async fn an_unconfigured_app_touches_no_session_and_resolves_no_backend_per_tick() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));

    harness.ticks(10).await;

    assert_eq!(harness.backend.reads.load(Ordering::Relaxed), 0);
    assert_eq!(
        harness.backends.resolutions.load(Ordering::Relaxed),
        1,
        "registration resolves the backend; no tick may resolve it again"
    );

    // ...and with a watcher configured, the reads happen and the resolution count is still 1.
    harness.configure("a1", vec![state_watcher("w", "never")]);
    harness.ticks(5).await;
    assert_eq!(harness.backend.reads.load(Ordering::Relaxed), 5);
    assert_eq!(harness.backends.resolutions.load(Ordering::Relaxed), 1);
}

/// 9.3.27 and acceptance criterion A4 - a state reading is idempotent. Twenty-five ticks -
/// five seconds of a screen being repainted with the same content, which is what a live
/// spinner does - produce ONE event.
#[tokio::test]
async fn a_state_watcher_emits_once_for_a_value_that_does_not_change() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("ctx", r"Context (\d+)%")]);
    harness.backend.paint(id, &["idle", "Context 42%"]);

    harness.ticks(25).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].captures, vec![Some("42".to_string())]);
    assert_eq!(emitted[0].row, "Context 42%");
    assert_eq!(emitted[0].mode, WatcherMode::State);
    assert_eq!(emitted[0].seq, 1);
}

/// 9.3.28 - ...and it emits again the moment the reading changes.
#[tokio::test]
async fn a_state_watcher_emits_again_when_the_matched_row_changes() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("ctx", r"Context (\d+)%")]);
    harness.backend.paint(id, &["Context 42%"]);
    harness.backend.paint(id, &["Context 42%"]);
    harness.backend.paint(id, &["Context 39%"]);

    harness.ticks(3).await;

    let captures: Vec<_> = harness
        .emitted()
        .iter()
        .map(|m| m.captures[0].clone().unwrap())
        .collect();
    assert_eq!(captures, vec!["42", "39"]);
}

/// 9.3.29 - a state watcher that stops matching emits NOTHING. "The prompt disappeared" is
/// not a log entry, and clearing the gate is exactly what lets an identical re-appearance
/// emit again.
#[tokio::test]
async fn a_state_watcher_that_stops_matching_emits_nothing_and_re_emits_on_reappearance() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness.backend.paint(id, &["Permission required"]);
    harness.backend.paint(id, &["gone"]);
    harness.backend.paint(id, &["Permission required"]);

    harness.ticks(3).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 2, "appearance, silence, appearance");
    assert_eq!(emitted[0].row, "Permission required");
    assert_eq!(emitted[1].row, "Permission required");
    assert_ne!(emitted[0].seq, emitted[1].seq);
}

/// 9.3.30 - **the case a `(captures, row)` gate cannot express, and the strongest argument
/// for this engine existing.** A second permission prompt appears while the first is still
/// on screen: the lowest match reads the same text, so only the generation can tell them
/// apart.
#[tokio::test]
async fn a_second_identical_match_appearing_above_an_existing_one_emits() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness.backend.paint(id, &["Permission required", "idle"]);
    harness
        .backend
        .paint(id, &["Permission required", "Permission required"]);

    harness.ticks(2).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 2);
    assert_eq!(
        emitted[0].row, emitted[1].row,
        "identical text: a (captures, row) gate would have suppressed the second"
    );
}

/// 9.3.31 - a scroll that leaves the match count unchanged emits nothing. This is what
/// "incrementing the generation only on a RISE" buys: the count of a persistent condition
/// does not change as the screen moves under it.
#[tokio::test]
async fn a_scroll_that_leaves_the_match_count_unchanged_emits_nothing() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness
        .backend
        .paint(id, &["Permission required", "a", "b"]);
    harness
        .backend
        .paint(id, &["a", "Permission required", "b"]);
    harness
        .backend
        .paint(id, &["a", "b", "Permission required"]);

    harness.ticks(3).await;

    assert_eq!(harness.emitted().len(), 1);
}

/// 9.3.32 - the LOWEST match wins, mirroring `rows::extract`, because a statusline always
/// sits below the transcript.
#[tokio::test]
async fn the_lowest_matching_logical_row_wins() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("ctx", r"Context (\d+)%")]);
    harness
        .backend
        .paint(id, &["Context 99%", "middle", "Context 42%"]);

    harness.ticks(1).await;

    assert_eq!(harness.emitted()[0].captures[0].as_deref(), Some("42"));
}

/// 9.3.33 - a pattern that does not compile is compiled ONCE and logged once, however many
/// ticks pass. The compile site is the log site, so the compile count IS the log count.
#[tokio::test]
async fn an_uncompilable_pattern_is_compiled_once_across_ten_ticks() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("broken", r"Read \((.+")]);
    harness.backend.paint(id, &["anything"]);

    harness.ticks(10).await;

    assert_eq!(harness.engine.compile_count(), 1);
    assert!(harness.emitted().is_empty());
}

/// A watcher whose pattern is edited recompiles, and its gate starts again - so the first
/// match under the new pattern is an event even if the old one had already emitted.
#[tokio::test]
async fn editing_a_pattern_recompiles_and_clears_that_watchers_gate() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "alpha")]);
    harness.backend.paint(id, &["alpha beta"]);
    harness.ticks(3).await;
    assert_eq!(harness.engine.compile_count(), 1);
    assert_eq!(harness.emitted().len(), 1);

    harness.configure("a1", vec![state_watcher("w", "beta")]);
    harness.ticks(3).await;

    assert_eq!(harness.engine.compile_count(), 2);
    assert_eq!(
        harness.emitted().len(),
        2,
        "the row is the same, but the question is not: the gate must not suppress it"
    );
}

/// 9.3.34 (first half) - `Gone` retires the session on that tick, with no waiting for the
/// 5 s probe.
#[tokio::test]
async fn a_session_reported_gone_is_retired_on_that_tick() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "x")]);
    harness.backend.push(id, Painted::Gone);
    harness.backend.push(id, Painted::Gone);

    harness.ticks(1).await;

    assert!(!harness.engine.is_session_registered(id));
}

/// `Missing` says NOTHING about the session, so it is kept and sampled again. Retiring on
/// it would strand a live session whose parser was momentarily unreadable.
#[tokio::test]
async fn a_session_reported_missing_is_kept() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "x")]);
    harness.backend.push(id, Painted::Missing);
    harness.backend.push(id, Painted::Missing);

    harness.ticks(1).await;

    assert!(harness.engine.is_session_registered(id));
}

/// 9.3.34 (second half) and 9.3.35 - liveness is probed on tick 25 and on no tick before
/// it, which is once per 5 s per session: exactly the rate the 5 s scraper probes at, and
/// the reason this engine is cheaper than the one it sits beside rather than 25x dearer.
#[tokio::test]
async fn liveness_is_probed_on_the_twenty_fifth_tick_and_retires_a_dead_session() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "never")]);
    harness.backend.paint(id, &["idle"]);
    harness
        .backends
        .liveness
        .lock()
        .unwrap()
        .insert(id, ContextSessionLiveness::SessionOver);

    harness.ticks(24).await;
    assert_eq!(harness.backends.liveness_calls.load(Ordering::Relaxed), 0);
    assert!(harness.engine.is_session_registered(id));

    harness.ticks(1).await;
    assert_eq!(harness.backends.liveness_calls.load(Ordering::Relaxed), 1);
    assert!(!harness.engine.is_session_registered(id));
}

/// 9.3.37 - one event per `(session, tick)`, carrying every match that tick produced. The
/// coalescing is what keeps the IPC load off the per-tick caps.
#[tokio::test]
async fn one_event_carries_all_of_a_ticks_matches_for_a_session() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![
            state_watcher("perm", "Permission required"),
            state_watcher("ctx", r"Context (\d+)%"),
        ],
    );
    harness
        .backend
        .paint(id, &["Permission required", "Context 42%"]);

    harness.ticks(1).await;

    assert_eq!(harness.batches(), 1);
    let batch = &harness.sink.0.lock().unwrap()[0];
    assert_eq!(batch.session_id, id.to_string());
    assert_eq!(batch.matches.len(), 2);
    assert_eq!(batch.matches[0].seq, 1);
    assert_eq!(batch.matches[1].seq, 2);
}

/// An `Unchanged` read evaluates nothing: the screen the caller last saw cannot have
/// started or stopped matching.
#[tokio::test]
async fn an_unchanged_frame_produces_no_event() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "match me")]);
    harness.backend.push(id, Painted::Unchanged);
    harness.backend.push(id, Painted::Unchanged);

    harness.ticks(2).await;

    assert!(harness.emitted().is_empty());
}

/// A session whose agent no watcher reaches is skipped, even while another session on a
/// configured agent is being sampled.
#[tokio::test]
async fn a_session_whose_agent_has_no_watcher_is_skipped() {
    let harness = Harness::new();
    let watched = Uuid::new_v4();
    let ignored = Uuid::new_v4();
    harness
        .engine
        .register_session(watched, Some("a1".to_string()));
    harness
        .engine
        .register_session(ignored, Some("a2".to_string()));
    harness.configure("a1", vec![state_watcher("w", "hit")]);
    harness.backend.paint(watched, &["hit"]);
    harness.backend.paint(ignored, &["hit"]);

    harness.ticks(1).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].session_id, watched.to_string());
}

/// A wrapped path is matched as ONE logical row, which is the whole reason evaluation is
/// not on physical rows.
#[tokio::test]
async fn a_state_watcher_matches_across_a_wrapped_row() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("read", r"Read \((.+)\)")]);
    harness.backend.push(
        id,
        Painted::Frame(
            vec![
                "idle".to_string(),
                "Read (C:/repo/very/long/pa".to_string(),
                "th/to/main.rs)".to_string(),
            ],
            vec![false, true, false],
            0,
        ),
    );

    harness.ticks(1).await;

    assert_eq!(
        harness.emitted()[0].captures[0].as_deref(),
        Some("C:/repo/very/long/path/to/main.rs")
    );
}

/// 9.6.38 - a row past the byte cap is truncated on a char boundary and SAYS so, because
/// `row.length >= 256` cannot answer that question in TypeScript.
#[tokio::test]
async fn an_over_long_row_is_truncated_on_a_char_boundary_and_flagged() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "start")]);
    // 3-byte chars, so the cap lands mid-character unless the boundary is respected.
    let long = format!("start{}", "\u{4e2d}".repeat(200));
    harness.backend.paint(id, &[&long]);

    harness.ticks(1).await;

    let emitted = harness.emitted();
    assert!(emitted[0].row_truncated);
    assert!(emitted[0].row.len() <= MAX_ROW_BYTES);
    assert!(long.starts_with(&emitted[0].row));
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

/// 9.4.45 (the engine half) - **the cursor-row rule, end to end.**
///
/// A path is written in two chunks across a tick boundary. Without the rule this produces
/// two events - one with `C:/repo/very/long/pa` and one with the whole path - and no
/// dedupe setting can merge them, because both the row AND the captures differ. With it,
/// exactly one event carries the complete capture.
#[tokio::test]
async fn a_row_written_in_two_chunks_produces_one_event_with_the_complete_capture() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher(
            "read",
            r"Read \((.+)\)",
            WatcherDedupe::None,
            2000,
        )],
    );

    // Seed, then scroll the half-written row in UNDER THE CURSOR, then finish it, then
    // hold still. The cursor on the bottom row is what a terminal that just printed a
    // newline really looks like.
    harness.backend.paint_with_cursor(id, &["a", "b", "c"], 2);
    harness
        .backend
        .paint_with_cursor(id, &["b", "c", "Read (C:/repo/very/long/pa"], 2);
    harness.backend.paint_with_cursor(
        id,
        &["b", "c", "Read (C:/repo/very/long/path/to/main.rs)"],
        2,
    );
    harness.backend.paint_with_cursor(
        id,
        &["b", "c", "Read (C:/repo/very/long/path/to/main.rs)"],
        2,
    );

    harness.ticks(4).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 1, "got {emitted:#?}");
    assert_eq!(
        emitted[0].captures[0].as_deref(),
        Some("C:/repo/very/long/path/to/main.rs")
    );
}

/// 9.4.50 - the same text at two positions at two times counts TWICE, and the two events
/// are told apart by `seq`. Layer 1 must not confuse them for each other; `at` cannot tell
/// them apart, which is why `seq` exists.
#[tokio::test]
async fn the_same_text_arriving_twice_counts_twice_with_different_seq() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher(
            "done",
            "Done",
            WatcherDedupe::None,
            2000,
        )],
    );
    harness.backend.paint(id, &["a", "b", "c"]);
    harness.backend.paint(id, &["b", "c", "Done"]);
    harness.backend.paint(id, &["c", "Done", "Done"]);

    harness.ticks(3).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 2);
    assert_eq!(emitted[0].row, emitted[1].row);
    assert_ne!(emitted[0].seq, emitted[1].seq);
}

/// 9.4.52 - the three dedupe keys, on the residual case layer 1 cannot separate: the same
/// path printed twice, truncated differently by the terminal.
#[tokio::test]
async fn the_capture_key_collapses_differently_truncated_rows_and_none_lets_everything_through() {
    for (dedupe, expected) in [(WatcherDedupe::Capture, 1), (WatcherDedupe::None, 2)] {
        let harness = Harness::new();
        let id = Uuid::new_v4();
        harness.engine.register_session(id, Some("a1".to_string()));
        harness.configure(
            "a1",
            vec![occurrence_watcher("read", r"Read (\S+)", dedupe, 60_000)],
        );
        harness.backend.paint(id, &["a", "b", "c"]);
        harness
            .backend
            .paint(id, &["b", "c", "Read C:/repo/main.rs"]);
        harness.backend.paint(
            id,
            &["c", "Read C:/repo/main.rs", "Read C:/repo/main.rs (2)"],
        );

        harness.ticks(3).await;

        assert_eq!(
            harness.emitted().len(),
            expected,
            "dedupe={dedupe:?} should have produced {expected}"
        );
    }
}

/// ...and the window is a DEDUPLICATION window, not a mute button: the same key comes back
/// once it has passed.
#[tokio::test]
async fn a_repeated_key_is_admitted_again_once_the_dedupe_window_has_passed() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher("done", "Done", WatcherDedupe::Row, 1)],
    );
    harness.backend.paint(id, &["a", "b", "c"]);
    harness.backend.paint(id, &["b", "c", "Done"]);
    harness.ticks(2).await;
    assert_eq!(harness.emitted().len(), 1);

    std::thread::sleep(Duration::from_millis(5));
    harness.backend.paint(id, &["c", "Done", "Done"]);
    harness.ticks(1).await;

    assert_eq!(harness.emitted().len(), 2);
}

/// 9.4.54 and 9.4.55 - **the caps, and where the overflow goes: nowhere.**
///
/// A pattern that matches every row, against a full-screen repaint that stabilizes. Three
/// watchers would produce 90 matches; the per-watcher cap holds each to 8 and the
/// per-session cap holds the tick to 16, and every match beyond them reaches neither the
/// event nor anything else.
#[tokio::test]
async fn the_per_tick_caps_bound_the_batch_and_mark_the_watchers_degraded() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![
            occurrence_watcher("w1", "row", WatcherDedupe::None, 2000),
            occurrence_watcher("w2", "row", WatcherDedupe::None, 2000),
            occurrence_watcher("w3", "row", WatcherDedupe::None, 2000),
        ],
    );

    let seed: Vec<String> = (0..30).map(|i| format!("seed {i}")).collect();
    let painted: Vec<String> = (0..30).map(|i| format!("row {i}")).collect();
    harness.backend.paint(id, &as_refs(&seed));
    harness.backend.paint(id, &as_refs(&painted));
    harness.backend.paint(id, &as_refs(&painted));

    harness.ticks(3).await;

    let emitted = harness.emitted();
    assert_eq!(
        emitted.len(),
        MATCHES_PER_SESSION_PER_TICK,
        "the session cap is the outer bound"
    );
    let w1 = emitted.iter().filter(|m| m.watcher_id == "w1").count();
    assert_eq!(w1, MATCHES_PER_WATCHER_PER_TICK, "the per-watcher cap");
    assert_eq!(
        emitted.iter().filter(|m| m.watcher_id == "w3").count(),
        0,
        "the session budget was already spent when w3 was reached"
    );

    for watcher in ["w1", "w2", "w3"] {
        let (degraded, _, _) = harness.engine.watcher_state(id, watcher).expect("state");
        assert!(degraded, "{watcher} must be marked degraded");
    }
}

/// **The regression that made a permission prompt vanish forever.**
///
/// Two saturated `occurrence` watchers ahead of a `state` one in `BTreeMap` order spend the
/// whole 16-per-session budget, so the `state` watcher is handed `budget == 0` and its match
/// is dropped. If the gate were committed while evaluating, the next tick would find the
/// candidate equal to the gate and return nothing: the activation would never reappear
/// while the text and the generation held - and silently, because `degraded` clears on the
/// tick after.
///
/// The plan says "emits nothing further for that key THIS TICK". This is the test that the
/// word "tick" is honoured.
#[tokio::test]
async fn a_state_match_dropped_by_the_session_cap_still_emits_on_the_next_tick() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![
            occurrence_watcher("a-reads", "row", WatcherDedupe::None, 2000),
            occurrence_watcher("b-tools", "row", WatcherDedupe::None, 2000),
            state_watcher("z-permission", "Permission required"),
        ],
    );

    // Seed, then a full repaint that settles nothing, then the tick where 29 rows become
    // evaluable AT THE SAME TIME as the prompt appears on the bottom row. That coincidence
    // is the whole scenario: the prompt has to arrive while the budget is already gone.
    let seed: Vec<String> = (0..30).map(|i| format!("seed {i}")).collect();
    let burst: Vec<String> = (0..30).map(|i| format!("row {i}")).collect();
    let mut prompted = burst.clone();
    prompted[29] = "Permission required".to_string();

    harness.backend.paint(id, &as_refs(&seed));
    harness.backend.paint(id, &as_refs(&burst));
    harness.backend.paint(id, &as_refs(&prompted));

    harness.ticks(3).await;

    let during = harness.emitted();
    assert_eq!(
        during.len(),
        MATCHES_PER_SESSION_PER_TICK,
        "the two occurrence watchers must have spent the whole session budget"
    );
    assert!(
        during.iter().all(|m| m.watcher_id != "z-permission"),
        "the prompt was dropped by the cap on this tick, which is what the plan allows"
    );
    let (degraded, _, _) = harness
        .engine
        .watcher_state(id, "z-permission")
        .expect("state");
    assert!(degraded, "and it must say so");

    // The screen holds still. The occurrence watchers take no budget - the only row that
    // stabilizes is the prompt, which their pattern does not match - and the prompt is
    // still on screen, so it MUST come through.
    harness.backend.paint(id, &as_refs(&prompted));
    harness.ticks(1).await;

    let after: Vec<_> = harness
        .emitted()
        .into_iter()
        .filter(|m| m.watcher_id == "z-permission")
        .collect();
    assert_eq!(
        after.len(),
        1,
        "the activation the cap dropped has to reappear, or it is lost forever"
    );
    assert_eq!(after[0].row, "Permission required");
}

/// **P1-2's regression: the day-one empty state has to be reachable.**
///
/// A registered agent session that no watcher reaches must answer `warmedUp: true` with
/// `activeWatchers: []`. That pair IS the window's "no watcher reaches this agent, here is
/// the Configure button" state, and it is what everybody sees on day one. Publishing
/// nothing left both this case and "the engine has not run yet" answering `warmedUp: false`
/// forever - byte-identical snapshots for the two cases the field exists to tell apart.
#[tokio::test]
async fn a_session_no_watcher_reaches_is_warmed_up_with_no_active_watchers() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.backend.paint(id, &["idle"]);

    // Before the first tick: the engine really has not run.
    assert!(!harness.history.snapshot(id, None).warmed_up);

    // Day one: nothing configured at all, so the tick takes the early exit.
    harness.ticks(1).await;

    let snapshot = harness.history.snapshot(id, None);
    assert!(snapshot.warmed_up);
    assert!(snapshot.active_watchers.is_empty());
    assert!(snapshot.matches.is_empty());
    assert_eq!(
        harness.backend.reads.load(Ordering::Relaxed),
        0,
        "and it must still not have touched the session: no read, no rows"
    );
    assert_eq!(
        harness.backends.liveness_calls.load(Ordering::Relaxed),
        0,
        "nor probed a child nobody is watching"
    );

    // Watchers configured, but for a DIFFERENT agent: same answer, same reason.
    harness.configure("other-agent", vec![state_watcher("w", "x")]);
    harness.ticks(1).await;

    let snapshot = harness.history.snapshot(id, None);
    assert!(snapshot.warmed_up);
    assert!(snapshot.active_watchers.is_empty());
    assert_eq!(harness.backend.reads.load(Ordering::Relaxed), 0);
}

/// 9.4.56 - the caps bound the RATE but not the duration, so a pair that stays saturated is
/// suspended, stays marked degraded while it is, and is retried afterwards.
#[tokio::test]
async fn a_pair_saturated_for_twenty_five_ticks_is_suspended_and_then_retried() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher(
            "flood",
            "row",
            WatcherDedupe::None,
            2000,
        )],
    );

    // A transcript scrolling ten rows a tick: every tick brings ten new matching rows, so
    // every tick is over the eight-per-watcher cap. A repaint would only degrade every
    // other tick and the counter would reset in between, which is the whole point of
    // requiring the ticks to be CONSECUTIVE.
    let transcript: Vec<String> = (0..1000).map(|i| format!("row {i}")).collect();
    for tick in 0..60usize {
        let start = tick * 10;
        harness
            .backend
            .paint(id, &as_refs(&transcript[start..start + 30]));
    }

    harness.ticks(40).await;

    let (degraded, suspended, _) = harness.engine.watcher_state(id, "flood").expect("state");
    assert!(suspended, "25 degraded ticks must suspend the pair");
    assert!(degraded, "suspension must not read as recovery");

    let before = harness.emitted().len();
    harness.ticks(1).await;
    assert_eq!(
        harness.emitted().len(),
        before,
        "a suspended pair emits nothing at all"
    );

    harness.engine.expire_suspension_for_test(id, "flood");
    harness.ticks(1).await;
    assert!(
        harness.emitted().len() > before,
        "and it is retried once the suspension passes"
    );
}

/// Acceptance criterion A5 - a transcript of more than a screenful delivered in ONE burst
/// reports its matches AND a non-zero `possiblyMissedFrames`, never a silent zero.
///
/// A jump larger than the screen has no alignment that explains it, so the honest answer
/// is "the sampler could not follow" plus whatever the next stable frame turns out to hold.
#[tokio::test]
async fn a_burst_larger_than_the_screen_reports_matches_and_admits_it_may_have_missed_some() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher("row", "row", WatcherDedupe::None, 2000)],
    );

    let seed: Vec<String> = (0..30).map(|i| format!("seed {i}")).collect();
    // 60 rows arrived between two samples: only the last 30 are still on the screen.
    let after: Vec<String> = (30..60).map(|i| format!("row {i}")).collect();
    harness.backend.paint(id, &as_refs(&seed));
    harness.backend.paint(id, &as_refs(&after));
    harness.backend.paint(id, &as_refs(&after));

    harness.ticks(3).await;

    assert!(!harness.emitted().is_empty(), "the burst must still report");
    let snapshot = harness.history.snapshot(id, None);
    assert!(
        snapshot.possibly_missed_frames > 0,
        "a jump larger than the screen must never report a silent zero"
    );
}

/// An occurrence watcher never fires on the tick a row is merely painted: the row has to
/// have arrived from below, or held still for a tick. That is layer 1 doing the work.
#[tokio::test]
async fn an_occurrence_watcher_fires_only_when_the_diff_declares_a_row_evaluable() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher("hit", "hit", WatcherDedupe::None, 2000)],
    );
    // The first tick seeds and evaluates nothing, even though the row is right there.
    harness.backend.paint(id, &["hit", "hit", "hit"]);
    harness.ticks(1).await;

    assert!(harness.emitted().is_empty());
}

/// 9.3.24 - **a session with no agent is never registered**, so a plain shell costs this
/// engine nothing, ever: no entry, no read, no allocation.
///
/// Asserted against the real registration site, which is one helper shared with the #1032
/// scraper, rather than against a re-implementation of its condition.
#[test]
fn a_session_with_no_agent_is_never_registered() {
    let harness = Harness::new();
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&harness.engine))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app");

    let shell = Uuid::new_v4();
    crate::commands::session::register_session_samplers(app.handle(), shell, None);
    assert!(!harness.engine.is_session_registered(shell));

    let agent = Uuid::new_v4();
    crate::commands::session::register_session_samplers(
        app.handle(),
        agent,
        Some("a1".to_string()),
    );
    assert!(harness.engine.is_session_registered(agent));
}

/// 9.5.62 and 9.5.66 - the engine publishes each session's STATUS at the end of every
/// tick, so the snapshot command can be synchronous, and the `seq` in the ring is the same
/// value the event carried. That shared identity is what lets the window merge a snapshot
/// with a live stream instead of guessing.
#[tokio::test]
async fn the_engine_publishes_status_every_tick_and_the_ring_shares_the_events_seq() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![
            state_watcher("perm", "Permission required"),
            occurrence_watcher("read", "Read", WatcherDedupe::None, 2000),
        ],
    );

    // One tick with no match at all: the status is still published.
    harness.backend.paint(id, &["idle"]);
    harness.ticks(1).await;

    let snapshot = harness.history.snapshot(id, None);
    assert!(snapshot.warmed_up);
    assert!(snapshot.matches.is_empty());
    let counters: Vec<(&str, u64)> = snapshot
        .active_watchers
        .iter()
        .map(|counter| (counter.watcher_id.as_str(), counter.count))
        .collect();
    assert_eq!(counters, vec![("perm", 0), ("read", 0)]);

    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(1).await;

    // The engine hands the batch to the sink; the ring is the SINK's job, so mirror what
    // the production sink does and assert the identity that makes the merge possible.
    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 1);
    harness.history.record(id, &emitted);
    let snapshot = harness.history.snapshot(id, None);
    assert_eq!(snapshot.matches[0].seq, emitted[0].seq);
    assert_eq!(snapshot.last_seq, emitted[0].seq);
    assert_eq!(
        snapshot
            .active_watchers
            .iter()
            .find(|counter| counter.watcher_id == "perm")
            .expect("counter")
            .count,
        1
    );
}

/// 9.5.72 - **a session whose child exits on its own KEEPS its buffer.** It is retired from
/// sampling and nothing else: an API error or a CLI crash is exactly when the evidence is
/// worth keeping, and it is the only thing left to look at.
#[tokio::test]
async fn a_session_that_dies_on_its_own_is_retired_but_keeps_its_history() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(1).await;
    harness.history.record(id, &harness.emitted());

    // Twice: a read that says `Gone` makes the engine re-resolve the backend once and ask
    // again, which is what covers a session that changed route.
    harness.backend.push(id, Painted::Gone);
    harness.backend.push(id, Painted::Gone);
    harness.ticks(1).await;

    assert!(!harness.engine.is_session_registered(id));
    let snapshot = harness.history.snapshot(id, None);
    assert_eq!(snapshot.matches.len(), 1, "the evidence must survive");
    assert!(snapshot.warmed_up);
}

/// 9.5.68 and 9.5.70 - `purge_session_side_state` retires FIRST and purges second, and the
/// entry cannot come back: only the engine creates entries, and only while holding its
/// registration lock.
#[tokio::test]
async fn purging_a_destroyed_session_retires_it_first_and_the_entry_never_returns() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(1).await;
    harness.history.record(id, &harness.emitted());
    assert!(harness.history.snapshot(id, None).warmed_up);

    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&harness.engine))
        .manage(Arc::clone(&harness.history))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app");
    crate::commands::session::purge_session_side_state(app.handle(), id);

    assert!(!harness.engine.is_session_registered(id));
    assert!(!harness.history.snapshot(id, None).warmed_up);

    // Whatever else ticks, the entry stays gone: the session is no longer registered, so
    // no tick can publish it back.
    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(5).await;
    assert!(!harness.history.snapshot(id, None).warmed_up);
}

/// 9.5.68 and 9.5.69 (call-site guard) - **the finding revision 1 of the plan missed.**
///
/// There are three production exits from `SessionManager`, not one, and the post-commit
/// cleanup is two parallel copies. A restart is a normal flow - the invoke handler, the
/// mailbox and a bulk settings save all reach it - so a purge wired into only the destroy
/// path would orphan up to 500 ring entries on every restart, under an id already gone
/// from the session list and therefore unreachable from the UI.
///
/// This pins BOTH call sites against the source, so removing either one fails here rather
/// than leaking silently.
#[test]
fn both_post_commit_cleanups_purge_the_per_session_side_state() {
    const SOURCE: &str = include_str!("../../../commands/session.rs");

    let restart = SOURCE
        .split("fn publish_restart_destroyed")
        .nth(1)
        .expect("publish_restart_destroyed must exist");
    // Delimited by the closing brace at column 0, which is where a top-level function ends
    // whatever comes after it. Splitting on the NEXT declaration would let an unrelated
    // function inserted in between extend this "body" downwards, and the guard would then
    // pass with the purge living somewhere else entirely.
    let restart_body = restart
        .split("\n}")
        .next()
        .expect("its body must end somewhere");
    assert!(
        restart_body.contains("purge_session_side_state("),
        "the restart cleanup must purge; both restart paths publish through it"
    );

    let destroy = SOURCE
        .split("for session_id in outcome.destroyed_ids.iter().copied()")
        .nth(1)
        .expect("the destroy transaction must purge over destroyed_ids ONLY");
    let loop_body = destroy
        .split("\n    }")
        .next()
        .expect("its body must end somewhere");
    assert!(
        loop_body.contains("purge_session_side_state("),
        "the destroyed-only loop must be the one that purges"
    );
    // 9.5.71 - and it must stay destroyed-ONLY. A root-agent session retained as `Exited`
    // stays in the manager and its row is still in the list, so its post-mortem view still
    // has a place to be shown and its buffer must survive.
    assert!(
        !loop_body.contains("retained_exited_ids"),
        "retained-exited sessions keep their buffer; the purge loop must not reach them"
    );
}

/// 9.6.74 and 9.6.75 - **the Rust half of the TypeScript mirror.** The exact camelCase
/// JSON, field for field, with EVERY field present: no `skip_serializing_if` anywhere, so
/// absent can never become a third state beside null and the value.
#[test]
fn the_payload_serializes_to_the_exact_camel_case_contract() {
    let batch = WatcherMatchBatch {
        session_id: "11111111-2222-3333-4444-555555555555".to_string(),
        matches: vec![WatcherMatchPayload {
            session_id: "11111111-2222-3333-4444-555555555555".to_string(),
            seq: 7,
            watcher_id: "permission-prompt".to_string(),
            mode: WatcherMode::Occurrence,
            at: chrono::DateTime::parse_from_rfc3339("2026-07-30T22:31:05Z")
                .expect("fixed instant")
                .with_timezone(&chrono::Utc),
            captures: vec![Some("C:/repo/main.rs".to_string()), None],
            row: "Read (C:/repo/main.rs)".to_string(),
            row_truncated: false,
        }],
    };

    assert_eq!(
        serde_json::to_value(&batch).expect("serializes"),
        serde_json::json!({
            "sessionId": "11111111-2222-3333-4444-555555555555",
            "matches": [{
                "sessionId": "11111111-2222-3333-4444-555555555555",
                "seq": 7,
                "watcherId": "permission-prompt",
                "mode": "occurrence",
                "at": "2026-07-30T22:31:05Z",
                "captures": ["C:/repo/main.rs", null],
                "row": "Read (C:/repo/main.rs)",
                "rowTruncated": false
            }]
        })
    );
}

/// The empty cases are present too, and are not elided: `captures: []` is a pattern with
/// no groups, and the UI falls back to the raw row on it.
#[test]
fn an_empty_capture_list_and_a_false_flag_are_both_written() {
    let payload = WatcherMatchPayload {
        session_id: "s".to_string(),
        seq: 1,
        watcher_id: "w".to_string(),
        mode: WatcherMode::State,
        at: chrono::DateTime::parse_from_rfc3339("2026-07-30T00:00:00Z")
            .expect("fixed instant")
            .with_timezone(&chrono::Utc),
        captures: Vec::new(),
        row: "Permission required".to_string(),
        row_truncated: false,
    };

    let json = serde_json::to_value(&payload).expect("serializes");
    let object = json.as_object().expect("object");
    assert_eq!(object.len(), 8, "every field, always: {json}");
    assert_eq!(object["captures"], serde_json::json!([]));
    assert_eq!(object["rowTruncated"], serde_json::json!(false));
    assert_eq!(object["mode"], serde_json::json!("state"));
}
