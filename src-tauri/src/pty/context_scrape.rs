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
    fn patterns(&self) -> BoxFuture<'_, Option<HashMap<String, String>>>;
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
        if patterns.is_some() && !ids.is_empty() {
            let start = self.sample_cursor.fetch_add(1, Ordering::Relaxed) % ids.len();
            ids.rotate_left(start);
        }

        // #1088 - accumulate this tick's changed readings, handed to the persist
        // sink exactly once after the loop (coalesced: <=1 whole-file write per
        // tick regardless of how many sessions changed).
        let mut changed_readings: Vec<(Uuid, Option<u8>)> = Vec::new();

        for (id, agent_id) in ids {
            let Some(patterns) = patterns.as_ref() else {
                if self.rows.get_session_liveness(id) == ContextSessionLiveness::SessionOver {
                    let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
                    if self
                        .registered
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .contains_key(&id)
                    {
                        self.samples
                            .observe(ContextSample::SessionOver { session_id: id });
                        if self.retire_session_under_sequence(id) {
                            changed_readings.push((id, None));
                        }
                    }
                }
                continue;
            };
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
mod tests;
