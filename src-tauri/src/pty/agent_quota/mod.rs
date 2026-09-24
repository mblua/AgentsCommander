//! #2482 - a per-session, best-effort reading of a coding agent's weekly (7-day) quota
//! usage, sampled from the same screen rows #1032's context badge reads.
//!
//! A narrowed copy of `context_scrape::ContextScraper`: no persistence, no alert
//! routing, a slower interval. The engine holds three narrow trait objects and no
//! `AppHandle`, `PtyManager` or settings type, so it can read, compile and emit and
//! nothing else. It names only `crate::shutdown` and `crate::pty::context_scrape`, which
//! keeps it out of the crate's cyclic SCC.

pub mod source;

use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use source::{ResolvedSource, SourceSpec};

/// 120 s. The requirement is "at least every 5 minutes"; 120 s keeps 2.5x
/// margin and is 1/24 of the work `context_scrape` already does at 5 s.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(120);

/// The rows and lightweight liveness of one session. Three-state, for the reason
/// `context_scrape` documents: "we could not ask" must never be confused with "the
/// session is over", or a live child unqueryable for one tick is deregistered for life.
pub trait QuotaRowsSource: Send + Sync {
    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead;
    fn get_session_liveness(&self, id: Uuid) -> ContextSessionLiveness;
}

/// Every agent's enabled source, keyed by agent id, resolved fresh each tick.
///
/// `BoxFuture` and not a sync fn: the settings live behind a `tokio::sync::RwLock`, whose
/// `blocking_read` panics inside a runtime - and the tick is inside one. A sync signature
/// here would kill the engine on tick 1, silently and permanently.
pub trait QuotaSourceProvider: Send + Sync {
    fn sources(&self) -> BoxFuture<'_, HashMap<String, SourceSpec>>;
}

/// Where a reading goes.
pub trait QuotaEventSink: Send + Sync {
    fn emit(&self, payload: AgentQuotaPayload);
}

/// The IPC contract, normative for phase 2 and phase 3.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQuotaPayload {
    pub session_id: String,
    /// USED percentage of the 7-day window, 0..=100, or None when unavailable.
    /// NEVER 0 and NEVER 100 for "unknown".
    ///
    /// Deliberately NO `skip_serializing_if`, for the reason
    /// `ContextUsagePayload::percent` states: `None` must serialize as an
    /// explicit `"weeklyUsedPercent": null`, not an absent key, or TS regains a
    /// third state beside `null` in a feature whose one hard rule is that
    /// unavailable is exactly one thing.
    pub weekly_used_percent: Option<u8>,
}

/// One registered session: which agent's source applies to it, and the last reading
/// emitted for it.
struct Registered {
    agent_id: String,
    /// Starts as `None` because `None` IS what the UI already shows, so an unconfigured
    /// session never emits: `None != None` is false.
    last_emitted: Option<u8>,
}

/// A resolved source, kept against the `spec_key` it came from. Keyed by agent rather
/// than by key text so the map is bounded by the number of configured agents.
enum Cached {
    Ok {
        key: String,
        source: Arc<ResolvedSource>,
    },
    /// The source does not resolve. Kept so the failure is sticky: neither resolved nor
    /// logged again until the user changes it.
    Failed { key: String },
}

impl Cached {
    fn key(&self) -> &str {
        match self {
            Cached::Ok { key, .. } | Cached::Failed { key } => key,
        }
    }

    fn source(&self) -> Option<Arc<ResolvedSource>> {
        match self {
            Cached::Ok { source, .. } => Some(Arc::clone(source)),
            Cached::Failed { .. } => None,
        }
    }
}

/// Samples every registered session on a timer.
pub struct AgentQuotaEngine {
    rows: Arc<dyn QuotaRowsSource>,
    sources: Arc<dyn QuotaSourceProvider>,
    sink: Arc<dyn QuotaEventSink>,
    /// Linearizes retirement against emission. Lock order: sequence, then registered.
    sequence: Mutex<()>,
    registered: Mutex<HashMap<Uuid, Registered>>,
    /// Keyed by AGENT id, bounded by agent count.
    resolved: Mutex<HashMap<String, Cached>>,
    /// Rotates the sorted order so partial saturation cannot starve a fixed tail.
    sample_cursor: AtomicUsize,
    /// The number of `source::resolve` calls. The resolve site is also the log site, so
    /// this is what "logged once per change, not once per tick" is measured by.
    resolves: AtomicUsize,
}

impl AgentQuotaEngine {
    pub fn new(
        rows: Arc<dyn QuotaRowsSource>,
        sources: Arc<dyn QuotaSourceProvider>,
        sink: Arc<dyn QuotaEventSink>,
    ) -> Arc<Self> {
        Arc::new(Self {
            rows,
            sources,
            sink,
            sequence: Mutex::new(()),
            registered: Mutex::new(HashMap::new()),
            resolved: Mutex::new(HashMap::new()),
            sample_cursor: AtomicUsize::new(0),
            resolves: AtomicUsize::new(0),
        })
    }

    /// Own thread, own runtime, shutdown token first: `ContextScraper::start`'s shape.
    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let engine = Arc::clone(self);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
                .expect("Failed to create tokio runtime for AgentQuotaEngine");
            rt.block_on(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.token().cancelled() => {
                            log::info!("[agent_quota] Shutdown signal received, stopping");
                            break;
                        }
                        _ = tokio::time::sleep(SAMPLE_INTERVAL) => {
                            engine.tick().await;
                        }
                    }
                }
            });
        });
    }

    /// Start sampling a session. A fresh entry always starts at `last_emitted: None`.
    pub fn register_session(&self, id: Uuid, agent_id: String) {
        let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
        let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        registered.insert(
            id,
            Registered {
                agent_id,
                last_emitted: None,
            },
        );
    }

    /// Stop sampling a session. Idempotent.
    pub fn retire_session(&self, id: Uuid) {
        let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
        self.retire_session_under_sequence(id);
    }

    /// Retire under an already-held sequence lock, emitting one final `None` only when
    /// the session had a live reading.
    fn retire_session_under_sequence(&self, id: Uuid) {
        let last_emitted = self
            .registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id)
            .and_then(|entry| entry.last_emitted);
        if last_emitted.is_some() {
            self.sink.emit(AgentQuotaPayload {
                session_id: id.to_string(),
                weekly_used_percent: None,
            });
        }
    }

    pub fn is_session_registered(&self, id: Uuid) -> bool {
        self.registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&id)
    }

    /// The last reading emitted for a session. `None` covers both "no reading" and "not
    /// registered", which are the same thing downstream.
    pub fn last_reading(&self, id: Uuid) -> Option<u8> {
        self.registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .and_then(|entry| entry.last_emitted)
    }

    /// Resolve an agent's source, recomputing only when its `spec_key` changed. The guard
    /// is a local and an owned `Arc` comes back, so the cache lock is never held across
    /// the rows read.
    fn resolve(&self, agent_id: &str, spec: &SourceSpec) -> Option<Arc<ResolvedSource>> {
        let key = source::spec_key(spec);
        let mut cache = self.resolved.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(cached) = cache.get(agent_id) {
            if cached.key() == key {
                return cached.source();
            }
        }

        self.resolves.fetch_add(1, Ordering::Relaxed);
        let cached = match source::resolve(spec) {
            Ok(resolved) => Cached::Ok {
                key,
                source: Arc::new(resolved),
            },
            Err(err) => {
                // Once per change, not once per tick: the cache makes it sticky.
                log::warn!("[agent_quota] agent {agent_id} has an unusable quota source: {err}");
                Cached::Failed { key }
            }
        };
        let resolved = cached.source();
        cache.insert(agent_id.to_string(), cached);
        resolved
    }

    pub(crate) async fn tick(&self) {
        // Before `sources()`, so an app with no agent session reads nothing at all.
        if self
            .registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            return;
        }

        let sources = self.sources.sources().await;

        let mut ids: Vec<(Uuid, String)> = {
            let registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
            registered
                .iter()
                .map(|(id, entry)| (*id, entry.agent_id.clone()))
                .collect()
        };
        ids.sort_unstable_by_key(|(id, _)| *id);
        if !ids.is_empty() {
            let start = self.sample_cursor.fetch_add(1, Ordering::Relaxed) % ids.len();
            ids.rotate_left(start);
        }

        for (id, agent_id) in ids {
            let usable = sources
                .get(&agent_id)
                .and_then(|spec| self.resolve(&agent_id, spec));

            let (reading, session_over) = match usable {
                // No entry, disabled, or unresolvable: liveness only, never the rows path.
                None => match self.rows.get_session_liveness(id) {
                    ContextSessionLiveness::Live | ContextSessionLiveness::Unavailable => {
                        (None, false)
                    }
                    ContextSessionLiveness::SessionOver => (None, true),
                },
                Some(resolved) => match self.rows.get_screen_rows(id) {
                    ScreenRowsRead::Rows(rows) => (source::sample(&resolved, &rows), false),
                    ScreenRowsRead::Unavailable => (None, false),
                    ScreenRowsRead::SessionOver => (None, true),
                },
            };

            let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
            if session_over {
                self.retire_session_under_sequence(id);
                continue;
            }

            let changed = {
                let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
                match registered.get_mut(&id) {
                    Some(entry) if entry.last_emitted != reading => {
                        entry.last_emitted = reading;
                        true
                    }
                    // Changed nothing, or retired mid-tick: no emit.
                    _ => false,
                }
            };
            if changed {
                self.sink.emit(AgentQuotaPayload {
                    session_id: id.to_string(),
                    weekly_used_percent: reading,
                });
            }
        }
    }

    #[cfg(test)]
    fn resolve_count(&self) -> usize {
        self.resolves.load(Ordering::Relaxed)
    }
}

/// #2482 - the engine's own fakes, shared with the call-site tests in
/// `commands::session` and `commands::pty` (phase 2). `#[cfg(test)]`, so nothing
/// here is compiled into a shipped binary; `pub(crate)`, because the two call
/// sites that need an engine live in other modules and this file is frozen once
/// phase 1 lands.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    type ReadHook = Box<dyn Fn(Uuid) + Send + Sync>;

    /// Rows are THREE-state and scripted, with an independent liveness script: a
    /// two-state fake cannot express "a live child we could not ask".
    #[derive(Default)]
    pub(crate) struct RowsFake {
        /// Per session, consumed front first. Empty = `Unavailable`.
        reads: Mutex<HashMap<Uuid, Vec<ScreenRowsRead>>>,
        /// Per session, sticky. Unset = `Unavailable`.
        liveness: Mutex<HashMap<Uuid, ContextSessionLiveness>>,
        rows_calls: Mutex<Vec<Uuid>>,
        liveness_calls: Mutex<Vec<Uuid>>,
        on_rows_read: Mutex<Option<ReadHook>>,
    }

    impl RowsFake {
        pub(crate) fn push(&self, id: Uuid, read: ScreenRowsRead) {
            self.reads.lock().unwrap().entry(id).or_default().push(read);
        }

        pub(crate) fn set_liveness(&self, id: Uuid, liveness: ContextSessionLiveness) {
            self.liveness.lock().unwrap().insert(id, liveness);
        }

        /// Every `get_screen_rows` call, in order.
        pub(crate) fn rows_calls(&self) -> Vec<Uuid> {
            self.rows_calls.lock().unwrap().clone()
        }

        /// Every `get_session_liveness` call, in order.
        pub(crate) fn liveness_calls(&self) -> Vec<Uuid> {
            self.liveness_calls.lock().unwrap().clone()
        }

        /// Runs inside `get_screen_rows`, after the read is taken: the mid-tick window.
        pub(crate) fn on_rows_read(&self, hook: impl Fn(Uuid) + Send + Sync + 'static) {
            *self.on_rows_read.lock().unwrap() = Some(Box::new(hook));
        }
    }

    impl QuotaRowsSource for RowsFake {
        fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
            self.rows_calls.lock().unwrap().push(id);
            let read = match self.reads.lock().unwrap().get_mut(&id) {
                Some(script) if !script.is_empty() => script.remove(0),
                _ => ScreenRowsRead::Unavailable,
            };
            if let Some(hook) = self.on_rows_read.lock().unwrap().as_ref() {
                hook(id);
            }
            read
        }

        fn get_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
            self.liveness_calls.lock().unwrap().push(id);
            self.liveness
                .lock()
                .unwrap()
                .get(&id)
                .copied()
                .unwrap_or(ContextSessionLiveness::Unavailable)
        }
    }

    #[derive(Default)]
    pub(crate) struct SourcesFake {
        specs: Mutex<HashMap<String, SourceSpec>>,
        calls: AtomicUsize,
    }

    impl SourcesFake {
        pub(crate) fn configure(&self, agent_id: &str, spec: SourceSpec) {
            self.specs
                .lock()
                .unwrap()
                .insert(agent_id.to_string(), spec);
        }

        /// What the adapter does for a disabled or removed entry: the engine sees none.
        pub(crate) fn disable(&self, agent_id: &str) {
            self.specs.lock().unwrap().remove(agent_id);
        }

        pub(crate) fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl QuotaSourceProvider for SourcesFake {
        fn sources(&self) -> BoxFuture<'_, HashMap<String, SourceSpec>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.specs.lock().unwrap().clone()
            })
        }
    }

    #[derive(Default)]
    pub(crate) struct SinkFake {
        emitted: Mutex<Vec<AgentQuotaPayload>>,
    }

    impl SinkFake {
        pub(crate) fn emitted(&self) -> Vec<AgentQuotaPayload> {
            self.emitted.lock().unwrap().clone()
        }
    }

    impl QuotaEventSink for SinkFake {
        fn emit(&self, payload: AgentQuotaPayload) {
            self.emitted.lock().unwrap().push(payload);
        }
    }

    /// Everything a test needs, built and wired, mirroring the `Harness` in
    /// `watchers/mod.rs`'s tests. The fakes stay reachable as FIELDS: a builder
    /// that returns only the engine cannot assert "rows were never read".
    pub(crate) struct QuotaHarness {
        pub(crate) engine: Arc<AgentQuotaEngine>,
        pub(crate) rows: Arc<RowsFake>,
        pub(crate) sources: Arc<SourcesFake>,
        pub(crate) sink: Arc<SinkFake>,
    }

    impl QuotaHarness {
        pub(crate) fn new() -> Self {
            let rows = Arc::new(RowsFake::default());
            let sources = Arc::new(SourcesFake::default());
            let sink = Arc::new(SinkFake::default());
            let engine = AgentQuotaEngine::new(
                Arc::clone(&rows) as Arc<dyn QuotaRowsSource>,
                Arc::clone(&sources) as Arc<dyn QuotaSourceProvider>,
                Arc::clone(&sink) as Arc<dyn QuotaEventSink>,
            );
            Self {
                engine,
                rows,
                sources,
                sink,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::QuotaHarness;
    use super::*;

    const AGENT: &str = "claude";
    const PATTERN: &str = r"Weekly (\d{1,3})% used";

    fn spec(pattern: &str) -> SourceSpec {
        SourceSpec::ScreenRegex {
            pattern: pattern.to_string(),
        }
    }

    fn rows(percent: u8) -> ScreenRowsRead {
        ScreenRowsRead::Rows(vec![format!("Weekly {percent}% used")])
    }

    fn payload(id: Uuid, percent: Option<u8>) -> AgentQuotaPayload {
        AgentQuotaPayload {
            session_id: id.to_string(),
            weekly_used_percent: percent,
        }
    }

    /// A harness with AGENT configured and one registered session.
    fn configured() -> (QuotaHarness, Uuid) {
        let h = QuotaHarness::new();
        h.sources.configure(AGENT, spec(PATTERN));
        let id = Uuid::new_v4();
        h.engine.register_session(id, AGENT.to_string());
        (h, id)
    }

    #[tokio::test]
    async fn an_empty_registration_set_never_calls_sources() {
        let h = QuotaHarness::new();
        h.sources.configure(AGENT, spec(PATTERN));

        h.engine.tick().await;

        assert_eq!(h.sources.calls(), 0);
        assert!(h.rows.rows_calls().is_empty());
    }

    #[tokio::test]
    async fn an_unconfigured_agent_never_reads_rows_and_never_emits() {
        let h = QuotaHarness::new();
        let id = Uuid::new_v4();
        h.engine.register_session(id, AGENT.to_string());
        h.rows.set_liveness(id, ContextSessionLiveness::Live);

        h.engine.tick().await;
        h.engine.tick().await;

        assert!(
            h.rows.rows_calls().is_empty(),
            "no source must mean no rows read"
        );
        assert_eq!(h.rows.liveness_calls(), vec![id, id]);
        assert!(h.sink.emitted().is_empty());
        assert_eq!(h.engine.last_reading(id), None);
    }

    #[tokio::test]
    async fn a_disabled_entry_never_reads_rows_and_never_emits() {
        let (h, id) = configured();
        h.sources.disable(AGENT);
        h.rows.push(id, rows(40));

        h.engine.tick().await;

        assert!(h.rows.rows_calls().is_empty());
        assert_eq!(h.rows.liveness_calls(), vec![id]);
        assert!(h.sink.emitted().is_empty());
    }

    #[tokio::test]
    async fn an_uncompilable_pattern_resolves_once_and_not_once_per_tick() {
        let h = QuotaHarness::new();
        h.sources.configure(AGENT, spec(r"(unclosed"));
        let id = Uuid::new_v4();
        h.engine.register_session(id, AGENT.to_string());

        for _ in 0..3 {
            h.engine.tick().await;
        }

        assert_eq!(h.engine.resolve_count(), 1);
        assert!(
            h.rows.rows_calls().is_empty(),
            "a failed source takes the liveness path"
        );
        assert_eq!(h.rows.liveness_calls().len(), 3);
        assert!(h.sink.emitted().is_empty());
    }

    #[tokio::test]
    async fn a_changed_pattern_resolves_again() {
        let (h, _id) = configured();
        h.engine.tick().await;
        h.engine.tick().await;
        assert_eq!(h.engine.resolve_count(), 1, "unchanged: no second resolve");

        h.sources.configure(AGENT, spec(r"Week (\d{1,3})%"));
        h.engine.tick().await;

        assert_eq!(h.engine.resolve_count(), 2);
    }

    #[tokio::test]
    async fn a_reading_emits_once_and_an_unchanged_reading_emits_nothing() {
        let (h, id) = configured();
        h.rows.push(id, rows(42));
        h.rows.push(id, rows(42));

        h.engine.tick().await;
        h.engine.tick().await;

        assert_eq!(h.sink.emitted(), vec![payload(id, Some(42))]);
        assert_eq!(h.engine.last_reading(id), Some(42));
    }

    #[tokio::test]
    async fn a_changed_reading_emits_the_new_value() {
        let (h, id) = configured();
        h.rows.push(id, rows(42));
        h.rows.push(id, rows(43));

        h.engine.tick().await;
        h.engine.tick().await;

        assert_eq!(
            h.sink.emitted(),
            vec![payload(id, Some(42)), payload(id, Some(43))]
        );
    }

    #[tokio::test]
    async fn a_reading_that_becomes_unavailable_emits_exactly_one_null() {
        let (h, id) = configured();
        h.rows.push(id, rows(42));
        h.rows.push(id, ScreenRowsRead::Unavailable);
        h.rows
            .push(id, ScreenRowsRead::Rows(vec!["no match".to_string()]));
        h.rows.push(id, ScreenRowsRead::Unavailable);

        for _ in 0..4 {
            h.engine.tick().await;
        }

        assert_eq!(
            h.sink.emitted(),
            vec![payload(id, Some(42)), payload(id, None)]
        );
        assert!(h.engine.is_session_registered(id));
    }

    #[tokio::test]
    async fn zero_percent_and_one_hundred_percent_are_real_readings_and_are_emitted() {
        let (h, id) = configured();
        h.rows.push(id, rows(0));
        h.rows.push(id, rows(100));
        h.rows.push(id, ScreenRowsRead::Unavailable);

        for _ in 0..3 {
            h.engine.tick().await;
        }

        assert_eq!(
            h.sink.emitted(),
            vec![
                payload(id, Some(0)),
                payload(id, Some(100)),
                payload(id, None)
            ]
        );
    }

    #[tokio::test]
    async fn session_over_retires_and_emits_one_final_null_only_when_a_reading_was_live() {
        // Live reading, then over: exactly one final null.
        let (h, id) = configured();
        h.rows.push(id, rows(42));
        h.rows.push(id, ScreenRowsRead::SessionOver);
        h.engine.tick().await;
        h.engine.tick().await;
        h.engine.tick().await;
        assert_eq!(
            h.sink.emitted(),
            vec![payload(id, Some(42)), payload(id, None)]
        );
        assert!(!h.engine.is_session_registered(id));

        // No reading ever, then over (liveness path): retired, nothing emitted.
        let h = QuotaHarness::new();
        let id = Uuid::new_v4();
        h.engine.register_session(id, AGENT.to_string());
        h.rows.set_liveness(id, ContextSessionLiveness::SessionOver);
        h.engine.tick().await;
        assert!(h.sink.emitted().is_empty());
        assert!(!h.engine.is_session_registered(id));
    }

    #[tokio::test]
    async fn rows_unavailable_keeps_the_registration_for_the_next_tick() {
        let (h, id) = configured();
        h.rows.push(id, ScreenRowsRead::Unavailable);
        h.rows.push(id, rows(55));

        h.engine.tick().await;
        assert!(h.engine.is_session_registered(id));
        h.engine.tick().await;

        assert_eq!(h.rows.rows_calls(), vec![id, id]);
        assert_eq!(h.sink.emitted(), vec![payload(id, Some(55))]);
    }

    #[tokio::test]
    async fn a_retired_session_reached_mid_tick_does_not_emit() {
        let (h, id) = configured();
        h.rows.push(id, rows(42));
        let engine = Arc::downgrade(&h.engine);
        h.rows.on_rows_read(move |id| {
            engine.upgrade().expect("engine alive").retire_session(id);
        });

        h.engine.tick().await;

        assert!(h.sink.emitted().is_empty());
        assert!(!h.engine.is_session_registered(id));
    }

    #[tokio::test]
    async fn the_sample_order_rotates_between_ticks() {
        let h = QuotaHarness::new();
        h.sources.configure(AGENT, spec(PATTERN));
        let mut ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
        for id in &ids {
            h.engine.register_session(*id, AGENT.to_string());
        }
        ids.sort();

        h.engine.tick().await;
        h.engine.tick().await;

        let calls = h.rows.rows_calls();
        assert_eq!(&calls[..3], &[ids[0], ids[1], ids[2]]);
        assert_eq!(&calls[3..], &[ids[1], ids[2], ids[0]]);
    }

    #[test]
    fn the_payload_serializes_unknown_as_an_explicit_null_key() {
        let value = serde_json::to_value(payload(Uuid::nil(), None)).unwrap();
        let object = value.as_object().expect("an object");
        assert!(
            object.contains_key("weeklyUsedPercent"),
            "the key is present"
        );
        assert_eq!(value["weeklyUsedPercent"], serde_json::Value::Null);

        let value = serde_json::to_value(payload(Uuid::nil(), Some(0))).unwrap();
        assert_eq!(value["weeklyUsedPercent"], serde_json::json!(0));
    }

    #[tokio::test]
    async fn the_test_support_harness_drives_a_reading_end_to_end() {
        let h = QuotaHarness::new();
        h.sources.configure(AGENT, spec(PATTERN));
        let id = Uuid::new_v4();
        h.engine.register_session(id, AGENT.to_string());
        h.rows.push(id, rows(73));

        h.engine.tick().await;

        assert_eq!(h.sink.emitted(), vec![payload(id, Some(73))]);
        assert_eq!(h.engine.last_reading(id), Some(73));
    }
}
