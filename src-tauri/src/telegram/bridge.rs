use std::sync::{Arc, Mutex};

use tauri::Emitter;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::telegram::api;
use crate::telegram::types::BridgeInfo;

pub struct BridgeHandle {
    pub info: BridgeInfo,
    pub cancel: CancellationToken,
    pub output_sender: mpsc::Sender<Vec<u8>>,
}

pub fn spawn_bridge(
    bot_token: String,
    chat_id: i64,
    session_id: Uuid,
    info: BridgeInfo,
    pty_mgr: Arc<Mutex<PtyManager>>,
    app_handle: tauri::AppHandle,
) -> BridgeHandle {
    let cancel = CancellationToken::new();
    let (tx, rx) = mpsc::channel::<Vec<u8>>(256);

    let session_id_str = session_id.to_string();

    // Output task: PTY bytes → strip ANSI → buffer → Telegram sendMessage
    tokio::spawn(output_task(
        rx,
        bot_token.clone(),
        chat_id,
        session_id_str.clone(),
        cancel.clone(),
        app_handle.clone(),
    ));

    // Poll task: Telegram getUpdates → write to PTY stdin
    tokio::spawn(poll_task(
        bot_token,
        chat_id,
        session_id,
        session_id_str,
        pty_mgr,
        cancel.clone(),
        app_handle,
    ));

    BridgeHandle {
        info,
        cancel,
        output_sender: tx,
    }
}

async fn output_task(
    mut rx: mpsc::Receiver<Vec<u8>>,
    token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle,
) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let mut buffer = String::new();
    let far_future = tokio::time::Duration::from_secs(86400);
    let flush_timeout = tokio::time::Duration::from_millis(500);
    let mut deadline = tokio::time::Instant::now() + far_future;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep_until(deadline) => {
                if !buffer.is_empty() {
                    flush_buffer(&mut buffer, &client, &token, chat_id, &session_id, &app).await;
                }
                deadline = tokio::time::Instant::now() + far_future;
            }
            maybe_data = rx.recv() => {
                match maybe_data {
                    Some(data) => {
                        let stripped = strip_ansi_escapes::strip(&data);
                        let text = String::from_utf8_lossy(&stripped);
                        buffer.push_str(&text);
                        deadline = tokio::time::Instant::now() + flush_timeout;

                        if buffer.contains('\n') || buffer.len() > 2000 {
                            flush_buffer(&mut buffer, &client, &token, chat_id, &session_id, &app).await;
                            deadline = tokio::time::Instant::now() + far_future;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    // Final flush
    if !buffer.is_empty() {
        flush_buffer(&mut buffer, &client, &token, chat_id, &session_id, &app).await;
    }
}

async fn flush_buffer(
    buffer: &mut String,
    client: &reqwest::Client,
    token: &str,
    chat_id: i64,
    session_id: &str,
    app: &tauri::AppHandle,
) {
    let text = std::mem::take(buffer);
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }

    for chunk in chunk_text(&text, 4000) {
        if let Err(e) = api::send_message(client, token, chat_id, &chunk).await {
            log::error!("Telegram send error for session {}: {}", session_id, e);
            let _ = app.emit(
                "telegram_bridge_error",
                serde_json::json!({
                    "sessionId": session_id,
                    "error": e.to_string(),
                }),
            );
        }
        // Rate limit: 35ms between sends
        tokio::time::sleep(tokio::time::Duration::from_millis(35)).await;
    }
}

fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + max_len).min(text.len());
        let actual_end = if end < text.len() {
            text[start..end]
                .rfind('\n')
                .map(|i| start + i + 1)
                .unwrap_or(end)
        } else {
            end
        };
        chunks.push(text[start..actual_end].to_string());
        start = actual_end;
    }
    chunks
}

async fn poll_task(
    token: String,
    chat_id: i64,
    session_id: Uuid,
    session_id_str: String,
    pty_mgr: Arc<Mutex<PtyManager>>,
    cancel: CancellationToken,
    app: tauri::AppHandle,
) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    let mut offset: i64 = 0;

    // Skip old messages
    match api::get_updates(&client, &token, 0, 0).await {
        Ok(updates) => {
            if let Some(last) = updates.last() {
                offset = last.update_id + 1;
            }
        }
        Err(e) => {
            log::warn!("Initial getUpdates failed: {}", e);
        }
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = api::get_updates(&client, &token, offset, 5) => {
                match result {
                    Ok(updates) => {
                        for update in updates {
                            offset = update.update_id + 1;

                            // Only process messages from the target chat
                            if update.chat_id != chat_id {
                                continue;
                            }

                            // Write to PTY stdin
                            let input = format!("{}\n", update.text);
                            if let Ok(mgr) = pty_mgr.lock() {
                                if let Err(e) = mgr.write(session_id, input.as_bytes()) {
                                    log::error!("Failed to write Telegram input to PTY: {}", e);
                                }
                            }

                            // Emit event for UI
                            let _ = app.emit(
                                "telegram_incoming",
                                serde_json::json!({
                                    "sessionId": session_id_str,
                                    "text": update.text,
                                    "from": update.from_name,
                                }),
                            );
                        }
                    }
                    Err(e) => {
                        log::error!("Telegram poll error: {}", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                    }
                }
            }
        }
    }
}
