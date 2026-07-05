//! In-daemon control-plane API (#791). A sibling of `web/`: same axum stack and
//! lifecycle, but a distinct trust model (per-client scoped tokens, not the
//! single web token) and its own router. Increment 1 serves exactly two verbs
//! (`send`, `list-peers-lean`) over a locally-bound TCP transport, funnelling
//! through the SAME actuation the filesystem poller uses (`actuation.rs`).
//!
//! Auth is UNCONDITIONAL in every build profile (no `web/`-style debug bypass):
//! a machine-to-machine control-plane that can wake/inject peer PTYs must never
//! skip token validation.

pub mod actuation;
pub mod audit;
pub mod auth;
pub mod error;
pub mod handlers;
pub mod identity;
pub mod idempotency;
pub mod schema;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;

/// Hard ceiling on a buffered request body (defense in depth above the send
/// handler's 16 KB semantic cap).
const MAX_BODY_LIMIT_BYTES: usize = 64 * 1024;

/// Shared state injected into every handler.
#[derive(Clone)]
pub struct ApiState {
    /// Read-through client-token registry (mtime-gated).
    pub store: Arc<auth::ApiClientStore>,
    /// Disk-persisted `send` idempotency ledger.
    pub ledger: Arc<idempotency::IdempotencyLedger>,
    /// Per-source failed-auth lockout.
    pub lockout: Arc<auth::FailedAuthLockout>,
    /// Reach to the live daemon (SessionManager / PtyManager / SettingsState)
    /// for actuation, via `app.state::<...>()`.
    pub app_handle: tauri::AppHandle,
}

/// Build the router (state already assembled). Split out so tests can mount it
/// without a socket.
pub fn build_router(state: ApiState) -> Router {
    Router::new()
        .route("/api/v1/send", post(handlers::send::handle))
        .route("/api/v1/peers", get(handlers::list_peers::handle))
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
    shutdown: crate::shutdown::ShutdownSignal,
) -> tauri::async_runtime::JoinHandle<()> {
    let store = match auth::ApiClientStore::at_config_dir() {
        Some(s) => Arc::new(s),
        None => {
            log::error!("[api-server] cannot resolve config_dir; API server not started");
            return tauri::async_runtime::spawn(async {});
        }
    };
    let ledger = match idempotency::IdempotencyLedger::at_config_dir() {
        Some(l) => Arc::new(l),
        None => {
            log::error!("[api-server] cannot resolve config_dir for idempotency ledger; API server not started");
            return tauri::async_runtime::spawn(async {});
        }
    };

    let state = ApiState {
        store,
        ledger,
        lockout: Arc::new(auth::FailedAuthLockout::default()),
        app_handle,
    };
    let router = build_router(state);

    tauri::async_runtime::spawn(async move {
        let addr: SocketAddr = match format!("{}:{}", bind, port).parse() {
            Ok(a) => a,
            Err(e) => {
                log::error!("[api-server] invalid bind address {}:{}: {}", bind, port, e);
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
