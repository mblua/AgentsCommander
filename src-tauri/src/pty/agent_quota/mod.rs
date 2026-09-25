//! #2482 - a per-session, best-effort reading of a coding agent's weekly (7-day) quota
//! usage, sampled from the same screen rows #1032's context badge reads.
//!
//! A narrowed copy of `context_scrape::ContextScraper`: no persistence, no alert
//! routing, a slower interval. The engine holds three narrow trait objects and no
//! `AppHandle`, `PtyManager` or settings type, so it can read, compile and emit and
//! nothing else. It names only `crate::shutdown` and `crate::pty::context_scrape`, which
//! keeps it out of the crate's cyclic SCC.
//!
//! #2566 - the reading belongs to an ACCOUNT, not to a session. The provider hands each
//! agent an OPAQUE `account_key` (its command's program token); a successful sample is
//! stored under that key and published to EVERY agent on it, so agents sharing a command
//! show one number whether or not they have a terminal open. `pty::agent_quota` gains NO
//! new import: the engine compares the key and never parses it. An import of
//! `config::settings` here would close `settings -> ... -> lib -> agent_quota -> settings`
//! and grow the crate's one 89-member cyclic SCC to 90.

pub mod source;

use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

use self::source::{ResolvedSource, SourceSpec};
use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};

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

/// #2566 - one agent's enabled source plus the ACCOUNT it belongs to. `account_key`
/// is OPAQUE here: the engine compares it and never parses it, which is what keeps
/// this module free of `config::` (see the module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentQuotaSource {
    pub account_key: String,
    pub spec: SourceSpec,
}

/// Every agent's source, keyed by agent id, resolved fresh each tick.
///
/// `BoxFuture` and not a sync fn: the settings live behind a `tokio::sync::RwLock`, whose
/// `blocking_read` panics inside a runtime - and the tick is inside one. A sync signature
/// here would kill the engine on tick 1, silently and permanently.
pub trait QuotaSourceProvider: Send + Sync {
    fn sources(&self) -> BoxFuture<'_, HashMap<String, AgentQuotaSource>>;
}

/// Where a reading goes.
pub trait QuotaEventSink: Send + Sync {
    fn emit(&self, payload: AgentQuotaPayload);
}

/// The IPC contract, normative for phase 2 and phase 3.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQuotaPayload {
    pub agent_id: String,
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

/// One registered session: which agent's source applies to it. #2566 - a session is no
/// longer a publication unit, so it carries no emitted value.
struct Registered {
    agent_id: String,
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
    /// Linearizes retirement against emission. Lock order: sequence, registered,
    /// account_readings, published. None is ever held across an `.await`.
    sequence: Mutex<()>,
    registered: Mutex<HashMap<Uuid, Registered>>,
    /// #2566 - the latest SUCCESSFUL sample per account key. An absent key IS "nothing
    /// read yet"; only a success ever writes here (D4), so there is no None to store.
    account_readings: Mutex<HashMap<String, u8>>,
    /// #2566 - the last value EMITTED per agent id. `Option` exists only for the removal
    /// branch of the reconcile step; absent and `Some(None)` compare equal there.
    published: Mutex<HashMap<String, Option<u8>>>,
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
            account_readings: Mutex::new(HashMap::new()),
            published: Mutex::new(HashMap::new()),
            resolved: Mutex::new(HashMap::new()),
            sample_cursor: AtomicUsize::new(0),
            resolves: AtomicUsize::new(0),
        })
    }

    /// Own thread, own runtime, shutdown token first: `ContextScraper::start`'s shape,
    /// except that the tick is raced against the token too (see `start_at_interval`).
    /// Returns the worker's handle so a test can prove it exited.
    pub fn start(
        self: &Arc<Self>,
        shutdown: crate::shutdown::ShutdownSignal,
    ) -> std::thread::JoinHandle<()> {
        self.start_at_interval(shutdown, SAMPLE_INTERVAL)
    }

    /// `start` with a short interval, for tests.
    #[cfg(test)]
    pub(crate) fn start_with_interval(
        self: &Arc<Self>,
        shutdown: crate::shutdown::ShutdownSignal,
        interval: Duration,
    ) -> std::thread::JoinHandle<()> {
        self.start_at_interval(shutdown, interval)
    }

    /// Two sequential selects per iteration, each `biased;` and token-first. Awaiting
    /// the tick inside the sleep branch (as `ContextScraper::start` does) stops polling
    /// the token, so a provider future that never resolves would keep the thread alive
    /// past shutdown and could still emit. Here an in-flight tick is DROPPED: no `std`
    /// guard is held across an `.await`, so at most one sample is lost.
    fn start_at_interval(
        self: &Arc<Self>,
        shutdown: crate::shutdown::ShutdownSignal,
        interval: Duration,
    ) -> std::thread::JoinHandle<()> {
        let engine = Arc::clone(self);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
                .expect("Failed to create tokio runtime for AgentQuotaEngine");
            rt.block_on(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.token().cancelled() => break,
                        _ = tokio::time::sleep(interval) => {}
                    }
                    tokio::select! {
                        biased;
                        _ = shutdown.token().cancelled() => break,
                        _ = engine.tick() => {}
                    }
                }
                log::info!("[agent_quota] Shutdown signal received, stopping");
            });
        })
    }

    /// Start sampling a session.
    pub fn register_session(&self, id: Uuid, agent_id: String) {
        let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
        let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        registered.insert(id, Registered { agent_id });
    }

    /// Stop sampling a session. Idempotent. Emits nothing: the account value outlives
    /// the terminal (#2566 D4).
    pub fn retire_session(&self, id: Uuid) {
        let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
        self.registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }

    pub fn is_session_registered(&self, id: Uuid) -> bool {
        self.registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&id)
    }

    /// #2566 - every value last emitted, by agent id. A `None` value and an absent key both
    /// mean unavailable.
    pub fn published(&self) -> HashMap<String, Option<u8>> {
        self.published
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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

    /// Drop every cached source whose agent is not configured, so the cache is bounded by
    /// the configured agent set rather than by every agent ever seen. #2566 - keyed by
    /// configured agents, not registered sessions: closing a terminal must not discard a
    /// pattern other sessions and the published value still need.
    fn prune_resolved(&self, configured: &HashMap<String, AgentQuotaSource>) {
        self.resolved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|agent_id, _| configured.contains_key(agent_id));
    }

    pub(crate) async fn tick(&self) {
        let mut ids: Vec<(Uuid, String)> = {
            let registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
            registered
                .iter()
                .map(|(id, entry)| (*id, entry.agent_id.clone()))
                .collect()
        };

        // Before `sources()`, so the fully idle app reads nothing at all. With something
        // published it continues: an agent deleted while no terminal is open must still
        // lose its chip.
        if ids.is_empty()
            && self
                .published
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        {
            self.resolved
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            return;
        }

        let sources = self.sources.sources().await;
        self.prune_resolved(&sources);

        ids.sort_unstable_by_key(|(id, _)| *id);
        if !ids.is_empty() {
            let start = self.sample_cursor.fetch_add(1, Ordering::Relaxed) % ids.len();
            ids.rotate_left(start);
        }

        for (id, agent_id) in ids {
            let usable = sources.get(&agent_id).and_then(|src| {
                self.resolve(&agent_id, &src.spec)
                    .map(|resolved| (src, resolved))
            });

            let session_over = match usable {
                // No source behind this command, or unresolvable: liveness only, never the
                // rows path.
                None => matches!(
                    self.rows.get_session_liveness(id),
                    ContextSessionLiveness::SessionOver
                ),
                Some((src, resolved)) => match self.rows.get_screen_rows(id) {
                    ScreenRowsRead::Rows(rows) => {
                        // Only a SUCCESSFUL sample writes the account reading (D4). A
                        // session retired mid-tick still contributes: the sample really
                        // was taken from that account.
                        if let Some(value) = source::sample(&resolved, &rows) {
                            let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
                            self.account_readings
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(src.account_key.clone(), value);
                        }
                        false
                    }
                    ScreenRowsRead::Unavailable => false,
                    ScreenRowsRead::SessionOver => true,
                },
            };
            if session_over {
                self.retire_session(id);
            }
        }

        let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
        let mut account_readings = self
            .account_readings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut published = self.published.lock().unwrap_or_else(|e| e.into_inner());

        let mut configured: Vec<(&String, &AgentQuotaSource)> = sources.iter().collect();
        configured.sort_unstable_by(|a, b| a.0.cmp(b.0));
        for (agent_id, src) in configured {
            let desired = account_readings.get(&src.account_key).copied();
            // `.flatten()`: "never spoken about" and "told it is unknown" are the same plain
            // chip, so a configured agent with no reading never emits a first null.
            if published.get(agent_id).copied().flatten() != desired {
                self.sink.emit(AgentQuotaPayload {
                    agent_id: agent_id.clone(),
                    weekly_used_percent: desired,
                });
                published.insert(agent_id.clone(), desired);
            }
        }

        // The ONLY writer of a null: an agent that left the source map loses its chip once.
        let mut gone: Vec<String> = published
            .keys()
            .filter(|agent_id| !sources.contains_key(*agent_id))
            .cloned()
            .collect();
        gone.sort_unstable();
        for agent_id in gone {
            if let Some(Some(_)) = published.remove(&agent_id) {
                self.sink.emit(AgentQuotaPayload {
                    agent_id,
                    weekly_used_percent: None,
                });
            }
        }

        // Bounded by the configured agents: the last agent leaving a command drops its reading.
        account_readings.retain(|key, _| sources.values().any(|src| &src.account_key == key));
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
        specs: Mutex<HashMap<String, AgentQuotaSource>>,
        calls: AtomicUsize,
        hang: std::sync::atomic::AtomicBool,
    }

    impl SourcesFake {
        /// Account key == agent id: one agent, one account.
        pub(crate) fn configure(&self, agent_id: &str, spec: SourceSpec) {
            self.configure_on(agent_id, agent_id, spec);
        }

        /// #2566 - an agent on a named account key, for the shared-command cases.
        pub(crate) fn configure_on(&self, agent_id: &str, account_key: &str, spec: SourceSpec) {
            self.specs.lock().unwrap().insert(
                agent_id.to_string(),
                AgentQuotaSource {
                    account_key: account_key.to_string(),
                    spec,
                },
            );
        }

        /// What the adapter does for a disabled or removed entry: the engine sees none.
        pub(crate) fn disable(&self, agent_id: &str) {
            self.specs.lock().unwrap().remove(agent_id);
        }

        pub(crate) fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        /// From now on `sources()` counts the call and never resolves: a tick stuck
        /// in flight.
        pub(crate) fn hang(&self) {
            self.hang.store(true, Ordering::SeqCst);
        }
    }

    impl QuotaSourceProvider for SourcesFake {
        fn sources(&self) -> BoxFuture<'_, HashMap<String, AgentQuotaSource>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if self.hang.load(Ordering::SeqCst) {
                    std::future::pending::<()>().await;
                }
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

    fn payload(agent_id: &str, percent: Option<u8>) -> AgentQuotaPayload {
        AgentQuotaPayload {
            agent_id: agent_id.to_string(),
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
    async fn an_empty_registration_set_with_nothing_published_never_calls_sources() {
        let h = QuotaHarness::new();
        h.sources.configure(AGENT, spec(PATTERN));

        h.engine.tick().await;

        assert_eq!(h.sources.calls(), 0);
        assert!(h.rows.rows_calls().is_empty());
    }

    #[tokio::test]
    async fn an_empty_registration_set_still_reconciles_what_is_published() {
        let (h, id) = configured();
        h.rows.push(id, rows(42));
        h.engine.tick().await;
        h.engine.retire_session(id);
        h.sources.disable(AGENT);
        let calls = h.sources.calls();

        h.engine.tick().await;

        assert_eq!(
            h.sources.calls(),
            calls + 1,
            "published state needs sources()"
        );
        assert_eq!(
            h.sink.emitted(),
            vec![payload(AGENT, Some(42)), payload(AGENT, None)]
        );
        assert!(h.engine.published().is_empty());
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
        assert!(h.engine.published().is_empty());
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

        assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
        assert_eq!(h.engine.published().get(AGENT), Some(&Some(42)));
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
            vec![payload(AGENT, Some(42)), payload(AGENT, Some(43))]
        );
    }

    #[tokio::test]
    async fn a_reading_that_becomes_unavailable_keeps_the_last_account_value() {
        let (h, id) = configured();
        h.rows.push(id, rows(42));
        h.rows.push(id, ScreenRowsRead::Unavailable);
        h.rows
            .push(id, ScreenRowsRead::Rows(vec!["no match".to_string()]));
        h.rows.push(id, ScreenRowsRead::Unavailable);

        for _ in 0..4 {
            h.engine.tick().await;
        }

        assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
        assert_eq!(h.engine.published().get(AGENT), Some(&Some(42)));
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
            vec![payload(AGENT, Some(0)), payload(AGENT, Some(100))]
        );
    }

    #[tokio::test]
    async fn session_over_retires_the_session_and_keeps_the_account_value() {
        // Live reading, then over: retired, and the account value stays published.
        let (h, id) = configured();
        h.rows.push(id, rows(42));
        h.rows.push(id, ScreenRowsRead::SessionOver);
        h.engine.tick().await;
        h.engine.tick().await;
        h.engine.tick().await;
        assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
        assert!(!h.engine.is_session_registered(id));
        assert_eq!(h.engine.published().get(AGENT), Some(&Some(42)));

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
        assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(55))]);
    }

    #[tokio::test]
    async fn a_session_retired_mid_tick_still_contributes_its_sample() {
        let (h, id) = configured();
        h.rows.push(id, rows(42));
        let engine = Arc::downgrade(&h.engine);
        h.rows.on_rows_read(move |id| {
            engine.upgrade().expect("engine alive").retire_session(id);
        });

        h.engine.tick().await;

        assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
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
        let value = serde_json::to_value(payload("a1", None)).unwrap();
        let object = value.as_object().expect("an object");
        assert!(
            object.contains_key("weeklyUsedPercent"),
            "the key is present"
        );
        assert_eq!(value["weeklyUsedPercent"], serde_json::Value::Null);

        let value = serde_json::to_value(payload("a1", Some(0))).unwrap();
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

        assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(73))]);
        assert_eq!(h.engine.published().get(AGENT), Some(&Some(73)));
    }

    #[test]
    fn a_pending_provider_does_not_outlive_shutdown() {
        let h = QuotaHarness::new();
        h.sources.configure(AGENT, spec(PATTERN));
        h.sources.hang();
        h.engine.register_session(Uuid::new_v4(), AGENT.to_string());
        let shutdown = crate::shutdown::ShutdownSignal::new();

        let handle = h
            .engine
            .start_with_interval(shutdown.clone(), Duration::from_millis(5));

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while h.sources.calls() < 1 {
            assert!(
                std::time::Instant::now() < deadline,
                "no tick reached sources()"
            );
            std::thread::sleep(Duration::from_millis(1));
        }

        shutdown.trigger();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !handle.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }

        assert!(
            handle.is_finished(),
            "the worker outlived shutdown with a tick in flight"
        );
        assert!(h.sink.emitted().is_empty());
    }

    #[tokio::test]
    async fn the_resolution_cache_evicts_agents_that_are_no_longer_configured() {
        let h = QuotaHarness::new();
        h.sources.configure("a", spec(PATTERN));
        h.sources.configure("b", spec(r"Week (\d{1,3})%"));
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        h.engine.register_session(a, "a".to_string());
        h.engine.register_session(b, "b".to_string());

        h.engine.tick().await;
        assert_eq!(h.engine.resolve_count(), 2);

        h.engine.retire_session(b);
        h.engine.tick().await;
        assert_eq!(
            h.engine.resolve_count(),
            2,
            "B is still configured: closing its terminal keeps it cached"
        );

        h.sources.disable("b");
        h.engine.tick().await;
        h.sources.configure("b", spec(r"Week (\d{1,3})%"));
        h.engine.register_session(b, "b".to_string());
        h.engine.tick().await;
        assert_eq!(h.engine.resolve_count(), 3, "B was evicted, not reused");

        h.engine.retire_session(a);
        h.engine.retire_session(b);
        let calls = h.sources.calls();
        h.engine.tick().await;
        assert_eq!(h.sources.calls(), calls, "the idle path skips sources()");

        h.engine.register_session(a, "a".to_string());
        h.engine.tick().await;
        assert_eq!(h.engine.resolve_count(), 4, "the idle tick cleared the map");
    }

    #[tokio::test]
    async fn two_agents_on_one_command_are_both_filled_from_one_session() {
        let h = QuotaHarness::new();
        h.sources.configure_on("a1", "claude", spec(PATTERN));
        h.sources.configure_on("a2", "claude", spec(PATTERN));
        let id = Uuid::new_v4();
        h.engine.register_session(id, "a1".to_string());
        h.rows.push(id, rows(42));

        h.engine.tick().await;

        assert_eq!(
            h.sink.emitted(),
            vec![payload("a1", Some(42)), payload("a2", Some(42))]
        );
    }

    #[tokio::test]
    async fn two_agents_on_different_commands_keep_separate_readings() {
        let h = QuotaHarness::new();
        h.sources.configure_on("a1", "claude", spec(PATTERN));
        h.sources.configure_on("a2", "/a/claude", spec(PATTERN));
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        h.engine.register_session(s1, "a1".to_string());
        h.engine.register_session(s2, "a2".to_string());
        h.rows.push(s1, rows(30));
        h.rows.push(s2, rows(70));

        h.engine.tick().await;
        h.engine.tick().await;

        assert_eq!(
            h.sink.emitted(),
            vec![payload("a1", Some(30)), payload("a2", Some(70))]
        );
        let published = h.engine.published();
        assert_eq!(published.get("a1"), Some(&Some(30)));
        assert_eq!(published.get("a2"), Some(&Some(70)));
    }

    #[tokio::test]
    async fn an_agent_with_no_session_is_filled_and_stays_filled_after_the_last_session_retires() {
        let (h, id) = configured();
        h.rows.push(id, rows(42));
        h.engine.tick().await;

        h.engine.retire_session(id);
        h.engine.tick().await;

        assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
        assert_eq!(h.engine.published().get(AGENT), Some(&Some(42)));
    }

    #[tokio::test]
    async fn an_agent_that_leaves_the_source_map_is_published_as_null_once() {
        let (h, id) = configured();
        h.rows.push(id, rows(42));
        h.engine.tick().await;

        h.sources.disable(AGENT);
        h.engine.tick().await;
        h.engine.tick().await;

        assert_eq!(
            h.sink.emitted(),
            vec![payload(AGENT, Some(42)), payload(AGENT, None)]
        );
        assert!(h.engine.published().is_empty());
    }

    #[tokio::test]
    async fn nothing_is_emitted_twice_for_an_unchanged_account_value() {
        let (h, id) = configured();
        for _ in 0..3 {
            h.rows.push(id, rows(42));
        }

        for _ in 0..3 {
            h.engine.tick().await;
        }

        assert_eq!(h.sink.emitted(), vec![payload(AGENT, Some(42))]);
    }

    #[tokio::test]
    async fn emission_order_is_sorted_by_agent_id() {
        let h = QuotaHarness::new();
        h.sources.configure_on("b", "claude", spec(PATTERN));
        h.sources.configure_on("a", "claude", spec(PATTERN));
        let id = Uuid::new_v4();
        h.engine.register_session(id, "b".to_string());
        h.rows.push(id, rows(42));

        h.engine.tick().await;

        assert_eq!(
            h.sink.emitted(),
            vec![payload("a", Some(42)), payload("b", Some(42))]
        );
    }

    #[tokio::test]
    async fn a_configured_agent_with_no_reading_emits_nothing_on_the_first_tick() {
        let (h, id) = configured();
        h.rows
            .push(id, ScreenRowsRead::Rows(vec!["no match".to_string()]));

        h.engine.tick().await;

        assert!(h.sink.emitted().is_empty());
        assert!(h.engine.published().is_empty());
    }

    #[tokio::test]
    async fn a_disabled_agent_is_still_filled_from_its_enabled_sibling_on_the_same_command() {
        // What `lib.rs` hands over when `a1` is disabled and `a2` is enabled on one command.
        let h = QuotaHarness::new();
        h.sources.configure_on("a1", "claude", spec(PATTERN));
        h.sources.configure_on("a2", "claude", spec(PATTERN));
        let id = Uuid::new_v4();
        h.engine.register_session(id, "a1".to_string());
        h.rows.push(id, rows(42));

        h.engine.tick().await;

        assert_eq!(
            h.sink.emitted(),
            vec![payload("a1", Some(42)), payload("a2", Some(42))]
        );
    }
}
