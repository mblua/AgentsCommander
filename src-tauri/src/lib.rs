pub mod api;
pub mod cli;
pub mod commands;
pub mod config;
pub mod errors;
pub mod logging;
pub mod loops;
pub mod network;
pub(crate) mod path_identity;
pub mod path_utils;
pub mod phone;
pub mod pty;
pub mod resource_monitor;
pub mod screenshot;
pub mod session;
pub mod shutdown;
pub mod telegram;
pub mod testability;
pub mod update_check;
pub mod voice;
pub mod web;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use commands::ac_discovery::DiscoveryBranchWatcher;
use config::sessions_persistence;
use config::settings::SettingsState;
use pty::context_scrape::{
    ContextEventSink, ContextPatternSource, ContextPersistSink, ContextSample, ContextSampleSink,
    ContextScraper, ContextSessionLiveness, ContextUsagePayload, ScreenRowsRead, ScreenRowsSource,
};
use pty::git_watcher::GitWatcher;
use pty::idle_detector::IdleDetector;
use pty::manager::PtyManager;
use session::manager::SessionManager;
use shutdown::ShutdownSignal;
use tauri::{Emitter, Manager};
use telegram::manager::{OutputSenderMap, TelegramBridgeManager, TelegramBridgeState};
use tokio_util::sync::CancellationToken;
use voice::tracker::{VoiceTracker, VoiceTrackingState};
use web::auth::WebAccessToken;
use web::broadcast::WsBroadcaster;

/// Snapshot scanner terminality is deliberately not an input here.
///
/// A retained scanner task publishes a response file; its only `SessionManager`
/// access is a read-lock clone, so it cannot leave session state inconsistent
/// and cannot make a session snapshot wrong. Gating persistence on it was not a
/// rare edge either: `SNAPSHOT_SERVER_TIMEOUT` is twice
/// `SHUTDOWN_CLEANUP_BUDGET_SECS`, and the drain correctly refuses to abort
/// owned or finalizer tasks, so any snapshot admitted inside the shutdown window
/// and running near its own legitimate deadline suppressed persistence and cost
/// the user their session list. Retained scanner work is still reported in the
/// shutdown diagnostics.
pub(crate) fn shutdown_persistence_allowed(
    selection_persistence_safe: bool,
    container_cleanup_terminal: bool,
) -> bool {
    selection_persistence_safe && container_cleanup_terminal
}

#[cfg(test)]
pub(crate) fn combined_shutdown_retained_diagnostics(
    selection_retained: Vec<String>,
    container_retained: Vec<String>,
) -> Vec<String> {
    combined_shutdown_retained_diagnostics_with_scanner(
        Vec::new(),
        selection_retained,
        container_retained,
    )
}

pub(crate) fn combined_shutdown_retained_diagnostics_with_scanner(
    scanner_retained: Vec<String>,
    selection_retained: Vec<String>,
    container_retained: Vec<String>,
) -> Vec<String> {
    let scanner = scanner_retained.into_iter().map(|context| {
        crate::pty::container_runtime::normalize_retained_owner_diagnostic(
            "terminalSnapshotScanner",
            context,
        )
    });
    let selection = selection_retained.into_iter().map(|context| {
        crate::pty::container_runtime::normalize_retained_owner_diagnostic("selection", context)
    });
    let container = container_retained.into_iter().map(|context| {
        crate::pty::container_runtime::normalize_retained_owner_diagnostic(
            "containerShutdown",
            context,
        )
    });
    crate::pty::container_runtime::cap_retained_owner_diagnostics(
        scanner.chain(selection).chain(container),
    )
}

fn remove_container_route_until(
    weak_pty_mgr: &std::sync::Weak<Mutex<PtyManager>>,
    session_id: uuid::Uuid,
    deadline: std::time::Instant,
) -> Result<(), crate::pty::container_backend::RouteRemovalError> {
    let Some(pty_mgr) = weak_pty_mgr.upgrade() else {
        return Ok(());
    };
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(crate::pty::container_backend::RouteRemovalError::Deadline(
                "ptyManager",
            ));
        }
        match pty_mgr.try_lock() {
            Ok(pty_manager) => match pty_manager.try_remove_route_if_kind(
                session_id,
                crate::pty::backend::SessionBackendKind::ContainerTransport,
            ) {
                Ok(()) => return Ok(()),
                Err(crate::pty::manager::PtyRouteRemovalError::LockPoisoned) => {
                    return Err(
                        crate::pty::container_backend::RouteRemovalError::LockPoisoned(
                            "ptyRouteRegistry",
                        ),
                    );
                }
                Err(crate::pty::manager::PtyRouteRemovalError::Busy) => {}
            },
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(
                    crate::pty::container_backend::RouteRemovalError::LockPoisoned("ptyManager"),
                );
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(crate::pty::container_backend::RouteRemovalError::Deadline(
                "ptyManager",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(2).min(remaining));
    }
}

pub(crate) fn install_container_route_remover(pty_mgr: &Arc<Mutex<PtyManager>>) {
    let weak_pty_mgr = Arc::downgrade(pty_mgr);
    let container_backend = pty_mgr.lock().unwrap().container_backend();
    container_backend.set_route_remover(Arc::new(move |session_id, deadline| {
        remove_container_route_until(&weak_pty_mgr, session_id, deadline)
    }));
}

/// Tracks which sessions are currently detached into their own windows.
pub type DetachedSessionsState = Arc<Mutex<HashSet<uuid::Uuid>>>;

struct OwnedWebServer {
    bind: String,
    port: u16,
    handle: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Default)]
pub struct WebServerHandle {
    inner: Arc<Mutex<Option<OwnedWebServer>>>,
}

impl WebServerHandle {
    pub fn store_owned(
        &self,
        bind: String,
        port: u16,
        handle: tauri::async_runtime::JoinHandle<()>,
    ) {
        let mut slot = self.inner.lock().unwrap();
        if let Some(existing) = slot.take() {
            existing.handle.abort();
        }
        *slot = Some(OwnedWebServer { bind, port, handle });
    }

    pub fn is_owned_running(&self, bind: &str, port: u16) -> bool {
        let mut slot = self.inner.lock().unwrap();
        if slot
            .as_ref()
            .map(|owned| owned.handle.inner().is_finished())
            .unwrap_or(false)
        {
            *slot = None;
            return false;
        }

        slot.as_ref()
            .map(|owned| owned.bind == bind && owned.port == port)
            .unwrap_or(false)
    }

    pub fn abort_running(&self) -> bool {
        if let Some(owned) = self.inner.lock().unwrap().take() {
            owned.handle.abort();
            true
        } else {
            false
        }
    }
}

#[derive(Default)]
pub struct ApiServerHandle {
    inner: Arc<Mutex<Option<ApiServerTask>>>,
}

pub struct ApiServerTask {
    join: tauri::async_runtime::JoinHandle<()>,
    shutdown: CancellationToken,
    bound_addr: SocketAddr,
}

impl ApiServerTask {
    pub fn new(
        join: tauri::async_runtime::JoinHandle<()>,
        shutdown: CancellationToken,
        bound_addr: SocketAddr,
    ) -> Self {
        Self {
            join,
            shutdown,
            bound_addr,
        }
    }
}

impl ApiServerHandle {
    /// #791 - handle to the running control-plane API server task.
    pub fn store_if_idle(&self, task: ApiServerTask) -> Result<bool, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "API server handle lock is poisoned".to_string())?;
        if let Some(stored) = inner
            .as_ref()
            .filter(|stored| !stored.join.inner().is_finished())
        {
            log::debug!(
                "[api-server] start ignored; server already running on {}",
                stored.bound_addr
            );
            task.shutdown.cancel();
            task.join.abort();
            return Ok(false);
        }
        *inner = Some(task);
        Ok(true)
    }

    pub fn has_running(&self) -> Result<bool, String> {
        Ok(self.running_bound_addr()?.is_some())
    }

    pub fn running_bound_addr(&self) -> Result<Option<SocketAddr>, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "API server handle lock is poisoned".to_string())?;
        if let Some(stored) = inner
            .as_ref()
            .filter(|stored| !stored.join.inner().is_finished())
        {
            return Ok(Some(stored.bound_addr));
        }
        *inner = None;
        Ok(None)
    }

    pub async fn shutdown_running(&self, timeout: std::time::Duration) -> Result<bool, String> {
        let task = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "API server handle lock is poisoned".to_string())?;
            match inner.as_ref() {
                Some(stored) if stored.join.inner().is_finished() => {
                    *inner = None;
                    return Ok(false);
                }
                Some(_) => inner.take(),
                None => return Ok(false),
            }
        };
        let Some(task) = task else {
            return Ok(false);
        };

        task.shutdown.cancel();
        let mut join = task.join;
        match tokio::time::timeout(timeout, &mut join).await {
            Ok(Ok(())) => Ok(true),
            Ok(Err(err)) => Err(format!("API server task failed during shutdown: {}", err)),
            Err(_) => {
                join.abort();
                let _ = join.await;
                Ok(true)
            }
        }
    }
}

/// Serializes the config-seed critical section (`perform_config_seed`) for a
/// replica, so two concurrent same-replica spawns cannot clobber each other's
/// in-flight `<dest>.acseed-*` scratch during `clear_stale_seed_scratch`
/// (grinch HIGH-1; see `config/config_seed.rs` CONCURRENCY CONTRACT). Acquired
/// in `commands::session.rs` around the seed swap.
pub type ConfigSeedLockState = Arc<tokio::sync::Mutex<()>>;

// Issue #609 - cached "npm update available" result. Set ONCE by the startup
// check task; read by `get_update_status` so a late-mounting sidebar still
// sees a pending update.
pub type UpdateCheckState = Arc<std::sync::OnceLock<update_check::UpdateInfo>>;

/// Floating spec/Mermaid board document state.
pub type SpecBoardState = Arc<tokio::sync::RwLock<commands::spec_board::SpecBoardManager>>;

/// #632 - hard ceiling on the shutdown reaper cleanup. For jobbed sessions the Job
/// Object kill already prevented orphans, so exceeding this just stops the
/// best-effort accounting reaper. For a job-less session (assign failed) this bound
/// CAN abandon a still-dying tree, so the Exit handler warns when that is possible
/// (MED-2).
const SHUTDOWN_CLEANUP_BUDGET_SECS: u64 = 5;

/// Master token generated at app startup. Allows bypassing team validation (can_reach).
/// Persisted to `master-token.txt` in config_dir for CLI use. Regenerated on each app startup. See #34.
/// Field is private — use `matches()` for constant-time comparison.
pub struct MasterToken(String);

impl MasterToken {
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// Constant-time comparison to prevent timing oracle attacks.
    pub fn matches(&self, candidate: &str) -> bool {
        let a = self.0.as_bytes();
        let b = candidate.as_bytes();
        if a.len() != b.len() {
            return false;
        }
        a.iter()
            .zip(b.iter())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
    }

    /// Display value (for printing to stdout at startup only).
    pub fn value(&self) -> &str {
        &self.0
    }
}

/// §224 A.2.5 / G1 — set true while the post-startup session-restore loop is
/// running (`lib.rs` setup task that calls `create_session_inner` for every
/// persisted session). Read by `mailbox::handle_close_session` to decide
/// whether `session_ids.is_empty()` means "no live session for this FQN" or
/// "restore loop hasn't reached this session yet — retry briefly."
pub struct RestoreInProgress(pub AtomicBool);

/// (#617/#668) Sessions with a self context operation awaiting their sustained-idle
/// window. Insert on queue; remove when the deferred task completes, the session
/// dies, or the safety cap expires. A session_id already present means a repeat
/// self operation is a no-op ("already_queued") - requests never stack.
/// In-memory only: a daemon restart drops pending requests (accepted, best-effort).
///
/// Newtype is mandatory: `DetachedSessionsState` (lib.rs:38) is already a managed
/// bare `Arc<Mutex<HashSet<Uuid>>>`, and Tauri keys managed state by Rust type, so
/// a second bare alias would collide. Mirrors `RestoreInProgress`.
#[derive(Default)]
pub struct PendingSelfClear(pub Mutex<HashSet<uuid::Uuid>>);

/// Instance-private outbox directory. Only this app instance polls it.
/// Created at startup, path printed to stdout alongside master token.
pub struct AppOutbox(String);

impl AppOutbox {
    pub fn new(path: String) -> Self {
        Self(path)
    }

    pub fn path(&self) -> &str {
        &self.0
    }
}

/// Decide whether a persisted session should be restored with a live PTY
/// at app startup, or deferred (created as a dormant `Exited(0)` record).
///
/// Inputs:
///   - `setting_on`: value of `AppSettings::restore_coordinator_wake_state`.
///   - `is_coord`: whether the agent FQN derived from `ps.working_directory`
///     is a coordinator of any discovered team.
///   - `persisted_status`: `PersistedSession::status` as snapshotted at the
///     last app shutdown. `None` means the snapshot was taken by an older
///     binary that did not record status. Treat `None` as **awake** for
///     forward-compat — better to wake a coord the user expected to be
///     awake than silently leave it dormant on first launch after upgrade.
///
/// Returns true ⇒ restore with PTY; false ⇒ defer (dormant).
pub(crate) fn should_wake_on_restore(
    setting_on: bool,
    is_coord: bool,
    persisted_status: Option<&crate::session::session::SessionStatus>,
) -> bool {
    if !setting_on {
        return false; // Setting OFF: defer everything.
    }
    if !is_coord {
        return false; // Non-coord: always deferred under the new policy.
    }
    match persisted_status {
        Some(crate::session::session::SessionStatus::Exited(_)) => false, // asleep at shutdown
        Some(_) | None => true, // awake at shutdown (or unknown → fail-open)
    }
}

pub(crate) fn restore_session_should_wake(
    archived_session: bool,
    setting_on: bool,
    is_coord: bool,
    persisted_status: Option<&crate::session::session::SessionStatus>,
) -> bool {
    !archived_session && should_wake_on_restore(setting_on, is_coord, persisted_status)
}

pub(crate) fn restore_session_should_become_active(
    was_active: bool,
    archived_session: bool,
) -> bool {
    was_active && !archived_session
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PersistedActiveFlagNormalization {
    Zero,
    One { index: usize },
    Multiple { identities: Vec<String> },
}

/// Normalize persisted canonical-selection intent before any restore side
/// effect. A single flag remains authoritative. Multiple flags are corrupt
/// input, so all are cleared and final restore uses the documented first
/// eligible live attached fallback.
pub(crate) fn normalize_persisted_active_flags(
    sessions: &mut [crate::config::sessions_persistence::PersistedSession],
) -> PersistedActiveFlagNormalization {
    let flagged = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| session.was_active)
        .map(|(index, session)| {
            (
                index,
                format!("{}:{}@{}", index, session.name, session.working_directory),
            )
        })
        .collect::<Vec<_>>();
    match flagged.as_slice() {
        [] => PersistedActiveFlagNormalization::Zero,
        [(index, _)] => PersistedActiveFlagNormalization::One { index: *index },
        _ => {
            let identities = flagged.into_iter().map(|(_, identity)| identity).collect();
            for session in sessions {
                session.was_active = false;
            }
            PersistedActiveFlagNormalization::Multiple { identities }
        }
    }
}

#[derive(Debug, Default)]
struct RestoreObserverStartBarrier {
    phase: AtomicU8,
}

impl RestoreObserverStartBarrier {
    fn mark_restore_admitted(&self) -> Result<(), String> {
        self.phase
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|phase| format!("restore observer barrier admission from phase {phase}"))
    }

    fn mark_restore_complete(&self) -> Result<(), String> {
        self.phase
            .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|phase| format!("restore observer barrier completion from phase {phase}"))
    }

    fn start(&self, producer: &str, start: impl FnOnce()) -> Result<(), String> {
        if self.phase.load(Ordering::Acquire) != 2 {
            return Err(format!(
                "startup producer {producer} attempted before restore completion"
            ));
        }
        start();
        Ok(())
    }
}

/// (#630) Resolve coordinator status for a restore decision, backstopping a
/// transient empty `discover_teams()` with the snapshot's persisted
/// `is_coordinator`. When live discovery returned teams we trust it. Only when
/// discovery came back EMPTY do we fall back, so a real coordinator is not
/// silently downgraded to "deferred" because project paths were not ready at
/// cold start. The woken session's `is_coordinator` is recomputed in
/// `create_session_inner`, so a stale backstop cannot poison identity.
pub(crate) fn resolve_is_coord_for_restore(
    live_is_coord: bool,
    teams_empty: bool,
    persisted_is_coord: bool,
) -> bool {
    live_is_coord || (teams_empty && persisted_is_coord)
}

/// (#630/#631) Bridge the persisted `start_fresh_on_restore` intent into the
/// restore wake path's `skip_auto_resume` argument. The two are value-identical
/// (both `true` => start fresh / suppress `--continue`); this is the single
/// named seam the wake path reads. Anti-revert guard: CI runs
/// `cargo clippy --all-targets -- -D warnings`, so reverting the call site to a
/// hardcoded value leaves this function unused and the `dead_code` lint fails the
/// build. The unit test `wake_path_passes_persisted_fresh_intent` pins this
/// bridge's own behavior but would not, by itself, catch a reverted call site.
pub(crate) fn skip_auto_resume_for_restore(start_fresh_on_restore: bool) -> bool {
    start_fresh_on_restore
}

pub(crate) fn should_wake_root_agent_on_restore(
    persisted_status: Option<&crate::session::session::SessionStatus>,
) -> bool {
    match persisted_status {
        Some(crate::session::session::SessionStatus::Exited(_)) => false,
        Some(crate::session::session::SessionStatus::Active)
        | Some(crate::session::session::SessionStatus::Running)
        | Some(crate::session::session::SessionStatus::Idle)
        | None => true,
    }
}

pub(crate) fn should_auto_create_root_agent_on_first_restore(
    settings: &crate::config::settings::AppSettings,
    last_coding_agent: Option<&str>,
) -> bool {
    commands::session::resolve_root_agent_command(settings, None, last_coding_agent).is_ok()
}

// ---- #1032/#1056: the four narrow scrape adapters ---------------------------------
//
// This is the capability boundary. A sample may enqueue an informational coordinator
// notice, but the scraper itself cannot route, inject, wake, or remediate a session.

/// Rows, via the routed backend. The three states come from the backend, which is the only
/// thing that holds a liveness oracle.
struct ScraperRows {
    pty_mgr: Arc<Mutex<PtyManager>>,
    /// A poisoned `PtyManager` is app-wide and permanent, so the warning is worth exactly
    /// one line, not one per configured session every 5 seconds.
    poison_logged: AtomicBool,
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
struct ScraperPatterns {
    settings: SettingsState,
}

impl ContextPatternSource for ScraperPatterns {
    fn patterns(&self) -> futures::future::BoxFuture<'_, HashMap<String, String>> {
        Box::pin(async move {
            let settings = self.settings.read().await;
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
                .collect()
        })
    }
}

/// The sink. `PtyOutputTarget` (`output.rs`) already wraps an `AppHandle` behind a plain
/// `Fn` for the same reason.
struct ScraperSink {
    app_handle: tauri::AppHandle,
}

impl ContextEventSink for ScraperSink {
    fn emit(&self, payload: ContextUsagePayload) {
        let _ = self.app_handle.emit("session_context", payload);
    }
}

/// Nonblocking, bounded bridge from the scraper thread to the alert actor. It deliberately
/// carries no app, PTY, session, filesystem, or delivery capability.
struct ScraperSamples {
    sender: tokio::sync::mpsc::Sender<ContextSample>,
    closed_logged: AtomicBool,
    saturated: AtomicBool,
    dropped: AtomicU64,
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
                        "[context-alert] sample queue saturated; advisory samples are being dropped (dropped={})",
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
struct ScraperPersist {
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
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
            crate::config::sessions_persistence::persist_current_state(&guard).await;
        })
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run(
    test_window_placement: Option<crate::testability::window_placement::TestWindowPlacement>,
    ui_automation_enabled: bool,
) {
    // Same backend the CLI path now installs in `main.rs` — see `logging.rs`
    // for the rationale. Idempotent, so a hypothetical second call (or the
    // CLI path having already run in this process) is a no-op.
    crate::logging::init_logger();

    // Generate master token — printed to stdout and persisted to master-token.txt for CLI use
    let master_token = MasterToken::new(uuid::Uuid::new_v4().to_string());

    // Create instance-private outbox directory and clean up stale ones
    let config_dir = config::config_dir().expect("Cannot determine home directory");
    let instances_dir = config_dir.join("instances");

    // Clean up old instance dirs (from previous runs)
    if let Ok(entries) = std::fs::read_dir(&instances_dir) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
        log::info!("[app-outbox] Cleaned stale instance directories");
    }

    // #769 Phase 1 - seed the externalized coding-agent catalog once (whole-file
    // seed-once, fail-soft; never aborts boot). Must run before the frontend can
    // call `get_coding_agent_catalog`.
    config::coding_agents_catalog::ensure_seeded(&config_dir);

    // #769 Phase 2 - seed the dest-keyed default config-folder masters once
    // (create-if-absent, fail-soft). They back the absent-only spawn tier and the
    // Settings re-seed button.
    config::coding_agents_catalog::ensure_seeded_masters(&config_dir);

    let instance_id = uuid::Uuid::new_v4().to_string();
    // #1149 - open the activity run here, before the rest of boot: a panic in the
    // remaining path then still leaves a run that had started and never stopped,
    // which the next startup reports as unclean. This is also the last point at
    // which `daemon.pid` still holds the PREVIOUS writer's PID, which is what
    // lets the scan tell a dead predecessor from a live sibling.
    crate::config::activity_log::init_run(&config_dir, &instance_id);
    let app_outbox_path = instances_dir.join(&instance_id).join("outbox");
    std::fs::create_dir_all(&app_outbox_path).expect("Failed to create app outbox directory");
    let app_outbox = AppOutbox::new(app_outbox_path.to_string_lossy().to_string());
    let ui_automation_state = crate::testability::ui_automation::UiAutomationState::new(
        ui_automation_enabled,
        config_dir.clone(),
    );

    // Generate web access token — separate from master token for limited blast radius
    let web_access_token = Arc::new(WebAccessToken::new(uuid::Uuid::new_v4().to_string()));

    println!("[master-token] {}", master_token.value());
    println!("[web-token] {}", web_access_token.value());
    println!("[app-outbox] {}", app_outbox.path());
    log::info!("[master-token] Generated (see stdout)");
    log::info!("[web-token] Generated (see stdout)");
    log::info!("[app-outbox] {} (see stdout)", app_outbox.path());

    // Write web token to a file so external tools can read it
    if let Some(token_path) = config::config_dir().map(|d| d.join("web-token.txt")) {
        let _ = std::fs::write(&token_path, web_access_token.value());
    }

    // Persist master token and app outbox path so the CLI can use them
    if let Some(dir) = config::config_dir() {
        let _ = std::fs::write(dir.join("master-token.txt"), master_token.value());
        let _ = std::fs::write(dir.join("app-outbox-path.txt"), app_outbox.path());
    }

    // Issue #231: write daemon.pid so CLI verbs can detect a dead daemon.
    config::daemon_pid::write_pid_file();

    // Create WS broadcaster (shared between Tauri commands and web server)
    let broadcaster = WsBroadcaster::new();

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let shutdown_signal = ShutdownSignal::new();
    let terminal_snapshot_state =
        crate::pty::terminal_snapshot::TerminalSnapshotState::new(shutdown_signal.clone());
    let selection_coordinator = crate::session::selection::SelectionCoordinator::new(
        Arc::clone(&session_mgr),
        shutdown_signal.token().clone(),
    );

    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));

    // Idle detector: emits session_idle / session_busy events.
    // Callbacks run on native threads (watcher + PTY read loop).
    // AppHandle.emit() is sync and thread-safe, so no tokio needed.
    // AppHandle is set in setup() via OnceLock; callbacks no-op until then.
    let app_handle_lock: Arc<OnceLock<tauri::AppHandle>> = Arc::new(OnceLock::new());
    let handle_for_idle = Arc::clone(&app_handle_lock);
    let handle_for_busy = Arc::clone(&app_handle_lock);
    let idle_detector = IdleDetector::new(
        move |id| {
            log::debug!("[idle] >>> EMIT session_idle for {}", &id.to_string()[..8]);
            if let Some(app) = handle_for_idle.get() {
                let _ = tauri::Emitter::emit(
                    app,
                    "session_idle",
                    serde_json::json!({ "id": id.to_string() }),
                );
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr_clone = session_mgr.inner().clone();
                let app_for_idle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let mgr = mgr_clone.read().await;
                    mgr.mark_idle(id).await;
                    crate::config::sessions_persistence::persist_current_state(&mgr).await;
                    if let Some(scheduler) =
                        app_for_idle.try_state::<Arc<loops::scheduler::LoopScheduler>>()
                    {
                        scheduler
                            .inner()
                            .on_session_idle(app_for_idle.clone(), id)
                            .await;
                    }
                });
            }
        },
        move |id| {
            log::debug!("[idle] >>> EMIT session_busy for {}", &id.to_string()[..8]);
            if let Some(app) = handle_for_busy.get() {
                let _ = tauri::Emitter::emit(
                    app,
                    "session_busy",
                    serde_json::json!({ "id": id.to_string() }),
                );
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr_clone = session_mgr.inner().clone();
                tauri::async_runtime::spawn(async move {
                    let mgr = mgr_clone.read().await;
                    mgr.mark_busy(id).await;
                    crate::config::sessions_persistence::persist_current_state(&mgr).await;
                });
            }
        },
    );
    let session_mgr_for_git = Arc::clone(&session_mgr);
    let session_mgr_for_discovery = Arc::clone(&session_mgr);
    let session_mgr_for_web = Arc::clone(&session_mgr);
    let session_mgr_for_api = Arc::clone(&session_mgr);
    let session_mgr_for_exit = Arc::clone(&session_mgr);
    // #1088 - handed to `ScraperPersist` so the context scraper can persist
    // changed readings through the same path the idle/busy callbacks use.
    let session_mgr_for_scraper = Arc::clone(&session_mgr);
    let output_senders_for_pty = output_senders.clone();
    let idle_detector_for_pty = Arc::clone(&idle_detector);
    // #552 manage the IdleDetector so the shared user-message helper, the mailbox
    // wake path, and the auto-close task can reach its silence clock.
    let idle_detector_for_state = Arc::clone(&idle_detector);
    let idle_detector_for_setup = Arc::clone(&idle_detector);
    let broadcaster_for_pty = broadcaster.clone();
    let broadcaster_for_web = broadcaster.clone();
    let web_token_for_server = Arc::clone(&web_access_token);

    let tg_mgr: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::new(output_senders),
    ));

    let loaded_settings = config::settings::load_settings();
    // (#621) Snapshot registered project paths for the startup orphan-clock prune
    // below (taken before the value is moved into the RwLock).
    let startup_project_paths = loaded_settings.project_paths.clone();
    let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(loaded_settings));
    // #552 persisted coordinator badge clock + auto-closed marker store (loaded
    // once at startup; flushed by the auto-close tick and on app exit).
    let coordinator_clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
        Arc::new(Mutex::new(crate::config::coordinator_clocks::load()));
    // (#621) Conservative backstop: drop clock keys for workgroups confirmed gone
    // on disk (historical orphans + CLI-removed wgs). Keep-on-any-doubt.
    crate::config::coordinator_clocks::prune_orphaned_workgroups_and_persist(
        &coordinator_clocks,
        &startup_project_paths,
    );
    let coordinator_clocks_for_exit = Arc::clone(&coordinator_clocks);
    let resource_monitor_state = Arc::new(resource_monitor::ResourceMonitorState::new());
    // #714 screenshot capture lifecycle + global-hotkey registration state.
    let screenshot_capture_state: screenshot::ScreenshotCaptureState = Arc::new(
        tokio::sync::Mutex::new(screenshot::ScreenshotCaptureLifecycle::Idle),
    );
    let screenshot_hotkey_state: screenshot::ScreenshotHotkeyState = Arc::new(
        std::sync::Mutex::new(screenshot::ScreenshotHotkeyRuntime::default()),
    );
    let settings_for_web = Arc::clone(&settings);
    let detached_sessions: DetachedSessionsState = Arc::new(Mutex::new(HashSet::new()));
    let voice_tracking: VoiceTrackingState = Arc::new(Mutex::new(VoiceTracker::new()));
    let spec_board_state: SpecBoardState = Arc::new(tokio::sync::RwLock::new(
        commands::spec_board::SpecBoardManager::new(),
    ));
    let loop_scheduler = Arc::new(loops::scheduler::LoopScheduler::new());
    let loop_scheduler_for_setup = Arc::clone(&loop_scheduler);

    // (#777) Non-stop watchdog: timing + actuation state. Managed for the
    // `non_stop_report` command; the background loop is started in setup.
    let non_stop_state = crate::loops::non_stop_watchdog::NonStopWatchdogState::new();
    let non_stop_state_for_setup = non_stop_state.clone();

    // Config-seed critical-section lock. Serializes `perform_config_seed` for a
    // replica so concurrent same-replica spawns cannot clobber each other's
    // in-flight seed scratch (see `ConfigSeedLockState`).
    let config_seed_lock: ConfigSeedLockState = Arc::new(tokio::sync::Mutex::new(()));

    // Issue #609 - cached "npm update available" result, set ONCE by the
    // detached startup check below and read by `get_update_status`.
    let update_check_state: UpdateCheckState = Arc::new(std::sync::OnceLock::new());
    let update_check_state_for_setup = Arc::clone(&update_check_state);

    let shutdown_for_setup = shutdown_signal.clone();
    let shutdown_for_exit = shutdown_signal.clone();
    let selection_coordinator_for_setup = selection_coordinator.clone();
    let selection_coordinator_for_exit = selection_coordinator.clone();
    let tg_mgr_for_exit = tg_mgr.clone();
    let resource_monitor_for_setup = Arc::clone(&resource_monitor_state);
    let resource_monitor_for_exit = Arc::clone(&resource_monitor_state);
    let ui_automation_state_for_setup = ui_automation_state.clone();
    let ui_automation_state_for_exit = ui_automation_state.clone();

    // One recovered operation store is shared by the filesystem poller and
    // both API start paths. A failure disables only privileged PTY input.
    let message_store_state = crate::api::message_store::MessageStoreState::initialize();
    let pty_target_gate_state = message_store_state.target_gate_state();

    // #714 clipboard + global-shortcut plugins are referenced ONLY on Windows so
    // non-Windows release builds never link them (screenshot capture is
    // Windows-only for this issue). The rest of the builder chain is shared.
    let builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());

    #[cfg(target_os = "windows")]
    let builder = builder
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        crate::screenshot::begin_capture_from_hotkey(app.clone());
                    }
                })
                .build(),
        );

    builder
        .manage(master_token)
        .manage(app_outbox)
        .manage(session_mgr)
        .manage(selection_coordinator)
        .manage(tg_mgr)
        .manage(network::OutboundNetwork::new().expect("failed to build shared network clients"))
        .manage(Arc::clone(&resource_monitor_state))
        .manage(voice_tracking)
        .manage(settings)
        .manage(idle_detector_for_state) // #552 managed type: Arc<IdleDetector>
        .manage(coordinator_clocks) // #552 managed type: CoordinatorClocksState
        .manage(std::sync::Arc::new(crate::session::purge_guard::PurgeGuard::default())) // #885
        .manage(detached_sessions.clone())
        .manage(spec_board_state.clone())
        .manage(loop_scheduler.clone())
        .manage(non_stop_state)
        .manage(web_access_token.clone())
        .manage(broadcaster.clone())
        .manage(WebServerHandle::default())
        .manage(ApiServerHandle::default())
        .manage(message_store_state)
        .manage(pty_target_gate_state)
        .manage(config_seed_lock)
        .manage(update_check_state)
        .manage(ui_automation_state)
        .manage(terminal_snapshot_state)
        .manage(shutdown_signal)
        .manage(Arc::new(RestoreInProgress(AtomicBool::new(false))))
        .manage(Arc::new(PendingSelfClear::default()))
        .manage(screenshot_capture_state) // #714
        .manage(screenshot_hotkey_state) // #714
        .manage(crate::pty::input_activity::new_state()) // #871 substantive-input tracker
        .manage(crate::session::warnings::new_session_warning_state())
        .setup(move |app| {
            use tauri::WebviewWindowBuilder;
            use tauri::WebviewUrl;

            // Make AppHandle available to idle detector callbacks
            let _ = app_handle_lock.set(app.handle().clone());

            // #264 — spawn the background task that emits `error_log_event`
            // pings to the UI when ERROR entries are captured. The task runs
            // OUTSIDE the env_logger format closure (see §3.7 / B1). Entries
            // logged before this point stay buffered; the frontend's first
            // `drain_error_logs` call collects them.
            crate::logging::spawn_error_emit_task(app.handle().clone());

            // #271 — seed `<config_dir>/agent-templates/` + README on startup.
            crate::commands::role_templates::ensure_default_templates_dir_at_config();

            // (#621) GC the context-cache: unlink generated *-context-*.md files
            // older than the retention window. Cleans orphans from removed
            // workgroups AND caps the unbounded-growth secondary finding. Robust +
            // self-healing: live agents re-write their cache every launch.
            crate::config::session_context::sweep_context_cache_at_startup();

            // Git branch watcher: polls git branch for each session every 5s
            let git_watcher = GitWatcher::new(session_mgr_for_git, app.handle().clone());
            // Register for Tauri commands that take `State<'_, Arc<GitWatcher>>`
            // (e.g. `update_team`, `sync_workgroup_repos`). Must happen BEFORE the
            // `PtyManager::new(..., git_watcher, ...)` move below.
            app.manage(Arc::clone(&git_watcher));

            // Discovery branch watcher: polls git branch for discovered replicas every 15s
            let discovery_branch_watcher = DiscoveryBranchWatcher::new(
                app.handle().clone(),
                session_mgr_for_discovery,
            );
            app.manage(Arc::clone(&discovery_branch_watcher));

            // PtyManager needs GitWatcher for cleanup on session kill
            let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
                output_senders_for_pty,
                idle_detector_for_pty,
                Arc::clone(&git_watcher),
                Some(broadcaster_for_pty),
                Some(selection_coordinator_for_setup.container_lifecycle_sender()),
            )));
            install_container_route_remover(&pty_mgr);
            pty_mgr
                .lock()
                .unwrap()
                .cleanup_container_orphans_on_startup();
            app.manage(pty_mgr.clone());

            selection_coordinator_for_setup
                .start(app.handle().clone())
                .map_err(|error| error.to_string())?;
            let restore_observer_barrier = RestoreObserverStartBarrier::default();
            let restore_barrier = tauri::async_runtime::block_on(
                selection_coordinator_for_setup.submit_restore_first(),
            )?;
            restore_observer_barrier.mark_restore_admitted()?;
            let restore_transaction = restore_barrier.transaction(app.handle().clone());
            app.state::<Arc<RestoreInProgress>>()
                .0
                .store(true, std::sync::atomic::Ordering::SeqCst);

            // #1056 context-alert actor. Start it before the scraper so the bounded sender
            // exists before the first sample; manage it for final joined shutdown.
            let context_alert_monitor =
                crate::session::context_alerts::ContextAlertMonitor::start(
                    app.handle().clone(),
                    shutdown_for_setup.token().child_token(),
                );
            app.manage(Arc::clone(&context_alert_monitor));

            // #1032 context scrape. Must be after `.manage(settings)` above, since the
            // pattern adapter reads settings back out of managed state. Mirrors GitWatcher.
            let context_scraper = ContextScraper::new(
                Arc::new(ScraperRows {
                    pty_mgr: pty_mgr.clone(),
                    poison_logged: AtomicBool::new(false),
                }),
                Arc::new(ScraperPatterns {
                    settings: app.state::<SettingsState>().inner().clone(),
                }),
                Arc::new(ScraperSink {
                    app_handle: app.handle().clone(),
                }),
                Arc::new(ScraperSamples {
                    sender: context_alert_monitor.sender(),
                    closed_logged: AtomicBool::new(false),
                    saturated: AtomicBool::new(false),
                    dropped: AtomicU64::new(0),
                }),
                Arc::new(ScraperPersist {
                    session_mgr: session_mgr_for_scraper,
                }),
            );
            context_scraper.start(shutdown_for_setup.clone());
            app.manage(Arc::clone(&context_scraper));

            // Start web server if enabled in settings
            {
                let web_settings = config::settings::load_settings();
                if web_settings.web_server_enabled {
                    let bind = web_settings.web_server_bind.clone();
                    let port = web_settings.web_server_port;

                    match tauri::async_runtime::block_on(web::start_server(
                        bind.clone(),
                        port,
                        web_token_for_server,
                        session_mgr_for_web,
                        pty_mgr.clone(),
                        settings_for_web,
                        broadcaster_for_web,
                        app.handle().clone(),
                        shutdown_for_setup.clone(),
                    )) {
                        Ok(join_handle) => {
                            println!(
                                "[web-token] Remote URL: http://{}:{}/?window=main&remoteToken={}",
                                bind,
                                port,
                                web_access_token.value()
                            );
                            let ws_handle = app.state::<WebServerHandle>();
                            ws_handle.store_owned(bind, port, join_handle);
                        }
                        Err(err) => {
                            log::warn!("[web-server] startup failed: {}", err);
                        }
                    }
                }
            }

            // #791 - start the control-plane API server if enabled in settings.
            // Opt-in (default false), mirroring the web server block above. The
            // managed handle is stored only after bind readiness is confirmed.
            {
                let api_settings = config::settings::load_settings();
                if api_settings.api_server_enabled {
                    let bind = api_settings.api_server_bind.clone();
                    let port = api_settings.api_server_port;
                    let api_shutdown = shutdown_for_setup.token().child_token();
                    let server_start = api::start_server(
                        bind,
                        port,
                        app.handle().clone(),
                        session_mgr_for_api.clone(),
                        pty_mgr.clone(),
                        api_shutdown.clone(),
                    );
                    match tauri::async_runtime::block_on(api::wait_for_startup_ready(
                        server_start.readiness,
                    )) {
                        Ok(bound_addr) => {
                            let api_handle = app.state::<ApiServerHandle>();
                            if let Err(e) = api_handle.store_if_idle(ApiServerTask::new(
                                server_start.join_handle,
                                api_shutdown,
                                bound_addr,
                            )) {
                                log::error!("[api-server] failed to store server handle: {}", e);
                            }
                        }
                        Err(err) => {
                            api_shutdown.cancel();
                            log::warn!("[api-server] startup failed: {}", err);
                        }
                    }
                }
            }

            // Issue #609 - detached "npm update available" check. Fully fail-silent;
            // detached so startup is never blocked or delayed (acceptance criterion).
            {
                let app_handle_for_update = app.handle().clone();
                let update_cache = Arc::clone(&update_check_state_for_setup);
                tauri::async_runtime::spawn(async move {
                    crate::update_check::run_startup_check(app_handle_for_update, update_cache).await;
                });
            }

            if let Err(e) = crate::config::root_agent::ensure_root_agent_dir() {
                log::error!("[root-agent] Failed to provision root agent directory: {}", e);
            }

            // §224 A.2.5 / G-IMPL-1 — Set restore_in_progress=TRUE BEFORE the
            // mailbox poller starts. The restore task now also owns the root-agent
            // first-start path, so it must run even with no persisted sessions.
            //
            // SEQUENCE-CRITICAL: `MailboxPoller::start()` spawns a tokio worker
            // task that runs its first poll WITHOUT delay (mailbox.rs:200-204)
            // in parallel with the rest of setup() on the main thread. If a
            // close-session message is queued in any outbox at startup, that
            // first poll picks it up. With the flag stuck false, the race-
            // guard wait loop in handle_close_session (§A.2.5,
            // mailbox.rs:1201-1242) is bypassed → status="no_match" instead
            // of "restore_in_progress", AND the A.7 cleanup (mailbox.rs:1293-
            // 1311) drops the failed-recoverable ghosts the restore loop was
            // about to retry. This recreates the exact silent-success bug
            // #224 was filed to fix.
            //
            // Hoisting the flag set above mailbox_poller.start() closes the
            // race window. The restore task spawned below (§A.2.5 RAII guard)
            // is still responsible for clearing the flag when restore
            // completes (or panics).
            //
            // The matching `load_sessions()` call at the original site is
            // removed; `persisted` is reused by the restore task below.
            let restore_settings_snapshot = config::settings::load_settings();
            // #698 — the orphan purge now takes the async `sessions_save_lock()`
            // across its load+filter+save so it cannot clobber a concurrently
            // persisted raise-hand. This sync `setup` body runs on the main
            // thread (outside any runtime worker), so `block_on` is safe here,
            // matching the existing `tauri::async_runtime::block_on` uses in the
            // run-event handler. The lock is uncontended at this point (the
            // mailbox poller and other writers start below), so it returns
            // immediately.
            let restore_session_paths =
                sessions_persistence::session_retention_project_paths(&restore_settings_snapshot);
            let mut persisted = tauri::async_runtime::block_on(
                sessions_persistence::load_sessions_purging_outside_project_paths(
                    &restore_session_paths,
                ),
            );
            match normalize_persisted_active_flags(&mut persisted) {
                PersistedActiveFlagNormalization::Zero => {
                    log::debug!("[restore] persisted selection flags normalized: zero");
                }
                PersistedActiveFlagNormalization::One { index } => {
                    log::debug!(
                        "[restore] persisted selection flags normalized: exactly one rowIndex={}",
                        index
                    );
                }
                PersistedActiveFlagNormalization::Multiple { identities } => {
                    log::warn!(
                        "[restore] inconsistent was_active flags count={} rows=[{}]; exact target cleared for eligible-live fallback",
                        identities.len(),
                        identities.join(", ")
                    );
                }
            }
            let restore_flag = app
                .state::<Arc<RestoreInProgress>>()
                .inner()
                .clone();
            restore_flag
                .0
                .store(true, std::sync::atomic::Ordering::SeqCst);

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))
                .expect("Failed to load app icon");

            // Load saved window geometry
            let saved_settings = config::settings::load_settings();

            // Collect available monitor bounds (physical) + scale factor for geometry validation
            // Tuple: (x, y, x2, y2, scale_factor) — all positions/sizes in physical pixels
            let monitors: Vec<(f64, f64, f64, f64, f64)> = app
                .available_monitors()
                .unwrap_or_default()
                .iter()
                .map(|m| {
                    let pos = m.position();
                    let size = m.size();
                    (
                        pos.x as f64,
                        pos.y as f64,
                        pos.x as f64 + size.width as f64,
                        pos.y as f64 + size.height as f64,
                        m.scale_factor(),
                    )
                })
                .collect();

            log::info!("[window-setup] {} monitors detected", monitors.len());
            for (i, (mx, my, mx2, my2, scale)) in monitors.iter().enumerate() {
                log::info!("[window-setup]   monitor {}: ({}, {}) -> ({}, {}) scale={}", i, mx, my, mx2, my2, scale);
            }

            /// Check if at least 50px of a window (physical coords) is visible on any monitor
            fn is_visible_on_monitors(
                geo: &config::settings::WindowGeometry,
                monitors: &[(f64, f64, f64, f64, f64)],
            ) -> bool {
                if monitors.is_empty() {
                    return true; // Can't validate, assume OK
                }
                let margin = 50.0;
                monitors.iter().any(|(mx, my, mx2, my2, _)| {
                    geo.x + geo.width > mx + margin
                        && geo.x < mx2 - margin
                        && geo.y + geo.height > my + margin
                        && geo.y < my2 - margin
                })
            }

            /// Convert saved geometry (physical pixels) to logical pixels for the builder.
            /// Finds which monitor the geometry center falls on and divides by that scale.
            fn physical_to_logical(
                geo: &config::settings::WindowGeometry,
                monitors: &[(f64, f64, f64, f64, f64)],
            ) -> config::settings::WindowGeometry {
                let cx = geo.x + geo.width / 2.0;
                let cy = geo.y + geo.height / 2.0;
                let scale = monitors
                    .iter()
                    .find(|(mx, my, mx2, my2, _)| cx >= *mx && cx < *mx2 && cy >= *my && cy < *my2)
                    .map(|(_, _, _, _, s)| *s)
                    .unwrap_or(1.0);
                config::settings::WindowGeometry {
                    x: geo.x / scale,
                    y: geo.y / scale,
                    width: geo.width / scale,
                    height: geo.height / scale,
                }
            }

            // Determine primary monitor size for the default "centered main" layout.
            // Convert to logical pixels (physical / scale) since WebviewWindowBuilder
            // ::inner_size() and ::position() expect logical coordinates.
            let primary = app.primary_monitor().ok().flatten();
            let primary_scale = primary.as_ref().map(|m| m.scale_factor()).unwrap_or(1.0);
            let (screen_w, screen_h) = primary
                .as_ref()
                .map(|m| {
                    let s = m.size();
                    (s.width as f64 / primary_scale, s.height as f64 / primary_scale)
                })
                .unwrap_or((1920.0, 1080.0));
            let primary_x = primary
                .as_ref()
                .map(|m| m.position().x as f64 / primary_scale)
                .unwrap_or(0.0);
            let primary_y = primary
                .as_ref()
                .map(|m| m.position().y as f64 / primary_scale)
                .unwrap_or(0.0);

            // Default main window: centered at 1400×900, or the primary monitor size
            // minus a small margin if the screen is narrower than 1400.
            let default_w = screen_w.min(1400.0);
            let default_h = screen_h.min(900.0);
            let default_main = config::settings::WindowGeometry {
                x: primary_x + (screen_w - default_w) / 2.0,
                y: primary_y + (screen_h - default_h) / 2.0,
                width: default_w,
                height: default_h,
            };

            fn log_main_window_info(win: &tauri::WebviewWindow) {
                let pid = std::process::id();
                let pos = win.outer_position().ok();
                let size = win.outer_size().ok();
                let maximized = win.is_maximized().ok();
                log::info!(
                    "[test-window] actual pid={} position={:?} size={:?} maximized={:?}",
                    pid,
                    pos,
                    size,
                    maximized
                );
                println!(
                    "{{\"event\":\"testWindowInfo\",\"pid\":{},\"position\":{},\"size\":{},\"maximized\":{}}}",
                    pid,
                    serde_json::to_string(&pos.map(|p| serde_json::json!({ "x": p.x, "y": p.y })))
                        .unwrap_or_else(|_| "null".to_string()),
                    serde_json::to_string(
                        &size.map(|s| serde_json::json!({ "width": s.width, "height": s.height }))
                    )
                    .unwrap_or_else(|_| "null".to_string()),
                    serde_json::to_string(&maximized).unwrap_or_else(|_| "null".to_string())
                );
            }

            fn apply_test_window_placement(
                win: &tauri::WebviewWindow,
                geo: &crate::testability::window_placement::TestWindowPlacement,
            ) -> bool {
                let x = geo.x.round() as i32;
                let y = geo.y.round() as i32;
                let width = geo.width.round().max(1.0) as u32;
                let height = geo.height.round().max(1.0) as u32;

                #[cfg(target_os = "windows")]
                {
                    use windows_sys::Win32::Graphics::Gdi::{
                        GetMonitorInfoW, MonitorFromRect, MONITORINFO,
                        MONITOR_DEFAULTTONEAREST,
                    };
                    use windows_sys::Win32::Foundation::{POINT, RECT};
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        GetWindowPlacement, IsZoomed, SetWindowPlacement, SetWindowPos,
                        ShowWindow, WINDOWPLACEMENT, SWP_NOACTIVATE, SWP_NOZORDER,
                        SWP_SHOWWINDOW, SW_RESTORE, SW_SHOWMAXIMIZED,
                    };

                    match win.hwnd() {
                        Ok(hwnd) => unsafe {
                            let requested = RECT {
                                left: x,
                                top: y,
                                right: x.saturating_add(width as i32),
                                bottom: y.saturating_add(height as i32),
                            };
                            let monitor = MonitorFromRect(&requested, MONITOR_DEFAULTTONEAREST);
                            if monitor.is_null() {
                                log::warn!(
                                    "[test-window] MonitorFromRect returned null for requested rect ({}, {}) {}x{}",
                                    x,
                                    y,
                                    width,
                                    height
                                );
                            } else {
                                let mut monitor_info = MONITORINFO {
                                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                                    rcMonitor: RECT {
                                        left: 0,
                                        top: 0,
                                        right: 0,
                                        bottom: 0,
                                    },
                                    rcWork: RECT {
                                        left: 0,
                                        top: 0,
                                        right: 0,
                                        bottom: 0,
                                    },
                                    dwFlags: 0,
                                };
                                if GetMonitorInfoW(monitor, &mut monitor_info) == 0 {
                                    log::warn!(
                                        "[test-window] GetMonitorInfoW failed for requested rect ({}, {}) {}x{}",
                                        x,
                                        y,
                                        width,
                                        height
                                    );
                                } else {
                                    log::info!(
                                        "[test-window] selected monitor rect=({}, {}) {}x{} work=({}, {}) {}x{} for requested rect ({}, {}) {}x{} maximized={}",
                                        monitor_info.rcMonitor.left,
                                        monitor_info.rcMonitor.top,
                                        monitor_info.rcMonitor.right - monitor_info.rcMonitor.left,
                                        monitor_info.rcMonitor.bottom - monitor_info.rcMonitor.top,
                                        monitor_info.rcWork.left,
                                        monitor_info.rcWork.top,
                                        monitor_info.rcWork.right - monitor_info.rcWork.left,
                                        monitor_info.rcWork.bottom - monitor_info.rcWork.top,
                                        x,
                                        y,
                                        width,
                                        height,
                                        geo.maximized
                                    );
                                }
                            }

                            if IsZoomed(hwnd.0 as _) != 0 || geo.maximized {
                                ShowWindow(hwnd.0 as _, SW_RESTORE);
                            }
                            let ok = SetWindowPos(
                                hwnd.0 as _,
                                std::ptr::null_mut(),
                                x,
                                y,
                                width as i32,
                                height as i32,
                                SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                            );
                            if ok == 0 {
                                log::warn!("[test-window] native SetWindowPos failed");
                            }
                            if geo.maximized {
                                let mut placement = WINDOWPLACEMENT {
                                    length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                                    flags: 0,
                                    showCmd: SW_SHOWMAXIMIZED as u32,
                                    ptMinPosition: POINT { x: -1, y: -1 },
                                    ptMaxPosition: POINT { x: -1, y: -1 },
                                    rcNormalPosition: requested,
                                };
                                if GetWindowPlacement(hwnd.0 as _, &mut placement) == 0 {
                                    log::warn!(
                                        "[test-window] GetWindowPlacement failed before maximize"
                                    );
                                }
                                placement.length =
                                    std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                                placement.showCmd = SW_SHOWMAXIMIZED as u32;
                                placement.rcNormalPosition = requested;
                                if SetWindowPlacement(hwnd.0 as _, &placement) == 0 {
                                    log::warn!("[test-window] SetWindowPlacement maximize failed");
                                    ShowWindow(hwnd.0 as _, SW_SHOWMAXIMIZED);
                                }
                            }
                            return true;
                        },
                        Err(e) => {
                            log::warn!("[test-window] failed to get HWND: {}", e);
                        }
                    }
                }

                if let Err(e) = win.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                    width,
                    height,
                })) {
                    log::warn!("[test-window] failed to set physical size: {}", e);
                }
                if let Err(e) = win.set_position(tauri::Position::Physical(
                    tauri::PhysicalPosition { x, y },
                )) {
                    log::warn!("[test-window] failed to set physical position: {}", e);
                }
                false
            }

            // Resolve main geometry: saved (physical) -> validate -> convert to logical -> fallback.
            // First-boot-after-upgrade users will have `main_geometry` seeded from legacy
            // `terminal_geometry` via the migration in `config::settings::load_settings`.
            let main_geo = if let Some(test_geo) = &test_window_placement {
                let requested = config::settings::WindowGeometry {
                    x: test_geo.x,
                    y: test_geo.y,
                    width: test_geo.width,
                    height: test_geo.height,
                };
                let logical = physical_to_logical(&requested, &monitors);
                log::info!(
                    "[test-window] requested physical ({}, {}) {}x{} maximized={} -> logical ({}, {}) {}x{}",
                    requested.x,
                    requested.y,
                    requested.width,
                    requested.height,
                    test_geo.maximized,
                    logical.x,
                    logical.y,
                    logical.width,
                    logical.height
                );
                logical
            } else {
                match &saved_settings.main_geometry {
                    Some(geo) if is_visible_on_monitors(geo, &monitors) => {
                        let logical = physical_to_logical(geo, &monitors);
                        log::info!(
                            "[window-setup] main: saved physical ({}, {}) {}x{} -> logical ({}, {}) {}x{}",
                            geo.x, geo.y, geo.width, geo.height,
                            logical.x, logical.y, logical.width, logical.height
                        );
                        logical
                    }
                    Some(geo) => {
                        log::warn!(
                            "[window-setup] main: saved geometry ({}, {}) {}x{} is off-screen, falling back to centered default",
                            geo.x, geo.y, geo.width, geo.height
                        );
                        default_main.clone()
                    }
                    None => {
                        log::info!("[window-setup] main: no saved geometry, using centered default");
                        default_main.clone()
                    }
                }
            };

            // Create the unified Main window (replaces sidebar + terminal windows).
            let main_win = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::App("index.html?window=main".into()),
            )
            .title(config::profile::app_title())
            .icon(icon)
            .expect("Failed to set main window icon")
            .min_inner_size(800.0, 500.0)
            .decorations(false)
            .zoom_hotkeys_enabled(false)
            .inner_size(main_geo.width, main_geo.height)
            .position(main_geo.x, main_geo.y)
            .build()?;

            if let Some(test_geo) = &test_window_placement {
                let native_handled = apply_test_window_placement(&main_win, test_geo);
                if test_geo.maximized && !native_handled {
                    if let Err(e) = main_win.maximize() {
                        log::warn!("[test-window] failed to maximize main window: {}", e);
                    }
                }
                log_main_window_info(&main_win);
            }

            if saved_settings.main_always_on_top {
                let _ = main_win.set_always_on_top(true);
            }

            // Suppress unused variable warning
            let _ = &main_win;

            // Restore sessions from last run
            //
            // §224 G-IMPL-1 — `persisted` and `restore_flag` are hoisted above
            // mailbox_poller.start() (see comment block there). `persisted` is
            // reused here; the flag is already TRUE when we enter this block.
            {
                use tauri::Manager;
                let session_mgr_clone = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>().inner().clone();
                let pty_mgr_clone = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
                let settings_state_clone = app.state::<SettingsState>().inner().clone();
                let app_handle = app.handle().clone();

                // #248 — read the new setting and always discover teams (the coord check
                // is run for every persisted session, regardless of the setting's value).
                let settings_snapshot = restore_settings_snapshot.clone();
                let setting_on = settings_snapshot.restore_coordinator_wake_state;
                let teams = crate::config::teams::discover_teams();

                // #248 Grinch Z10 — diagnostic: empty `teams` after a project-path rename
                // is a real failure mode; without this line the user sees coords stay
                // dormant with no log clue. Emits exactly once per launch.
                log::info!(
                    "[restore] {} teams discovered across {} project paths; setting_on={}; evaluating {} persisted sessions",
                    teams.len(),
                    settings_snapshot.project_paths.len(),
                    setting_on,
                    persisted.len()
                );

                // §224 A.2.5 — RAII guard inside the closure clears the flag
                // on normal exit AND on panic unwind so the daemon can't get
                // stuck advertising "still restoring" forever.
                //
                // §224 G-IMPL-1 — the upper hoisted block already set the flag
                // TRUE before mailbox_poller.start(); we only need to grab a
                // fresh Arc clone here for the RAII guard inside the spawned task.
                let restore_flag_for_task = app
                    .state::<Arc<RestoreInProgress>>()
                    .inner()
                    .clone();
                let restore_transaction_for_task = restore_transaction.clone();

                tauri::async_runtime::block_on(async move {
                    struct RestoreGuard(Arc<RestoreInProgress>);
                    impl Drop for RestoreGuard {
                        fn drop(&mut self) {
                            self.0
                                 .0
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    let _restore_guard = RestoreGuard(restore_flag_for_task);

                    let mut active_id = None;
                    let mut failed_recoverable: Vec<sessions_persistence::PersistedSession> = Vec::new();

                    // #248 Grinch Z5 — count outcomes for the end-of-restore summary line.
                    let mut n_woken: usize = 0;
                    let mut n_deferred: usize = 0;

                    let root_agent_path = match crate::config::root_agent::ensure_root_agent_dir() {
                        Ok(path) => Some(path),
                        Err(e) => {
                            log::error!("[root-agent] Failed to provision root agent during restore: {}", e);
                            None
                        }
                    };
                    let root_ps = persisted
                        .iter()
                        .find(|ps| {
                            ps.is_root_agent
                                || crate::config::root_agent::is_root_agent_path(
                                    &ps.working_directory,
                                )
                        })
                        .cloned();

                    if let Some(root_path) = root_agent_path.clone() {
                        match root_ps.as_ref() {
                            None => {
                                let last_coding_agent =
                                    crate::config::root_agent::read_last_coding_agent(&root_path);
                                let should_auto_create = {
                                    let cfg = settings_state_clone.read().await;
                                    should_auto_create_root_agent_on_first_restore(
                                        &cfg,
                                        last_coding_agent.as_deref(),
                                    )
                                };

                                if should_auto_create {
                                    match commands::session::execute_root_transaction(
                                        &restore_transaction_for_task,
                                        commands::session::RootJobRequest {
                                            requested_agent_id: None,
                                            requested_profile: None,
                                            skip_auto_resume_for_new_session: true,
                                            intent: crate::session::selection::TrustedCreateIntent::Background,
                                            select_after: false,
                                        },
                                    )
                                    .await {
                                        Ok(_) => n_woken += 1,
                                        Err(e) => log::error!(
                                            "[root-agent] Failed to auto-create root session: {}",
                                            e
                                        ),
                                    }
                                } else {
                                    log::info!(
                                        "[root-agent] Skipping startup auto-create: no resolvable coding agent is configured"
                                    );
                                }
                            }
                            Some(ps)
                                if should_wake_root_agent_on_restore(ps.status.as_ref()) =>
                            {
                                let existing_root = {
                                    let mgr = session_mgr_clone.read().await;
                                    mgr.list_sessions().await.into_iter().find(|s| {
                                        s.is_root_agent
                                            || crate::config::root_agent::is_root_agent_path(
                                                &s.working_directory,
                                            )
                                    })
                                };
                                let mut should_create = true;
                                if let Some(existing) = existing_root {
                                    if matches!(
                                        existing.status,
                                        crate::session::session::SessionStatus::Exited(_)
                                    ) {
                                        if let Ok(uuid) = uuid::Uuid::parse_str(&existing.id) {
                                            let stale_destroy = commands::session::execute_destroy_transaction(
                                                &restore_transaction_for_task,
                                                commands::session::DestroyRequest {
                                                    ids: vec![uuid],
                                                    source: commands::session::DestructionSource::BackgroundCleanup,
                                                    force_destroy_root: true,
                                                },
                                            )
                                            .await
                                            .and_then(|outcome| {
                                                outcome
                                                    .succeeded(uuid)
                                                    .then_some(())
                                                    .ok_or_else(|| "stale dormant Root was not destroyed".to_string())
                                            });
                                            if let Err(e) = stale_destroy {
                                                log::warn!(
                                                    "[root-agent] Failed to force-destroy stale dormant root during restore: {}",
                                                    e
                                                );
                                            }
                                        }
                                    } else {
                                        if ps.was_active {
                                            active_id = Some(existing.id.clone());
                                        }
                                        n_woken += 1;
                                        if let Ok(uuid) = uuid::Uuid::parse_str(&existing.id) {
                                            commands::session::attach_persisted_telegram_if_configured(
                                                &app_handle,
                                                uuid,
                                                ps.telegram_bot_id.as_deref(),
                                            )
                                            .await;
                                            if let Some(ref prompt) = ps.last_prompt {
                                                let mgr = session_mgr_clone.read().await;
                                                mgr.set_last_prompt(uuid, prompt.clone()).await;
                                            }
                                        }
                                        should_create = false;
                                    }
                                }
                                if should_create {
                                    let mut rebuild_failed = false;
                                    let resolved_spawn = if let Some(aid) = ps.agent_id.as_deref() {
                                        match commands::session::build_configured_agent_spawn_for_cwd(
                                            &settings_snapshot,
                                            aid,
                                            &root_path,
                                            ps.requested_profile.as_deref(),
                                        ) {
                                            Ok(spawn) => spawn,
                                            Err(e) => {
                                                log::error!(
                                                    "[root-agent] Failed to rebuild configured agent command for restore '{}': {}",
                                                    ps.name,
                                                    e
                                                );
                                                failed_recoverable.push(ps.clone());
                                                rebuild_failed = true;
                                                None
                                            }
                                        }
                                    } else {
                                        None
                                    };
                                    if !rebuild_failed {
                                    let (shell, shell_args, agent_label) =
                                        if let Some(spawn) = resolved_spawn.as_ref() {
                                            (
                                                spawn.shell.clone(),
                                                spawn.shell_args.clone(),
                                                Some(spawn.trusted_agent_label.clone()),
                                            )
                                        } else {
                                            (
                                                ps.shell.clone(),
                                                ps.shell_args.clone(),
                                                ps.agent_label.clone(),
                                            )
                                        };
                                    match commands::session::create_session_inner_for_restore(
                                        &restore_transaction_for_task,
                                        &session_mgr_clone,
                                        &pty_mgr_clone,
                                        shell,
                                        shell_args,
                                        root_path.clone(),
                                        Some(ps.name.clone()),
                                        ps.agent_id.clone(),
                                        agent_label,
                                        false,
                                        ps.git_repos.clone(),
                                        false,
                                        resolved_spawn,
                                        // #973 - headless caller: no terminal to measure, keep 120x30.
                                        None,
                                        Some(ps.start_fresh_on_restore),
                                        None,
                                    )
                                    .await
                                    {
                                        Ok(info) => {
                                            if ps.was_active {
                                                active_id = Some(info.id.clone());
                                            }
                                            n_woken += 1;
                                            if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                                commands::session::attach_persisted_telegram_if_configured(
                                                    &app_handle,
                                                    uuid,
                                                    ps.telegram_bot_id.as_deref(),
                                                )
                                                .await;
                                            }

                                            if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                                if let Some(ref prompt) = ps.last_prompt {
                                                    let mgr = session_mgr_clone.read().await;
                                                    mgr.set_last_prompt(uuid, prompt.clone()).await;
                                                }
                                            }

                                            if ps.was_detached {
                                                if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                                    {
                                                        let mgr = session_mgr_clone.read().await;
                                                        if let Some(ref geo) = ps.detached_geometry
                                                        {
                                                            mgr.set_detached_geometry(
                                                                uuid,
                                                                geo.clone(),
                                                            )
                                                            .await;
                                                        }
                                                    }

                                                    let detached_result =
                                                        commands::window::execute_detach_transaction(
                                                            &restore_transaction_for_task,
                                                            uuid,
                                                            ps.detached_geometry.clone(),
                                                            true,
                                                        )
                                                        .await;
                                                    if let Err(e) = detached_result {
                                                        log::warn!(
                                                            "[restore] detach_terminal_inner failed for root agent '{}': {} — session stays live (attached)",
                                                            ps.name,
                                                            e
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            log::error!(
                                                "[root-agent] Failed to restore root session '{}': {}",
                                                ps.name,
                                                e
                                            );
                                            failed_recoverable.push(ps.clone());
                                        }
                                    }
                                    }
                                }
                            }
                            Some(ps) => {
                                let existing_root = {
                                    let mgr = session_mgr_clone.read().await;
                                    mgr.list_sessions().await.into_iter().find(|s| {
                                        s.is_root_agent
                                            || crate::config::root_agent::is_root_agent_path(
                                                &s.working_directory,
                                            )
                                    })
                                };
                                if let Some(existing) = existing_root {
                                    if let Ok(uuid) = uuid::Uuid::parse_str(&existing.id) {
                                        let mgr = session_mgr_clone.read().await;
                                        commands::session::preserve_deferred_telegram_intent_if_valid(
                                            &mgr,
                                            &settings_state_clone,
                                            uuid,
                                            &ps.name,
                                            ps.telegram_bot_id.as_deref(),
                                        )
                                        .await;
                                        if let Some(ref prompt) = ps.last_prompt {
                                            mgr.set_last_prompt(uuid, prompt.clone()).await;
                                        }
                                    }
                                    if ps.was_active {
                                        active_id = Some(existing.id.clone());
                                    }
                                    n_deferred += 1;
                                } else {
                                    match restore_transaction_for_task
                                        .restore_dormant_inline(
                                            crate::session::selection::DormantRestoreRequest {
                                                persisted: ps.clone(),
                                                working_directory: root_path,
                                                is_coordinator: false,
                                                is_root_agent: true,
                                            },
                                        )
                                        .await
                                    {
                                        Ok(info) => {
                                            if ps.was_active {
                                                active_id = Some(info.id);
                                            }
                                            n_deferred += 1;
                                        }
                                        Err(e) => {
                                            log::error!(
                                                "[root-agent] Failed to create dormant root session '{}': {}",
                                                ps.name,
                                                e
                                            );
                                            failed_recoverable.push(ps.clone());
                                        }
                                    }
                                }
                            }
                        }
                    } else if let Some(ps) = root_ps.as_ref() {
                        failed_recoverable.push(ps.clone());
                    }

                    let archived_roots = sessions_persistence::normalize_project_roots(
                        &settings_snapshot.archived_project_paths,
                    );

                    for ps in &persisted {
                        if ps.is_root_agent
                            || crate::config::root_agent::is_root_agent_path(
                                &ps.working_directory,
                            )
                        {
                            continue;
                        }

                        // Skip sessions whose CWD no longer exists (permanent failure)
                        if !std::path::Path::new(&ps.working_directory).exists() {
                            log::warn!("Skipping restore of '{}': CWD '{}' no longer exists", ps.name, ps.working_directory);
                            continue;
                        }

                        // #248 — decide wake vs defer for this session.
                        // §DR2: use `agent_fqn_from_path` so WG replicas get project-precise
                        // team membership and coordinator checks. Strict `is_coordinator`
                        // (§AR2-strict) requires the FQN to avoid cross-project flag leaks.
                        let agent_name = crate::config::teams::agent_fqn_from_path(&ps.working_directory);
                        let live_is_coord = crate::config::teams::is_any_coordinator(&agent_name, &teams);
                        // (#630) Backstop a transient empty discover_teams() with the snapshot's
                        // persisted is_coordinator so a real coordinator is not silently downgraded
                        // to "deferred" when project paths were not ready at cold start.
                        let is_coord = resolve_is_coord_for_restore(
                            live_is_coord,
                            teams.is_empty(),
                            ps.is_coordinator,
                        );
                        let archived_session =
                            sessions_persistence::is_under_normalized_archived_roots(
                                &ps.working_directory,
                                &archived_roots,
                            );
                        let wake = restore_session_should_wake(
                            archived_session,
                            setting_on,
                            is_coord,
                            ps.status.as_ref(),
                        );

                        if !wake {
                            // Defer: create a dormant Session record (no PTY, status = Exited(0)).
                            match restore_transaction_for_task
                                .restore_dormant_inline(
                                    crate::session::selection::DormantRestoreRequest {
                                        persisted: ps.clone(),
                                        working_directory: ps.working_directory.clone(),
                                        is_coordinator: is_coord,
                                        is_root_agent: false,
                                    },
                                )
                                .await
                            {
                                Ok(info) => {
                                    // Grinch Z5 — debug, not info: under the new default every
                                    // persisted session lands here, and an info-level line per
                                    // session creates a "mass defer" wall in startup logs that
                                    // looks like an alarm. The end-of-loop info summary below
                                    // carries the load-bearing signal.
                                    log::debug!(
                                        "Deferred session '{}' on startup (agent: {}, is_coord: {}, setting: {}, persisted_status: {:?}, was_detached: {})",
                                        ps.name, agent_name, is_coord, setting_on, ps.status, ps.was_detached
                                    );
                                    n_deferred += 1;
                                    // Preserve `was_active` for the post-loop active-switch:
                                    // a deferred session can still be the persisted-active one.
                                    // The post-loop branching (Fix A) ensures `set_active_only`
                                    // is used (not `switch_session`), so the dormant status
                                    // survives selection.
                                    if restore_session_should_become_active(
                                        ps.was_active,
                                        archived_session,
                                    ) {
                                        active_id = Some(info.id);
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to create deferred session '{}': {}", ps.name, e);
                                    failed_recoverable.push(ps.clone());
                                }
                            }
                            continue;
                        }

                        // Wake: rebuild configured-agent sessions from the persisted recipe,
                        // while custom-shell records keep their materialized shell args.
                        let resolved_spawn = if let Some(aid) = ps.agent_id.as_deref() {
                            match commands::session::build_configured_agent_spawn_for_cwd(
                                &settings_snapshot,
                                aid,
                                &ps.working_directory,
                                ps.requested_profile.as_deref(),
                            ) {
                                Ok(spawn) => spawn,
                                Err(e) => {
                                    log::error!(
                                        "Failed to rebuild configured agent command for restore '{}': {}",
                                        ps.name,
                                        e
                                    );
                                    failed_recoverable.push(ps.clone());
                                    continue;
                                }
                            }
                        } else {
                            None
                        };
                        let (shell, shell_args, agent_label) =
                            if let Some(spawn) = resolved_spawn.as_ref() {
                                (
                                    spawn.shell.clone(),
                                    spawn.shell_args.clone(),
                                    Some(spawn.trusted_agent_label.clone()),
                                )
                            } else {
                                (
                                    ps.shell.clone(),
                                    ps.shell_args.clone(),
                                    ps.agent_label.clone(),
                                )
                            };

                        // Wake: full PTY restore inside the restore transaction.
                        match commands::session::create_session_inner_for_restore(
                            &restore_transaction_for_task,
                            &session_mgr_clone,
                            &pty_mgr_clone,
                            shell,
                            shell_args,
                            ps.working_directory.clone(),
                            Some(ps.name.clone()),
                            ps.agent_id.clone(),
                            agent_label,
                            false, // Persist tooling on restore
                            ps.git_repos.clone(),
                            skip_auto_resume_for_restore(ps.start_fresh_on_restore), // (#630/#631) resume unless restarted fresh
                            resolved_spawn,
                            // #973 - headless caller: no terminal to measure, keep 120x30.
                            None,
                            Some(ps.start_fresh_on_restore),
                            is_coord.then(|| {
                                crate::commands::session::carry_communication_for_restart(
                                    ps.communication.clone(),
                                    ps.start_fresh_on_restore,
                                )
                            }).flatten(),
                        ).await {
                            Ok(info) => {
                                if ps.was_active {
                                    active_id = Some(info.id.clone());
                                }
                                n_woken += 1;
                                // (#630/#631) Restore-decision trace (INFO during stabilization).
                                log::info!(
                                    "[restore] woke '{}' (is_coord={}, live_is_coord={}, start_fresh_on_restore={})",
                                    ps.name, is_coord, live_is_coord, ps.start_fresh_on_restore
                                );
                                if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                    commands::session::attach_persisted_telegram_if_configured(
                                        &app_handle,
                                        uuid,
                                        ps.telegram_bot_id.as_deref(),
                                    )
                                    .await;
                                }

                                if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                    if let Some(ref prompt) = ps.last_prompt {
                                        let mgr = session_mgr_clone.read().await;
                                        mgr.set_last_prompt(uuid, prompt.clone()).await;
                                    }
                                }

                                // Phase 3 restore: reconstruct detach state for the live session.
                                // Deferred sessions (handled above with a `continue`) never reach
                                // this branch, so R.9's "skip detached-window spawn for deferred"
                                // guard is enforced structurally by this code path.
                                if ps.was_detached {
                                    if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                                        // Restore geometry independently. The detach transaction
                                        // commits persisted intent only after the window and PTY
                                        // rechecks pass.
                                        {
                                            let mgr = session_mgr_clone.read().await;
                                            if let Some(ref geo) = ps.detached_geometry {
                                                mgr.set_detached_geometry(uuid, geo.clone()).await;
                                            }
                                        }

                                        let detached_result = commands::window::execute_detach_transaction(
                                            &restore_transaction_for_task,
                                            uuid,
                                            ps.detached_geometry.clone(),
                                            true,
                                        )
                                        .await;
                                        if let Err(e) = detached_result {
                                            log::warn!(
                                                "[restore] detach_terminal_inner failed for '{}': {} — session stays live (attached)",
                                                ps.name,
                                                e
                                            );
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to restore session '{}': {}", ps.name, e);
                                // Preserve for next startup attempt (CWD exists, transient failure)
                                failed_recoverable.push(ps.clone());
                            }
                        }
                    }

                    // #248 Grinch Z5 — load-bearing summary line. Replaces the per-session
                    // info noise demoted to debug above. Must be emitted BEFORE the post-loop
                    // active-switch block so the restore-decision summary is grouped with
                    // the restore log in chronological order, not interleaved with switch events.
                    log::info!(
                        "[restore] complete — {} woken, {} deferred (setting_on={}, total_evaluated={})",
                        n_woken, n_deferred, setting_on, persisted.len()
                    );

                    let persisted_target = active_id
                        .as_deref()
                        .and_then(|id| uuid::Uuid::parse_str(id).ok());
                    if let Err(error) = restore_transaction_for_task
                        .restore_selection_inline(persisted_target)
                        .await
                    {
                        log::error!(
                            "[restore] final canonical selection failed target={:?}: {}",
                            persisted_target,
                            error
                        );
                    }

                    // Persist restored sessions + failed-but-recoverable entries
                    let mgr: tokio::sync::RwLockReadGuard<'_, SessionManager> = session_mgr_clone.read().await;
                    sessions_persistence::persist_merging_failed(&mgr, &failed_recoverable).await;

                    if !failed_recoverable.is_empty() {
                        log::warn!(
                            "Session restore: {} sessions failed (preserved for next attempt): {:?}",
                            failed_recoverable.len(),
                            failed_recoverable.iter().map(|s| &s.name).collect::<Vec<_>>()
                        );
                    }
                });
            }

            restore_barrier.finish();
            restore_observer_barrier.mark_restore_complete()?;

            // These observers mutate session metadata or persistence directly.
            // Start them only after restore has completed, which is stricter than
            // merely placing restore first and prevents an intermediate snapshot.
            restore_observer_barrier.start("idle", || {
                idle_detector_for_setup.start(shutdown_for_setup.clone());
            })?;
            restore_observer_barrier.start("git", || {
                git_watcher.start(shutdown_for_setup.clone());
            })?;
            restore_observer_barrier.start("discovery", || {
                discovery_branch_watcher.start(shutdown_for_setup.clone());
            })?;

            resource_monitor::watchdog::start(
                (*resource_monitor_for_setup).clone(),
                app.state::<SettingsState>().inner().clone(),
                selection_coordinator_for_setup.clone(),
                shutdown_for_setup.clone(),
            );
            pty_mgr
                .lock()
                .unwrap()
                .start_container_pending_reaper(shutdown_for_setup.clone());

            app.state::<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>>()
                .start_artifact_cleanup();
            let mailbox_poller = phone::mailbox::MailboxPoller::new();
            mailbox_poller.start(app.handle().clone(), shutdown_for_setup.clone());
            loop_scheduler_for_setup
                .clone()
                .start(app.handle().clone(), shutdown_for_setup.clone());
            crate::session::auto_close::start(app.handle().clone(), shutdown_for_setup.clone());
            crate::loops::non_stop_watchdog::start(
                app.handle().clone(),
                non_stop_state_for_setup.clone(),
                shutdown_for_setup.clone(),
            );
            ui_automation_state_for_setup.start(app.handle().clone(), shutdown_for_setup.clone());

            let screenshot_hotkey = {
                let settings = app.state::<SettingsState>();
                tauri::async_runtime::block_on(async {
                    settings.read().await.screenshot_capture_hotkey.clone()
                })
            };
            if let Err(error) =
                crate::screenshot::register_configured_hotkey(app.handle(), &screenshot_hotkey)
            {
                log::warn!("[screenshot] global hotkey registration failed: {}", error);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::session::create_session,
            commands::session::destroy_session,
            commands::session::close_coordinator,
            commands::session::restart_session,
            commands::session::switch_session,
            commands::session::rename_session,
            commands::session::set_last_prompt,
            commands::session::list_sessions,
            commands::session::get_active_session,
            session::warnings::drain_session_warnings,
            commands::session::create_root_agent_session,
            commands::task::task_get_title,
            commands::task::task_set_title,
              commands::task::task_clean,
            commands::task::task_clean_at,
                        commands::pty::pty_write,
            commands::pty::pty_resize,
            commands::pty::get_screen_snapshot,
            commands::pty::get_session_context,
            commands::config::get_settings,
            commands::config::get_coding_agent_catalog,
            commands::config::list_reseedable_agent_commands,
            commands::config::reseed_coding_agent_default,
            commands::config::update_settings,
            commands::resource_monitor::get_resource_snapshot,
            commands::resource_monitor::kill_resource_group,
            commands::config::save_settings_draft,
            commands::config::set_terminal_snapshots_enabled,
            commands::config::update_coding_agent_profiles,
            commands::config::update_coding_agent_env_settings,
            commands::config::set_agent_default_profile,
            commands::config::set_instance_profile_override,
            commands::config::resolve_coding_agent_profile,
            commands::config::preview_coding_agent_profile_selection,
            commands::config::apply_coding_agent_profile_selection,
            commands::config::set_sounds_enabled,
            commands::config::set_theme_light,
            commands::config::set_main_resource_monitor_attached,
            commands::config::set_rail_collapse,
            commands::config::set_log_level,
            commands::config::get_update_status,
            commands::repos::search_repos,
            commands::repos::git_remote_url,
            commands::telegram::telegram_attach,
            commands::telegram::telegram_detach,
            commands::telegram::telegram_list_bridges,
            commands::telegram::telegram_get_bridge,
            commands::telegram::telegram_send_test,
            commands::telegram::telegram_send_image,
            commands::testability::ui_automation_enabled,
            commands::testability::ui_automation_frontend_ready,
            commands::testability::ui_automation_complete,
            commands::window::detach_terminal,
            commands::window::attach_terminal,
            commands::window::list_detached_sessions,
            commands::window::set_detached_geometry,
            commands::window::open_in_explorer,
            commands::window::open_guide_window,
            commands::window::open_spec_board_window,
            commands::window::open_resource_monitor_window,
            commands::window::dock_resource_monitor_window,
            commands::window::open_external_url,
            commands::window::focus_main_window,
            commands::spec_board::spec_board_new,
            commands::spec_board::spec_board_pick_open,
            commands::spec_board::spec_board_open,
            commands::spec_board::spec_board_save,
            commands::spec_board::spec_board_pick_save,
            commands::spec_board::spec_board_update_content,
            commands::spec_board::spec_board_list_snapshots,
            commands::spec_board::spec_board_checkout_snapshot,
            commands::spec_board::spec_board_apply_external,
            commands::spec_board::spec_board_keep_mine,
            commands::spec_board::spec_board_close,
            commands::phone::phone_send_message,
            commands::phone::phone_get_inbox,
            commands::phone::phone_list_agents,
            commands::phone::phone_ack_messages,
            commands::voice::voice_transcribe,
            commands::voice::voice_mark_recording,
            commands::voice::voice_had_typing,
            commands::config::save_debug_logs,
            commands::config::drain_error_logs,
            commands::config::open_web_remote,
            commands::config::start_api_server,
            commands::config::stop_api_server,
            commands::config::api_server_status,
            commands::config::mint_api_client,
            commands::config::start_web_server,
            commands::config::stop_web_server,
            commands::config::get_web_server_status,
            commands::config::get_web_server_owned_status,
            commands::config::get_instance_label,
            commands::config::fetch_home_markdown,
            commands::agent_creator::pick_folder,
            commands::agent_creator::create_agent_folder,
            commands::ac_discovery::discover_ac_agents,
            commands::ac_discovery::check_project_path,
            commands::ac_discovery::create_ac_project,
            commands::ac_discovery::open_project,
            commands::ac_discovery::new_project,
            commands::ac_discovery::remove_project,
            commands::ac_discovery::archive_project,
            commands::ac_discovery::unarchive_project,
            commands::ac_discovery::list_archived_projects,
            commands::ac_discovery::discover_project,
            commands::project_settings::get_project_groups,
            commands::project_settings::update_project_groups,
            commands::non_stop::non_stop_report,
            commands::ac_discovery::keep_custom_context_template,
            commands::ac_discovery::overwrite_context_template_with_default,
            commands::ac_discovery::get_replica_context_files,
            commands::ac_discovery::set_replica_context_files,
            commands::loops::create_loop,
            commands::loops::update_loop,
            commands::loops::delete_loop,
            commands::loops::toggle_loop,
            commands::loops::run_loop_now,
            commands::loops::get_loop_config,
            commands::loops::preview_loop_cron,
            commands::entity_creation::create_agent_matrix,
            commands::entity_creation::delete_agent_matrix,
            commands::entity_creation::list_all_agents,
            commands::entity_creation::create_team,
            commands::entity_creation::delete_team,
            commands::entity_creation::update_team,
            commands::entity_creation::get_team_config,
            commands::entity_creation::create_workgroup,
            commands::entity_creation::delete_workgroup,
            commands::entity_creation::sync_workgroup_repos,
            commands::role_templates::list_role_templates,
            commands::role_templates::get_agency_templates_status,
            commands::role_templates::update_agency_templates,
            commands::screenshot::screenshot_get_overlay_state,
            commands::screenshot::screenshot_confirm_selection,
            commands::screenshot::screenshot_cancel_capture,
            commands::screenshot::screenshot_get_hotkey_status,
            commands::screenshot::screenshot_reload_hotkey,
        ])
        .build(tauri::generate_context!())
        .expect("error while building application")
        .run({
            let detached_set = detached_sessions.clone();
            let spec_board_state = spec_board_state.clone();
            move |app_handle, event| match event {
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::Destroyed,
                    ..
                } => {
                    if label == "spec-board" {
                        let state = spec_board_state.clone();
                        tauri::async_runtime::spawn(async move {
                            commands::spec_board::spec_board_close_all(state).await;
                        });
                    }
                    // #566 - when the main window is destroyed (any close path:
                    // X, Alt+F4, programmatic, silent- or confirm-quit), close the
                    // Resource Monitor window so it cannot orphan and keep the app
                    // alive. No-op if it was never opened or already closed.
                    if label == "main" {
                        if let Some(rm) = app_handle.get_webview_window("resource-monitor") {
                            // G4: log on failure rather than swallow. A swallowed
                            // error would hide the exact orphan bug this fixes;
                            // mirrors the FE quit path's console.warn.
                            if let Err(e) = rm.destroy() {
                                log::warn!("[shutdown] RM window destroy failed: {e}");
                            }
                        }
                    }
                    // Detached-window destroyed (by any mechanism — X, Alt+F4, programmatic).
                    // Two jobs:
                    //   1) Clear from `DetachedSessionsState` — switch_session needs an
                    //      accurate view of which sessions have live windows.
                    //   2) Emit `terminal_attached` — frontend stores subscribed to this event
                    //      clear the id from `sessionsStore.detachedIds` (Phase 2+ only;
                    //      Phase 1 has no subscriber — the event is harmlessly dropped).
                    //
                    // DELIBERATELY ABSENT: we do NOT call `SessionManager::set_was_detached`
                    // here. That mutation is reserved for `detach_terminal_inner` (→true)
                    // and `attach_terminal` (→false) under Fix A (plan §A3.2 / NEW-3).
                    // Mirroring the clear here would reintroduce NEW-1: A3.7 quit path
                    // destroys every detached window → Destroyed fires N times → all
                    // `Session::was_detached` flipped to false → `persist_current_state`
                    // on `RunEvent::Exit` writes was_detached=false for every session →
                    // restart restores nothing detached. See plan §10 rule.
                    if let Some(id_no_dashes) = label.strip_prefix("terminal-") {
                        if id_no_dashes.len() == 32 {
                            let formatted = format!(
                                "{}-{}-{}-{}-{}",
                                &id_no_dashes[0..8],
                                &id_no_dashes[8..12],
                                &id_no_dashes[12..16],
                                &id_no_dashes[16..20],
                                &id_no_dashes[20..32],
                            );
                            if let Ok(uuid) = uuid::Uuid::parse_str(&formatted) {
                                {
                                    let mut set = detached_set.lock().unwrap();
                                    set.remove(&uuid);
                                }
                                let _ = tauri::Emitter::emit(
                                    app_handle,
                                    "terminal_attached",
                                    serde_json::json!({ "sessionId": formatted }),
                                );
                            }
                        }
                    }
                    // #714 screenshot overlay destroyed (user close, crash, or our
                    // own teardown): clear capture state and destroy sibling
                    // overlays. Idempotent — a no-op once state is already Idle.
                    if label.starts_with("screenshot-overlay-") {
                        let label = label.to_string();
                        let app = app_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            crate::screenshot::handle_overlay_window_destroyed(app, label).await;
                        });
                    }
                }
                tauri::RunEvent::Exit => {
                    // Cancel all active Telegram bridges before general shutdown
                    let bridge_shutdowns = {
                        let mut tg = tauri::async_runtime::block_on(tg_mgr_for_exit.lock());
                        tg.cancel_all()
                    };
                    for shutdown in bridge_shutdowns {
                        shutdown.abort_now();
                    }

                    // #1149 - close every open activity interval here. `trigger()`
                    // below stops the IdleDetector before anything else, so this
                    // is the only position where the session map is still
                    // populated AND the detector is still alive. Without it a
                    // clean exit would drop every open interval, which is the
                    // defect this issue names first.
                    //
                    // Reaching the manager needs the outer lock, and `block_on`
                    // is forbidden on this path, so take it with `try_read`, clone
                    // the manager out (it is an `Arc` over its own state) and drop
                    // the guard before spinning.
                    let manager_for_activity = session_mgr_for_exit
                        .try_read()
                        .ok()
                        .map(|guard| guard.clone());
                    let working_snapshot = match manager_for_activity {
                        Some(manager) => {
                            // Bounded at 500 ms, and it holds no lock the writer
                            // needs while it sleeps, so it can never starve the
                            // writer it waits on. It can only fail to observe a
                            // gap, and that failure is already correct: the
                            // consumer closes every open interval at `app_stop`'s
                            // timestamp regardless of enumeration.
                            let deadline = std::time::Instant::now()
                                + std::time::Duration::from_millis(500);
                            loop {
                                if let Some(rows) = manager.try_snapshot_working_sessions() {
                                    break Some(rows);
                                }
                                if std::time::Instant::now() >= deadline {
                                    break None;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(2));
                            }
                        }
                        // The outer lock has no production writer, so unreachable
                        // in practice; falls through to the degraded path.
                        None => None,
                    };
                    let activity_batch = match &working_snapshot {
                        Some(rows) => {
                            let mut batch: Vec<_> = rows
                                .iter()
                                .map(|row| {
                                    crate::config::activity_log::build_idle_from_snapshot(
                                        row,
                                        crate::config::activity_log::IdleReason::AppStop,
                                    )
                                })
                                .collect();
                            batch.push(crate::config::activity_log::build_app_stop(
                                true,
                                rows.len(),
                            ));
                            batch
                        }
                        // The degraded path is a designed outcome, not a fallback
                        // to optimise away: the spin may legitimately exhaust
                        // under teardown load, and the enumerated records are pure
                        // precision on top of a close that happens either way.
                        None => vec![crate::config::activity_log::build_app_stop(false, 0)],
                    };
                    // One open, write and close for all N+1 lines.
                    crate::config::activity_log::append_batch(&activity_batch);

                    // #632 B1 - trigger background-task shutdown FIRST so the resource
                    // watchdog stops dispatching NEW ticks and the idle detectors stop.
                    // (An already-dispatched spawn_blocking kill_group still runs;
                    // safety there rests on B2b's bounded set + kill_group's
                    // Terminating/Terminated idempotency guard, not on trigger().)
                    let snapshot_scanner_shutdown = phone::mailbox::MailboxPoller::
                        active_terminal_snapshot_shutdown_owner();
                    if let Some(owner) = &snapshot_scanner_shutdown {
                        owner.seal();
                    }
                    log::info!("[shutdown] Triggering background task shutdown (async, not awaited)...");
                    shutdown_for_exit.trigger();
                    let scanner_shutdown = match snapshot_scanner_shutdown {
                        Some(owner) => tauri::async_runtime::block_on(owner.seal_and_drain_until(
                            tokio::time::Instant::now()
                                + std::time::Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS),
                        )),
                        None => phone::terminal_snapshot::SnapshotScannerDrainResult {
                            terminal: true,
                            ..Default::default()
                        },
                    };
                    log::info!(
                        "[shutdown] terminal snapshot scanner drained joined={} aborted={} terminal={}",
                        scanner_shutdown.joined,
                        scanner_shutdown.aborted,
                        scanner_shutdown.terminal
                    );

                    let context_alert_monitor = app_handle
                        .try_state::<Arc<crate::session::context_alerts::ContextAlertMonitor>>()
                        .map(|monitor| Arc::clone(monitor.inner()));
                    if let Some(monitor) = context_alert_monitor.as_ref() {
                        monitor.request_close();
                    }

                    let selection_shutdown = tauri::async_runtime::block_on(
                        selection_coordinator_for_exit.close_and_join(),
                    );
                    log::info!("[shutdown] selection coordinator joined before global cleanup");

                    // Selection shutdown seals and accounts for session preparation before
                    // the alert actor consumes its sole join handle.
                    if let Some(monitor) = context_alert_monitor {
                        if let Err(error) = tauri::async_runtime::block_on(monitor.close_and_join()) {
                            log::warn!("[shutdown] context alert monitor join failed: {}", error);
                        }
                    }

                    // #632 A - kill every agent's Job Object: atomically terminates
                    // each jobbed session's whole descendant tree via the job handle.
                    // This is the durable, orphan-free guarantee for jobbed sessions.
                    let pty_mgr = app_handle.state::<Arc<Mutex<PtyManager>>>();
                    let pty_lock_budget =
                        std::time::Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS);
                    let container_backend = {
                        let deadline = std::time::Instant::now() + pty_lock_budget;
                        loop {
                            match pty_mgr.try_lock() {
                                Ok(guard) => break Some(guard.container_backend()),
                                Err(std::sync::TryLockError::Poisoned(error)) => {
                                    break Some(error.into_inner().container_backend());
                                }
                                Err(std::sync::TryLockError::WouldBlock)
                                    if std::time::Instant::now() < deadline =>
                                {
                                    std::thread::sleep(std::time::Duration::from_millis(2));
                                }
                                Err(std::sync::TryLockError::WouldBlock) => break None,
                            }
                        }
                    };
                    let mut container_shutdown = match container_backend {
                        Some(container_backend) => container_backend
                            .stop_all_started_containers_blocking(pty_lock_budget),
                        None => {
                            log::error!(
                                "[shutdown] PTY owner lock reached the global cleanup deadline before container ownership transfer"
                            );
                            crate::pty::container_backend::ContainerShutdownReport {
                                terminal: false,
                                retained: vec![
                                    "reason=global-pty-owner state=retained".to_string(),
                                ],
                            }
                        }
                    };
                    let jobs = {
                        let deadline = std::time::Instant::now() + pty_lock_budget;
                        loop {
                            match pty_mgr.try_lock() {
                                Ok(guard) => break Some(guard.kill_all_jobs()),
                                Err(std::sync::TryLockError::Poisoned(error)) => {
                                    break Some(error.into_inner().kill_all_jobs());
                                }
                                Err(std::sync::TryLockError::WouldBlock)
                                    if std::time::Instant::now() < deadline =>
                                {
                                    std::thread::sleep(std::time::Duration::from_millis(2));
                                }
                                Err(std::sync::TryLockError::WouldBlock) => break None,
                            }
                        }
                    };
                    let (jobs_killed, jobless_sessions) = match jobs {
                        Some(counts) => counts,
                        None => {
                            log::error!(
                                "[shutdown] PTY owner lock reached the job cleanup deadline state=retained"
                            );
                            container_shutdown.terminal = false;
                            container_shutdown
                                .retained
                                .push("reason=global-job-owner state=retained".to_string());
                            (0, 0)
                        }
                    };
                    log::info!(
                        "[shutdown] terminated {jobs_killed} agent job object(s); {jobless_sessions} session(s) had no job"
                    );

                    // #632 B2+B4 - run the identity reaper for accounting and as the
                    // backstop for job-less sessions, TIME-BOXED. Fresh-only targets
                    // (B2a) make it near-instant for jobbed sessions (their live tree is
                    // already dead). Abandoning it on timeout is safe for jobbed
                    // sessions; the job-less warning below covers the MED-2 residual.
                    let rm_for_cleanup = resource_monitor_for_exit.clone();
                    let cleanup = crate::shutdown::run_time_boxed(
                        std::time::Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS),
                        move || {
                            rm_for_cleanup.kill_all_owned_groups(
                                resource_monitor::ResourceKillReason::AppShutdown,
                            )
                        },
                    );
                    match &cleanup {
                        Ok(results) => {
                            for result in results {
                                if result.quarantined {
                                    log::warn!(
                                        "[shutdown] resource group {} quarantined during cleanup: {}",
                                        result.session_id,
                                        result.message
                                    );
                                }
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => log::warn!(
                            "[shutdown] resource cleanup exceeded {SHUTDOWN_CLEANUP_BUDGET_SECS}s budget; proceeding to exit (job objects already terminated jobbed trees)"
                        ),
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => log::error!(
                            "[shutdown] resource cleanup thread panicked before completing; jobbed trees were still terminated by the job kill"
                        ),
                    }
                    // #632 MED-2 - job-less sessions are reaper-only; if the reaper did
                    // not finish, their trees may be orphaned. Make it visible.
                    if cleanup.is_err() && jobless_sessions > 0 {
                        log::warn!(
                            "[shutdown] {jobless_sessions} session(s) had no Job Object and the reaper did not finish in budget; their process trees may be orphaned"
                        );
                    }

                    if shutdown_persistence_allowed(
                        selection_shutdown.persistence_safe,
                        container_shutdown.terminal,
                    ) {
                        log::info!("[shutdown] Persisting session state...");
                        let mgr_clone = session_mgr_for_exit.clone();
                        tauri::async_runtime::block_on(async move {
                            let mgr = mgr_clone.read().await;
                            sessions_persistence::persist_current_state(&mgr).await;
                        });
                        log::info!("[shutdown] Session state persisted, process exiting");
                    } else {
                        let retained = combined_shutdown_retained_diagnostics_with_scanner(
                            scanner_shutdown.retained,
                            selection_shutdown.retained,
                            container_shutdown.retained,
                        );
                        log::error!(
                            "[shutdown] final session persistence skipped because cleanup ownership is retained work=[{}]",
                            retained.join(", ")
                        );
                    }

                    // #552 flush the coordinator badge / auto-closed store on clean
                    // exit (the 60s tick can leave up to one tick of recency
                    // unpersisted). Sync snapshot + save; best-effort on poison.
                    {
                        let snap = coordinator_clocks_for_exit
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .snapshot();
                        if let Err(e) = crate::config::coordinator_clocks::save_map(&snap) {
                            log::warn!("[coordinator-clocks] exit flush failed: {}", e);
                        }
                    }

                    // Issue #231 + grinch G-LOW (#246): remove daemon.pid AFTER
                    // persist_current_state so a concurrent CLI invocation never
                    // observes NoPidFile while sessions.json is being rewritten.
                    // Still runs before process exit — subsequent CLI invocations
                    // see NoPidFile (not StalePidFile) once we return.
                    crate::config::daemon_pid::remove_pid_file();
                    ui_automation_state_for_exit.cleanup_session_file();
                }
                _ => {}
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_persisted_active_flags, resolve_is_coord_for_restore,
        restore_session_should_become_active, restore_session_should_wake,
        should_auto_create_root_agent_on_first_restore, should_wake_on_restore,
        should_wake_root_agent_on_restore, skip_auto_resume_for_restore, ApiServerHandle,
        ApiServerTask, ContextPatternSource, ContextSample, ContextSampleSink,
        PersistedActiveFlagNormalization, RestoreObserverStartBarrier, ScraperPatterns,
        ScraperSamples, SettingsState, WebServerHandle,
    };
    use crate::config::sessions_persistence::PersistedSession;
    use crate::config::settings::{AgentConfig, AppSettings};
    use crate::session::session::SessionStatus;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn api_test_addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    // Stage E (#1064) shutdown-decision conformance (plan section 10.4 items
    // 14/15/34). Stage E extends only lib.rs `#[cfg(test)]` coverage; the exit
    // wiring order itself is exercised by the selection/alert/scraper shutdown
    // tests. Persistence is allowed only when BOTH the selection tracker and the
    // container cleanup are terminal, and retained-owner diagnostics from both
    // shutdown halves are merged.
    #[test]
    fn shutdown_persistence_is_allowed_only_when_selection_and_container_are_terminal() {
        assert!(super::shutdown_persistence_allowed(true, true));
        assert!(!super::shutdown_persistence_allowed(false, true));
        assert!(!super::shutdown_persistence_allowed(true, false));
        assert!(!super::shutdown_persistence_allowed(false, false));
    }

    #[test]
    fn combined_shutdown_diagnostics_merge_selection_and_container_owners() {
        assert!(
            super::combined_shutdown_retained_diagnostics(Vec::new(), Vec::new()).is_empty(),
            "no retained owners yields no diagnostics"
        );
        let combined = super::combined_shutdown_retained_diagnostics(
            vec!["reason=blocking-seed-transaction-await state=retained".to_string()],
            vec!["reason=container-stop state=retained".to_string()],
        );
        assert_eq!(
            combined.len(),
            2,
            "each retained shutdown owner is represented once, got {combined:?}"
        );
        assert!(
            combined.iter().all(|entry| !entry.is_empty()),
            "retained diagnostics are non-empty"
        );
    }

    #[test]
    fn scanner_retained_owner_is_merged_without_request_content_or_paths() {
        let combined = super::combined_shutdown_retained_diagnostics_with_scanner(
            vec!["reason=terminal-snapshot-finalizer state=retained".to_string()],
            vec!["reason=selection-worker state=retained".to_string()],
            vec!["reason=container-stop state=retained".to_string()],
        );
        assert_eq!(combined.len(), 3);
        assert!(combined
            .iter()
            .any(|entry| entry.contains("owner=terminalSnapshotScanner")));
        assert!(combined.iter().all(|entry| !entry.contains('\\')));
        assert!(combined.iter().all(|entry| !entry.contains('/')));
    }

    fn settings_with_agent() -> AppSettings {
        AppSettings {
            agents: vec![AgentConfig {
                id: "codex".to_string(),
                label: "Codex".to_string(),
                command: "codex".to_string(),
                color: "#10b981".to_string(),
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                backend: Default::default(),
            }],
            ..AppSettings::default()
        }
    }

    fn settings_with_context_regex(regex: &str) -> AppSettings {
        let mut settings = settings_with_agent();
        settings.agents[0].context_regex = Some(regex.to_string());
        settings
    }

    fn resolved(settings: AppSettings) -> std::collections::HashMap<String, String> {
        let source = ScraperPatterns {
            settings: std::sync::Arc::new(tokio::sync::RwLock::new(settings)) as SettingsState,
        };
        futures::executor::block_on(source.patterns())
    }

    #[tokio::test]
    async fn sample_adapter_is_nonblocking_and_recovers_only_after_quarter_capacity() {
        let capacity = crate::session::context_alerts::CONTEXT_SAMPLE_QUEUE_CAPACITY;
        let (sender, mut receiver) = tokio::sync::mpsc::channel(capacity);
        let adapter = ScraperSamples {
            sender,
            closed_logged: AtomicBool::new(false),
            saturated: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
        };
        let sample = || ContextSample::Unavailable {
            session_id: uuid::Uuid::nil(),
        };

        for _ in 0..capacity {
            adapter.observe(sample());
        }
        adapter.observe(sample());
        assert!(adapter.saturated.load(Ordering::Relaxed));
        assert_eq!(adapter.dropped.load(Ordering::Relaxed), 1);

        receiver.recv().await.expect("one queued sample");
        adapter.observe(sample());
        assert!(
            adapter.saturated.load(Ordering::Relaxed),
            "a one-slot drain must not end the saturation episode"
        );

        for _ in 0..257 {
            receiver.recv().await.expect("queued sample to drain");
        }
        adapter.observe(sample());
        assert!(!adapter.saturated.load(Ordering::Relaxed));
        assert_eq!(adapter.dropped.load(Ordering::Relaxed), 0);
        assert!(adapter.sender.capacity() >= capacity / 4);

        drop(receiver);
        adapter.observe(sample());
        adapter.observe(sample());
        assert!(adapter.closed_logged.load(Ordering::Relaxed));
    }

    /// #1032 - the adapter must hand `compile` the string the user wrote, byte for byte.
    ///
    /// The pattern is the ONLY defence this feature has: the engine deliberately ships no
    /// anchoring rules of its own, so every rule that makes a reading trustworthy lives in
    /// the user's text. An engine that edits that text can only weaken it, and it does so
    /// silently, in the one place nobody thinks to look.
    ///
    /// The concrete loss this pins: `  Context [\u{2591}\u{2588}]+ (\d{1,3})%` is the natural
    /// transcription of a row the plan says ALWAYS starts at column 2 - copy the row, swap
    /// the bar and the number for classes. Trimming it deletes the column-2 anchor, and with
    /// the statusline suppressed the pattern then matches input-box prose and reports a
    /// confident 99% that is a lie. Failing open, in a design where everything else fails
    /// closed.
    #[test]
    fn the_adapter_hands_over_the_users_pattern_verbatim() {
        let user_wrote = "  Context [\u{2591}\u{2588}]+ (\\d{1,3})%";
        let patterns = resolved(settings_with_context_regex(user_wrote));

        assert_eq!(
            patterns.get("codex").map(String::as_str),
            Some(user_wrote),
            "the engine must not rewrite the user's regex, and leading spaces ARE the anchor"
        );
    }

    /// The consequence, end to end through the real adapter: what the user configured is
    /// what gets read, and it reports NO number rather than a wrong one.
    ///
    /// The grid here is the statusline-suppressed case (`/help`, autocomplete, a hook turned
    /// off), where the only `Context ... %` on screen is prose the user typed themselves.
    /// The column-2 anchor is the single thing that rejects it. Resolve the pattern through
    /// the adapter, compile it, run it: `None`. With the adapter trimming, this same row
    /// read `Some(99)` - a confident lie - which is why the string identity above matters.
    #[test]
    fn a_pattern_resolved_through_the_adapter_still_rejects_input_box_prose() {
        let user_wrote = "  Context [\u{2591}\u{2588}]+ (\\d{1,3})%";
        let patterns = resolved(settings_with_context_regex(user_wrote));
        let resolved_source = patterns.get("codex").expect("configured");

        let pattern = crate::pty::context_scrape::pattern::compile(resolved_source)
            .expect("the user's pattern compiles");
        let grid = vec![
            "\u{276f} The row says Context \u{2588}\u{2588}\u{2588}\u{2588}\u{2588} 99% right now"
                .to_string(),
        ];

        assert_eq!(
            crate::pty::context_scrape::rows::extract(&pattern, &grid),
            None,
            "no number beats a wrong number: the engine must not edit the only defence there is"
        );
    }

    /// Trailing whitespace is just as much the user's business: `%` then a space is a
    /// pattern that requires a space, and only the user knows whether their row has one.
    #[test]
    fn trailing_whitespace_in_a_pattern_is_the_users_business_too() {
        let user_wrote = "Context (\\d{1,3})% ";
        let patterns = resolved(settings_with_context_regex(user_wrote));

        assert_eq!(patterns.get("codex").map(String::as_str), Some(user_wrote));
    }

    /// A blank field is the field being blank, not a pattern. Skipped for log hygiene ONLY:
    /// `pattern::compile` already refuses "" and "   " (no capture group 1), so this cannot
    /// become "a pattern that matches everything" - it would merely warn on every change.
    #[test]
    fn a_blank_context_regex_is_treated_as_unconfigured() {
        assert!(resolved(settings_with_context_regex("")).is_empty());
        assert!(resolved(settings_with_context_regex("   ")).is_empty());
        assert!(
            resolved(settings_with_agent()).is_empty(),
            "None is unconfigured"
        );
    }

    #[test]
    fn web_and_api_server_handles_can_be_managed_together() {
        let _app = tauri::Builder::default()
            .any_thread()
            .manage(WebServerHandle::default())
            .manage(ApiServerHandle::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("web and api server handles must be distinct managed types");
    }

    #[test]
    fn restore_loop_normalizes_archived_roots_before_persisted_session_loop() {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
            .expect("read lib.rs");
        let production = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("production lib source");
        let hoist = production
            .find("let archived_roots = sessions_persistence::normalize_project_roots")
            .expect("archived root normalization");
        let loop_start = production
            .find("for ps in &persisted")
            .expect("persisted session loop");

        assert!(
            hoist < loop_start,
            "startup restore must normalize archived roots once before the session loop"
        );
    }

    fn persisted_row(name: &str, was_active: bool) -> PersistedSession {
        PersistedSession {
            name: name.to_string(),
            working_directory: format!("C:/restore/{name}"),
            was_active,
            ..PersistedSession::default()
        }
    }

    #[test]
    fn restore_active_flag_normalization_accepts_zero_flags() {
        let mut sessions = vec![
            persisted_row("first", false),
            persisted_row("second", false),
        ];
        assert_eq!(
            normalize_persisted_active_flags(&mut sessions),
            PersistedActiveFlagNormalization::Zero
        );
        assert!(sessions.iter().all(|session| !session.was_active));
    }

    #[test]
    fn restore_active_flag_normalization_keeps_exactly_one_target() {
        let mut sessions = vec![persisted_row("first", false), persisted_row("second", true)];
        assert_eq!(
            normalize_persisted_active_flags(&mut sessions),
            PersistedActiveFlagNormalization::One { index: 1 }
        );
        assert!(!sessions[0].was_active);
        assert!(sessions[1].was_active);
    }

    #[test]
    fn restore_active_flag_normalization_clears_all_conflicting_targets_stably() {
        let mut sessions = vec![persisted_row("first", true), persisted_row("second", true)];
        assert_eq!(
            normalize_persisted_active_flags(&mut sessions),
            PersistedActiveFlagNormalization::Multiple {
                identities: vec![
                    "0:first@C:/restore/first".to_string(),
                    "1:second@C:/restore/second".to_string(),
                ],
            }
        );
        assert!(sessions.iter().all(|session| !session.was_active));
    }

    #[test]
    fn idle_git_and_discovery_are_held_by_the_real_restore_completion_barrier() {
        let barrier = RestoreObserverStartBarrier::default();
        let starts = std::sync::Mutex::new(Vec::new());
        for producer in ["idle", "git", "discovery"] {
            assert!(barrier
                .start(producer, || starts.lock().unwrap().push(producer))
                .is_err());
        }
        assert!(starts.lock().unwrap().is_empty());

        barrier.mark_restore_admitted().unwrap();
        for producer in ["idle", "git", "discovery"] {
            assert!(barrier
                .start(producer, || starts.lock().unwrap().push(producer))
                .is_err());
        }
        assert!(starts.lock().unwrap().is_empty());

        barrier.mark_restore_complete().unwrap();
        for producer in ["idle", "git", "discovery"] {
            barrier
                .start(producer, || starts.lock().unwrap().push(producer))
                .unwrap();
        }
        assert_eq!(*starts.lock().unwrap(), ["idle", "git", "discovery"]);
    }

    #[tokio::test]
    async fn web_server_handle_reports_owned_bind_and_port() {
        let handle = WebServerHandle::default();
        let task = tauri::async_runtime::spawn(async {
            std::future::pending::<()>().await;
        });

        handle.store_owned("127.0.0.1".to_string(), 8765, task);

        assert!(handle.is_owned_running("127.0.0.1", 8765));
        assert!(!handle.is_owned_running("0.0.0.0", 8765));
        assert!(!handle.is_owned_running("127.0.0.1", 8766));

        assert!(handle.abort_running());
    }

    #[tokio::test]
    async fn web_server_handle_clears_finished_task() {
        let handle = WebServerHandle::default();
        let task = tauri::async_runtime::spawn(async {});

        handle.store_owned("127.0.0.1".to_string(), 8765, task);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert!(!handle.is_owned_running("127.0.0.1", 8765));
        assert!(!handle.abort_running());
    }

    #[tokio::test]
    async fn web_server_handle_abort_clears_owned_status() {
        let handle = WebServerHandle::default();
        let task = tauri::async_runtime::spawn(async {
            std::future::pending::<()>().await;
        });

        handle.store_owned("127.0.0.1".to_string(), 8765, task);

        assert!(handle.abort_running());
        assert!(!handle.is_owned_running("127.0.0.1", 8765));
    }

    #[tokio::test]
    async fn api_server_handle_shutdown_cancels_running_task() {
        let handle = ApiServerHandle::default();
        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();
        let join = tauri::async_runtime::spawn(async move {
            task_shutdown.cancelled().await;
        });

        assert!(handle
            .store_if_idle(ApiServerTask::new(join, shutdown, api_test_addr(9906)))
            .unwrap());
        assert!(handle.has_running().unwrap());
        assert_eq!(
            handle.running_bound_addr().unwrap(),
            Some(api_test_addr(9906))
        );
        assert!(handle
            .shutdown_running(Duration::from_secs(1))
            .await
            .unwrap());
        assert!(!handle.has_running().unwrap());
    }

    #[tokio::test]
    async fn api_server_handle_rejects_duplicate_running_task() {
        let handle = ApiServerHandle::default();
        let first_shutdown = CancellationToken::new();
        let first_task_shutdown = first_shutdown.clone();
        let first_join = tauri::async_runtime::spawn(async move {
            first_task_shutdown.cancelled().await;
        });
        assert!(handle
            .store_if_idle(ApiServerTask::new(
                first_join,
                first_shutdown,
                api_test_addr(9906),
            ))
            .unwrap());

        let second_shutdown = CancellationToken::new();
        let second_observer = second_shutdown.clone();
        let second_task_shutdown = second_shutdown.clone();
        let second_join = tauri::async_runtime::spawn(async move {
            second_task_shutdown.cancelled().await;
        });
        assert!(!handle
            .store_if_idle(ApiServerTask::new(
                second_join,
                second_shutdown,
                api_test_addr(9907),
            ))
            .unwrap());
        assert!(second_observer.is_cancelled());
        assert!(handle.has_running().unwrap());

        assert!(handle
            .shutdown_running(Duration::from_secs(1))
            .await
            .unwrap());
    }

    #[test]
    fn setting_off_always_defers() {
        assert!(!should_wake_on_restore(
            false,
            true,
            Some(&SessionStatus::Running)
        ));
        assert!(!should_wake_on_restore(false, false, None));
    }

    #[test]
    fn non_coord_always_defers_when_on() {
        assert!(!should_wake_on_restore(
            true,
            false,
            Some(&SessionStatus::Running)
        ));
    }

    #[test]
    fn coord_awake_at_shutdown_wakes_when_on() {
        assert!(should_wake_on_restore(
            true,
            true,
            Some(&SessionStatus::Running)
        ));
        assert!(should_wake_on_restore(
            true,
            true,
            Some(&SessionStatus::Idle)
        ));
        assert!(should_wake_on_restore(
            true,
            true,
            Some(&SessionStatus::Active)
        ));
    }

    #[test]
    fn coord_asleep_at_shutdown_defers_when_on() {
        assert!(!should_wake_on_restore(
            true,
            true,
            Some(&SessionStatus::Exited(0))
        ));
        assert!(!should_wake_on_restore(
            true,
            true,
            Some(&SessionStatus::Exited(137))
        ));
    }

    #[test]
    fn coord_unknown_status_fails_open_when_on() {
        assert!(should_wake_on_restore(true, true, None));
    }

    #[test]
    fn archived_project_session_is_forced_dormant_on_restore_decision() {
        assert!(!restore_session_should_wake(
            true,
            true,
            true,
            Some(&SessionStatus::Running)
        ));
        assert!(restore_session_should_wake(
            false,
            true,
            true,
            Some(&SessionStatus::Running)
        ));
    }

    #[test]
    fn archived_project_session_is_never_adopted_as_active_on_restore() {
        assert!(!restore_session_should_become_active(true, true));
        assert!(restore_session_should_become_active(true, false));
        assert!(!restore_session_should_become_active(false, false));
    }

    #[test]
    fn root_agent_live_or_legacy_status_wakes() {
        assert!(should_wake_root_agent_on_restore(Some(
            &SessionStatus::Running
        )));
        assert!(should_wake_root_agent_on_restore(Some(
            &SessionStatus::Idle
        )));
        assert!(should_wake_root_agent_on_restore(Some(
            &SessionStatus::Active
        )));
        assert!(should_wake_root_agent_on_restore(None));
    }

    #[test]
    fn root_agent_exited_status_stays_dormant() {
        assert!(!should_wake_root_agent_on_restore(Some(
            &SessionStatus::Exited(0)
        )));
        assert!(!should_wake_root_agent_on_restore(Some(
            &SessionStatus::Exited(137)
        )));
    }

    #[test]
    fn first_restore_does_not_auto_create_root_agent_without_agents() {
        let settings = AppSettings::default();

        assert!(!should_auto_create_root_agent_on_first_restore(
            &settings, None
        ));
    }

    // (#630) Backstop truth table: a real coordinator survives a transient empty
    // discover_teams(); a healthy non-empty discovery is trusted as-is.
    #[test]
    fn resolve_is_coord_for_restore_truth_table() {
        // Empty discovery + persisted coord => backstop wakes the real coord.
        assert!(resolve_is_coord_for_restore(false, true, true));
        // Healthy discovery (non-empty) that no longer lists this agent => trust it.
        assert!(!resolve_is_coord_for_restore(false, false, true));
        // Live discovery already says coord => trust it regardless of the rest.
        assert!(resolve_is_coord_for_restore(true, false, false));
        assert!(resolve_is_coord_for_restore(true, true, false));
        // Empty discovery + not a persisted coord => stays deferred.
        assert!(!resolve_is_coord_for_restore(false, true, false));
    }

    // (#630/#631) The wake path passes the persisted fresh intent straight through
    // as create_session_inner's skip_auto_resume. Guards the lib.rs read seam: if
    // this identity is ever broken, restore stops honoring "Restart Session".
    #[test]
    fn wake_path_passes_persisted_fresh_intent() {
        assert!(skip_auto_resume_for_restore(true)); // restarted fresh => suppress --continue
        assert!(!skip_auto_resume_for_restore(false)); // default => resume
    }

    #[test]
    fn first_restore_auto_creates_root_agent_with_configured_agent() {
        let settings = settings_with_agent();

        assert!(should_auto_create_root_agent_on_first_restore(
            &settings, None
        ));
    }

    #[test]
    fn first_restore_auto_creates_root_agent_with_valid_last_coding_agent() {
        let settings = settings_with_agent();

        assert!(should_auto_create_root_agent_on_first_restore(
            &settings,
            Some("codex")
        ));
    }
}
