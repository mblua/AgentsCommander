//! Tauri sinks for PTY watchers: `ContextEventSink` emits `session_context` and
//! `QuotaEventSink` emits `agent_quota` to all windows.
//! `WatcherDelivery::to_watchers_window` sends `watcher_matches` batches only to the
//! watchers window and reports whether that window exists. No parsing happens here.

use crate::commands;
use crate::pty::agent_quota::{AgentQuotaPayload, QuotaEventSink};
use crate::pty::context_scrape::{ContextEventSink, ContextUsagePayload};
use crate::pty_wiring::{WatcherDelivery, WatchersScopeEvents};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime};

impl<R: Runtime> ContextEventSink for AppHandle<R> {
    fn emit(&self, payload: ContextUsagePayload) {
        let _ = tauri::Emitter::emit(self, "session_context", payload);
    }
}

impl<R: Runtime> QuotaEventSink for AppHandle<R> {
    fn emit(&self, payload: AgentQuotaPayload) {
        let _ = tauri::Emitter::emit(self, "agent_quota", payload);
    }
}

impl<R: Runtime> WatchersScopeEvents for AppHandle<R> {
    fn request_watchers_scope(&self, session_id: uuid::Uuid) -> Result<(), String> {
        self.emit_to(
            commands::window::WATCHERS_WINDOW_LABEL,
            "watchers_scope_request",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .map_err(|e| e.to_string())
    }
}

impl WatcherDelivery {
    pub(crate) fn to_watchers_window(app_handle: AppHandle) -> Self {
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
}
