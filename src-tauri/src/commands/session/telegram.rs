use crate::config::agent_config;
use crate::config::sessions_persistence::persist_current_state;
use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use crate::telegram::manager::TelegramBridgeState;
use std::sync::Arc;
use tauri::AppHandle;
use tauri::Manager;
use uuid::Uuid;

pub(crate) async fn attach_persisted_telegram_if_configured<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    bot_id: Option<&str>,
) {
    let Some(bot_id) = bot_id else {
        return;
    };

    let settings = app.state::<SettingsState>();
    let exists = {
        let cfg = match settings.current_read().await {
            Ok(cfg) => cfg,
            Err(error) => {
                log::debug!("[telegram] skipping association: {error}");
                return;
            }
        };
        cfg.telegram_bots.iter().any(|b| b.id == bot_id)
    };

    if !exists {
        log::warn!(
            "[telegram] Persisted bot '{}' for session {} no longer exists in settings; leaving \
             bridge OFF",
            bot_id,
            session_id
        );
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_telegram_bot_id(session_id, None).await;
        persist_current_state(&mgr).await;
        return;
    }

    let already_attached = {
        let tg_mgr = app.state::<TelegramBridgeState>();
        let tg = tg_mgr.lock().await;
        tg.get_bridge(session_id)
    };
    if let Some(info) = already_attached {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        if info.bot_id == bot_id {
            mgr.set_telegram_bot_id(session_id, Some(bot_id.to_string()))
                .await;
        } else {
            log::warn!(
                "[telegram] Session {} already has Telegram bot '{}' attached; persisted bot '{}' \
                 will be cleared",
                session_id,
                info.bot_id,
                bot_id
            );
            mgr.set_telegram_bot_id(session_id, None).await;
        }
        persist_current_state(&mgr).await;
        return;
    }

    if let Err(e) =
        crate::commands::telegram::attach_telegram_bot_by_id(app, session_id, bot_id).await
    {
        log::warn!(
            "[telegram] Failed to restore persisted bot '{}' for session {}: {}",
            bot_id,
            session_id,
            e
        );
        // Availability failure during attach must not erase the persisted intent.
        if let Err(error) = settings.current_read().await {
            log::debug!("[telegram] preserving association intent: {error}");
            return;
        }
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        mgr.set_telegram_bot_id(session_id, None).await;
        persist_current_state(&mgr).await;
    }
}

pub(crate) async fn preserve_deferred_telegram_intent_if_valid(
    mgr: &SessionManager,
    settings: &SettingsState,
    session_id: Uuid,
    session_name: &str,
    bot_id: Option<&str>,
) {
    let Some(bot_id) = bot_id else {
        return;
    };

    let exists = {
        let cfg = match settings.current_read().await {
            Ok(cfg) => cfg,
            Err(error) => {
                log::debug!("[telegram] skipping association: {error}");
                return;
            }
        };
        cfg.telegram_bots.iter().any(|b| b.id == bot_id)
    };

    if exists {
        mgr.set_telegram_bot_id(session_id, Some(bot_id.to_string()))
            .await;
    } else {
        log::warn!(
            "[telegram] Persisted bot '{}' for deferred session '{}' no longer exists in settings; \
             leaving bridge OFF",
            bot_id,
            session_name
        );
        mgr.set_telegram_bot_id(session_id, None).await;
    }
}

pub(crate) async fn attach_local_config_telegram_if_any<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    cwd: &str,
) {
    let config_dir = std::path::Path::new(cwd).join(crate::config::agent_local_dir_name());

    // #2786 (C1) - through the shared loader, off the async worker: the
    // loader does blocking file reads.
    let Some(bot_label) =
        tokio::task::spawn_blocking(move || agent_config::read_agent_local_config(&config_dir))
            .await
            .ok()
            .flatten()
            .and_then(|local_config| local_config.tooling.telegram_bot)
    else {
        return;
    };

    let settings = app.state::<SettingsState>();
    let bot_id = {
        let cfg = match settings.current_read().await {
            Ok(cfg) => cfg,
            Err(error) => {
                log::debug!("[telegram] skipping association: {error}");
                return;
            }
        };
        cfg.telegram_bots
            .iter()
            .find(|b| b.label == bot_label)
            .map(|b| b.id.clone())
    };

    match bot_id {
        Some(bot_id) => {
            if let Err(e) =
                crate::commands::telegram::attach_telegram_bot_by_id(app, session_id, &bot_id).await
            {
                log::warn!(
                    "[telegram] Failed to auto-attach configured bot '{}' for session {}: {}",
                    bot_label,
                    session_id,
                    e
                );
            }
        }
        None => {
            log::warn!(
                "[telegram] Configured bot label '{}' for session {} no longer exists in settings; \
                 leaving bridge OFF",
                bot_label,
                session_id
            );
        }
    }
}
