pub mod agent_update;
pub mod agent_version;
pub mod api;
pub mod capture;
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
pub mod test_support;
pub mod testability;
pub mod update_check;
pub mod voice;
pub mod web;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use commands::ac_discovery::DiscoveryBranchWatcher;
use config::sessions_persistence;
use config::settings::SettingsState;
use futures_util::FutureExt;
use pty::context_scrape::{
    ContextEventSink, ContextPatternSource, ContextPersistSink, ContextSample, ContextSampleSink,
    ContextScraper, ContextSessionLiveness, ContextUsagePayload, ScreenRowsRead, ScreenRowsSource,
};
use pty::git_watcher::GitWatcher;
use pty::idle_detector::IdleDetector;
use pty::manager::PtyManager;
use pty::watchers::{
    SessionFrameReader, WatcherBackendSource, WatcherEngine, WatcherEventSink, WatcherPatternSource,
};
use session::manager::SessionManager;
use shutdown::ShutdownSignal;
use tauri::{Emitter, Manager};
use telegram::manager::{OutputSenderMap, TelegramBridgeManager, TelegramBridgeState};
use tokio::sync::watch;
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

const WEB_SERVER_STOP_TIMEOUT_MS: u64 = 5_000;
pub(crate) const WEB_SERVER_START_CANCELLED: &str = "Web server start cancelled by stop";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebServerLifecycle {
    Stopped,
    Starting,
    Running,
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebServerLifecycleSnapshot {
    pub generation: Option<u64>,
    pub revision: u64,
    pub lifecycle: WebServerLifecycle,
    pub endpoint: Option<(String, u16)>,
}

struct WebServerGeneration {
    generation: u64,
    revision: u64,
    lifecycle: WebServerLifecycle,
    endpoint: Option<(String, u16)>,
    generation_token: CancellationToken,
    shutdown: ShutdownSignal,
    admission: Arc<web::WebSocketAdmission>,
    start_result: watch::Receiver<Option<Result<bool, String>>>,
    stop_result: watch::Receiver<Option<Result<(), String>>>,
    stop_deadline: Option<Instant>,
}

#[derive(Default)]
struct WebServerControl {
    next_generation: u64,
    stopped_revision: u64,
    last_generation: Option<u64>,
    current: Option<WebServerGeneration>,
}

#[derive(Clone)]
pub struct WebServerStartWaiter {
    receiver: watch::Receiver<Option<Result<bool, String>>>,
}

impl WebServerStartWaiter {
    pub async fn wait(mut self) -> Result<bool, String> {
        loop {
            if let Some(result) = self.receiver.borrow().clone() {
                return result;
            }
            self.receiver.changed().await.map_err(|_| {
                "Web server start supervisor channel closed before terminal result".to_string()
            })?;
        }
    }
}

#[derive(Clone)]
pub struct WebServerStopWaiter {
    receiver: watch::Receiver<Option<Result<(), String>>>,
    deadline: Instant,
}

impl WebServerStopWaiter {
    pub async fn wait(mut self) -> Result<(), String> {
        let wait = async {
            loop {
                if let Some(result) = self.receiver.borrow().clone() {
                    return result;
                }
                self.receiver.changed().await.map_err(|_| {
                    "Web server stop supervisor channel closed before terminal state".to_string()
                })?;
            }
        };
        tokio::time::timeout_at(self.deadline.into(), wait)
            .await
            .map_err(|_| "Timed out waiting for web server generation to stop".to_string())?
    }

    #[cfg(test)]
    fn deadline(&self) -> Instant {
        self.deadline
    }
}

#[derive(Clone, Default)]
pub struct WebServerHandle {
    inner: Arc<Mutex<WebServerControl>>,
    /// §1453: ultimo arranque fallido (autostart o comando). Lo consume
    /// get_web_server_owned_status; lo limpian el start exitoso y el stop.
    bind_failure: Arc<std::sync::Mutex<Option<crate::web::StartServerError>>>,
    #[cfg(test)]
    start_output_gate: Arc<Mutex<Option<WebServerStartOutputGate>>>,
}

#[cfg(test)]
struct WebServerStartOutputGate {
    reached: tokio::sync::oneshot::Sender<bool>,
    release: tokio::sync::oneshot::Receiver<()>,
}

impl WebServerHandle {
    pub(crate) fn begin_start<F, Fut>(
        &self,
        shutdown: ShutdownSignal,
        factory: F,
    ) -> WebServerStartWaiter
    where
        F: FnOnce(u64, Arc<web::WebSocketAdmission>, CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<tauri::async_runtime::JoinHandle<()>>, String>>
            + Send
            + 'static,
    {
        let mut factory = Some(factory);
        let mut supervisor = None;
        let waiter = {
            let mut inner = self.inner.lock().unwrap();
            match inner.current.as_ref() {
                Some(generation) if generation.lifecycle == WebServerLifecycle::Starting => {
                    WebServerStartWaiter {
                        receiver: generation.start_result.clone(),
                    }
                }
                Some(generation) if generation.lifecycle == WebServerLifecycle::Running => {
                    Self::completed_start_waiter(Ok(true))
                }
                Some(_) => Self::completed_start_waiter(Err(WEB_SERVER_START_CANCELLED.into())),
                None => {
                    inner.next_generation = inner
                        .next_generation
                        .checked_add(1)
                        .expect("web server generation id exhausted");
                    let generation = inner.next_generation;
                    let revision = inner
                        .stopped_revision
                        .checked_add(1)
                        .expect("web server lifecycle revision exhausted");
                    let generation_token = CancellationToken::new();
                    let admission =
                        Arc::new(web::WebSocketAdmission::new(generation_token.clone()));
                    let (start_sender, start_result) = watch::channel(None);
                    let (stop_sender, stop_result) = watch::channel(None);
                    let result = WebServerStartWaiter {
                        receiver: start_result.clone(),
                    };
                    inner.current = Some(WebServerGeneration {
                        generation,
                        revision,
                        lifecycle: WebServerLifecycle::Starting,
                        endpoint: None,
                        generation_token: generation_token.clone(),
                        shutdown: shutdown.clone(),
                        admission: Arc::clone(&admission),
                        start_result,
                        stop_result,
                        stop_deadline: None,
                    });
                    supervisor = Some((
                        generation,
                        admission,
                        generation_token,
                        shutdown,
                        start_sender,
                        stop_sender,
                        factory.take().expect("new generation owns start factory"),
                    ));
                    result
                }
            }
        };

        if let Some((
            generation,
            admission,
            generation_token,
            shutdown,
            start_sender,
            stop_sender,
            factory,
        )) = supervisor
        {
            let handle = self.clone();
            tauri::async_runtime::spawn(async move {
                handle
                    .supervise_generation(
                        generation,
                        admission,
                        generation_token,
                        shutdown,
                        start_sender,
                        stop_sender,
                        factory,
                    )
                    .await;
            });
        }

        waiter
    }

    fn completed_start_waiter(result: Result<bool, String>) -> WebServerStartWaiter {
        let (_sender, receiver) = watch::channel(Some(result));
        WebServerStartWaiter { receiver }
    }

    pub(crate) fn begin_stop(&self) -> Option<WebServerStopWaiter> {
        self.begin_stop_with_timeout(Duration::from_millis(WEB_SERVER_STOP_TIMEOUT_MS))
    }

    fn begin_stop_with_timeout(&self, timeout: Duration) -> Option<WebServerStopWaiter> {
        let mut inner = self.inner.lock().unwrap();
        let Some(generation) = inner.current.as_mut() else {
            self.clear_bind_failure();
            return None;
        };
        if matches!(
            generation.lifecycle,
            WebServerLifecycle::Starting | WebServerLifecycle::Running
        ) {
            generation.lifecycle = WebServerLifecycle::Stopping;
            generation.revision = generation
                .revision
                .checked_add(1)
                .expect("web server lifecycle revision exhausted");
            generation.admission.close();
            generation.generation_token.cancel();
        }
        let deadline = *generation
            .stop_deadline
            .get_or_insert_with(|| Instant::now() + timeout);
        Some(WebServerStopWaiter {
            receiver: generation.stop_result.clone(),
            deadline,
        })
    }

    pub fn publish_effective_endpoint(&self, generation: u64, bind: String, port: u16) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(current) = inner.current.as_mut() else {
            return false;
        };
        if current.generation != generation
            || !matches!(
                current.lifecycle,
                WebServerLifecycle::Starting | WebServerLifecycle::Stopping
            )
        {
            return false;
        }
        let endpoint = (bind, port);
        if current.endpoint.as_ref() != Some(&endpoint) {
            current.endpoint = Some(endpoint);
            current.revision = current
                .revision
                .checked_add(1)
                .expect("web server lifecycle revision exhausted");
        }
        current.lifecycle == WebServerLifecycle::Starting
    }

    pub fn snapshot(&self) -> WebServerLifecycleSnapshot {
        let inner = self.inner.lock().unwrap();
        Self::snapshot_locked(&inner)
    }

    pub fn snapshot_is_current(&self, snapshot: &WebServerLifecycleSnapshot) -> bool {
        let inner = self.inner.lock().unwrap();
        Self::snapshot_locked(&inner) == *snapshot
    }

    fn snapshot_locked(inner: &WebServerControl) -> WebServerLifecycleSnapshot {
        match inner.current.as_ref() {
            Some(generation) => WebServerLifecycleSnapshot {
                generation: Some(generation.generation),
                revision: generation.revision,
                lifecycle: generation.lifecycle,
                endpoint: generation.endpoint.clone(),
            },
            None => WebServerLifecycleSnapshot {
                generation: inner.last_generation,
                revision: inner.stopped_revision,
                lifecycle: WebServerLifecycle::Stopped,
                endpoint: None,
            },
        }
    }

    fn publish_running(&self, generation: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(current) = inner.current.as_mut() else {
            return false;
        };
        if current.generation != generation
            || current.lifecycle != WebServerLifecycle::Starting
            || current.endpoint.is_none()
            || current.generation_token.is_cancelled()
            || current.shutdown.is_cancelled()
        {
            return false;
        }

        current.lifecycle = WebServerLifecycle::Running;
        current.revision = current
            .revision
            .checked_add(1)
            .expect("web server lifecycle revision exhausted");
        if current.admission.open(&current.shutdown) {
            true
        } else {
            current.lifecycle = WebServerLifecycle::Stopping;
            current.revision = current
                .revision
                .checked_add(1)
                .expect("web server lifecycle revision exhausted");
            current.admission.close();
            current.generation_token.cancel();
            false
        }
    }

    fn move_generation_to_stopping(&self, generation: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(current) = inner.current.as_mut() else {
            return false;
        };
        if current.generation != generation {
            return false;
        }
        if current.lifecycle != WebServerLifecycle::Stopping {
            current.lifecycle = WebServerLifecycle::Stopping;
            current.revision = current
                .revision
                .checked_add(1)
                .expect("web server lifecycle revision exhausted");
        }
        current.admission.close();
        current.generation_token.cancel();
        true
    }

    fn transition_start_result_to_stopping(
        &self,
        generation: u64,
        result_if_starting: Result<bool, String>,
    ) -> Result<bool, String> {
        let mut inner = self.inner.lock().unwrap();
        let Some(current) = inner.current.as_mut() else {
            return Err(WEB_SERVER_START_CANCELLED.to_string());
        };
        if current.generation != generation {
            return Err(WEB_SERVER_START_CANCELLED.to_string());
        }

        let result = if current.lifecycle == WebServerLifecycle::Starting {
            result_if_starting
        } else {
            Err(WEB_SERVER_START_CANCELLED.to_string())
        };
        if current.lifecycle != WebServerLifecycle::Stopping {
            current.lifecycle = WebServerLifecycle::Stopping;
            current.revision = current
                .revision
                .checked_add(1)
                .expect("web server lifecycle revision exhausted");
        }
        current.admission.close();
        current.generation_token.cancel();
        result
    }

    #[cfg(test)]
    fn gate_next_start_output(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<bool>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (reached_sender, reached_receiver) = tokio::sync::oneshot::channel();
        let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
        let previous = self
            .start_output_gate
            .lock()
            .unwrap()
            .replace(WebServerStartOutputGate {
                reached: reached_sender,
                release: release_receiver,
            });
        assert!(
            previous.is_none(),
            "only one start-output gate may be installed"
        );
        (reached_receiver, release_sender)
    }

    #[cfg(test)]
    async fn pause_after_start_output(&self, generation: u64) {
        let gate = self.start_output_gate.lock().unwrap().take();
        let Some(gate) = gate else {
            return;
        };
        let was_starting = {
            let inner = self.inner.lock().unwrap();
            inner.current.as_ref().is_some_and(|current| {
                current.generation == generation
                    && current.lifecycle == WebServerLifecycle::Starting
            })
        };
        let _ = gate.reached.send(was_starting);
        let _ = gate.release.await;
    }

    fn finish_generation(&self, generation: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(current) = inner.current.as_ref() else {
            return false;
        };
        if current.generation != generation {
            return false;
        }
        let revision = current
            .revision
            .checked_add(1)
            .expect("web server lifecycle revision exhausted");
        if current.stop_deadline.is_some() {
            self.clear_bind_failure();
        }
        inner.current = None;
        inner.last_generation = Some(generation);
        inner.stopped_revision = revision;
        true
    }

    #[allow(clippy::too_many_arguments)]
    async fn supervise_generation<F, Fut>(
        &self,
        generation: u64,
        admission: Arc<web::WebSocketAdmission>,
        generation_token: CancellationToken,
        shutdown: ShutdownSignal,
        start_sender: watch::Sender<Option<Result<bool, String>>>,
        stop_sender: watch::Sender<Option<Result<(), String>>>,
        factory: F,
    ) where
        F: FnOnce(u64, Arc<web::WebSocketAdmission>, CancellationToken) -> Fut,
        Fut: Future<Output = Result<Option<tauri::async_runtime::JoinHandle<()>>, String>>,
    {
        let start_future = AssertUnwindSafe(factory(
            generation,
            Arc::clone(&admission),
            generation_token.clone(),
        ))
        .catch_unwind();
        tokio::pin!(start_future);

        let start_output = tokio::select! {
            result = &mut start_future => result,
            _ = shutdown.token().cancelled() => {
                self.move_generation_to_stopping(generation);
                start_future.await
            }
        };

        let mut server = match start_output {
            Ok(Ok(Some(server))) => Some(server),
            Ok(Ok(None)) => {
                #[cfg(test)]
                self.pause_after_start_output(generation).await;
                let result = self.transition_start_result_to_stopping(generation, Ok(false));
                let _ = start_sender.send(Some(result));
                None
            }
            Ok(Err(error)) => {
                #[cfg(test)]
                self.pause_after_start_output(generation).await;
                let result = self.transition_start_result_to_stopping(generation, Err(error));
                let _ = start_sender.send(Some(result));
                None
            }
            Err(_) => {
                #[cfg(test)]
                self.pause_after_start_output(generation).await;
                let result = self.transition_start_result_to_stopping(
                    generation,
                    Err("Web server start supervisor panicked".to_string()),
                );
                let _ = start_sender.send(Some(result));
                None
            }
        };

        if let Some(join) = server.as_mut() {
            if self.publish_running(generation) {
                let _ = start_sender.send(Some(Ok(true)));
            } else {
                self.move_generation_to_stopping(generation);
                let _ = start_sender.send(Some(Err(WEB_SERVER_START_CANCELLED.to_string())));
            }
            if let Err(error) = join.await {
                log::error!("[web-server] server task join failed: {}", error);
            }
        }

        self.move_generation_to_stopping(generation);
        admission.wait().await;
        if self.finish_generation(generation) {
            let _ = stop_sender.send(Some(Ok(())));
        }
    }

    pub fn record_bind_failure(&self, failure: crate::web::StartServerError) {
        *self.bind_failure.lock().unwrap() = Some(failure);
    }

    pub fn clear_bind_failure(&self) {
        *self.bind_failure.lock().unwrap() = None;
    }

    pub fn last_bind_failure(&self) -> Option<crate::web::StartServerError> {
        self.bind_failure.lock().unwrap().clone()
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

#[derive(Debug)]
pub struct StartupError {
    kind: StartupErrorKind,
}

#[derive(Debug)]
enum StartupErrorKind {
    Config(config::ConfigStartupError),
    AppOutboxCreate {
        config_dir: PathBuf,
        app_outbox_path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            StartupErrorKind::Config(error) => error.fmt(formatter),
            StartupErrorKind::AppOutboxCreate {
                config_dir,
                app_outbox_path,
                source,
            } => write!(
                formatter,
                "AgentsCommander cannot start because it could not create app outbox directory \"{}\" for configuration directory \"{}\": {}. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.",
                app_outbox_path.display(),
                config_dir.display(),
                source
            ),
        }
    }
}

impl std::error::Error for StartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            StartupErrorKind::Config(error) => Some(error),
            StartupErrorKind::AppOutboxCreate { source, .. } => Some(source),
        }
    }
}

pub fn preflight_config_startup() -> Result<(), StartupError> {
    match config::config_startup_error() {
        Some(error) => Err(StartupError {
            kind: StartupErrorKind::Config(error),
        }),
        None => Ok(()),
    }
}

fn prepare_app_outbox(
    config_dir: &Path,
    instance_id: &str,
) -> Result<(PathBuf, AppOutbox), StartupError> {
    let app_outbox_path = config_dir
        .join(crate::config::instance_artifacts::INSTANCES_DIR_NAME)
        .join(instance_id)
        .join("outbox");
    std::fs::create_dir_all(&app_outbox_path).map_err(|source| StartupError {
        kind: StartupErrorKind::AppOutboxCreate {
            config_dir: config_dir.to_path_buf(),
            app_outbox_path: app_outbox_path.clone(),
            source,
        },
    })?;
    let app_outbox = AppOutbox::new(app_outbox_path.to_string_lossy().to_string());
    Ok((app_outbox_path, app_outbox))
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

/// (#1793) The non-coordinator arm of the restore wake decision. #248's
/// `should_wake_on_restore` covers coordinators and is left unchanged. This is
/// the ONLY rule that can wake a non-coordinator, and the two arms are disjoint
/// on `is_coord`, so they can never both fire for one row.
pub(crate) fn should_wake_working_agent_on_restore(
    resume_agents_on: bool,
    is_coord: bool,
    persisted_working: bool,
) -> bool {
    resume_agents_on && !is_coord && persisted_working
}

pub(crate) fn restore_session_should_wake(
    archived_session: bool,
    setting_on: bool,
    resume_agents_on: bool,
    is_coord: bool,
    persisted_status: Option<&crate::session::session::SessionStatus>,
    persisted_working: bool,
) -> bool {
    !archived_session
        && (should_wake_on_restore(setting_on, is_coord, persisted_status)
            || should_wake_working_agent_on_restore(resume_agents_on, is_coord, persisted_working))
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
            crate::config::sessions_persistence::persist_current_state_prune_dormant(&guard).await;
        })
    }
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
struct WatcherBackends {
    pty_mgr: Arc<Mutex<PtyManager>>,
    /// A poisoned `PtyManager` is app-wide and permanent, so the warning is worth exactly one
    /// line, not one per registered session five times a second.
    poison_logged: AtomicBool,
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
struct WatcherPatterns {
    settings: SettingsState,
    /// Log-once bookkeeping for the resolution notices, so a configuration that stays wrong
    /// costs one line rather than five per second.
    log: crate::pty::watchers::ResolutionLog,
}

impl WatcherPatternSource for WatcherPatterns {
    fn resolve(
        &self,
    ) -> futures::future::BoxFuture<'_, HashMap<String, crate::pty::watchers::AgentResolution>>
    {
        Box::pin(async move {
            let settings = self.settings.read().await;

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
            resolved
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
struct WatcherDelivery {
    window_present: Arc<dyn Fn() -> bool + Send + Sync>,
    emit: Arc<dyn Fn(crate::pty::watchers::WatcherMatchBatch) + Send + Sync>,
}

impl WatcherDelivery {
    fn to_watchers_window(app_handle: tauri::AppHandle) -> Self {
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
struct WatcherSink {
    history: crate::pty::watchers::history::WatcherHistoryState,
    delivery: WatcherDelivery,
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

#[cfg(test)]
mod watcher_sink_tests {
    use super::*;
    use crate::pty::watchers::WatcherMatchBatch;
    use std::sync::Mutex as StdMutex;

    use crate::pty::watchers::history::{SessionStatus, WatcherHistory};
    use crate::pty::watchers::{WatcherMatchPayload, WatcherMode};

    type Recorded = (
        WatcherSink,
        Arc<StdMutex<Vec<WatcherMatchBatch>>>,
        crate::pty::watchers::history::WatcherHistoryState,
    );

    fn recording(present: bool) -> Recorded {
        let delivered = Arc::new(StdMutex::new(Vec::new()));
        let sink_delivered = Arc::clone(&delivered);
        let history: crate::pty::watchers::history::WatcherHistoryState =
            Arc::new(WatcherHistory::default());
        (
            WatcherSink {
                history: Arc::clone(&history),
                delivery: WatcherDelivery {
                    window_present: Arc::new(move || present),
                    emit: Arc::new(move |batch| sink_delivered.lock().unwrap().push(batch)),
                },
            },
            delivered,
            history,
        )
    }

    fn batch(session_id: uuid::Uuid) -> WatcherMatchBatch {
        WatcherMatchBatch {
            session_id: session_id.to_string(),
            matches: vec![WatcherMatchPayload {
                session_id: session_id.to_string(),
                seq: 1,
                watcher_id: "w".to_string(),
                mode: WatcherMode::Occurrence,
                at: chrono::Utc::now(),
                captures: Vec::new(),
                row: "hit".to_string(),
                row_truncated: false,
            }],
        }
    }

    /// #1171, 9.3.36 - with the activity window closed, NOTHING is emitted - not a broadcast
    /// nobody reads, not an emit to a label that does not exist - **and the ring still
    /// records**, so opening the window later shows the history.
    #[test]
    fn no_event_is_emitted_when_the_window_is_closed_and_the_ring_still_records() {
        let id = uuid::Uuid::new_v4();
        let (sink, delivered, history) = recording(false);
        history.publish(id, SessionStatus::default());

        sink.emit(batch(id));

        assert!(delivered.lock().unwrap().is_empty());
        assert_eq!(history.snapshot(id, None).matches.len(), 1);
    }

    #[test]
    fn the_batch_is_delivered_and_recorded_when_the_window_is_open() {
        let id = uuid::Uuid::new_v4();
        let (sink, delivered, history) = recording(true);
        history.publish(id, SessionStatus::default());

        sink.emit(batch(id));

        assert_eq!(delivered.lock().unwrap().len(), 1);
        assert_eq!(history.snapshot(id, None).matches.len(), 1);
    }
}

/// #1341 - releases the restore selection barrier and completes the observer
/// barrier. `complete()` is the normal path; `Drop` is the backstop for a
/// panic anywhere in the spawned restore task, so the selection coordinator
/// (which queues every webview session command behind the restore barrier)
/// can never wedge.
struct RestoreCompletionGuard {
    barrier: Option<crate::session::selection::RestoreBarrierGuard>,
    observer: Arc<RestoreObserverStartBarrier>,
    completed: bool,
}

impl RestoreCompletionGuard {
    fn new(
        barrier: crate::session::selection::RestoreBarrierGuard,
        observer: Arc<RestoreObserverStartBarrier>,
    ) -> Self {
        Self {
            barrier: Some(barrier),
            observer,
            completed: false,
        }
    }

    fn complete(&mut self) {
        if let Some(barrier) = self.barrier.take() {
            barrier.finish();
        }
        if let Err(e) = self.observer.mark_restore_complete() {
            log::error!("[restore] observer barrier completion failed: {}", e);
        }
        self.completed = true;
    }
}

impl Drop for RestoreCompletionGuard {
    fn drop(&mut self) {
        if !self.completed {
            log::error!(
                "[restore] restore task ended abnormally; force-releasing the selection barrier"
            );
            if let Some(barrier) = self.barrier.take() {
                barrier.finish();
            }
        }
    }
}
/// #1341 - the ONLY sanctioned way to launch the startup restore: as a spawned
/// runtime task, never inside a main-thread `block_on`. A `block_on` here
/// freezes the main thread while any session open awaits the #1327 update
/// gate, which starves the WebView2 page and makes the SI/NO prompt expire
/// unseen (the #1341 freeze). Anti-revert guard: bypassing this seam (e.g.
/// inlining a `block_on` around the restore body) leaves this function dead
/// code, and `cargo clippy --workspace --all-targets -- -D warnings` fails CI.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_restore_startup(
    app: tauri::AppHandle,
    restore_barrier: crate::session::selection::RestoreBarrierGuard,
    restore_observer_barrier: Arc<RestoreObserverStartBarrier>,
    restore_transaction: crate::session::selection::SelectionTransaction<tauri::Wry>,
    restore_flag: Arc<RestoreInProgress>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    settings_state: SettingsState,
    settings_snapshot: config::settings::AppSettings,
    persisted: Vec<sessions_persistence::PersistedSession>,
    teams: Vec<crate::config::teams::DiscoveredTeam>,
    setting_on: bool,
    idle_detector: Arc<IdleDetector>,
    git_watcher: Arc<GitWatcher>,
    discovery_branch_watcher: Arc<DiscoveryBranchWatcher>,
    resource_monitor: Arc<resource_monitor::ResourceMonitorState>,
    selection_coordinator: crate::session::selection::SelectionCoordinator,
    loop_scheduler: Arc<loops::scheduler::LoopScheduler>,
    non_stop_state: crate::loops::non_stop_watchdog::NonStopWatchdogState,
    ui_automation_state: crate::testability::ui_automation::UiAutomationState,
    shutdown: ShutdownSignal,
) -> tauri::async_runtime::JoinHandle<()> {
    use futures::FutureExt;
    tauri::async_runtime::spawn(async move {
        // §224 A.2.5 RAII guard (moved verbatim from the old block_on body):
        // clears the flag on normal exit AND on panic unwind so the daemon
        // can't get stuck advertising "still restoring" forever.
        struct RestoreGuard(Arc<RestoreInProgress>);
        impl Drop for RestoreGuard {
            fn drop(&mut self) {
                self.0 .0.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let _restore_guard = RestoreGuard(restore_flag);

        // The moved body keeps the setup block's local names; re-bind the seam
        // parameters to them here so the body is a byte-identical move. `app`
        // and `pty_mgr` are also used by the tail, so the body gets clones;
        // the body-only handles move by value.
        let app_handle = app.clone();
        let pty_mgr_clone = pty_mgr.clone();
        let session_mgr_clone = session_mgr;
        let settings_state_clone = settings_state;
        let restore_transaction_for_task = restore_transaction;
        // 14.4 - the body's shutdown checks need their own handle; the tail
        // keeps the `shutdown` parameter (observer starts and services).
        let shutdown_for_body = shutdown.clone();

        // #1341 - the selection barrier must never wedge behind a failed
        // restore (the webview's session commands queue on it).
        let mut completion =
            RestoreCompletionGuard::new(restore_barrier, Arc::clone(&restore_observer_barrier));

        // (#1793) Collected in the restore loop, consumed once by the pass in
        // the tail. Shared rather than returned so a body panic still hands the
        // tail the targets restored before it.
        let restart_resume_targets: Arc<Mutex<Vec<commands::session::RestartResumeTarget>>> =
            Arc::new(Mutex::new(Vec::new()));
        let restart_resume_targets_for_body = Arc::clone(&restart_resume_targets);

        // #1341 - a panic inside the restore body degrades (logged + partial
        // restore, retried next boot) instead of aborting the task and
        // skipping the startup continuation. Mirrors the FinishGuard "never
        // wedge" rule from #1327.
        let body = std::panic::AssertUnwindSafe(async move {
            let mut active_id = None;
            let mut failed_recoverable: Vec<sessions_persistence::PersistedSession> = Vec::new();

            // #248 Grinch Z5 — count outcomes for the end-of-restore summary line.
            let mut n_woken: usize = 0;
            let mut n_deferred: usize = 0;

            let root_agent_path = match crate::config::root_agent::ensure_root_agent_dir() {
                Ok(path) => Some(path),
                Err(e) => {
                    log::error!(
                        "[root-agent] Failed to provision root agent during restore: {}",
                        e
                    );
                    None
                }
            };
            let root_ps = persisted
                .iter()
                .find(|ps| {
                    ps.is_root_agent
                        || crate::config::root_agent::is_root_agent_path(&ps.working_directory)
                })
                .cloned();

            if let Some(root_path) = root_agent_path.clone() {
                if shutdown_for_body.token().is_cancelled() {
                    // 14.4 - exit during restore: never spawn a root PTY
                    // once shutdown is underway (it would be orphaned until
                    // the next boot's container-orphan cleanup). Keep the
                    // row for next boot via the failed_recoverable merge.
                    if let Some(ps) = root_ps.as_ref() {
                        failed_recoverable.push(ps.clone());
                    }
                } else {
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
                        Some(ps) if should_wake_root_agent_on_restore(ps.status.as_ref()) => {
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
                                // #1271 - the host shell is rebuilt at restore
                                // time from the SAME settings snapshot that
                                // re-resolved the agent command (no persistence:
                                // a persisted shell could pair with a freshly
                                // re-resolved command across a config change).
                                let resolved_agent_host_shell = if resolved_spawn.is_some() {
                                    Some(crate::pty::backend::ResolvedAgentHostShell {
                                        program: settings_snapshot.default_shell.clone(),
                                        args: settings_snapshot.default_shell_args.clone(),
                                    })
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
                                        resolved_agent_host_shell,
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
                }
            } else if let Some(ps) = root_ps.as_ref() {
                failed_recoverable.push(ps.clone());
            }

            let archived_roots = sessions_persistence::normalize_project_roots(
                &settings_snapshot.archived_project_paths,
            );

            for ps in &persisted {
                if ps.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&ps.working_directory)
                {
                    continue;
                }

                // 14.4 - exit during restore: stop waking once shutdown
                // is underway. The row rides in failed_recoverable so
                // persist_merging_failed keeps it on disk for next boot
                // (a bare break would drop it from sessions.json).
                if shutdown_for_body.token().is_cancelled() {
                    failed_recoverable.push(ps.clone());
                    break;
                }

                // Skip sessions whose CWD no longer exists (permanent failure)
                if !std::path::Path::new(&ps.working_directory).exists() {
                    log::warn!(
                        "Skipping restore of '{}': CWD '{}' no longer exists",
                        ps.name,
                        ps.working_directory
                    );
                    // §1295 site C (restore-skip): append ONE archive
                    // record BEFORE the `continue`. The restore task is
                    // async and holds no `sessions_save_lock` here, so we
                    // use the locking public variant (S4). The record is
                    // written before the continue unchanged; it does not
                    // touch sessions.json (site C leaves the row's disk
                    // fate to the next persist, §224 G5).
                    let config_dir = crate::config::config_dir();
                    if let Some(config_dir) = config_dir {
                        sessions_persistence::append_orphan_archive_record(
                            &config_dir,
                            "restoreCwdMissing",
                            "archived",
                            ps,
                        )
                        .await;
                    }
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
                let archived_session = sessions_persistence::is_under_normalized_archived_roots(
                    &ps.working_directory,
                    &archived_roots,
                );
                let persisted_working = crate::session::session::persisted_is_working(
                    ps.status.as_ref(),
                    ps.waiting_for_input,
                );
                let wake = restore_session_should_wake(
                    archived_session,
                    setting_on,
                    settings_snapshot.restart_resume_wake_working_agents,
                    is_coord,
                    ps.status.as_ref(),
                    persisted_working,
                );

                if !wake {
                    // Defer: create a dormant Session record (no PTY, status = Exited(0)).
                    match restore_transaction_for_task
                        .restore_dormant_inline(crate::session::selection::DormantRestoreRequest {
                            persisted: ps.clone(),
                            working_directory: ps.working_directory.clone(),
                            is_coordinator: is_coord,
                            is_root_agent: false,
                        })
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
                            if restore_session_should_become_active(ps.was_active, archived_session)
                            {
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
                // #1271 - rebuilt at restore time from the SAME settings
                // snapshot that re-resolved the agent command (no
                // persistence, same rationale as the root-agent restore).
                let resolved_agent_host_shell = if resolved_spawn.is_some() {
                    Some(crate::pty::backend::ResolvedAgentHostShell {
                        program: settings_snapshot.default_shell.clone(),
                        args: settings_snapshot.default_shell_args.clone(),
                    })
                } else {
                    None
                };
                let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref()
                {
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
                    resolved_agent_host_shell,
                    // #973 - headless caller: no terminal to measure, keep 120x30.
                    None,
                    Some(ps.start_fresh_on_restore),
                    is_coord
                        .then(|| {
                            crate::commands::session::carry_communication_for_restart(
                                ps.communication.clone(),
                                ps.start_fresh_on_restore,
                            )
                        })
                        .flatten(),
                )
                .await
                {
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
                        // (#1793) Pair the row with the session it just became;
                        // both are in hand, so no correlation key is needed.
                        if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                            let prompt = commands::session::restart_resume_prompt_for(
                                &settings_snapshot,
                                is_coord,
                                ps.start_fresh_on_restore,
                                persisted_working,
                            );
                            if let Some(prompt) = prompt {
                                let target = commands::session::RestartResumeTarget {
                                    session_id: uuid,
                                    name: ps.name.clone(),
                                    working_directory: ps.working_directory.clone(),
                                    prompt: prompt.to_string(),
                                    // (#1793) Stamped HERE, after the PTY was
                                    // spawned and therefore after
                                    // `register_session` seeded the activity
                                    // clock, so "printable output strictly
                                    // after this instant" cannot be satisfied
                                    // by the spawn seed alone.
                                    collected_at: std::time::Instant::now(),
                                };
                                restart_resume_targets_for_body
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .push(target);
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
                n_woken,
                n_deferred,
                setting_on,
                persisted.len()
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
            let mgr: tokio::sync::RwLockReadGuard<'_, SessionManager> =
                session_mgr_clone.read().await;
            sessions_persistence::persist_merging_failed(&mgr, &failed_recoverable).await;

            if !failed_recoverable.is_empty() {
                log::warn!(
                    "Session restore: {} sessions failed (preserved for next attempt): {:?}",
                    failed_recoverable.len(),
                    failed_recoverable
                        .iter()
                        .map(|s| &s.name)
                        .collect::<Vec<_>>()
                );
            }
        });
        if let Err(panic) = body.catch_unwind().await {
            log::error!(
                "[restore] restore task panicked (partial restore; retried next boot): {:?}",
                panic
            );
        }

        // Flag window preserved (14.2): cleared at the end of the body, BEFORE
        // the barrier release and the tail, exactly as today (pre-fix the guard
        // dropped at the end of the block_on body, before finish/mark_complete
        // and the tail). A body panic still clears via unwind, unchanged.
        drop(_restore_guard);

        completion.complete();

        // Tail: same ordering as today; a panic here is logged and leaves the
        // barrier already released (complete() ran), with services after the
        // panic point not started - a documented, VISIBLE degradation (14.3).
        let tail = std::panic::AssertUnwindSafe(async move {
            // These observers mutate session metadata or persistence directly.
            // Start them only after restore has completed, which is stricter than
            // merely placing restore first and prevents an intermediate snapshot.
            if let Err(e) = restore_observer_barrier.start("idle", || {
                idle_detector.start(shutdown.clone());
            }) {
                log::error!("[restore] observer 'idle' start failed: {}", e);
            }

            // (#1793) After the idle observer start, because that observer
            // maintains the `waiting_for_input` this polls, and before the rest
            // of the tail so a later tail panic cannot lose it. This single
            // drain is the whole once-per-app-start latch.
            let restart_resume_batch: Vec<commands::session::RestartResumeTarget> =
                restart_resume_targets
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .drain(..)
                    .collect();
            if !restart_resume_batch.is_empty() {
                let resume_app = app.app_handle().clone();
                let resume_shutdown = shutdown.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::select! {
                        biased;
                        _ = resume_shutdown.token().cancelled() => {}
                        _ = commands::session::run_restart_resume(
                            &resume_app,
                            restart_resume_batch,
                        ) => {}
                    }
                });
            }

            if let Err(e) = restore_observer_barrier.start("git", || {
                git_watcher.start(shutdown.clone());
            }) {
                log::error!("[restore] observer 'git' start failed: {}", e);
            }

            if let Err(e) = restore_observer_barrier.start("discovery", || {
                discovery_branch_watcher.start(shutdown.clone());
            }) {
                log::error!("[restore] observer 'discovery' start failed: {}", e);
            }

            resource_monitor::watchdog::start(
                (*resource_monitor).clone(),
                app.state::<SettingsState>().inner().clone(),
                selection_coordinator.clone(),
                shutdown.clone(),
            );
            pty_mgr
                .lock()
                .unwrap()
                .start_container_pending_reaper(shutdown.clone());

            app.state::<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>>()
                .start_artifact_cleanup();
            let mailbox_poller = phone::mailbox::MailboxPoller::new();
            mailbox_poller.start(app.app_handle().clone(), shutdown.clone());
            loop_scheduler
                .clone()
                .start(app.app_handle().clone(), shutdown.clone());
            crate::session::auto_close::start(app.app_handle().clone(), shutdown.clone());
            crate::loops::non_stop_watchdog::start(
                app.app_handle().clone(),
                non_stop_state.clone(),
                shutdown.clone(),
            );
            ui_automation_state.start(app.app_handle().clone(), shutdown.clone());
        });
        if let Err(panic) = tail.catch_unwind().await {
            log::error!(
                "[restore] post-restore startup panicked (services after the panic point not started): {:?}",
                panic
            );
        }
    })
}

/// (#1652) Pins the type of the closure `generate_handler!` produces. The
/// macro's closure parameter is UNANNOTATED and rustc type-checks the body at
/// the `let` that binds it. A bare binding is therefore
/// `error[E0282]: type annotations needed`, and annotating the OUTER closure's
/// parameter does not reach it. This identity function supplies the expected
/// type on the binding itself. Preferred over
/// `let generated: Box<dyn Fn(..) -> bool + Send + Sync> = ...` because
/// `pty_write` is the highest-frequency command and boxing adds an indirection
/// to every invoke.
fn pin_handler<F: Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static>(f: F) -> F {
    f
}

// ---------------------------------------------------------------------------
// #2296 — backend quit gate.
//
// Closing the main window must not kill the app while another window still
// holds unsaved work. Every window that owns such work registers as a *gate*;
// a quit round asks each one and exits only when all of them consent.
//
// The whole state machine is synchronous and lives behind one `std::sync::Mutex`
// that is never held across an await. Every mutating entry point returns the
// events it wants emitted (`QuitGateEffects`) instead of emitting under the
// lock, and `quit_gate_run` awaits outside it.
// ---------------------------------------------------------------------------

/// Unpaused budget a non-force round may spend waiting for its gates.
///
/// Strictly beyond main's 10-second wall-clock Force offer, so the user is
/// always offered Force before the backend gives up on its own.
pub const QUIT_GATE_TIMEOUT: Duration = Duration::from_secs(30);

/// The only window allowed to start or force a quit.
pub const QUIT_GATE_MAIN_LABEL: &str = "main";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum QuitOutcomeKind {
    Exiting,
    Aborted,
    InFlight,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QuitAbortReason {
    Refused,
    Timeout,
    Unregistered,
    Destroyed,
    Cancelled,
}

/// Typed return of `quit_application` and payload of `app_quit_outcome`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuitOutcome {
    pub outcome: QuitOutcomeKind,
    pub epoch: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<QuitAbortReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusing_labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unanswered_labels: Option<Vec<String>>,
}

impl QuitOutcome {
    fn bare(outcome: QuitOutcomeKind, epoch: u64) -> Self {
        Self {
            outcome,
            epoch,
            reason: None,
            refusing_labels: None,
            unanswered_labels: None,
        }
    }

    pub fn exiting(epoch: u64) -> Self {
        Self::bare(QuitOutcomeKind::Exiting, epoch)
    }

    pub fn in_flight(epoch: u64) -> Self {
        Self::bare(QuitOutcomeKind::InFlight, epoch)
    }

    pub fn stale(epoch: u64) -> Self {
        Self::bare(QuitOutcomeKind::Stale, epoch)
    }

    fn aborted(
        epoch: u64,
        reason: QuitAbortReason,
        refusing: Vec<String>,
        unanswered: Vec<String>,
    ) -> Self {
        Self {
            outcome: QuitOutcomeKind::Aborted,
            epoch,
            reason: Some(reason),
            refusing_labels: (!refusing.is_empty()).then_some(refusing),
            unanswered_labels: (!unanswered.is_empty()).then_some(unanswered),
        }
    }
}

/// Typed return of `quit_gate_register`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status")]
pub enum QuitGateRegistration {
    Registered,
    InFlight { epoch: u64 },
}

/// Events a state transition asks the caller to emit, outside the lock.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct QuitGateEffects {
    /// Terminal abort outcome for `main` (`app_quit_outcome`).
    pub outcome_to_main: Option<QuitOutcome>,
    /// `app_quit_cancelled` targets: every gate enrolled in the cancelled epoch.
    pub cancelled_gates: Vec<String>,
    /// Epoch carried by `cancelled_gates`.
    pub cancelled_epoch: u64,
}

impl QuitGateEffects {
    fn is_empty(&self) -> bool {
        self.outcome_to_main.is_none() && self.cancelled_gates.is_empty()
    }
}

/// Time seam. Ticks are a monotonic offset from gate creation, so the whole
/// deadline arithmetic is plain `Duration` math and a test clock can step it
/// exactly. Injected in tests.
pub trait QuitGateClock: Send + Sync + 'static {
    fn now(&self) -> Duration;
    fn sleep_until(&self, at: Duration) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// Production clock: real monotonic time on the Tokio timer.
pub struct TokioQuitClock {
    origin: tokio::time::Instant,
}

impl Default for TokioQuitClock {
    fn default() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

impl QuitGateClock for TokioQuitClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep_until(&self, at: Duration) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(tokio::time::sleep_until(self.origin + at))
    }
}

/// Side-effect seam: event emission and process exit. Injected in tests.
pub trait QuitGateHost: Send + Sync + 'static {
    fn emit_to(&self, label: &str, event: &str, payload: serde_json::Value) -> Result<(), String>;
    fn exit(&self, code: i32);
}

/// Production host: targeted Tauri emits and `AppHandle::exit`.
pub struct TauriQuitHost {
    app: tauri::AppHandle,
}

impl TauriQuitHost {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl QuitGateHost for TauriQuitHost {
    fn emit_to(&self, label: &str, event: &str, payload: serde_json::Value) -> Result<(), String> {
        tauri::Emitter::emit_to(&self.app, label, event, payload).map_err(|e| e.to_string())
    }

    fn exit(&self, code: i32) {
        // SINGLE production exit site for the quit gate. Everything else routes
        // its Exiting decision through `quit_gate_run`, which calls this once
        // per terminal Exiting (see `try_claim_exit`).
        self.app.exit(code);
    }
}

/// One live quit round.
struct QuitRound {
    epoch: u64,
    /// Every label enrolled by the atomic snapshot, answered or not.
    snapshot: std::collections::BTreeSet<String>,
    /// Gates that have not answered yet. Holding the sender *is* the pending
    /// record; dropping it closes the waiter's receiver.
    pending: HashMap<String, tokio::sync::oneshot::Sender<bool>>,
    refusing: std::collections::BTreeSet<String>,
    busy: HashSet<String>,
    /// Budget left when paused; authoritative across pause/resume.
    remaining: Duration,
    /// `Some` while the clock runs, `None` while a busy gate pauses it.
    /// Expressed in clock ticks (see `QuitGateClock`).
    deadline: Option<Duration>,
}

#[derive(Default)]
struct QuitGateInner {
    next_epoch: u64,
    registered: std::collections::BTreeSet<String>,
    round: Option<QuitRound>,
    /// Terminal result of the most recent round. Written before any cleanup so
    /// a waiter woken by receiver closure reads a decision, never infers one.
    terminal: Option<(u64, QuitOutcome)>,
    /// Guards the one-exit rule for the current terminal `Exiting`.
    exit_claimed: bool,
}

impl QuitGateInner {
    fn alloc_epoch(&mut self) -> u64 {
        self.next_epoch += 1;
        self.next_epoch
    }

    fn live_round(&mut self, epoch: u64) -> Option<&mut QuitRound> {
        match self.round.as_mut() {
            Some(round) if round.epoch == epoch => Some(round),
            _ => None,
        }
    }

    fn terminal_for(&self, epoch: u64) -> Option<QuitOutcome> {
        match &self.terminal {
            Some((e, outcome)) if *e == epoch => Some(outcome.clone()),
            _ => None,
        }
    }

    /// Writes `outcome` as this epoch's terminal result and tears the round
    /// down. Never replaces an existing terminal for the same epoch.
    fn finish(&mut self, epoch: u64, outcome: QuitOutcome) -> Option<QuitOutcome> {
        if self.terminal_for(epoch).is_some() {
            return None;
        }
        self.live_round(epoch)?;
        // Terminal result first, cleanup second — the ordering the waiter relies on.
        self.terminal = Some((epoch, outcome.clone()));
        self.exit_claimed = false;
        self.round = None;
        Some(outcome)
    }

    /// Recomputes the deadline from `remaining` when no busy gate holds it.
    fn resume_if_idle(round: &mut QuitRound, now: Duration) {
        if round.busy.is_empty() && round.deadline.is_none() {
            round.deadline = Some(now + round.remaining);
        }
    }

    fn pause(round: &mut QuitRound, now: Duration) {
        if let Some(deadline) = round.deadline.take() {
            round.remaining = deadline.saturating_sub(now);
        }
    }
}

/// Managed quit-gate state.
pub struct QuitGate {
    inner: Mutex<QuitGateInner>,
    /// Woken on every state change so the waiter re-reads deadline and terminal.
    notify: tokio::sync::Notify,
    clock: Arc<dyn QuitGateClock>,
}

impl Default for QuitGate {
    fn default() -> Self {
        Self::new()
    }
}

impl QuitGate {
    pub fn new() -> Self {
        Self::with_clock(Arc::new(TokioQuitClock::default()))
    }

    pub fn with_clock(clock: Arc<dyn QuitGateClock>) -> Self {
        Self {
            inner: Mutex::new(QuitGateInner::default()),
            notify: tokio::sync::Notify::new(),
            clock,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QuitGateInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Builds the abort effects for a terminal outcome of `round`.
    fn abort_effects(round: &QuitRound, outcome: QuitOutcome) -> QuitGateEffects {
        QuitGateEffects {
            cancelled_gates: round.snapshot.iter().cloned().collect(),
            cancelled_epoch: round.epoch,
            outcome_to_main: Some(outcome),
        }
    }

    /// Aborts the live round `epoch`. No-op for a stale or already terminal one.
    fn abort(&self, epoch: u64, reason: QuitAbortReason) -> QuitGateEffects {
        let mut inner = self.lock();
        let (refusing, mut unanswered) = {
            let Some(round) = inner.live_round(epoch) else {
                return QuitGateEffects::default();
            };
            let refusing: Vec<String> = round.refusing.iter().cloned().collect();
            let unanswered: Vec<String> = match reason {
                QuitAbortReason::Timeout => round.pending.keys().cloned().collect(),
                _ => Vec::new(),
            };
            (refusing, unanswered)
        };
        unanswered.sort();
        let outcome = QuitOutcome::aborted(epoch, reason, refusing, unanswered);
        let effects = {
            let round = inner
                .live_round(epoch)
                .expect("round checked live above under the same lock");
            Self::abort_effects(round, outcome.clone())
        };
        match inner.finish(epoch, outcome) {
            Some(_) => {
                drop(inner);
                self.notify.notify_waiters();
                effects
            }
            None => QuitGateEffects::default(),
        }
    }

    /// Writes terminal `Exiting` once the live round has no unanswered gate.
    fn finalize_consent_locked(inner: &mut QuitGateInner, epoch: u64) -> bool {
        let settled = match inner.live_round(epoch) {
            Some(round) => round.pending.is_empty() && round.refusing.is_empty(),
            None => false,
        };
        if !settled {
            return false;
        }
        inner.finish(epoch, QuitOutcome::exiting(epoch)).is_some()
    }

    fn finalize_consent(&self, epoch: u64) {
        let mut inner = self.lock();
        if Self::finalize_consent_locked(&mut inner, epoch) {
            drop(inner);
            self.notify.notify_waiters();
        }
    }

    /// True exactly once per terminal `Exiting`, for the single exit site.
    fn try_claim_exit(&self, epoch: u64) -> bool {
        let mut inner = self.lock();
        let is_exiting = matches!(
            inner.terminal_for(epoch),
            Some(QuitOutcome {
                outcome: QuitOutcomeKind::Exiting,
                ..
            })
        );
        if is_exiting && !inner.exit_claimed {
            inner.exit_claimed = true;
            true
        } else {
            false
        }
    }

    // --- command-facing state transitions ------------------------------------

    /// Enrols `label`. Rejected during an active round: the snapshot is atomic,
    /// so a late registrant would otherwise be silently ungated.
    pub fn register(&self, label: &str) -> QuitGateRegistration {
        let mut inner = self.lock();
        if let Some(round) = inner.round.as_ref() {
            return QuitGateRegistration::InFlight { epoch: round.epoch };
        }
        inner.registered.insert(label.to_string());
        QuitGateRegistration::Registered
    }

    /// Drops `label`'s registration. Aborts the live round only if that gate
    /// had not already answered — same-epoch consent is final.
    pub fn unregister(&self, label: &str) -> QuitGateEffects {
        self.teardown_gate(label, QuitAbortReason::Unregistered)
    }

    /// Window destroyed: same as unregister, with reason `destroyed`.
    /// Main's destruction is not an abort — main is never a gate.
    pub fn window_gone(&self, label: &str) -> QuitGateEffects {
        self.teardown_gate(label, QuitAbortReason::Destroyed)
    }

    /// Drops `label`'s registration and, if it was still unanswered, aborts the
    /// live round under ONE lock acquisition.
    ///
    /// The pending sender is removed into a local and dropped only after the
    /// terminal result has been written, so no waiter can ever observe a closed
    /// receiver before the decision that explains it.
    fn teardown_gate(&self, label: &str, reason: QuitAbortReason) -> QuitGateEffects {
        let mut inner = self.lock();
        inner.registered.remove(label);

        let now = self.clock.now();
        let (epoch, pending_sender) = match inner.round.as_mut() {
            Some(round) => {
                // Only an UNANSWERED gate aborts the round; same-epoch consent
                // is final and cannot be revoked by teardown.
                let sender = round.pending.remove(label);
                round.busy.remove(label);
                QuitGateInner::resume_if_idle(round, now);
                match sender {
                    Some(sender) => (Some(round.epoch), Some(sender)),
                    None => (None, None),
                }
            }
            None => (None, None),
        };
        let Some(epoch) = epoch else {
            return QuitGateEffects::default();
        };

        let refusing: Vec<String> = inner
            .live_round(epoch)
            .map(|round| round.refusing.iter().cloned().collect())
            .unwrap_or_default();
        let outcome = QuitOutcome::aborted(epoch, reason, refusing, vec![label.to_string()]);
        let effects = {
            let round = inner
                .live_round(epoch)
                .expect("round checked live above under the same lock");
            Self::abort_effects(round, outcome.clone())
        };
        let finished = inner.finish(epoch, outcome).is_some();
        // Terminal written; only now may the sender close.
        drop(pending_sender);
        drop(inner);
        if finished {
            self.notify.notify_waiters();
            effects
        } else {
            QuitGateEffects::default()
        }
    }

    /// Gate answer. Stale epoch, duplicate answer and unknown label do nothing.
    pub fn resolve(&self, label: &str, epoch: u64, consent: bool) -> QuitGateEffects {
        let mut inner = self.lock();
        {
            let Some(round) = inner.live_round(epoch) else {
                return QuitGateEffects::default();
            };
            let Some(sender) = round.pending.remove(label) else {
                return QuitGateEffects::default();
            };
            round.busy.remove(label);
            let now = self.clock.now();
            QuitGateInner::resume_if_idle(round, now);
            // The receiver wakes the waiter; the decision itself always comes
            // from the terminal slot, never from this value or from closure.
            let _ = sender.send(consent);
            if !consent {
                round.refusing.insert(label.to_string());
            }
        }
        if !consent {
            // A refusal is terminal immediately, even while a peer is busy.
            let refusing: Vec<String> = inner
                .live_round(epoch)
                .map(|round| round.refusing.iter().cloned().collect())
                .unwrap_or_default();
            let outcome =
                QuitOutcome::aborted(epoch, QuitAbortReason::Refused, refusing, Vec::new());
            let effects = {
                let round = inner
                    .live_round(epoch)
                    .expect("round checked live above under the same lock");
                Self::abort_effects(round, outcome.clone())
            };
            return match inner.finish(epoch, outcome) {
                Some(_) => {
                    drop(inner);
                    self.notify.notify_waiters();
                    effects
                }
                None => QuitGateEffects::default(),
            };
        }
        QuitGate::finalize_consent_locked(&mut inner, epoch);
        drop(inner);
        self.notify.notify_waiters();
        QuitGateEffects::default()
    }

    /// Busy progress pauses the timeout and preserves the remaining budget.
    /// Only the current epoch's busy set can move the clock.
    pub fn progress(&self, label: &str, epoch: u64, busy: bool) {
        let mut inner = self.lock();
        let Some(round) = inner.live_round(epoch) else {
            return;
        };
        if !round.pending.contains_key(label) {
            return;
        }
        let now = self.clock.now();
        if busy {
            round.busy.insert(label.to_string());
            QuitGateInner::pause(round, now);
        } else {
            round.busy.remove(label);
            QuitGateInner::resume_if_idle(round, now);
        }
        drop(inner);
        self.notify.notify_waiters();
    }

    /// Expires the round if its deadline passed and no gate is busy.
    fn on_timeout(&self, epoch: u64) -> QuitGateEffects {
        {
            let mut inner = self.lock();
            let now = self.clock.now();
            let Some(round) = inner.live_round(epoch) else {
                return QuitGateEffects::default();
            };
            // Recheck busy under the lock: a pause may have landed between the
            // sleep firing and this acquisition.
            if !round.busy.is_empty() {
                return QuitGateEffects::default();
            }
            match round.deadline {
                Some(deadline) if now >= deadline => {}
                _ => return QuitGateEffects::default(),
            }
        }
        self.abort(epoch, QuitAbortReason::Timeout)
    }

    #[cfg(test)]
    fn terminal_snapshot_for_test(&self, epoch: u64) -> Option<QuitOutcome> {
        self.lock().terminal_for(epoch)
    }

    fn deadline_for(&self, epoch: u64) -> Option<Duration> {
        let mut inner = self.lock();
        inner.live_round(epoch).and_then(|round| round.deadline)
    }
}

/// Emits the events a transition asked for. Never called with the lock held.
pub fn apply_quit_gate_effects(host: &dyn QuitGateHost, effects: &QuitGateEffects) {
    if effects.is_empty() {
        return;
    }
    for label in &effects.cancelled_gates {
        if let Err(e) = host.emit_to(
            label,
            "app_quit_cancelled",
            serde_json::json!({ "epoch": effects.cancelled_epoch, "label": label }),
        ) {
            log::warn!("[quit-gate] app_quit_cancelled emit to {label} failed: {e}");
        }
    }
    if let Some(outcome) = &effects.outcome_to_main {
        if let Err(e) = host.emit_to(
            QUIT_GATE_MAIN_LABEL,
            "app_quit_outcome",
            serde_json::to_value(outcome).unwrap_or_else(|_| serde_json::json!({})),
        ) {
            log::warn!("[quit-gate] app_quit_outcome emit failed: {e}");
        }
    }
}

/// Crate-root hook for the builder's `Destroyed` arm.
pub fn quit_gate_window_gone(gate: &QuitGate, host: &dyn QuitGateHost, label: &str) {
    let effects = gate.window_gone(label);
    apply_quit_gate_effects(host, &effects);
}

/// Releases the round if the quit task is cancelled or panics mid-await.
/// Only ever touches its own epoch.
struct QuitRoundGuard {
    gate: Arc<QuitGate>,
    host: Arc<dyn QuitGateHost>,
    epoch: u64,
    armed: bool,
}

impl Drop for QuitRoundGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let effects = self.gate.abort(self.epoch, QuitAbortReason::Cancelled);
        apply_quit_gate_effects(self.host.as_ref(), &effects);
    }
}

/// Runs a quit round to its terminal outcome. `force` takes the epoch to
/// supersede; a non-force call allocates a fresh one.
pub async fn quit_gate_run(
    gate: Arc<QuitGate>,
    host: Arc<dyn QuitGateHost>,
    caller_label: &str,
    force: bool,
    epoch: Option<u64>,
    attempt_id: Option<String>,
) -> Result<QuitOutcome, String> {
    if caller_label != QUIT_GATE_MAIN_LABEL {
        return Err(format!(
            "quit_application is restricted to the '{QUIT_GATE_MAIN_LABEL}' window (caller: {caller_label})"
        ));
    }

    if force {
        // Force must prove it targets the live, nonterminal round before any
        // exit effect; an omitted or stale epoch cannot authorize exit.
        let supplied = epoch.unwrap_or(0);
        let claimed = {
            let mut inner = gate.lock();
            if inner.live_round(supplied).is_some() {
                // Invalidate any stale work still referencing this epoch.
                inner.next_epoch += 1;
                // Terminal `Exiting` for the ORIGINAL epoch, written before the
                // pending senders are dropped, so the superseded waiter returns
                // `Exiting` with its own epoch and does not exit a second time.
                inner
                    .finish(supplied, QuitOutcome::exiting(supplied))
                    .is_some()
            } else {
                false
            }
        };
        if !claimed {
            return Ok(QuitOutcome::stale(supplied));
        }
        gate.notify.notify_waiters();
        if gate.try_claim_exit(supplied) {
            host.exit(0);
        }
        return Ok(QuitOutcome::exiting(supplied));
    }

    let attempt_id = attempt_id.unwrap_or_default();
    if attempt_id.is_empty() {
        return Err("quit_application requires a nonempty attemptId".to_string());
    }

    // Atomic install: allocate the epoch, snapshot the registered gates and
    // install every pending sender under one lock.
    let (new_epoch, receivers) = {
        let mut inner = gate.lock();
        if let Some(round) = inner.round.as_ref() {
            return Ok(QuitOutcome::in_flight(round.epoch));
        }
        let new_epoch = inner.alloc_epoch();
        let snapshot = inner.registered.clone();
        let mut pending = HashMap::new();
        let mut receivers = Vec::new();
        for label in &snapshot {
            let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
            pending.insert(label.clone(), tx);
            receivers.push(rx);
        }
        inner.round = Some(QuitRound {
            epoch: new_epoch,
            snapshot,
            pending,
            refusing: std::collections::BTreeSet::new(),
            busy: HashSet::new(),
            remaining: QUIT_GATE_TIMEOUT,
            deadline: Some(gate.clock.now() + QUIT_GATE_TIMEOUT),
        });
        (new_epoch, receivers)
    };

    let mut guard = QuitRoundGuard {
        gate: Arc::clone(&gate),
        host: Arc::clone(&host),
        epoch: new_epoch,
        armed: true,
    };

    // Start event before awaiting any gate, so main can show its progress UI
    // and its 10-second Force offer for a round that is already installed.
    if let Err(e) = host.emit_to(
        QUIT_GATE_MAIN_LABEL,
        "app_quit_started",
        serde_json::json!({ "epoch": new_epoch, "attemptId": attempt_id }),
    ) {
        log::warn!("[quit-gate] app_quit_started emit failed: {e}");
        // An unobservable live round is worse than no round: tear it down and
        // return its own terminal result.
        guard.armed = false;
        let effects = gate.abort(new_epoch, QuitAbortReason::Cancelled);
        apply_quit_gate_effects(host.as_ref(), &effects);
        let outcome = gate.lock().terminal_for(new_epoch).unwrap_or_else(|| {
            QuitOutcome::aborted(
                new_epoch,
                QuitAbortReason::Cancelled,
                Vec::new(),
                Vec::new(),
            )
        });
        return Ok(outcome);
    }

    let targets: Vec<String> = {
        let mut inner = gate.lock();
        inner
            .live_round(new_epoch)
            .map(|round| round.snapshot.iter().cloned().collect())
            .unwrap_or_default()
    };
    for label in &targets {
        if let Err(e) = host.emit_to(
            label,
            "app_quit_requested",
            serde_json::json!({ "epoch": new_epoch, "label": label }),
        ) {
            log::warn!("[quit-gate] app_quit_requested emit to {label} failed: {e}");
        }
    }

    // A round with no gates consents immediately.
    gate.finalize_consent(new_epoch);

    let mut receivers: futures_util::stream::FuturesUnordered<_> = receivers.into_iter().collect();

    let outcome = loop {
        // ENROL FIRST. `Notified` snapshots the notify-waiters counter when it
        // is CREATED and registers on first poll, so reading the round state
        // before taking that snapshot opens a window in which a
        // `notify_waiters()` is dropped on the floor. A lost resume wakeup
        // would leave this task parked on `pending()` with a stale
        // `deadline: None` and defeat QUIT_GATE_TIMEOUT entirely.
        // Pinned by `waiter_enrols_before_reading_round_state`.
        let notified = gate.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if let Some(outcome) = gate.lock().terminal_for(new_epoch) {
            break outcome;
        }
        let deadline = gate.deadline_for(new_epoch);
        let has_receivers = !receivers.is_empty();
        tokio::select! {
            _ = &mut notified => {}
            Some(_) = futures_util::StreamExt::next(&mut receivers), if has_receivers => {}
            _ = async {
                match deadline {
                    Some(deadline) => gate.clock.sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let effects = gate.on_timeout(new_epoch);
                apply_quit_gate_effects(host.as_ref(), &effects);
            }
        }
    };

    guard.armed = false;
    if outcome.outcome == QuitOutcomeKind::Exiting && gate.try_claim_exit(new_epoch) {
        host.exit(0);
    }
    Ok(outcome)
}

#[tauri::command]
fn quit_gate_register(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
) -> QuitGateRegistration {
    gate.register(window.label())
}

#[tauri::command]
fn quit_gate_unregister(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
) {
    let effects = gate.unregister(window.label());
    apply_quit_gate_effects(&TauriQuitHost::new(app), &effects);
}

#[tauri::command]
fn quit_gate_resolve(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
    epoch: u64,
    consent: bool,
) {
    let effects = gate.resolve(window.label(), epoch, consent);
    apply_quit_gate_effects(&TauriQuitHost::new(app), &effects);
}

#[tauri::command]
fn quit_gate_progress(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
    epoch: u64,
    busy: bool,
) {
    gate.progress(window.label(), epoch, busy);
}

#[tauri::command]
async fn quit_application(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
    force: bool,
    epoch: Option<u64>,
    attempt_id: Option<String>,
) -> Result<QuitOutcome, String> {
    let gate = Arc::clone(gate.inner());
    let host: Arc<dyn QuitGateHost> = Arc::new(TauriQuitHost::new(app));
    let label = window.label().to_string();
    quit_gate_run(gate, host, &label, force, epoch, attempt_id).await
}

/// #2348 - check if at least 50px of a window (physical coords) is visible on any monitor.
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

/// #2348 - the default "centered main" layout for the given LOGICAL monitor
/// metrics: at most 1400x900, centered on the monitor. With no primary monitor
/// the caller passes the historical 1920x1080 fallback at the origin.
fn centered_default_main_geometry(
    primary_x: f64,
    primary_y: f64,
    screen_w: f64,
    screen_h: f64,
) -> config::settings::WindowGeometry {
    let default_w = screen_w.min(1400.0);
    let default_h = screen_h.min(900.0);
    config::settings::WindowGeometry {
        x: primary_x + (screen_w - default_w) / 2.0,
        y: primary_y + (screen_h - default_h) / 2.0,
        width: default_w,
        height: default_h,
    }
}

/// #2348 - the display state that applies at startup. A test placement always
/// overrides the saved state, so `saved` is consulted only when the test branch
/// is absent; a test `maximized=false` therefore cannot inherit a saved maximize.
fn effective_main_display_state(
    saved: config::settings::MainWindowDisplayState,
    test_placement: Option<&crate::testability::window_placement::TestWindowPlacement>,
) -> config::settings::MainWindowDisplayState {
    match test_placement {
        Some(test_geo) if test_geo.maximized => config::settings::MainWindowDisplayState::Maximized,
        Some(_) => config::settings::MainWindowDisplayState::Normal,
        None => saved,
    }
}

/// #2348 - apply the resolved display state after the window is built.
/// Maximizing is best effort: a failure is logged and the window stays usable in
/// its normal state.
fn apply_main_display_state<E: fmt::Display>(
    state: config::settings::MainWindowDisplayState,
    maximize: impl FnOnce() -> Result<(), E>,
) {
    if state == config::settings::MainWindowDisplayState::Maximized {
        if let Err(e) = maximize() {
            log::warn!(
                "[window-setup] main: failed to maximize the main window; leaving it normal: {}",
                e
            );
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run(
    test_window_placement: Option<crate::testability::window_placement::TestWindowPlacement>,
    ui_automation_enabled: bool,
) -> Result<(), StartupError> {
    // #1842: FIRST STATEMENT OF run(), deliberately. GTK's Wayland backend
    // calls wl_display_connect(), which unsetenv()s WAYLAND_SOCKET, so a
    // session identified only by an inherited fd loses its only signal the
    // moment GTK starts. Tao initialises GTK when it builds the event loop, in
    // `.run(..)`; `.setup` and register_configured_hotkey (2809) both run after
    // that and would read an erased variable and answer X11 — the silent dead
    // hotkey this phase exists to prevent.
    //
    // THE REAL INVARIANT IS "before anything initialises GTK", not "before
    // `.setup`". §8's controls check the second because it is mechanically
    // checkable; the two coincide only because nothing in this crate touches
    // gtk/gdk/tao before the event loop, which §8 control 5 pins. If that ever
    // stops being true, this line must move, not the control.
    #[cfg(target_os = "linux")]
    let display_env = crate::screenshot::display_env_snapshot();

    preflight_config_startup()?;

    // Same backend the CLI path now installs in `main.rs` — see `logging.rs`
    // for the rationale. Idempotent, so a hypothetical second call (or the
    // CLI path having already run in this process) is a no-op.
    crate::logging::init_logger();

    // Generate master token — printed to stdout and persisted to master-token.txt for CLI use
    let master_token = MasterToken::new(uuid::Uuid::new_v4().to_string());

    // Create instance-private outbox directory and clean up stale ones
    let config_dir = config::config_dir().expect("Cannot determine home directory");
    let instances_dir = config_dir.join(crate::config::instance_artifacts::INSTANCES_DIR_NAME);

    // Clean up old instance dirs (from previous runs)
    if let Ok(entries) = std::fs::read_dir(&instances_dir) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
        log::info!("[app-outbox] Cleaned stale instance directories");
    }

    // #769 Phase 1 + #1318 - seed the externalized coding-agent catalog and the
    // dest-keyed default config-folder masters once per registered project
    // (whole-file seed-once + create-if-absent, fail-soft; never aborts boot).
    // Must run before the frontend can call `get_coding_agent_catalog`. The
    // read-only CLI loader avoids a boot-time settings write (no root_token
    // auto-gen); the Tauri setup's own `load_settings()` performs the standard
    // migrations. The per-project steady-state pre-check keeps the common
    // already-seeded case lock-free.
    let settings = config::settings::load_settings_for_cli();
    let registered_roots = config::coding_agents_catalog::registered_project_roots(&settings);
    if registered_roots.is_empty() {
        // #2021: with no registered project, initialize or refresh the instance
        // catalog at <config_dir>/coding-agents so the Welcome surface lists
        // the built-in agents on first run. Fail-soft: log-only, never aborts
        // boot.
        if let Some(dir) = config::config_dir() {
            config::coding_agents_catalog::ensure_seeded_instance(&dir);
        }
    }
    for root in registered_roots {
        config::coding_agents_catalog::ensure_seeded_for_project(&root);
    }

    let instance_id = uuid::Uuid::new_v4().to_string();
    // #1149 - open the activity run here, before the rest of boot: a panic in the
    // remaining path then still leaves a run that had started and never stopped,
    // which the next startup reports as unclean. This is also the last point at
    // which `daemon.pid` still holds the PREVIOUS writer's PID, which is what
    // lets the scan tell a dead predecessor from a live sibling.
    let activity_log_enabled = config::settings::read_activity_log_enabled_only();
    crate::config::activity_log::init_run(&config_dir, &instance_id, activity_log_enabled);
    let (_app_outbox_path, app_outbox) = prepare_app_outbox(&config_dir, &instance_id)?;
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

    // #2232 phase 7: the Co-managed supervisor. Created before the idle
    // detector so its callback can read the armed flag synchronously and hand
    // the edge over without touching settings, disk or a session lookup.
    let (co_managed_supervisor, co_managed_triggers) = CoManagedSupervisorHandle::new();
    let co_managed_supervisor_for_idle = co_managed_supervisor.clone();
    let co_managed_supervisor_for_setup = co_managed_supervisor.clone();

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
                // #2232 phase 7: the Co-managed decision travels in the same
                // payload, emitted synchronously and FIRST (epic 3.6). A
                // separate later event would always paint waiting before red.
                // `is_armed` is lock-free and never reads settings or disk.
                let _ = emit_session_idle_edge(
                    app,
                    &co_managed_supervisor_for_idle.armed,
                    Some(&co_managed_supervisor_for_idle),
                    id,
                );
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr_clone = session_mgr.inner().clone();
                let app_for_idle = app.clone();
                // #1682 - read the control-write mark on the watcher thread, at
                // the edge instant. `Option<Duration>` is `Copy`, so it moves
                // into the spawned task with no clone and no guard held.
                let control_write_age = app
                    .try_state::<Arc<crate::pty::idle_detector::IdleDetector>>()
                    .and_then(|detector| detector.control_write_age(id));
                tauri::async_runtime::spawn(async move {
                    let mgr = mgr_clone.read().await;
                    mgr.mark_idle(id).await;
                    crate::config::sessions_persistence::persist_current_state_prune_dormant(&mgr)
                        .await;
                    // #1682 - a busy->idle edge on an armed session stamps
                    // `tooling.lastAgentMessageAt`. A no-op unless a message write reached
                    // an arming site for this session in this process (R7 and R8 arm with
                    // nothing submitted), the user has no recent unsubmitted input in it,
                    // and no recent control write of ours is what re-opened the busy period.
                    crate::commands::session::record_agent_turn_completed(
                        &app_for_idle,
                        &mgr,
                        id,
                        control_write_age,
                    )
                    .await;
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
                    crate::config::sessions_persistence::persist_current_state_prune_dormant(&mgr)
                        .await;
                });
            }
        },
    );
    let session_mgr_for_git = Arc::clone(&session_mgr);
    let session_mgr_for_git_sweeper = Arc::clone(&session_mgr);
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

    // #2232 phase 3 section 5.1 / phase 4 section 4.2: the **single**
    // `CaptureRegistry` of the app. It is stored in Tauri state below and
    // handed to the bridge manager, so the supervisor can open a session's
    // endpoints and pass the live sender into both watchers. Without this the
    // phase-1 and phase-3 chain is unreachable from production.
    let capture_registry = Arc::new(capture::registry::CaptureRegistry::new());
    let capture_registry_for_state = Arc::clone(&capture_registry);

    let tg_mgr: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::with_captures(output_senders, capture_registry),
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

    // (#1652) IPC observer: the process-side view of the frontend -> backend
    // invoke stream. Managed for `ipc_blackbox_report`, stamped by the
    // `invoke_handler` wrapper below, and watched by the loop started in setup.
    let ipc_observer = crate::loops::ipc_observer::IpcObserver::new();
    let ipc_observer_for_setup = std::sync::Arc::clone(&ipc_observer);
    let ipc_observer_for_handler = std::sync::Arc::clone(&ipc_observer);

    // Config-seed critical-section lock. Serializes `perform_config_seed` for a
    // replica so concurrent same-replica spawns cannot clobber each other's
    // in-flight seed scratch (see `ConfigSeedLockState`).
    let config_seed_lock: ConfigSeedLockState = Arc::new(tokio::sync::Mutex::new(()));

    // Issue #609 - cached "npm update available" result, set ONCE by the
    // detached startup check below and read by `get_update_status`.
    let update_check_state: UpdateCheckState = Arc::new(std::sync::OnceLock::new());
    let update_check_state_for_setup = Arc::clone(&update_check_state);

    // Issue #1327 - startup coding-agent update flow: blocks every session open
    // (via `create_session_inner_impl`) until the per-command update run
    // finishes or times out. Managed before the restore task is submitted.
    let agent_update_gate: Arc<agent_update::AgentUpdateGate> =
        Arc::new(agent_update::AgentUpdateGate::new());
    let agent_update_gate_for_setup = Arc::clone(&agent_update_gate);
    // #1551 - process-lifetime install-state cache for the coding-agent version
    // probes (Settings overview, the pass's pre-update probes and the post-pass
    // re-probes all schedule through it).
    let agent_install_cache: Arc<crate::agent_version::AgentInstallCache> =
        Arc::new(crate::agent_version::AgentInstallCache::new());

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

    // #714/#1842/#2079 clipboard + global-shortcut plugins are referenced ONLY
    // on Windows, Linux and macOS; every other target never links them. The
    // rest of the builder chain is shared. This predicate must match
    // `Cargo.toml` and `screenshot/mod.rs`.
    let builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
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

    #[cfg(target_os = "linux")]
    let builder = builder.manage(display_env);

    builder
        .manage(master_token)
        .manage(app_outbox)
        .manage(session_mgr)
        .manage(selection_coordinator)
        .manage(tg_mgr)
        .manage(capture_registry_for_state)
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
        .manage(ipc_observer)
        .manage(web_access_token.clone())
        .manage(broadcaster.clone())
        .manage(WebServerHandle::default())
        .manage(ApiServerHandle::default())
        .manage(message_store_state)
        .manage(pty_target_gate_state)
        .manage(config_seed_lock)
        .manage(update_check_state)
        .manage(agent_update_gate)
        .manage(agent_install_cache)
        .manage(ui_automation_state)
        .manage(terminal_snapshot_state)
        .manage(shutdown_signal)
        .manage(Arc::new(RestoreInProgress(AtomicBool::new(false))))
        .manage(Arc::new(PendingSelfClear::default()))
        .manage(screenshot_capture_state) // #714
        .manage(screenshot_hotkey_state) // #714
        .manage(crate::pty::input_activity::new_state()) // #871 substantive-input tracker
        .manage(crate::pty::input_activity::new_typing_hold_state()) // #2336 typing hold
        .manage(crate::session::warnings::new_session_warning_state())
        .manage(Arc::new(QuitGate::new())) // #2296 managed type: Arc<QuitGate>
        .setup(move |app| {
            use tauri::WebviewWindowBuilder;
            use tauri::WebviewUrl;

            // Make AppHandle available to idle detector callbacks
            let _ = app_handle_lock.set(app.handle().clone());

            // #2232 phase 7: start the Co-managed supervisor once the app
            // handle exists. It owns the slot watchers, the idle-edge handling
            // and the effect, all inside existing SCC members.
            spawn_co_managed_supervisor(
                app.handle().clone(),
                co_managed_supervisor_for_setup.clone(),
                co_managed_triggers,
                shutdown_for_setup.token().clone(),
            );

            // #1398 - registered here, at the top of setup, and NOT in the
            // post-restore tail where a308271c parked it by adjacency: the
            // registration depends only on the global-shortcut plugin plus the
            // `SettingsState` and `ScreenshotHotkeyState` managed above, so
            // waiting for the restore left the hotkey dead for the whole
            // restore window, which grows with the number of sessions. Its own
            // short task keeps the async settings read off the main thread; a
            // `block_on` here would reintroduce the #1341 WebView2 starvation.
            {
                let app_for_hotkey = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let configured = app_for_hotkey
                        .state::<SettingsState>()
                        .read()
                        .await
                        .screenshot_capture_hotkey
                        .clone();
                    match crate::screenshot::register_configured_hotkey(&app_for_hotkey, &configured)
                    {
                        Ok(()) => log::info!(
                            "[screenshot] global hotkey registered '{}'",
                            configured
                        ),
                        Err(error) => {
                            log::warn!("[screenshot] global hotkey registration failed: {}", error)
                        }
                    }
                });
            }

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
            // (e.g. `update_team`). Must happen BEFORE the
            // `PtyManager::new(..., git_watcher, ...)` move below.
            app.manage(Arc::clone(&git_watcher));

            // Discovery branch watcher: polls git branch for discovered replicas every 15s
            let discovery_branch_watcher = DiscoveryBranchWatcher::new(
                app.handle().clone(),
                session_mgr_for_discovery,
            );
            app.manage(Arc::clone(&discovery_branch_watcher));

            // #1298 - the single global POLLING producer of per-repo git state. Both
            // watchers read its published snapshot instead of spawning `git` themselves.
            //
            // Started HERE and not inside `restore_observer_barrier` for exactly one
            // reason: that barrier gates observers which mutate session metadata or
            // persistence, and this one mutates neither, it only publishes into
            // process-local maps. The construction point buys no head start by itself,
            // because at this point there is nothing to sweep (sessions are restored
            // below, discovery is frontend-driven). Cold start is governed by the
            // empty-round floor instead (plan D3). Do not move this under the barrier, and
            // do not "restore" a head-start rationale that was never true.
            let git_sweeper = crate::pty::git_watcher::GitSweeper::new(
                session_mgr_for_git_sweeper,
                app.state::<SettingsState>().inner().clone(),
            );
            git_sweeper.start(shutdown_for_setup.clone());

            // #2064 - the remote activity producer: asks GitHub, through `gh`,
            // whether CI runs on each room repo's exact HEAD and whether the
            // default branch is ahead. Beside `GitSweeper` for the same reason it
            // sits there: it mutates no session metadata, it only publishes into
            // process-local maps. Phase B consumes the transition stream: the
            // notifier started below is its only consumer.
            let (remote_sweeper, remote_transitions) =
                crate::pty::remote_watcher::RemoteSweeper::new(
                    app.state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                        .inner()
                        .clone(),
                    app.state::<SettingsState>().inner().clone(),
                    Box::new({
                        let app_for_remote_activity = app.handle().clone();
                        move |payload: &crate::pty::remote_watcher::RemoteActivityPayload| {
                            let _ = app_for_remote_activity
                                .emit("ac_remote_activity_updated", payload);
                        }
                    }),
                    // #2374 - the neutral snapshot destination: the instance
                    // config directory, resolved here and never inferred from
                    // settings or another artifact.
                    crate::pty::remote_watcher::RemoteSweeper::production_seams(
                        crate::config::config_dir(),
                    ),
                );
            let _ = remote_sweeper.start(shutdown_for_setup.clone());
            // #2064 Phase B - the transition stream's consumer. Started beside the
            // sweeper that produces it, in this same setup closure, and handed a
            // child of the same shutdown token, so the notifier stops when the app
            // does. It injects only into a room orchestrator, and only when both
            // its feature dial and its notify dial are on.
            // The JoinHandle is dropped on purpose: the monitor's lifecycle is its
            // shutdown token and the sweeper's channel, not a join at exit.
            drop(crate::session::remote_alerts::start(
                app.handle().clone(),
                remote_transitions,
                shutdown_for_setup.token().child_token(),
            ));

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

            // #1327 - startup coding-agent update flow. Spawned BEFORE the
            // restore task is submitted so every session open (restore, GUI,
            // web, phone) blocks inside `create_session_inner_impl` until the
            // updates finish or time out.
            {
                let app_for_agent_updates = app.handle().clone();
                let gate_for_setup = Arc::clone(&agent_update_gate_for_setup);
                tauri::async_runtime::spawn(async move {
                    crate::agent_update::run_startup_updates(app_for_agent_updates, gate_for_setup)
                        .await;
                });
            }

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

            // #1171 watcher engine. A SIBLING of the scraper above, not an extension of it:
            // different interval, different modes, its own history. Same construction shape,
            // and for the same reason it must come after `.manage(settings)`.
            let watcher_history: crate::pty::watchers::history::WatcherHistoryState =
                Arc::new(crate::pty::watchers::history::WatcherHistory::default());
            let watcher_engine = WatcherEngine::new(
                Arc::new(WatcherBackends {
                    pty_mgr: pty_mgr.clone(),
                    poison_logged: AtomicBool::new(false),
                }),
                Arc::new(WatcherPatterns {
                    settings: app.state::<SettingsState>().inner().clone(),
                    log: Default::default(),
                }),
                Arc::new(WatcherSink {
                    history: Arc::clone(&watcher_history),
                    delivery: WatcherDelivery::to_watchers_window(app.handle().clone()),
                }),
                Arc::clone(&watcher_history),
            );
            watcher_engine.start(shutdown_for_setup.clone());
            app.manage(Arc::clone(&watcher_engine));
            app.manage(watcher_history);
            // #1646 / #1647 - proactive detection of terminal blocking menus
            let menu_guard = Arc::new(crate::pty::menu_guard::MenuGuard::with_store(
                config::settings::BlockingMenusStore::load_from_config_dir(),
            ));
            crate::config::settings::refresh_shipped_agent_help_from_config_dir();
            menu_guard.start(app.handle().clone(), shutdown_for_setup.clone());
            // (#1652) Started at the top of setup and NOT in the post-restore
            // tail: a freeze detector that waits for the restore is blind for
            // exactly the window that grows with the session count. It depends
            // only on the app handle and the shutdown signal, both available
            // here. Same reasoning as the #1398 hotkey registration above.
            crate::loops::ipc_observer::start(
                app.handle().clone(),
                std::sync::Arc::clone(&ipc_observer_for_setup),
                shutdown_for_setup.clone(),
            );
            app.manage(Arc::clone(&menu_guard));
            // The authoritative scope of the activity window. `open_watchers_window` writes it
            // before every emit and the window pulls it after subscribing, so a re-scope that
            // races the window's own load is recovered instead of dropped in silence.
            app.manage(commands::window::WatchersScopeState::default());

            // Start web server if enabled in settings
            {
                let web_settings = config::settings::load_settings();
                if web_settings.web_server_enabled {
                    let ws_handle = app.state::<WebServerHandle>().inner().clone();
                    let waiter = commands::config::begin_web_server_start(
                        ws_handle.clone(),
                        settings_for_web,
                        web_token_for_server,
                        session_mgr_for_web,
                        pty_mgr.clone(),
                        broadcaster_for_web,
                        app.handle().clone(),
                        shutdown_for_setup.clone(),
                    );
                    match tauri::async_runtime::block_on(waiter.wait()) {
                        Ok(true) => {
                            if let Some((bind, port)) = ws_handle.snapshot().endpoint {
                                println!(
                                    "[web-token] Remote URL: http://{}:{}/?window=main&remoteToken={}",
                                    bind,
                                    port,
                                    web_access_token.value()
                                );
                            }
                        }
                        Ok(false) => {}
                        Err(error) if error == WEB_SERVER_START_CANCELLED => {
                            log::info!("[web-server] autostart cancelled by lifecycle stop");
                        }
                        Err(error) => {
                            log::warn!("[web-server] startup failed: {}", error);
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

            // #1925 - detached remote blocking-menu patterns download. Fail-silent; applies at the next start.
            {
                let app_handle_for_remote_menus = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    crate::update_check::run_remote_blocking_menus_startup(
                        app_handle_for_remote_menus,
                    )
                    .await;
                });
            }

            if let Err(e) = crate::config::root_agent::ensure_root_agent_dir() {
                log::error!("[root-agent] Failed to provision root agent directory: {}", e);
            }

            // #1157 - seed and reconcile the operator-editable injected-message
            // templates. Best effort, never fatal, matching the block above.
            //
            // The ordering is NOT load-bearing: `render` resolves from the
            // registry or the embedded default and never fails, so an alert
            // firing before this point still renders correctly. The cost is
            // bounded and small (one read plus at most three small atomic
            // writes on the setup thread), not "non-blocking"; moving it to
            // tokio::spawn would buy nothing and would race the registry load.
            if let Some(dir) = crate::config::config_dir() {
                if let Err(e) = crate::config::injected_messages::ensure_injected_messages(&dir) {
                    log::warn!("[injected-messages] provisioning failed: {}", e);
                }
            }
            // Force the registry once, so the first alert - whose line() runs
            // on a tokio worker - never performs a blocking read there.
            let _ = crate::config::injected_messages::registry();

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
            let default_main =
                centered_default_main_geometry(primary_x, primary_y, screen_w, screen_h);

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

            // #2348: resolve the startup display state once. A test placement's
            // `maximized` boolean overrides the saved state entirely; otherwise the
            // saved state applies (and a saved maximize is only a request, never a
            // fatal failure: `apply_main_display_state` logs and keeps the window
            // usable).
            let effective_display_state = effective_main_display_state(
                saved_settings.main_window_display_state,
                test_window_placement.as_ref(),
            );

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
                if effective_display_state == config::settings::MainWindowDisplayState::Maximized
                    && !native_handled
                {
                    if let Err(e) = main_win.maximize() {
                        log::warn!("[test-window] failed to maximize main window: {}", e);
                    }
                }
                log_main_window_info(&main_win);
            } else {
                apply_main_display_state(effective_display_state, || main_win.maximize());
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
            // #1341 - the restore now runs as a spawned runtime task
            // (spawn_restore_startup), never inside a main-thread block_on; the
            // main thread returns to the event loop so the webview can load
            // while the #1327 update gate is pending.
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

                spawn_restore_startup(
                    app_handle,
                    restore_barrier,
                    Arc::new(restore_observer_barrier),
                    restore_transaction_for_task,
                    restore_flag_for_task,
                    session_mgr_clone,
                    pty_mgr_clone,
                    settings_state_clone,
                    settings_snapshot,
                    persisted,
                    teams,
                    setting_on,
                    idle_detector_for_setup,
                    git_watcher,
                    discovery_branch_watcher,
                    resource_monitor_for_setup,
                    selection_coordinator_for_setup,
                    loop_scheduler_for_setup,
                    non_stop_state_for_setup,
                    ui_automation_state_for_setup,
                    shutdown_for_setup,
                );
            }

            Ok(())
        })
        // (#1652) Every invoke that reaches the app handler is stamped here,
        // before dispatch: the only place in the process that sees the
        // frontend -> backend direction as a stream, which is what lets
        // `[ipc-observer] SILENCE` speak about the CHANNEL and not one command.
        // Entry only - `generate_handler!` returns as soon as the command is
        // dispatched, so there is no exit to hook. The exit side is the
        // renderer's `settled` accounting (phase 2); the pair separates "never
        // arrived" from "never came back".
        //
        // UNSTABLE API: `tauri::ipc::Invoke`'s own doc comment says it "is used
        // internally by macros and is explicitly **NOT** stable"
        // (tauri-2.10.3/src/ipc/mod.rs:209). `Cargo.lock` pins 2.10.3 and this
        // wrapper is the ONLY place in the app depending on an explicitly
        // unstable tauri type. The next tauri bump must re-check three things:
        // `Invoke::message` public, `InvokeMessage::command()` public, and
        // `generate_handler!` still expanding to a bindable closure expression.
        //
        // Blind spot, stated: Tauri routes plugin commands (`plugin:window|*`,
        // `plugin:event|emit`, `plugin:dialog|open`) through `manager.extend_api`
        // in a branch that never reaches `run_invoke_handler`
        // (tauri-2.10.3/src/webview/mod.rs:1840-1890), so they never reach this
        // closure - and the renderer's registry does not see them either, because
        // `@tauri-apps/api`'s window/event/dialog helpers call
        // `__TAURI_INTERNALS__.invoke` directly, not `TauriTransport.invoke`. The
        // two sides therefore stay symmetric and no false "sent but never
        // entered" signal is produced. The `[ipc-blackbox] coverage:` line repeats
        // this in every record block.
        .invoke_handler({
            let observer = ipc_observer_for_handler;
            let generated = pin_handler(tauri::generate_handler![
                commands::session::create_session,
                commands::session::destroy_session,
                commands::session::close_coordinator,
                commands::session::restart_session,
                commands::session::resolve_blocking_menu,
                commands::session::switch_session,
                commands::session::rename_session,
                commands::session::set_last_prompt,
                commands::session::list_sessions,
                commands::session::get_active_session,
                commands::session::get_last_agent_message,
                session::warnings::drain_session_warnings,
                quit_gate_register,
                quit_gate_unregister,
                quit_gate_resolve,
                quit_gate_progress,
                quit_application,
                commands::session::create_root_agent_session,
                commands::task::task_get_title,
                commands::task::task_set_title,
                  commands::task::task_clean,
                commands::task::task_clean_at,
                commands::task::task_set_title_at,
                commands::pty::pty_write,
                commands::pty::get_typing_hold,
                commands::pty::toggle_typing_hold,
                commands::pty::pty_resize,
                commands::pty::get_screen_snapshot,
                commands::pty::activate_terminal_output,
                commands::pty::detach_terminal_output,
                commands::pty::get_session_context,
                commands::pty::get_watcher_activity,
                commands::pty::preview_watcher_pattern,
                commands::pty::preview_watcher_reach,
                commands::config::get_settings,
                commands::config::get_agent_help,
                commands::config::get_coding_agent_catalog,
                commands::config::get_coding_agent_catalog_report,
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
                commands::config::preview_selection_lock_removal,
                commands::config::apply_selection_lock_removal,
                commands::config::get_replica_selection_default,
                commands::config::set_replica_selection_default,
                commands::config::set_sounds_enabled,
                commands::config::set_theme_light,
                commands::config::set_main_resource_monitor_attached,
                commands::config::set_rail_collapse,
                commands::config::move_coding_agent,
                commands::config::set_log_level,
                commands::config::get_update_status,
                commands::config::get_agent_update_status,
                commands::config::agent_update_answer,
                commands::config::agent_update_cancel,
                commands::config::agent_updates_cancel_all,
                commands::config::get_agent_update_overview,
                commands::co_managed::co_managed_get,
                commands::co_managed::co_managed_set_enabled,
                commands::co_managed::co_managed_effective_state,
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
                commands::window::set_watchers_geometry,
                commands::window::set_main_window_placement,
                commands::window::open_in_explorer,
                commands::window::open_spec_board_window,
                commands::window::open_resource_monitor_window,
                commands::window::dock_resource_monitor_window,
                commands::window::open_watchers_window,
                commands::window::get_watchers_scope,
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
                commands::config::list_web_server_interfaces,
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
                commands::ipc_blackbox::ipc_blackbox_report,
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
                commands::loops::list_unresolved_loop_targets,
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
                commands::role_templates::list_role_templates,
                commands::role_templates::get_agency_templates_status,
                commands::role_templates::update_agency_templates,
                commands::screenshot::screenshot_get_overlay_state,
                commands::screenshot::screenshot_confirm_selection,
                commands::screenshot::screenshot_cancel_capture,
                commands::screenshot::screenshot_get_hotkey_status,
                commands::screenshot::screenshot_reload_hotkey,
            ]);
            move |invoke: tauri::ipc::Invoke<tauri::Wry>| {
                observer.note_invoke(invoke.message.command());
                generated(invoke)
            }
        })
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
                    // #2296 - a gate window that dies without answering must not
                    // hold a quit round open. Synchronous: it only drops the
                    // registration, clears busy and (if that gate was still
                    // unanswered) aborts the round with reason `destroyed`.
                    if let Some(gate) = app_handle.try_state::<Arc<QuitGate>>() {
                        let gate = Arc::clone(gate.inner());
                        quit_gate_window_gone(&gate, &TauriQuitHost::new(app_handle.clone()), &label);
                    }
                    // #1363 - a destroyed window's terminal-output attachments are released
                    // here, in the backend, without any frontend cooperation. It is what keeps
                    // a window that died without detaching from leaving a session emitting to
                    // a webview that is gone, and it is why the frontend close hook does not
                    // need to block the close.
                    if let Some(pty_mgr) = app_handle
                        .try_state::<Arc<Mutex<crate::pty::manager::PtyManager>>>()
                        .map(|state| Arc::clone(state.inner()))
                    {
                        let destroyed = label.clone();
                        tauri::async_runtime::spawn(async move {
                            pty_mgr
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .release_window_attachments(&destroyed);
                        });
                    }
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
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// #2232 phase 7: the Co-managed supervisor
//
// Lives here, at the crate root, on purpose: `lib.rs` is already an SCC member,
// so the arcs the effect needs (`config::teams`, `phone::messaging`,
// `phone::mailbox`, `capture::*`) are internal to the cycle or SCC-to-leaf and
// cannot grow the 88-member SCC (phase 7 section 4).
// ═══════════════════════════════════════════════════════════════════════════

/// Attempts per trigger: the first plus two retries (plan section 5.1).
const CO_MANAGED_MAX_ATTEMPTS: u32 = 3;
/// Backoff before retry 2 and retry 3, in milliseconds. The length is derived
/// from `CO_MANAGED_MAX_ATTEMPTS` so the two cannot drift: raising the attempt
/// bound without extending the backoff table is a compile error, not a runtime
/// `index out of bounds`.
const CO_MANAGED_RETRY_BACKOFF_MS: [u64; CO_MANAGED_MAX_ATTEMPTS as usize - 1] = [25, 50];
/// Excerpt cap in UTF-8 bytes, following the 500-byte trim the bridge logger
/// already uses (`telegram/output.rs:403`). The excerpt is persisted and
/// travels over IPC and `list-peers`, so it cannot be uncapped. It is never a
/// multi-byte character cut in half.
const CO_MANAGED_EXCERPT_BYTES: usize = 500;

/// A supervisor input. (a) is the orchestrator's idle edge, emitted from the
/// real `IdleDetector` callback; (b) is a slot transition, which the slot
/// watcher forwards for every record, invalidation and clearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoManagedTrigger {
    IdleEdge(uuid::Uuid),
    SlotChanged(uuid::Uuid),
}

impl CoManagedTrigger {
    fn session_id(self) -> uuid::Uuid {
        match self {
            Self::IdleEdge(id) | Self::SlotChanged(id) => id,
        }
    }
}

#[derive(Default)]
struct CoManagedSupervisorState {
    /// Sessions whose last cycle ended in contention, keyed to the slot
    /// sequence that candidate had. The same sequence must not re-trigger.
    contended: HashMap<String, u64>,
    /// Sessions whose slot already has a watcher task.
    watching: HashSet<String>,
}

#[cfg(test)]
#[derive(Default)]
struct CoManagedTestHooks {
    steps: std::sync::Mutex<Vec<&'static str>>,
    commit_calls: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl CoManagedTestHooks {
    fn record_step(&self, step: &'static str) {
        self.steps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(step);
    }

    fn steps(&self) -> Vec<&'static str> {
        self.steps.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn commit_calls(&self) -> usize {
        self.commit_calls.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// App-wide supervisor handle. Cheap to clone; every clone shares the armed
/// flags, the contention state and the single trigger channel.
#[derive(Clone)]
struct CoManagedSupervisorHandle {
    armed: Arc<capture::armed::ArmedFlags>,
    state: Arc<Mutex<CoManagedSupervisorState>>,
    triggers: tokio::sync::mpsc::UnboundedSender<CoManagedTrigger>,
    #[cfg(test)]
    test_hooks: Option<Arc<CoManagedTestHooks>>,
}

impl CoManagedSupervisorHandle {
    fn new() -> (Self, tokio::sync::mpsc::UnboundedReceiver<CoManagedTrigger>) {
        let (triggers, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Self {
                armed: Arc::new(capture::armed::ArmedFlags::new()),
                state: Arc::new(Mutex::new(CoManagedSupervisorState::default())),
                triggers,
                #[cfg(test)]
                test_hooks: None,
            },
            rx,
        )
    }

    #[cfg(test)]
    fn with_test_hooks(
        hooks: Arc<CoManagedTestHooks>,
    ) -> (Self, tokio::sync::mpsc::UnboundedReceiver<CoManagedTrigger>) {
        let (mut handle, rx) = Self::new();
        handle.test_hooks = Some(hooks);
        (handle, rx)
    }

    fn notify_idle_edge(&self, session_id: uuid::Uuid) -> bool {
        self.triggers
            .send(CoManagedTrigger::IdleEdge(session_id))
            .is_ok()
    }

    fn notify_slot_changed(&self, session_id: uuid::Uuid) -> bool {
        self.triggers
            .send(CoManagedTrigger::SlotChanged(session_id))
            .is_ok()
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, CoManagedSupervisorState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn is_watching(&self, id: &str) -> bool {
        self.lock_state().watching.contains(id)
    }

    fn mark_watching(&self, id: &str) {
        self.lock_state().watching.insert(id.to_string());
    }

    fn mark_unwatched(&self, id: &str) {
        self.lock_state().watching.remove(id);
    }

    fn is_contended(&self, id: &str, seq: u64) -> bool {
        self.lock_state().contended.get(id) == Some(&seq)
    }

    fn mark_contended(&self, id: &str, seq: u64) {
        self.lock_state().contended.insert(id.to_string(), seq);
    }

    fn clear_contended(&self, id: &str) {
        self.lock_state().contended.remove(id);
    }

    fn record_step(&self, step: &'static str) {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            hooks.record_step(step);
        }
        let _ = step;
    }

    fn record_commit_call(&self) {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            hooks
                .commit_calls
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }
}

/// The synchronous first half of the idle edge (plan section 9.1). Emits
/// `session_idle` **first**, with the Co-managed decision in the same payload,
/// then hands the edge to the supervisor. A separate later event would always
/// paint waiting first, which is what this signature exists to prevent.
fn emit_session_idle_edge<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    armed: &capture::armed::ArmedFlags,
    supervisor: Option<&CoManagedSupervisorHandle>,
    session_id: uuid::Uuid,
) -> bool {
    let comanaged = armed.is_armed(&session_id.to_string());
    let _ = tauri::Emitter::emit(
        app,
        "session_idle",
        serde_json::json!({ "id": session_id.to_string(), "comanaged": comanaged }),
    );
    if let Some(supervisor) = supervisor {
        let _ = supervisor.notify_idle_edge(session_id);
    }
    comanaged
}

/// `session_comanaged_state`, emitted with `emit` (every window), never
/// `emit_to`, so a detached terminal window receives it too (plan 9.2).
fn emit_co_managed_state<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: uuid::Uuid,
    active: bool,
    reason: Option<&str>,
) {
    let _ = tauri::Emitter::emit(
        app,
        "session_comanaged_state",
        serde_json::json!({
            "id": session_id.to_string(),
            "active": active,
            "reason": reason,
        }),
    );
}

fn co_managed_state_reason(
    state: &Result<crate::config::co_managed::CoManagedState, String>,
) -> String {
    use crate::config::co_managed::{CoManagedState, OffReason};
    match state {
        Ok(CoManagedState::Ready) => "Ready".to_string(),
        Ok(CoManagedState::Off { reason }) => match reason {
            OffReason::NotAnOrchestrator => "NotAnOrchestrator".to_string(),
            OffReason::UnsupportedProvider { agent } => format!("UnsupportedProvider({agent})"),
            OffReason::RoomFlagOff => "RoomFlagOff".to_string(),
            OffReason::NoApiKey => "NoApiKey".to_string(),
            OffReason::NoCatalogFile => "NoCatalogFile".to_string(),
            OffReason::CatalogUnreadable => "CatalogUnreadable".to_string(),
        },
        Err(error) => format!("EffectiveStateError({error})"),
    }
}

fn abstain_reason_label(reason: &capture::state::AbstainReason) -> String {
    use capture::state::AbstainReason;
    match reason {
        AbstainReason::PreconditionsStale => "PreconditionsStale".to_string(),
        AbstainReason::LockBusy => "LockBusy".to_string(),
        AbstainReason::LockUnavailable(error) => format!("LockUnavailable({error})"),
        AbstainReason::PreconditionsRejected => "PreconditionsRejected".to_string(),
        AbstainReason::SlotChanged => "SlotChanged".to_string(),
        AbstainReason::AlreadyConsumed => "AlreadyConsumed".to_string(),
        AbstainReason::BudgetExhausted => "BudgetExhausted".to_string(),
    }
}

/// The user-visible label for a candidate. `provider_final == false` is every
/// Claude candidate (epic 3.3), and no rendering may ever call it a final
/// message or imply the user approved anything.
fn co_managed_candidate_label(provider_final: bool) -> &'static str {
    if provider_final {
        "final message"
    } else {
        "latest captured assistant text at the idle edge"
    }
}

/// The 500-byte cap, UTF-8 safe: never cut a multi-byte character in half.
fn utf8_safe_excerpt(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn co_managed_user_message(reason: &str, candidate: Option<(&str, bool, Option<&Path>)>) -> String {
    let mut out = format!("Co-managed: {reason}");
    if let Some((text, provider_final, path)) = candidate {
        let excerpt = utf8_safe_excerpt(text, CO_MANAGED_EXCERPT_BYTES);
        out.push_str(&format!(
            "\n{} ({} bytes); excerpt is {} of {} bytes:\n{}",
            co_managed_candidate_label(provider_final),
            text.len(),
            excerpt.len(),
            text.len(),
            excerpt,
        ));
        if let Some(path) = path {
            out.push_str(&format!("\nMessage file: {}", path.display()));
        }
    }
    out
}

/// Surface a user-facing communication on the session. The slot is single:
/// `raise_hand` and `set_blocked_menu` overwrite it, and when that happens the
/// pointer is lost, not the file, which stays findable in `messaging/`.
async fn surface_co_managed_user_message<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: uuid::Uuid,
    message: String,
) {
    let Some(manager) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() else {
        return;
    };
    let outcome = {
        let guard = manager.read().await;
        guard
            .set_co_managed(session_id, message, chrono::Utc::now())
            .await
    };
    if let Some((changed, communication)) = outcome {
        if changed {
            crate::session::selection::publish_session_communication(
                app,
                session_id,
                Some(&communication),
            );
        }
    }
}

/// Read the six `AppSettings` Jev fields into the leaf's plain value type.
async fn co_managed_jev_settings<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> capture::jev::JevSettings {
    let settings = app.state::<SettingsState>();
    let guard = settings.read().await;
    capture::jev::JevSettings {
        api_key: guard.jev_api_key.clone(),
        model: guard.jev_model.clone(),
        endpoint: guard.jev_endpoint.clone(),
        timeout_secs: guard.jev_timeout_secs,
        threshold: guard.jev_threshold,
        margin: guard.jev_margin,
    }
}

/// Load and validate the room's catalog through the leaf loader. A missing
/// path or a missing file is `Catalog::missing()`, which `classify` turns into
/// an abstention with `NoCatalogFile`.
fn co_managed_catalog(room_root: &Path) -> capture::catalog::Catalog {
    let config = crate::config::co_managed::load_config(room_root);
    let Some(path) = config.catalog_path.as_deref() else {
        return capture::catalog::Catalog::missing();
    };
    let candidate = Path::new(path);
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        room_root.join(candidate)
    };
    capture::catalog::Catalog::load(&resolved)
}

// ── the routed body is a pointer, not the text (plan section 7) ─────────────

/// Write the message file into the sending orchestrator's own room
/// `messaging/`, then return `(path, pointer)`. The pointer is the canonical
/// `Process this inter-agent message: <abs path>` body.
///
/// `ensure_workgroup_root_is_authoritative` is called explicitly: the CLI did
/// that for every file it wrote, and an in-process caller does not inherit it.
fn write_co_managed_message_file(
    room_root: &Path,
    from_fqn: &str,
    to_fqn: &str,
    content: &str,
) -> Result<(PathBuf, String), String> {
    crate::phone::messaging::ensure_workgroup_root_is_authoritative(room_root)?;
    let dir = crate::phone::messaging::messaging_dir(room_root).map_err(|e| e.to_string())?;
    let from_short = crate::phone::messaging::agent_short_name(from_fqn);
    let to_short = crate::phone::messaging::agent_short_name(to_fqn);
    let slug = crate::phone::messaging::sanitize_slug("co-managed").map_err(|e| e.to_string())?;
    let base =
        crate::phone::messaging::build_filename(chrono::Utc::now(), &from_short, &to_short, &slug);
    let (path, mut file) =
        crate::phone::messaging::create_message_file(&dir, &base).map_err(|e| e.to_string())?;
    use std::io::Write;
    let written = (|| -> Result<(), String> {
        file.write_all(content.as_bytes())
            .map_err(|e| format!("message file write failed: {e}"))?;
        file.flush()
            .map_err(|e| format!("message file flush failed: {e}"))
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    let abs = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
    // UNC-strip at the single emission site, exactly as `cli/send` does.
    let abs_str = abs.to_string_lossy();
    let abs_display = abs_str.trim_start_matches(r"\\?\");
    Ok((
        path,
        crate::phone::messaging::format_file_notification(abs_display),
    ))
}

/// The queue envelope: a wake carrying the pointer, no token, no action field.
fn write_co_managed_queue_message(
    room_root: &Path,
    from_fqn: &str,
    to_fqn: &str,
    pointer: &str,
) -> Result<PathBuf, String> {
    let dir = crate::config::co_managed::queue_dir(room_root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("queue dir create failed: {e}"))?;
    let envelope = crate::phone::types::OutboxMessage {
        id: uuid::Uuid::new_v4().to_string(),
        token: None,
        from: from_fqn.to_string(),
        to: to_fqn.to_string(),
        body: pointer.to_string(),
        mode: "wake".to_string(),
        get_output: false,
        request_id: None,
        sender_agent: None,
        preferred_agent: "auto".to_string(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: None,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    };
    let bytes = serde_json::to_vec_pretty(&envelope)
        .map_err(|e| format!("queue envelope serialize failed: {e}"))?;
    let path = dir.join(format!("{}.json", envelope.id));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(&path)
        .map_err(|e| format!("queue envelope create failed: {}", e))?;
    use std::io::Write;
    file.write_all(&bytes)
        .map_err(|e| format!("queue envelope write failed: {e}"))?;
    file.flush()
        .map_err(|e| format!("queue envelope flush failed: {e}"))?;
    Ok(path)
}

/// Create the message file, then enqueue the pointer. The file exists before
/// the queue entry so the pointer resolves when the recipient reads it. The
/// line budget is the CLI's formula with the **composed** sender, which is
/// longer; a body that does not fit is rejected with a visible reason and is
/// never truncated.
fn route_co_managed_wake(
    handle: &CoManagedSupervisorHandle,
    room_root: &Path,
    from_fqn: &str,
    to_fqn: &str,
    content: &str,
) -> Result<(), String> {
    let (path, pointer) = write_co_managed_message_file(room_root, from_fqn, to_fqn, content)?;
    handle.record_step("messaging_file");
    let display_from = crate::phone::messaging::compose_sender_for_comanaged_origin(from_fqn);
    let overhead = crate::phone::messaging::PTY_WRAP_FIXED + display_from.len();
    if pointer.len() + overhead > crate::phone::messaging::PTY_SAFE_MAX {
        // Nothing was delivered; the file was only just created by us, so it is
        // removed rather than left as a never-referenced orphan.
        let _ = std::fs::remove_file(&path);
        return Err(format!(
            "notification exceeds PTY-safe length (body {} + overhead {} > {}); shorten slug or move the room to a shallower path",
            pointer.len(),
            overhead,
            crate::phone::messaging::PTY_SAFE_MAX
        ));
    }
    write_co_managed_queue_message(room_root, from_fqn, to_fqn, &pointer)?;
    handle.record_step("queue_file");
    Ok(())
}

// ── routing decision ────────────────────────────────────────────────────────

enum CoManagedRoute {
    /// Text for the user, never routed. `write_candidate_file` is false only in
    /// the secret case, where phase 6 forbids any file.
    User {
        reason: String,
        write_candidate_file: bool,
    },
    /// A wake carrying a pointer. Spending budget is decided by the caller.
    Wake {
        to: String,
        content: String,
        reason: String,
    },
}

impl CoManagedRoute {
    fn kind(&self) -> capture::state::EffectKind {
        match self {
            Self::User { .. } => capture::state::EffectKind::TextToUser,
            Self::Wake { .. } => capture::state::EffectKind::Automatic,
        }
    }
}

/// Settings project paths plus the project derived from the room root, the
/// same effective slice the CLI (`cli::send::execute`) and the mailbox's
/// Co-managed branch build before the Root check. The queue lives under
/// `<project>/.ac/<room>`, so the project is the room root's grandparent; a
/// room whose project is not (or no longer) registered in settings must still
/// let a verified coordinator reach Root (F4).
fn co_managed_project_paths(settings_paths: &[String], room_root: &Path) -> Vec<String> {
    let mut paths = settings_paths.to_vec();
    let Some(project_dir) = room_root.parent().and_then(|ac_root| ac_root.parent()) else {
        return paths;
    };
    let canon_project = std::fs::canonicalize(project_dir).ok();
    let already_present = paths.iter().any(|p| match &canon_project {
        Some(canon_target) => std::fs::canonicalize(p).ok().as_ref() == Some(canon_target),
        None => Path::new(p) == project_dir,
    });
    if !already_present {
        paths.push(project_dir.to_string_lossy().to_string());
    }
    paths
}

/// The authorization branch is mandatory, not a style choice: `can_communicate`
/// returns false for `Root` by its own rules, because Root belongs to no team
/// and is nobody's coordinator. Root goes through the verified-coordinator
/// validator; every other destination goes through `can_communicate`.
fn resolve_co_managed_route(
    outcome: capture::jev::ClassifyOutcome,
    catalog: &capture::catalog::Catalog,
    from_fqn: &str,
    project_paths: &[String],
    candidate_text: &str,
) -> CoManagedRoute {
    use capture::catalog::{Destination, Resolution};
    match outcome {
        capture::jev::ClassifyOutcome::Abstained { reason } => CoManagedRoute::User {
            reason: format!("abstained: {reason}"),
            write_candidate_file: true,
        },
        capture::jev::ClassifyOutcome::Classified {
            category,
            destination,
            score,
            runner_up,
        } => {
            let decision =
                format!("category '{category}' (noul {score:.2}, runner-up {runner_up:.2})");
            match destination {
                Destination::User => CoManagedRoute::User {
                    reason: format!("{decision} routes to the user"),
                    write_candidate_file: true,
                },
                Destination::Orchestrator => match catalog.resolve(&category) {
                    Resolution::Valid {
                        peer: Some(peer), ..
                    } => {
                        let discovered_teams = crate::config::teams::discover_teams();
                        if crate::config::teams::can_communicate(from_fqn, &peer, &discovered_teams)
                        {
                            CoManagedRoute::Wake {
                                to: peer.clone(),
                                content: candidate_text.to_string(),
                                reason: format!("{decision} routed to {peer}"),
                            }
                        } else {
                            CoManagedRoute::User {
                                reason: format!(
                                    "{decision} names peer '{peer}', which is not reachable; nothing was routed"
                                ),
                                write_candidate_file: true,
                            }
                        }
                    }
                    _ => CoManagedRoute::User {
                        reason: format!(
                            "{decision} names an orchestrator destination without a peer; nothing was routed"
                        ),
                        write_candidate_file: true,
                    },
                },
                Destination::Root => {
                    if crate::config::teams::verified_wg_coordinator_target(from_fqn, project_paths)
                        .is_some()
                    {
                        CoManagedRoute::Wake {
                            to: crate::config::root_agent::ROOT_AGENT_SENDER.to_string(),
                            content: candidate_text.to_string(),
                            reason: format!("{decision} routed to the Root Agent"),
                        }
                    } else {
                        CoManagedRoute::User {
                            reason: format!(
                                "{decision} chose Root, but this session is not a verified room orchestrator; nothing was routed"
                            ),
                            write_candidate_file: true,
                        }
                    }
                }
                Destination::DefaultReply => match catalog.resolve(&category) {
                    Resolution::Valid {
                        reply: Some(reply), ..
                    } => CoManagedRoute::Wake {
                        to: from_fqn.to_string(),
                        content: reply,
                        reason: format!("{decision} replied to this session"),
                    },
                    _ => CoManagedRoute::User {
                        reason: format!("{decision} has no reply; nothing was routed"),
                        write_candidate_file: true,
                    },
                },
            }
        }
    }
}

// ── the commit and its retry policy ─────────────────────────────────────────

enum CoManagedCommit {
    Committed(capture::state::CommittedEffect),
    Contention,
    Rejected(capture::state::AbstainReason),
}

/// The preconditions, gathered fresh immediately before each attempt. No lock
/// of ours is held across the awaits; the results travel in as a typed value.
async fn gather_co_managed_preconditions<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: uuid::Uuid,
    room_root: &Path,
    candidate: &capture::record::CapturedRecord,
    kind: capture::state::EffectKind,
) -> capture::state::EffectPreconditions {
    let (session_alive, anchor, unique_live_session_for_cwd) = {
        let Some(manager) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() else {
            return capture::state::EffectPreconditions {
                session_alive: false,
                session_id: session_id.to_string(),
                anchor: String::new(),
                provider: candidate.provider,
                unique_live_session_for_cwd: false,
                no_pending_user_input: false,
                effective_ready: false,
                observed_at: Instant::now(),
                kind,
            };
        };
        let guard = manager.read().await;
        let session = guard.get_session(session_id).await;
        let anchor = session
            .as_ref()
            .map(|s| s.working_directory.clone())
            .unwrap_or_default();
        let sessions = guard.list_sessions().await;
        let unique = sessions
            .iter()
            .filter(|s| {
                s.working_directory == anchor
                    && !matches!(s.status, crate::session::session::SessionStatus::Exited(_))
            })
            .count()
            == 1;
        (session.is_some(), anchor, unique)
    };

    let no_pending_user_input =
        match app.try_state::<crate::pty::input_activity::SubstantiveInputState>() {
            Some(state) => {
                let tracker = state.lock().unwrap_or_else(|e| e.into_inner());
                !tracker.pending_within(
                    session_id,
                    crate::pty::input_activity::USER_WRITE_STAMP_WINDOW,
                )
            }
            None => true,
        };

    let effective_ready = matches!(
        crate::commands::session::co_managed_effective_state_for_session(
            app,
            room_root,
            &session_id.to_string()
        )
        .await,
        Ok(crate::config::co_managed::CoManagedState::Ready)
    );

    capture::state::EffectPreconditions {
        session_alive,
        session_id: session_id.to_string(),
        anchor,
        provider: candidate.provider,
        unique_live_session_for_cwd,
        no_pending_user_input,
        effective_ready,
        observed_at: Instant::now(),
        kind,
    }
}

/// `commit_effect` is synchronous and takes a blocking advisory file lock for
/// up to `LOCK_WAIT_BUDGET` (100 ms). Called directly it would park a Tokio
/// worker on every idle edge, so it runs on the blocking pool and a join error
/// is an abstention, never a silent success.
async fn commit_co_managed_effect(
    room_root: PathBuf,
    slot: capture::sink::CaptureSlot,
    expected_seq: u64,
    expected_key: capture::key::ConsumptionKey,
    pre: capture::state::EffectPreconditions,
) -> Result<capture::state::CommittedEffect, capture::state::AbstainReason> {
    tauri::async_runtime::spawn_blocking(move || {
        capture::state::commit_effect(&room_root, &slot, expected_seq, &expected_key, &pre)
    })
    .await
    .unwrap_or(Err(capture::state::AbstainReason::LockUnavailable(
        "commit task join failed".to_string(),
    )))
}

/// Three attempts per trigger, the first plus two retries, with a fresh full
/// snapshot before each; `PreconditionsStale` and `LockBusy` are the only
/// retryable outcomes. The ceiling is the executable form of the bound: under
/// contention the feature abstains with a reason instead of looping forever.
#[allow(clippy::too_many_arguments)]
async fn commit_co_managed_with_retries<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
    session_id: uuid::Uuid,
    room_root: &Path,
    slot: &capture::sink::CaptureSlot,
    expected_seq: u64,
    expected_key: &capture::key::ConsumptionKey,
    candidate: &capture::record::CapturedRecord,
    kind: capture::state::EffectKind,
) -> CoManagedCommit {
    for attempt in 0..CO_MANAGED_MAX_ATTEMPTS {
        if attempt > 0 {
            let backoff = CO_MANAGED_RETRY_BACKOFF_MS[(attempt - 1) as usize];
            tokio::time::sleep(Duration::from_millis(backoff)).await;
        }
        let pre =
            gather_co_managed_preconditions(app, session_id, room_root, candidate, kind).await;
        handle.record_commit_call();
        let result = commit_co_managed_effect(
            room_root.to_path_buf(),
            slot.clone(),
            expected_seq,
            expected_key.clone(),
            pre,
        )
        .await;
        match result {
            Ok(committed) => return CoManagedCommit::Committed(committed),
            Err(capture::state::AbstainReason::PreconditionsStale)
            | Err(capture::state::AbstainReason::LockBusy) => continue,
            Err(error) => return CoManagedCommit::Rejected(error),
        }
    }
    handle.mark_contended(&session_id.to_string(), expected_seq);
    CoManagedCommit::Contention
}

enum CoManagedOutcome {
    Contention,
    Done(String),
}

/// One full effect: secrets first, then classification, then the bounded
/// commit, then the action. Every path returns the reason the cycle ended.
async fn co_managed_cycle<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
    session_id: uuid::Uuid,
    room_root: &Path,
    slot: &capture::sink::CaptureSlot,
    expected_seq: u64,
    candidate: &Arc<capture::record::CapturedRecord>,
) -> CoManagedOutcome {
    let session = {
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = manager.read().await;
        guard.get_session(session_id).await
    };
    let Some(session) = session else {
        return CoManagedOutcome::Done("session vanished".to_string());
    };
    let from_fqn = crate::config::teams::agent_fqn_from_path(&session.working_directory);
    let expected_key = capture::state::consumption_key(room_root, candidate.as_ref());
    let text = candidate.text.clone();

    // (1) The detector runs before any write and before any network call.
    if let Some(detection) = capture::secrets::detect(&text) {
        let reason = detection.reason();
        let user_message = co_managed_user_message(
            &format!("{reason}; no file was written and nothing was routed"),
            None,
        );
        return match commit_co_managed_with_retries(
            app,
            handle,
            session_id,
            room_root,
            slot,
            expected_seq,
            &expected_key,
            candidate.as_ref(),
            capture::state::EffectKind::TextToUser,
        )
        .await
        {
            CoManagedCommit::Committed(_) => {
                // The candidate is consumed: clearing the slot publishes the
                // transition, and a later idle edge can never act on it again.
                slot.clear();
                surface_co_managed_user_message(app, session_id, user_message).await;
                CoManagedOutcome::Done(reason)
            }
            CoManagedCommit::Contention => CoManagedOutcome::Contention,
            CoManagedCommit::Rejected(reason) => {
                CoManagedOutcome::Done(abstain_reason_label(&reason))
            }
        };
    }

    // (2) Classify through the phase-6 leaf call.
    let catalog = co_managed_catalog(room_root);
    let settings = co_managed_jev_settings(app).await;
    let outcome = {
        let network = app.state::<crate::network::OutboundNetwork>();
        capture::jev::classify(&network, &settings, &catalog, &text).await
    };

    // (3) Decide before committing, so an unreachable peer never spends budget.
    let project_paths = {
        let settings = app.state::<SettingsState>();
        let guard = settings.read().await;
        co_managed_project_paths(&guard.project_paths, room_root)
    };
    let route = resolve_co_managed_route(outcome, &catalog, &from_fqn, &project_paths, &text);
    let kind = route.kind();

    // (4) The bounded commit.
    match commit_co_managed_with_retries(
        app,
        handle,
        session_id,
        room_root,
        slot,
        expected_seq,
        &expected_key,
        candidate.as_ref(),
        kind,
    )
    .await
    {
        CoManagedCommit::Committed(committed) => {
            // Consumed: the slot must not offer this candidate again on the
            // next idle edge, and the transition clears the armed flag.
            slot.clear();
            // `routable` is false for a baseline consumed by an Automatic
            // request (the plan's section 8 demotion). A TextToUser commit is
            // equally not routable, but its effect is the user message below.
            if committed.kind == capture::state::EffectKind::Automatic && !committed.routable {
                return CoManagedOutcome::Done(
                    "baseline record consumed; never routed".to_string(),
                );
            }
            perform_co_managed_route(
                app, handle, session_id, room_root, &from_fqn, candidate, route, &text,
            )
            .await
        }
        CoManagedCommit::Contention => CoManagedOutcome::Contention,
        CoManagedCommit::Rejected(reason) => {
            if matches!(reason, capture::state::AbstainReason::BudgetExhausted)
                && kind == capture::state::EffectKind::Automatic
            {
                // Budget exhaustion abstains from the automatic action and
                // falls back to the budget-free text-to-user channel, so the
                // user sees why nothing was routed and nothing is enqueued.
                return match commit_co_managed_with_retries(
                    app,
                    handle,
                    session_id,
                    room_root,
                    slot,
                    expected_seq,
                    &expected_key,
                    candidate.as_ref(),
                    capture::state::EffectKind::TextToUser,
                )
                .await
                {
                    CoManagedCommit::Committed(_) => {
                        slot.clear();
                        let message = co_managed_user_message(
                            &format!(
                                "abstained: the automatic budget is exhausted ({} actions since the last recharge); the candidate was not routed",
                                capture::state::BUDGET_CAP
                            ),
                            Some((&text, candidate.provider_final, None)),
                        );
                        surface_co_managed_user_message(app, session_id, message).await;
                        CoManagedOutcome::Done("BudgetExhausted".to_string())
                    }
                    CoManagedCommit::Contention => CoManagedOutcome::Contention,
                    CoManagedCommit::Rejected(second) => {
                        CoManagedOutcome::Done(abstain_reason_label(&second))
                    }
                };
            }
            CoManagedOutcome::Done(abstain_reason_label(&reason))
        }
    }
}

/// The committed effect. A `Rejected` automatic commit falls back to the
/// budget-free text-to-user channel so the user sees the reason; a route
/// failure after the commit is reported there too (the declared residual: the
/// reservation is not rolled back).
#[allow(clippy::too_many_arguments)]
async fn perform_co_managed_route<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
    session_id: uuid::Uuid,
    room_root: &Path,
    from_fqn: &str,
    candidate: &Arc<capture::record::CapturedRecord>,
    route: CoManagedRoute,
    text: &str,
) -> CoManagedOutcome {
    match route {
        CoManagedRoute::User {
            reason,
            write_candidate_file,
        } => {
            let mut message = co_managed_user_message(&reason, None);
            if write_candidate_file {
                // Except in the secret case, the candidate is also written as a
                // file in `messaging/`, and the communication carries the
                // reason, an excerpt with its real length, and the path.
                match write_co_managed_message_file(room_root, from_fqn, from_fqn, text) {
                    Ok((path, _pointer)) => {
                        handle.record_step("messaging_file");
                        message = co_managed_user_message(
                            &reason,
                            Some((text, candidate.provider_final, Some(path.as_path()))),
                        );
                    }
                    Err(error) => {
                        message.push_str(&format!("\nmessage file write failed: {error}"));
                    }
                }
            }
            surface_co_managed_user_message(app, session_id, message).await;
            CoManagedOutcome::Done(reason)
        }
        CoManagedRoute::Wake {
            to,
            content,
            reason,
        } => match route_co_managed_wake(handle, room_root, from_fqn, &to, &content) {
            Ok(()) => CoManagedOutcome::Done(reason),
            Err(error) => {
                let message = co_managed_user_message(
                    &format!("routing failed after the effect was reserved: {error}"),
                    Some((text, candidate.provider_final, None)),
                );
                surface_co_managed_user_message(app, session_id, message).await;
                CoManagedOutcome::Done(format!("route failed: {error}"))
            }
        },
    }
}

/// One supervisor input. Trigger (b) runs a cycle only when the session is
/// already idle; any other slot transition only keeps the armed flag current,
/// which is how the idle edge finds it set.
async fn handle_co_managed_trigger<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
    trigger: CoManagedTrigger,
) {
    let session_id = trigger.session_id();
    let id = session_id.to_string();

    let Some(registry) = app.try_state::<Arc<capture::registry::CaptureRegistry>>() else {
        return;
    };
    let Some(slot) = registry.slot(&id) else {
        handle.armed.remove(&id);
        return;
    };
    let state = slot.snapshot();
    let Some(candidate) = state.value.record().cloned() else {
        // Consumed, invalidated or cleared: the candidate this flag described
        // is gone, so the flag goes with it.
        handle.armed.remove(&id);
        return;
    };

    let session = {
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = manager.read().await;
        guard.get_session(session_id).await
    };
    let Some(session) = session else {
        handle.armed.remove(&id);
        return;
    };
    let Some(room_root) =
        crate::config::co_managed::room_root_for_path(Path::new(&session.working_directory))
    else {
        handle.armed.remove(&id);
        return;
    };

    let effective =
        crate::commands::session::co_managed_effective_state_for_session(app, &room_root, &id)
            .await;
    let ready = matches!(
        effective,
        Ok(crate::config::co_managed::CoManagedState::Ready)
    );
    let was_armed = handle.armed.is_armed(&id);
    if !ready {
        if was_armed {
            emit_co_managed_state(
                app,
                session_id,
                false,
                Some(&co_managed_state_reason(&effective)),
            );
        }
        handle.armed.remove(&id);
        return;
    }

    // Ready with an unconsumed candidate: the idle edge must find the flag set.
    handle.armed.arm(&id);

    // Contention leaves the candidate pending. A trigger for the SAME sequence
    // must not re-run it; a new record changes the sequence and re-triggers.
    // This check sits AFTER the readiness gate (F1): a session that loses
    // readiness while contended must clear the armed flag and publish the
    // readiness reason, not keep reporting `comanaged: true` forever.
    if handle.is_contended(&id, state.seq) {
        return;
    }

    let already_idle = matches!(session.status, crate::session::session::SessionStatus::Idle)
        || session.waiting_for_input;
    let is_idle_edge = matches!(trigger, CoManagedTrigger::IdleEdge(_));
    if !is_idle_edge && !already_idle {
        return;
    }
    if !is_idle_edge {
        // Trigger (b): a record arrived while the session was already idle.
        // This transition does not coincide with the idle edge, so it carries
        // its own event before the cycle performs anything.
        emit_co_managed_state(app, session_id, true, None);
    }

    let outcome = co_managed_cycle(
        app, handle, session_id, &room_root, &slot, state.seq, &candidate,
    )
    .await;
    match outcome {
        CoManagedOutcome::Contention => {
            emit_co_managed_state(app, session_id, false, Some("contention"));
            // The flag stays set while the unchanged candidate remains pending.
        }
        CoManagedOutcome::Done(reason) => {
            handle.clear_contended(&id);
            handle.armed.disarm(&id);
            emit_co_managed_state(app, session_id, false, Some(&reason));
        }
    }
}

/// Start the app-wide supervisor. The discovery tick subscribes a watcher to
/// every live session's slot; the watcher forwards each slot transition as
/// trigger (b) and keeps the armed flag current on records while the session
/// is still busy.
fn spawn_co_managed_supervisor<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    handle: CoManagedSupervisorHandle,
    mut triggers: tokio::sync::mpsc::UnboundedReceiver<CoManagedTrigger>,
    shutdown: CancellationToken,
) {
    tauri::async_runtime::spawn(async move {
        let mut discovery = tokio::time::interval(Duration::from_millis(500));
        discovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                _ = discovery.tick() => watch_capture_slots(&app, &handle).await,
                trigger = triggers.recv() => {
                    let Some(trigger) = trigger else { break };
                    handle_co_managed_trigger(&app, &handle, trigger).await;
                }
            }
        }
    });
}

async fn watch_capture_slots<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
) {
    let Some(registry) = app.try_state::<Arc<capture::registry::CaptureRegistry>>() else {
        return;
    };
    let session_ids: Vec<uuid::Uuid> = {
        let Some(manager) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() else {
            return;
        };
        let guard = manager.read().await;
        guard
            .list_sessions()
            .await
            .into_iter()
            .filter_map(|s| uuid::Uuid::parse_str(&s.id).ok())
            .collect()
    };
    for session_id in session_ids {
        let id = session_id.to_string();
        if handle.is_watching(&id) {
            continue;
        }
        let Some(slot) = registry.slot(&id) else {
            continue;
        };
        handle.mark_watching(&id);
        // Subscribe FIRST, then evaluate the slot's CURRENT value immediately:
        // `subscribe()` marks the existing state as seen, so without the
        // immediate trigger a candidate that arrived before discovery would
        // wait for the next transition. Subscribing first closes the race with
        // a record arriving between the two steps.
        let mut changes = slot.subscribe();
        let _ = handle.notify_slot_changed(session_id);
        let watcher_handle = handle.clone();
        tokio::spawn(async move {
            loop {
                if changes.changed().await.is_err() {
                    break;
                }
                if !watcher_handle.notify_slot_changed(session_id) {
                    break;
                }
            }
            watcher_handle.mark_unwatched(&id);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_main_display_state, centered_default_main_geometry, effective_main_display_state,
        is_visible_on_monitors, normalize_persisted_active_flags, physical_to_logical,
        prepare_app_outbox, resolve_is_coord_for_restore, restore_session_should_become_active,
        restore_session_should_wake, should_auto_create_root_agent_on_first_restore,
        should_wake_on_restore, should_wake_root_agent_on_restore,
        should_wake_working_agent_on_restore, skip_auto_resume_for_restore, ApiServerHandle,
        ApiServerTask, ContextPatternSource, ContextSample, ContextSampleSink,
        PersistedActiveFlagNormalization, RestoreObserverStartBarrier, ScraperPatterns,
        ScraperSamples, SettingsState, StartupError, StartupErrorKind, WebServerHandle,
        WebServerLifecycle, WebServerLifecycleSnapshot, WebServerStopWaiter,
        WEB_SERVER_START_CANCELLED,
    };
    use crate::config::sessions_persistence::PersistedSession;
    use crate::config::settings::{
        AgentConfig, AppSettings, MainWindowDisplayState, WindowGeometry,
    };
    use crate::testability::window_placement::TestWindowPlacement;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::{oneshot, watch};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn issue_1577_app_outbox_error_is_typed_and_exact() {
        let config_dir = std::path::PathBuf::from("config-root");
        let app_outbox_path = config_dir.join("instances/fixed/outbox");
        let error = StartupError {
            kind: StartupErrorKind::AppOutboxCreate {
                config_dir: config_dir.clone(),
                app_outbox_path: app_outbox_path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
            },
        };
        assert_eq!(
            error.to_string(),
            format!(
                "AgentsCommander cannot start because it could not create app outbox directory \"{}\" for configuration directory \"{}\": denied. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.",
                app_outbox_path.display(),
                config_dir.display()
            )
        );
        let StartupErrorKind::AppOutboxCreate { source, .. } = &error.kind else {
            panic!("expected typed app-outbox error");
        };
        assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn issue_1577_prepare_app_outbox_creates_the_shared_path() {
        let temp = tempfile::TempDir::new().unwrap();
        let instance_id = "00000000-0000-4000-8000-000000001577";
        let (path, outbox) = prepare_app_outbox(temp.path(), instance_id).unwrap();
        let expected = temp
            .path()
            .join("instances")
            .join(instance_id)
            .join("outbox");
        assert_eq!(path, expected);
        assert_eq!(outbox.path(), expected.to_string_lossy());
        assert!(expected.is_dir());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn issue_1930_linux_unmarked_adjacent_unwritable_refuses_to_start() {
        use std::fs::OpenOptions;
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Command, Stdio};
        use std::thread;
        use std::time::{Duration, Instant};

        const CHILD_SENTINEL: &str = "AGENTSCOMMANDER_ISSUE_1577_CHILD";
        const TEST_NAME: &str =
            "tests::issue_1930_linux_unmarked_adjacent_unwritable_refuses_to_start";

        if std::env::var_os(CHILD_SENTINEL).is_some() {
            let executable = std::env::current_exe().unwrap();
            let case_root = executable.parent().unwrap();
            let adjacent = case_root.join(".agentscommander_issue1577_linux_subprocess");

            assert_eq!(crate::config::config_dir(), Some(adjacent.clone()));
            let Some(crate::config::ConfigStartupError::AdjacentDirectoryUnwritable {
                config_dir,
                reason,
            }) = crate::config::config_startup_error()
            else {
                panic!("an unwritable unmarked adjacent directory must refuse startup");
            };
            assert_eq!(config_dir, adjacent);
            let reason_prefix = format!(
                "write probe could not create configuration directory \"{}\" after 1 attempt(s): ",
                adjacent.display()
            );
            assert!(
                reason.starts_with(&reason_prefix),
                "unexpected reason: {reason}"
            );
            let refusal = crate::preflight_config_startup().expect_err("startup must be refused");
            assert_eq!(
                refusal.to_string(),
                format!(
                    "AgentsCommander cannot start because it cannot write its configuration directory \"{}\" next to the executable: {}{} Move the executable to a writable folder, or set AGENTSCOMMANDER_CONFIG_DIR to a writable directory, and restart.",
                    adjacent.display(),
                    reason,
                    if reason.ends_with('.') { "" } else { "." }
                )
            );
            return;
        }

        let case_temp = tempfile::TempDir::new().unwrap();
        let home_temp = tempfile::TempDir::new().unwrap();
        let case_root = case_temp.path().to_path_buf();
        let home_root = home_temp.path().to_path_buf();
        let original_mode = std::fs::metadata(&case_root).unwrap().permissions().mode();
        let copied_executable = case_root.join("agentscommander_issue1577_linux_subprocess");
        let adjacent = case_root.join(".agentscommander_issue1577_linux_subprocess");

        let body = (|| -> Result<(), String> {
            let source_executable =
                std::env::current_exe().map_err(|error| format!("current_exe failed: {error}"))?;
            std::fs::copy(&source_executable, &copied_executable).map_err(|error| {
                format!(
                    "copy {} -> {} failed: {error}",
                    source_executable.display(),
                    copied_executable.display()
                )
            })?;
            let mut executable_permissions = std::fs::metadata(&copied_executable)
                .map_err(|error| format!("copied executable metadata failed: {error}"))?
                .permissions();
            executable_permissions.set_mode(executable_permissions.mode() | 0o111);
            std::fs::set_permissions(&copied_executable, executable_permissions)
                .map_err(|error| format!("set executable mode failed: {error}"))?;

            if adjacent.exists() {
                return Err(format!("fixture not fresh adjacent={}", adjacent.display()));
            }

            let mut read_only_permissions = std::fs::metadata(&case_root)
                .map_err(|error| format!("case-root metadata failed: {error}"))?
                .permissions();
            read_only_permissions.set_mode(0o555);
            std::fs::set_permissions(&case_root, read_only_permissions)
                .map_err(|error| format!("chmod 0555 {} failed: {error}", case_root.display()))?;

            let fixture_probe = case_root.join("fixture-write-preflight.tmp");
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&fixture_probe)
            {
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
                Err(error) => {
                    return Err(format!(
                        "0555 preflight returned {:?} instead of PermissionDenied for {}: {error}",
                        error.kind(),
                        fixture_probe.display()
                    ));
                }
                Ok(file) => {
                    drop(file);
                    let mut restored = std::fs::metadata(&case_root)
                        .map_err(|error| format!("restore metadata failed: {error}"))?
                        .permissions();
                    restored.set_mode(original_mode);
                    std::fs::set_permissions(&case_root, restored).map_err(|error| {
                        format!("restore after false preflight failed: {error}")
                    })?;
                    std::fs::remove_file(&fixture_probe).map_err(|error| {
                        format!("remove false-positive fixture probe failed: {error}")
                    })?;
                    return Err(format!(
                        "mode 0555 did not block create_new in {}; refusing false pass",
                        case_root.display()
                    ));
                }
            }

            let mut child = Command::new(&copied_executable)
                .arg(TEST_NAME)
                .arg("--exact")
                .arg("--nocapture")
                .arg("--test-threads=1")
                .env("HOME", &home_root)
                .env_remove("AGENTSCOMMANDER_CONFIG_DIR")
                .env_remove("AGENTSCOMMANDER_TEST_CONFIG_DIR")
                .env(CHILD_SENTINEL, "1")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| {
                    format!(
                        "spawn copied child {} failed: {error}",
                        copied_executable.display()
                    )
                })?;

            let deadline = Instant::now() + Duration::from_secs(15);
            let mut status_poll_error = None;
            let terminal_status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Ok(None) => break None,
                    Err(error) => {
                        status_poll_error = Some(error);
                        break None;
                    }
                }
            };

            if terminal_status.is_none() {
                let kill_error = child.kill().err();
                let reap_deadline = Instant::now() + Duration::from_secs(5);
                let reaped = loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break true,
                        Ok(None) if Instant::now() < reap_deadline => {
                            thread::sleep(Duration::from_millis(25));
                        }
                        Ok(None) | Err(_) => break false,
                    }
                };
                if !reaped {
                    return Err(format!(
                        "child did not reap within 5 seconds; kill_error={kill_error:?} poll_error={status_poll_error:?} case_root={} home_root={}",
                        case_root.display(),
                        home_root.display()
                    ));
                }
                let output = child
                    .wait_with_output()
                    .map_err(|error| format!("timed-out child final reap failed: {error}"))?;
                return Err(format!(
                    "child did not reach a clean terminal status; kill_error={kill_error:?} poll_error={status_poll_error:?} status={:?} stdout={:?} stderr={:?} case_root={} home_root={}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                    case_root.display(),
                    home_root.display()
                ));
            }

            let output = child
                .wait_with_output()
                .map_err(|error| format!("child output collection failed: {error}"))?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !output.status.success() {
                return Err(format!(
                    "child failed status={:?} stdout={stdout:?} stderr={stderr:?} case_root={} home_root={}",
                    output.status.code(),
                    case_root.display(),
                    home_root.display()
                ));
            }
            for text in [&*stdout, &*stderr] {
                if text.contains("panicked at") || text.contains("stack backtrace:") {
                    return Err(format!(
                        "child emitted panic/backtrace text stdout={stdout:?} stderr={stderr:?}"
                    ));
                }
            }

            let home_entries: Vec<_> = std::fs::read_dir(&home_root)
                .map_err(|error| format!("read home failed: {error}"))?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<Result<_, _>>()
                .map_err(|error| format!("read home entry failed: {error}"))?;
            if !home_entries.is_empty() {
                return Err(format!(
                    "a suffixed build wrote into HOME: {home_entries:?}"
                ));
            }
            if adjacent.exists() {
                return Err(format!(
                    "adjacent state appeared adjacent={}",
                    adjacent.display()
                ));
            }
            let case_entries: Vec<_> = std::fs::read_dir(&case_root)
                .map_err(|error| format!("read case root failed: {error}"))?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<Result<_, _>>()
                .map_err(|error| format!("read case-root entry failed: {error}"))?;
            if case_entries
                != [copied_executable
                    .file_name()
                    .expect("copied executable file name")
                    .to_os_string()]
            {
                return Err(format!("unexpected case-root entries: {case_entries:?}"));
            }
            Ok(())
        })();

        let restore_result = (|| -> Result<(), String> {
            let mut permissions = std::fs::metadata(&case_root)
                .map_err(|error| format!("mode-restore metadata failed: {error}"))?
                .permissions();
            permissions.set_mode(original_mode);
            std::fs::set_permissions(&case_root, permissions).map_err(|error| {
                format!(
                    "restore exact mode on {} failed: {error}",
                    case_root.display()
                )
            })
        })();
        let case_close = case_temp
            .close()
            .map_err(|error| format!("close case TempDir {} failed: {error}", case_root.display()));
        let home_close = home_temp
            .close()
            .map_err(|error| format!("close home TempDir {} failed: {error}", home_root.display()));

        let mut failures = Vec::new();
        if let Err(error) = body {
            failures.push(error);
        }
        if let Err(error) = restore_result {
            failures.push(error);
        }
        if let Err(error) = case_close {
            failures.push(error);
        }
        if let Err(error) = home_close {
            failures.push(error);
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

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
                order: None,
                envs: Vec::new(),
                isolated_home: false,
                instructions_filename: None,
                config_seed: None,
                context_regex: None,
                blocking_menus: None,
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
        let _app = crate::test_support::test_builder()
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

    async fn wait_for_web_lifecycle(
        handle: &WebServerHandle,
        expected: WebServerLifecycle,
    ) -> WebServerLifecycleSnapshot {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = handle.snapshot();
                if snapshot.lifecycle == expected {
                    return snapshot;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("web lifecycle did not converge")
    }

    async fn start_test_web_generation(
        handle: &WebServerHandle,
        bind: &str,
        port: u16,
    ) -> (u64, Arc<crate::web::WebSocketAdmission>) {
        let lifecycle = handle.clone();
        let bind = bind.to_string();
        let shutdown = crate::shutdown::ShutdownSignal::new();
        let (admission_sender, admission_receiver) = oneshot::channel();
        let waiter = handle.begin_start(
            shutdown,
            move |generation, admission, generation_token| async move {
                assert!(lifecycle.publish_effective_endpoint(generation, bind, port));
                assert!(admission_sender.send(Arc::clone(&admission)).is_ok());
                let server = tauri::async_runtime::spawn(async move {
                    generation_token.cancelled().await;
                });
                Ok::<_, String>(Some(server))
            },
        );
        let admission = tokio::time::timeout(Duration::from_secs(2), admission_receiver)
            .await
            .expect("factory did not expose admission")
            .expect("factory dropped admission sender");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), waiter.wait())
                .await
                .expect("start waiter timed out"),
            Ok(true)
        );
        let snapshot = wait_for_web_lifecycle(handle, WebServerLifecycle::Running).await;
        assert_eq!(snapshot.endpoint, Some(("127.0.0.1".to_string(), port)));
        (
            snapshot.generation.expect("running generation has id"),
            admission,
        )
    }

    #[tokio::test]
    async fn web_server_stop_during_start_is_sticky() {
        let handle = WebServerHandle::default();
        let shutdown = crate::shutdown::ShutdownSignal::new();
        let lifecycle = handle.clone();
        let (entered_sender, entered_receiver) = oneshot::channel();
        let (release_sender, release_receiver) = oneshot::channel();
        let (admission_sender, admission_receiver) = oneshot::channel();
        let first = handle.begin_start(
            shutdown.clone(),
            move |generation, admission, _generation_token| async move {
                assert!(entered_sender.send(()).is_ok());
                assert!(admission_sender.send(Arc::clone(&admission)).is_ok());
                release_receiver
                    .await
                    .expect("test releases blocked start factory");
                if !lifecycle.publish_effective_endpoint(generation, "127.0.0.1".to_string(), 8765)
                {
                    return Err(WEB_SERVER_START_CANCELLED.to_string());
                }
                panic!("a stopped generation must not reach bind");
            },
        );
        entered_receiver.await.expect("factory entered");
        let admission = admission_receiver.await.expect("admission exposed");

        let unexpected_factory_calls = Arc::new(AtomicU64::new(0));
        let calls = Arc::clone(&unexpected_factory_calls);
        let second = handle.begin_start(
            shutdown,
            move |_generation, _admission, _generation_token| async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok::<_, String>(None)
            },
        );
        let stop = handle.begin_stop().expect("starting generation is owned");
        let stopping = handle.snapshot();
        assert_eq!(stopping.lifecycle, WebServerLifecycle::Stopping);
        assert_eq!(stopping.endpoint, None);
        assert!(admission.try_acquire().is_none());
        assert!(release_sender.send(()).is_ok());

        let first_result = first.wait().await;
        let second_result = second.wait().await;
        assert_eq!(first_result, second_result);
        assert_eq!(first_result, Err(WEB_SERVER_START_CANCELLED.to_string()));
        assert_eq!(unexpected_factory_calls.load(Ordering::SeqCst), 0);
        assert_eq!(stop.wait().await, Ok(()));
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopped);
        assert!(admission.is_empty());
    }

    async fn assert_stop_after_start_output_cancels_result(
        factory_output: Result<Option<tauri::async_runtime::JoinHandle<()>>, String>,
    ) {
        let handle = WebServerHandle::default();
        let (output_reached, release_output) = handle.gate_next_start_output();
        let start = handle.begin_start(
            crate::shutdown::ShutdownSignal::new(),
            move |_generation, _admission, _generation_token| async move { factory_output },
        );

        assert!(
            output_reached
                .await
                .expect("supervisor reports the observed lifecycle"),
            "factory output must be complete while the generation is still Starting"
        );
        let stop = handle
            .begin_stop()
            .expect("Stop linealizes before start-result publication");
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
        release_output
            .send(())
            .expect("release start-result publication");

        assert_eq!(
            start.wait().await,
            Err(WEB_SERVER_START_CANCELLED.to_string())
        );
        assert_eq!(stop.wait().await, Ok(()));
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopped);
    }

    #[tokio::test]
    async fn web_server_stop_between_factory_output_and_start_result_publication_is_sticky() {
        assert_stop_after_start_output_cancels_result(Err("late factory error".to_string())).await;
        assert_stop_after_start_output_cancels_result(Ok(None)).await;
    }

    #[tokio::test]
    async fn web_server_stop_waits_for_generation_drain() {
        let handle = WebServerHandle::default();
        let (_generation, admission) = start_test_web_generation(&handle, "127.0.0.1", 8765).await;
        let connection_guard = admission
            .try_acquire()
            .expect("running generation admits connection");
        let frame_guard = admission
            .try_acquire()
            .expect("running generation admits frame");
        let waiter = handle.begin_stop().expect("running generation is owned");
        let mut stop_task = tokio::spawn(waiter.wait());

        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut stop_task)
                .await
                .is_err()
        );
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
        drop(frame_guard);
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut stop_task)
                .await
                .is_err()
        );
        drop(connection_guard);

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), stop_task)
                .await
                .expect("drain stop task timed out")
                .expect("drain stop task panicked"),
            Ok(())
        );
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopped);
    }

    #[tokio::test]
    async fn web_server_stop_timeout_is_shared_and_fail_closed() {
        let handle = WebServerHandle::default();
        let (_generation, admission) = start_test_web_generation(&handle, "127.0.0.1", 8766).await;
        let retained_guard = admission
            .try_acquire()
            .expect("running generation admits retained work");
        let first = handle
            .begin_stop_with_timeout(Duration::from_millis(40))
            .expect("running generation is owned");
        let second = handle
            .begin_stop_with_timeout(Duration::from_secs(30))
            .expect("stopping generation remains owned");
        assert_eq!(first.deadline(), second.deadline());

        let (first_result, second_result) = tokio::join!(first.wait(), second.wait());
        assert_eq!(first_result, second_result);
        assert_eq!(
            first_result,
            Err("Timed out waiting for web server generation to stop".to_string())
        );
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
        let rejected_start = handle.begin_start(
            crate::shutdown::ShutdownSignal::new(),
            |_generation, _admission, _generation_token| async { Ok::<_, String>(None) },
        );
        assert_eq!(
            rejected_start.wait().await,
            Err(WEB_SERVER_START_CANCELLED.to_string())
        );

        drop(retained_guard);
        wait_for_web_lifecycle(&handle, WebServerLifecycle::Stopped).await;
    }

    #[tokio::test]
    async fn web_server_lost_stop_channel_is_fail_closed() {
        let handle = WebServerHandle::default();
        let (_generation, admission) = start_test_web_generation(&handle, "127.0.0.1", 8770).await;
        let retained_guard = admission
            .try_acquire()
            .expect("running generation admits retained work");
        let generation_stop = handle.begin_stop().expect("running generation is owned");
        let (sender, receiver) = watch::channel(None);
        drop(sender);
        let lost_channel = WebServerStopWaiter {
            receiver,
            deadline: Instant::now() + Duration::from_secs(1),
        };

        assert_eq!(
            lost_channel.wait().await,
            Err("Web server stop supervisor channel closed before terminal state".to_string())
        );
        assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);

        drop(retained_guard);
        generation_stop
            .wait()
            .await
            .expect("real generation Stop drains after channel-loss seam");
    }

    #[tokio::test]
    async fn web_server_restart_uses_new_generation_and_ignores_stale_completion() {
        let handle = WebServerHandle::default();
        let (generation_one, _admission_one) =
            start_test_web_generation(&handle, "127.0.0.1", 8767).await;
        handle
            .begin_stop()
            .expect("first generation is owned")
            .wait()
            .await
            .expect("first generation drains");
        let (generation_two, _admission_two) =
            start_test_web_generation(&handle, "127.0.0.1", 8768).await;
        assert!(generation_two > generation_one);
        let current = handle.snapshot();

        assert!(!handle.move_generation_to_stopping(generation_one));
        assert!(!handle.finish_generation(generation_one));
        assert_eq!(handle.snapshot(), current);

        handle
            .begin_stop()
            .expect("second generation is owned")
            .wait()
            .await
            .expect("second generation drains");
    }

    #[tokio::test]
    async fn web_server_finished_outer_task_converges_after_start_terminal() {
        let handle = WebServerHandle::default();
        let lifecycle = handle.clone();
        let waiter = handle.begin_start(
            crate::shutdown::ShutdownSignal::new(),
            move |generation, _admission, _generation_token| async move {
                assert!(lifecycle.publish_effective_endpoint(
                    generation,
                    "127.0.0.1".to_string(),
                    8769,
                ));
                Ok::<_, String>(Some(tauri::async_runtime::spawn(async {})))
            },
        );
        assert_eq!(waiter.wait().await, Ok(true));
        wait_for_web_lifecycle(&handle, WebServerLifecycle::Stopped).await;
    }

    fn web_server_loopback_test_state() -> (
        tauri::App,
        Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>,
        Arc<Mutex<crate::pty::manager::PtyManager>>,
        SettingsState,
    ) {
        let session_mgr = Arc::new(tokio::sync::RwLock::new(
            crate::session::manager::SessionManager::new(),
        ));
        let settings: SettingsState = Arc::new(tokio::sync::RwLock::new(AppSettings::default()));
        let app = crate::test_support::test_builder()
            .manage(Arc::clone(&settings))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build loopback websocket test app");
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let git_watcher = crate::pty::git_watcher::GitWatcher::new(
            Arc::clone(&session_mgr),
            app.handle().clone(),
        );
        let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new(
            Arc::new(Mutex::new(HashMap::new())),
            idle_detector,
            git_watcher,
            None,
            None,
        )));
        (app, session_mgr, pty_mgr, settings)
    }

    #[tokio::test]
    async fn real_loopback_websocket_is_closed_and_reaped_by_generation_stop() {
        let (app, session_mgr, pty_mgr, settings) = web_server_loopback_test_state();
        let shutdown = crate::shutdown::ShutdownSignal::new();
        let generation_token = CancellationToken::new();
        let admission = Arc::new(crate::web::WebSocketAdmission::new(
            generation_token.clone(),
        ));
        assert!(admission.open(&shutdown));
        let router = crate::web::build_router(
            Arc::new(crate::web::auth::WebAccessToken::new(
                "loopback-test-token".to_string(),
            )),
            session_mgr,
            pty_mgr,
            settings,
            crate::web::broadcast::WsBroadcaster::new(),
            app.handle().clone(),
            Arc::clone(&admission),
            generation_token.clone(),
            None,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback websocket listener");
        let addr = listener.local_addr().expect("loopback listener address");
        let server = crate::web::spawn_server_on_listener(
            listener,
            router,
            generation_token.clone(),
            shutdown,
        );
        let mut client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect loopback websocket client");
        let request = format!(
            "GET /ws?token=loopback-test-token HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        client
            .write_all(request.as_bytes())
            .await
            .expect("write RFC 6455 handshake");
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut chunk = [0_u8; 512];
            while !response.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = client
                    .read(&mut chunk)
                    .await
                    .expect("read RFC 6455 handshake");
                assert!(read > 0, "server closed before handshake completed");
                response.extend_from_slice(&chunk[..read]);
            }
        })
        .await
        .expect("RFC 6455 handshake timed out");
        assert!(
            String::from_utf8_lossy(&response).starts_with("HTTP/1.1 101"),
            "unexpected handshake: {}",
            String::from_utf8_lossy(&response)
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while admission.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("connection guard was not tracked");

        admission.close();
        generation_token.cancel();
        let mut eof = [0_u8; 1];
        let (server_result, read_result, ()) =
            tokio::time::timeout(Duration::from_secs(2), async {
                tokio::join!(server, client.read(&mut eof), admission.wait())
            })
            .await
            .expect("generation stop did not reap loopback websocket");
        server_result.expect("loopback server task panicked");
        assert_eq!(read_result.expect("read client EOF"), 0);
        assert!(admission.is_empty());
    }

    #[test]
    fn web_server_handle_bind_failure_roundtrip() {
        let handle = WebServerHandle::default();
        assert!(handle.last_bind_failure().is_none());

        let failure = crate::web::StartServerError::BindFailed {
            bind: "192.168.1.12".to_string(),
            addr: "192.168.1.12:8888".parse().unwrap(),
            detail: "os error 10049".to_string(),
        };
        handle.record_bind_failure(failure.clone());
        assert_eq!(
            handle
                .last_bind_failure()
                .expect("failure must be recorded"),
            failure
        );

        handle.clear_bind_failure();
        assert!(handle.last_bind_failure().is_none());
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
            false,
            true,
            Some(&SessionStatus::Running),
            false
        ));
        assert!(restore_session_should_wake(
            false,
            true,
            false,
            true,
            Some(&SessionStatus::Running),
            false
        ));
    }

    #[test]
    fn should_wake_working_agent_on_restore_truth_table() {
        for resume_agents_on in [false, true] {
            for is_coord in [false, true] {
                for persisted_working in [false, true] {
                    let got = should_wake_working_agent_on_restore(
                        resume_agents_on,
                        is_coord,
                        persisted_working,
                    );
                    let expected = resume_agents_on && !is_coord && persisted_working;
                    assert_eq!(
                        got, expected,
                        "resume_agents_on={resume_agents_on} is_coord={is_coord} persisted_working={persisted_working}"
                    );
                }
            }
        }
    }

    #[test]
    fn restore_session_should_wake_wakes_a_working_replica_only_when_opted_in() {
        assert!(!restore_session_should_wake(
            false,
            true,
            false,
            false,
            Some(&SessionStatus::Running),
            true
        ));
        assert!(restore_session_should_wake(
            false,
            true,
            true,
            false,
            Some(&SessionStatus::Running),
            true
        ));
        // The two arms are independent: the replica arm fires with the
        // coordinator setting OFF.
        assert!(restore_session_should_wake(
            false,
            false,
            true,
            false,
            Some(&SessionStatus::Running),
            true
        ));
    }

    #[test]
    fn restore_session_should_wake_never_wakes_an_idle_replica() {
        for status in [
            Some(SessionStatus::Idle),
            Some(SessionStatus::Running),
            None,
        ] {
            assert!(
                !restore_session_should_wake(false, true, true, false, status.as_ref(), false),
                "an idle replica must never wake, status={status:?}"
            );
        }
    }

    #[test]
    fn restore_session_should_wake_arms_are_disjoint_on_is_coord() {
        // Coordinator: wakes through the #248 arm, and the replica arm is false
        // for it.
        assert!(restore_session_should_wake(
            false,
            true,
            true,
            true,
            Some(&SessionStatus::Running),
            true
        ));
        assert!(!should_wake_working_agent_on_restore(true, true, true));

        // Non-coordinator: wakes through the new arm, and the #248 arm is false
        // for it.
        assert!(restore_session_should_wake(
            false,
            true,
            true,
            false,
            Some(&SessionStatus::Running),
            true
        ));
        assert!(!should_wake_on_restore(
            true,
            false,
            Some(&SessionStatus::Running)
        ));

        // Archived beats both arms.
        assert!(!restore_session_should_wake(
            true,
            true,
            true,
            true,
            Some(&SessionStatus::Running),
            true
        ));
        assert!(!restore_session_should_wake(
            true,
            true,
            true,
            false,
            Some(&SessionStatus::Running),
            true
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

    // ══════════════════════════════════════════════════════════════════════
    // #2232 phase 7: Co-managed supervisor tests
    // ══════════════════════════════════════════════════════════════════════

    use super::{
        abstain_reason_label, co_managed_candidate_label, co_managed_state_reason,
        emit_session_idle_edge, resolve_co_managed_route, route_co_managed_wake, utf8_safe_excerpt,
        CoManagedSupervisorHandle, CoManagedTestHooks, CoManagedTrigger,
    };
    use crate::capture::record::{CaptureProvider, CapturedRecord, RecordOrigin};
    use crate::capture::registry::CaptureRegistry;
    use crate::capture::sink::CaptureSlot;
    use crate::config::co_managed::CoManagedState;
    use crate::session::manager::SessionManager;
    use crate::session::profile::CodingAgentKind;
    use crate::session::session::SessionStatus;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicUsize;
    use tauri::Listener;

    struct CoManagedFixture {
        _temp: tempfile::TempDir,
        project: PathBuf,
        room_root: PathBuf,
        coordinator_cwd: PathBuf,
        dev_cwd: PathBuf,
        projects_dir: PathBuf,
    }

    fn make_co_managed_fixture() -> CoManagedFixture {
        let temp = tempfile::TempDir::new().unwrap();
        let root = crate::path_utils::normalize_windows_verbatim_path_buf(
            &std::fs::canonicalize(temp.path()).expect("canonicalize fixture temp"),
        );
        make_co_managed_fixture_in(temp, root)
    }

    /// A room under an arbitrarily deep `root`, for the pointer-budget test.
    fn make_co_managed_fixture_in(temp: tempfile::TempDir, root: PathBuf) -> CoManagedFixture {
        let project = root.join("proj-a");
        let ac_root = project.join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let origin_tech_lead = ac_root.join("_agent_tech-lead");
        let origin_dev_rust = ac_root.join("_agent_dev-rust");
        let room_root = ac_root.join("room-1-dev-team");
        let coordinator_cwd = room_root.join("__agent_tech-lead");
        let dev_cwd = room_root.join("__agent_dev-rust");
        for dir in [
            &team_dir,
            &origin_tech_lead,
            &origin_dev_rust,
            &coordinator_cwd,
            &dev_cwd,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"coordinator":"../_agent_tech-lead"}"#,
        )
        .unwrap();
        std::fs::write(
            coordinator_cwd.join("config.json"),
            r#"{"identity":"../../_agent_tech-lead"}"#,
        )
        .unwrap();
        std::fs::write(
            dev_cwd.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();
        let projects_dir = root.join("claude-projects");
        std::fs::create_dir_all(&projects_dir).unwrap();
        CoManagedFixture {
            _temp: temp,
            project,
            room_root,
            coordinator_cwd,
            dev_cwd,
            projects_dir,
        }
    }

    fn write_co_managed_room_config(room_root: &Path, enabled: bool, catalog: Option<&str>) {
        let dir = crate::config::co_managed::co_managed_dir(room_root);
        std::fs::create_dir_all(&dir).unwrap();
        let catalog_value = match catalog {
            Some(name) => serde_json::Value::String(name.to_string()),
            None => serde_json::Value::Null,
        };
        let json = serde_json::json!({ "enabled": enabled, "catalogPath": catalog_value });
        std::fs::write(
            crate::config::co_managed::config_path(room_root),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();
    }

    fn write_co_managed_catalog(room_root: &Path) {
        let catalog = r#"{"categories":{
            "to-peer":{"destination":"orchestrator","peer":"proj-a:room-1-dev-team/dev-rust","question":"route to the peer?"},
            "to-user":{"destination":"user","question":"route to the user?"},
            "to-root":{"destination":"root","question":"route to the root?"},
            "to-reply":{"destination":"default_reply","reply":"Acknowledged by the Co-managed room.","question":"send the default reply?"}
        }}"#;
        std::fs::write(room_root.join("catalog.json"), catalog).unwrap();
    }

    fn co_managed_app(
        fixture: &CoManagedFixture,
        endpoint: String,
        enabled: bool,
    ) -> (
        tauri::App<tauri::test::MockRuntime>,
        Arc<tokio::sync::RwLock<SessionManager>>,
        Arc<CaptureRegistry>,
    ) {
        co_managed_app_with_project_paths(
            fixture,
            endpoint,
            enabled,
            vec![fixture.project.to_string_lossy().to_string()],
        )
    }

    /// F4: the settings slice is a parameter so a test can omit the project the
    /// room lives in; the supervisor must still derive it from the room root.
    fn co_managed_app_with_project_paths(
        fixture: &CoManagedFixture,
        endpoint: String,
        enabled: bool,
        project_paths: Vec<String>,
    ) -> (
        tauri::App<tauri::test::MockRuntime>,
        Arc<tokio::sync::RwLock<SessionManager>>,
        Arc<CaptureRegistry>,
    ) {
        write_co_managed_room_config(&fixture.room_root, enabled, Some("catalog.json"));
        write_co_managed_catalog(&fixture.room_root);
        let settings = AppSettings {
            project_paths,
            jev_api_key: "test-key".to_string(),
            jev_model: "jev-1.13.0".to_string(),
            jev_endpoint: endpoint,
            jev_timeout_secs: 5,
            jev_threshold: 0.70,
            jev_margin: 0.15,
            ..AppSettings::default()
        };
        let session_manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let registry = Arc::new(CaptureRegistry::new());
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let app = tauri::test::mock_builder()
            .manage(Arc::new(tokio::sync::RwLock::new(settings)))
            .manage(session_manager.clone())
            .manage(crate::network::OutboundNetwork::new().expect("shared outbound network"))
            .manage(registry.clone())
            .manage(crate::pty::input_activity::new_state())
            .manage(idle_detector)
            .manage(Arc::new(crate::session::purge_guard::PurgeGuard::default()))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build Co-managed test app");
        (app, session_manager, registry)
    }

    async fn add_claude_session(
        session_manager: &Arc<tokio::sync::RwLock<SessionManager>>,
        cwd: &Path,
        status: SessionStatus,
        projects_dir: &Path,
    ) -> uuid::Uuid {
        let guard = session_manager.read().await;
        let session = guard
            .create_session(
                "claude".to_string(),
                Vec::new(),
                cwd.to_string_lossy().to_string(),
                Some("claude".to_string()),
                Some("claude".to_string()),
                Vec::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        guard
            .set_agent_kind(session.id, Some(CodingAgentKind::Claude))
            .await;
        guard
            .set_resolved_claude_projects_dir(session.id, Some(projects_dir.to_path_buf()))
            .await;
        if matches!(status, SessionStatus::Idle) {
            guard.mark_idle(session.id).await;
        }
        session.id
    }

    fn co_managed_candidate(session_id: &str, text: &str) -> Arc<CapturedRecord> {
        let text_sha256: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes()).into();
        let path = PathBuf::from(format!("/tmp/#2270/{session_id}.jsonl"));
        Arc::new(CapturedRecord {
            session_id: session_id.to_string(),
            text: text.to_string(),
            file: path.clone(),
            epoch: 0,
            record_start: Some(0),
            reader_seq: 0,
            text_sha256,
            turn_id: None,
            provider: CaptureProvider::Claude,
            provider_final: false,
            turn_identified: false,
            origin: RecordOrigin::Live,
            observed_path: path,
            observed_len: text.len() as u64,
            observed_prefix: Vec::new(),
        })
    }

    fn install_candidate(
        registry: &Arc<CaptureRegistry>,
        session_id: uuid::Uuid,
        text: &str,
    ) -> (CaptureSlot, Arc<CapturedRecord>) {
        let (capture, _rx) = registry.open(&session_id.to_string());
        let record = co_managed_candidate(&session_id.to_string(), text);
        capture.slot.offer(record.clone(), 0);
        (capture.slot, record)
    }

    fn capture_events(
        app: &tauri::App<tauri::test::MockRuntime>,
    ) -> Arc<Mutex<Vec<(String, serde_json::Value)>>> {
        let events = Arc::new(Mutex::new(Vec::new()));
        for name in ["session_idle", "session_comanaged_state"] {
            let events = Arc::clone(&events);
            app.listen_any(name, move |event| {
                let payload = serde_json::from_str::<serde_json::Value>(event.payload())
                    .unwrap_or(serde_json::Value::Null);
                events.lock().unwrap().push((name.to_string(), payload));
            });
        }
        events
    }

    fn queue_files(room_root: &Path) -> Vec<PathBuf> {
        let dir = crate::config::co_managed::queue_dir(room_root);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    fn messaging_files(room_root: &Path) -> Vec<PathBuf> {
        let dir = room_root.join(crate::phone::messaging::MESSAGING_DIR_NAME);
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    async fn session_communication(
        session_manager: &Arc<tokio::sync::RwLock<SessionManager>>,
        session_id: uuid::Uuid,
    ) -> Option<crate::session::session::SessionCommunication> {
        let guard = session_manager.read().await;
        guard.get_session(session_id).await?.communication
    }

    /// Build the raw HTTP response body Jev expects: every requested category
    /// with its score. `decide` requires all ids present and no unknowns.
    fn jev_response_body(scores: &[(&str, f32)]) -> String {
        let results: Vec<serde_json::Value> = scores
            .iter()
            .map(|(id, noul)| serde_json::json!({ "id": id, "noul": noul }))
            .collect();
        serde_json::json!({ "results": results }).to_string()
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let Ok(n) = stream.read(&mut chunk).await else {
                return;
            };
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(header_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
                let content_length = header
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if buf.len() >= header_end + 4 + content_length {
                    return;
                }
            }
        }
    }

    async fn write_http_json(stream: &mut tokio::net::TcpStream, body: &str) {
        use tokio::io::AsyncWriteExt;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    }

    async fn spawn_jev_listener(scores: Vec<(&'static str, f32)>) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                read_http_request(&mut stream).await;
                let body = jev_response_body(&scores);
                write_http_json(&mut stream, &body).await;
            }
        });
        (format!("http://127.0.0.1:{port}/v1/systemone"), hits)
    }

    /// Test 1: idle edge with the room disabled makes zero Jev calls, writes
    /// zero files, queues nothing, and `session_idle` carries `comanaged:false`.
    #[tokio::test]
    async fn idle_edge_with_a_disabled_room_makes_no_call_and_emits_comanaged_false() {
        let fixture = make_co_managed_fixture();
        let (endpoint, hits) = spawn_jev_listener(vec![("to-peer", 0.9), ("to-user", 0.1)]).await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, false);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Running,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let events = capture_events(&app);
        let (handle, _rx) = CoManagedSupervisorHandle::new();

        let comanaged =
            emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;

        assert!(!comanaged);
        assert_eq!(hits.load(Ordering::SeqCst), 0, "zero Jev calls");
        assert!(messaging_files(&fixture.room_root).is_empty(), "zero files");
        assert!(
            queue_files(&fixture.room_root).is_empty(),
            "zero queue entries"
        );
        let captured = events.lock().unwrap().clone();
        let idle = captured
            .iter()
            .find(|(name, _)| name == "session_idle")
            .expect("session_idle");
        assert_eq!(idle.1["comanaged"], serde_json::Value::Bool(false));
        assert!(
            !captured
                .iter()
                .any(|(name, _)| name == "session_comanaged_state"),
            "a disabled room must not emit the state event"
        );
    }

    /// Test 2: the enabled room writes exactly one message file before the
    /// queue entry, and the pointer resolves to it.
    #[tokio::test]
    async fn enabled_room_writes_the_message_file_before_the_queue_entry() {
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-peer", 0.9),
            ("to-user", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Running,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let hooks = Arc::new(CoManagedTestHooks::default());
        let (handle, _rx) = CoManagedSupervisorHandle::with_test_hooks(Arc::clone(&hooks));

        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;

        let files = messaging_files(&fixture.room_root);
        assert_eq!(files.len(), 1, "exactly one message file: {files:?}");
        let queue = queue_files(&fixture.room_root);
        assert_eq!(queue.len(), 1, "exactly one queue entry: {queue:?}");
        assert_eq!(
            hooks.steps(),
            vec!["messaging_file", "queue_file"],
            "the file must be created before the queue entry"
        );
        let envelope: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&queue[0]).unwrap()).unwrap();
        let pointer = envelope["body"].as_str().expect("pointer body");
        let pointer_path = pointer
            .strip_prefix(crate::phone::messaging::FILE_NOTIFICATION_PREFIX)
            .expect("canonical pointer prefix");
        assert_eq!(
            std::fs::canonicalize(pointer_path).unwrap(),
            std::fs::canonicalize(&files[0]).unwrap(),
            "the pointer must resolve to the message file"
        );
        assert_eq!(
            envelope["to"],
            serde_json::json!("proj-a:room-1-dev-team/dev-rust")
        );
        assert_eq!(
            envelope["from"],
            serde_json::json!("proj-a:room-1-dev-team/tech-lead")
        );
        assert!(envelope["token"].is_null(), "the queue carries no token");
        assert!(envelope["action"].is_null());
        assert!(envelope["command"].is_null());
    }

    /// Test 3: trigger (b) enqueues and emits `active:true`; a disabled room
    /// does neither.
    #[tokio::test]
    async fn a_record_while_already_idle_emits_active_true_and_enqueues_only_when_enabled() {
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-peer", 0.9),
            ("to-user", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Idle,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let events = capture_events(&app);
        let (handle, _rx) = CoManagedSupervisorHandle::new();

        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::SlotChanged(session_id),
        )
        .await;

        assert_eq!(queue_files(&fixture.room_root).len(), 1, "(b) enqueues");
        let captured = events.lock().unwrap().clone();
        assert_eq!(captured[0].0, "session_comanaged_state");
        assert_eq!(captured[0].1["active"], serde_json::Value::Bool(true));
        assert!(captured[0].1["reason"].is_null());
        assert_eq!(
            captured.last().unwrap().1["active"],
            serde_json::Value::Bool(false)
        );

        // Disabled room: no queue entry and no active:true.
        let disabled = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![("to-peer", 0.9)]).await;
        let (app2, manager2, registry2) = co_managed_app(&disabled, endpoint, false);
        let session2 = add_claude_session(
            &manager2,
            &disabled.coordinator_cwd,
            SessionStatus::Idle,
            &disabled.projects_dir,
        )
        .await;
        install_candidate(&registry2, session2, "candidate text");
        let events2 = capture_events(&app2);
        let (handle2, _rx2) = CoManagedSupervisorHandle::new();
        super::handle_co_managed_trigger(
            app2.handle(),
            &handle2,
            CoManagedTrigger::SlotChanged(session2),
        )
        .await;
        assert!(queue_files(&disabled.room_root).is_empty());
        assert!(!events2
            .lock()
            .unwrap()
            .iter()
            .any(|(_, payload)| payload["active"] == serde_json::Value::Bool(true)));
    }

    /// Test 4: a non-orchestrator session in an enabled room never triggers.
    #[tokio::test]
    async fn a_non_orchestrator_session_never_triggers() {
        let fixture = make_co_managed_fixture();
        let (endpoint, hits) = spawn_jev_listener(vec![("to-peer", 0.9)]).await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.dev_cwd,
            SessionStatus::Idle,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let (handle, _rx) = CoManagedSupervisorHandle::new();

        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::SlotChanged(session_id),
        )
        .await;

        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(messaging_files(&fixture.room_root).is_empty());
        assert!(queue_files(&fixture.room_root).is_empty());
        assert!(!handle.armed.is_armed(&session_id.to_string()));
    }

    /// Test 5: `can_communicate` is applied to peer destinations, and an
    /// unreachable peer is rejected instead of routed.
    #[test]
    fn peer_destinations_apply_can_communicate_and_reject_unreachable_peers() {
        let catalog = crate::capture::catalog::Catalog::from_json_str(
            r#"{"categories":{"ok":{"destination":"orchestrator","peer":"proj-a:room-1-dev-team/dev-rust","question":"?"},"bad":{"destination":"orchestrator","peer":"proj-b:room-9-dev-team/nobody","question":"?"}}}"#,
        );
        let from = "proj-a:room-1-dev-team/tech-lead";
        let reachable = resolve_co_managed_route(
            crate::capture::jev::ClassifyOutcome::Classified {
                category: "ok".to_string(),
                destination: crate::capture::catalog::Destination::Orchestrator,
                score: 0.9,
                runner_up: 0.1,
            },
            &catalog,
            from,
            &[],
            "body",
        );
        match reachable {
            super::CoManagedRoute::Wake { to, .. } => {
                assert_eq!(to, "proj-a:room-1-dev-team/dev-rust")
            }
            _ => panic!("a same-room peer must be reachable"),
        }
        let unreachable = resolve_co_managed_route(
            crate::capture::jev::ClassifyOutcome::Classified {
                category: "bad".to_string(),
                destination: crate::capture::catalog::Destination::Orchestrator,
                score: 0.9,
                runner_up: 0.1,
            },
            &catalog,
            from,
            &[],
            "body",
        );
        match unreachable {
            super::CoManagedRoute::User { reason, .. } => {
                assert!(reason.contains("not reachable"), "{reason}")
            }
            _ => panic!("an unreachable peer must not be routed"),
        }
    }

    /// Test 6: Root succeeds only for a verified coordinator, and
    /// `can_communicate(x, Root)` is false, which is why the branch is needed.
    #[tokio::test]
    async fn root_destination_uses_the_verified_coordinator_validator_not_can_communicate() {
        let fixture = make_co_managed_fixture();
        let catalog = crate::capture::catalog::Catalog::from_json_str(
            r#"{"categories":{"root":{"destination":"root","question":"?"}}}"#,
        );
        let paths = vec![fixture.project.to_string_lossy().to_string()];
        let verified = "proj-a:room-1-dev-team/tech-lead";
        let outcome = || crate::capture::jev::ClassifyOutcome::Classified {
            category: "root".to_string(),
            destination: crate::capture::catalog::Destination::Root,
            score: 0.9,
            runner_up: 0.1,
        };
        match resolve_co_managed_route(outcome(), &catalog, verified, &paths, "body") {
            super::CoManagedRoute::Wake { to, .. } => {
                assert_eq!(to, crate::config::root_agent::ROOT_AGENT_SENDER)
            }
            _ => panic!("a verified coordinator may route to Root"),
        }
        match resolve_co_managed_route(
            outcome(),
            &catalog,
            "proj-a:room-1-dev-team/dev-rust",
            &paths,
            "body",
        ) {
            super::CoManagedRoute::User { reason, .. } => {
                assert!(
                    reason.contains("not a verified room orchestrator"),
                    "{reason}"
                )
            }
            _ => panic!("a non-verified sender may not route to Root"),
        }
        assert!(!crate::config::teams::can_communicate(
            verified,
            crate::config::root_agent::ROOT_AGENT_SENDER,
            &crate::config::teams::discover_teams(),
        ));
    }

    /// Test 9: a 50 KiB candidate reaches the message file byte-for-byte and
    /// the user surface carries the path and a capped excerpt.
    #[tokio::test]
    async fn a_fifty_kib_candidate_reaches_the_file_intact() {
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-user", 0.9),
            ("to-peer", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Running,
            &fixture.projects_dir,
        )
        .await;
        let big = "x".repeat(50 * 1024);
        install_candidate(&registry, session_id, &big);
        let (handle, _rx) = CoManagedSupervisorHandle::new();

        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;

        let files = messaging_files(&fixture.room_root);
        assert_eq!(files.len(), 1);
        let written = std::fs::read(&files[0]).unwrap();
        assert_eq!(written.len(), big.len(), "no truncation");
        assert_eq!(written, big.as_bytes(), "byte-for-byte intact");
        let communication = session_communication(&manager, session_id)
            .await
            .expect("user communication");
        assert_eq!(
            communication.kind,
            crate::session::session::SessionCommunicationKind::CoManaged
        );
        let message = communication.message.expect("message text");
        assert!(
            message.contains(&files[0].display().to_string()),
            "{message}"
        );
        assert!(message.contains("51200 bytes"), "{message}");
    }

    /// Test 10: a pointer that does not fit is rejected with a visible reason;
    /// nothing is truncated and no partial message is queued.
    #[tokio::test]
    async fn a_pointer_that_does_not_fit_is_rejected_and_nothing_is_queued() {
        // Deep, long path components push the absolute pointer over PTY_SAFE_MAX.
        //
        // The deep components are created FIRST and the root is canonicalized
        // only afterwards, so Windows keeps the `\?\` verbatim form. The
        // readiness gate repairs the replica `config.json` through the
        // pre-existing `local_config_io` publisher, whose Windows step calls
        // `ReplaceFileW`; a path beyond MAX_PATH is only reachable there when it
        // is verbatim (Windows CI otherwise fails with os error 3, and the gate
        // answers `NotAnOrchestrator` before the pointer check). The pointer
        // budget invariant asserted below is the same on every OS.
        let long = "d".repeat(200);
        let temp = tempfile::TempDir::new().unwrap();
        let mut deep = std::fs::canonicalize(temp.path()).unwrap();
        for _ in 0..5 {
            deep = deep.join(&long);
        }
        std::fs::create_dir_all(&deep).unwrap();
        let root = std::fs::canonicalize(&deep).unwrap();
        let fixture = make_co_managed_fixture_in(temp, root);
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-peer", 0.9),
            ("to-user", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Running,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let (handle, _rx) = CoManagedSupervisorHandle::new();

        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;

        assert!(queue_files(&fixture.room_root).is_empty(), "nothing queued");
        assert!(
            messaging_files(&fixture.room_root).is_empty(),
            "the over-long file is removed, never delivered"
        );
        let communication = session_communication(&manager, session_id)
            .await
            .expect("visible reason");
        let message = communication.message.expect("message text");
        assert!(message.contains("PTY-safe"), "{message}");
    }

    /// Test 11: the excerpt is capped at 500 bytes and never splits a
    /// multi-byte character.
    #[test]
    fn the_excerpt_is_capped_at_500_bytes_without_splitting_a_character() {
        // One 300-byte line of 'a', then a multi-byte character straddling the
        // 500-byte boundary, then more text.
        let mut text = "a".repeat(300);
        text.push_str(&"\u{e9}".repeat(200));
        text.push_str("tail");
        let excerpt = utf8_safe_excerpt(&text, super::CO_MANAGED_EXCERPT_BYTES);
        assert!(excerpt.len() <= 500, "excerpt is {} bytes", excerpt.len());
        assert!(text.starts_with(&excerpt));
        assert!(text.is_char_boundary(excerpt.len()));
        assert!(!excerpt.is_empty());

        let exact = "b".repeat(500);
        assert_eq!(utf8_safe_excerpt(&exact, 500), exact, "no cut at the cap");
    }

    /// Test 12: flipping the room flag off during the classifier call produces
    /// zero effects.
    #[tokio::test]
    async fn flipping_the_room_flag_during_the_classifier_call_produces_zero_effects() {
        let fixture = make_co_managed_fixture();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint = format!("http://127.0.0.1:{port}/v1/systemone");
        let request_received = Arc::new(tokio::sync::Notify::new());
        let notify = Arc::clone(&request_received);
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let body = jev_response_body(&[("to-peer", 0.9)]);
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                read_http_request(&mut stream).await;
                notify.notify_one();
                let _ = release_rx.await;
                write_http_json(&mut stream, &body).await;
            }
        });

        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Running,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let (handle, _rx) = CoManagedSupervisorHandle::new();
        let trigger_handle = app.handle().clone();
        let supervisor = handle.clone();
        let task = tokio::spawn(async move {
            super::handle_co_managed_trigger(
                &trigger_handle,
                &supervisor,
                CoManagedTrigger::IdleEdge(session_id),
            )
            .await;
        });

        request_received.notified().await;
        crate::config::co_managed::set_enabled(&fixture.room_root, false).unwrap();
        let _ = release_tx.send(());
        task.await.unwrap();

        assert!(messaging_files(&fixture.room_root).is_empty());
        assert!(queue_files(&fixture.room_root).is_empty());
        let communication = session_communication(&manager, session_id).await;
        assert!(communication.is_none(), "no user-facing effect either");
    }

    /// Test 13: budget exhaustion abstains, sends text to the user, and
    /// enqueues nothing.
    #[tokio::test]
    async fn budget_exhaustion_abstains_sends_text_and_enqueues_nothing() {
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-peer", 0.9),
            ("to-user", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        // Three automatic actions already spent since the last recharge.
        let state_path =
            crate::config::co_managed::co_managed_dir(&fixture.room_root).join("state.json");
        std::fs::write(
            &state_path,
            r#"{"spends_since_recharge":3,"recharge_stamp":1}"#,
        )
        .unwrap();
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Running,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let (handle, _rx) = CoManagedSupervisorHandle::new();

        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;

        assert!(
            queue_files(&fixture.room_root).is_empty(),
            "nothing enqueued"
        );
        let communication = session_communication(&manager, session_id)
            .await
            .expect("text to the user");
        let message = communication.message.expect("message text");
        assert!(message.to_lowercase().contains("budget"), "{message}");
    }

    /// Test 14: a `provider_final == false` candidate is rendered as the
    /// latest captured assistant text, and no rendering implies approval.
    #[test]
    fn a_non_final_candidate_is_rendered_as_latest_captured_assistant_text() {
        assert_eq!(
            co_managed_candidate_label(false),
            "latest captured assistant text at the idle edge"
        );
        assert_eq!(co_managed_candidate_label(true), "final message");
        let text = "A status update for the room.";
        let rendered = super::co_managed_user_message(
            "category 'x' routes to the user",
            Some((text, false, Some(Path::new("/tmp/msg.md")))),
        );
        assert!(rendered.contains("latest captured assistant text at the idle edge"));
        assert!(!rendered.to_lowercase().contains("final message"));
        for phrase in ["approved", "go ahead", "lgtm", "ship it"] {
            assert!(
                !rendered.to_lowercase().contains(phrase),
                "rendering must not express approval: {rendered}"
            );
        }
    }

    /// Test 15: the idle edge never emits a bare idle for an armed session.
    /// Drive the real emission site and capture emissions in order.
    #[tokio::test]
    async fn the_idle_edge_never_emits_a_bare_idle_for_an_armed_session() {
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-user", 0.9),
            ("to-peer", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Running,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let events = capture_events(&app);
        let (handle, _rx) = CoManagedSupervisorHandle::new();
        handle.armed.arm(&session_id.to_string());

        let comanaged =
            emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;

        assert!(comanaged);
        let captured = events.lock().unwrap().clone();
        assert_eq!(captured[0].0, "session_idle");
        assert_eq!(captured[0].1["comanaged"], serde_json::Value::Bool(true));
        assert!(
            !captured.iter().any(|(name, payload)| {
                name == "session_comanaged_state"
                    && payload["active"] == serde_json::Value::Bool(true)
            }),
            "the idle edge must not be preceded or duplicated by active:true: {captured:?}"
        );
    }

    /// Test 16: enabling the flag alone emits nothing; a cycle emits
    /// `active:true` then `active:false` with a non-null reason.
    #[tokio::test]
    async fn enabling_the_flag_alone_emits_nothing_and_a_cycle_emits_true_then_false() {
        // Disabled room: the enable alone (config write) emits nothing.
        let disabled = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![("to-user", 0.9)]).await;
        let (app, manager, registry) = co_managed_app(&disabled, endpoint, false);
        let session_id = add_claude_session(
            &manager,
            &disabled.coordinator_cwd,
            SessionStatus::Running,
            &disabled.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let events = capture_events(&app);
        let (handle, _rx) = CoManagedSupervisorHandle::new();
        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;
        assert!(!events
            .lock()
            .unwrap()
            .iter()
            .any(|(name, _)| name == "session_comanaged_state"));

        // Enabled room: one cycle emits exactly true then false(reason).
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-user", 0.9),
            ("to-peer", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        let (app2, manager2, registry2) = co_managed_app(&fixture, endpoint, true);
        let session2 = add_claude_session(
            &manager2,
            &fixture.coordinator_cwd,
            SessionStatus::Idle,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry2, session2, "candidate text");
        let events2 = capture_events(&app2);
        let (handle2, _rx2) = CoManagedSupervisorHandle::new();
        super::handle_co_managed_trigger(
            app2.handle(),
            &handle2,
            CoManagedTrigger::SlotChanged(session2),
        )
        .await;
        let captured = events2.lock().unwrap().clone();
        let states: Vec<&serde_json::Value> = captured
            .iter()
            .filter(|(name, _)| name == "session_comanaged_state")
            .map(|(_, payload)| payload)
            .collect();
        assert_eq!(states.len(), 2, "{captured:?}");
        assert_eq!(states[0]["active"], serde_json::Value::Bool(true));
        assert!(states[0]["reason"].is_null());
        assert_eq!(states[1]["active"], serde_json::Value::Bool(false));
        assert!(states[1]["reason"].is_string());
    }

    /// Test 20: `session_comanaged_state` is emitted with `emit`, so a general
    /// listener receives it.
    #[tokio::test]
    async fn the_state_event_is_emitted_with_emit_not_emit_to() {
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-user", 0.9),
            ("to-peer", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Idle,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let events = capture_events(&app);
        let (handle, _rx) = CoManagedSupervisorHandle::new();

        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::SlotChanged(session_id),
        )
        .await;

        let captured = events.lock().unwrap().clone();
        let state = captured
            .iter()
            .find(|(name, _)| name == "session_comanaged_state")
            .expect("a general listener must receive the event");
        assert_eq!(state.1["id"], serde_json::json!(session_id.to_string()));
        assert!(state.1["active"].is_boolean());
        assert!(state.1.as_object().unwrap().contains_key("reason"));
    }

    /// Test 21: contention terminates after exactly three attempts, abstains
    /// with reason "contention", keeps the flag armed and never re-triggers
    /// the same sequence.
    #[tokio::test]
    async fn contention_terminates_after_three_attempts_and_keeps_the_flag_armed() {
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-peer", 0.9),
            ("to-user", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Running,
            &fixture.projects_dir,
        )
        .await;
        let (slot, _record) = install_candidate(&registry, session_id, "candidate text");
        let seq_before = slot.seq();
        let events = capture_events(&app);
        let hooks = Arc::new(CoManagedTestHooks::default());
        let (handle, _rx) = CoManagedSupervisorHandle::with_test_hooks(Arc::clone(&hooks));

        // Hold the advisory lock for the whole cycle.
        let lock_path = crate::config::co_managed::lock_path(&fixture.room_root);
        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        lock_file.try_lock().expect("test holds the lock");

        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;

        assert_eq!(hooks.commit_calls(), 3, "three attempts, then abstain");
        assert!(messaging_files(&fixture.room_root).is_empty());
        assert!(queue_files(&fixture.room_root).is_empty());
        let captured = events.lock().unwrap().clone();
        let last = captured.last().expect("events");
        assert_eq!(last.0, "session_comanaged_state");
        assert_eq!(last.1["active"], serde_json::Value::Bool(false));
        assert_eq!(last.1["reason"], serde_json::json!("contention"));
        assert!(handle.armed.is_armed(&session_id.to_string()));
        assert_eq!(slot.seq(), seq_before, "the candidate is unchanged");
        assert!(slot.snapshot().value.record().is_some());

        // A trigger for the same sequence must not run a fourth attempt.
        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;
        assert_eq!(
            hooks.commit_calls(),
            3,
            "no fourth attempt for the same sequence"
        );

        // A later idle callback reports the candidate as armed.
        let comanaged =
            emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
        assert!(comanaged, "the pending candidate stays armed");
    }

    /// F1 (step 9): a contended candidate whose session loses readiness must
    /// clear the armed flag and publish the readiness reason, without ever
    /// re-running the candidate. Before the fix the early contended return kept
    /// `comanaged: true` on every later idle edge for the same `seq`.
    #[tokio::test]
    async fn losing_readiness_clears_the_armed_flag_for_a_contended_candidate() {
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-peer", 0.9),
            ("to-user", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Running,
            &fixture.projects_dir,
        )
        .await;
        let (slot, _record) = install_candidate(&registry, session_id, "candidate text");
        let seq_before = slot.seq();
        let events = capture_events(&app);
        let hooks = Arc::new(CoManagedTestHooks::default());
        let (handle, _rx) = CoManagedSupervisorHandle::with_test_hooks(Arc::clone(&hooks));

        // Hold the advisory lock so the first trigger abstains on contention.
        let lock_path = crate::config::co_managed::lock_path(&fixture.room_root);
        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        lock_file.try_lock().expect("test holds the lock");
        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;
        assert_eq!(hooks.commit_calls(), 3, "the first trigger contends");
        assert!(handle.armed.is_armed(&session_id.to_string()));

        // The room flag goes off while the same candidate is still pending.
        write_co_managed_room_config(&fixture.room_root, false, Some("catalog.json"));
        emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;

        assert_eq!(
            hooks.commit_calls(),
            3,
            "readiness loss must not re-run the contended candidate"
        );
        assert!(
            !handle.armed.is_armed(&session_id.to_string()),
            "readiness loss must clear the armed flag"
        );
        let captured = events.lock().unwrap().clone();
        let last = captured.last().expect("events");
        assert_eq!(last.0, "session_comanaged_state", "{captured:?}");
        assert_eq!(last.1["active"], serde_json::Value::Bool(false));
        assert_eq!(last.1["reason"], serde_json::json!("RoomFlagOff"));
        assert_eq!(slot.seq(), seq_before, "the candidate is unchanged");
        assert!(slot.snapshot().value.record().is_some());

        // The next idle edge reports waiting, not red.
        let comanaged =
            emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
        assert!(!comanaged, "the cleared flag must reach the idle payload");
    }

    /// F4 (step 9): the Root branch must use the room-derived project even when
    /// settings omit it, exactly as the CLI and the mailbox's Co-managed branch
    /// do. Without the derived path a verified coordinator degrades to a user
    /// message instead of the route the mailbox would accept.
    #[tokio::test]
    async fn root_destination_derives_the_project_from_the_room() {
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-root", 0.9),
            ("to-user", 0.1),
            ("to-peer", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        // Settings do NOT contain the project the room lives in.
        let (app, manager, registry) =
            co_managed_app_with_project_paths(&fixture, endpoint, true, Vec::new());
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Running,
            &fixture.projects_dir,
        )
        .await;
        install_candidate(&registry, session_id, "candidate text");
        let (handle, _rx) = CoManagedSupervisorHandle::new();

        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::IdleEdge(session_id),
        )
        .await;

        assert!(
            !queue_files(&fixture.room_root).is_empty(),
            "the verified coordinator must reach Root through the room-derived project"
        );
        assert_eq!(
            messaging_files(&fixture.room_root).len(),
            1,
            "the routed wake writes exactly one message file"
        );
    }

    /// Test 22: `commit_effect` runs on the blocking pool, so a current-thread
    /// runtime keeps making progress while the advisory lock is held.
    #[tokio::test(flavor = "current_thread")]
    async fn the_blocking_commit_does_not_park_a_current_thread_worker() {
        let fixture = make_co_managed_fixture();
        write_co_managed_room_config(&fixture.room_root, true, Some("catalog.json"));
        let registry = Arc::new(CaptureRegistry::new());
        let session_id = uuid::Uuid::new_v4();
        let (slot, record) = install_candidate(&registry, session_id, "candidate text");
        let expected_seq = slot.seq();
        let expected_key =
            crate::capture::state::consumption_key(&fixture.room_root, record.as_ref());
        // The holder signals the acquired lock before the commit starts, so the
        // commit cannot win the race and find it free, and it holds the lock
        // until the async worker proves progress instead of for a fixed sleep,
        // so a descheduled test thread cannot make the commit run uncontended.
        let lock_path = crate::config::co_managed::lock_path(&fixture.room_root);
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        let lock_path_for_thread = lock_path.clone();
        let (lock_ready_tx, lock_ready_rx) = std::sync::mpsc::channel();
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let lock_holder = std::thread::spawn(move || {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path_for_thread)
                .unwrap();
            file.try_lock().expect("lock holder");
            lock_ready_tx.send(()).expect("announce the held lock");
            // The bound is a liveness guard only: a runtime-parking mutant must
            // fail the progress assertion instead of hanging the suite. The
            // healthy path is released by the tick, never by the timeout.
            let _ = progress_rx.recv_timeout(Duration::from_secs(10));
            drop(file);
        });
        lock_ready_rx
            .recv()
            .expect("the lock holder must hold the lock before the commit starts");

        let ticks = Arc::new(AtomicUsize::new(0));
        let ticker = {
            let ticks = Arc::clone(&ticks);
            tokio::spawn(async move {
                // This first tick runs only once the commit has yielded the
                // runtime: it is both the property under test and the holder's
                // release signal, so a direct in-task call never reaches it.
                tokio::task::yield_now().await;
                ticks.fetch_add(1, Ordering::SeqCst);
                let _ = progress_tx.send(());
                for _ in 0..2 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    ticks.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        // Built as late as possible: the test must not spend the commit's
        // staleness window on its own synchronization.
        let pre = crate::capture::state::EffectPreconditions {
            session_alive: true,
            session_id: session_id.to_string(),
            anchor: fixture.coordinator_cwd.to_string_lossy().to_string(),
            provider: CaptureProvider::Claude,
            unique_live_session_for_cwd: true,
            no_pending_user_input: true,
            effective_ready: true,
            observed_at: Instant::now(),
            kind: crate::capture::state::EffectKind::Automatic,
        };
        let commit = super::commit_co_managed_effect(
            fixture.room_root.clone(),
            slot.clone(),
            expected_seq,
            expected_key,
            pre,
        )
        .await;
        let ticks_when_commit_returned = ticks.load(Ordering::SeqCst);
        ticker.await.unwrap();
        lock_holder.join().unwrap();

        // Checked before `commit`: an in-task mutant can return `LockBusy`
        // before the ticker ever runs, and the missing progress is the finding.
        assert!(
            ticks_when_commit_returned >= 1,
            "the async worker must make real progress WHILE the blocking commit waits for the lock; \
             a direct in-task call parks it and observes zero ticks here (F2)"
        );
        assert!(
            commit.is_ok(),
            "the lock is released and the commit applies"
        );
        assert_eq!(
            ticks.load(Ordering::SeqCst),
            3,
            "the async worker kept making progress while the blocking commit waited"
        );
    }

    /// Test 23: the late-poll residual is real. A record delivered after the
    /// idle edge paints waiting (`comanaged:false`) and then red (`active:true`).
    #[tokio::test]
    async fn the_late_poll_residual_paints_waiting_then_red() {
        let fixture = make_co_managed_fixture();
        let (endpoint, _hits) = spawn_jev_listener(vec![
            ("to-user", 0.9),
            ("to-peer", 0.1),
            ("to-root", 0.0),
            ("to-reply", 0.0),
        ])
        .await;
        let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
        let session_id = add_claude_session(
            &manager,
            &fixture.coordinator_cwd,
            SessionStatus::Idle,
            &fixture.projects_dir,
        )
        .await;
        let events = capture_events(&app);
        let (handle, _rx) = CoManagedSupervisorHandle::new();

        // The idle edge fires with no candidate: false.
        emit_session_idle_edge(app.handle(), &handle.armed, Some(&handle), session_id);
        // The record arrives afterwards, while the session is already idle.
        install_candidate(&registry, session_id, "candidate text");
        super::handle_co_managed_trigger(
            app.handle(),
            &handle,
            CoManagedTrigger::SlotChanged(session_id),
        )
        .await;

        let captured = events.lock().unwrap().clone();
        assert_eq!(captured[0].0, "session_idle", "{captured:?}");
        assert_eq!(captured[0].1["comanaged"], serde_json::Value::Bool(false));
        let active = captured
            .iter()
            .find(|(name, payload)| {
                name == "session_comanaged_state"
                    && payload["active"] == serde_json::Value::Bool(true)
            })
            .expect("the late record arms red after the waiting dot");
        assert_eq!(captured[0].0, "session_idle");
        assert_eq!(active.1["id"], serde_json::json!(session_id.to_string()));
    }

    /// The two supervisor helpers the contention/abstention reasons must stay
    /// stable for phase 8 and the log readers.
    #[test]
    fn supervisor_reason_strings_are_stable() {
        assert_eq!(
            abstain_reason_label(&crate::capture::state::AbstainReason::LockBusy),
            "LockBusy"
        );
        assert_eq!(co_managed_state_reason(&Ok(CoManagedState::Ready)), "Ready");
        assert_eq!(
            co_managed_state_reason(&Ok(CoManagedState::Off {
                reason: crate::config::co_managed::OffReason::RoomFlagOff
            })),
            "RoomFlagOff"
        );
    }

    /// `route_co_managed_wake` is the in-process writer: it must own the file
    /// authority check and leave a resolvable pointer.
    #[test]
    fn route_co_managed_wake_requires_an_authoritative_room() {
        let temp = tempfile::TempDir::new().unwrap();
        let not_a_room = temp.path().join("not-a-room");
        std::fs::create_dir_all(&not_a_room).unwrap();
        let (handle, _rx) = CoManagedSupervisorHandle::new();
        let error = route_co_managed_wake(
            &handle,
            &not_a_room,
            "proj-a:room-1-dev-team/tech-lead",
            "proj-a:room-1-dev-team/dev-rust",
            "body",
        )
        .expect_err("a non-authoritative root must be refused before any write");
        assert!(!error.is_empty());
        assert!(
            !not_a_room
                .join(crate::phone::messaging::MESSAGING_DIR_NAME)
                .exists(),
            "nothing may be created under a refused root"
        );
    }

    // ── #2348 - the extracted window-placement helpers ───────────────────────

    fn issue_2348_geometry(x: f64, y: f64, width: f64, height: f64) -> WindowGeometry {
        WindowGeometry {
            x,
            y,
            width,
            height,
        }
    }

    fn issue_2348_test_placement(maximized: bool) -> TestWindowPlacement {
        TestWindowPlacement {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
            maximized,
        }
    }

    #[test]
    fn issue_2348_visibility_requires_fifty_pixels_on_a_monitor() {
        let monitors = [(0.0, 0.0, 1920.0, 1080.0, 1.0)];
        assert!(is_visible_on_monitors(
            &issue_2348_geometry(0.0, 0.0, 800.0, 600.0),
            &monitors
        ));
        // 100px overlap with the monitor edge -> visible.
        assert!(is_visible_on_monitors(
            &issue_2348_geometry(-700.0, 0.0, 800.0, 600.0),
            &monitors
        ));
        // 40px overlap -> off-screen.
        assert!(!is_visible_on_monitors(
            &issue_2348_geometry(-760.0, 0.0, 800.0, 600.0),
            &monitors
        ));
        assert!(!is_visible_on_monitors(
            &issue_2348_geometry(5000.0, 0.0, 800.0, 600.0),
            &monitors
        ));
        // No monitors at all: cannot validate, so visible (the existing behavior).
        assert!(is_visible_on_monitors(
            &issue_2348_geometry(5000.0, 0.0, 800.0, 600.0),
            &[]
        ));
    }

    #[test]
    fn issue_2348_physical_to_logical_uses_the_monitor_under_the_center() {
        // Mixed scales: 1.5x primary, 2.0x secondary to its right.
        let monitors = [
            (0.0, 0.0, 1920.0, 1080.0, 1.5),
            (1920.0, 0.0, 3840.0, 1080.0, 2.0),
        ];
        // Center (2000, 400) lands on the 2.0x secondary.
        let logical =
            physical_to_logical(&issue_2348_geometry(1600.0, 100.0, 800.0, 600.0), &monitors);
        assert_eq!((logical.x, logical.y), (800.0, 50.0));
        assert_eq!((logical.width, logical.height), (400.0, 300.0));
        // Center on the primary uses its 1.5x scale.
        let logical =
            physical_to_logical(&issue_2348_geometry(100.0, 100.0, 800.0, 600.0), &monitors);
        assert_eq!((logical.width, logical.height), (800.0 / 1.5, 600.0 / 1.5));
        // No monitor contains the center: scale 1.0.
        let logical = physical_to_logical(
            &issue_2348_geometry(-5000.0, -5000.0, 800.0, 600.0),
            &monitors,
        );
        assert_eq!((logical.x, logical.y), (-5000.0, -5000.0));
    }

    #[test]
    fn issue_2348_centered_default_clamps_to_1400x900_and_follows_the_primary_offset() {
        // The no-primary fallback: 1920x1080 at the origin.
        let geo = centered_default_main_geometry(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!((geo.x, geo.y), (260.0, 90.0));
        assert_eq!((geo.width, geo.height), (1400.0, 900.0));
        // A screen narrower than the default keeps its full size at the origin.
        let geo = centered_default_main_geometry(100.0, 50.0, 1000.0, 700.0);
        assert_eq!((geo.x, geo.y), (100.0, 50.0));
        assert_eq!((geo.width, geo.height), (1000.0, 700.0));
        // An offset primary centers the clamped size within the monitor.
        let geo = centered_default_main_geometry(100.0, 50.0, 2000.0, 1200.0);
        assert_eq!((geo.x, geo.y), (400.0, 200.0));
        assert_eq!((geo.width, geo.height), (1400.0, 900.0));
    }

    #[test]
    fn issue_2348_effective_display_state_gives_test_placement_precedence() {
        let normal_test = issue_2348_test_placement(false);
        let maximized_test = issue_2348_test_placement(true);

        // Test branch absent: the saved state applies.
        assert_eq!(
            effective_main_display_state(MainWindowDisplayState::Normal, None),
            MainWindowDisplayState::Normal
        );
        assert_eq!(
            effective_main_display_state(MainWindowDisplayState::Maximized, None),
            MainWindowDisplayState::Maximized
        );
        // Test branch present, maximized=false: a saved maximize is NOT applied.
        assert_eq!(
            effective_main_display_state(MainWindowDisplayState::Maximized, Some(&normal_test)),
            MainWindowDisplayState::Normal
        );
        assert_eq!(
            effective_main_display_state(MainWindowDisplayState::Normal, Some(&normal_test)),
            MainWindowDisplayState::Normal
        );
        // Test branch present, maximized=true: maximize applies even when saved is normal.
        assert_eq!(
            effective_main_display_state(MainWindowDisplayState::Normal, Some(&maximized_test)),
            MainWindowDisplayState::Maximized
        );
        assert_eq!(
            effective_main_display_state(MainWindowDisplayState::Maximized, Some(&maximized_test)),
            MainWindowDisplayState::Maximized
        );
    }

    #[test]
    fn issue_2348_apply_main_display_state_maximizes_only_for_maximized() {
        let calls = std::cell::Cell::new(0);
        apply_main_display_state(MainWindowDisplayState::Normal, || {
            calls.set(calls.get() + 1);
            Ok::<(), String>(())
        });
        assert_eq!(calls.get(), 0, "Normal must not maximize");

        apply_main_display_state(MainWindowDisplayState::Maximized, || {
            calls.set(calls.get() + 1);
            Ok::<(), String>(())
        });
        assert_eq!(calls.get(), 1, "Maximized must maximize exactly once");
    }

    #[test]
    fn issue_2348_apply_main_display_state_survives_a_maximize_failure() {
        let attempted = std::cell::Cell::new(false);
        apply_main_display_state(MainWindowDisplayState::Maximized, || {
            attempted.set(true);
            Err::<(), String>("simulated maximize failure".to_string())
        });
        assert!(attempted.get(), "the maximize attempt must have run");
        // Reaching this line proves the injected failure was handled as a warning
        // and startup continued with a usable normal window.
    }
}

/// #2296 quit-gate unit tests.
///
/// Time is injected through `TestClock`, so every deadline assertion is exact
/// and the suite never sleeps on wall-clock time. Events and process exit are
/// injected through `TestHost`.
#[cfg(test)]
mod quit_gate_tests {
    use super::{
        apply_quit_gate_effects, quit_gate_run, quit_gate_window_gone, QuitAbortReason, QuitGate,
        QuitGateClock, QuitGateHost, QuitGateRegistration, QuitOutcome, QuitOutcomeKind,
        QUIT_GATE_TIMEOUT,
    };
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, Weak};
    use std::time::Duration;

    #[derive(Default)]
    struct TestHost {
        events: Mutex<Vec<(String, String, serde_json::Value)>>,
        exits: AtomicUsize,
        fail_event: Mutex<Option<String>>,
        gate: Mutex<Option<Weak<QuitGate>>>,
        /// Registration attempted from inside the `app_quit_started` emit, to
        /// prove the round was already installed when the event went out.
        probe_at_start: Mutex<Option<QuitGateRegistration>>,
    }

    impl QuitGateHost for TestHost {
        fn emit_to(
            &self,
            label: &str,
            event: &str,
            payload: serde_json::Value,
        ) -> Result<(), String> {
            if event == "app_quit_started" {
                let probe = self
                    .gate
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .map(|gate| gate.register("probe-at-start"));
                *self.probe_at_start.lock().unwrap() = probe;
            }
            if self.fail_event.lock().unwrap().as_deref() == Some(event) {
                return Err(format!("injected emit failure for {event}"));
            }
            self.events
                .lock()
                .unwrap()
                .push((label.to_string(), event.to_string(), payload));
            Ok(())
        }

        fn exit(&self, _code: i32) {
            self.exits.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl TestHost {
        fn exits(&self) -> usize {
            self.exits.load(Ordering::SeqCst)
        }

        fn events_named(&self, event: &str) -> Vec<(String, serde_json::Value)> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, name, _)| name == event)
                .map(|(label, _, payload)| (label.clone(), payload.clone()))
                .collect()
        }

        fn outcome_to_main(&self) -> Option<serde_json::Value> {
            self.events_named("app_quit_outcome")
                .into_iter()
                .last()
                .map(|(_, payload)| payload)
        }
    }

    /// Manually stepped clock. `watch` (not `Notify`) so an `advance` between a
    /// sleeper's tick read and its await cannot be lost.
    struct TestClock {
        ticks: tokio::sync::watch::Sender<Duration>,
    }

    impl Default for TestClock {
        fn default() -> Self {
            Self {
                ticks: tokio::sync::watch::channel(Duration::ZERO).0,
            }
        }
    }

    impl TestClock {
        fn advance(&self, by: Duration) {
            self.ticks.send_modify(|now| *now += by);
        }
    }

    impl QuitGateClock for TestClock {
        fn now(&self) -> Duration {
            *self.ticks.borrow()
        }

        fn sleep_until(&self, at: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            let mut rx = self.ticks.subscribe();
            Box::pin(async move {
                loop {
                    if *rx.borrow_and_update() >= at {
                        return;
                    }
                    if rx.changed().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
            })
        }
    }

    fn new_gate() -> (
        Arc<QuitGate>,
        Arc<TestHost>,
        Arc<dyn QuitGateHost>,
        Arc<TestClock>,
    ) {
        let clock = Arc::new(TestClock::default());
        let gate = Arc::new(QuitGate::with_clock(clock.clone()));
        let host = Arc::new(TestHost::default());
        *host.gate.lock().unwrap() = Some(Arc::downgrade(&gate));
        let dyn_host: Arc<dyn QuitGateHost> = host.clone();
        (gate, host, dyn_host, clock)
    }

    /// Lets the spawned quit task reach its next await point. The clock only
    /// moves when a test calls `TestClock::advance`, so yielding is safe.
    async fn settle() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    fn spawn_quit(
        gate: &Arc<QuitGate>,
        host: &Arc<dyn QuitGateHost>,
        attempt: &str,
    ) -> tokio::task::JoinHandle<Result<QuitOutcome, String>> {
        let gate = Arc::clone(gate);
        let host = Arc::clone(host);
        let attempt = attempt.to_string();
        tokio::spawn(
            async move { quit_gate_run(gate, host, "main", false, None, Some(attempt)).await },
        )
    }

    async fn force_quit(
        gate: &Arc<QuitGate>,
        host: &Arc<dyn QuitGateHost>,
        epoch: Option<u64>,
    ) -> QuitOutcome {
        quit_gate_run(
            Arc::clone(gate),
            Arc::clone(host),
            "main",
            true,
            epoch,
            None,
        )
        .await
        .expect("force quit from main is never an Err")
    }

    // --- consent, refusal, timeout -------------------------------------------

    #[tokio::test]
    async fn all_consent_exits_once() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        gate.register("watchers");

        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("watchers", 1, true));

        let outcome = quit.await.unwrap().unwrap();
        assert_eq!(outcome, QuitOutcome::exiting(1));
        assert_eq!(host.exits(), 1);
        // Terminal Exiting is not an abort: no app_quit_outcome, no cancels.
        assert!(host.events_named("app_quit_outcome").is_empty());
        assert!(host.events_named("app_quit_cancelled").is_empty());
    }

    #[tokio::test]
    async fn no_registered_gates_exits_immediately() {
        let (gate, host, dyn_host, _clock) = new_gate();
        let outcome = spawn_quit(&gate, &dyn_host, "attempt-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome, QuitOutcome::exiting(1));
        assert_eq!(host.exits(), 1);
    }

    #[tokio::test]
    async fn all_consent_writes_terminal_before_cleanup() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
        // Cleanup already ran (a new round may be allocated), yet the terminal
        // slot still reads Exiting — cleanup cannot replace it with Aborted.
        assert_eq!(
            gate.terminal_snapshot_for_test(1),
            Some(QuitOutcome::exiting(1))
        );
        assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
        assert_eq!(host.exits(), 1);
    }

    #[tokio::test]
    async fn refusal_aborts_immediately_despite_busy_peer() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        gate.register("watchers");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        gate.progress("watchers", 1, true); // peer is mid-save
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));

        let outcome = quit.await.unwrap().unwrap();
        assert_eq!(outcome.outcome, QuitOutcomeKind::Aborted);
        assert_eq!(outcome.epoch, 1);
        assert_eq!(outcome.reason, Some(QuitAbortReason::Refused));
        assert_eq!(
            outcome.refusing_labels.as_deref(),
            Some(["spec-board".to_string()].as_slice())
        );
        assert_eq!(host.exits(), 0);
        let emitted = host.outcome_to_main().unwrap();
        assert_eq!(emitted["outcome"], "Aborted");
        assert_eq!(emitted["epoch"], 1);
        assert_eq!(emitted["reason"], "refused");
        assert_eq!(emitted["refusingLabels"][0], "spec-board");
    }

    #[tokio::test]
    async fn cancellation_targets_every_gate_of_the_epoch() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        gate.register("watchers");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        apply_quit_gate_effects(host.as_ref(), &gate.resolve("watchers", 1, false));
        quit.await.unwrap().unwrap();

        let mut targets: Vec<String> = host
            .events_named("app_quit_cancelled")
            .into_iter()
            .map(|(label, payload)| {
                assert_eq!(payload["epoch"], 1);
                assert_eq!(payload["label"], label);
                label
            })
            .collect();
        targets.sort();
        assert_eq!(targets, vec!["spec-board", "watchers"]);
    }

    #[tokio::test]
    async fn no_timeout_at_29999ms_aborts_at_30s() {
        let (gate, host, dyn_host, clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        clock.advance(Duration::from_millis(29_999));
        settle().await;
        assert!(!quit.is_finished(), "must outlast main's 10s Force offer");
        assert_eq!(gate.terminal_snapshot_for_test(1), None);

        clock.advance(Duration::from_millis(1));
        let outcome = quit.await.unwrap().unwrap();
        assert_eq!(outcome.outcome, QuitOutcomeKind::Aborted);
        assert_eq!(outcome.reason, Some(QuitAbortReason::Timeout));
        assert_eq!(
            outcome.unanswered_labels.as_deref(),
            Some(["spec-board".to_string()].as_slice())
        );
        assert_eq!(host.exits(), 0);
        assert_eq!(QUIT_GATE_TIMEOUT, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn timeout_labels_are_sorted() {
        let (gate, _host, dyn_host, clock) = new_gate();
        gate.register("zeta");
        gate.register("alpha");
        gate.register("mid");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        clock.advance(QUIT_GATE_TIMEOUT);
        let outcome = quit.await.unwrap().unwrap();
        assert_eq!(
            outcome.unanswered_labels.as_deref(),
            Some(["alpha".to_string(), "mid".to_string(), "zeta".to_string()].as_slice())
        );
    }

    // --- pause / resume ------------------------------------------------------

    #[tokio::test]
    async fn busy_pause_preserves_remaining_budget() {
        let (gate, host, dyn_host, clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        clock.advance(Duration::from_secs(10));
        settle().await;
        gate.progress("spec-board", 1, true); // 20s left, clock stops

        clock.advance(Duration::from_secs(300)); // slow save
        settle().await;
        assert!(!quit.is_finished(), "a busy gate must not time out");

        gate.progress("spec-board", 1, false); // 20s budget restored
        clock.advance(Duration::from_millis(19_999));
        settle().await;
        assert!(!quit.is_finished());

        clock.advance(Duration::from_millis(1));
        let outcome = quit.await.unwrap().unwrap();
        assert_eq!(outcome.reason, Some(QuitAbortReason::Timeout));
        assert_eq!(host.exits(), 0);
    }

    #[tokio::test]
    async fn near_deadline_busy_wins_over_timeout() {
        let (gate, _host, dyn_host, clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        clock.advance(Duration::from_millis(29_999));
        settle().await;
        gate.progress("spec-board", 1, true);
        clock.advance(Duration::from_secs(600));
        settle().await;
        assert!(!quit.is_finished());

        gate.progress("spec-board", 1, false);
        clock.advance(Duration::from_millis(1));
        assert_eq!(
            quit.await.unwrap().unwrap().reason,
            Some(QuitAbortReason::Timeout)
        );
    }

    #[tokio::test]
    async fn busy_progress_cannot_affect_a_later_epoch() {
        let (gate, host, dyn_host, clock) = new_gate();
        gate.register("spec-board");
        let first = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));
        first.await.unwrap().unwrap();

        gate.register("spec-board");
        let second = spawn_quit(&gate, &dyn_host, "attempt-2");
        settle().await;
        gate.progress("spec-board", 1, true); // stale epoch: ignored

        clock.advance(QUIT_GATE_TIMEOUT);
        let outcome = second.await.unwrap().unwrap();
        assert_eq!(outcome.epoch, 2);
        assert_eq!(outcome.reason, Some(QuitAbortReason::Timeout));
    }

    // --- resolve validation --------------------------------------------------

    #[tokio::test]
    async fn stale_double_and_unknown_resolve_do_nothing() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        assert!(gate
            .resolve("spec-board", 99, true)
            .outcome_to_main
            .is_none());
        assert!(gate.resolve("ghost", 1, false).outcome_to_main.is_none());
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
        // Duplicate answer, now with a refusal, must not reopen the decision.
        assert!(gate
            .resolve("spec-board", 1, false)
            .outcome_to_main
            .is_none());

        assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
        assert_eq!(host.exits(), 1);
    }

    // --- registration lifecycle ----------------------------------------------

    #[tokio::test]
    async fn registration_is_rejected_during_a_round_and_retried_after() {
        let (gate, host, dyn_host, clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        assert_eq!(
            gate.register("watchers"),
            QuitGateRegistration::InFlight { epoch: 1 }
        );
        // The late registrant is excluded from the atomic snapshot, so it is
        // not an unanswered gate either.
        clock.advance(QUIT_GATE_TIMEOUT);
        let outcome = quit.await.unwrap().unwrap();
        assert_eq!(
            outcome.unanswered_labels.as_deref(),
            Some(["spec-board".to_string()].as_slice())
        );

        // Retry after the epoch terminates now succeeds.
        assert_eq!(gate.register("watchers"), QuitGateRegistration::Registered);
        let second = spawn_quit(&gate, &dyn_host, "attempt-2");
        settle().await;
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("watchers", 2, true));
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 2, true));
        assert_eq!(second.await.unwrap().unwrap(), QuitOutcome::exiting(2));
    }

    #[tokio::test]
    async fn unanswered_unregister_aborts_but_consented_unregister_does_not() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        gate.register("watchers");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        // Consented gate leaves: its consent is final, the round survives.
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("watchers", 1, true));
        apply_quit_gate_effects(host.as_ref(), &gate.unregister("watchers"));
        settle().await;
        assert!(!quit.is_finished());

        // Unanswered gate leaves: abort with `unregistered`.
        apply_quit_gate_effects(host.as_ref(), &gate.unregister("spec-board"));
        let outcome = quit.await.unwrap().unwrap();
        assert_eq!(outcome.reason, Some(QuitAbortReason::Unregistered));
        assert_eq!(
            outcome.unanswered_labels.as_deref(),
            Some(["spec-board".to_string()].as_slice())
        );
        assert_eq!(host.exits(), 0);
    }

    #[tokio::test]
    async fn unanswered_destruction_aborts_and_consented_destruction_does_not() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        gate.register("watchers");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        apply_quit_gate_effects(host.as_ref(), &gate.resolve("watchers", 1, true));
        quit_gate_window_gone(&gate, host.as_ref(), "watchers");
        settle().await;
        assert!(!quit.is_finished());

        // A dirty board destroyed without answering is NOT consent.
        quit_gate_window_gone(&gate, host.as_ref(), "spec-board");
        let outcome = quit.await.unwrap().unwrap();
        assert_eq!(outcome.reason, Some(QuitAbortReason::Destroyed));
        assert_eq!(
            outcome.unanswered_labels.as_deref(),
            Some(["spec-board".to_string()].as_slice())
        );
        assert_eq!(host.exits(), 0);
    }

    #[tokio::test]
    async fn main_destruction_retains_the_launched_round() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        quit_gate_window_gone(&gate, host.as_ref(), "main");
        settle().await;
        assert!(!quit.is_finished(), "main is not a gate; the round stands");

        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
        assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
        assert_eq!(host.exits(), 1);
    }

    #[tokio::test]
    async fn destroyed_gate_is_dropped_from_the_next_round() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        quit_gate_window_gone(&gate, host.as_ref(), "spec-board");

        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
        assert_eq!(host.exits(), 1);
    }

    // --- start event ---------------------------------------------------------

    #[tokio::test]
    async fn start_event_follows_installation_and_precedes_a_slow_gate() {
        let (gate, host, dyn_host, clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-xyz");
        settle().await;

        // Emitted while the round was already installed: the probe registration
        // made from inside the emit was rejected as InFlight.
        assert_eq!(
            *host.probe_at_start.lock().unwrap(),
            Some(QuitGateRegistration::InFlight { epoch: 1 })
        );
        let started = host.events_named("app_quit_started");
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].0, "main");
        assert_eq!(started[0].1["epoch"], 1);
        assert_eq!(started[0].1["attemptId"], "attempt-xyz");

        // ...and before the slow gate answers.
        let requested = host.events_named("app_quit_requested");
        assert_eq!(requested.len(), 1);
        assert_eq!(requested[0].0, "spec-board");
        gate.progress("spec-board", 1, true);
        clock.advance(Duration::from_secs(120));
        settle().await;
        assert!(!quit.is_finished());

        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
        assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
    }

    #[tokio::test]
    async fn failed_start_emit_aborts_the_installed_round() {
        let (gate, host, dyn_host, _clock) = new_gate();
        *host.fail_event.lock().unwrap() = Some("app_quit_started".to_string());
        gate.register("spec-board");

        let outcome = spawn_quit(&gate, &dyn_host, "attempt-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.outcome, QuitOutcomeKind::Aborted);
        assert_eq!(outcome.epoch, 1);
        assert_eq!(outcome.reason, Some(QuitAbortReason::Cancelled));
        assert_eq!(host.exits(), 0);
        // The round is released, so a later attempt can run.
        assert_eq!(gate.register("watchers"), QuitGateRegistration::Registered);
    }

    // --- concurrency and epochs ----------------------------------------------

    #[tokio::test]
    async fn concurrent_non_force_returns_in_flight_matching_the_terminal_outcome() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let first = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        let second = quit_gate_run(
            Arc::clone(&gate),
            Arc::clone(&dyn_host),
            "main",
            false,
            None,
            Some("attempt-2".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(second, QuitOutcome::in_flight(1));
        // A rejected invoke emits no second start event.
        assert_eq!(host.events_named("app_quit_started").len(), 1);

        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));
        let terminal = first.await.unwrap().unwrap();
        assert_eq!(terminal.epoch, second.epoch);
        assert_eq!(host.outcome_to_main().unwrap()["epoch"], 1);
    }

    #[tokio::test]
    async fn successive_rounds_get_distinct_increasing_epochs() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");

        let first = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));
        assert_eq!(first.await.unwrap().unwrap().epoch, 1);

        gate.register("spec-board");
        let second = spawn_quit(&gate, &dyn_host, "attempt-2");
        settle().await;

        // Late prior-round traffic must not touch the live round.
        gate.progress("spec-board", 1, true);
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
        apply_quit_gate_effects(host.as_ref(), &gate.unregister("gone-with-round-1"));
        settle().await;
        assert!(!second.is_finished());

        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 2, true));
        assert_eq!(second.await.unwrap().unwrap(), QuitOutcome::exiting(2));
        assert_eq!(host.exits(), 1);
    }

    // --- force ---------------------------------------------------------------

    #[tokio::test]
    async fn force_supersedes_the_original_waiter_with_one_exit() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        let forced = force_quit(&gate, &dyn_host, Some(1)).await;
        assert_eq!(forced, QuitOutcome::exiting(1));

        let original = quit.await.unwrap().unwrap();
        assert_eq!(original, QuitOutcome::exiting(1));
        assert_eq!(host.exits(), 1, "exit is called exactly once");
    }

    #[tokio::test]
    async fn force_wins_while_a_gate_is_busy() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;
        gate.progress("spec-board", 1, true); // paused mid-save

        assert_eq!(force_quit(&gate, &dyn_host, Some(1)).await.epoch, 1);
        assert_eq!(quit.await.unwrap().unwrap(), QuitOutcome::exiting(1));
        assert_eq!(host.exits(), 1);
    }

    #[tokio::test]
    async fn force_after_abort_is_stale_and_never_exits() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));
        quit.await.unwrap().unwrap();

        assert_eq!(
            force_quit(&gate, &dyn_host, Some(1)).await,
            QuitOutcome::stale(1)
        );
        assert_eq!(host.exits(), 0);
    }

    #[tokio::test]
    async fn stale_force_cannot_clear_a_later_live_round() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let first = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, false));
        first.await.unwrap().unwrap();

        gate.register("spec-board");
        let second = spawn_quit(&gate, &dyn_host, "attempt-2");
        settle().await;

        assert_eq!(
            force_quit(&gate, &dyn_host, Some(1)).await,
            QuitOutcome::stale(1)
        );
        assert_eq!(host.exits(), 0);
        settle().await;
        assert!(!second.is_finished(), "round 2 survives a stale Force");

        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 2, true));
        assert_eq!(second.await.unwrap().unwrap(), QuitOutcome::exiting(2));
        assert_eq!(host.exits(), 1);
    }

    #[tokio::test]
    async fn force_without_an_epoch_cannot_authorize_exit() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        assert_eq!(
            force_quit(&gate, &dyn_host, None).await,
            QuitOutcome::stale(0)
        );
        assert_eq!(host.exits(), 0);
        settle().await;
        assert!(!quit.is_finished());

        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 1, true));
        quit.await.unwrap().unwrap();
        assert_eq!(host.exits(), 1);
    }

    #[tokio::test]
    async fn force_bumps_the_allocator_so_the_next_round_is_distinct() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;
        force_quit(&gate, &dyn_host, Some(1)).await;
        quit.await.unwrap().unwrap();

        apply_quit_gate_effects(host.as_ref(), &gate.unregister("spec-board"));
        let next = spawn_quit(&gate, &dyn_host, "attempt-2")
            .await
            .unwrap()
            .unwrap();
        assert!(
            next.epoch > 1,
            "epoch {} must be past the forced one",
            next.epoch
        );
        assert_eq!(host.exits(), 2, "the new round exits on its own account");
    }

    // --- cancellation / drop -------------------------------------------------

    #[tokio::test]
    async fn dropped_quit_task_releases_the_round_with_a_terminal_outcome() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
        settle().await;

        quit.abort();
        settle().await;

        // The dropped task cannot return, but the terminal result is explicit
        // and observable by main.
        assert_eq!(
            gate.terminal_snapshot_for_test(1).map(|o| o.reason),
            Some(Some(QuitAbortReason::Cancelled))
        );
        let emitted = host.outcome_to_main().unwrap();
        assert_eq!(emitted["outcome"], "Aborted");
        assert_eq!(emitted["reason"], "cancelled");
        assert_eq!(emitted["epoch"], 1);
        assert_eq!(host.exits(), 0);

        // In-flight was released: a fresh round runs with a new epoch.
        gate.register("spec-board");
        let second = spawn_quit(&gate, &dyn_host, "attempt-2");
        settle().await;
        apply_quit_gate_effects(host.as_ref(), &gate.resolve("spec-board", 2, true));
        assert_eq!(second.await.unwrap().unwrap(), QuitOutcome::exiting(2));
    }

    // --- authority -----------------------------------------------------------

    #[tokio::test]
    async fn only_main_may_invoke_quit() {
        let (gate, host, dyn_host, _clock) = new_gate();
        gate.register("spec-board");
        for force in [false, true] {
            let err = quit_gate_run(
                Arc::clone(&gate),
                Arc::clone(&dyn_host),
                "spec-board",
                force,
                Some(1),
                Some("attempt-1".to_string()),
            )
            .await
            .unwrap_err();
            assert!(err.contains("restricted to the 'main' window"), "{err}");
        }
        assert_eq!(host.exits(), 0);
        assert!(host.events_named("app_quit_started").is_empty());
    }

    #[tokio::test]
    async fn attempt_id_is_required_for_a_new_round() {
        let (gate, host, dyn_host, _clock) = new_gate();
        for attempt in [None, Some(String::new())] {
            let err = quit_gate_run(
                Arc::clone(&gate),
                Arc::clone(&dyn_host),
                "main",
                false,
                None,
                attempt,
            )
            .await
            .unwrap_err();
            assert!(err.contains("nonempty attemptId"), "{err}");
        }
        assert_eq!(host.exits(), 0);
    }

    // --- B1 regression: lost resume wakeup ------------------------------------

    /// Documents the primitive the waiter loop depends on.
    ///
    /// `Notified` snapshots the notify-waiters counter when it is CREATED, so
    /// a `notify_waiters()` landing before that snapshot is lost forever while
    /// one landing after it is observed even before the first poll. That is
    /// exactly why `quit_gate_run` creates and enables its `Notified` ahead of
    /// reading the round state instead of after it.
    #[tokio::test]
    async fn notified_snapshot_must_be_taken_before_the_state_read() {
        let notify = Arc::new(tokio::sync::Notify::new());

        // Snapshot taken too late: the notification is already history.
        notify.notify_waiters();
        let late = notify.notified();
        tokio::pin!(late);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut late)
                .await
                .is_err(),
            "a Notified created after notify_waiters must miss it"
        );

        // Snapshot taken first: observed, even though it had not been polled.
        let early = notify.notified();
        tokio::pin!(early);
        early.as_mut().enable();
        notify.notify_waiters();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut early)
                .await
                .is_ok(),
            "a Notified created before notify_waiters must observe it"
        );
    }

    /// B1 REGRESSION BARRIER.
    ///
    /// The lost-wakeup window is a handful of instructions inside one
    /// synchronous stretch of `quit_gate_run`: between the round-state read and
    /// the `notified()` counter snapshot. It is not reachable from a black-box
    /// test — I swept a resume across that window from a second thread for
    /// thousands of rounds against the buggy ordering and never hit it — so the
    /// ordering is pinned structurally instead, the way the single-exit site is.
    ///
    /// If someone moves the enrolment back below the state read, this fails.
    #[test]
    fn waiter_enrols_before_reading_round_state() {
        let source = include_str!("lib.rs");
        let production = source
            .split("mod quit_gate_tests {")
            .next()
            .expect("test module marker present");

        let loops: Vec<&str> = production.split("let outcome = loop {").skip(1).collect();
        assert_eq!(loops.len(), 1, "expected exactly one quit waiter loop");
        let head = loops[0]
            .split("tokio::select! {")
            .next()
            .expect("waiter loop selects");

        let enrol = head
            .find("gate.notify.notified()")
            .expect("waiter loop enrols on the gate notify");
        let terminal = head
            .find("terminal_for(new_epoch)")
            .expect("waiter loop reads the terminal slot");
        let deadline = head
            .find("deadline_for(new_epoch)")
            .expect("waiter loop reads the deadline");

        assert!(
            enrol < terminal && enrol < deadline,
            "the Notified snapshot must be taken BEFORE the round-state read, \
             otherwise a notify_waiters() in between is lost and the waiter \
             parks forever with a stale deadline"
        );
        assert!(
            head.contains(".enable()"),
            "the Notified must be enabled up front, not only on first poll"
        );
    }

    /// Companion smoke test: pause/resume driven from real threads must always
    /// leave a round that can still time out. This does NOT reach the B1
    /// window (see `waiter_enrols_before_reading_round_state`); it guards the
    /// ordinary multi-threaded pause/resume path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn busy_resume_from_another_thread_still_times_out() {
        for attempt in 0..200 {
            let (gate, _host, dyn_host, clock) = new_gate();
            gate.register("spec-board");
            let quit = spawn_quit(&gate, &dyn_host, "attempt-1");

            // Wait for the atomic install rather than assuming a yield count.
            while gate.deadline_for(1).is_none() {
                tokio::task::yield_now().await;
            }

            gate.progress("spec-board", 1, true);
            tokio::time::sleep(Duration::from_micros(200)).await;

            let hammer = {
                let gate = Arc::clone(&gate);
                let delay = Duration::from_nanos((attempt % 250) * 100);
                tokio::task::spawn_blocking(move || {
                    gate.progress("spec-board", 1, true);
                    let spin_until = std::time::Instant::now() + delay;
                    while std::time::Instant::now() < spin_until {
                        std::hint::spin_loop();
                    }
                    gate.progress("spec-board", 1, false);
                })
            };
            hammer.await.unwrap();

            // The gate is idle and never answers: only the timeout can end it.
            clock.advance(QUIT_GATE_TIMEOUT * 2);
            let outcome = tokio::time::timeout(Duration::from_secs(5), quit)
                .await
                .unwrap_or_else(|_| panic!("attempt {attempt}: waiter never timed out"))
                .unwrap()
                .unwrap();
            assert_eq!(outcome.reason, Some(QuitAbortReason::Timeout));
            assert_eq!(outcome.epoch, 1);
        }
    }

    // --- N2 regression: terminal precedes sender drop -------------------------

    /// The waiter must be able to read a terminal result the moment its
    /// receiver closes, for BOTH teardown paths.
    #[tokio::test]
    async fn teardown_writes_the_terminal_before_dropping_the_sender() {
        for (reason, teardown) in [
            (QuitAbortReason::Unregistered, false),
            (QuitAbortReason::Destroyed, true),
        ] {
            let (gate, host, dyn_host, _clock) = new_gate();
            gate.register("spec-board");
            let quit = spawn_quit(&gate, &dyn_host, "attempt-1");
            settle().await;

            let effects = if teardown {
                gate.window_gone("spec-board")
            } else {
                gate.unregister("spec-board")
            };
            // Observed straight after the call returns, before the waiter has
            // run at all: the decision is already durable.
            assert_eq!(
                gate.terminal_snapshot_for_test(1).map(|o| o.reason),
                Some(Some(reason))
            );
            apply_quit_gate_effects(host.as_ref(), &effects);

            let outcome = quit.await.unwrap().unwrap();
            assert_eq!(outcome.reason, Some(reason));
            assert_eq!(host.exits(), 0);
        }
    }

    #[test]
    fn outcome_serialization_omits_absent_fields() {
        let json = serde_json::to_value(QuitOutcome::in_flight(7)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "outcome": "InFlight", "epoch": 7 })
        );
        let json = serde_json::to_value(QuitOutcome::aborted(
            2,
            QuitAbortReason::Timeout,
            vec!["b".into(), "a".into()],
            vec!["c".into()],
        ))
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "outcome": "Aborted",
                "epoch": 2,
                "reason": "timeout",
                "refusingLabels": ["b", "a"],
                "unansweredLabels": ["c"],
            })
        );
        assert_eq!(
            serde_json::to_value(QuitGateRegistration::Registered).unwrap(),
            serde_json::json!({ "status": "Registered" })
        );
    }

    /// The gate owns the only production `AppHandle::exit` call. Guarding it
    /// here keeps a second exit path from being added silently.
    #[test]
    fn exactly_one_production_exit_site() {
        let source = include_str!("lib.rs");
        let production = source
            .split("mod quit_gate_tests {")
            .next()
            .expect("test module marker present");

        // Collect the RECEIVER of every `.exit(` call in production code, so a
        // second exit smuggled in as `app_handle.exit(0)` or `handle.exit(0)`
        // fails here instead of slipping past a literal match.
        let receivers: Vec<String> = production
            .match_indices(".exit(")
            .map(|(idx, _)| {
                production[..idx]
                    .chars()
                    .rev()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect::<Vec<char>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .collect();

        // `app` is the one real `AppHandle::exit`; `host` is the injected seam
        // that routes every Exiting decision to it.
        let unexpected: Vec<&String> = receivers
            .iter()
            .filter(|recv| recv.as_str() != "app" && recv.as_str() != "host")
            .collect();
        assert!(
            unexpected.is_empty(),
            "unexpected exit receivers: {unexpected:?}"
        );
        assert_eq!(
            receivers
                .iter()
                .filter(|recv| recv.as_str() == "app")
                .count(),
            1,
            "quit gate must keep exactly one AppHandle::exit site"
        );
    }
}
