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
use pty::agent_quota::source::SourceSpec;
use pty::agent_quota::{
    AgentQuotaEngine, AgentQuotaPayload, AgentQuotaSource, QuotaEventSink, QuotaRowsSource,
    QuotaSourceProvider,
};
use pty::context_scrape::{
    ContextEventSink, ContextPatternSource, ContextPersistSink, ContextSample, ContextSampleSink,
    ContextScraper, ContextSessionLiveness, ContextUsagePayload, ScreenRowsRead, ScreenRowsSource,
};
use pty::git_watcher::GitWatcher;
use pty::idle_detector::IdleDetector;
use pty::manager::PtyManager;
use pty::watchers::{
    ActivityConsumer, ActivityFactory, ActivityObservation, SessionFrameReader,
    WatcherBackendSource, WatcherEngine, WatcherEventSink, WatcherPatternSource,
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
/// Persisted to `master-token.txt` in config_dir for CLI use. Regenerated on each app startup. See
/// #34.
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
    /// #2714 - a fatal outcome of the startup naming migration: an expired lock
    /// budget with no winner, an I/O error, or an unreadable journal.
    NamingMigration {
        scope: String,
        reason: String,
        path: PathBuf,
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
                "AgentsCommander cannot start because it could not create app outbox directory \
                    \"{}\" for configuration directory \"{}\": {}. Set \
                    AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.",
                app_outbox_path.display(),
                config_dir.display(),
                source
            ),
            StartupErrorKind::NamingMigration {
                scope,
                reason,
                path,
            } => write!(
                formatter,
                "AgentsCommander cannot start because the file-naming migration \"{}\" could \
                    not finish at \"{}\": {}. If another AgentsCommander process is running, \
                    retry once it has finished; otherwise make the directory writable, or move \
                    an unreadable naming-migration journal aside, and restart.",
                scope,
                path.display(),
                reason
            ),
        }
    }
}

impl std::error::Error for StartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            StartupErrorKind::Config(error) => Some(error),
            StartupErrorKind::AppOutboxCreate { source, .. } => Some(source),
            StartupErrorKind::NamingMigration { .. } => None,
        }
    }
}

pub fn preflight_config_startup() -> Result<(), StartupError> {
    if let Some(error) = config::config_startup_error() {
        return Err(StartupError {
            kind: StartupErrorKind::Config(error),
        });
    }
    preflight_config_startup_for(config::config_dir().as_deref())
}

/// #2714 - the scope key of the one naming migration that runs at startup.
const INSTANCE_FAMILIES_SCOPE: &str = "instance-families";

/// The summary `run()` logs once `init_logger` has run: the migration runs
/// before the logger, so `log::*` from inside it goes nowhere.
static NAMING_MIGRATION_SUMMARY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn record_naming_migration_summary(summary: Option<String>) {
    if let Ok(mut slot) = NAMING_MIGRATION_SUMMARY.lock() {
        *slot = summary;
    }
}

fn take_naming_migration_summary() -> Option<String> {
    NAMING_MIGRATION_SUMMARY
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
}

/// Called by both entries right after `init_logger`.
pub fn log_naming_migration_summary() {
    if let Some(summary) = take_naming_migration_summary() {
        log::info!("{summary}");
    }
}

/// Decides the `None` case: with no configuration directory every path this
/// scope renames is absent, so the migration is skipped and startup continues.
pub(crate) fn preflight_config_startup_for(config_dir: Option<&Path>) -> Result<(), StartupError> {
    match config_dir {
        Some(config_dir) => preflight_config_startup_in(config_dir),
        None => {
            record_naming_migration_summary(Some(format!(
                "[naming-migration] {INSTANCE_FAMILIES_SCOPE}: skipped, no configuration directory"
            )));
            Ok(())
        }
    }
}

/// All of the startup migration work, against an explicit directory.
pub(crate) fn preflight_config_startup_in(config_dir: &Path) -> Result<(), StartupError> {
    let summary = migrate_instance_families(config_dir)?;
    record_naming_migration_summary(summary);
    Ok(())
}

fn naming_migration_error(reason: impl Into<String>, path: PathBuf) -> StartupError {
    StartupError {
        kind: StartupErrorKind::NamingMigration {
            scope: INSTANCE_FAMILIES_SCOPE.to_string(),
            reason: reason.into(),
            path,
        },
    }
}

fn refusal_reason(refusal: crate::config::naming_migration::Refusal) -> String {
    match refusal {
        crate::config::naming_migration::Refusal::LockUnavailable => "lock unavailable".to_string(),
        crate::config::naming_migration::Refusal::Io(message) => message,
    }
}

/// #2714 - the pre-migration names of the two single files, in rename order.
const INSTANCE_FAMILIES_RENAMES: [crate::config::naming_migration::Rename; 2] = [
    crate::config::naming_migration::Rename {
        from: "settings.json",
        to: config::instance_artifacts::SETTINGS_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "settings.local.json",
        to: config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME,
    },
];

/// #2714 - the four blocking-menus files, renamed after the settings families.
const BLOCKING_MENUS_RENAMES: [crate::config::naming_migration::Rename; 4] = [
    crate::config::naming_migration::Rename {
        from: "settings-blocking-menus.json",
        to: config::instance_artifacts::BLOCKING_MENUS_SHIPPED_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "settings-blocking-menus.local.json",
        to: config::instance_artifacts::BLOCKING_MENUS_LOCAL_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "settings-blocking-menus.remote.json",
        to: config::instance_artifacts::BLOCKING_MENUS_REMOTE_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "blocking-menus-remote-check.json",
        to: config::instance_artifacts::BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME,
    },
];

/// The ordered rename list of B1b section 3.5, under both settings locks: the
/// settings file, its personal overlay, the two derived families, then the four
/// blocking-menus files. Returns the post-logger summary, `None` when the scope
/// was already settled.
fn migrate_instance_families(dir: &Path) -> Result<Option<String>, StartupError> {
    use crate::config::naming_migration as nm;
    use config::instance_artifacts as names;

    let scope = INSTANCE_FAMILIES_SCOPE;
    let journal_path = dir.join(names::NAMING_MIGRATION_STATE_NAME);
    let read = || {
        nm::read_journal(Some(dir)).map_err(|refusal| {
            naming_migration_error(refusal_reason(refusal), journal_path.clone())
        })
    };

    // A directory that does not exist yet holds nothing to rename, and it is
    // not created here. Anything else that is not a readable directory is fatal.
    match dir.metadata() {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(naming_migration_error(
                "the configuration directory path is not a directory",
                dir.to_path_buf(),
            ))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Some(format!(
                "[naming-migration] {scope}: skipped, configuration directory {} does not exist",
                dir.display()
            )))
        }
        Err(e) => {
            return Err(naming_migration_error(
                format!("failed to inspect the configuration directory: {e}"),
                dir.to_path_buf(),
            ))
        }
    }
    // `scope_is_settled`, never `is_complete`: a `Complete` the disk
    // contradicts is re-run.
    if nm::scope_is_settled(read()?.as_ref(), scope) {
        return Ok(None);
    }
    let held = match nm::lock_scope(
        dir,
        names::SETTINGS_LOCK_FILE_NAME,
        Some(names::SETTINGS_RETIRED_LOCK_NAME),
        nm::MIGRATION_LOCK_BUDGET,
    ) {
        Ok(held) => held,
        Err(nm::Refusal::LockUnavailable) => {
            // Another process may have completed the scope while we waited.
            if nm::scope_is_settled(read()?.as_ref(), scope) {
                return Ok(None);
            }
            return Err(naming_migration_error(
                "the settings lock was still held after the five-second budget",
                dir.join(names::SETTINGS_LOCK_FILE_NAME),
            ));
        }
        Err(refusal) => {
            return Err(naming_migration_error(
                refusal_reason(refusal),
                dir.join(names::SETTINGS_LOCK_FILE_NAME),
            ))
        }
    };
    // Re-check under the lock: a winner of the race leaves nothing to do.
    if nm::scope_is_settled(read()?.as_ref(), scope) {
        return Ok(None);
    }

    let journal_dir = Some(dir);
    let fail = |from: &str, outcome: nm::Outcome| match outcome {
        nm::Outcome::Refused(refusal) => Err(naming_migration_error(
            refusal_reason(refusal),
            dir.join(from),
        )),
        _ => Ok(()),
    };
    for rename in &INSTANCE_FAMILIES_RENAMES {
        fail(
            rename.from,
            nm::rename_step(dir, rename, scope, journal_dir, &held),
        )?;
    }
    let pre_glob = names::SETTINGS_MIGRATION_BACKUP_GLOB;
    let (pre_prefix, pre_suffix) = pre_glob.split_once('*').unwrap_or((pre_glob, ""));
    for (old_prefix, new_prefix, suffix) in [
        (
            "settings.backup.",
            names::SETTINGS_BACKUP_PREFIX,
            names::SETTINGS_BACKUP_SUFFIX,
        ),
        ("settings.pre-", pre_prefix, pre_suffix),
    ] {
        for (from, outcome) in nm::rename_prefix_family(
            dir,
            old_prefix,
            new_prefix,
            suffix,
            scope,
            journal_dir,
            &held,
        ) {
            fail(&from, outcome)?;
        }
    }
    for rename in &BLOCKING_MENUS_RENAMES {
        fail(
            rename.from,
            nm::rename_step(dir, rename, scope, journal_dir, &held),
        )?;
    }

    // A lock sidecar or a stale write temporary is never renamed or deleted,
    // only noted.
    let mut notes = Vec::new();
    if dir.join(names::SETTINGS_RETIRED_LOCK_NAME).exists() {
        notes.push(format!(
            "left on disk: {} (a lock sidecar is never renamed or deleted)",
            names::SETTINGS_RETIRED_LOCK_NAME
        ));
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut stale: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with("settings.json.") && name.ends_with(".tmp"))
            .collect();
        stale.sort();
        notes.extend(stale.into_iter().map(|name| {
            format!("left on disk: {name} (a stale write temporary is never renamed or deleted)")
        }));
    }
    let record = nm::update_journal(journal_dir, |journal| {
        for note in &notes {
            journal.note(scope, note);
        }
        journal.set_status(scope, nm::ScopeStatus::Complete);
        journal.scope(scope).cloned()
    })
    .map_err(|refusal| naming_migration_error(refusal_reason(refusal), journal_path.clone()))?
    .flatten();
    drop(held);

    let steps = record.map(|record| record.steps).unwrap_or_default();
    let renamed = steps
        .iter()
        .filter(|step| step.state == nm::StepState::Renamed)
        .count();
    let set_aside: Vec<&str> = steps
        .iter()
        .filter_map(|step| step.set_aside.as_deref())
        .collect();
    let mut summary = format!("[naming-migration] {scope}: complete, {renamed} renamed");
    if !set_aside.is_empty() {
        summary.push_str(&format!(", set aside: {}", set_aside.join(", ")));
    }
    Ok(Some(summary))
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
struct ScraperSink {
    app_handle: tauri::AppHandle,
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
struct QuotaRows {
    pty_mgr: Arc<Mutex<PtyManager>>,
    /// A poisoned `PtyManager` is app-wide and permanent: one line, not one per tick.
    poison_logged: AtomicBool,
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
struct QuotaSources {
    settings: SettingsState,
}

impl QuotaSources {
    /// #2566 D3 - membership is by COMMAND, and this is the only place it is decided. Every
    /// agent whose command yields an account key with at least one enabled, valid source
    /// behind it gets an entry: its OWN pattern when its own entry is enabled, otherwise the
    /// key winner's, the first enabled entry in `quota_sources` order (a `BTreeMap`, so the
    /// LOWEST agent id). `enabled: false` withdraws a pattern, never an agent.
    fn sources_map(settings: &config::settings::AppSettings) -> HashMap<String, AgentQuotaSource> {
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
struct QuotaSink {
    app_handle: tauri::AppHandle,
}

impl QuotaEventSink for QuotaSink {
    fn emit(&self, payload: AgentQuotaPayload) {
        let _ = self.app_handle.emit("agent_quota", payload);
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
fn load_startup_sessions(
    settings: &config::settings::AppSettings,
    load: impl FnOnce(&[String]) -> Vec<sessions_persistence::PersistedSession>,
) -> Result<Vec<sessions_persistence::PersistedSession>, String> {
    let paths = sessions_persistence::session_retention_project_paths(settings)?;
    Ok(load(&paths))
}

#[allow(clippy::too_many_arguments)]
fn start_native_restore_startup(
    app: tauri::AppHandle,
    restore_barrier: crate::session::selection::RestoreBarrierGuard,
    restore_observer_barrier: Arc<RestoreObserverStartBarrier>,
    restore_transaction: crate::session::selection::SelectionTransaction<tauri::Wry>,
    restore_flag: Arc<RestoreInProgress>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    settings_state: SettingsState,
    settings_snapshot: config::settings::AppSettings,
    persisted: Result<Vec<sessions_persistence::PersistedSession>, String>,
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
    let lifecycle_owner = Arc::clone(&settings_state);
    let lifecycle_observer = Arc::clone(&restore_observer_barrier);
    let app_handle = app.clone();
    let pty_mgr_clone = pty_mgr.clone();
    let session_mgr_clone = session_mgr;
    let settings_state_clone = settings_state;
    let restore_transaction_for_task = restore_transaction;
    let shutdown_for_body = shutdown.clone();
    let restart_resume_targets: Arc<Mutex<Vec<commands::session::RestartResumeTarget>>> =
        Arc::new(Mutex::new(Vec::new()));
    let restart_resume_targets_for_body = Arc::clone(&restart_resume_targets);
    let body = Box::new(move || -> futures::future::BoxFuture<'static, ()> {
        Box::pin(async move {
            let mut persisted = match persisted {
                Ok(persisted) => persisted,
                Err(error) => {
                    log::error!(
                        "[restore] Project state unavailable; automatic restore skipped: {}",
                        error
                    );
                    return;
                }
            };
            {
                let manager = session_mgr_clone.read().await;
                sessions_persistence::initialize_prompt_retention(&manager).await;
                sessions_persistence::block_duplicate_restore_prompt_domains(&manager, &persisted)
                    .await;
            }
            for ps in &mut persisted {
                if let Err(error) = ps.capture_binding_seed(&settings_snapshot) {
                    log::error!(
                        "[restore] binding seed unavailable for '{}': {}",
                        ps.name,
                        error
                    );
                }
            }
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
                                let cfg = match settings_state_clone.current_read().await {
                                    Ok(cfg) => cfg,
                                    Err(error) => {
                                        log::debug!("[restore] settings unavailable: {error}");
                                        return;
                                    }
                                };
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
                                    "[root-agent] Skipping startup auto-create: no resolvable \
                                        coding agent is configured"
                                );
                            }
                        }
                        Some(ps) if ps.binding_metadata().is_err() => {
                            log::error!("[restore] ambiguous Root binding; preserving recipe");
                            failed_recoverable.push(ps.clone());
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
                                                "[root-agent] Failed to force-destroy stale \
                                                    dormant root during restore: {}",
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
                                        commands::session::seed_root_last_prompt(
                                            &app_handle,
                                            &session_mgr_clone,
                                            uuid,
                                            ps,
                                        )
                                        .await;
                                    }
                                    should_create = false;
                                }
                            }
                            if should_create {
                                let mut rebuild_failed = false;
                                let resolved_spawn = if ps.agent_id.is_some()
                                    && settings_snapshot.agents_layer.unreadable_path().is_some()
                                {
                                    // #2716 (B3, 4.5b option S): skip, and push nothing to
                                    // failed_recoverable, so the row keeps its wake inputs;
                                    // save_sessions_to_dir suppresses the rewrite.
                                    log::warn!(
                                        "[root-agent] Skipping restore of '{}': the agents \
                                            layer is unreadable",
                                        ps.name
                                    );
                                    rebuild_failed = true;
                                    None
                                } else if let Some(aid) = ps.agent_id.as_deref() {
                                    match commands::session::build_configured_agent_spawn_for_cwd(
                                        &settings_snapshot,
                                        aid,
                                        &root_path,
                                        ps.requested_profile.as_deref(),
                                    ) {
                                        Ok(spawn) => spawn,
                                        Err(e) => {
                                            log::error!(
                                                "[root-agent] Failed to rebuild configured \
                                                    agent command for restore '{}': {}",
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
                                        // #973 - headless caller: no terminal to measure, keep
                                        // 120x30.
                                        None,
                                        Some(ps.start_fresh_on_restore),
                                        None,
                                        ps.binding_metadata()
                                            .expect("restore binding validated before spawn"),
                                        Some(ps),
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
                                                        "[restore] detach_terminal_inner failed \
                                                            for root agent '{}': {} — session \
                                                            stays live (attached)",
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
                                    drop(mgr);
                                    commands::session::seed_root_last_prompt(
                                        &app_handle,
                                        &session_mgr_clone,
                                        uuid,
                                        ps,
                                    )
                                    .await;
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
                                            "[root-agent] Failed to create dormant root session \
                                                '{}': {}",
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

                if let Err(error) = ps.binding_metadata() {
                    log::error!("[restore] invalid binding for '{}': {}", ps.name, error);
                    failed_recoverable.push(ps.clone());
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
                                "Deferred session '{}' on startup (agent: {}, is_coord: {}, \
                                    setting: {}, persisted_status: {:?}, was_detached: {})",
                                ps.name,
                                agent_name,
                                is_coord,
                                setting_on,
                                ps.status,
                                ps.was_detached
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
                let resolved_spawn = if ps.agent_id.is_some()
                    && settings_snapshot.agents_layer.unreadable_path().is_some()
                {
                    // #2716 (B3, 4.5b option S): skip without pushing to failed_recoverable.
                    log::warn!(
                        "Skipping restore of '{}': the agents layer is unreadable",
                        ps.name
                    );
                    continue;
                } else if let Some(aid) = ps.agent_id.as_deref() {
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
                    ps.binding_metadata()
                        .expect("restore binding validated before spawn"),
                    Some(ps),
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
                            "[restore] woke '{}' (is_coord={}, live_is_coord={}, \
                                start_fresh_on_restore={})",
                            ps.name,
                            is_coord,
                            live_is_coord,
                            ps.start_fresh_on_restore
                        );
                        if let Ok(uuid) = uuid::Uuid::parse_str(&info.id) {
                            commands::session::attach_persisted_telegram_if_configured(
                                &app_handle,
                                uuid,
                                ps.telegram_bot_id.as_deref(),
                            )
                            .await;
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
                                        "[restore] detach_terminal_inner failed for '{}': {} — \
                                            session stays live (attached)",
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
        })
    });
    let continuation = Box::new(move || -> futures::future::BoxFuture<'static, ()> {
        Box::pin(async move {
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
        })
    });
    spawn_restore_startup(
        restore_barrier,
        lifecycle_observer,
        restore_flag,
        lifecycle_owner,
        body,
        continuation,
    )
}

/// Shared admission and completion boundary for native startup restore.
/// Callbacks are invoked inside the unwind boundary, including synchronous panics.
pub(crate) fn spawn_restore_startup(
    restore_barrier: crate::session::selection::RestoreBarrierGuard,
    restore_observer_barrier: Arc<RestoreObserverStartBarrier>,
    restore_flag: Arc<RestoreInProgress>,
    settings_state: SettingsState,
    body: Box<dyn FnOnce() -> futures::future::BoxFuture<'static, ()> + Send + 'static>,
    continuation: Box<dyn FnOnce() -> futures::future::BoxFuture<'static, ()> + Send + 'static>,
) -> tauri::async_runtime::JoinHandle<()> {
    use futures::FutureExt;
    tauri::async_runtime::spawn(async move {
        struct RestoreGuard(Arc<RestoreInProgress>);
        impl Drop for RestoreGuard {
            fn drop(&mut self) {
                self.0 .0.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let restore_guard = RestoreGuard(restore_flag);
        let mut completion = RestoreCompletionGuard::new(restore_barrier, restore_observer_barrier);
        let caught_body = std::panic::AssertUnwindSafe(async move {
            let read_guard = match settings_state.current_read().await {
                Ok(guard) => guard,
                Err(error) => {
                    log::debug!(
                        "[restore] settings unavailable; automatic restore skipped: {error}"
                    );
                    return;
                }
            };
            drop(read_guard);
            (body)().await;
        });
        if let Err(panic) = caught_body.catch_unwind().await {
            log::error!(
                "[restore] restore task panicked (partial restore; retried next boot): {:?}",
                panic
            );
        }
        drop(restore_guard);
        completion.complete();
        let tail = std::panic::AssertUnwindSafe(async move { (continuation)().await });
        if let Err(panic) = tail.catch_unwind().await {
            log::error!(
                "[restore] post-restore startup panicked (services after the panic point not \
                    started): {:?}",
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
    /// Destroys every detached terminal window. Returns the labels destroyed.
    /// Called immediately before `exit`, never on a non-Exiting outcome.
    fn close_detached_terminals(&self) -> Vec<String>;
    fn exit(&self, code: i32);
}

/// #2563 - closes detached terminal windows at the moment quit is decided,
/// before the blocking `RunEvent::Exit` work, then exits.
fn exit_closing_detached(host: &dyn QuitGateHost) {
    let closed = host.close_detached_terminals();
    if !closed.is_empty() {
        log::info!(
            "[shutdown] destroyed {} detached terminal window(s)",
            closed.len()
        );
    }
    host.exit(0);
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

    fn close_detached_terminals(&self) -> Vec<String> {
        // Never touches `was_detached`: restore reads the session flag, not
        // the live window set, so detached sessions restore detached.
        let mut closed = Vec::new();
        for (label, window) in self.app.webview_windows() {
            if !label.starts_with("terminal-") {
                continue;
            }
            if let Err(e) = window.destroy() {
                log::warn!("[shutdown] detached window destroy failed: {label}: {e}");
            }
            closed.push(label);
        }
        closed
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
            "quit_application is restricted to the '{QUIT_GATE_MAIN_LABEL}' window (caller: \
                {caller_label})"
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
            exit_closing_detached(host.as_ref());
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
        exit_closing_detached(host.as_ref());
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

fn response_close_wiring_matches(
    state: &crate::pty::manager::ResponseCloseState,
    manager: &SessionManager,
    pty: &PtyManager,
    detector: &IdleDetector,
    menu: &crate::pty::menu_guard::MenuGuard,
) -> bool {
    state.publishers_bound()
        && state.same_state(&manager.response_close_state())
        && state.same_state(&pty.response_close_state())
        && detector.raw_output_observer_identity() == Some(state.wiring_identity())
        && menu
            .response_close_state()
            .is_some_and(|bound| state.same_state(bound))
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
    log_naming_migration_summary();

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

    let response_close = crate::pty::manager::ResponseCloseState::default();
    let response_close_for_setup = response_close.clone();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(
        SessionManager::with_response_close(response_close.clone()),
    ));
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
    let response_close_for_idle = response_close.clone();
    let response_close_for_busy = response_close.clone();
    let idle_detector = IdleDetector::new(
        move |id| {
            response_close_for_idle.observe_detector(id, false, std::time::Instant::now());
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
            response_close_for_busy.observe_detector(id, true, std::time::Instant::now());
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
    response_close.bind_publisher(crate::pty::manager::ResponseClosePublisher::Detector);
    let response_close_for_output = response_close.clone();
    if !idle_detector.bind_raw_output_observer(response_close.wiring_identity(), move |id, now| {
        response_close_for_output.observe_raw_output(id, now)
    }) {
        response_close.fail_closed();
    }
    response_close.bind_publisher(crate::pty::manager::ResponseClosePublisher::RawOutput);
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
    response_close.apply_settings(
        loaded_settings.response_close_enabled,
        loaded_settings.response_close_idle_seconds,
        std::time::Instant::now(),
    );
    // (#621) Snapshot registered project paths for the startup orphan-clock prune
    // below (taken before the value is moved into the RwLock).
    let startup_project_paths = loaded_settings.project_paths.clone();
    let settings: SettingsState = crate::config::settings::new_settings_state(loaded_settings);
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
    if let Ok(store) = &message_store_state.store {
        store.bind_response_close(response_close.clone());
    } else {
        response_close.fail_closed();
    }
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
        .manage(response_close.clone())
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
            pty_mgr.lock().unwrap().bind_response_close(response_close_for_setup.clone());
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

            // #2482 agent quota engine. A sibling of the scraper, same construction shape, and
            // like it must come after `.manage(settings)`.
            let agent_quota_engine = AgentQuotaEngine::new(
                Arc::new(QuotaRows {
                    pty_mgr: pty_mgr.clone(),
                    poison_logged: AtomicBool::new(false),
                }),
                Arc::new(QuotaSources {
                    settings: app.state::<SettingsState>().inner().clone(),
                }),
                Arc::new(QuotaSink {
                    app_handle: app.handle().clone(),
                }),
            );
            agent_quota_engine.start(shutdown_for_setup.clone());
            app.manage(Arc::clone(&agent_quota_engine));

            // #1171 watcher engine. A SIBLING of the scraper above, not an extension of it:
            // different interval, different modes, its own history. Same construction shape,
            // and for the same reason it must come after `.manage(settings)`.
            let watcher_history: crate::pty::watchers::history::WatcherHistoryState =
                Arc::new(crate::pty::watchers::history::WatcherHistory::default());
            // #2879 - plus the internal Codex activity factory over the idle detector.
            let watcher_engine = WatcherEngine::new_with_activity_factory(
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
                Some(codex_activity_factory(&idle_detector_for_setup)),
            );
            watcher_engine.start(shutdown_for_setup.clone());
            app.manage(Arc::clone(&watcher_engine));
            app.manage(watcher_history);
            // #1646 / #1647 - proactive detection of terminal blocking menus
            let menu_guard = Arc::new(crate::pty::menu_guard::MenuGuard::with_response_close(
                config::settings::BlockingMenusStore::load_from_config_dir(),
                response_close_for_setup.clone(),
            ));
            if app.try_state::<crate::pty::input_activity::TypingHoldState>().is_some()
                && app.try_state::<crate::pty::manager::ResponseCloseState>().is_some_and(|bound| bound.same_state(&response_close_for_setup)) {
                response_close_for_setup.bind_publisher(crate::pty::manager::ResponseClosePublisher::Hold);
            }
            let manager = tauri::async_runtime::block_on(async {
                let managed = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let guard = managed.read().await;
                guard.clone()
            });
            let response_close_wired = {
                let pty = pty_mgr.lock().unwrap();
                response_close_wiring_matches(&response_close_for_setup, &manager, &pty, &idle_detector_for_setup, &menu_guard)
            };
            if !response_close_wired || response_close_for_setup.try_enable_readiness().is_err() {
                response_close_for_setup.fail_closed();
                log::warn!("[response-close] safety publisher wiring unavailable; readiness remains disabled");
            }
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
                                    "[web-token] Remote URL: \
                                        http://{}:{}/?window=main&remoteToken={}",
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

            // #1925 - detached remote blocking-menu patterns download. Fail-silent; applies at the
            // next start.
            {
                let app_handle_for_remote_menus = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    crate::update_check::run_remote_blocking_menus_startup(
                        app_handle_for_remote_menus,
                    )
                    .await;
                });
            }
            // #2133 - detached remote per-agent help download. Fail-silent; applies at the next
            // start.
            {
                let app_handle_for_remote_agent_help = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    crate::update_check::run_remote_agent_help_startup(
                        app_handle_for_remote_agent_help,
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
            let mut persisted = load_startup_sessions(&restore_settings_snapshot, |paths| {
                tauri::async_runtime::block_on(
                    sessions_persistence::load_sessions_purging_outside_project_paths(paths, &restore_settings_snapshot),
                )
            });
            if let Err(error) = &persisted {
                log::error!("[restore] Project state unavailable; session load/purge skipped: {}", error);
            }
            if let Ok(persisted) = &mut persisted {
                match normalize_persisted_active_flags(persisted) {
                    PersistedActiveFlagNormalization::Zero => {
                        log::debug!("[restore] persisted selection flags normalized: zero");
                    }
                    PersistedActiveFlagNormalization::One { index } => {
                        log::debug!(
                            "[restore] persisted selection flags normalized: exactly one \
                                rowIndex={}",
                            index
                        );
                    }
                    PersistedActiveFlagNormalization::Multiple { identities } => {
                        log::warn!(
                            "[restore] inconsistent was_active flags count={} rows=[{}]; exact \
                                target cleared for eligible-live fallback",
                            identities.len(),
                            identities.join(", ")
                        );
                    }
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
                                    "[test-window] MonitorFromRect returned null for requested \
                                        rect ({}, {}) {}x{}",
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
                                        "[test-window] GetMonitorInfoW failed for requested \
                                            rect ({}, {}) {}x{}",
                                        x,
                                        y,
                                        width,
                                        height
                                    );
                                } else {
                                    log::info!(
                                        "[test-window] selected monitor rect=({}, {}) {}x{} \
                                            work=({}, {}) {}x{} for requested rect ({}, {}) \
                                            {}x{} maximized={}",
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

            // Resolve main geometry: saved (physical) -> validate -> convert to logical ->
            // fallback.
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
                    "[test-window] requested physical ({}, {}) {}x{} maximized={} -> logical \
                        ({}, {}) {}x{}",
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
                            "[window-setup] main: saved physical ({}, {}) {}x{} -> logical ({}, \
                                {}) {}x{}",
                            geo.x, geo.y, geo.width, geo.height,
                            logical.x, logical.y, logical.width, logical.height
                        );
                        logical
                    }
                    Some(geo) => {
                        log::warn!(
                            "[window-setup] main: saved geometry ({}, {}) {}x{} is off-screen, \
                                falling back to centered default",
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
                    "[restore] {} teams discovered across {} project paths; setting_on={}; \
                        evaluating {} persisted sessions",
                    teams.len(),
                    settings_snapshot.project_paths.len(),
                    setting_on,
                    persisted.as_ref().map(Vec::len).unwrap_or(0)
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

                start_native_restore_startup(
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
                commands::pty::get_agent_quota_readings,
                commands::pty::get_watcher_activity,
                commands::pty::preview_watcher_pattern,
                commands::pty::preview_watcher_reach,
                commands::config::get_settings,
                commands::config::list_agent_file_sources,
                commands::config::read_agent_file_source,
                commands::config::save_agent_file_source,
                commands::config::remember_agent_file_source,
                commands::config::get_agent_help,
                commands::config::get_coding_agent_catalog,
                commands::config::get_coding_agent_welcome_status,
                commands::config::install_coding_agent,
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
                commands::config::reorder_coding_agent,
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
                commands::config::append_rail_diagnostic,
                commands::config::drain_error_logs,
                commands::config::get_error_popup_mutes,
                commands::config::set_error_popup_muted,
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
                commands::ac_discovery::task_get_snapshot,
                commands::ac_discovery::task_get_snapshot_at,
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
                commands::ac_discovery::get_project_file_difference,
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
                        // #2563 - same orphan class for detached terminals when
                        // main is destroyed without a quit round (OS logoff,
                        // programmatic destroy). Leaves `was_detached` intact.
                        let mut destroyed = 0usize;
                        for (label, window) in app_handle.webview_windows() {
                            if !label.starts_with("terminal-") {
                                continue;
                            }
                            if let Err(e) = window.destroy() {
                                log::warn!(
                                    "[shutdown] detached window destroy failed: {label}: {e}"
                                );
                            }
                            destroyed += 1;
                        }
                        if destroyed > 0 {
                            log::info!(
                                "[shutdown] main destroyed: destroyed {} detached terminal \
                                    window(s)",
                                destroyed
                            );
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
                        "[shutdown] terminal snapshot scanner drained joined={} aborted={} \
                            terminal={}",
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
                                "[shutdown] PTY owner lock reached the global cleanup deadline \
                                    before container ownership transfer"
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
                                "[shutdown] PTY owner lock reached the job cleanup deadline \
                                    state=retained"
                            );
                            container_shutdown.terminal = false;
                            container_shutdown
                                .retained
                                .push("reason=global-job-owner state=retained".to_string());
                            (0, 0)
                        }
                    };
                    log::info!(
                        "[shutdown] terminated {jobs_killed} agent job object(s); \
                            {jobless_sessions} session(s) had no job"
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
                                        "[shutdown] resource group {} quarantined during \
                                            cleanup: {}",
                                        result.session_id,
                                        result.message
                                    );
                                }
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => log::warn!(
                            "[shutdown] resource cleanup exceeded \
                                {SHUTDOWN_CLEANUP_BUDGET_SECS}s budget; proceeding to exit (job \
                                objects already terminated jobbed trees)"
                        ),
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => log::error!(
                            "[shutdown] resource cleanup thread panicked before completing; \
                                jobbed trees were still terminated by the job kill"
                        ),
                    }
                    // #632 MED-2 - job-less sessions are reaper-only; if the reaper did
                    // not finish, their trees may be orphaned. Make it visible.
                    if cleanup.is_err() && jobless_sessions > 0 {
                        log::warn!(
                            "[shutdown] {jobless_sessions} session(s) had no Job Object and the \
                                reaper did not finish in budget; their process trees may be \
                                orphaned"
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
                            "[shutdown] final session persistence skipped because cleanup \
                                ownership is retained work=[{}]",
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
/// watcher forwards for every record, invalidation and clearing. #2756 U2a
/// `Recovery` re-offers a candidate that a not-ready trigger left pending, once
/// the room is `Ready`; it takes the `SlotChanged` path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoManagedTrigger {
    IdleEdge(uuid::Uuid),
    SlotChanged(uuid::Uuid),
    Recovery(uuid::Uuid),
}

impl CoManagedTrigger {
    fn session_id(self) -> uuid::Uuid {
        match self {
            Self::IdleEdge(id) | Self::SlotChanged(id) | Self::Recovery(id) => id,
        }
    }
}

/// #2756 U2a: a candidate dropped because the room was not `Ready`. Both legs
/// must match the slot for a recovery: `seq`, and the retained `Arc`, which
/// keeps the allocation alive so `Arc::ptr_eq` cannot alias a new record.
struct PendingRecovery {
    seq: u64,
    record: Arc<capture::record::CapturedRecord>,
}

#[derive(Default)]
struct CoManagedSupervisorState {
    /// Sessions whose last cycle ended in contention, keyed to the slot
    /// sequence that candidate had. The same sequence must not re-trigger.
    contended: HashMap<String, u64>,
    /// Sessions whose slot already has a watcher task.
    watching: HashSet<String>,
    /// #2756 U2a: candidates dropped while not `Ready`, awaiting recovery.
    pending_recovery: HashMap<String, PendingRecovery>,
}

#[cfg(test)]
#[derive(Default)]
struct CoManagedTestHooks {
    steps: std::sync::Mutex<Vec<&'static str>>,
    commit_calls: std::sync::atomic::AtomicUsize,
    /// Runs once inside `commit_co_managed_with_retries`, after a successful commit and before the
    /// caller consumes the slot. One-shot: the only way a test can offer a newer record in that
    /// window.
    after_commit: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
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

    fn set_after_commit(&self, hook: Box<dyn FnOnce() + Send>) {
        *self.after_commit.lock().unwrap_or_else(|e| e.into_inner()) = Some(hook);
    }

    fn take_after_commit(&self) -> Option<Box<dyn FnOnce() + Send>> {
        self.after_commit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
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

    fn notify_recovery(&self, session_id: uuid::Uuid) -> bool {
        self.triggers
            .send(CoManagedTrigger::Recovery(session_id))
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
        let mut state = self.lock_state();
        state.watching.remove(id);
        state.pending_recovery.remove(id);
    }

    fn mark_pending_recovery(
        &self,
        id: &str,
        seq: u64,
        record: Arc<capture::record::CapturedRecord>,
    ) {
        self.lock_state()
            .pending_recovery
            .insert(id.to_string(), PendingRecovery { seq, record });
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

    /// Test-only seam: see `CoManagedTestHooks::after_commit`.
    fn run_after_commit_hook(&self) {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            if let Some(hook) = hooks.take_after_commit() {
                hook();
            }
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
            OffReason::GlobalSwitchOff => "GlobalSwitchOff".to_string(),
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
) -> Result<capture::jev::JevSettings, config::settings::SettingsOwnerError> {
    let settings = app.state::<SettingsState>();
    let guard = settings.current_read().await?;
    Ok(capture::jev::JevSettings {
        api_key: guard.jev_api_key.clone(),
        model: guard.jev_model.clone(),
        endpoint: guard.jev_endpoint.clone(),
        timeout_secs: guard.jev_timeout_secs,
        threshold: guard.jev_threshold,
        margin: guard.jev_margin,
    })
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
) -> Result<Option<String>, String> {
    let (path, pointer) = write_co_managed_message_file(room_root, from_fqn, to_fqn, content)?;
    handle.record_step("messaging_file");
    let display_from = crate::phone::messaging::compose_sender_for_comanaged_origin(from_fqn);
    let overhead = crate::phone::messaging::PTY_WRAP_FIXED + display_from.len();
    if pointer.len() + overhead > crate::phone::messaging::PTY_SAFE_MAX {
        // Nothing was delivered; the file was only just created by us, so it is
        // removed rather than left as a never-referenced orphan.
        let _ = std::fs::remove_file(&path);
        return Err(format!(
            "notification exceeds PTY-safe length (body {} + overhead {} > {}); shorten slug or \
                move the room to a shallower path",
            pointer.len(),
            overhead,
            crate::phone::messaging::PTY_SAFE_MAX
        ));
    }
    let archive_warning = (|| -> Result<(), String> {
        let source = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
        let snapshot = std::fs::read(&source).map_err(|e| e.to_string())?;
        let basename = source
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("invalid message basename")?;
        let ac_root = room_root.parent().ok_or("room has no Project AC Root")?;
        crate::phone::messaging::archive_message_snapshot(ac_root, &source, basename, &snapshot)?;
        Ok(())
    })()
    .err()
    .map(|error| format!("Message archive warning: {}: {error}", path.display()));
    write_co_managed_queue_message(room_root, from_fqn, to_fqn, &pointer).map_err(|error| {
        match &archive_warning {
            Some(warning) => format!("{error}; {warning}"),
            None => error,
        }
    })?;
    handle.record_step("queue_file");
    Ok(archive_warning)
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
                                    "{decision} names peer '{peer}', which is not reachable; \
                                        nothing was routed"
                                ),
                                write_candidate_file: true,
                            }
                        }
                    }
                    _ => CoManagedRoute::User {
                        reason: format!(
                            "{decision} names an orchestrator destination without a peer; \
                                nothing was routed"
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
                                "{decision} chose Root, but this session is not a verified room \
                                    orchestrator; nothing was routed"
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
        match &result {
            Ok(committed) => log::info!(
                "[co-managed] commit [{session_id}]: attempt={} committed kind={:?} routable={}",
                attempt + 1,
                committed.kind,
                committed.routable
            ),
            Err(reason) => log::info!(
                "[co-managed] commit [{session_id}]: attempt={} abstained: {}",
                attempt + 1,
                abstain_reason_label(reason)
            ),
        }
        match result {
            Ok(committed) => {
                handle.run_after_commit_hook();
                return CoManagedCommit::Committed(committed);
            }
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
    let settings = match co_managed_jev_settings(app).await {
        Ok(settings) => settings,
        Err(error) => {
            log::debug!("[co-managed] settings unavailable: {error}");
            return CoManagedOutcome::Done("settings_unavailable".to_string());
        }
    };
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
                // Consume this exact candidate; a newer offer stays in the
                // slot and re-triggers its own cycle.
                slot.try_consume(expected_seq, &expected_key, expected_key.epoch);
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
    let outcome = {
        let network = app.state::<crate::network::OutboundNetwork>();
        capture::jev::classify(
            &network,
            &settings,
            &catalog,
            &text,
            &session_id.to_string()[..8],
        )
        .await
    };

    // (3) Decide before committing, so an unreachable peer never spends budget.
    let project_paths = {
        let settings = app.state::<SettingsState>();
        let guard = match settings.current_read().await {
            Ok(guard) => guard,
            Err(error) => {
                log::debug!("[co-managed] settings unavailable: {error}");
                return CoManagedOutcome::Done("settings_unavailable".to_string());
            }
        };
        co_managed_project_paths(&guard.project_paths, room_root)
    };
    let route = resolve_co_managed_route(outcome, &catalog, &from_fqn, &project_paths, &text);
    log::info!(
        "[co-managed] route [{session_id}]: variant={} kind={:?}",
        match &route {
            CoManagedRoute::User { .. } => "User",
            CoManagedRoute::Wake { .. } => "Wake",
        },
        route.kind()
    );
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
            // Consume this exact candidate; a newer offer stays in the slot
            // and re-triggers its own cycle.
            slot.try_consume(expected_seq, &expected_key, expected_key.epoch);
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
                        // Consume this exact candidate; a newer offer stays
                        // in the slot and re-triggers its own cycle.
                        slot.try_consume(expected_seq, &expected_key, expected_key.epoch);
                        let message = co_managed_user_message(
                            &format!(
                                "abstained: the automatic budget is exhausted ({} actions since \
                                    the last recharge); the candidate was not routed",
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
            Ok(warning) => {
                if let Some(warning) = warning {
                    surface_co_managed_user_message(
                        app,
                        session_id,
                        format!("Send queued; message archive warning: {warning}"),
                    )
                    .await;
                }
                CoManagedOutcome::Done(reason)
            }
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
        log::info!("[co-managed] trigger dropped [{id}]: no capture registry in state");
        return;
    };
    let Some(slot) = registry.slot(&id) else {
        log::info!(
            "[co-managed] trigger dropped [{id}]: no capture slot for the session; flag removed"
        );
        handle.armed.remove(&id);
        return;
    };
    let state = slot.snapshot();
    let Some(candidate) = state.value.record().cloned() else {
        // Consumed, invalidated or cleared: the candidate this flag described
        // is gone, so the flag goes with it.
        log::info!(
            "[co-managed] trigger dropped [{id}]: slot holds no candidate record; flag removed"
        );
        handle.armed.remove(&id);
        return;
    };

    let session = {
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = manager.read().await;
        guard.get_session(session_id).await
    };
    let Some(session) = session else {
        log::info!("[co-managed] trigger dropped [{id}]: session is gone; flag removed");
        handle.armed.remove(&id);
        return;
    };
    let Some(room_root) =
        crate::config::co_managed::room_root_for_path(Path::new(&session.working_directory))
    else {
        log::info!(
            "[co-managed] trigger dropped [{id}]: working directory {} resolves to no room \
                root; flag removed",
            session.working_directory
        );
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
        let catalog_path = crate::config::co_managed::load_config(&room_root)
            .catalog_path
            .map(|path| room_root.join(path));
        log::warn!(
            "[co-managed] trigger dropped [{id}]: not ready ({}); room {}, catalog {:?}; flag \
                removed",
            co_managed_state_reason(&effective),
            room_root.display(),
            catalog_path
        );
        // #2756 U2a: the slot keeps the candidate; remember it so the next
        // reader-demand pass can re-offer it once the room is Ready.
        handle.mark_pending_recovery(&id, state.seq, Arc::clone(&candidate));
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
        log::info!(
            "[co-managed] trigger dropped [{id}]: already contended for sequence {}",
            state.seq
        );
        return;
    }

    let already_idle = matches!(session.status, crate::session::session::SessionStatus::Idle)
        || session.waiting_for_input;
    let is_idle_edge = matches!(trigger, CoManagedTrigger::IdleEdge(_));
    if !is_idle_edge && !already_idle {
        log::info!(
            "[co-managed] trigger dropped [{id}]: not an idle edge and the session is not idle"
        );
        return;
    }
    if !is_idle_edge {
        // Trigger (b): a record arrived while the session was already idle.
        // This transition does not coincide with the idle edge, so it carries
        // its own event before the cycle performs anything.
        emit_co_managed_state(app, session_id, true, None);
    }

    log::info!(
        "[co-managed] cycle start [{id}]: trigger={} seq={}",
        match trigger {
            CoManagedTrigger::IdleEdge(_) => "IdleEdge",
            CoManagedTrigger::SlotChanged(_) => "SlotChanged",
            CoManagedTrigger::Recovery(_) => "Recovery",
        },
        state.seq
    );
    let outcome = co_managed_cycle(
        app, handle, session_id, &room_root, &slot, state.seq, &candidate,
    )
    .await;
    match outcome {
        CoManagedOutcome::Contention => {
            log::info!("[co-managed] cycle end [{id}]: Contention, reason contention; candidate stays pending");
            emit_co_managed_state(app, session_id, false, Some("contention"));
            // The flag stays set while the unchanged candidate remains pending.
        }
        CoManagedOutcome::Done(reason) => {
            handle.clear_contended(&id);
            handle.armed.disarm(&id);
            log::info!(
                "[co-managed] cycle end [{id}]: Done({}); flag disarmed",
                capture::jev::safe_reason(&reason)
            );
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
        let mut reader_demand = tokio::time::interval(Duration::from_secs(READER_DEMAND_TICK_SECS));
        reader_demand.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                _ = discovery.tick() => watch_capture_slots(&app, &handle).await,
                _ = reader_demand.tick() => {
                    reader_demand_pass(&app).await;
                    recovery_pass(&app, &handle).await;
                }
                trigger = triggers.recv() => {
                    let Some(trigger) = trigger else { break };
                    handle_co_managed_trigger(&app, &handle, trigger).await;
                }
            }
        }
    });
}

/// #2456 period of the Room reader-demand re-raise tick.
const READER_DEMAND_TICK_SECS: u64 = 5;

/// #2456 test-only count of the tick's calls into readiness resolution, per
/// session: a global counter would move under a concurrent test's tick.
#[cfg(test)]
static TICK_RAISE_CALLS: std::sync::LazyLock<Mutex<HashMap<uuid::Uuid, usize>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
pub(crate) fn tick_raise_calls(session_id: uuid::Uuid) -> usize {
    TICK_RAISE_CALLS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&session_id)
        .copied()
        .unwrap_or(0)
}

/// #2456 D5-j test-only count of the tick's post-raise rechecks, per session.
#[cfg(test)]
static RECHECK_CALLS: std::sync::LazyLock<Mutex<HashMap<uuid::Uuid, usize>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
pub(crate) fn tick_recheck_calls(session_id: uuid::Uuid) -> usize {
    RECHECK_CALLS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&session_id)
        .copied()
        .unwrap_or(0)
}

/// #2525 one reader-demand tick: sweep when Co-managed is unavailable
/// globally, otherwise the unchanged #2456 re-raise pass.
async fn reader_demand_pass<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if release_room_reader_demands_when_globally_off(app).await {
        return;
    }
    reraise_room_reader_demands(app).await;
}

/// #2525 release the Room demand of every live session when the global switch
/// is off; `true` iff it swept. #2756 U2a: an empty Jev API key no longer
/// sweeps, since the key is a `Ready` input, not a capture input. The release runs
/// for sessions holding nothing too: it still bumps the Room `DemandEpoch`,
/// which aborts a raise already in flight (D-e). Logs once per pass, and only
/// when a session actually held a Room demand, so a steady Off is silent.
async fn release_room_reader_demands_when_globally_off<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> bool {
    let Some(settings) = app.try_state::<SettingsState>() else {
        return false;
    };
    let enabled = match settings.current_read().await {
        Ok(guard) => guard.co_managed_enabled,
        Err(error) => {
            log::debug!("[co-managed] settings unavailable; reader demands retained: {error}");
            return false;
        }
    };
    if crate::config::co_managed::globally_capturable(enabled) {
        return false;
    }
    let session_ids: Vec<uuid::Uuid> = {
        let Some(manager) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() else {
            return false;
        };
        let guard = manager.read().await;
        guard
            .list_sessions()
            .await
            .into_iter()
            .filter_map(|s| uuid::Uuid::parse_str(&s.id).ok())
            .collect()
    };
    let mut held = 0usize;
    for session_id in session_ids {
        if commands::telegram::holds_room_reader_demand(app, session_id).await {
            held += 1;
        }
        commands::session::release_room_reader_demand(app, session_id).await;
    }
    if held > 0 {
        log::info!(
            "[co-managed] {held} reader demands released: Co-managed is unavailable globally"
        );
    }
    true
}

/// #2456 raise the Room reader demand for every live session that lacks one.
///
/// The create, restart and room-toggle raises each check readiness once, so a
/// room that becomes Ready later (key set, catalog created, `config.json`
/// edited) would keep no reader. The cheap in-memory gate runs first; only a
/// session without a Room demand pays for readiness resolution, which
/// `raise_room_reader_demand` owns. The tick releases only a demand it
/// installed in the same iteration and then found no longer Ready (D5-h). It
/// logs only for a session it raised and kept, so a steady state is silent.
/// The raise's `true` means "a reader is running afterwards", not "inserted".
async fn reraise_room_reader_demands<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
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
        if commands::telegram::holds_room_reader_demand(app, session_id).await {
            continue;
        }
        #[cfg(test)]
        {
            *TICK_RAISE_CALLS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(session_id)
                .or_insert(0) += 1;
        }
        if !commands::session::raise_room_reader_demand(app, session_id).await {
            continue;
        }
        #[cfg(test)]
        {
            *RECHECK_CALLS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(session_id)
                .or_insert(0) += 1;
        }
        // D5-h: a disable landing while the raise resolved and installed has
        // already released, so undo the demand this iteration installed.
        if !room_still_capturable(app, session_id).await {
            commands::session::release_room_reader_demand(app, session_id).await;
            continue;
        }
        log::info!("[co-managed] reader demand re-raised [{session_id}]");
    }
}

/// #2456 D5-h re-resolve the session's room and capturability after a raise
/// (#2756 U2a: the same predicate the raise used).
async fn room_still_capturable<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: uuid::Uuid,
) -> bool {
    let session = {
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = manager.read().await;
        guard.get_session(session_id).await
    };
    let Some(session) = session else {
        return false;
    };
    let Some(room_root) =
        crate::config::co_managed::room_root_for_path(Path::new(&session.working_directory))
    else {
        return false;
    };
    matches!(
        crate::commands::session::co_managed_capturable_for_session(
            app,
            &room_root,
            &session_id.to_string()
        )
        .await,
        Ok(true)
    )
}

/// #2756 U2a: re-offer every candidate a not-ready trigger left pending, once
/// its room is `Ready`. Only marked sessions are visited. A mark whose slot is
/// gone, or whose `seq` or record no longer match the slot, is dropped; a
/// matching mark in a not-yet-Ready room waits for the next pass. The trigger
/// goes through the channel so this tick arm never runs a cycle inline, and it
/// is sent even to a busy session: the handler arms, and the next idle edge
/// runs the cycle.
async fn recovery_pass<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
) {
    let marked: Vec<String> = handle
        .lock_state()
        .pending_recovery
        .keys()
        .cloned()
        .collect();
    if marked.is_empty() {
        return;
    }
    let registry = app.try_state::<Arc<capture::registry::CaptureRegistry>>();
    for id in marked {
        let Some(slot) = registry.as_ref().and_then(|registry| registry.slot(&id)) else {
            handle.lock_state().pending_recovery.remove(&id);
            continue;
        };
        let current = slot.snapshot();
        let matches = {
            let state = handle.lock_state();
            let Some(mark) = state.pending_recovery.get(&id) else {
                continue;
            };
            mark.seq == current.seq
                && current
                    .value
                    .record()
                    .is_some_and(|record| Arc::ptr_eq(record, &mark.record))
        };
        let session_id = match uuid::Uuid::parse_str(&id) {
            Ok(session_id) if matches => session_id,
            _ => {
                handle.lock_state().pending_recovery.remove(&id);
                continue;
            }
        };
        let session = match app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() {
            Some(manager) => manager.read().await.get_session(session_id).await,
            None => None,
        };
        let Some(room_root) = session.and_then(|session| {
            crate::config::co_managed::room_root_for_path(Path::new(&session.working_directory))
        }) else {
            continue;
        };
        let ready = matches!(
            crate::commands::session::co_managed_effective_state_for_session(app, &room_root, &id)
                .await,
            Ok(crate::config::co_managed::CoManagedState::Ready)
        );
        if !ready {
            continue;
        }
        handle.lock_state().pending_recovery.remove(&id);
        log::info!("[co-managed] recovery emitted [{id}]: seq={}", current.seq);
        let _ = handle.notify_recovery(session_id);
    }
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
mod tests;
