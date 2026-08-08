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
pub(crate) mod window_target_registry;

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

/// Shared state injected into every handler.
#[derive(Clone)]
pub struct ApiState {
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
    Router::new()
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
        .route("/api/v1/healthz", get(handlers::health))
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
