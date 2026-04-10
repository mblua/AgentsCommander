use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::telegram::manager::TelegramBridgeState;
use crate::telegram::types::BridgeInfo;

#[tauri::command]
pub async fn telegram_attach(
    app: AppHandle,
    tg_mgr: State<'_, TelegramBridgeState>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    settings: State<'_, SettingsState>,
    session_id: String,
    bot_id: String,
) -> Result<BridgeInfo, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

    // Look up session to check is_claude flag for JSONL watcher mode
    let jsonl_cwd = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let session = mgr.get_session(uuid).await.ok_or("Session not found")?;
        if session.is_claude {
            Some(session.working_directory.clone())
        } else {
            None
        }
    };

    let cfg = settings.read().await;
    let bot = cfg
        .telegram_bots
        .iter()
        .find(|b| b.id == bot_id)
        .ok_or_else(|| format!("Bot not found: {}", bot_id))?
        .clone();
    drop(cfg);

    let pty_arc = pty_mgr.inner().clone();
    let mut tg = tg_mgr.lock().await;
    let info = tg
        .attach(uuid, &bot, pty_arc, app.clone(), jsonl_cwd)
        .map_err(|e| e.to_string())?;

    let _ = app.emit("telegram_bridge_attached", info.clone());

    Ok(info)
}

#[tauri::command]
pub async fn telegram_detach(
    app: AppHandle,
    tg_mgr: State<'_, TelegramBridgeState>,
    session_id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

    let mut tg = tg_mgr.lock().await;
    tg.detach(uuid).map_err(|e| e.to_string())?;

    let _ = app.emit(
        "telegram_bridge_detached",
        serde_json::json!({ "sessionId": session_id }),
    );

    Ok(())
}

#[tauri::command]
pub async fn telegram_list_bridges(
    tg_mgr: State<'_, TelegramBridgeState>,
) -> Result<Vec<BridgeInfo>, String> {
    let tg = tg_mgr.lock().await;
    Ok(tg.list_bridges())
}

#[tauri::command]
pub async fn telegram_get_bridge(
    tg_mgr: State<'_, TelegramBridgeState>,
    session_id: String,
) -> Result<Option<BridgeInfo>, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let tg = tg_mgr.lock().await;
    Ok(tg.get_bridge(uuid))
}

/// Test bot connection: discovers chat_id from the latest message sent to the bot,
/// sends a confirmation message back, and returns the discovered chat_id.
/// The user just needs to send any message to the bot before clicking Test.
#[tauri::command]
pub async fn telegram_send_test(token: String) -> Result<i64, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    // Fetch recent updates to discover chat_id
    let updates = crate::telegram::api::get_updates(&client, &token, 0, 0)
        .await
        .map_err(|e| e.to_string())?;

    let chat_id = updates
        .last()
        .map(|u| u.chat_id)
        .ok_or_else(|| "No messages found. Send any message to your bot in Telegram first, then click Test again.".to_string())?;

    crate::telegram::api::send_message(
        &client,
        &token,
        chat_id,
        "agentscommander connected",
    )
    .await
    .map_err(|e| e.to_string())?;

    Ok(chat_id)
}
