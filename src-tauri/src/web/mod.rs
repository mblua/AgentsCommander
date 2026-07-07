pub mod auth;
pub mod broadcast;
pub mod commands;
mod embedded;

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
}

/// Start the embedded HTTP/WebSocket server.
/// Called from Tauri's setup(), runs on the same tokio runtime.
// Wired by a single setup() call with all shared state already in scope; an
// args struct would just rename the same fields.
#[allow(clippy::too_many_arguments)]
pub async fn start_server(
    bind: String,
    port: u16,
    web_token: Arc<WebAccessToken>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    settings: SettingsState,
    broadcaster: WsBroadcaster,
    app_handle: tauri::AppHandle,
    shutdown: crate::shutdown::ShutdownSignal,
) -> Result<tauri::async_runtime::JoinHandle<()>, String> {
    // Resolve dist path BEFORE moving app_handle into WsState
    let dist_path = resolve_dist_path(&app_handle);

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

    let addr: SocketAddr = format!("{}:{}", bind, port)
        .parse()
        .map_err(|e| format!("Invalid web server bind address {}:{}: {}", bind, port, e))?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Failed to bind web server on {}: {}", addr, e))?;

    log::info!("[web-server] Listening on http://{}", addr);
    println!("[web-server] Listening on http://{}", addr);

    let handle = tauri::async_runtime::spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown.token().cancelled().await;
                log::info!("[web-server] Shutdown signal received, stopping");
            })
            .await
        {
            log::error!("[web-server] server error: {}", e);
        }
    });

    Ok(handle)
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

    ws.on_upgrade(move |socket| handle_ws_connection(socket, state.ws_state))
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
async fn handle_ws_connection(socket: WebSocket, ws_state: WsState) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Subscribe to broadcasts
    let mut broadcast_rx = ws_state.broadcaster.subscribe();

    // Forward broadcasts to this client
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = broadcast_rx.recv().await {
            let ws_msg = match msg {
                broadcast::WsOutMsg::Text(text) => Message::Text(text.into()),
                broadcast::WsOutMsg::Binary(data) => Message::Binary(data.into()),
            };
            if SinkExt::send(&mut ws_sender, ws_msg).await.is_err() {
                break;
            }
        }
    });

    // Read commands from client
    let state_clone = ws_state.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = StreamExt::next(&mut ws_receiver).await {
            match msg {
                Message::Text(text) => {
                    handle_text_message(&state_clone, &text).await;
                }
                Message::Binary(data) => {
                    handle_binary_message(&state_clone, &data).await;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // Wait for either task to finish, then abort and reap the other.
    tokio::select! {
        _ = &mut send_task => {
            recv_task.abort();
            let _ = recv_task.await;
        }
        _ = &mut recv_task => {
            send_task.abort();
            let _ = send_task.await;
        }
    }
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
    // Write FIRST (the std Mutex guard is dropped at the end of this statement,
    // never held across the await below), then record the user message (#552).
    let write_result = state.pty_mgr.lock().unwrap().write(uuid, pty_data);
    if !binary_pty_write_succeeded(write_result, uuid) {
        return;
    }

    // #552 web UI keystrokes (binary frame) are the real web input path and a
    // genuine user message: reset the badge clock + auto-close silence.
    crate::commands::pty::note_user_message_to_session(&state.app_handle, uuid).await;
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
}
