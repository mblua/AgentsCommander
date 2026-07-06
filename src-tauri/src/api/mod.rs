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
pub mod idempotency;
pub mod identity;
pub mod message_store;
pub mod schema;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;

use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

/// Hard ceiling on a buffered request body. Inline sends allow a 256 KiB
/// semantic body plus JSON envelope overhead.
const MAX_BODY_LIMIT_BYTES: usize = message_store::INLINE_BODY_MAX_BYTES + (16 * 1024);

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

/// Start the control-plane API server on the shared tokio runtime, mirroring
/// `web::start_server`. Returns the join handle for the managed
/// `ApiServerHandle`. On any startup failure (unresolvable config dir, invalid
/// address, or `bind` error) it logs at error and the task returns cleanly: it
/// does NOT panic (§0.5 dev-rust F7, unlike `web/mod.rs`'s `.expect()`).
pub fn start_server(
    bind: String,
    port: u16,
    app_handle: tauri::AppHandle,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    shutdown: crate::shutdown::ShutdownSignal,
) -> tauri::async_runtime::JoinHandle<()> {
    let store = match auth::ApiClientStore::at_config_dir() {
        Some(s) => Arc::new(s),
        None => {
            log::error!("[api-server] cannot resolve config_dir; API server not started");
            return tauri::async_runtime::spawn(async {});
        }
    };
    let message_store = match message_store::MessageStore::at_config_dir() {
        Ok(s) => Arc::new(s),
        Err(e) => {
            log::error!(
                "[api-server] cannot initialize DB message store; API server not started: {}",
                e
            );
            return tauri::async_runtime::spawn(async {});
        }
    };

    let state = ApiState {
        store,
        message_store: message_store.clone(),
        lockout: Arc::new(auth::FailedAuthLockout::default()),
        app_handle: app_handle.clone(),
        session_mgr,
        pty_mgr,
    };
    let router = build_router(state);

    tauri::async_runtime::spawn(async move {
        let dispatcher_handle = dispatcher::start_dispatcher(
            message_store,
            app_handle.clone(),
            shutdown.clone(),
            dispatcher::DispatcherConfig::default(),
        );
        let addr: SocketAddr = match format!("{}:{}", bind, port).parse() {
            Ok(a) => a,
            Err(e) => {
                log::error!("[api-server] invalid bind address {}:{}: {}", bind, port, e);
                dispatcher_handle.abort();
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
                log::error!(
                    "[api-server] bind failed on {} (port occupied?): {}; API server not started",
                    addr,
                    e
                );
                dispatcher_handle.abort();
                return;
            }
        };

        log::info!("[api-server] listening on http://{}", addr);
        println!("[api-server] listening on http://{}", addr);

        let service = router.into_make_service_with_connect_info::<SocketAddr>();
        if let Err(e) = axum::serve(listener, service)
            .with_graceful_shutdown(async move {
                shutdown.token().cancelled().await;
                log::info!("[api-server] shutdown signal received, stopping");
            })
            .await
        {
            log::error!("[api-server] server error: {}", e);
        }
    })
}
