use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
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
        .attach(uuid, &bot, pty_arc, app.clone())
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

#[tauri::command]
pub async fn telegram_send_test(token: String, chat_id: i64) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    crate::telegram::api::send_message(
        &client,
        &token,
        chat_id,
        "summongate test connection OK",
    )
    .await
    .map_err(|e| e.to_string())
}
