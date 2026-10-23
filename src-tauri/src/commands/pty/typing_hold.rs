use super::response_close_hold_publisher;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

pub(super) fn record_typing_hold_keystroke<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    data: &[u8],
) {
    if !crate::pty::input_activity::chunk_qualifies_typing_hold(data) {
        return;
    }
    let window = app
        .try_state::<crate::config::settings::SettingsState>()
        .and_then(|settings| {
            settings
                .try_current_read()
                .inspect_err(|error| {
                    log::debug!("[typing-hold] typing window unavailable: {error}");
                })
                .ok()
                .map(|settings| {
                    Duration::from_secs(u64::from(settings.typing_hold_seconds.clamp(
                        crate::config::settings::TYPING_HOLD_SECONDS_MIN,
                        crate::config::settings::TYPING_HOLD_SECONDS_MAX,
                    )))
                })
        });
    if let Some(state) = app.try_state::<crate::pty::input_activity::TypingHoldState>() {
        let mut hold = state.lock().unwrap_or_else(|error| error.into_inner());
        hold.note_qualifying_key(session_id);
        if let Some(publisher) = response_close_hold_publisher(app) {
            publisher.publish_typing_key(session_id, window, std::time::Instant::now());
        }
    }
}

/// #2336 - the live natural-hold window. Read from settings on every evaluation
/// so an edit takes effect without a restart; clamped because the Settings API
/// rejects out-of-range values but a hand-edited settings.json still loads.
pub(super) async fn typing_hold_window<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<Duration, String> {
    let settings = app
        .try_state::<crate::config::settings::SettingsState>()
        .ok_or_else(|| "settings owner unavailable (missing state)".to_string())?;
    let seconds = settings
        .current_read()
        .await
        .map_err(|error| error.to_string())?
        .typing_hold_seconds
        .clamp(
            crate::config::settings::TYPING_HOLD_SECONDS_MIN,
            crate::config::settings::TYPING_HOLD_SECONDS_MAX,
        );
    Ok(Duration::from_secs(u64::from(seconds)))
}

/// #2336 - pure in-memory padlock snapshot for one session. An absent state or
/// session reads as open with no held ids; no lock is held across an await.
pub(super) fn typing_hold_snapshot<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    window: Duration,
) -> crate::pty::input_activity::TypingHoldSnapshot {
    match app.try_state::<crate::pty::input_activity::TypingHoldState>() {
        Some(state) => {
            let hold = state.lock().unwrap_or_else(|error| error.into_inner());
            let snapshot = hold.snapshot(session_id, window);
            if let Some(publisher) = response_close_hold_publisher(app) {
                publisher.refresh_hold_snapshot(
                    session_id,
                    window,
                    snapshot.closed,
                    std::time::Instant::now(),
                );
            }
            snapshot
        }
        None => crate::pty::input_activity::TypingHoldSnapshot {
            closed: false,
            held_count: 0,
        },
    }
}

/// #2336 - evaluated inside `pty::inject` at the serialized write boundary of a
/// peer wake. Read-only, so evaluating it never consumes the hold.
pub(crate) async fn typing_hold_defers_injection<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> bool {
    let window = match typing_hold_window(app).await {
        Ok(window) => window,
        Err(error) => {
            log::debug!("[typing-hold] deferring injection: {error}");
            return true;
        }
    };
    typing_hold_snapshot(app, session_id, window).closed
}

/// #2336 - count a peer wake message ID as deferred, once (the set dedupes, so
/// a retried poll of the same message never increases the count).
pub(crate) fn note_held_wake<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    message_id: &str,
) {
    if let Some(state) = app.try_state::<crate::pty::input_activity::TypingHoldState>() {
        let mut hold = state.lock().unwrap_or_else(|error| error.into_inner());
        hold.record_held_message(session_id, message_id);
        if let Some(publisher) = response_close_hold_publisher(app) {
            publisher.publish_held_key(session_id, message_id, true, std::time::Instant::now());
        }
    }
}

/// #2336 - drop a held message ID after an observed delivery.
pub(crate) fn clear_held_wake<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    message_id: &str,
) {
    if let Some(state) = app.try_state::<crate::pty::input_activity::TypingHoldState>() {
        let mut hold = state.lock().unwrap_or_else(|error| error.into_inner());
        hold.clear_held_message(session_id, message_id);
        if let Some(publisher) = response_close_hold_publisher(app) {
            publisher.publish_held_key(session_id, message_id, false, std::time::Instant::now());
        }
    }
}
