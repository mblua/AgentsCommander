//! In-daemon control-plane API (#791). A sibling of `web/`: same axum stack and
//! lifecycle, but a distinct trust model (per-client scoped tokens, not the
//! single web token) and its own router. It exposes locally-bound send,
//! list-peers, and container session-transport endpoints, funnelling sends
//! through the SAME actuation the filesystem poller uses (`actuation.rs`).
//!
//! Auth is UNCONDITIONAL in every build profile (no `web/`-style debug bypass):
//! a machine-to-machine control-plane that can wake/inject peer PTYs must never
//! skip token validation.

pub mod actuation;
pub mod audit;
pub mod auth;
pub mod dispatcher;
pub mod error;
pub mod handlers;
pub mod identity;
pub mod message_store;
pub mod schema;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use tauri::Manager;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

/// Hard ceiling on a buffered request body. Inline sends allow a 256 KiB
/// semantic body plus JSON envelope overhead.
const MAX_BODY_LIMIT_BYTES: usize = crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES;
const STARTUP_READY_TIMEOUT: Duration = Duration::from_secs(2);

pub struct ApiServerStart {
    pub join_handle: tauri::async_runtime::JoinHandle<()>,
    pub readiness: oneshot::Receiver<Result<SocketAddr, String>>,
}

pub(crate) const WINDOW_SCREENSHOT_MAX_ACTIVE: usize = 1;
pub(crate) const WINDOW_SCREENSHOT_MAX_QUEUED: usize = 2;
pub(crate) const WINDOW_SCREENSHOT_MAX_ADMITTED: usize =
    WINDOW_SCREENSHOT_MAX_ACTIVE + WINDOW_SCREENSHOT_MAX_QUEUED;
pub(crate) const WINDOW_SCREENSHOT_ADVISORY_SOURCE_PIXELS: u64 = 16_777_216;
pub(crate) const WINDOW_SCREENSHOT_MAX_PNG_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub(crate) enum WindowScreenshotAdmissionError {
    CaptureBusy,
}

/// Process-local admission and active-slot limiter for native window
/// screenshots. Two permits gate every request:
///
/// - `admission` (capacity `WINDOW_SCREENSHOT_MAX_ADMITTED`) bounds the
///   requests that are queued or running at once. `try_admit` acquires it
///   synchronously and returns `CaptureBusy` when full, so the handler refuses
///   with 429 before any native work starts.
/// - `active` (capacity `WINDOW_SCREENSHOT_MAX_ACTIVE`) bounds the single
///   native capture worker. `acquire_active` awaits a free slot, so admitted
///   requests queue here instead of launching concurrent captures.
///
/// Permit ownership protocol: the handler holds the admission permit while it
/// awaits the active slot, then moves both permits into a
/// `WindowScreenshotLease` and into the worker. A request future dropped
/// while waiting drops its admission permit and frees that slot; a dropped
/// request whose worker already started keeps the lease until the native work
/// ends, so detached workers cannot exceed the one-active, three-admitted
/// bound. Covered by `window_screenshot_limiter_queue_is_bounded_and_waiter_drop_releases_admission`
/// and the route-level queue tests in `pty/terminal_snapshot/acceptance_tests.rs`.
pub(crate) struct WindowScreenshotLimiter {
    admission: std::sync::Arc<tokio::sync::Semaphore>,
    active: std::sync::Arc<tokio::sync::Semaphore>,
}

/// Owned admission and active permit pair. Created only after both permits are
/// held, moved into the capture worker, and released together when the
/// worker's native capture and PNG encoding finish. Holding both for the full
/// worker lifetime keeps the limiter bounds intact even when the requesting
/// HTTP client disconnects and the route future is dropped.
pub(crate) struct WindowScreenshotLease {
    _admission: tokio::sync::OwnedSemaphorePermit,
    _active: tokio::sync::OwnedSemaphorePermit,
}

impl WindowScreenshotLimiter {
    pub(crate) fn new() -> Self {
        Self {
            admission: std::sync::Arc::new(tokio::sync::Semaphore::new(
                WINDOW_SCREENSHOT_MAX_ADMITTED,
            )),
            active: std::sync::Arc::new(tokio::sync::Semaphore::new(WINDOW_SCREENSHOT_MAX_ACTIVE)),
        }
    }

    pub(crate) fn try_admit(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, WindowScreenshotAdmissionError> {
        self.admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| WindowScreenshotAdmissionError::CaptureBusy)
    }

    pub(crate) async fn acquire_active(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, WindowScreenshotAdmissionError> {
        self.active
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| WindowScreenshotAdmissionError::CaptureBusy)
    }
}

impl WindowScreenshotLease {
    pub(crate) fn new(
        admission: tokio::sync::OwnedSemaphorePermit,
        active: tokio::sync::OwnedSemaphorePermit,
    ) -> Self {
        Self {
            _admission: admission,
            _active: active,
        }
    }
}

#[cfg(test)]
mod window_screenshot_limiter_tests {
    use super::*;

    #[tokio::test]
    async fn window_screenshot_limiter_queue_is_bounded_and_waiter_drop_releases_admission() {
        let limiter = std::sync::Arc::new(WindowScreenshotLimiter::new());
        let first_admission = match limiter.try_admit() {
            Ok(permit) => permit,
            Err(error) => panic!("first request must be admitted: {error:?}"),
        };
        let first_active = match limiter.acquire_active().await {
            Ok(active) => active,
            Err(error) => panic!("first request must acquire the active slot: {error:?}"),
        };
        let first_lease = WindowScreenshotLease::new(first_admission, first_active);

        let waiting_admission = match limiter.try_admit() {
            Ok(permit) => permit,
            Err(error) => panic!("second request must be admitted: {error:?}"),
        };
        let queued_admission = match limiter.try_admit() {
            Ok(permit) => permit,
            Err(error) => panic!("third request must be admitted: {error:?}"),
        };
        assert!(matches!(
            limiter.try_admit(),
            Err(WindowScreenshotAdmissionError::CaptureBusy)
        ));

        let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
        let waiter_limiter = std::sync::Arc::clone(&limiter);
        let mut waiter = tokio::spawn(async move {
            let _ = started_sender.send(());
            let active = waiter_limiter.acquire_active().await?;
            Ok::<_, WindowScreenshotAdmissionError>(WindowScreenshotLease::new(
                waiting_admission,
                active,
            ))
        });
        assert!(started_receiver.await.is_ok());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut waiter)
                .await
                .is_err()
        );

        waiter.abort();
        let _ = waiter.await;
        assert!(limiter.try_admit().is_ok());

        drop(queued_admission);
        drop(first_lease);
    }
}

/// Shared state injected into every handler.
#[derive(Clone)]
pub struct ApiState {
    #[allow(private_interfaces)]
    pub window_screenshot_limiter: std::sync::Arc<WindowScreenshotLimiter>,
    /// Read-through client-token registry (mtime-gated).
    pub store: Arc<auth::ApiClientStore>,
    /// Durable inline send queue and idempotency store.
    pub message_store: Arc<message_store::MessageStore>,
    /// Per-source failed-auth lockout.
    pub lockout: Arc<auth::FailedAuthLockout>,
    /// Reach to the live daemon (SessionManager / PtyManager / SettingsState)
    /// for actuation, via `app.state::<...>()`.
    pub app_handle: tauri::AppHandle,
    /// Live sessions, used by the container session transport.
    pub session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    /// PTY facade, used to reach the container transport backend.
    pub pty_mgr: Arc<Mutex<PtyManager>>,
}

/// Build the router (state already assembled). Split out so tests can mount it
/// without a socket.
pub fn build_router(state: ApiState) -> Router {
    let router = Router::new()
        .route("/api/v1/send", post(handlers::send::handle))
        .route("/api/v1/pty-input", post(handlers::pty_input::post))
        .route(
            "/api/v1/terminal-snapshot",
            post(handlers::terminal_snapshot::post),
        )
        .route("/api/v1/pty-input/{op_id}", get(handlers::pty_input::get))
        .route("/api/v1/peers", get(handlers::list_peers::handle))
        .route(
            "/api/v1/session-transport",
            get(handlers::session_transport::handle),
        )
        // Unauthenticated liveness; body pinned to {"ok":true} (§0.5 G9).
        .route("/api/v1/healthz", get(handlers::health));

    #[cfg(target_os = "windows")]
    let router = router.route(
        handlers::window_screenshot::WINDOW_SCREENSHOT_ROUTE,
        handlers::window_screenshot::route(),
    );

    router
        .layer(DefaultBodyLimit::max(MAX_BODY_LIMIT_BYTES))
        .with_state(state)
}

pub async fn wait_for_startup_ready(
    readiness: oneshot::Receiver<Result<SocketAddr, String>>,
) -> Result<SocketAddr, String> {
    match tokio::time::timeout(STARTUP_READY_TIMEOUT, readiness).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("API server startup task ended before reporting readiness".to_string()),
        Err(_) => Err(format!(
            "API server did not report bind readiness within {:?}",
            STARTUP_READY_TIMEOUT
        )),
    }
}

/// Start the control-plane API server on the shared tokio runtime, mirroring
/// `web::start_server`. Returns the join handle plus a readiness receiver for
/// the managed `ApiServerHandle`. On any startup failure (unresolvable config
/// dir, invalid address, or `bind` error) it logs at error, sends readiness
/// `Err`, and the task returns cleanly: it does NOT panic (§0.5 dev-rust F7,
/// unlike `web/mod.rs`'s `.expect()`).
pub fn start_server(
    bind: String,
    port: u16,
    app_handle: tauri::AppHandle,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    shutdown: CancellationToken,
) -> ApiServerStart {
    let (readiness_tx, readiness) = oneshot::channel();
    let store = match auth::ApiClientStore::at_config_dir() {
        Some(s) => Arc::new(s),
        None => {
            let message = "Cannot resolve config_dir for API server".to_string();
            log::error!("[api-server] {}; API server not started", message);
            let _ = readiness_tx.send(Err(message));
            return ApiServerStart {
                join_handle: tauri::async_runtime::spawn(async {}),
                readiness,
            };
        }
    };
    let message_store = match app_handle.try_state::<message_store::MessageStoreState>() {
        Some(state) => match &state.store {
            Ok(store) => Arc::clone(store),
            Err(code) => {
                let message = format!("Cannot initialize API DB message store: {code}");
                log::error!("[api-server] {}; API server not started", message);
                let _send_result = readiness_tx.send(Err(message));
                return ApiServerStart {
                    join_handle: tauri::async_runtime::spawn(async {}),
                    readiness,
                };
            }
        },
        None => {
            let message = "Managed API DB message store is unavailable".to_string();
            log::error!("[api-server] {}; API server not started", message);
            let _send_result = readiness_tx.send(Err(message));
            return ApiServerStart {
                join_handle: tauri::async_runtime::spawn(async {}),
                readiness,
            };
        }
    };

    let state = ApiState {
        window_screenshot_limiter: Arc::new(WindowScreenshotLimiter::new()),
        store: store.clone(),
        message_store: message_store.clone(),
        lockout: Arc::new(auth::FailedAuthLockout::default()),
        app_handle: app_handle.clone(),
        session_mgr,
        pty_mgr,
    };
    let router = build_router(state);

    let join_handle = tauri::async_runtime::spawn(async move {
        let mut readiness_tx = Some(readiness_tx);
        let dispatcher_handle = dispatcher::start_dispatcher(
            message_store,
            store,
            app_handle.clone(),
            shutdown.clone(),
            dispatcher::DispatcherConfig::default(),
        );
        let addr: SocketAddr =
            match crate::config::settings::parse_api_server_socket_addr(&bind, port) {
                Ok(a) => a,
                Err(e) => {
                    let message =
                        format!("Invalid API server bind address {}:{}: {}", bind, port, e);
                    log::error!("[api-server] {}", message);
                    if let Some(tx) = readiness_tx.take() {
                        let _ = tx.send(Err(message));
                    }
                    shutdown.cancel();
                    wait_for_dispatcher(dispatcher_handle, "invalid-bind").await;
                    return;
                }
            };

        // Loud warning on any non-loopback bind (§0.5 DESIGN DECISION).
        if !addr.ip().is_loopback() {
            let warning = format!(
                "[api-server] WARNING: bound on {} (non-loopback); ensure a host firewall restricts this port to the Docker/WSL subnet.",
                addr
            );
            log::warn!("{}", warning);
            println!("{}", warning);
        }

        // Bind-failure = log-and-return, NOT panic (§0.5 dev-rust F7).
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                let message = format!("API server bind failed on {}: {}", addr, e);
                log::error!("[api-server] {}; API server not started", message);
                if let Some(tx) = readiness_tx.take() {
                    let _ = tx.send(Err(message));
                }
                shutdown.cancel();
                wait_for_dispatcher(dispatcher_handle, "bind-failure").await;
                return;
            }
        };

        if let Some(tx) = readiness_tx.take() {
            let _ = tx.send(Ok(addr));
        }
        log::info!("[api-server] listening on http://{}", addr);
        println!("[api-server] listening on http://{}", addr);

        let service = router.into_make_service_with_connect_info::<SocketAddr>();
        let server_shutdown = shutdown.clone();
        if let Err(e) = axum::serve(listener, service)
            .with_graceful_shutdown(async move {
                server_shutdown.cancelled().await;
                log::info!("[api-server] shutdown signal received, stopping");
            })
            .await
        {
            log::error!("[api-server] server error: {}", e);
        }
        shutdown.cancel();
        wait_for_dispatcher(dispatcher_handle, "server-stop").await;
    });

    ApiServerStart {
        join_handle,
        readiness,
    }
}

const DISPATCHER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

async fn wait_for_dispatcher(
    mut dispatcher_handle: tauri::async_runtime::JoinHandle<()>,
    reason: &'static str,
) {
    match tokio::time::timeout(DISPATCHER_SHUTDOWN_TIMEOUT, &mut dispatcher_handle).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            log::warn!(
                "[api-server] dispatcher task failed during {} shutdown: {}",
                reason,
                err
            );
        }
        Err(_) => {
            log::warn!(
                "[api-server] dispatcher did not stop within {:?} during {}; aborting",
                DISPATCHER_SHUTDOWN_TIMEOUT,
                reason
            );
            dispatcher_handle.abort();
        }
    }
}
