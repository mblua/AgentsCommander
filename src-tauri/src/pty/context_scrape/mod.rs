//! #1032 - a per-session, best-effort reading of a coding agent's context-window usage,
//! taken by running a per-agent, user-configured regex over the plain de-ANSI'd rows of the
//! `vt100` screen mirror AC already keeps for every session.
//!
//! **The percentage is a signal for a human. It never drives an action.** That is not a
//! rule anyone here has to remember: the scraper holds three narrow trait objects and
//! nothing else - no `AppHandle`, no `PtyManager` - so the capability to write to a PTY or
//! kill a session is not reachable from this module at all.

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

/// The rows of one session's screen. Three states in, three states out.
pub trait ScreenRowsSource: Send + Sync {
    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead;
}

/// Every agent's configured pattern STRING, resolved fresh each tick.
///
/// `BoxFuture` and not a sync fn: the settings live behind a `tokio::sync::RwLock`, whose
/// `blocking_read` panics inside a runtime - and the tick is inside one. A sync signature
/// here would kill the scraper on tick 1, silently and permanently, for every session.
pub trait ContextPatternSource: Send + Sync {
    fn patterns(&self) -> BoxFuture<'_, HashMap<String, String>>;
}

/// Where a reading goes. The ONLY thing this module can do to the outside world.
pub trait ContextEventSink: Send + Sync {
    fn emit(&self, payload: ContextUsagePayload);
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

/// Samples every registered session on a timer and emits a reading when it changes.
///
/// Holds three trait objects and NOTHING else. No `AppHandle`: an `AppHandle` reaches
/// `PtyManager` through managed state, and from there `kill`, `write` and session
/// destruction - so "this never drives an action" would be a promise instead of a fact.
/// None of the three traits is downcastable back to its implementation (no `Any`
/// supertrait, no `as_any`), so the total capability of this type is: read rows, read
/// patterns, emit a payload.
pub struct ContextScraper {
    rows: Arc<dyn ScreenRowsSource>,
    patterns: Arc<dyn ContextPatternSource>,
    sink: Arc<dyn ContextEventSink>,
    registered: Mutex<HashMap<Uuid, Registered>>,
    compiled: Mutex<HashMap<String, Cached>>,
    /// Test-visible: the number of `pattern::compile` calls. The compile site is also the
    /// log site, so this is what "logged once per change, not once per tick" is measured by.
    compiles: AtomicUsize,
}

impl ContextScraper {
    pub fn new(
        rows: Arc<dyn ScreenRowsSource>,
        patterns: Arc<dyn ContextPatternSource>,
        sink: Arc<dyn ContextEventSink>,
    ) -> Arc<Self> {
        Arc::new(Self {
            rows,
            patterns,
            sink,
            registered: Mutex::new(HashMap::new()),
            compiled: Mutex::new(HashMap::new()),
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
        let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        registered.insert(
            id,
            Registered {
                agent_id,
                last_emitted: None,
            },
        );
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

    async fn tick(&self) {
        // Before `patterns()`, so an app with no agent session running reads nothing at all.
        // Honest bound: this window is app start until the first agent session, and no
        // longer - entries for unconfigured agents are never pruned, because reaching them
        // would cost either a PTY probe or a discarded 30-row clone per session per tick,
        // purely to reclaim ~100 bytes for a feature that is switched off.
        if self
            .registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            return;
        }

        let patterns = self.patterns.patterns().await;

        // Snapshot first: the loop must not mutate `registered` while iterating it.
        let ids: Vec<(Uuid, String)> = {
            let registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
            registered
                .iter()
                .map(|(id, entry)| (*id, entry.agent_id.clone()))
                .collect()
        };

        let mut over: Vec<Uuid> = Vec::new();

        for (id, agent_id) in ids {
            let (reading, session_over): (Option<u8>, bool) = match patterns.get(&agent_id) {
                // Not configured: no lock, no rows, no compile.
                None => (None, false),
                Some(source) => match self.resolve(&agent_id, source) {
                    // Does not compile: no lock, no rows either.
                    None => (None, false),
                    Some(pattern) => match self.rows.get_screen_rows(id) {
                        ScreenRowsRead::Rows(rows) => (rows::extract(&pattern, &rows), false),
                        ScreenRowsRead::Unavailable => (None, false),
                        ScreenRowsRead::SessionOver => (None, true),
                    },
                },
            };

            // ONE gate, for every state there is. Clearing a pattern, an invalid pattern, a
            // suppressed statusline, a dead session and a real decrease all arrive here, and
            // all of them emit exactly when the value changed.
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
            }

            if session_over {
                over.push(id);
            }
        }

        // After the loop, never during it.
        if !over.is_empty() {
            let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
            registered.retain(|id, _| !over.contains(id));
        }
    }

    #[cfg(test)]
    fn compile_count(&self) -> usize {
        self.compiles.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn is_registered(&self, id: Uuid) -> bool {
        self.registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&id)
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
    }

    impl RowsFake {
        fn scripted(reads: Vec<ScreenRowsRead>) -> Arc<Self> {
            Arc::new(Self {
                script: StdMutex::new(reads),
                calls: StdMutex::new(Vec::new()),
            })
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
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

    struct Harness {
        scraper: Arc<ContextScraper>,
        rows: Arc<RowsFake>,
        patterns: Arc<PatternFake>,
        sink: Arc<SinkFake>,
    }

    fn harness(rows: Arc<RowsFake>, patterns: Arc<PatternFake>) -> Harness {
        let sink = Arc::new(SinkFake::default());
        let scraper = ContextScraper::new(
            Arc::clone(&rows) as Arc<dyn ScreenRowsSource>,
            Arc::clone(&patterns) as Arc<dyn ContextPatternSource>,
            Arc::clone(&sink) as Arc<dyn ContextEventSink>,
        );
        Harness {
            scraper,
            rows,
            patterns,
            sink,
        }
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
