//! Narrow adapters that wire the scraper, quota and watcher engines to PTYs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::settings::SettingsState;
use crate::pty::agent_quota::source::SourceSpec;
use crate::pty::agent_quota::{
    AgentQuotaPayload, AgentQuotaSource, QuotaEventSink, QuotaRowsSource, QuotaSourceProvider,
};
use crate::pty::context_scrape::{
    ContextEventSink, ContextPatternSource, ContextPersistSink, ContextSample, ContextSampleSink,
    ContextSessionLiveness, ContextUsagePayload, ScreenRowsRead, ScreenRowsSource,
};
use crate::pty::idle_detector::IdleDetector;
use crate::pty::manager::PtyManager;
use crate::pty::watchers::{
    ActivityConsumer, ActivityFactory, ActivityObservation, SessionFrameReader,
    WatcherBackendSource, WatcherEventSink, WatcherPatternSource,
};
use crate::session::manager::SessionManager;
use tauri::{Emitter, Manager};

use crate::{commands, config};

// ---- #1032/#1056: the four narrow scrape adapters ---------------------------------
//
// This is the capability boundary. A sample may enqueue an informational coordinator
// notice, but the scraper itself cannot route, inject, wake, or remediate a session.

/// Rows, via the routed backend. The three states come from the backend, which is the only
/// thing that holds a liveness oracle.
pub(crate) struct ScraperRows {
    pub(crate) pty_mgr: Arc<Mutex<PtyManager>>,
    /// A poisoned `PtyManager` is app-wide and permanent, so the warning is worth exactly
    /// one line, not one per configured session every 5 seconds.
    pub(crate) poison_logged: AtomicBool,
}

impl ScreenRowsSource for ScraperRows {
    fn get_screen_rows(&self, id: uuid::Uuid) -> ScreenRowsRead {
        match self.pty_mgr.lock() {
            Ok(mgr) => mgr.get_screen_rows(id),
            Err(_) => {
                if !self
                    .poison_logged
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    log::warn!(
                        "[context] PtyManager lock is poisoned; context readings are unavailable"
                    );
                }
                ScreenRowsRead::Unavailable
            }
        }
    }

    fn get_session_liveness(&self, id: uuid::Uuid) -> ContextSessionLiveness {
        match self.pty_mgr.lock() {
            Ok(mgr) => mgr.context_session_liveness(id),
            Err(_) => {
                if !self
                    .poison_logged
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    log::warn!(
                        "[context] PtyManager lock is poisoned; context liveness is unavailable"
                    );
                }
                ContextSessionLiveness::Unavailable
            }
        }
    }
}

/// Every agent's configured pattern string, read fresh from settings each tick. One
/// `RwLock` read per tick for all sessions, not one per session.
pub(crate) struct ScraperPatterns {
    pub(crate) settings: SettingsState,
}

impl ContextPatternSource for ScraperPatterns {
    fn patterns(&self) -> futures::future::BoxFuture<'_, Option<HashMap<String, String>>> {
        Box::pin(async move {
            let settings = match self.settings.current_read().await {
                Ok(settings) => settings,
                Err(error) => {
                    log::debug!("[context] settings unavailable: {error}");
                    return None;
                }
            };
            Some(
                settings
                    .agents
                    .iter()
                    .filter_map(|agent| {
                        let regex = agent.context_regex.as_deref()?;
                        // A blank field is the field being blank, and skipping it here is a
                        // LOG-HYGIENE choice and nothing more: `pattern::compile` already
                        // refuses "" and "   " for having no capture group 1, so this can never
                        // become a pattern that matches everything - it would only warn on every
                        // change while a user is still typing.
                        //
                        // Note what is trimmed and what is not: the emptiness TEST looks at a
                        // trimmed view, the VALUE handed over is the user's string, byte for
                        // byte. The pattern is the only defence this feature has - the engine
                        // ships no anchoring rules of its own - so editing it can only weaken
                        // it. Trimming would eat the leading spaces of `  Context ...`, which
                        // ARE the column-2 anchor, and the reading would fail open.
                        (!regex.trim().is_empty()).then(|| (agent.id.clone(), regex.to_string()))
                    })
                    .collect(),
            )
        })
    }
}

/// The sink. `PtyOutputTarget` (`output.rs`) already wraps an `AppHandle` behind a plain
/// `Fn` for the same reason.
pub(crate) struct ScraperSink {
    pub(crate) app_handle: tauri::AppHandle,
}

impl ContextEventSink for ScraperSink {
    fn emit(&self, payload: ContextUsagePayload) {
        let _ = self.app_handle.emit("session_context", payload);
    }
}

// ---- #2482: the three quota adapters ----------------------------------------------
//
// Each is its `context_scrape` twin above, narrowed. They live here, and only here, because
// this is the file allowed to touch both the engine and the settings / session / Tauri world;
// `pty::agent_quota` names none of those, which keeps it out of the cyclic SCC.

/// Rows and liveness, via the routed backend. `ScraperRows` in shape.
pub(crate) struct QuotaRows {
    pub(crate) pty_mgr: Arc<Mutex<PtyManager>>,
    /// A poisoned `PtyManager` is app-wide and permanent: one line, not one per tick.
    pub(crate) poison_logged: AtomicBool,
}

impl QuotaRowsSource for QuotaRows {
    fn get_screen_rows(&self, id: uuid::Uuid) -> ScreenRowsRead {
        match self.pty_mgr.lock() {
            Ok(mgr) => mgr.get_screen_rows(id),
            Err(_) => {
                if !self
                    .poison_logged
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    log::warn!(
                        "[quota] PtyManager lock is poisoned; quota readings are unavailable"
                    );
                }
                ScreenRowsRead::Unavailable
            }
        }
    }

    /// `Unavailable` and never `SessionOver` on a poisoned lock: that lock is app-wide, and
    /// `SessionOver` would deregister every session for life.
    fn get_session_liveness(&self, id: uuid::Uuid) -> ContextSessionLiveness {
        match self.pty_mgr.lock() {
            Ok(mgr) => mgr.context_session_liveness(id),
            Err(_) => {
                if !self
                    .poison_logged
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    log::warn!(
                        "[quota] PtyManager lock is poisoned; quota liveness is unavailable"
                    );
                }
                ContextSessionLiveness::Unavailable
            }
        }
    }
}

/// Every agent's quota source and ACCOUNT key, read fresh from settings each tick. One
/// `RwLock` read per tick for all sessions. Maps the settings type onto the engine's
/// settings-free `AgentQuotaSource`.
///
/// **The `match` in `sources_map` is the multi-agent extension point's second and last site.** A
/// new
/// source kind adds one arm in `pty::agent_quota::source` and one arm here. There is no
/// registry to go looking for.
pub(crate) struct QuotaSources {
    pub(crate) settings: SettingsState,
}

impl QuotaSources {
    /// #2566 D3 - membership is by COMMAND, and this is the only place it is decided. Every
    /// agent whose command yields an account key with at least one enabled, valid source
    /// behind it gets an entry: its OWN pattern when its own entry is enabled, otherwise the
    /// key winner's, the first enabled entry in `quota_sources` order (a `BTreeMap`, so the
    /// LOWEST agent id). `enabled: false` withdraws a pattern, never an agent.
    pub(crate) fn sources_map(
        settings: &config::settings::AppSettings,
    ) -> HashMap<String, AgentQuotaSource> {
        use config::settings::QuotaSourceConfig;

        // Pass 1 - own specs.
        let own: HashMap<String, SourceSpec> = settings
            .quota_sources
            .iter()
            .filter_map(|(agent_id, entry)| {
                // Every kind shares one filter; only the spec it builds differs.
                let (pattern, enabled, build): (&String, bool, fn(String) -> SourceSpec) =
                    match entry.valid()? {
                        QuotaSourceConfig::ScreenRegex { pattern, enabled } => {
                            (pattern, *enabled, |pattern| SourceSpec::ScreenRegex {
                                pattern,
                            })
                        }
                        QuotaSourceConfig::ScreenRegexRemaining { pattern, enabled } => {
                            (pattern, *enabled, |pattern| {
                                SourceSpec::ScreenRegexRemaining { pattern }
                            })
                        }
                    };
                if !enabled {
                    return None;
                }
                // Emptiness is tested on a TRIMMED view; the value handed over is the
                // user's string byte for byte. `ScraperPatterns` documents why: leading
                // spaces ARE the column anchor, and trimming makes the reading fail OPEN.
                (!pattern.trim().is_empty()).then(|| (agent_id.clone(), build(pattern.clone())))
            })
            .collect();

        // Pass 2 - keys. No key, no entry, ever.
        let keys: Vec<(&String, String)> = settings
            .agents
            .iter()
            .filter_map(|agent| {
                config::coding_agents_catalog::command_account_key(&agent.command)
                    .map(|key| (&agent.id, key))
            })
            .collect();

        // Pass 3 - key winner: first in `quota_sources` order wins.
        let mut by_key: std::collections::BTreeMap<&str, &SourceSpec> =
            std::collections::BTreeMap::new();
        for agent_id in settings.quota_sources.keys() {
            let Some(spec) = own.get(agent_id) else {
                continue;
            };
            if let Some((_, key)) = keys.iter().find(|(id, _)| *id == agent_id) {
                by_key.entry(key.as_str()).or_insert(spec);
            }
        }

        // Pass 4 - emit; an agent with no spec anywhere behind its command is skipped.
        keys.iter()
            .filter_map(|(agent_id, key)| {
                let spec = own
                    .get(*agent_id)
                    .or_else(|| by_key.get(key.as_str()).copied())?;
                Some((
                    (*agent_id).clone(),
                    AgentQuotaSource {
                        account_key: key.clone(),
                        spec: spec.clone(),
                    },
                ))
            })
            .collect()
    }
}

impl QuotaSourceProvider for QuotaSources {
    fn sources(&self) -> futures::future::BoxFuture<'_, Option<HashMap<String, AgentQuotaSource>>> {
        Box::pin(async move {
            let settings = match self.settings.current_read().await {
                Ok(settings) => settings,
                Err(error) => {
                    log::debug!("[agent_quota] settings unavailable: {error}");
                    return None;
                }
            };
            Some(Self::sources_map(&settings))
        })
    }
}

/// The sink. Emitted UNSCOPED, exactly like `session_context`, so a detached terminal window
/// receives it too.
pub(crate) struct QuotaSink {
    pub(crate) app_handle: tauri::AppHandle,
}

impl QuotaEventSink for QuotaSink {
    fn emit(&self, payload: AgentQuotaPayload) {
        let _ = self.app_handle.emit("agent_quota", payload);
    }
}

/// Nonblocking, bounded bridge from the scraper thread to the alert actor. It deliberately
/// carries no app, PTY, session, filesystem, or delivery capability.
pub(crate) struct ScraperSamples {
    pub(crate) sender: tokio::sync::mpsc::Sender<ContextSample>,
    pub(crate) closed_logged: AtomicBool,
    pub(crate) saturated: AtomicBool,
    pub(crate) dropped: AtomicU64,
}

impl ContextSampleSink for ScraperSamples {
    fn observe(&self, sample: ContextSample) {
        match self.sender.try_send(sample) {
            Ok(()) => {
                let recovery_capacity =
                    crate::session::context_alerts::CONTEXT_SAMPLE_QUEUE_CAPACITY / 4;
                if self.saturated.load(Ordering::Relaxed)
                    && self.sender.capacity() >= recovery_capacity
                    && self
                        .saturated
                        .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    let dropped = self.dropped.swap(0, Ordering::Relaxed);
                    log::info!(
                        "[context-alert] sample queue recovered remainingCapacity={} dropped={}",
                        self.sender.capacity(),
                        dropped
                    );
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                if !self.saturated.swap(true, Ordering::Relaxed) {
                    log::warn!(
                        "[context-alert] sample queue saturated; advisory samples are being \
                            dropped (dropped={})",
                        dropped
                    );
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                if !self.closed_logged.swap(true, Ordering::Relaxed) {
                    log::warn!(
                        "[context-alert] sample queue is closed; advisory samples are being dropped"
                    );
                }
            }
        }
    }
}

/// #1088 - the fifth sink's concrete impl. It owns the `SessionManager` handle
/// so the scraper never has to: `commit` writes each changed reading onto its
/// `Session` and triggers the same whole-file persist the idle/busy callbacks
/// already call. It holds no `AppHandle` and no `PtyManager`, so the scraper's
/// documented capability boundary is preserved.
pub(crate) struct ScraperPersist {
    pub(crate) session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
}

impl ContextPersistSink for ScraperPersist {
    fn commit(&self, changed: Vec<(uuid::Uuid, Option<u8>)>) -> futures::future::BoxFuture<'_, ()> {
        // Clone the Arc before the `async move` so the returned future is
        // 'static + Send (it captures the Arc, not `&self`).
        let mgr = Arc::clone(&self.session_mgr);
        Box::pin(async move {
            if changed.is_empty() {
                return;
            }
            // One outer read guard held across the per-session writes (interior
            // `state.write`) and the persist (interior `state.read`) - the exact
            // lock discipline the idle/busy callbacks use (`mark_idle` + persist
            // under one `session_mgr.read()`).
            let guard = mgr.read().await;
            for (id, percent) in &changed {
                guard.set_context_percent(*id, *percent).await;
            }
            crate::config::sessions_persistence::persist_current_state_prune_dormant(&guard).await;
        })
    }
}

/// #2879 - the watcher engine's internal activity factory, composed over the idle detector.
/// Each call is one attach (`activity_consumer`); the returned consumer only maps the engine's
/// observation onto that attach's own `observe`/`retire` and returns their verdict unchanged.
/// The engine never names the detector, and the detector never learns about the engine.
pub(crate) fn codex_activity_factory(detector: &Arc<IdleDetector>) -> ActivityFactory {
    let detector = Arc::clone(detector);
    Arc::new(move |id: uuid::Uuid| {
        let handle = detector.activity_consumer(id)?;
        let consumer: ActivityConsumer =
            Arc::new(move |observation: ActivityObservation| match observation {
                ActivityObservation::Present => (handle.observe)(Some(true)),
                ActivityObservation::Absent => (handle.observe)(Some(false)),
                ActivityObservation::Unavailable | ActivityObservation::Validate => {
                    (handle.observe)(None)
                }
                ActivityObservation::Retired => (handle.retire)(),
            });
        Some(consumer)
    })
}

// ---- #1171: the three narrow watcher adapters -------------------------------------------
//
// The same capability boundary the scrape adapters draw. The engine can read one session's
// screen through a narrowed reader, ask for its lightweight liveness, read the resolved
// watcher set, and hand a batch to a sink. It holds no `AppHandle` and no `PtyManager`.

/// The per-session frame reader and the lightweight liveness, via the routed backend.
///
/// `reader_for` is called ONCE per session at registration, and again only when a read comes
/// back `Missing` or `Gone`. That is what keeps the `PtyManager` mutex out of a 200 ms loop:
/// the tick calls the backend directly through the `Arc` this handed over.
pub(crate) struct WatcherBackends {
    pub(crate) pty_mgr: Arc<Mutex<PtyManager>>,
    /// A poisoned `PtyManager` is app-wide and permanent, so the warning is worth exactly one
    /// line, not one per registered session five times a second.
    pub(crate) poison_logged: AtomicBool,
}

impl WatcherBackends {
    fn warn_poisoned_once(&self, what: &str) {
        if !self
            .poison_logged
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            log::warn!("[watchers] PtyManager lock is poisoned; {what} is unavailable");
        }
    }
}

impl WatcherBackendSource for WatcherBackends {
    fn reader_for(&self, id: uuid::Uuid) -> Option<SessionFrameReader> {
        match self.pty_mgr.lock() {
            Ok(mgr) => {
                let kind = mgr.backend_kind(id)?;
                Some(SessionFrameReader::new(mgr.backend_for_kind(kind)))
            }
            Err(_) => {
                self.warn_poisoned_once("watcher sampling");
                None
            }
        }
    }

    fn liveness(&self, id: uuid::Uuid) -> ContextSessionLiveness {
        match self.pty_mgr.lock() {
            Ok(mgr) => mgr.context_session_liveness(id),
            Err(_) => {
                self.warn_poisoned_once("watcher liveness");
                ContextSessionLiveness::Unavailable
            }
        }
    }
}

/// The configured watchers crossed with the configured agents, read fresh each tick. One
/// `RwLock` read per tick for all sessions, not one per session.
pub(crate) struct WatcherPatterns {
    pub(crate) settings: SettingsState,
    /// Log-once bookkeeping for the resolution notices, so a configuration that stays wrong
    /// costs one line rather than five per second.
    pub(crate) log: crate::pty::watchers::ResolutionLog,
}

impl WatcherPatternSource for WatcherPatterns {
    fn resolve(
        &self,
    ) -> futures::future::BoxFuture<
        '_,
        Option<HashMap<String, crate::pty::watchers::AgentResolution>>,
    > {
        Box::pin(async move {
            let settings = match self.settings.current_read().await {
                Ok(settings) => settings,
                Err(error) => {
                    log::debug!("[watchers] settings unavailable: {error}");
                    return None;
                }
            };

            // The agent list is cloned only when at least one watcher could possibly use it.
            // Otherwise this ran five times a second, forever, in every installation that
            // never touches the feature: at twelve agents that is 24 `String` allocations per
            // tick, 120 a second, for an answer that is always empty.
            //
            // Resolution is still CALLED with an empty agent slice rather than skipped, so a
            // malformed or unreadable watcher entry keeps producing its one log line even when
            // nothing is enabled.
            let has_enabled_watcher = settings
                .watchers
                .values()
                .any(|entry| entry.valid().is_some_and(|config| config.enabled));
            let agents: Vec<crate::pty::watchers::WatcherAgent> = if has_enabled_watcher {
                settings
                    .agents
                    .iter()
                    .map(|agent| crate::pty::watchers::WatcherAgent {
                        id: agent.id.clone(),
                        command: agent.command.clone(),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let (resolved, notices) =
                crate::pty::watchers::resolve_watchers(&agents, &settings.watchers);
            drop(settings);
            self.log.publish(notices);
            Some(resolved)
        })
    }
}

/// Delivery of one coalesced batch, behind two plain `Fn`s.
///
/// Mould: `PtyOutputTarget` (`output.rs:58-92`), which wraps an `AppHandle` the same way and
/// for the same reason - the DECISION is testable, an `AppHandle` is not.
///
/// **Directed, and silent when nobody is listening.** `app.emit` reaches every window, so at
/// 50 saturated sessions a broadcast would deliver thousands of payloads per second to four
/// windows and make every detached terminal pay to deserialize events it discards. The
/// activity window is closed most of the time, and this makes that case cost nothing at all.
#[derive(Clone)]
pub(crate) struct WatcherDelivery {
    pub(crate) window_present: Arc<dyn Fn() -> bool + Send + Sync>,
    pub(crate) emit: Arc<dyn Fn(crate::pty::watchers::WatcherMatchBatch) + Send + Sync>,
}

impl WatcherDelivery {
    pub(crate) fn to_watchers_window(app_handle: tauri::AppHandle) -> Self {
        let present = app_handle.clone();
        Self {
            window_present: Arc::new(move || {
                present
                    .get_webview_window(commands::window::WATCHERS_WINDOW_LABEL)
                    .is_some()
            }),
            emit: Arc::new(move |batch| {
                let _ = app_handle.emit_to(
                    commands::window::WATCHERS_WINDOW_LABEL,
                    "watcher_matches",
                    batch,
                );
            }),
        }
    }

    fn deliver(&self, batch: crate::pty::watchers::WatcherMatchBatch) {
        if !(self.window_present)() {
            return;
        }
        (self.emit)(batch);
    }
}

/// Where a tick's matches go: into the ring ALWAYS, and out to the window only when there is
/// one.
///
/// The ring lives here, in the concrete sink, and the caps live in the engine loop immediately
/// before the call that reaches this type. Consequence: everything that passes the caps
/// reaches both the event and the buffer, and nothing else reaches either - and that ordering
/// is a property of WHERE each piece lives rather than a rule someone has to maintain.
///
/// The window check lives here too, and not in the engine, exactly as `ScraperSink` holds the
/// `AppHandle` the scraper is not allowed to have. Adding a second consumer later means adding
/// its label or falling back to broadcast, which is a one-line change in this type.
pub(crate) struct WatcherSink {
    pub(crate) history: crate::pty::watchers::history::WatcherHistoryState,
    pub(crate) delivery: WatcherDelivery,
}

impl WatcherEventSink for WatcherSink {
    fn emit(&self, batch: crate::pty::watchers::WatcherMatchBatch) {
        if let Ok(id) = uuid::Uuid::parse_str(&batch.session_id) {
            // Recorded whether or not anyone is listening, so opening the window later shows
            // the history rather than starting from nothing.
            self.history.record(id, &batch.matches);
        }
        self.delivery.deliver(batch);
    }
}
