pub mod auth;
pub mod broadcast;
pub mod commands;
mod embedded;
pub mod event_broadcast;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use tauri::Manager;
use tokio_util::sync::CancellationToken;
use tokio_util::task::task_tracker::TaskTrackerToken;
use tokio_util::task::TaskTracker;
use tower_http::services::ServeDir;

use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

use self::auth::WebAccessToken;
use self::broadcast::WsBroadcaster;
use self::commands::WsState;

/// Shared state for the axum server.
#[derive(Clone)]
struct AppState {
    web_token: Arc<WebAccessToken>,
    ws_state: WsState,
    admission: Arc<WebSocketAdmission>,
    generation_token: CancellationToken,
}

#[derive(Clone)]
struct WebSocketConnectionContext {
    ws_state: WsState,
    admission: Arc<WebSocketAdmission>,
    generation_token: CancellationToken,
}

/// Per-generation admission gate and semantic-work tracker. The lifecycle
/// always takes its own mutex before calling `open` or `close`; this type never
/// calls back into the lifecycle, preserving the global lock order.
pub(super) struct WebSocketAdmission {
    open: Mutex<bool>,
    tracker: TaskTracker,
    generation_token: CancellationToken,
}

impl WebSocketAdmission {
    pub(super) fn new(generation_token: CancellationToken) -> Self {
        Self {
            open: Mutex::new(false),
            tracker: TaskTracker::new(),
            generation_token,
        }
    }

    pub(super) fn open(&self, shutdown: &crate::shutdown::ShutdownSignal) -> bool {
        let mut open = self.open.lock().unwrap();
        if self.generation_token.is_cancelled() || shutdown.is_cancelled() {
            return false;
        }
        *open = true;
        // A global trigger does not take the lifecycle mutex. Recheck after the
        // write so a trigger racing the first observation cannot leave a window
        // in which the generation appears open after shutdown won.
        if self.generation_token.is_cancelled() || shutdown.is_cancelled() {
            *open = false;
            self.tracker.close();
            return false;
        }
        true
    }

    pub(super) fn close(&self) {
        let mut open = self.open.lock().unwrap();
        *open = false;
        self.tracker.close();
    }

    pub(super) fn try_acquire(&self) -> Option<WebSocketAdmissionGuard> {
        let open = self.open.lock().unwrap();
        if !*open || self.generation_token.is_cancelled() {
            return None;
        }
        let token = self.tracker.token();
        drop(open);
        Some(WebSocketAdmissionGuard { _token: token })
    }

    pub(super) async fn wait(&self) {
        self.tracker.wait().await;
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.tracker.is_empty()
    }
}

pub(super) struct WebSocketAdmissionGuard {
    _token: TaskTrackerToken,
}

fn acquire_websocket_upgrade_guard(
    admission: &WebSocketAdmission,
) -> Result<WebSocketAdmissionGuard, axum::http::StatusCode> {
    admission
        .try_acquire()
        .ok_or(axum::http::StatusCode::SERVICE_UNAVAILABLE)
}

/// §1453: error tipado del arranque. Reemplaza el String opaco para que los
/// call sites registren bind/port/causa sin parsear texto. `Display`
/// reproduce byte a byte los dos mensajes historicos que app.log ya conoce
/// (los dashboards/greps existentes no cambian).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartServerError {
    /// El valor de settings no parsea como SocketAddr.
    #[error("Invalid web server bind address {bind}:{port}: {detail}")]
    InvalidAddr {
        bind: String,
        port: u16,
        detail: String,
    },
    /// TcpListener::bind fallo sobre una direccion parseada.
    /// `bind` es el string CRUDO de settings y existe solo para la guardia de
    /// obsolescencia y el payload (contrato de D2); `addr` es el SocketAddr
    /// parseado y existe solo para que Display sea byte-exacto.
    #[error("Failed to bind web server on {addr}: {detail}")]
    BindFailed {
        bind: String,
        addr: std::net::SocketAddr,
        detail: String,
    },
}

/// Start the embedded HTTP/WebSocket server.
/// Called from Tauri's setup(), runs on the same tokio runtime.
// Wired by a single setup() call with all shared state already in scope; an
// args struct would just rename the same fields.
#[allow(clippy::too_many_arguments)]
pub(super) async fn start_server(
    bind: String,
    port: u16,
    web_token: Arc<WebAccessToken>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    settings: SettingsState,
    broadcaster: WsBroadcaster,
    app_handle: tauri::AppHandle,
    admission: Arc<WebSocketAdmission>,
    generation_token: CancellationToken,
    shutdown: crate::shutdown::ShutdownSignal,
) -> Result<tauri::async_runtime::JoinHandle<()>, StartServerError> {
    // Resolve dist path BEFORE moving app_handle into WsState
    let dist_path = resolve_dist_path(&app_handle);

    let app = build_router(
        web_token,
        session_mgr,
        pty_mgr,
        settings,
        broadcaster,
        app_handle,
        admission,
        generation_token.clone(),
        dist_path,
    );

    // El turbofish es OBLIGATORIO: map_err se interpone entre parse() y el `?`,
    // asi que anotar el `let` no alcanza para inferir el target del parse y
    // rustc emite E0282 (plan 5.2.2 afirmaba lo contrario).
    let addr = format!("{}:{}", bind, port)
        .parse::<SocketAddr>()
        .map_err(|e| StartServerError::InvalidAddr {
            bind: bind.clone(),
            port,
            detail: e.to_string(),
        })?;

    let listener =
        tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| StartServerError::BindFailed {
                bind: bind.clone(),
                addr,
                detail: e.to_string(),
            })?;

    log::info!("[web-server] Listening on http://{}", addr);
    println!("[web-server] Listening on http://{}", addr);

    Ok(spawn_server_on_listener(
        listener,
        app,
        generation_token,
        shutdown,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_router(
    web_token: Arc<WebAccessToken>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    settings: SettingsState,
    broadcaster: WsBroadcaster,
    app_handle: tauri::AppHandle,
    admission: Arc<WebSocketAdmission>,
    generation_token: CancellationToken,
    dist_path: Option<std::path::PathBuf>,
) -> Router {
    let ws_state = WsState {
        session_mgr,
        pty_mgr,
        settings,
        broadcaster,
        app_handle,
    };
    let state = AppState {
        web_token,
        ws_state,
        admission,
        generation_token,
    };
    let mut app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/sessions", get(api_sessions_handler))
        .with_state(state);

    #[cfg(has_embedded_dist)]
    {
        if prefer_embedded_dist() {
            log::info!("[web-server] Serving static files from embedded dist");
            app = app.fallback(embedded::embedded_static_handler);
        } else if let Some(path) = dist_path {
            log::info!("[web-server] Serving static files from {:?}", path);
            app = app.fallback_service(ServeDir::new(path).append_index_html_on_directories(true));
        } else {
            log::warn!("[web-server] No dist/ directory found; static file serving disabled");
        }
    }

    #[cfg(not(has_embedded_dist))]
    {
        if let Some(path) = dist_path {
            log::info!("[web-server] Serving static files from {:?}", path);
            app = app.fallback_service(ServeDir::new(path).append_index_html_on_directories(true));
        } else {
            log::warn!("[web-server] No dist/ directory found; static file serving disabled");
        }
    }

    app
}

pub(super) fn spawn_server_on_listener(
    listener: tokio::net::TcpListener,
    app: Router,
    generation_token: CancellationToken,
    shutdown: crate::shutdown::ShutdownSignal,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let global_token = shutdown.token().clone();
        let generation_for_shutdown = generation_token.clone();
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                tokio::select! {
                    _ = generation_token.cancelled() => {
                        log::info!("[web-server] Generation stop received, stopping listener");
                    }
                    _ = global_token.cancelled() => {
                        generation_for_shutdown.cancel();
                        log::info!("[web-server] Global shutdown received, stopping listener");
                    }
                }
            })
            .await
        {
            log::error!("[web-server] server error: {}", e);
        }
    })
}

/// WebSocket upgrade handler with token validation.
async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // In dev mode, skip token validation for easier testing
    if !cfg!(debug_assertions) {
        let token = params.get("token").cloned().unwrap_or_default();
        if !state.web_token.matches(&token) {
            return (axum::http::StatusCode::UNAUTHORIZED, "Invalid token").into_response();
        }
    }

    let connection_guard = match acquire_websocket_upgrade_guard(&state.admission) {
        Ok(guard) => guard,
        Err(status) => return (status, "Web server is stopping").into_response(),
    };
    let context = WebSocketConnectionContext {
        ws_state: state.ws_state,
        admission: state.admission,
        generation_token: state.generation_token,
    };

    ws.on_upgrade(move |socket| handle_ws_connection(socket, context, connection_guard))
}

/// Public session view for the HTTP API, omits sensitive fields like `token`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiSessionView {
    id: String,
    name: String,
    working_directory: String,
    status: crate::session::session::SessionStatus,
    waiting_for_input: bool,
    created_at: String,
    shell: String,
    git_branch: Option<String>,
    last_prompt: Option<String>,
}

/// HTTP GET /api/sessions, returns JSON array of all sessions.
async fn api_sessions_handler(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Token validation (same as WS: skip in dev)
    if !cfg!(debug_assertions) {
        let token = params.get("token").cloned().unwrap_or_default();
        if !state.web_token.matches(&token) {
            return (axum::http::StatusCode::UNAUTHORIZED, "Invalid token").into_response();
        }
    }

    let mgr = state.ws_state.session_mgr.read().await;
    let sessions = mgr.list_sessions().await;

    // Project to public view (no token) and apply optional status filter
    let status_filter = params.get("status").map(|s| s.to_lowercase());
    let views: Vec<ApiSessionView> = sessions
        .into_iter()
        .filter(|s| {
            if let Some(ref filter) = status_filter {
                let s_status = match &s.status {
                    crate::session::session::SessionStatus::Active => "active",
                    crate::session::session::SessionStatus::Running => "running",
                    crate::session::session::SessionStatus::Idle => "idle",
                    crate::session::session::SessionStatus::Exited(_) => "exited",
                };
                s_status == filter.as_str()
            } else {
                true
            }
        })
        .map(|s| ApiSessionView {
            id: s.id,
            name: s.name,
            working_directory: s.working_directory,
            status: s.status,
            waiting_for_input: s.waiting_for_input,
            created_at: s.created_at,
            shell: s.shell,
            // Back-compat: present each repo as "<label>/<branch>" (or bare label when
            // branch unknown), joined with ", ". Comma, not newline, so single-line
            // JSON clients don't truncate.
            git_branch: if s.git_repos.is_empty() {
                None
            } else {
                Some(
                    s.git_repos
                        .iter()
                        .map(|r| match &r.branch {
                            Some(b) => format!("{}/{}", r.label, b),
                            None => r.label.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            },
            last_prompt: s.last_prompt,
        })
        .collect();

    Json(views).into_response()
}

/// Handle an authenticated WebSocket connection.
async fn handle_ws_connection(
    socket: WebSocket,
    context: WebSocketConnectionContext,
    connection_guard: WebSocketAdmissionGuard,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let connection_token = context.generation_token.child_token();

    // Subscribe to broadcasts
    let mut broadcast_rx = context.ws_state.broadcaster.subscribe();

    // Forward broadcasts to this client
    let send_token = connection_token.clone();
    let send_task = tokio::spawn(async move {
        loop {
            let msg = tokio::select! {
                _ = send_token.cancelled() => break,
                msg = broadcast_rx.recv() => match msg {
                    Some(msg) => msg,
                    None => break,
                },
            };
            let ws_msg = match msg {
                broadcast::WsOutMsg::Text(text) => Message::Text(text.into()),
                broadcast::WsOutMsg::Binary(data) => Message::Binary(data.into()),
            };
            let send = SinkExt::send(&mut ws_sender, ws_msg);
            tokio::pin!(send);
            tokio::select! {
                _ = send_token.cancelled() => break,
                result = &mut send => {
                    if result.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Read commands from client
    let recv_token = connection_token.clone();
    let recv_context = context.clone();
    let recv_task = tokio::spawn(async move {
        loop {
            let next = tokio::select! {
                _ = recv_token.cancelled() => break,
                next = StreamExt::next(&mut ws_receiver) => next,
            };
            let Some(Ok(msg)) = next else {
                break;
            };
            match msg {
                Message::Text(text) => {
                    if !run_admitted_websocket_frame(&recv_context.admission, || {
                        handle_text_message(&recv_context.ws_state, &text)
                    })
                    .await
                    {
                        break;
                    }
                }
                Message::Binary(data) => {
                    if !run_admitted_websocket_frame(&recv_context.admission, || {
                        handle_binary_message(&recv_context.ws_state, &data)
                    })
                    .await
                    {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    reap_websocket_halves(send_task, recv_task, connection_token, connection_guard).await;
}

async fn reap_websocket_halves(
    mut send_task: tokio::task::JoinHandle<()>,
    mut recv_task: tokio::task::JoinHandle<()>,
    connection_token: CancellationToken,
    _connection_guard: WebSocketAdmissionGuard,
) {
    // Wait for either half, cooperatively cancel I/O in the other, and reap it.
    // An admitted handler does not select on the token, so it always completes.
    let send_finished = tokio::select! {
        _ = &mut send_task => {
            true
        }
        _ = &mut recv_task => {
            false
        }
    };
    connection_token.cancel();
    if send_finished {
        let _ = recv_task.await;
    } else {
        let _ = send_task.await;
    }
}

async fn run_admitted_websocket_frame<F, Fut>(admission: &WebSocketAdmission, handler: F) -> bool
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let Some(_frame_guard) = admission.try_acquire() else {
        return false;
    };
    handler().await;
    true
}

/// Handle a JSON text command from a WS client.
async fn handle_text_message(state: &WsState, text: &str) {
    let parsed: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };

    let id = parsed.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
    let cmd = match parsed.get("cmd").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return,
    };
    let args = parsed.get("args").cloned().unwrap_or(serde_json::json!({}));

    let response = commands::dispatch(state, id, cmd, &args).await;

    // Send response back to this specific client via broadcast
    // (We use the broadcaster's text broadcast, which goes to all clients.
    //  For command responses, we include the id so the client matches it.)
    let response_text = response.to_string();
    state.broadcaster.broadcast_event(
        "__cmd_response",
        &serde_json::json!({
            "id": id,
            "data": response,
        }),
    );

    // Actually, command responses should go to the requesting client only.
    // Since we don't have a per-client sender here, we broadcast with the id.
    // The client filters by id. This is acceptable for low client counts.
    let _ = response_text; // suppress warning
}

/// Handle a binary PTY write from a WS client.
/// Format: [36 bytes UUID ASCII][raw PTY input bytes]
async fn handle_binary_message(state: &WsState, data: &[u8]) {
    if data.len() < 36 {
        return;
    }

    let session_id_str = match std::str::from_utf8(&data[..36]) {
        Ok(s) => s.trim(),
        Err(_) => return,
    };

    let uuid = match uuid::Uuid::parse_str(session_id_str) {
        Ok(u) => u,
        Err(_) => return,
    };

    let pty_data = &data[36..];
    let write_result =
        match crate::pty::manager::PtyManager::acquire_input_writer(&state.pty_mgr, uuid).await {
            Ok(permit) => {
                if state
                    .app_handle
                    .try_state::<std::sync::Arc<crate::session::purge_guard::PurgeGuard>>()
                    .is_some_and(|guard| guard.blocks_session(uuid))
                {
                    return;
                }
                let result = crate::pty::manager::PtyManager::write_with_permit(&permit, pty_data);
                if result.is_ok() {
                    crate::commands::pty::mark_successful_pty_write_busy(
                        &state.app_handle,
                        uuid,
                        pty_data.len(),
                    )
                    .await;
                }
                result
            }
            Err(error) => Err(error),
        };
    if !binary_pty_write_succeeded(write_result, uuid) {
        return;
    }

    // #552 web UI keystrokes (binary frame) are the real web input path and a
    // genuine user message: reset the badge clock + auto-close silence.
    crate::commands::pty::note_user_message_to_session(
        &state.app_handle,
        uuid,
        crate::commands::pty::UserInputSource::Web(pty_data),
    )
    .await;
}

fn binary_pty_write_succeeded(
    result: Result<(), crate::errors::AppError>,
    uuid: uuid::Uuid,
) -> bool {
    match result {
        Ok(()) => true,
        Err(err) => {
            log::warn!(
                "[web-server] PTY binary write failed for session {}: {}",
                uuid,
                err
            );
            false
        }
    }
}

#[cfg(has_embedded_dist)]
fn prefer_embedded_dist() -> bool {
    crate::config::profile::BUILD_PROFILE != "dev"
}

/// Resolve the dist/ directory for static file serving.
fn resolve_dist_path(app_handle: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    // 1. Tauri resource dir (production NSIS bundle)
    if let Ok(resource_dir) = app_handle.path().resource_dir() {
        let dist = resource_dir.join("dist");
        if dist.exists() && dist.is_dir() {
            log::info!("[web-server] Found dist via resource_dir: {:?}", dist);
            return Some(dist);
        }
    }

    // 2. Relative to executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let dist = parent.join("dist");
            if dist.exists() && dist.is_dir() {
                log::info!("[web-server] Found dist next to exe: {:?}", dist);
                return Some(dist);
            }
            // Dev mode: target/debug/exe to project root/dist.
            if let Some(grandparent) = parent.parent() {
                let dist = grandparent.join("dist");
                if dist.exists() && dist.is_dir() {
                    return Some(dist);
                }
                if let Some(ggparent) = grandparent.parent() {
                    let dist = ggparent.join("dist");
                    if dist.exists() && dist.is_dir() {
                        return Some(dist);
                    }
                }
            }
        }
    }

    // 3. CWD fallbacks (dev mode)
    for path in &["dist", "../dist"] {
        let p = std::path::PathBuf::from(path);
        if p.exists() && p.is_dir() {
            return Some(p.canonicalize().unwrap_or(p));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::AppError;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::{mpsc, Semaphore};

    // §1453 D2: Display preserva byte a byte los strings historicos de app.log,
    // incluida la forma IPv6 bracketed que solo el SocketAddr produce bien.
    #[test]
    fn start_server_error_display_matches_legacy_log_format() {
        let invalid = StartServerError::InvalidAddr {
            bind: "notanip".to_string(),
            port: 8888,
            detail: "invalid socket address syntax".to_string(),
        };
        assert_eq!(
            invalid.to_string(),
            "Invalid web server bind address notanip:8888: invalid socket address syntax"
        );

        let bindfail = StartServerError::BindFailed {
            bind: "192.168.1.12".to_string(),
            addr: "192.168.1.12:8888".parse().unwrap(),
            detail: "The requested address is not valid in its context. (os error 10049)"
                .to_string(),
        };
        assert_eq!(
            bindfail.to_string(),
            "Failed to bind web server on 192.168.1.12:8888: The requested address is not valid in its context. (os error 10049)"
        );

        // El crudo bracketed se conserva para la guardia, y Display usa el addr.
        let v6 = StartServerError::BindFailed {
            bind: "[::1]".to_string(),
            addr: "[::1]:8888".parse().unwrap(),
            detail: "os error 10049".to_string(),
        };
        assert_eq!(
            v6.to_string(),
            "Failed to bind web server on [::1]:8888: os error 10049"
        );
    }

    #[test]
    fn binary_pty_write_success_allows_user_message_note() {
        assert!(binary_pty_write_succeeded(Ok(()), uuid::Uuid::new_v4()));
    }

    #[test]
    fn binary_pty_write_failure_blocks_user_message_note() {
        let uuid = uuid::Uuid::new_v4();
        let result = Err(AppError::SessionNotFound(uuid.to_string()));

        assert!(!binary_pty_write_succeeded(result, uuid));
    }

    #[tokio::test]
    async fn websocket_upgrade_guard_precedes_on_upgrade() {
        let generation_token = CancellationToken::new();
        let admission = WebSocketAdmission::new(generation_token);
        let shutdown = crate::shutdown::ShutdownSignal::new();
        assert!(admission.open(&shutdown));

        let guard = acquire_websocket_upgrade_guard(&admission)
            .expect("open generation admits upgrade before callback creation");
        admission.close();
        assert_eq!(
            acquire_websocket_upgrade_guard(&admission).err(),
            Some(axum::http::StatusCode::SERVICE_UNAVAILABLE)
        );
        let wait = admission.wait();
        tokio::pin!(wait);
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut wait)
            .await
            .is_err());
        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), wait)
            .await
            .expect("upgrade guard drains after callback cleanup");
    }

    #[tokio::test]
    async fn old_websocket_frames_after_stop_never_dispatch() {
        let generation_token = CancellationToken::new();
        let admission = WebSocketAdmission::new(generation_token.clone());
        let shutdown = crate::shutdown::ShutdownSignal::new();
        assert!(admission.open(&shutdown));
        admission.close();
        generation_token.cancel();

        let text_general = Arc::new(AtomicUsize::new(0));
        let text_switch = Arc::new(AtomicUsize::new(0));
        let binary_pty = Arc::new(AtomicUsize::new(0));
        for counter in [&text_general, &text_switch, &binary_pty] {
            let counter = Arc::clone(counter);
            assert!(
                !run_admitted_websocket_frame(&admission, move || async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                })
                .await
            );
        }
        assert_eq!(text_general.load(Ordering::SeqCst), 0);
        assert_eq!(text_switch.load(Ordering::SeqCst), 0);
        assert_eq!(binary_pty.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stop_drains_admitted_text_and_binary_semantics() {
        let generation_token = CancellationToken::new();
        let admission = Arc::new(WebSocketAdmission::new(generation_token.clone()));
        let shutdown = crate::shutdown::ShutdownSignal::new();
        assert!(admission.open(&shutdown));
        let release = Arc::new(Semaphore::new(0));
        let (entered_sender, mut entered_receiver) = mpsc::unbounded_channel();
        let effects = Arc::new([
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
        ]);
        let mut tasks = Vec::new();
        for index in 0..3 {
            let admission = Arc::clone(&admission);
            let release = Arc::clone(&release);
            let entered_sender = entered_sender.clone();
            let effects = Arc::clone(&effects);
            tasks.push(tokio::spawn(async move {
                run_admitted_websocket_frame(&admission, move || async move {
                    entered_sender
                        .send(index)
                        .expect("test observes admitted semantic handler");
                    let permit = release.acquire().await.expect("release semaphore open");
                    permit.forget();
                    effects[index].fetch_add(1, Ordering::SeqCst);
                })
                .await
            }));
        }
        drop(entered_sender);
        let mut entered = Vec::new();
        for _ in 0..3 {
            entered.push(
                tokio::time::timeout(Duration::from_secs(1), entered_receiver.recv())
                    .await
                    .expect("handler admission timed out")
                    .expect("handler admission channel closed"),
            );
        }
        entered.sort_unstable();
        assert_eq!(entered, [0, 1, 2]);

        admission.close();
        generation_token.cancel();
        let wait = admission.wait();
        tokio::pin!(wait);
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut wait)
            .await
            .is_err());
        release.add_permits(3);
        for task in tasks {
            assert!(task.await.expect("semantic task panicked"));
        }
        tokio::time::timeout(Duration::from_secs(1), wait)
            .await
            .expect("admitted semantic work drains");
        assert_eq!(effects[0].load(Ordering::SeqCst), 1); // Text general.
        assert_eq!(effects[1].load(Ordering::SeqCst), 1); // Text switch_session.
        assert_eq!(effects[2].load(Ordering::SeqCst), 1); // Binary PTY.
    }

    #[tokio::test]
    async fn socket_cleanup_reaps_both_halves_without_aborting_admitted_handler() {
        let generation_token = CancellationToken::new();
        let connection_token = generation_token.child_token();
        let admission = Arc::new(WebSocketAdmission::new(generation_token.clone()));
        let shutdown = crate::shutdown::ShutdownSignal::new();
        assert!(admission.open(&shutdown));
        let connection_guard = admission
            .try_acquire()
            .expect("running generation admits connection");
        let handler_effect = Arc::new(AtomicBool::new(false));
        let sender_reaped = Arc::new(AtomicBool::new(false));
        let receiver_reaped = Arc::new(AtomicBool::new(false));
        let (handler_entered_sender, mut handler_entered_receiver) = mpsc::unbounded_channel();
        let release = Arc::new(Semaphore::new(0));

        let sender_token = connection_token.clone();
        let sender_reaped_for_task = Arc::clone(&sender_reaped);
        let send_task = tokio::spawn(async move {
            sender_token.cancelled().await;
            sender_reaped_for_task.store(true, Ordering::SeqCst);
        });
        let recv_admission = Arc::clone(&admission);
        let recv_effect = Arc::clone(&handler_effect);
        let recv_release = Arc::clone(&release);
        let receiver_reaped_for_task = Arc::clone(&receiver_reaped);
        let recv_task = tokio::spawn(async move {
            assert!(
                run_admitted_websocket_frame(&recv_admission, move || async move {
                    handler_entered_sender
                        .send(())
                        .expect("test observes admitted receive handler");
                    let permit = recv_release
                        .acquire()
                        .await
                        .expect("release semaphore open");
                    permit.forget();
                    recv_effect.store(true, Ordering::SeqCst);
                })
                .await
            );
            receiver_reaped_for_task.store(true, Ordering::SeqCst);
        });
        handler_entered_receiver
            .recv()
            .await
            .expect("receive handler entered");
        let mut cleanup = tokio::spawn(reap_websocket_halves(
            send_task,
            recv_task,
            connection_token,
            connection_guard,
        ));

        admission.close();
        generation_token.cancel();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !sender_reaped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("send half did not observe cancellation");
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut cleanup)
                .await
                .is_err()
        );
        assert!(!handler_effect.load(Ordering::SeqCst));
        assert!(!receiver_reaped.load(Ordering::SeqCst));

        release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(1), cleanup)
            .await
            .expect("socket cleanup timed out")
            .expect("socket cleanup task panicked");
        tokio::time::timeout(Duration::from_secs(1), admission.wait())
            .await
            .expect("connection and frame guards did not drain");
        assert!(handler_effect.load(Ordering::SeqCst));
        assert!(sender_reaped.load(Ordering::SeqCst));
        assert!(receiver_reaped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn spawn_server_on_listener_stops_for_generation_or_global_signal() {
        let generation_shutdown = crate::shutdown::ShutdownSignal::new();
        let generation_token = CancellationToken::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind generation listener");
        let generation_server = spawn_server_on_listener(
            listener,
            Router::new(),
            generation_token.clone(),
            generation_shutdown.clone(),
        );
        generation_token.cancel();
        tokio::time::timeout(Duration::from_secs(1), generation_server)
            .await
            .expect("generation cancellation did not stop listener")
            .expect("generation server task panicked");
        assert!(!generation_shutdown.is_cancelled());

        let global_shutdown = crate::shutdown::ShutdownSignal::new();
        let independent_token = CancellationToken::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind global listener");
        let global_server = spawn_server_on_listener(
            listener,
            Router::new(),
            independent_token.clone(),
            global_shutdown.clone(),
        );
        global_shutdown.trigger();
        tokio::time::timeout(Duration::from_secs(1), global_server)
            .await
            .expect("global shutdown did not stop listener")
            .expect("global server task panicked");
        assert!(independent_token.is_cancelled());
    }
}
