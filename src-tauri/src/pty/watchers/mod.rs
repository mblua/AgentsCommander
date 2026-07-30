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

pub mod frame;
pub mod pattern;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use uuid::Uuid;

use crate::config::coding_agents_catalog::command_executable_basename;
use crate::config::settings::{WatcherEntry, WatcherMode};
use crate::pty::backend::PtyBackend;
use crate::pty::context_scrape::ContextSessionLiveness;

pub use crate::config::settings::WatcherDedupe;

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

/// One enabled, well-formed watcher, ready for a tick.
///
/// Shared behind an `Arc` because the same watcher usually reaches many agents and its
/// pattern string must not be cloned once per agent per tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWatcher {
    pub id: String,
    pub mode: WatcherMode,
    pub pattern: String,
    pub dedupe: WatcherDedupe,
    /// Already clamped to `MAX_DEDUPE_WINDOW_MS`.
    pub dedupe_window_ms: u64,
}

/// What resolution needs to know about one configured agent: `settings.agents[i]`, reduced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatcherAgent {
    pub id: String,
    pub command: String,
}

/// The watchers that apply to one agent right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentResolution {
    /// In `BTreeMap` key order, at most `WATCHERS_PER_AGENT_BUDGET`.
    pub running: Vec<Arc<ResolvedWatcher>>,
    /// The ids that reach this agent but fell outside the budget, in the same order.
    pub over_budget: Vec<String>,
}

/// Something resolution wants to say, at most once per changed value.
///
/// Returned rather than logged, so the preview commands can run the exact same rule on every
/// keystroke without writing anything to `app.log`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionNotice {
    /// What the notice is ABOUT, kind-prefixed: a watcher id or an agent id. The log-once
    /// map is keyed by this and holds one entry per subject, so it is bounded by the number
    /// of configured watchers and agents. Keying by the message instead would let a user
    /// typing in Settings accumulate one entry per keystroke, which is the mistake
    /// `context_scrape`'s compile cache documents at `mod.rs:127-132`.
    pub subject: String,
    /// The value the message was derived from. A changed detail logs again; an unchanged one
    /// stays silent however many ticks pass.
    pub detail: String,
    pub message: String,
}

/// Cross the configured agents with the configured watchers.
///
/// Pure: no logging, no locks, no I/O. Called every tick by the engine's pattern source and
/// on every keystroke by `preview_watcher_reach`, and both get the same answer because there
/// is only one rule.
///
/// An agent appears in the map only when at least one watcher reaches it, so "the map is
/// empty" is exactly "there is nothing to do this tick" - which is what lets the tick return
/// before touching any session.
pub fn resolve_watchers(
    agents: &[WatcherAgent],
    watchers: &BTreeMap<String, WatcherEntry>,
) -> (HashMap<String, AgentResolution>, Vec<ResolutionNotice>) {
    let mut notices = Vec::new();
    let mut usable: Vec<(Arc<ResolvedWatcher>, Option<Vec<String>>)> = Vec::new();

    for (id, entry) in watchers {
        let config = match entry {
            WatcherEntry::Valid(config) => config,
            WatcherEntry::Invalid(value) => {
                let detail = value.to_string();
                notices.push(ResolutionNotice {
                    subject: format!("invalid:{id}"),
                    message: format!(
                        "[watchers] watcher '{id}' is not a valid watcher and is being skipped; \
                         every other watcher and every other setting is unaffected: {detail}"
                    ),
                    detail,
                });
                continue;
            }
        };

        // Disabled is a state the user chose, not a problem. It keeps its configuration and
        // says nothing.
        if !config.enabled {
            continue;
        }

        // A selector that does not tokenize skips the WHOLE watcher. Never "reaches
        // everything": a typo in one entry must not silently widen a pattern to every agent.
        let selector = match &config.commands {
            None => None,
            Some(list) => {
                let mut stems = Vec::with_capacity(list.len());
                let mut broken: Option<String> = None;
                for token in list {
                    match command_executable_basename(token) {
                        Some(stem) => stems.push(stem),
                        None => {
                            broken = Some(token.clone());
                            break;
                        }
                    }
                }
                if let Some(token) = broken {
                    let detail = list.join("\u{1f}");
                    notices.push(ResolutionNotice {
                        subject: format!("commands:{id}"),
                        message: format!(
                            "[watchers] watcher '{id}' is being skipped: its commands selector \
                             entry '{token}' is not a command. A watcher with an unreadable \
                             selector reaches nobody, never everybody"
                        ),
                        detail,
                    });
                    continue;
                }
                Some(stems)
            }
        };

        let mut dedupe_window_ms = config.dedupe_window_ms;
        if dedupe_window_ms > MAX_DEDUPE_WINDOW_MS {
            notices.push(ResolutionNotice {
                subject: format!("clamp:{id}"),
                message: format!(
                    "[watchers] watcher '{id}' asks for a {dedupe_window_ms} ms dedupe window; \
                     clamping to {MAX_DEDUPE_WINDOW_MS} ms"
                ),
                detail: dedupe_window_ms.to_string(),
            });
            dedupe_window_ms = MAX_DEDUPE_WINDOW_MS;
        }

        usable.push((
            Arc::new(ResolvedWatcher {
                id: id.clone(),
                mode: config.mode,
                pattern: config.pattern.clone(),
                dedupe: config.dedupe,
                dedupe_window_ms,
            }),
            selector,
        ));
    }

    let mut resolved: HashMap<String, AgentResolution> = HashMap::new();
    if usable.is_empty() {
        return (resolved, notices);
    }

    for agent in agents {
        // An agent whose own command does not tokenize is reached by selectorless watchers
        // and by no watcher with a selector: there is no stem to compare against, and
        // guessing one would be inventing a match. `validate_agent_commands`
        // (`settings.rs:1473-1475`) already rejects such a command on save.
        let agent_stem = command_executable_basename(&agent.command);

        let mut running = Vec::new();
        let mut over_budget = Vec::new();
        for (watcher, selector) in &usable {
            let reaches = match selector {
                None => true,
                Some(stems) => agent_stem
                    .as_ref()
                    .is_some_and(|stem| stems.iter().any(|candidate| candidate == stem)),
            };
            if !reaches {
                continue;
            }
            if running.len() < WATCHERS_PER_AGENT_BUDGET {
                running.push(Arc::clone(watcher));
            } else {
                over_budget.push(watcher.id.clone());
            }
        }

        if running.is_empty() {
            continue;
        }
        if !over_budget.is_empty() {
            let detail = over_budget.join(",");
            notices.push(ResolutionNotice {
                subject: format!("budget:{}", agent.id),
                message: format!(
                    "[watchers] agent '{}' is over the {WATCHERS_PER_AGENT_BUDGET}-watcher \
                     budget; these are configured but not running on it: {detail}",
                    agent.id
                ),
                detail,
            });
        }
        resolved.insert(
            agent.id.clone(),
            AgentResolution {
                running,
                over_budget,
            },
        );
    }

    (resolved, notices)
}

/// Log-once bookkeeping for `ResolutionNotice`s, across ticks.
///
/// Resolution runs five times a second, so every one of these messages would otherwise be a
/// five-per-second log line for as long as the configuration stays wrong. Keyed by subject
/// and holding the last detail logged for it: an unchanged problem stays silent, a CHANGED
/// one speaks again, and the map cannot grow past one entry per watcher and agent.
#[derive(Default)]
pub struct ResolutionLog {
    last: Mutex<HashMap<String, String>>,
}

impl ResolutionLog {
    /// Log the notices whose subject is new or whose detail changed. Returns how many lines
    /// were actually written, so "logged once across N ticks" is something a test can assert
    /// rather than something a reader has to trust.
    pub fn publish(&self, notices: Vec<ResolutionNotice>) -> usize {
        if notices.is_empty() {
            return 0;
        }
        let mut written = 0;
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        for notice in notices {
            let changed = last
                .get(&notice.subject)
                .is_none_or(|previous| previous != &notice.detail);
            if changed {
                log::warn!("{}", notice.message);
                last.insert(notice.subject, notice.detail);
                written += 1;
            }
        }
        written
    }

    /// Number of subjects currently remembered. The bound this type exists to keep.
    #[cfg(test)]
    fn remembered(&self) -> usize {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// Identifies one frame of one session's screen mirror.
///
/// **The size is part of the stamp on purpose.** `resize_screen_and_broadcast` reflows the
/// grid without bumping `output_sequence` (`output.rs:202-212`), so a sequence-only stamp
/// would report `Unchanged` over a screen that was just re-laid at a new width.
///
/// **`sequence` is monotonic only within one parser instance.** `register_session` inserts a
/// fresh parser at `output_sequence: 0` (`output.rs:109-116`). That is safe here only because
/// session ids are minted per spawn and AC never reuses one, not even on respawn
/// (`context_scrape/mod.rs:232-233`). #955's replay tolerates a reset; this engine does not.
/// If a future "reattach in place" ever reuses an id, the stamp would move BACKWARDS and the
/// engine could report `Unchanged` over a completely different screen.
///
/// `output_sequence` itself is never modified by this module: it is also the replay ordering
/// key `get_screen_snapshot` hands the frontend (`output.rs:274-285`, #955), and changing when
/// it advances would change that contract. The seam only reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStamp {
    /// `ScreenReplayState::output_sequence` (`output.rs:160`).
    pub sequence: u64,
    pub rows: u16,
    pub cols: u16,
}

/// One session's screen, as the engine needs to see it.
///
/// `wrapped` and `cursor_row` are read under the same guard the row clone already takes, at
/// O(1) each. Fetching them separately would mean a second lock acquisition on a possibly
/// different frame.
pub struct ScreenFrame {
    /// One entry per physical row, from `Screen::rows(0, cols)`.
    pub rows: Vec<String>,
    /// `Screen::row_wrapped(i)` for each i: does this physical row continue into the next
    /// one. Same length as `rows`, by construction.
    pub wrapped: Vec<bool>,
    /// `Screen::cursor_position().0`. The row currently being written.
    pub cursor_row: u16,
    /// `None` only from the default trait implementation, which has no sequence to report.
    /// `None` means "treat as changed", so "the default never reports `Unchanged`" falls out
    /// of the type rather than out of a rule someone has to remember.
    pub stamp: Option<FrameStamp>,
}

/// What a read of one session's screen can say.
///
/// The `Missing` and `Gone` split is what preserves the distinction `ScreenRowsRead` argues
/// for at length (`context_scrape/mod.rs:39-45`): "we could not read" must never be confused
/// with "there is nothing here any more".
pub enum ScreenRowsSince {
    /// The stamp matched. NO rows were cloned and no allocation was made.
    Unchanged,
    Frame(ScreenFrame),
    /// No parser for this id. Says NOTHING about whether the session is over, exactly like
    /// `get_screen_rows` returning `None` (`output.rs:287-294`). Keep sampling it.
    Missing,
    /// This backend has no session behind this id. Retire it now.
    Gone,
}

impl ScreenRowsSince {
    /// The frame, when there is one. Convenience for callers that treat every non-frame
    /// outcome the same way.
    pub fn frame(&self) -> Option<&ScreenFrame> {
        match self {
            ScreenRowsSince::Frame(frame) => Some(frame),
            _ => None,
        }
    }
}

/// #1171 - the default `PtyBackend::screen_rows_since`, shared by the trait default so the
/// mapping lives beside the types it maps into.
///
/// Everything that is not rows becomes `Missing`: a backend that only implements
/// `get_screen_rows` has no richer oracle to offer, and `Missing` is the arm that keeps
/// sampling rather than retiring.
pub(crate) fn frame_from_screen_rows_read(
    read: crate::pty::context_scrape::ScreenRowsRead,
) -> ScreenRowsSince {
    match read {
        crate::pty::context_scrape::ScreenRowsRead::Rows(rows) => {
            let wrapped = vec![false; rows.len()];
            ScreenRowsSince::Frame(ScreenFrame {
                rows,
                wrapped,
                cursor_row: 0,
                stamp: None,
            })
        }
        crate::pty::context_scrape::ScreenRowsRead::Unavailable
        | crate::pty::context_scrape::ScreenRowsRead::SessionOver => ScreenRowsSince::Missing,
    }
}

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
    pub fn new(backend: Arc<dyn PtyBackend>) -> Self {
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
    fn resolve(&self) -> BoxFuture<'_, HashMap<String, AgentResolution>>;
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
fn truncate_row(text: &str) -> (String, bool) {
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

/// The `state`-mode gate for one `(session, watcher)` pair.
///
/// The gate is NOT `(captures, row)` alone. With only that pair, a second instance of a
/// condition appearing while the first is still visible never emits: the lowest match still
/// reads the same text, so the tuple never changes. That is the failure mode of the
/// permission-prompt watcher, which is the strongest argument for this engine existing.
#[derive(Default)]
struct StateGate {
    /// What the watcher looked like when this gate was built. A watcher whose pattern or mode
    /// was edited gets a fresh gate, because the old one describes a question no longer asked.
    signature: Option<(String, WatcherMode)>,
    last: Option<(Vec<Option<String>>, String, u64)>,
    /// Incremented whenever the number of matching logical rows RISES. Incrementing only on a
    /// rise is what makes the gate scroll-stable: the count of a persistent condition does not
    /// change as the screen moves under it.
    generation: u64,
    last_match_count: usize,
}

/// One session the engine is sampling.
struct RegisteredSession {
    agent_id: String,
    /// Resolved at registration; re-resolved only when a read comes back `Missing` or `Gone`,
    /// which covers a session that changed route.
    reader: Option<SessionFrameReader>,
    stamp: Option<FrameStamp>,
    /// Monotonic per session. The identity of a match, in the event and in the ring alike.
    seq: u64,
    gates: HashMap<String, StateGate>,
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
    /// Linearizes registration and retirement against evaluation and emission. Lock order is
    /// always sequence, then registered.
    sequence: Mutex<()>,
    registered: Mutex<HashMap<Uuid, RegisteredSession>>,
    compiled: Mutex<HashMap<String, Cached>>,
    ticks: AtomicU64,
    /// Test-visible: the number of `pattern::compile` calls. The compile site is also the log
    /// site, so this is what "logged once per change, not once per tick" is measured by.
    compiles: AtomicUsize,
}

impl WatcherEngine {
    pub fn new(
        backends: Arc<dyn WatcherBackendSource>,
        patterns: Arc<dyn WatcherPatternSource>,
        sink: Arc<dyn WatcherEventSink>,
    ) -> Arc<Self> {
        Arc::new(Self {
            backends,
            patterns,
            sink,
            sequence: Mutex::new(()),
            registered: Mutex::new(HashMap::new()),
            compiled: Mutex::new(HashMap::new()),
            ticks: AtomicU64::new(0),
            compiles: AtomicUsize::new(0),
        })
    }

    /// Own thread, own runtime, shutdown token: `ContextScraper::start`'s shape
    /// (`context_scrape/mod.rs:207-227`), which is `GitWatcher`'s.
    pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
        let engine = Arc::clone(self);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
                .expect("Failed to create tokio runtime for WatcherEngine");
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
        });
    }

    /// Start sampling a session. Called once per AGENT session at the spawn chokepoint;
    /// sessions with no agent are never registered, so a plain shell costs nothing - the same
    /// rule `ContextScraper` follows (`commands/session.rs:2268-2274`).
    ///
    /// A fresh entry can never meet an existing one: session ids are minted per spawn and AC
    /// never reuses one, not even on respawn. That is also what makes `FrameStamp` safe.
    pub fn register_session(&self, id: Uuid, agent_id: String) {
        let reader = self.backends.reader_for(id);
        if reader.is_none() {
            // Not fatal and not worth a warning per tick: the first read re-resolves it.
            log::debug!("[watchers] session {id} has no backend route yet; will resolve on read");
        }
        let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
        self.registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id,
                RegisteredSession {
                    agent_id,
                    reader,
                    stamp: None,
                    seq: 0,
                    gates: HashMap::new(),
                },
            );
    }

    /// Stop sampling a session. Idempotent, so every caller can just call it.
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

    pub(crate) async fn tick(&self) {
        let tick_number = self.ticks.fetch_add(1, Ordering::Relaxed) + 1;

        // Settings FIRST, and the early exit is on the RESOLVED set rather than on an empty
        // registry: registration is per session, not per watcher, so a running agent with no
        // watcher configured would keep any registry non-empty. This is what actually
        // delivers "zero cost when unconfigured" - no `screen_parsers` lock, no allocation.
        let resolved = self.patterns.resolve().await;
        if resolved.is_empty() {
            return;
        }

        let compiled = self.compile_for_tick(&resolved);

        // Snapshot first: the loop must not mutate `registered` while iterating it.
        let mut ids: Vec<(Uuid, String)> = {
            let registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
            registered
                .iter()
                .map(|(id, session)| (*id, session.agent_id.clone()))
                .collect()
        };
        ids.sort_unstable_by_key(|(id, _)| *id);

        let probe_liveness = tick_number.is_multiple_of(LIVENESS_PROBE_EVERY_TICKS);
        let at = chrono::Utc::now();

        for (id, agent_id) in ids {
            let Some(agent) = resolved.get(&agent_id) else {
                continue;
            };

            if probe_liveness
                && matches!(
                    self.backends.liveness(id),
                    ContextSessionLiveness::SessionOver
                )
            {
                self.retire_session(id);
                continue;
            }

            let (batch, retire) = self.tick_session(id, agent, &compiled, at);
            if retire {
                self.retire_session(id);
            }
            if let Some(batch) = batch {
                self.sink.emit(batch);
            }
        }
    }

    /// One session's whole tick, with both locks scoped inside so retirement and emission
    /// happen outside them.
    fn tick_session(
        &self,
        id: Uuid,
        agent: &AgentResolution,
        compiled: &HashMap<String, Option<Arc<pattern::WatcherPattern>>>,
        at: chrono::DateTime<chrono::Utc>,
    ) -> (Option<WatcherMatchBatch>, bool) {
        let _sequence = self.sequence.lock().unwrap_or_else(|e| e.into_inner());
        let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        let Some(session) = registered.get_mut(&id) else {
            return (None, false);
        };

        let frame = match self.read_frame(id, session) {
            // Nothing changed, so nothing can have started or stopped matching. No rows were
            // cloned to reach this arm.
            ScreenRowsSince::Unchanged => return (None, false),
            // No reading this tick and NO claim about the session: keep it.
            ScreenRowsSince::Missing => return (None, false),
            // The backend says there is nothing behind this id. Retire it now.
            ScreenRowsSince::Gone => return (None, true),
            ScreenRowsSince::Frame(frame) => frame,
        };
        session.stamp = frame.stamp;

        // A watcher that stopped reaching this agent leaves no gate behind, so re-creating it
        // later starts from nothing - which is what "renaming is delete plus create" means.
        session
            .gates
            .retain(|gate_id, _| agent.running.iter().any(|w| &w.id == gate_id));

        let needs_state = agent
            .running
            .iter()
            .any(|watcher| watcher.mode == WatcherMode::State);
        if !needs_state {
            return (None, false);
        }

        let logical = frame::logical_rows(&frame);
        let mut matches = Vec::new();
        for watcher in &agent.running {
            if watcher.mode != WatcherMode::State {
                continue;
            }
            let Some(Some(compiled)) = compiled.get(&watcher.id) else {
                continue;
            };
            if let Some(payload) = evaluate_state(session, id, watcher, compiled, &logical, at) {
                matches.push(payload);
            }
        }

        if matches.is_empty() {
            return (None, false);
        }
        (
            Some(WatcherMatchBatch {
                session_id: id.to_string(),
                matches,
            }),
            false,
        )
    }

    /// Read a session's frame, re-resolving its backend once when the read says the route may
    /// have changed. The re-resolution is the only thing on this path that touches
    /// `PtyManager`, and it does not run on a healthy tick.
    fn read_frame(&self, id: Uuid, session: &mut RegisteredSession) -> ScreenRowsSince {
        let first = match &session.reader {
            Some(reader) => reader.read(id, session.stamp),
            None => ScreenRowsSince::Missing,
        };
        match first {
            ScreenRowsSince::Missing | ScreenRowsSince::Gone => {
                match self.backends.reader_for(id) {
                    Some(reader) => {
                        let again = reader.read(id, session.stamp);
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
}

/// `state` mode over one frame: the LOWEST matching logical row wins, mirroring
/// `rows::extract` (`context_scrape/rows.rs:19-26`), because a statusline always sits below
/// the transcript.
///
/// A transition to "no match" CLEARS the gate and emits nothing. The only consumer here is an
/// activity log, and "the prompt disappeared" is not a log entry; clearing is what lets an
/// identical re-appearance emit again, and it is why the payload needs no `present` field.
fn evaluate_state(
    session: &mut RegisteredSession,
    id: Uuid,
    watcher: &ResolvedWatcher,
    compiled: &pattern::WatcherPattern,
    logical: &[frame::LogicalRow],
    at: chrono::DateTime<chrono::Utc>,
) -> Option<WatcherMatchPayload> {
    let gate = session.gates.entry(watcher.id.clone()).or_default();
    let signature = (watcher.pattern.clone(), watcher.mode);
    if gate.signature.as_ref() != Some(&signature) {
        *gate = StateGate {
            signature: Some(signature),
            ..StateGate::default()
        };
    }

    let regex = compiled.regex();
    let mut count = 0usize;
    let mut lowest: Option<&frame::LogicalRow> = None;
    for row in logical {
        if regex.is_match(&row.text) {
            count += 1;
            lowest = Some(row);
        }
    }

    if count > gate.last_match_count {
        gate.generation += 1;
    }
    gate.last_match_count = count;

    let Some(lowest) = lowest else {
        gate.last = None;
        return None;
    };

    let captures = regex
        .captures(&lowest.text)
        .map(|found| {
            found
                .iter()
                .skip(1)
                .map(|group| group.map(|m| m.as_str().to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let (row, row_truncated) = truncate_row(&lowest.text);

    let candidate = (captures, row, gate.generation);
    if gate.last.as_ref() == Some(&candidate) {
        return None;
    }
    gate.last = Some(candidate.clone());

    session.seq += 1;
    Some(WatcherMatchPayload {
        session_id: id.to_string(),
        seq: session.seq,
        watcher_id: watcher.id.clone(),
        mode: watcher.mode,
        at,
        captures: candidate.0,
        row: candidate.1,
        row_truncated,
    })
}

#[cfg(test)]
mod engine_tests {
    use super::*;
    use std::collections::VecDeque;

    /// What the scripted backend paints on the next read.
    enum Painted {
        Frame(Vec<String>, Vec<bool>),
        Unchanged,
        Missing,
        Gone,
    }

    /// The rows of a frame with their wrap flags: what a repaint replays.
    type PaintedRows = (Vec<String>, Vec<bool>);

    /// A `PtyBackend` whose only real method is the watcher seam. Every other method is a
    /// stub, which is exactly the point: the engine never calls one.
    #[derive(Default)]
    struct ScriptedBackend {
        script: Mutex<HashMap<Uuid, VecDeque<Painted>>>,
        last: Mutex<HashMap<Uuid, PaintedRows>>,
        sequence: Mutex<HashMap<Uuid, u64>>,
        reads: AtomicUsize,
    }

    impl ScriptedBackend {
        fn paint(&self, id: Uuid, rows: &[&str]) {
            self.push(
                id,
                Painted::Frame(
                    rows.iter().map(|r| r.to_string()).collect(),
                    vec![false; rows.len()],
                ),
            );
        }

        fn push(&self, id: Uuid, painted: Painted) {
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

        fn screen_rows_since(&self, id: Uuid, _seen: Option<FrameStamp>) -> ScreenRowsSince {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let next = self
                .script
                .lock()
                .unwrap()
                .get_mut(&id)
                .and_then(|queue| queue.pop_front());
            let (rows, wrapped) = match next {
                Some(Painted::Missing) => return ScreenRowsSince::Missing,
                Some(Painted::Gone) => return ScreenRowsSince::Gone,
                Some(Painted::Unchanged) => return ScreenRowsSince::Unchanged,
                Some(Painted::Frame(rows, wrapped)) => {
                    self.last
                        .lock()
                        .unwrap()
                        .insert(id, (rows.clone(), wrapped.clone()));
                    (rows, wrapped)
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
                cursor_row: rows.len().saturating_sub(1) as u16,
                stamp: Some(FrameStamp {
                    sequence: *sequence,
                    rows: rows.len() as u16,
                    cols: 120,
                }),
            })
        }
    }

    struct FakeBackends {
        backend: Arc<ScriptedBackend>,
        liveness: Mutex<HashMap<Uuid, ContextSessionLiveness>>,
        resolutions: AtomicUsize,
        liveness_calls: AtomicUsize,
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
            self.liveness
                .lock()
                .unwrap()
                .get(&id)
                .copied()
                .unwrap_or(ContextSessionLiveness::Live)
        }
    }

    #[derive(Default)]
    struct FakePatterns(Mutex<HashMap<String, AgentResolution>>);

    impl WatcherPatternSource for FakePatterns {
        fn resolve(&self) -> BoxFuture<'_, HashMap<String, AgentResolution>> {
            Box::pin(async move { self.0.lock().unwrap().clone() })
        }
    }

    #[derive(Default)]
    struct FakeSink(Mutex<Vec<WatcherMatchBatch>>);

    impl WatcherEventSink for FakeSink {
        fn emit(&self, batch: WatcherMatchBatch) {
            self.0.lock().unwrap().push(batch);
        }
    }

    struct Harness {
        engine: Arc<WatcherEngine>,
        backend: Arc<ScriptedBackend>,
        backends: Arc<FakeBackends>,
        patterns: Arc<FakePatterns>,
        sink: Arc<FakeSink>,
    }

    impl Harness {
        fn new() -> Self {
            let backend = Arc::new(ScriptedBackend::default());
            let backends = Arc::new(FakeBackends {
                backend: Arc::clone(&backend),
                liveness: Mutex::new(HashMap::new()),
                resolutions: AtomicUsize::new(0),
                liveness_calls: AtomicUsize::new(0),
            });
            let patterns = Arc::new(FakePatterns::default());
            let sink = Arc::new(FakeSink::default());
            Self {
                engine: WatcherEngine::new(
                    Arc::clone(&backends) as Arc<dyn WatcherBackendSource>,
                    Arc::clone(&patterns) as Arc<dyn WatcherPatternSource>,
                    Arc::clone(&sink) as Arc<dyn WatcherEventSink>,
                ),
                backend,
                backends,
                patterns,
                sink,
            }
        }

        fn configure(&self, agent_id: &str, watchers: Vec<ResolvedWatcher>) {
            self.patterns.0.lock().unwrap().insert(
                agent_id.to_string(),
                AgentResolution {
                    running: watchers.into_iter().map(Arc::new).collect(),
                    over_budget: Vec::new(),
                },
            );
        }

        async fn ticks(&self, count: usize) {
            for _ in 0..count {
                self.engine.tick().await;
            }
        }

        fn emitted(&self) -> Vec<WatcherMatchPayload> {
            self.sink
                .0
                .lock()
                .unwrap()
                .iter()
                .flat_map(|batch| batch.matches.clone())
                .collect()
        }

        fn batches(&self) -> usize {
            self.sink.0.lock().unwrap().len()
        }
    }

    fn state_watcher(id: &str, pattern: &str) -> ResolvedWatcher {
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
        harness.engine.register_session(id, "a1".to_string());

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

    /// 9.3.27 - a state reading is idempotent. Five ticks of a screen that keeps being
    /// repainted with the same content produce ONE event.
    #[tokio::test]
    async fn a_state_watcher_emits_once_for_a_value_that_does_not_change() {
        let harness = Harness::new();
        let id = Uuid::new_v4();
        harness.engine.register_session(id, "a1".to_string());
        harness.configure("a1", vec![state_watcher("ctx", r"Context (\d+)%")]);
        harness.backend.paint(id, &["idle", "Context 42%"]);

        harness.ticks(5).await;

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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(watched, "a1".to_string());
        harness.engine.register_session(ignored, "a2".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
        harness.engine.register_session(id, "a1".to_string());
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
}

#[cfg(test)]
mod resolution_tests {
    use super::*;
    use crate::config::settings::WatcherConfig;

    fn agent(id: &str, command: &str) -> WatcherAgent {
        WatcherAgent {
            id: id.to_string(),
            command: command.to_string(),
        }
    }

    fn watcher(commands: Option<&[&str]>) -> WatcherEntry {
        WatcherEntry::Valid(WatcherConfig {
            enabled: true,
            mode: WatcherMode::Occurrence,
            pattern: "x".to_string(),
            commands: commands.map(|list| list.iter().map(|s| s.to_string()).collect()),
            dedupe: WatcherDedupe::Row,
            dedupe_window_ms: 2000,
            captured_against: None,
        })
    }

    fn map(entries: Vec<(&str, WatcherEntry)>) -> BTreeMap<String, WatcherEntry> {
        entries
            .into_iter()
            .map(|(id, entry)| (id.to_string(), entry))
            .collect()
    }

    fn running_ids(resolved: &HashMap<String, AgentResolution>, agent_id: &str) -> Vec<String> {
        resolved
            .get(agent_id)
            .map(|entry| entry.running.iter().map(|w| w.id.clone()).collect())
            .unwrap_or_default()
    }

    /// 9.2.10 - the configuration the issue says cannot be expressed today: one watcher, every
    /// agent. And its exact opposite, which only exists because `commands` is an `Option`.
    #[test]
    fn commands_absent_reaches_every_agent_and_an_empty_list_reaches_none() {
        let agents = [agent("a1", "claude"), agent("a2", "codex")];

        let (all, _) = resolve_watchers(&agents, &map(vec![("w", watcher(None))]));
        assert_eq!(running_ids(&all, "a1"), vec!["w"]);
        assert_eq!(running_ids(&all, "a2"), vec!["w"]);

        let (none, _) = resolve_watchers(&agents, &map(vec![("w", watcher(Some(&[])))]));
        assert!(
            none.is_empty(),
            "`[]` is the opposite of absent and must reach nobody"
        );
    }

    /// 9.2.11 and 9.2.12 - the stem rule, in full. EXACT equality on the executable stem,
    /// never a prefix: the catalog rejects `starts_with` in writing because `pi` and `agent`
    /// false-match under it (`coding_agents_catalog.rs:494-497`), and this is the rule that
    /// must not be re-derived anywhere else, in Rust or in TypeScript.
    #[test]
    fn a_commands_selector_matches_the_executable_stem_exactly() {
        let agents = [
            agent("plain", "claude"),
            agent("shouting", "CLAUDE.EXE"),
            agent("full-path", r"C:\Users\x\claude-sandbox-runtime\claude.cmd"),
            agent("pi", "pi --provider claude"),
            agent("through-cmd", "cmd /c claude"),
            agent("through-npx", "npx claude"),
            agent("prefix", "claude-phi"),
        ];

        let (resolved, _) =
            resolve_watchers(&agents, &map(vec![("w", watcher(Some(&["claude"])))]));
        let mut reached: Vec<&String> = resolved.keys().collect();
        reached.sort();
        assert_eq!(reached, vec!["full-path", "plain", "shouting"]);
    }

    /// 9.2.13 - disabled is a state the user chose. It reaches nobody and keeps every byte of
    /// its configuration, so re-enabling it is one flag and not a re-entry.
    #[test]
    fn a_disabled_watcher_reaches_nobody_and_keeps_its_configuration() {
        let mut entry = watcher(None);
        if let WatcherEntry::Valid(config) = &mut entry {
            config.enabled = false;
        }
        let watchers = map(vec![("w", entry)]);

        let (resolved, notices) = resolve_watchers(&[agent("a1", "claude")], &watchers);
        assert!(resolved.is_empty());
        assert!(
            notices.is_empty(),
            "disabled is not a problem and must not produce a log line"
        );
        assert_eq!(watchers["w"].valid().expect("still valid").pattern, "x");
    }

    /// 9.2.14 - a selector entry that is not a command skips the WHOLE watcher. The failure
    /// direction matters: "reaches nobody" loses a detection, "reaches everybody" would run a
    /// user's pattern on agents they never selected.
    #[test]
    fn a_selector_entry_that_does_not_tokenize_skips_the_whole_watcher() {
        let agents = [agent("a1", "claude"), agent("a2", "codex")];
        let watchers = map(vec![("w", watcher(Some(&["claude", ""])))]);

        let (resolved, notices) = resolve_watchers(&agents, &watchers);
        assert!(
            resolved.is_empty(),
            "not even the readable half of the selector may reach anything"
        );
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].subject, "commands:w");

        // ...and the notice is written once, however many ticks resolve the same thing.
        let log = ResolutionLog::default();
        assert_eq!(log.publish(notices.clone()), 1);
        for _ in 0..10 {
            assert_eq!(log.publish(notices.clone()), 0);
        }
        assert_eq!(log.remembered(), 1, "one subject, not one per tick");
    }

    /// 9.2.15 - an agent whose own command does not tokenize has no stem to compare against.
    /// Selectorless watchers still reach it; no watcher WITH a selector does, because the
    /// alternative would be inventing a stem and matching on the guess.
    #[test]
    fn an_agent_whose_command_does_not_tokenize_is_reached_only_without_a_selector() {
        let agents = [agent("broken", "\"unterminated")];
        let watchers = map(vec![
            ("selectorless", watcher(None)),
            ("selected", watcher(Some(&["claude"]))),
        ]);

        let (resolved, _) = resolve_watchers(&agents, &watchers);
        assert_eq!(running_ids(&resolved, "broken"), vec!["selectorless"]);
    }

    /// 9.2.16 - the budget, and the fact that it resolves by alphabetical id order. Declared
    /// debt (10.8), surfaced rather than fixed: the dropped ids come back in `over_budget` so
    /// Settings can show "not running on <agent> (budget)" per row.
    #[test]
    fn only_the_first_eight_watchers_in_key_order_run_on_one_agent() {
        let entries: Vec<(String, WatcherEntry)> = (1..=12)
            .map(|i| (format!("w{i:02}"), watcher(None)))
            .collect();
        let watchers: BTreeMap<String, WatcherEntry> = entries.into_iter().collect();

        let (resolved, notices) = resolve_watchers(&[agent("a1", "claude")], &watchers);
        let entry = resolved.get("a1").expect("reached");
        assert_eq!(
            entry
                .running
                .iter()
                .map(|w| w.id.as_str())
                .collect::<Vec<_>>(),
            vec!["w01", "w02", "w03", "w04", "w05", "w06", "w07", "w08"]
        );
        assert_eq!(entry.over_budget, vec!["w09", "w10", "w11", "w12"]);

        assert_eq!(notices.len(), 1, "the four dropped ids are ONE line");
        assert!(notices[0].message.contains("w09,w10,w11,w12"));
        let log = ResolutionLog::default();
        assert_eq!(log.publish(notices.clone()), 1);
        assert_eq!(log.publish(notices), 0);
    }

    /// 9.2.20 - the clamp, and its one log line. Without it a large window plus a `row` key
    /// grows the dedupe key set to every distinct row seen inside the window.
    #[test]
    fn a_dedupe_window_over_the_maximum_is_clamped_and_logged_once() {
        let mut entry = watcher(None);
        if let WatcherEntry::Valid(config) = &mut entry {
            config.dedupe_window_ms = 3_600_000;
        }

        let (resolved, notices) =
            resolve_watchers(&[agent("a1", "claude")], &map(vec![("w", entry)]));
        assert_eq!(
            resolved["a1"].running[0].dedupe_window_ms,
            MAX_DEDUPE_WINDOW_MS
        );
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].subject, "clamp:w");

        let log = ResolutionLog::default();
        assert_eq!(log.publish(notices.clone()), 1);
        assert_eq!(log.publish(notices), 0);
    }

    /// An entry that did not deserialize is skipped and logged once, and every OTHER watcher
    /// still resolves. The settings half of this is pinned in `settings.rs`; this is the
    /// resolution half.
    #[test]
    fn an_invalid_entry_is_skipped_and_logged_once_while_the_others_resolve() {
        let watchers = map(vec![
            (
                "bad",
                WatcherEntry::Invalid(serde_json::json!({ "mode": "State" })),
            ),
            ("good", watcher(None)),
        ]);

        let (resolved, notices) = resolve_watchers(&[agent("a1", "claude")], &watchers);
        assert_eq!(running_ids(&resolved, "a1"), vec!["good"]);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].subject, "invalid:bad");

        let log = ResolutionLog::default();
        assert_eq!(log.publish(notices.clone()), 1);
        assert_eq!(log.publish(notices), 0);
    }

    /// The log-once map is keyed by SUBJECT, so a user typing a selector in Settings cannot
    /// accumulate one entry per keystroke - and a CHANGED value still speaks again, because a
    /// second mistake is not the first one.
    #[test]
    fn the_log_once_map_holds_one_entry_per_subject_and_speaks_again_on_a_change() {
        let log = ResolutionLog::default();
        for attempt in 0..50 {
            let notice = ResolutionNotice {
                subject: "commands:w".to_string(),
                detail: format!("half-typed-{attempt}"),
                message: "…".to_string(),
            };
            assert_eq!(log.publish(vec![notice]), 1, "a changed value speaks again");
        }
        assert_eq!(log.remembered(), 1);
    }

    /// Two watchers reaching the same agent both run: there is no "most specific wins" rule,
    /// because it would silently discard a pattern the user configured.
    #[test]
    fn two_watchers_reaching_the_same_agent_both_run() {
        let watchers = map(vec![
            ("a", watcher(Some(&["claude"]))),
            ("b", watcher(None)),
        ]);

        let (resolved, _) = resolve_watchers(&[agent("a1", "claude")], &watchers);
        assert_eq!(running_ids(&resolved, "a1"), vec!["a", "b"]);
    }

    /// A stem no agent has is not an error, it just reaches nobody. Settings shows
    /// "reaches 0 agents" so the typo is visible where it was made.
    #[test]
    fn a_stem_no_agent_has_reaches_nobody_and_is_not_an_error() {
        let (resolved, notices) = resolve_watchers(
            &[agent("a1", "claude")],
            &map(vec![("w", watcher(Some(&["gemni"])))]),
        );
        assert!(resolved.is_empty());
        assert!(notices.is_empty());
    }
}

#[cfg(test)]
mod read_seam_tests {
    use super::*;
    use crate::pty::output::{PtyOutputTarget, SessionIoFanout};
    use crate::session::profile::IdleTuning;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    use uuid::Uuid;

    fn timing_session_id() -> Uuid {
        Uuid::new_v4()
    }

    fn fanout() -> SessionIoFanout {
        SessionIoFanout::new(
            Arc::new(Mutex::new(HashMap::new())),
            crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {}),
            None,
        )
    }

    fn feed(fanout: &SessionIoFanout, id: Uuid, chunk: &[u8]) {
        fanout.handle_output(
            &PtyOutputTarget::noop(),
            id,
            &id.to_string(),
            chunk.to_vec(),
        );
    }

    /// #1171, section 7.3 - the three numbers recorded in this module's doc comment.
    ///
    /// `#[ignore]`d so it never becomes a flaky timing gate, and kept in the tree so the
    /// numbers can be re-taken rather than guessed at again:
    ///
    /// ```text
    /// cargo test --config profile.dev.opt-level=2 --lib read_seam_timing -- --ignored --nocapture
    /// ```
    ///
    /// The changed-frame measurement feeds a chunk between reads, which is what a session
    /// with a live spinner does several times per second.
    #[test]
    #[ignore]
    fn read_seam_timing() {
        const ITERATIONS: u32 = 2_000;

        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        for row in 0..30u16 {
            feed(
                &fanout,
                id,
                format!(
                    "\x1b[{};1Hrow {row} of a coding agent's screen, wide enough to be real\r\n",
                    row + 1
                )
                .as_bytes(),
            );
        }

        let start = Instant::now();
        for _ in 0..ITERATIONS {
            std::hint::black_box(fanout.get_screen_rows(id));
        }
        let full = start.elapsed() / ITERATIONS;

        let seen = match fanout.get_screen_rows_since(id, None) {
            ScreenRowsSince::Frame(frame) => frame.stamp,
            other => panic!(
                "expected a frame, got {}",
                match other {
                    ScreenRowsSince::Unchanged => "Unchanged",
                    ScreenRowsSince::Missing => "Missing",
                    ScreenRowsSince::Gone => "Gone",
                    ScreenRowsSince::Frame(_) => unreachable!(),
                }
            ),
        };

        let start = Instant::now();
        for _ in 0..ITERATIONS {
            std::hint::black_box(fanout.get_screen_rows_since(id, seen));
        }
        let unchanged = start.elapsed() / ITERATIONS;

        // The chunk that makes the frame change is fed OUTSIDE the timed region: what is
        // being measured is the read, not `handle_output`.
        let mut changed_total = std::time::Duration::ZERO;
        for i in 0..ITERATIONS {
            feed(&fanout, id, format!("\x1b[30;1Hspinner {i}").as_bytes());
            let start = Instant::now();
            std::hint::black_box(fanout.get_screen_rows_since(id, seen));
            changed_total += start.elapsed();
        }
        let changed = changed_total / ITERATIONS;

        println!("[#1171] get_screen_rows:                 {full:?}");
        println!("[#1171] get_screen_rows_since UNCHANGED: {unchanged:?}");
        println!("[#1171] get_screen_rows_since CHANGED:   {changed:?}");
    }

    /// 9.1.1 - an unchanged frame short-circuits, and the variant it returns cannot carry
    /// rows even if someone later wanted it to. This is the acceptance criterion for
    /// contention, enforced by the type rather than by a timing assertion (7.3).
    #[test]
    fn an_unchanged_frame_returns_unchanged_and_carries_no_rows() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        feed(&fanout, id, b"hello");

        let seen = fanout
            .get_screen_rows_since(id, None)
            .frame()
            .and_then(|frame| frame.stamp)
            .expect("first read must be a frame carrying a stamp");

        let again = fanout.get_screen_rows_since(id, Some(seen));
        assert!(matches!(again, ScreenRowsSince::Unchanged));
        assert!(
            again.frame().is_none(),
            "Unchanged must carry no frame and therefore no rows"
        );
    }

    /// 9.1.2 - one `handle_output` chunk moves `sequence`, so the next read is a frame.
    #[test]
    fn one_output_chunk_makes_the_next_read_a_frame() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        feed(&fanout, id, b"first");

        let seen = fanout
            .get_screen_rows_since(id, None)
            .frame()
            .and_then(|frame| frame.stamp)
            .expect("first read must be a frame");
        assert!(matches!(
            fanout.get_screen_rows_since(id, Some(seen)),
            ScreenRowsSince::Unchanged
        ));

        feed(&fanout, id, b" second");
        let next = fanout.get_screen_rows_since(id, Some(seen));
        let frame = next.frame().expect("a new chunk must produce a frame");
        assert_eq!(frame.stamp.unwrap().sequence, seen.sequence + 1);
    }

    /// 9.1.3 - **the regression a sequence-only stamp would let through.**
    ///
    /// `resize_screen_and_broadcast` reflows the grid and does NOT bump `output_sequence`
    /// (`output.rs:202-212`). With the size out of the stamp this read would return
    /// `Unchanged` over a screen that was just re-laid at a different width.
    #[test]
    fn a_resize_that_does_not_move_the_sequence_still_returns_a_frame() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        feed(&fanout, id, b"content");

        let seen = fanout
            .get_screen_rows_since(id, None)
            .frame()
            .and_then(|frame| frame.stamp)
            .expect("first read must be a frame");

        fanout.resize_screen_and_broadcast(id, 100, 24);

        let after = fanout.get_screen_rows_since(id, Some(seen));
        let frame = after
            .frame()
            .expect("a reflow must not be reported as Unchanged");
        let stamp = frame.stamp.unwrap();
        assert_eq!(
            stamp.sequence, seen.sequence,
            "the resize must not have moved the sequence, or this test proves nothing"
        );
        assert_eq!((stamp.rows, stamp.cols), (24, 100));
    }

    /// 9.1.5 - a poisoned `screen_parsers` is `Missing`, never `Unchanged`. Reporting
    /// `Unchanged` would claim the screen is the one the caller last saw, which is precisely
    /// what a lock we could not take cannot say.
    #[test]
    fn a_poisoned_parser_map_is_missing_and_not_unchanged() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        feed(&fanout, id, b"content");

        let seen = fanout
            .get_screen_rows_since(id, None)
            .frame()
            .and_then(|frame| frame.stamp)
            .expect("first read must be a frame");

        fanout.poison_screen_parsers_for_test();

        assert!(matches!(
            fanout.get_screen_rows_since(id, Some(seen)),
            ScreenRowsSince::Missing
        ));
    }

    /// 9.1.6 - the changed-frame read returns exactly what `get_screen_rows` returns, row
    /// for row. The seam is a cheaper way to ask the same question, not a different one.
    #[test]
    fn a_changed_frame_returns_the_same_rows_as_get_screen_rows() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 30, 120);
        feed(
            &fanout,
            id,
            b"alpha\r\nbeta\r\ngamma with rather more text on it\r\n",
        );

        let expected = fanout.get_screen_rows(id).expect("rows");
        let frame = fanout
            .get_screen_rows_since(id, None)
            .frame()
            .map(|frame| frame.rows.clone())
            .expect("frame");
        assert_eq!(frame, expected);
    }

    /// 9.1.7 - `wrapped` and `cursor_row` are the parser's, for every row, and `wrapped` is
    /// the same length as `rows` so the frame diff can index them together without a bound
    /// check of its own.
    #[test]
    fn wrapped_and_cursor_row_mirror_the_parser() {
        let fanout = fanout();
        let id = timing_session_id();
        fanout.register_session(id, IdleTuning::DEFAULT, 4, 10);
        // 14 chars into a 10-column grid: row 0 wraps into row 1.
        feed(&fanout, id, b"0123456789abcd");

        let read = fanout.get_screen_rows_since(id, None);
        let frame = read.frame().expect("frame");
        assert_eq!(frame.rows.len(), frame.wrapped.len());
        assert_eq!(frame.wrapped, vec![true, false, false, false]);
        assert_eq!(frame.cursor_row, 1);
    }

    /// A session the fanout never registered is `Missing` at the fanout boundary: the fanout
    /// knows nothing about children, so it can make no claim about the session (`output.rs:287-294`).
    #[test]
    fn an_unregistered_session_is_missing_at_the_fanout() {
        let fanout = fanout();
        assert!(matches!(
            fanout.get_screen_rows_since(timing_session_id(), None),
            ScreenRowsSince::Missing
        ));
    }

    /// 9.1.8 - the default trait implementation never reports `Unchanged` and never invents a
    /// stamp, whatever it is handed as `seen`.
    #[test]
    fn the_default_mapping_never_reports_unchanged_and_reports_no_stamp() {
        use crate::pty::context_scrape::ScreenRowsRead;

        let mapped = frame_from_screen_rows_read(ScreenRowsRead::Rows(vec![
            "one".to_string(),
            "two".to_string(),
        ]));
        let frame = mapped.frame().expect("rows must map to a frame");
        assert!(frame.stamp.is_none());
        assert_eq!(frame.wrapped, vec![false, false]);
        assert_eq!(frame.cursor_row, 0);

        assert!(matches!(
            frame_from_screen_rows_read(ScreenRowsRead::Unavailable),
            ScreenRowsSince::Missing
        ));
        assert!(matches!(
            frame_from_screen_rows_read(ScreenRowsRead::SessionOver),
            ScreenRowsSince::Missing
        ));
    }
}
