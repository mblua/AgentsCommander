//! #1032 - a per-session, best-effort reading of a coding agent's context-window usage,
//! taken by running a per-agent, user-configured regex over the plain de-ANSI'd rows of the
//! `vt100` screen mirror AC already keeps for every session.
//!
//! **The percentage is a signal for a human.** A sample may be offered to the bounded
//! context-alert monitor so it can enqueue an informational coordinator notice, but this
//! scraper cannot route, inject, or remediate the sampled session. That boundary is
//! structural: it holds four narrow trait objects and no `AppHandle` or `PtyManager`.

pub mod pattern;
pub mod rows;

use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

use pattern::ContextPattern;

/// Matches `GitWatcher`'s poll interval. A full sample is ~200 us at AC's default 30x120,
/// so fifty sessions at 5s is ~0.2% of one core, and the liveness gate adds ~0.9 us per
/// configured session per tick - against the ~1.9-2.4 us a single keystroke already holds
/// the same `ptys` lock for.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// Lightweight liveness used when no usable regex exists. It deliberately does not expose
/// or clone the terminal parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSessionLiveness {
    Live,
    Unavailable,
    SessionOver,
}

/// What a backend can say about one session's rows.
///
/// THREE states, because the oracle behind it has three. A two-state channel here would
/// make `None` mean four different things - session unknown, parser poisoned, child dead,
/// child unqueryable - of which "stop sampling" is right for three and destroys the fourth:
/// a live child whose handle could not be queried for one tick would be deregistered for
/// the rest of its life. `ChildLiveness::Unqueryable` exists precisely so "we could not
/// ask" is never confused with a definite answer, and this enum is what carries that the
/// rest of the way.
pub enum ScreenRowsRead {
    /// The live grid's rows.
    Rows(Vec<String>),
    /// No reading this tick. Says NOTHING about whether the session is over: retry next
    /// tick, keep the entry. (Child alive but unqueryable; parser missing or poisoned.)
    Unavailable,
    /// The session is over. Emit null once, then stop sampling it.
    SessionOver,
}

/// The rows and lightweight liveness of one session.
pub trait ScreenRowsSource: Send + Sync {
    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead;

    fn get_session_liveness(&self, _id: Uuid) -> ContextSessionLiveness {
        ContextSessionLiveness::Unavailable
    }
}

/// Every agent's configured pattern STRING, resolved fresh each tick.
///
/// `BoxFuture` and not a sync fn: the settings live behind a `tokio::sync::RwLock`, whose
/// `blocking_read` panics inside a runtime - and the tick is inside one. A sync signature
/// here would kill the scraper on tick 1, silently and permanently, for every session.
pub trait ContextPatternSource: Send + Sync {
    fn patterns(&self) -> BoxFuture<'_, HashMap<String, String>>;
}

/// Where a badge reading goes. Its equality gate remains independent of internal samples.
pub trait ContextEventSink: Send + Sync {
    fn emit(&self, payload: ContextUsagePayload);
}

/// Every scrape outcome offered to the bounded internal monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextSample {
    Reading { session_id: Uuid, percent: u8 },
    Unavailable { session_id: Uuid },
    SessionOver { session_id: Uuid },
}

pub trait ContextSampleSink: Send + Sync {
    fn observe(&self, sample: ContextSample);
}

/// #1088 - where a tick's changed readings go to be written onto their sessions
/// and persisted to `sessions.json` so the disk-reading CLI can surface them.
///
/// Like the other sinks, the scraper holds ONLY this narrow trait: the concrete
/// impl (`lib.rs`) owns the `SessionManager` handle, so the scraper never gains
/// the ability to route, inject, wake, or destroy - preserving the module's
/// stated capability boundary. `BoxFuture` (like `ContextPatternSource`) because
/// `commit` must `await` the manager lock and the sessions save path.
pub trait ContextPersistSink: Send + Sync {
    fn commit(&self, changed: Vec<(Uuid, Option<u8>)>) -> BoxFuture<'_, ()>;
}

/// The IPC contract, normative for #1033.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsagePayload {
    pub session_id: String,
    /// 0..=100, or None when unavailable. NEVER 0 for "unknown".
    ///
    /// Deliberately NO `skip_serializing_if`: `None` MUST serialize as an explicit
    /// `"percent": null` rather than an absent key. With the skip, TS would have to type
    /// this `percent?: number`, which re-introduces *absent* as a third state beside `null`
    /// and `0` - in a feature whose one hard rule is that unavailable is exactly one thing.
    pub percent: Option<u8>,
}

/// One registered session: which agent's pattern applies to it, and the last thing the
/// badge was told.
struct Registered {
    agent_id: String,
    /// Starts as `None` because `None` IS what the badge already shows, so an unconfigured
    /// session never emits: `None != None` is false. The "no event when unconfigured"
    /// requirement holds by construction rather than by a special case.
    last_emitted: Option<u8>,
}

/// A compiled pattern, kept against the source string it came from.
///
/// Keyed by agent rather than by pattern text so the map is bounded by the number of
/// configured agents: a user typing a regex passes through a new string on every keystroke,
/// and none of those may accumulate. The source string is what decides a recompile, which
/// is the property the plan asks for.
enum Cached {
    Ok(Arc<ContextPattern>),
    /// The pattern does not compile. Kept so the failure is sticky: we neither recompile it
    /// nor log it again until the user changes it.
    Failed {
        source: String,
    },
}

impl Cached {
    fn source(&self) -> &str {
        match self {
            Cached::Ok(pattern) => pattern.source(),
            Cached::Failed { source } => source,
        }
    }

    fn pattern(&self) -> Option<Arc<ContextPattern>> {
        match self {
            Cached::Ok(pattern) => Some(Arc::clone(pattern)),
            Cached::Failed { .. } => None,
        }
    }
}

/// Samples every registered session on a timer.
///
/// No trait is downcastable to its implementation. The total capability of this type is:
/// read rows or lightweight liveness, read patterns, emit a badge payload, and synchronously
/// offer one small sample. It cannot route, inject, wake, or destroy anything.
pub struct ContextScraper {
    rows: Arc<dyn ScreenRowsSource>,
    patterns: Arc<dyn ContextPatternSource>,
    sink: Arc<dyn ContextEventSink>,
    samples: Arc<dyn ContextSampleSink>,
    /// #1088 - the fifth narrow sink: commits a tick's changed readings onto
    /// their sessions and triggers the whole-file persist. The concrete impl
    /// (`lib.rs`) owns the `SessionManager` handle; the scraper holds only this.
    persist: Arc<dyn ContextPersistSink>,
    /// Linearizes registration retirement against sample and badge publication.
    /// Lock order is always sequence, then registered.
    sequence: Mutex<()>,
    registered: Mutex<HashMap<Uuid, Registered>>,
    compiled: Mutex<HashMap<String, Cached>>,
    /// Rotates the sorted producer order so partial saturation cannot starve a fixed tail.
    sample_cursor: AtomicUsize,
    /// Test-visible: the number of `pattern::compile` calls. The compile site is also the
    /// log site, so this is what "logged once per change, not once per tick" is measured by.
    compiles: AtomicUsize,
}

impl ContextScraper {
    pub fn new(
        rows: Arc<dyn ScreenRowsSource>,
        patterns: Arc<dyn ContextPatternSource>,
        sink: Arc<dyn ContextEventSink>,
        samples: Arc<dyn ContextSampleSink>,
        persist: Arc<dyn ContextPersistSink>,
    ) -> Arc<Self> {
        Arc::new(Self {
            rows,
            patterns,
            sink,
            samples,
            persist,
            sequence: Mutex::new(()),
            registered: Mutex::new(HashMap::new()),
            compiled: Mutex::new(HashMap::new()),
            sample_cursor: AtomicUsize::new(0),
            compiles: AtomicUsize::new(0),
        })
    }

    /// Own thread, own runtime, shutdown token: `GitWatcher`'s shape.
    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let scraper = Arc::clone(self);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
                .expect("Failed to create tokio runtime for ContextScraper");
            rt.block_on(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.token().cancelled() => {
                            log::info!("[context] Shutdown signal received, stopping");
                            break;
                        }
                        _ = tokio::time::sleep(SAMPLE_INTERVAL) => {
                            scraper.tick().await;
                        }
                    }
                }
            });
        });
    }

    /// Start sampling a session. Called once per agent session at the spawn chokepoint;
    /// sessions with no agent are never registered, so a plain shell costs nothing.
    ///
    /// A fresh entry always starts at `last_emitted: None`. It can never meet an existing
    /// entry: session ids are minted per spawn and AC never reuses one, not even on respawn.
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

    /// Stop sampling a session. Producer-side end pruning and monitor maintenance both use
    /// this idempotent operation.
    pub(crate) fn retire_session(&self, id: Uuid) {
        let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
        self.retire_session_under_sequence(id);
    }

    /// Retire under an already-held sequence lock. Returns whether a final `None`
    /// badge was emitted (true only when the session had a live last reading), so the
    /// tick loop can persist that same clear through the #1088 sink: the gate that
    /// drives the badge drives the persist.
    fn retire_session_under_sequence(&self, id: Uuid) -> bool {
        let last_emitted = self
            .registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id)
            .and_then(|entry| entry.last_emitted);
        if last_emitted.is_some() {
            self.sink.emit(ContextUsagePayload {
                session_id: id.to_string(),
                percent: None,
            });
            true
        } else {
            false
        }
    }

    pub(crate) fn is_session_registered(&self, id: Uuid) -> bool {
        self.registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&id)
    }

    /// The last reading emitted for a session, for the snapshot command. `None` covers both
    /// "no reading" and "not registered", which are the same thing to the badge.
    pub fn last_reading(&self, id: Uuid) -> Option<u8> {
        self.registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .and_then(|entry| entry.last_emitted)
    }

    /// Resolve an agent's pattern, compiling only when its source string changed.
    ///
    /// The guard is a local and an owned `Arc` comes back, so the cache lock is never held
    /// across the rows read below it.
    fn resolve(&self, agent_id: &str, source: &str) -> Option<Arc<ContextPattern>> {
        let mut cache = self.compiled.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(cached) = cache.get(agent_id) {
            if cached.source() == source {
                return cached.pattern();
            }
        }

        self.compiles.fetch_add(1, Ordering::Relaxed);
        let cached = match pattern::compile(source) {
            Ok(pattern) => Cached::Ok(Arc::new(pattern)),
            Err(err) => {
                // Once per change, not once per tick: the cache below makes it sticky.
                log::warn!("[context] agent {agent_id} has an unusable context regex: {err}");
                Cached::Failed {
                    source: source.to_string(),
                }
            }
        };
        let pattern = cached.pattern();
        cache.insert(agent_id.to_string(), cached);
        pattern
    }

    pub(crate) async fn tick(&self) {
        // Before `patterns()`, so an app with no agent session running reads nothing at all.
        if self
            .registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            return;
        }

        let patterns = self.patterns.patterns().await;

        // Snapshot first: the loop must not mutate `registered` while iterating it. Sorting
        // makes the rotation deterministic and rotating prevents a fixed saturation tail.
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

        // #1088 - accumulate this tick's changed readings, handed to the persist
        // sink exactly once after the loop (coalesced: <=1 whole-file write per
        // tick regardless of how many sessions changed).
        let mut changed_readings: Vec<(Uuid, Option<u8>)> = Vec::new();

        for (id, agent_id) in ids {
            let usable_pattern = patterns
                .get(&agent_id)
                .filter(|source| !source.trim().is_empty())
                .and_then(|source| self.resolve(&agent_id, source));

            let (reading, sample, session_over) = match usable_pattern {
                None => match self.rows.get_session_liveness(id) {
                    ContextSessionLiveness::Live | ContextSessionLiveness::Unavailable => {
                        (None, ContextSample::Unavailable { session_id: id }, false)
                    }
                    ContextSessionLiveness::SessionOver => {
                        (None, ContextSample::SessionOver { session_id: id }, true)
                    }
                },
                Some(pattern) => match self.rows.get_screen_rows(id) {
                    ScreenRowsRead::Rows(rows) => match rows::extract(&pattern, &rows) {
                        Some(percent) => (
                            Some(percent),
                            ContextSample::Reading {
                                session_id: id,
                                percent,
                            },
                            false,
                        ),
                        None => (None, ContextSample::Unavailable { session_id: id }, false),
                    },
                    ScreenRowsRead::Unavailable => {
                        (None, ContextSample::Unavailable { session_id: id }, false)
                    }
                    ScreenRowsRead::SessionOver => {
                        (None, ContextSample::SessionOver { session_id: id }, true)
                    }
                },
            };

            let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
            if !self
                .registered
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&id)
            {
                continue;
            }

            // Every scrape outcome is offered before the badge equality gate. The production
            // sink is nonblocking and fail-soft, so a drop cannot suppress this gate or pruning.
            self.samples.observe(sample);

            if session_over {
                // #1088 - if retirement cleared a live reading it emits the final
                // `None` badge; persist that same clear so the CLI value tracks the
                // badge. An already-cleared session emits nothing and persists nothing.
                if self.retire_session_under_sequence(id) {
                    changed_readings.push((id, None));
                }
                continue;
            }

            let changed = {
                let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
                match registered.get_mut(&id) {
                    Some(entry) if entry.last_emitted != reading => {
                        entry.last_emitted = reading;
                        true
                    }
                    _ => false,
                }
            };
            if changed {
                self.sink.emit(ContextUsagePayload {
                    session_id: id.to_string(),
                    percent: reading,
                });
                // #1088 - same change-gate that drives the badge drives the
                // persist, so the CLI value tracks the badge point-for-point.
                changed_readings.push((id, reading));
            }
        }

        // #1088 - hand this tick's changed readings to the persist sink exactly
        // once. The `registered` std Mutex and the per-iteration `sequence` guard
        // are both scoped inside the loop and released before this `.await`, so no
        // std guard is held across the commit. Empty vector => no commit call, so an
        // all-unchanged tick takes no manager lock and performs no write.
        if !changed_readings.is_empty() {
            self.persist.commit(changed_readings).await;
        }
    }

    #[cfg(test)]
    fn compile_count(&self) -> usize {
        self.compiles.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn is_registered(&self, id: Uuid) -> bool {
        self.is_session_registered(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    const CLAUDE: &str = r"^ {2}Context [\u{2591}\u{2588}]+ (\d{1,3})%";
    const AGENT: &str = "claude";

    fn row(percent: u8) -> String {
        format!("  Context \u{2591}\u{2591}\u{2588} {percent}%")
    }

    /// A THREE-state rows fake, because the seam is three-state. Under a two-state
    /// `Option<Vec<String>>` the H1 defect was not merely untested, it was unsayable: the
    /// fake had no way to express "a live child we could not ask", so no test could have
    /// caught the design pruning it forever.
    #[derive(Default)]
    struct RowsFake {
        script: StdMutex<Vec<ScreenRowsRead>>,
        calls: StdMutex<Vec<Uuid>>,
        liveness_script: StdMutex<Vec<ContextSessionLiveness>>,
        liveness_calls: StdMutex<Vec<Uuid>>,
    }

    impl RowsFake {
        fn scripted(reads: Vec<ScreenRowsRead>) -> Arc<Self> {
            Arc::new(Self {
                script: StdMutex::new(reads),
                calls: StdMutex::new(Vec::new()),
                liveness_script: StdMutex::new(Vec::new()),
                liveness_calls: StdMutex::new(Vec::new()),
            })
        }
        fn with_liveness(reads: Vec<ContextSessionLiveness>) -> Arc<Self> {
            Arc::new(Self {
                script: StdMutex::new(Vec::new()),
                calls: StdMutex::new(Vec::new()),
                liveness_script: StdMutex::new(reads),
                liveness_calls: StdMutex::new(Vec::new()),
            })
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
        fn liveness_call_count(&self) -> usize {
            self.liveness_calls.lock().unwrap().len()
        }
    }

    impl ScreenRowsSource for RowsFake {
        fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
            self.calls.lock().unwrap().push(id);
            let mut script = self.script.lock().unwrap();
            if script.is_empty() {
                ScreenRowsRead::Unavailable
            } else {
                script.remove(0)
            }
        }

        fn get_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
            self.liveness_calls.lock().unwrap().push(id);
            let mut script = self.liveness_script.lock().unwrap();
            if script.is_empty() {
                ContextSessionLiveness::Unavailable
            } else {
                script.remove(0)
            }
        }
    }

    #[derive(Default)]
    struct PatternFake {
        patterns: StdMutex<HashMap<String, String>>,
        calls: StdMutex<usize>,
    }

    impl PatternFake {
        fn with(agent: &str, source: &str) -> Arc<Self> {
            let fake = Self::default();
            fake.patterns
                .lock()
                .unwrap()
                .insert(agent.to_string(), source.to_string());
            Arc::new(fake)
        }
        fn set(&self, agent: &str, source: Option<&str>) {
            let mut patterns = self.patterns.lock().unwrap();
            match source {
                Some(source) => patterns.insert(agent.to_string(), source.to_string()),
                None => patterns.remove(agent),
            };
        }
        fn call_count(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    impl ContextPatternSource for PatternFake {
        fn patterns(&self) -> BoxFuture<'_, HashMap<String, String>> {
            Box::pin(async move {
                *self.calls.lock().unwrap() += 1;
                self.patterns.lock().unwrap().clone()
            })
        }
    }

    #[derive(Default)]
    struct SinkFake {
        emitted: StdMutex<Vec<Option<u8>>>,
    }

    impl SinkFake {
        fn emitted(&self) -> Vec<Option<u8>> {
            self.emitted.lock().unwrap().clone()
        }
    }

    impl ContextEventSink for SinkFake {
        fn emit(&self, payload: ContextUsagePayload) {
            self.emitted.lock().unwrap().push(payload.percent);
        }
    }

    #[derive(Default)]
    struct SamplesFake {
        observed: StdMutex<Vec<ContextSample>>,
    }

    impl SamplesFake {
        fn observed(&self) -> Vec<ContextSample> {
            self.observed.lock().unwrap().clone()
        }
    }

    impl ContextSampleSink for SamplesFake {
        fn observe(&self, sample: ContextSample) {
            self.observed.lock().unwrap().push(sample);
        }
    }

    /// One recorded `commit` call: the changed `(id, percent)` pairs for one tick.
    type CommittedTick = Vec<(Uuid, Option<u8>)>;

    /// #1088 - records every `commit(changed)` the scraper hands the persist sink.
    /// Each element is one tick's committed vector; `commits().len()` is the commit
    /// call count (0 when a tick changed nothing).
    #[derive(Default)]
    struct PersistFake {
        commits: StdMutex<Vec<CommittedTick>>,
    }

    impl PersistFake {
        fn commits(&self) -> Vec<CommittedTick> {
            self.commits.lock().unwrap().clone()
        }
    }

    impl ContextPersistSink for PersistFake {
        fn commit(&self, changed: Vec<(Uuid, Option<u8>)>) -> BoxFuture<'_, ()> {
            self.commits.lock().unwrap().push(changed);
            Box::pin(async {})
        }
    }

    struct Harness {
        scraper: Arc<ContextScraper>,
        rows: Arc<RowsFake>,
        patterns: Arc<PatternFake>,
        sink: Arc<SinkFake>,
        samples: Arc<SamplesFake>,
        persist: Arc<PersistFake>,
    }

    fn harness(rows: Arc<RowsFake>, patterns: Arc<PatternFake>) -> Harness {
        let sink = Arc::new(SinkFake::default());
        let samples = Arc::new(SamplesFake::default());
        let persist = Arc::new(PersistFake::default());
        let scraper = ContextScraper::new(
            Arc::clone(&rows) as Arc<dyn ScreenRowsSource>,
            Arc::clone(&patterns) as Arc<dyn ContextPatternSource>,
            Arc::clone(&sink) as Arc<dyn ContextEventSink>,
            Arc::clone(&samples) as Arc<dyn ContextSampleSink>,
            Arc::clone(&persist) as Arc<dyn ContextPersistSink>,
        );
        Harness {
            scraper,
            rows,
            patterns,
            sink,
            samples,
            persist,
        }
    }

    // Stage E (#1064) retirement race sentinel (plan section 10.4 item 21,
    // acceptance item 35): retiring a session that never emitted a positive
    // reading (last_emitted == None) emits NO final null, and repeated retirement
    // is idempotent. A mutation that emitted a duplicate/unconditional null fails.
    #[tokio::test]
    async fn stage_e_retiring_a_never_emitted_session_emits_no_final_null() {
        let h = harness(RowsFake::scripted(vec![]), Arc::new(PatternFake::default()));
        let id = Uuid::new_v4();
        h.scraper.register_session(id, AGENT.to_string());

        // Retire before any positive reading was ever emitted.
        h.scraper.retire_session(id);
        assert!(
            h.sink.emitted().is_empty(),
            "a never-emitted session must not produce a final null"
        );
        // Repeated retirement stays idempotent and emits nothing.
        h.scraper.retire_session(id);
        assert!(
            h.sink.emitted().is_empty(),
            "repeated retirement must not emit a duplicate null"
        );
        assert!(
            !h.scraper.is_session_registered(id),
            "a retired session is unregistered"
        );
    }

    /// Criterion 5. Not "emits nothing", but "costs nothing": the rows fake counts calls,
    /// and an unconfigured agent must never reach it - no PTY lock, no 30-row clone.
    #[tokio::test]
    async fn a_session_whose_agent_has_no_pattern_takes_no_lock_and_emits_nothing() {
        let h = harness(RowsFake::scripted(vec![]), Arc::new(PatternFake::default()));
        h.scraper
            .register_session(Uuid::new_v4(), AGENT.to_string());

        h.scraper.tick().await;
        h.scraper.tick().await;

        assert_eq!(h.rows.call_count(), 0, "no pattern must mean no rows read");
        assert!(h.sink.emitted().is_empty(), "and no event");
        assert_eq!(h.samples.observed().len(), 2, "one sample per tick");
    }

    #[tokio::test]
    async fn missing_pattern_uses_only_lightweight_liveness_and_prunes_on_end() {
        let rows = RowsFake::with_liveness(vec![
            ContextSessionLiveness::Live,
            ContextSessionLiveness::SessionOver,
        ]);
        let h = harness(Arc::clone(&rows), Arc::new(PatternFake::default()));
        let id = Uuid::new_v4();
        h.scraper.register_session(id, AGENT.to_string());

        h.scraper.tick().await;
        assert!(h.scraper.is_registered(id));
        h.scraper.tick().await;

        assert_eq!(rows.call_count(), 0, "no screen rows may be cloned");
        assert_eq!(rows.liveness_call_count(), 2);
        assert_eq!(
            h.samples.observed(),
            vec![
                ContextSample::Unavailable { session_id: id },
                ContextSample::SessionOver { session_id: id },
            ]
        );
        assert!(!h.scraper.is_registered(id));
    }

    #[tokio::test]
    async fn blank_and_invalid_patterns_use_liveness_without_reading_rows() {
        for (source, liveness) in [
            ("   ", ContextSessionLiveness::Live),
            (r"(unclosed", ContextSessionLiveness::Unavailable),
        ] {
            let rows = RowsFake::with_liveness(vec![liveness]);
            let h = harness(Arc::clone(&rows), PatternFake::with(AGENT, source));
            let id = Uuid::new_v4();
            h.scraper.register_session(id, AGENT.to_string());

            h.scraper.tick().await;

            assert_eq!(rows.call_count(), 0, "source={source:?}");
            assert_eq!(rows.liveness_call_count(), 1, "source={source:?}");
            assert_eq!(
                h.samples.observed(),
                vec![ContextSample::Unavailable { session_id: id }]
            );
            assert!(h.scraper.is_registered(id));
        }
    }

    #[tokio::test]
    async fn unregister_is_idempotent_and_prevents_future_probes() {
        let rows = RowsFake::with_liveness(vec![ContextSessionLiveness::Live]);
        let h = harness(Arc::clone(&rows), Arc::new(PatternFake::default()));
        let id = Uuid::new_v4();
        h.scraper.register_session(id, AGENT.to_string());

        h.scraper.retire_session(id);
        h.scraper.retire_session(id);
        h.scraper.tick().await;

        assert!(!h.scraper.is_session_registered(id));
        assert_eq!(rows.liveness_call_count(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retirement_discards_an_inflight_row_result_and_emits_final_null_once() {
        struct BlockingRows {
            calls: AtomicUsize,
            started: std::sync::mpsc::Sender<()>,
            release: StdMutex<std::sync::mpsc::Receiver<()>>,
        }

        impl ScreenRowsSource for BlockingRows {
            fn get_screen_rows(&self, _id: Uuid) -> ScreenRowsRead {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    return ScreenRowsRead::Rows(vec![row(42)]);
                }
                self.started.send(()).expect("signal row probe");
                self.release
                    .lock()
                    .unwrap()
                    .recv()
                    .expect("release row probe");
                ScreenRowsRead::Rows(vec![row(7)])
            }
        }

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let rows = Arc::new(BlockingRows {
            calls: AtomicUsize::new(0),
            started: started_tx,
            release: StdMutex::new(release_rx),
        });
        let patterns = PatternFake::with(AGENT, CLAUDE);
        let sink = Arc::new(SinkFake::default());
        let samples = Arc::new(SamplesFake::default());
        let persist = Arc::new(PersistFake::default());
        let scraper = ContextScraper::new(
            Arc::clone(&rows) as Arc<dyn ScreenRowsSource>,
            patterns as Arc<dyn ContextPatternSource>,
            Arc::clone(&sink) as Arc<dyn ContextEventSink>,
            Arc::clone(&samples) as Arc<dyn ContextSampleSink>,
            Arc::clone(&persist) as Arc<dyn ContextPersistSink>,
        );
        let id = Uuid::new_v4();
        scraper.register_session(id, AGENT.to_string());
        scraper.tick().await;
        assert_eq!(sink.emitted(), vec![Some(42)]);

        let tick = {
            let scraper = Arc::clone(&scraper);
            tokio::spawn(async move { scraper.tick().await })
        };
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("row probe started");
        scraper.retire_session(id);
        scraper.retire_session(id);
        release_tx.send(()).expect("release row probe");
        tick.await.expect("join tick");

        assert_eq!(sink.emitted(), vec![Some(42), None]);
        assert_eq!(
            samples.observed(),
            vec![ContextSample::Reading {
                session_id: id,
                percent: 42,
            }]
        );
        assert!(!scraper.is_registered(id));
    }

    /// The empty-map guard, in the only window it covers: app start until the first agent
    /// session. It is two lines and it is honest about its scope.
    #[tokio::test]
    async fn an_empty_registered_map_never_reads_patterns() {
        let h = harness(RowsFake::scripted(vec![]), PatternFake::with(AGENT, CLAUDE));

        h.scraper.tick().await;

        assert_eq!(h.patterns.call_count(), 0);
    }

    /// Clearing the regex must clear the badge. An earlier design skipped unconfigured
    /// agents entirely, which left the badge showing 42 forever for a feature the user had
    /// just switched off.
    #[tokio::test]
    async fn clearing_a_pattern_emits_null_once() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec![row(42)]),
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        let id = Uuid::new_v4();
        h.scraper.register_session(id, AGENT.to_string());

        h.scraper.tick().await;
        assert_eq!(h.sink.emitted(), vec![Some(42)]);

        h.patterns.set(AGENT, None);
        h.scraper.tick().await;
        h.scraper.tick().await;

        assert_eq!(
            h.sink.emitted(),
            vec![Some(42), None],
            "null once, then silence"
        );
        assert!(
            h.scraper.is_registered(id),
            "clearing a regex does not end a session"
        );
    }

    /// The other half: typing a regex passes THROUGH invalid states, and an invalid pattern
    /// must clear the badge rather than freeze it.
    #[tokio::test]
    async fn a_pattern_edited_to_something_invalid_emits_null_once() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec![row(42)]),
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        h.scraper
            .register_session(Uuid::new_v4(), AGENT.to_string());

        h.scraper.tick().await;
        h.patterns.set(AGENT, Some(r"^ {2}Context (\d{1,3}%"));
        h.scraper.tick().await;
        h.scraper.tick().await;

        assert_eq!(h.sink.emitted(), vec![Some(42), None]);
    }

    /// The failure is sticky, so the log is too: the compile site IS the log site, and a
    /// broken regex must not write a line every 5 seconds for the rest of the app's life.
    #[tokio::test]
    async fn an_uncompilable_pattern_logs_once_per_change_not_once_per_tick() {
        let h = harness(
            RowsFake::scripted(vec![]),
            PatternFake::with(AGENT, r"^ {2}Context (\d{1,3}%"),
        );
        h.scraper
            .register_session(Uuid::new_v4(), AGENT.to_string());

        h.scraper.tick().await;
        h.scraper.tick().await;
        h.scraper.tick().await;

        assert_eq!(h.scraper.compile_count(), 1, "one compile, hence one log");
        assert_eq!(h.rows.call_count(), 0, "and a broken pattern reads no rows");
    }

    /// The compile cache is what makes per-tick resolution affordable, and per-tick
    /// resolution is what deletes the respawn requirement: paste a regex, see it work,
    /// without restarting every running agent.
    #[tokio::test]
    async fn a_changed_pattern_is_recompiled_within_one_tick() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec!["  Ctx 7%".to_string()]),
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        h.scraper
            .register_session(Uuid::new_v4(), AGENT.to_string());

        h.scraper.tick().await;
        assert_eq!(h.sink.emitted(), vec![Some(42)]);

        h.patterns.set(AGENT, Some(r"^ {2}Ctx (\d{1,3})%"));
        h.scraper.tick().await;

        assert_eq!(h.scraper.compile_count(), 2);
        assert_eq!(h.sink.emitted(), vec![Some(42), Some(7)]);
    }

    #[tokio::test]
    async fn an_unchanged_pattern_is_not_recompiled() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec![row(42)]),
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        h.scraper
            .register_session(Uuid::new_v4(), AGENT.to_string());

        h.scraper.tick().await;
        h.scraper.tick().await;
        h.scraper.tick().await;

        assert_eq!(h.scraper.compile_count(), 1);
    }

    #[tokio::test]
    async fn session_over_emits_null_once_and_prunes() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::SessionOver,
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        let id = Uuid::new_v4();
        h.scraper.register_session(id, AGENT.to_string());

        h.scraper.tick().await;
        h.scraper.tick().await;
        let reads_at_prune = h.rows.call_count();

        assert_eq!(h.sink.emitted(), vec![Some(42), None]);
        assert!(
            !h.scraper.is_registered(id),
            "a session that is over stops being sampled"
        );

        h.scraper.tick().await;
        assert_eq!(
            h.rows.call_count(),
            reads_at_prune,
            "and is never read again"
        );
        assert_eq!(
            h.sink.emitted(),
            vec![Some(42), None],
            "nor emitted for again"
        );
    }

    /// Criterion 7, and the reason `ScreenRowsRead` exists.
    ///
    /// A child that is alive but whose handle cannot be queried is NOT a dead session. The
    /// design this replaced pruned on it, which is not a 5-second flicker: `registered` is
    /// only ever written at spawn, so the entry never comes back and the badge stays N/A for
    /// the rest of a session whose child never died. Reproduced against real Windows
    /// processes: the same live process reads Alive through a full-rights handle and
    /// Unqueryable through one stripped of SYNCHRONIZE.
    #[tokio::test]
    async fn a_live_child_that_cannot_be_queried_is_not_pruned() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Unavailable,
                ScreenRowsRead::Rows(vec![row(42)]),
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        let id = Uuid::new_v4();
        h.scraper.register_session(id, AGENT.to_string());

        h.scraper.tick().await;
        h.scraper.tick().await;
        assert!(
            h.scraper.is_registered(id),
            "we could not ask is not the child is dead: the entry must survive"
        );

        h.scraper.tick().await;

        assert_eq!(
            h.sink.emitted(),
            vec![Some(42), None, Some(42)],
            "the reading comes back on the tick after the handle answers again"
        );
        assert_eq!(
            h.samples.observed(),
            vec![
                ContextSample::Reading {
                    session_id: id,
                    percent: 42,
                },
                ContextSample::Unavailable { session_id: id },
                ContextSample::Reading {
                    session_id: id,
                    percent: 42,
                },
            ]
        );
    }

    #[tokio::test]
    async fn an_unchanged_value_emits_once() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec![row(42)]),
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        h.scraper
            .register_session(Uuid::new_v4(), AGENT.to_string());

        h.scraper.tick().await;
        h.scraper.tick().await;
        h.scraper.tick().await;

        assert_eq!(h.sink.emitted(), vec![Some(42)]);
        assert_eq!(
            h.samples.observed(),
            vec![
                ContextSample::Reading {
                    session_id: h
                        .scraper
                        .registered
                        .lock()
                        .unwrap()
                        .keys()
                        .next()
                        .copied()
                        .unwrap(),
                    percent: 42,
                };
                3
            ]
        );
    }

    /// No monotonicity: `/clear` and `/compact` are the whole point of watching this number.
    #[tokio::test]
    async fn a_decrease_is_emitted() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(80)]),
                ScreenRowsRead::Rows(vec![row(12)]),
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        h.scraper
            .register_session(Uuid::new_v4(), AGENT.to_string());

        h.scraper.tick().await;
        h.scraper.tick().await;

        assert_eq!(h.sink.emitted(), vec![Some(80), Some(12)]);
    }

    /// Two sessions of the same agent share a pattern and nothing else. Each gets its own
    /// entry and its own first emit - which is a thing production does every time a user
    /// launches a second Claude.
    #[tokio::test]
    async fn a_second_session_for_the_same_agent_gets_its_own_entry_and_first_emit() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec![row(42)]),
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        h.scraper
            .register_session(Uuid::new_v4(), AGENT.to_string());
        h.scraper
            .register_session(Uuid::new_v4(), AGENT.to_string());

        h.scraper.tick().await;

        assert_eq!(
            h.sink.emitted(),
            vec![Some(42), Some(42)],
            "both sessions are new to the badge, so both emit"
        );
        assert_eq!(h.scraper.compile_count(), 1, "one agent, one compile");
    }

    // ── #1088: the coalesced per-tick persist hand-off ──

    #[tokio::test]
    async fn a_changed_reading_commits_once_and_is_independent_of_badge_and_samples() {
        let h = harness(
            RowsFake::scripted(vec![ScreenRowsRead::Rows(vec![row(42)])]),
            PatternFake::with(AGENT, CLAUDE),
        );
        let id = Uuid::new_v4();
        h.scraper.register_session(id, AGENT.to_string());

        h.scraper.tick().await;

        assert_eq!(h.persist.commits(), vec![vec![(id, Some(42))]]);
        // The badge and sample paths still behave exactly as before.
        assert_eq!(h.sink.emitted(), vec![Some(42)]);
        assert_eq!(h.samples.observed().len(), 1);
    }

    #[tokio::test]
    async fn an_unchanged_reading_does_not_commit_again() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec![row(42)]),
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        let id = Uuid::new_v4();
        h.scraper.register_session(id, AGENT.to_string());

        h.scraper.tick().await;
        assert_eq!(
            h.persist.commits(),
            vec![vec![(id, Some(42))]],
            "the first reading commits once"
        );

        // Second tick: reading unchanged => change-gate suppresses => no new commit.
        h.scraper.tick().await;
        assert_eq!(
            h.persist.commits().len(),
            1,
            "an unchanged tick performs no commit (idle app => ~0 writes)"
        );
    }

    #[tokio::test]
    async fn two_sessions_changing_in_one_tick_coalesce_into_one_commit() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::Rows(vec![row(42)]),
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        h.scraper.register_session(id1, AGENT.to_string());
        h.scraper.register_session(id2, AGENT.to_string());

        h.scraper.tick().await;

        let commits = h.persist.commits();
        assert_eq!(commits.len(), 1, "coalesced: one commit for the whole tick");
        let committed = &commits[0];
        assert_eq!(committed.len(), 2, "both changed sessions in one commit");
        assert!(committed.contains(&(id1, Some(42))));
        assert!(committed.contains(&(id2, Some(42))));
    }

    #[tokio::test]
    async fn a_cleared_reading_commits_none_once() {
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(42)]),
                ScreenRowsRead::SessionOver,
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        let id = Uuid::new_v4();
        h.scraper.register_session(id, AGENT.to_string());

        h.scraper.tick().await;
        h.scraper.tick().await;

        assert_eq!(
            h.persist.commits(),
            vec![vec![(id, Some(42))], vec![(id, None)]],
            "Some(42) then a single (id, None) on the clearing tick"
        );
    }

    #[tokio::test]
    async fn sorted_sample_order_rotates_one_position_each_tick() {
        let ids = [Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)];
        let h = harness(
            RowsFake::scripted(vec![
                ScreenRowsRead::Rows(vec![row(10)]),
                ScreenRowsRead::Rows(vec![row(20)]),
                ScreenRowsRead::Rows(vec![row(30)]),
                ScreenRowsRead::Rows(vec![row(40)]),
                ScreenRowsRead::Rows(vec![row(50)]),
                ScreenRowsRead::Rows(vec![row(60)]),
            ]),
            PatternFake::with(AGENT, CLAUDE),
        );
        for id in ids {
            h.scraper.register_session(id, AGENT.to_string());
        }

        h.scraper.tick().await;
        h.scraper.tick().await;

        let observed_ids: Vec<Uuid> = h
            .samples
            .observed()
            .into_iter()
            .map(|sample| match sample {
                ContextSample::Reading { session_id, .. }
                | ContextSample::Unavailable { session_id }
                | ContextSample::SessionOver { session_id } => session_id,
            })
            .collect();
        assert_eq!(
            observed_ids,
            vec![ids[0], ids[1], ids[2], ids[1], ids[2], ids[0]]
        );
    }

    #[test]
    fn none_serializes_as_an_explicit_null_not_an_absent_key() {
        let json = serde_json::to_string(&ContextUsagePayload {
            session_id: "3f2a".to_string(),
            percent: None,
        })
        .expect("serializes");
        assert_eq!(json, r#"{"sessionId":"3f2a","percent":null}"#);

        let json = serde_json::to_string(&ContextUsagePayload {
            session_id: "3f2a".to_string(),
            percent: Some(42),
        })
        .expect("serializes");
        assert_eq!(json, r#"{"sessionId":"3f2a","percent":42}"#);
    }
}
