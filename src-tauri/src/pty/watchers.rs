//! #1171 - generic, user-configured regex watchers over the plain de-ANSI'd rows of the
//! `vt100` screen mirror AC already keeps for every session.
//!
//! A sibling of `context_scrape`, not an extension of it. The two engines share only the
//! read boundary on `SessionIoFanout`; `ContextScraper` keeps its 5 s interval, its single
//! pattern per agent and its five sinks, untouched.
//!
//! **The engine cannot act.** Like `ContextScraper` (`context_scrape/mod.rs:5-8`) it holds
//! narrow trait objects and no `AppHandle` and no `PtyManager` of its own, so the worst a
//! hostile or careless pattern can do is put a wrong row in a window and a wrong number in a
//! counter. Injection capability plus a loose pattern would be a feedback loop: inject,
//! printed to the PTY, matches its own injection, inject again.
//!
//! **This is a best-effort indicator and not an audit log.** That is a property of the PTY
//! channel and it is stated in the UI.
//!
//! # Measured cost
//!
//! Taken by `read_seam_timing` below at AC's default 30x120 grid, against a session with a
//! live spinner - `output_sequence` advances on every CHUNK (`output.rs:160`), including
//! chunks that change no character, so a visually still screen is not a quiet one and the
//! unchanged short circuit must not be measured against one. Same form as
//! `context_scrape/mod.rs:22-25`:
//!
//! - `get_screen_rows`: ~81 us
//! - `get_screen_rows_since` on an UNCHANGED frame: ~30 ns
//! - `get_screen_rows_since` on a CHANGED frame: ~82 us
//!
//! So a changed read costs what the existing sample costs, plus the wrap flags and the cursor
//! row, which are O(1) under the guard the row clone already takes. An unchanged read is
//! ~2700x cheaper because it clones nothing, and that is enforced by the TYPE -
//! `ScreenRowsSince::Unchanged` carries no rows - rather than by this measurement.
//!
//! Machine: Windows 11, taken with optimizations on so the numbers are comparable to the
//! ~200 us `context_scrape` records. A plain `cargo test` build is unoptimized and its
//! numbers are not comparable to either; `--release` does not build the test target in this
//! tree, because `load_sessions_raw_from_dir_for_test` is `#[cfg(debug_assertions)]`.

mod activity;
pub mod dedupe;
mod evaluation;
pub(crate) mod frame;
pub(crate) mod history;
pub(crate) mod pattern;
mod resolution;

#[cfg(test)]
pub(crate) use crate::pty::backend::screen_frame::frame_from_screen_rows_read;
pub use crate::pty::backend::screen_frame::{FrameStamp, ScreenFrame, ScreenRowsSince};
pub(crate) use activity::{ActivityConsumer, ActivityFactory, ActivityObservation};
use activity::{ActivityDelivery, CODEX_WORKING_PATTERN};
use evaluation::{evaluate_occurrence, evaluate_state, status_of};
pub use resolution::{
    resolve_watchers, AgentResolution, ResolutionLog, ResolutionNotice, ResolvedWatcher,
    WatcherAgent,
};

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use uuid::Uuid;

use crate::pty::backend::PtyBackend;
use crate::pty::context_scrape::ContextSessionLiveness;

/// Re-exported so every watcher type reads from one module. They are DEFINED in
/// `config/settings.rs`, because that is where the user writes them.
pub use crate::config::settings::{WatcherDedupe, WatcherMode};

/// 200 ms, against `ContextScraper`'s 5 s. The engine has to catch rows in transit, and a row
/// that scrolls past between two samples is gone: the mirror keeps zero scrollback.
pub const TICK_INTERVAL: Duration = Duration::from_millis(200);

/// Liveness is probed on every 25th tick, that is once per 5 seconds per session - exactly
/// the rate `ContextScraper` already probes at. The probe is the expensive part of the old
/// read path (it takes the `ptys` mutex and asks the OS about the child), and running it at
/// the tick rate would make this engine 25x more expensive than the one it sits beside.
pub const LIVENESS_PROBE_EVERY_TICKS: u64 = 25;

/// The `row` field of a payload is capped at this many bytes, on a char boundary.
pub const MAX_ROW_BYTES: usize = 256;

/// At most this many matches per `(watcher, session)` per tick.
pub const MATCHES_PER_WATCHER_PER_TICK: usize = 8;

/// At most this many matches per session per tick, across all its watchers.
pub const MATCHES_PER_SESSION_PER_TICK: usize = 16;

/// The caps bound the RATE but not the duration, so a watcher that stays saturated would emit
/// 40 events a second forever and turn the 500-entry ring over in 0.31 seconds - making the
/// "older activations were dropped" banner permanent and useless. After this many consecutive
/// degraded ticks the pair is suspended, then retried.
pub const DEGRADED_TICKS_BEFORE_SUSPENSION: u32 = 25;

/// How long a saturated `(watcher, session)` pair stays suspended before being retried.
/// `degraded` stays true throughout, so the UI never claims it recovered.
pub const SUSPENSION: Duration = Duration::from_secs(5);

/// How many watchers may run on one agent. Resolution takes the first 8 in `BTreeMap` key
/// order, which is alphabetical over user-chosen ids, so adding a watcher named `aaa-test`
/// really can displace the eighth. That is why the dropped ones are both logged and reported
/// per row by `preview_watcher_reach`, instead of leaving the user with a log line.
pub const WATCHERS_PER_AGENT_BUDGET: usize = 8;

/// `dedupeWindowMs` is clamped to this on read.
///
/// Without it, a large window plus a `row` key would grow the key set to every distinct row
/// seen in that window; the 256-key bound per `(watcher, session)` is the other half of the
/// same defence.
pub const MAX_DEDUPE_WINDOW_MS: u64 = 60_000;

// ---- the engine ------------------------------------------------------------------------

/// One session's backend, narrowed to the ONE call the engine is allowed to make on it.
///
/// The engine resolves this once at registration and keeps it, so the tick never touches the
/// `PtyManager` mutex - "the one every terminal write, resize and kill locks on"
/// (`local_backend.rs:1116-1117`) - nor the route registry inside `kind_for_session`. What is
/// left per session per tick is a single `screen_parsers` acquisition, and that map is per
/// backend, so local and container sessions do not contend with each other at all.
///
/// It is a wrapper and not a bare `Arc<dyn PtyBackend>` because a bare one would hand the
/// engine `write`, `kill` and `spawn` along with the read it actually needs. The field is
/// private and there is no accessor, so the capability boundary this module claims is
/// expressed in the type rather than in a comment.
pub struct SessionFrameReader(Arc<dyn PtyBackend>);

impl SessionFrameReader {
    pub(crate) fn new(backend: Arc<dyn PtyBackend>) -> Self {
        Self(backend)
    }

    pub fn read(&self, id: Uuid, seen: Option<FrameStamp>) -> ScreenRowsSince {
        self.0.screen_rows_since(id, seen)
    }
}

/// Where a session's frame reader and its lightweight liveness come from.
///
/// The concrete implementation (`lib.rs`) owns the `PtyManager` handle; the engine holds only
/// this, exactly as `ContextScraper` holds only `ScreenRowsSource`.
pub trait WatcherBackendSource: Send + Sync {
    fn reader_for(&self, id: Uuid) -> Option<SessionFrameReader>;

    fn liveness(&self, id: Uuid) -> ContextSessionLiveness;
}

/// The configured watchers, resolved per agent, read fresh every tick.
///
/// `BoxFuture` and not a sync fn for the reason `ContextPatternSource` documents
/// (`context_scrape/mod.rs:65-72`): the settings live behind a `tokio::sync::RwLock` whose
/// `blocking_read` panics inside a runtime, and the tick is inside one.
pub trait WatcherPatternSource: Send + Sync {
    fn resolve(&self) -> BoxFuture<'_, Option<HashMap<String, AgentResolution>>>;
}

/// Where a tick's matches go. The concrete implementation holds the `AppHandle` and decides
/// whether anyone is listening; the engine never learns either.
pub trait WatcherEventSink: Send + Sync {
    fn emit(&self, batch: WatcherMatchBatch);
}

/// One tick's matches for one session. Coalesced on purpose: `app.emit` reaches every window,
/// so an uncoalesced per-match event would deliver thousands of payloads per second to four
/// windows and make every detached terminal pay to deserialize events it discards.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherMatchBatch {
    pub session_id: String,
    pub matches: Vec<WatcherMatchPayload>,
}

/// The IPC contract. Mould: `ContextUsagePayload` (`context_scrape/mod.rs:104-115`).
///
/// **No `skip_serializing_if` on any field**, for the reason that payload documents in
/// writing: an absent key must never become a third state beside null and the value. A
/// frontend row is self-contained once inserted, which is why `session_id` is repeated on
/// every match rather than read from the batch.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherMatchPayload {
    pub session_id: String,
    /// Monotonic per session, assigned by the engine as the match passes the caps. The SAME
    /// value the ring stores, so the window merges snapshot and stream on `(sessionId, seq)`
    /// instead of guessing. Without it two matches from one tick are indistinguishable, and
    /// identical rows at two positions are required to count twice.
    pub seq: u64,
    /// The key of the root `watchers` map. The same grouping key everywhere.
    pub watcher_id: String,
    pub mode: WatcherMode,
    /// The TICK's instant, not the match's: a match has no instant of its own.
    pub at: chrono::DateTime<chrono::Utc>,
    /// Groups 1..n IN ORDER, without group 0. `Option` per element because an optional group
    /// may not participate, and `""` is not "did not capture".
    pub captures: Vec<Option<String>>,
    /// The logical row, truncated to `MAX_ROW_BYTES` on a char boundary.
    pub row: String,
    /// Whether `row` lost bytes to the cap. `row.length >= 256` cannot answer this in
    /// TypeScript, because the cap is on bytes and the row is multibyte.
    pub row_truncated: bool,
}

/// Cap a row at `MAX_ROW_BYTES`, never splitting a character.
///
/// `pub(crate)` so `preview_watcher_pattern` shows the user exactly the string a real payload
/// would carry, rather than a second truncation rule that happens to agree today.
pub(crate) fn truncate_row(text: &str) -> (String, bool) {
    if text.len() <= MAX_ROW_BYTES {
        return (text.to_string(), false);
    }
    let mut end = MAX_ROW_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// A compiled pattern kept against the source it came from, so a recompile is decided by a
/// changed string and a failure is STICKY: logged once per change, never once per tick.
/// Mould: `Cached` (`context_scrape/mod.rs:127-156`).
enum Cached {
    Ok(Arc<pattern::WatcherPattern>),
    Failed { source: String },
}

impl Cached {
    fn source(&self) -> &str {
        match self {
            Cached::Ok(compiled) => compiled.source(),
            Cached::Failed { source } => source,
        }
    }

    fn pattern(&self) -> Option<Arc<pattern::WatcherPattern>> {
        match self {
            Cached::Ok(compiled) => Some(Arc::clone(compiled)),
            Cached::Failed { .. } => None,
        }
    }
}

/// One `(session, watcher)` pair's whole state: the `state` gate, the `occurrence` dedupe
/// keys, and the saturation bookkeeping.
///
/// **The `state` gate is NOT `(captures, row)` alone.** With only that pair, a second instance
/// of a condition appearing while the first is still visible never emits: the lowest match
/// still reads the same text, so the tuple never changes. That is the failure mode of the
/// permission-prompt watcher, which is the strongest argument for this engine existing.
#[derive(Default)]
struct WatcherSessionState {
    /// What the watcher looked like when this state was built. A watcher whose pattern or mode
    /// was edited starts again, because the old state describes a question no longer asked.
    signature: Option<(String, WatcherMode)>,
    last: Option<(Vec<Option<String>>, String, u64)>,
    /// Incremented whenever the number of matching logical rows RISES. Incrementing only on a
    /// rise is what makes the gate scroll-stable: the count of a persistent condition does not
    /// change as the screen moves under it.
    generation: u64,
    last_match_count: usize,
    keys: dedupe::DedupeKeys,
    /// True while this pair is hitting a per-tick cap or is suspended.
    degraded: bool,
    degraded_ticks: u32,
    degraded_logged: bool,
    suspended_until: Option<std::time::Instant>,
    /// Matches emitted for this pair since the session started.
    count: u64,
}

/// One session the engine is sampling.
struct RegisteredSession {
    /// `None` for a session sampled only by an internal activity consumer (#2879).
    agent_id: Option<String>,
    /// #2879 - the internal consumer attached at registration, if any.
    activity: Option<ActivityConsumer>,
    /// #2879 - the consumer's own view: the last frame it judged, and whether it must see a
    /// complete frame again (after `Missing`) before it may trust an `Unchanged`. Separate
    /// from `stamp`, which belongs to the user watchers.
    activity_stamp: Option<FrameStamp>,
    activity_full_read: bool,
    /// Resolved at registration; re-resolved only when a read comes back `Missing` or `Gone`,
    /// which covers a session that changed route.
    reader: Option<SessionFrameReader>,
    stamp: Option<FrameStamp>,
    /// Monotonic per session. The identity of a match, in the event and in the ring alike.
    seq: u64,
    /// Maintained for EVERY session regardless of configured modes, so switching a watcher's
    /// mode at runtime needs no reseed.
    diff: frame::FrameDiffState,
    watchers: HashMap<String, WatcherSessionState>,
    // Deliberately NO `warmed_up` flag here. "The engine has ticked this session" is already
    // expressed by the existence of the session's `WatcherHistory` entry, which is the value
    // the snapshot actually answers with; a second copy nobody reads is a fact that can only
    // ever drift out of step with the one that counts.
}

/// Runs every configured watcher over every registered session, five times a second.
///
/// Its total capability is: read one session's screen through a narrowed reader, ask for that
/// session's lightweight liveness, read the resolved watcher set, and hand a batch of matches
/// to a sink. It cannot route, inject, wake, or destroy anything.
pub struct WatcherEngine {
    backends: Arc<dyn WatcherBackendSource>,
    patterns: Arc<dyn WatcherPatternSource>,
    sink: Arc<dyn WatcherEventSink>,
    /// The engine publishes each session's STATUS here at the end of its tick, so
    /// `get_watcher_activity` can be synchronous and take one per-session mutex instead of
    /// resolving settings and the session manager itself. The MATCHES are written by the sink,
    /// which is where the ring belongs.
    history: history::WatcherHistoryState,
    /// Linearizes registration and retirement against evaluation and emission. Lock order is
    /// always sequence, then registered.
    sequence: Mutex<()>,
    registered: Mutex<HashMap<Uuid, RegisteredSession>>,
    compiled: Mutex<HashMap<String, Cached>>,
    ticks: AtomicU64,
    /// Test-visible: the number of `pattern::compile` calls. The compile site is also the log
    /// site, so this is what "logged once per change, not once per tick" is measured by.
    compiles: AtomicUsize,
    /// #2879 - fixed at construction. `None` keeps the engine exactly user-watchers-only.
    activity_factory: Option<ActivityFactory>,
    /// #2879 - `CODEX_WORKING_PATTERN`, compiled once. `None` with a factory means it did not
    /// compile, and every verdict is `Unavailable`.
    codex_working: Option<pattern::WatcherPattern>,
}

impl WatcherEngine {
    pub fn new(
        backends: Arc<dyn WatcherBackendSource>,
        patterns: Arc<dyn WatcherPatternSource>,
        sink: Arc<dyn WatcherEventSink>,
        history: history::WatcherHistoryState,
    ) -> Arc<Self> {
        Self::new_with_activity_factory(backends, patterns, sink, history, None)
    }

    /// #2879 - `new`, plus an internal activity factory consulted once per registration.
    pub(crate) fn new_with_activity_factory(
        backends: Arc<dyn WatcherBackendSource>,
        patterns: Arc<dyn WatcherPatternSource>,
        sink: Arc<dyn WatcherEventSink>,
        history: history::WatcherHistoryState,
        activity_factory: Option<ActivityFactory>,
    ) -> Arc<Self> {
        let codex_working = activity_factory.as_ref().and_then(|_| {
            pattern::compile(CODEX_WORKING_PATTERN)
                .map_err(|err| {
                    log::error!(
                        "[watchers] the internal Codex Working pattern does not compile; Codex \
                         activity will be reported unavailable: {err}"
                    )
                })
                .ok()
        });
        Arc::new(Self {
            backends,
            patterns,
            sink,
            history,
            sequence: Mutex::new(()),
            registered: Mutex::new(HashMap::new()),
            compiled: Mutex::new(HashMap::new()),
            ticks: AtomicU64::new(0),
            compiles: AtomicUsize::new(0),
            activity_factory,
            codex_working,
        })
    }

    /// Own thread, own runtime, shutdown token: `ContextScraper::start`'s shape
    /// (`context_scrape/mod.rs:207-227`), which is `GitWatcher`'s.
    ///
    /// The loop is wrapped so a panic in a tick cannot end the engine SILENTLY. Without this,
    /// the thread would unwind, every snapshot would freeze at its last value for the life of
    /// the process, and nothing anywhere would say why: the window would keep answering, with
    /// stale counters, which is worse than answering nothing. Containing at this boundary
    /// follows `probe_child_contained` (`local_backend.rs:1119-1124`), which catches for the
    /// same reason - a diagnostic path must not take something else down without a word.
    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let engine = Arc::clone(self);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
                .expect("Failed to create tokio runtime for WatcherEngine");
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                rt.block_on(async move {
                    loop {
                        tokio::select! {
                            biased;
                            _ = shutdown.token().cancelled() => {
                                log::info!("[watchers] Shutdown signal received, stopping");
                                break;
                            }
                            _ = tokio::time::sleep(TICK_INTERVAL) => {
                                engine.tick().await;
                            }
                        }
                    }
                });
            }));
            if outcome.is_err() {
                log::error!(
                    "[watchers] the watcher engine thread PANICKED and has stopped. No further \
                     activity will be sampled and every snapshot is frozen at its last value \
                     until AgentsCommander restarts. Nothing else is affected: the engine holds \
                     no PTY and no session state."
                );
            }
        });
    }

    /// Start sampling a session. Called once per session at the spawn chokepoint. A session
    /// with no agent and no internal activity consumer is never registered, so a plain shell
    /// costs nothing - the same rule `ContextScraper` follows (`commands/session.rs:2268-2274`).
    ///
    /// #2879 - the factory attaches outside every engine guard, and the attach is validated
    /// before the entry exists: one invalidated in between never samples, so it can neither
    /// publish nor reinstate detector state. A replaced entry's consumer is retired outside
    /// the guards; its identity is its own attach, so that can never release the new one.
    ///
    /// A fresh entry can never meet an existing one: session ids are minted per spawn and AC
    /// never reuses one, not even on respawn. That is also what makes `FrameStamp` safe.
    pub fn register_session(&self, id: Uuid, agent_id: Option<String>) {
        let activity = self
            .activity_factory
            .as_ref()
            .and_then(|factory| factory(id))
            .filter(|consumer| consumer(ActivityObservation::Validate));
        if agent_id.is_none() && activity.is_none() {
            return;
        }
        let reader = self.backends.reader_for(id);
        if reader.is_none() {
            // Not fatal and not worth a warning per tick: the first read re-resolves it.
            log::debug!("[watchers] session {id} has no backend route yet; will resolve on read");
        }
        let replaced = {
            let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
            self.registered
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    id,
                    RegisteredSession {
                        agent_id,
                        activity,
                        activity_stamp: None,
                        activity_full_read: false,
                        reader,
                        stamp: None,
                        seq: 0,
                        diff: frame::FrameDiffState::default(),
                        watchers: HashMap::new(),
                    },
                )
        };
        if let Some(consumer) = replaced.and_then(|session| session.activity) {
            consumer(ActivityObservation::Retired);
        }
    }

    /// Stop sampling a session. Idempotent, so every caller can just call it. #2879 - its
    /// internal consumer, if any, is retired after the engine guards are released.
    pub fn retire_session(&self, id: Uuid) {
        let retired = {
            let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
            self.registered
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id)
        };
        if let Some(consumer) = retired.and_then(|session| session.activity) {
            consumer(ActivityObservation::Retired);
        }
    }

    /// #2879 - whether any registered session has an internal consumer to sample for.
    fn samples_activity(&self) -> bool {
        self.registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .any(|session| session.activity.is_some())
    }

    pub fn is_session_registered(&self, id: Uuid) -> bool {
        self.registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&id)
    }

    /// Compile every watcher this tick resolved, once each, and drop the cache entries of
    /// watchers that are no longer configured.
    fn compile_for_tick(
        &self,
        resolved: &HashMap<String, AgentResolution>,
    ) -> HashMap<String, Option<Arc<pattern::WatcherPattern>>> {
        let mut wanted: HashMap<&str, &Arc<ResolvedWatcher>> = HashMap::new();
        for agent in resolved.values() {
            for watcher in &agent.running {
                wanted.entry(watcher.id.as_str()).or_insert(watcher);
            }
        }

        let mut cache = self.compiled.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|id, _| wanted.contains_key(id.as_str()));

        let mut out = HashMap::with_capacity(wanted.len());
        for (id, watcher) in wanted {
            if let Some(cached) = cache.get(id) {
                if cached.source() == watcher.pattern {
                    out.insert(id.to_string(), cached.pattern());
                    continue;
                }
            }

            self.compiles.fetch_add(1, Ordering::Relaxed);
            let cached = match pattern::compile(&watcher.pattern) {
                Ok(compiled) => Cached::Ok(Arc::new(compiled)),
                Err(err) => {
                    // Once per change, not once per tick: the cache below makes it sticky.
                    log::warn!("[watchers] watcher '{id}' has an unusable pattern: {err}");
                    Cached::Failed {
                        source: watcher.pattern.clone(),
                    }
                }
            };
            out.insert(id.to_string(), cached.pattern());
            cache.insert(id.to_string(), cached);
        }
        out
    }

    /// Publish "the engine reached you, and nothing watches you" for every registered session
    /// no watcher applies to.
    ///
    /// **This is what makes the window's day-one empty state reachable at all.** Without it,
    /// an unconfigured app and a session whose agent no watcher selects both answer
    /// `warmedUp: false` FOREVER rather than for 200 ms. The condition the window uses for
    /// "no watcher reaches this agent, here is the Configure button" is
    /// `warmedUp && activeWatchers == []`, so it would never hold, and the two cases
    /// `warmedUp` exists to tell apart would return byte-identical snapshots.
    ///
    /// It touches the registry and the history, and NOT the session: no backend is resolved,
    /// no PTY is read, no `screen_parsers` lock is taken and no row is allocated - which is
    /// what "the tick returns before touching any session" is protecting, and what the
    /// zero-cost criterion measures. `Vec::new()` does not allocate either.
    ///
    /// Held under the same registration lock the tick uses, so the "creation requires
    /// registration" rule that stops a purged session from being resurrected still holds.
    fn publish_unwatched(&self, resolved: &HashMap<String, AgentResolution>) {
        let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
        let registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        for (id, session) in registered.iter() {
            // #2879 - a session registered only for internal observation has no user view.
            let Some(agent_id) = &session.agent_id else {
                continue;
            };
            if resolved.contains_key(agent_id) {
                continue;
            }
            self.history.publish(
                *id,
                history::SessionStatus {
                    possibly_missed_frames: session.diff.possibly_missed_frames(),
                    active_watchers: Vec::new(),
                },
            );
        }
    }

    pub(crate) async fn tick(&self) {
        let tick_number = self.ticks.fetch_add(1, Ordering::Relaxed) + 1;

        // Settings FIRST, and the early exit is on the RESOLVED set rather than on an empty
        // registry: registration is per session, not per watcher, so a running agent with no
        // watcher configured would keep any registry non-empty. This is what actually
        // delivers "zero cost when unconfigured" - no `screen_parsers` lock, no row allocated,
        // no backend resolved, no liveness probed.
        let Some(resolved) = self.patterns.resolve().await else {
            let mut ids: Vec<(Uuid, bool)> = {
                let registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
                registered
                    .iter()
                    .map(|(id, session)| (*id, session.activity.is_some()))
                    .collect()
            };
            ids.sort_unstable_by_key(|(id, _)| *id);
            let probe_liveness = tick_number.is_multiple_of(LIVENESS_PROBE_EVERY_TICKS);
            let at = chrono::Utc::now();
            let compiled = HashMap::new();
            for (id, activity) in ids {
                if probe_liveness
                    && matches!(
                        self.backends.liveness(id),
                        ContextSessionLiveness::SessionOver
                    )
                {
                    self.retire_session(id);
                    continue;
                }
                if activity {
                    let (_, retire, delivery) = self.tick_session(id, None, &compiled, at);
                    if retire {
                        self.retire_session(id);
                    } else if let Some((consumer, observation)) = delivery {
                        consumer(observation);
                    }
                }
            }
            return;
        };
        // #2879 - an internal consumer is sampled whatever the user configured.
        if resolved.is_empty() && !self.samples_activity() {
            self.publish_unwatched(&resolved);
            return;
        }

        let compiled = if resolved.is_empty() {
            HashMap::new()
        } else {
            self.compile_for_tick(&resolved)
        };
        self.publish_unwatched(&resolved);

        // Snapshot first: the loop must not mutate `registered` while iterating it. The ids
        // are the union of sessions a user watcher reaches and those with internal consumers.
        let mut ids: Vec<(Uuid, Option<String>)> = {
            let registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
            registered
                .iter()
                .filter(|(_, session)| {
                    session.activity.is_some()
                        || session
                            .agent_id
                            .as_ref()
                            .is_some_and(|agent_id| resolved.contains_key(agent_id))
                })
                .map(|(id, session)| (*id, session.agent_id.clone()))
                .collect()
        };
        ids.sort_unstable_by_key(|(id, _)| *id);

        let probe_liveness = tick_number.is_multiple_of(LIVENESS_PROBE_EVERY_TICKS);
        let at = chrono::Utc::now();

        for (id, agent_id) in ids {
            let agent = agent_id
                .as_ref()
                .and_then(|agent_id| resolved.get(agent_id));

            if probe_liveness
                && matches!(
                    self.backends.liveness(id),
                    ContextSessionLiveness::SessionOver
                )
            {
                self.retire_session(id);
                continue;
            }

            let (batch, retire, delivery) = self.tick_session(id, agent, &compiled, at);
            if retire {
                self.retire_session(id);
            } else if let Some((consumer, observation)) = delivery {
                // Every engine guard is released; the callback returns before the next tick.
                consumer(observation);
            }
            if let Some(batch) = batch {
                self.sink.emit(batch);
            }
        }
    }

    /// One session's whole tick, with both locks scoped inside so retirement, emission and
    /// the internal consumer's callback all happen outside them. `agent` is `None` when no
    /// user watcher reaches the session and only its internal consumer is sampled.
    fn tick_session(
        &self,
        id: Uuid,
        agent: Option<&AgentResolution>,
        compiled: &HashMap<String, Option<Arc<pattern::WatcherPattern>>>,
        at: chrono::DateTime<chrono::Utc>,
    ) -> (Option<WatcherMatchBatch>, bool, ActivityDelivery) {
        let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
        let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        let Some(session) = registered.get_mut(&id) else {
            return (None, false, None);
        };

        // #2879 - one read serves both views. When they disagree on what they last saw (the
        // consumer owes itself a complete frame after `Missing`, or one view is newer), read
        // the complete frame and let each view decide below whether it is new to it.
        let consumer = session.activity.clone();
        let activity_seen = if session.activity_full_read {
            None
        } else {
            session.activity_stamp
        };
        let seen = match (agent.is_some(), consumer.is_some()) {
            (true, true) if activity_seen != session.stamp => None,
            (_, true) => activity_seen,
            _ => session.stamp,
        };
        let (frame, missing) = match self.read_frame(id, session, seen) {
            // The backend says there is nothing behind this id. Retire it now, and publish
            // nothing: an entry created here is one the purge that follows would have to race.
            ScreenRowsSince::Gone => return (None, true, None),
            ScreenRowsSince::Unchanged => (None, false),
            ScreenRowsSince::Missing => (None, true),
            ScreenRowsSince::Frame(frame) => (Some(frame), false),
        };
        let logical = frame.as_ref().map(frame::logical_rows);

        // #3059 - the internal verdict: ANY complete physical or logical indicator is `Present`.
        // Independent of every user watcher, cap, suspension and budget.
        let delivery = consumer.and_then(|consumer| {
            let observation = match &frame {
                Some(frame) if !(frame.stamp.is_some() && frame.stamp == activity_seen) => {
                    session.activity_stamp = frame.stamp;
                    session.activity_full_read = false;
                    Some(match &self.codex_working {
                        Some(working) => {
                            let rows = logical.as_deref().unwrap_or_default();
                            if frame.rows.iter().any(|row| working.regex().is_match(row))
                                || rows.iter().any(|row| working.regex().is_match(&row.text))
                            {
                                ActivityObservation::Present
                            } else {
                                ActivityObservation::Absent
                            }
                        }
                        None => ActivityObservation::Unavailable,
                    })
                }
                // The same frame it already judged: its verdict stands.
                Some(_) => None,
                // Unreadable: the verdict stands, and only a complete frame may refresh it.
                None if missing => {
                    session.activity_full_read = true;
                    Some(ActivityObservation::Unavailable)
                }
                None => None,
            };
            observation.map(|observation| (consumer, observation))
        });

        let Some(agent) = agent else {
            return (None, false, delivery);
        };
        // A frame the user view already saw is its `Unchanged`: a recovery read made for the
        // internal consumer must not advance the diff or re-emit anything.
        let Some(frame) = frame
            .as_ref()
            .filter(|frame| !(frame.stamp.is_some() && frame.stamp == session.stamp))
        else {
            // Nothing changed, so nothing can have started or stopped matching. No rows were
            // cloned to reach this arm - but the session HAS been ticked, so its status is
            // published and `warmedUp` becomes true.
            self.history.publish(id, status_of(session, agent));
            return (None, false, delivery);
        };
        session.stamp = frame.stamp;

        // A watcher that stopped reaching this agent leaves no state behind, so re-creating it
        // later starts from nothing - which is what "renaming is delete plus create" means.
        session
            .watchers
            .retain(|watcher_id, _| agent.running.iter().any(|w| &w.id == watcher_id));

        // The diff is advanced for EVERY session on every changed frame, whatever modes are
        // configured, so switching a watcher to `occurrence` at runtime needs no reseed.
        let evaluable = session.diff.advance(frame);
        let logical = logical.unwrap_or_default();
        let selected = frame::select_logical(&logical, &evaluable);

        let now = std::time::Instant::now();
        let mut next_seq = session.seq;
        let mut used_by_session = 0usize;
        let mut matches = Vec::new();

        for watcher in &agent.running {
            let Some(Some(compiled)) = compiled.get(&watcher.id) else {
                continue;
            };
            let state = session.watchers.entry(watcher.id.clone()).or_default();

            let signature = (watcher.pattern.clone(), watcher.mode);
            if state.signature.as_ref() != Some(&signature) {
                let count = state.count;
                *state = WatcherSessionState {
                    signature: Some(signature),
                    count,
                    ..WatcherSessionState::default()
                };
            }

            // A suspended pair is skipped whole, and stays marked degraded while it is: the UI
            // must never read "recovered" from a pair that is only being left alone.
            if let Some(until) = state.suspended_until {
                if now < until {
                    continue;
                }
                state.suspended_until = None;
                state.degraded_ticks = 0;
            }

            // The caps live HERE, immediately before the sink call, which is exactly where
            // `ContextScraper`'s equality gate lives. Everything that passes them reaches both
            // the event and the ring, and nothing else reaches either.
            let budget = MATCHES_PER_WATCHER_PER_TICK
                .min(MATCHES_PER_SESSION_PER_TICK.saturating_sub(used_by_session));

            let (found, capped) = match watcher.mode {
                WatcherMode::State => match evaluate_state(state, compiled, &logical) {
                    // Dropped by the cap: the gate is NOT committed, so the same activation is
                    // offered again next tick. Committing it here would turn the plan's "emits
                    // nothing further for that key THIS TICK" into "never again", silently -
                    // `degraded` clears on the next tick, so nothing would ever say the
                    // activation was lost.
                    Some(_) if budget == 0 => (Vec::new(), true),
                    Some((found, commit)) => {
                        state.commit_gate(commit);
                        (vec![found], false)
                    }
                    None => (Vec::new(), false),
                },
                WatcherMode::Occurrence => {
                    let outcome =
                        evaluate_occurrence(state, watcher, compiled, &selected, now, budget);
                    state
                        .keys
                        .prune(now, Duration::from_millis(watcher.dedupe_window_ms));
                    outcome
                }
            };

            for (captures, row, row_truncated) in found {
                next_seq += 1;
                used_by_session += 1;
                state.count += 1;
                matches.push(WatcherMatchPayload {
                    session_id: id.to_string(),
                    seq: next_seq,
                    watcher_id: watcher.id.clone(),
                    mode: watcher.mode,
                    at,
                    captures,
                    row,
                    row_truncated,
                });
            }

            if capped {
                state.degraded = true;
                if !state.degraded_logged {
                    log::warn!(
                        "[watchers] watcher '{}' on session {id} is over its per-tick cap; \
                         matches beyond it are counted and dropped",
                        watcher.id
                    );
                    state.degraded_logged = true;
                }
                state.degraded_ticks += 1;
                if state.degraded_ticks >= DEGRADED_TICKS_BEFORE_SUSPENSION {
                    state.suspended_until = Some(now + SUSPENSION);
                    log::warn!(
                        "[watchers] watcher '{}' on session {id} has been saturated for {} ticks; \
                         suspending it for {}s, then retrying",
                        watcher.id,
                        state.degraded_ticks,
                        SUSPENSION.as_secs()
                    );
                }
            } else {
                state.degraded = false;
                state.degraded_ticks = 0;
                state.degraded_logged = false;
            }
        }

        session.seq = next_seq;

        // Published while the registration lock is still held, so a retirement cannot
        // interleave: `purge_session_side_state` retires first and then purges, and creation
        // requires registration, so a tick in flight can never resurrect a purged session.
        self.history.publish(id, status_of(session, agent));

        if matches.is_empty() {
            return (None, false, delivery);
        }
        (
            Some(WatcherMatchBatch {
                session_id: id.to_string(),
                matches,
            }),
            false,
            delivery,
        )
    }

    /// Read a session's frame, re-resolving its backend once when the read says the route may
    /// have changed. The re-resolution is the only thing on this path that touches
    /// `PtyManager`, and it does not run on a healthy tick.
    fn read_frame(
        &self,
        id: Uuid,
        session: &mut RegisteredSession,
        seen: Option<FrameStamp>,
    ) -> ScreenRowsSince {
        let first = match &session.reader {
            Some(reader) => reader.read(id, seen),
            None => ScreenRowsSince::Missing,
        };
        match first {
            ScreenRowsSince::Missing | ScreenRowsSince::Gone => {
                match self.backends.reader_for(id) {
                    Some(reader) => {
                        let again = reader.read(id, seen);
                        session.reader = Some(reader);
                        again
                    }
                    None => first,
                }
            }
            other => other,
        }
    }

    #[cfg(test)]
    fn compile_count(&self) -> usize {
        self.compiles.load(Ordering::Relaxed)
    }

    /// `(degraded, suspended, count)` for one `(session, watcher)` pair.
    #[cfg(test)]
    fn watcher_state(&self, id: Uuid, watcher_id: &str) -> Option<(bool, bool, u64)> {
        let registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        let state = registered.get(&id)?.watchers.get(watcher_id)?;
        Some((state.degraded, state.suspended_until.is_some(), state.count))
    }

    /// Bring a suspension forward to now, so the retry can be observed without a test that
    /// sleeps for the suspension window.
    #[cfg(test)]
    fn expire_suspension_for_test(&self, id: Uuid, watcher_id: &str) {
        let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = registered
            .get_mut(&id)
            .and_then(|session| session.watchers.get_mut(watcher_id))
        {
            state.suspended_until = Some(std::time::Instant::now());
        }
    }
}

#[cfg(test)]
mod tests;
