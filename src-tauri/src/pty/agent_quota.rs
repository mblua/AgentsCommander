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

pub(crate) mod source;

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
    fn sources(&self) -> BoxFuture<'_, Option<HashMap<String, AgentQuotaSource>>>;
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

    #[cfg(test)]
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

        let Some(sources) = self.sources.sources().await else {
            for (id, _) in ids {
                if self.rows.get_session_liveness(id) == ContextSessionLiveness::SessionOver {
                    self.retire_session(id);
                }
            }
            return;
        };
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
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
