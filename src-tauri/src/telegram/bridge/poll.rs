use super::PollNetworkErrorState;
use super::PollNetworkLogAction;
use super::{BridgeLogger, TelegramErrKind};
use crate::network::NetworkBackoff;
use crate::network::OutboundNetwork;
use crate::pty::inject::inject_text_into_session;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::telegram::api;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
use tokio::sync::RwLock;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ── Poll task (Telegram -> PTY) ──────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(super) async fn poll_task<R: tauri::Runtime>(
    token: String,
    chat_id: i64,
    session_id: Uuid,
    session_id_str: String,
    _pty_mgr: Arc<Mutex<PtyManager>>,
    network: OutboundNetwork,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
) {
    let mut logger = BridgeLogger::new(&session_id_str);
    let mut offset: i64 = 0;
    let network_error_policy = {
        let settings = app.state::<crate::config::settings::SettingsState>();
        let cfg = settings.read().await;
        cfg.telegram_network_poll_error_logging.clone()
    };
    let mut network_error_state = PollNetworkErrorState::new();
    let mut poll_backoff = new_poll_backoff(&session_id_str);

    // Skip old messages
    match api::get_updates(&network, &token, 0, 0).await {
        Ok(updates) => {
            poll_backoff.reset();
            if let Some(last) = updates.last() {
                offset = last.update_id + 1;
                logger.log(
                    "POLL_INIT",
                    &session_id_str,
                    &format!("skipped {} old messages, offset={}", updates.len(), offset),
                );
            }
        }
        Err(e) => {
            let msg = crate::telegram::redact::redact(&e.to_string());
            let kind = TelegramErrKind::classify(&msg);

            if matches!(kind, TelegramErrKind::Network) {
                match network_error_state.record_network_failure(&network_error_policy) {
                    PollNetworkLogAction::Failure {
                        level,
                        suppressed_since_last_emit,
                        sustained,
                    } => {
                        logger.log("POLL_ERR", &session_id_str, &msg);
                        let suffix = if suppressed_since_last_emit == 0 {
                            String::new()
                        } else {
                            format!(" suppressed_repeats={}", suppressed_since_last_emit)
                        };
                        log::log!(
                            level,
                            "[bridge] Initial getUpdates network error kind={} session_id={} \
                            sustained={} err={}{}",
                            kind.as_str(),
                            session_id_str,
                            sustained,
                            msg,
                            suffix
                        );
                    }
                    PollNetworkLogAction::Suppressed { level } => {
                        logger.log(
                            "POLL_ERR_SUPPRESSED",
                            &session_id_str,
                            &format!("kind={} phase=initial", kind.as_str()),
                        );
                        log::log!(
                            level,
                            "[bridge] Initial getUpdates network error suppressed kind={} \
                            session_id={}",
                            kind.as_str(),
                            session_id_str
                        );
                    }
                    PollNetworkLogAction::Recovery { .. } | PollNetworkLogAction::None => {
                        unreachable!("record_network_failure only returns Failure or Suppressed")
                    }
                }
            } else {
                logger.log("POLL_ERR", &session_id_str, &msg);
                log::error!(
                    "[bridge] Initial getUpdates failed kind={} session_id={} err={}",
                    kind.as_str(),
                    session_id_str,
                    msg
                );
            }
            if !sleep_backoff_or_cancel(&cancel, &mut poll_backoff, &mut logger, &session_id_str)
                .await
            {
                return;
            }
        }
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = api::get_updates(&network, &token, offset, 5) => {
                match result {
                    Ok(updates) => {
                        poll_backoff.reset();
                        match network_error_state.record_success(&network_error_policy) {
                            PollNetworkLogAction::Recovery {
                                level,
                                outage_seconds,
                                suppressed_total,
                                sustained,
                            } => {
                                logger.log(
                                    "POLL_RECOVERY",
                                    &session_id_str,
                                    &format!(
                                        "outage_seconds={} suppressed_total={} sustained={}",
                                        outage_seconds, suppressed_total, sustained
                                    ),
                                );
                                log::log!(
                                    level,
                                    "[bridge] Telegram poll recovered session_id={} \
                                    outage_seconds={} suppressed_total={} sustained={}",
                                    session_id_str,
                                    outage_seconds,
                                    suppressed_total,
                                    sustained
                                );
                            }
                            PollNetworkLogAction::None => {}
                            PollNetworkLogAction::Failure { .. }
                            | PollNetworkLogAction::Suppressed { .. } => {
                                unreachable!("record_success only returns Recovery or None")
                            }
                        }
                        for update in updates {
                            offset = update.update_id + 1;

                            if update.chat_id != chat_id {
                                logger.log(
                                    "POLL_SKIP",
                                    &session_id_str,
                                    &format!("wrong chat_id={}", update.chat_id),
                                );
                                continue;
                            }

                            let inject_text = match update.content {
                                api::TelegramContent::Text(text) => {
                                    logger.log(
                                        "RECV_TG",
                                        &session_id_str,
                                        &format!("from={} text={}", update.from_name, text),
                                    );
                                    text
                                }
                                api::TelegramContent::Voice { file_id } => {
                                    match transcribe_voice_update(
                                        &app,
                                        &network,
                                        &token,
                                        chat_id,
                                        &session_id_str,
                                        &mut logger,
                                        &update.from_name,
                                        &file_id,
                                    )
                                    .await
                                    {
                                        Some(text) => text,
                                        None => continue,
                                    }
                                }
                            };

                            if let Err(e) =
                                inject_text_into_session(&app, session_id, &inject_text)
                                    .await
                            {
                                logger.log("PTY_ERR", &session_id_str, &e.to_string());
                                log::error!("Failed to write Telegram input to PTY: {}", e);
                            }

                            // #552 a Telegram message is a real user message to this
                            // coordinator: reset the badge clock + auto-close silence
                            // (covers text and transcribed-voice inbound; both converge here).
                            crate::commands::pty::note_user_message_to_session(
                                &app,
                                session_id,
                                crate::commands::pty::UserInputSource::CompleteMessage,
                            )
                            .await;

                            let _ = app.emit(
                                "telegram_incoming",
                                serde_json::json!({
                                    "sessionId": session_id_str,
                                    "text": inject_text,
                                    "from": update.from_name,
                                }),
                            );

                            let tg_prompt = format!("[TG] {}", inject_text);
                            {
                                let mgr_state = app.state::<Arc<RwLock<SessionManager>>>();
                                let mgr = mgr_state.read().await;
                                if let Ok(uuid) = uuid::Uuid::parse_str(&session_id_str) {
                                    mgr.set_last_prompt(uuid, tg_prompt.clone()).await;
                                }
                            }
                            let _ = app.emit(
                                "last_prompt",
                                serde_json::json!({
                                    "text": tg_prompt,
                                    "sessionId": session_id_str,
                                }),
                            );
                        }
                    }
                    Err(e) => {
                        let msg = crate::telegram::redact::redact(&e.to_string());
                        let kind = TelegramErrKind::classify(&msg);
                        let pid = std::process::id();
                        let token_prefix = token.split(':').next().unwrap_or("?");

                        if !matches!(kind, TelegramErrKind::Network) {
                            network_error_state.reset_without_recovery();
                            logger.log("POLL_ERR", &session_id_str, &msg);
                            log::error!(
                                "[bridge] Telegram poll error kind={} session_id={} pid={} \
                                bot_id={} err={}",
                                kind.as_str(),
                                session_id_str,
                                pid,
                                token_prefix,
                                msg
                            );
                            if !sleep_backoff_or_cancel(
                                &cancel,
                                &mut poll_backoff,
                                &mut logger,
                                &session_id_str,
                            )
                            .await
                            {
                                break;
                            }
                            continue;
                        }

                        match network_error_state.record_network_failure(&network_error_policy) {
                            PollNetworkLogAction::Failure {
                                level,
                                suppressed_since_last_emit,
                                sustained,
                            } => {
                                logger.log("POLL_ERR", &session_id_str, &msg);
                                let suffix = if suppressed_since_last_emit == 0 {
                                    String::new()
                                } else {
                                    format!(" suppressed_repeats={}", suppressed_since_last_emit)
                                };
                                log::log!(
                                    level,
                                    "[bridge] Telegram poll network error kind={} session_id={} \
                                    pid={} bot_id={} sustained={} err={}{}",
                                    kind.as_str(),
                                    session_id_str,
                                    pid,
                                    token_prefix,
                                    sustained,
                                    msg,
                                    suffix
                                );
                            }
                            PollNetworkLogAction::Suppressed { level } => {
                                logger.log(
                                    "POLL_ERR_SUPPRESSED",
                                    &session_id_str,
                                    &format!("kind={} phase=poll", kind.as_str()),
                                );
                                log::log!(
                                    level,
                                    "[bridge] Telegram poll network error suppressed kind={} \
                                    session_id={}",
                                    kind.as_str(),
                                    session_id_str
                                );
                            }
                            PollNetworkLogAction::Recovery { .. } | PollNetworkLogAction::None => {
                                unreachable!("record_network_failure only returns Failure or \
                                Suppressed")
                            }
                        }
                        if !sleep_backoff_or_cancel(
                            &cancel,
                            &mut poll_backoff,
                            &mut logger,
                            &session_id_str,
                        )
                        .await
                        {
                            break;
                        }
                    }
                }
            }
        }
    }
}

async fn sleep_backoff_or_cancel(
    cancel: &CancellationToken,
    backoff: &mut NetworkBackoff,
    logger: &mut BridgeLogger,
    session_id: &str,
) -> bool {
    let delay = backoff.next_delay();
    let sleep_ms = delay.as_millis();
    logger.log("POLL_BACKOFF", session_id, &format!("sleep_ms={sleep_ms}"));
    log::debug!(
        "[bridge] POLL_BACKOFF session_id={} sleep_ms={}",
        session_id,
        sleep_ms
    );
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

pub(super) fn new_poll_backoff(session_id: &str) -> NetworkBackoff {
    NetworkBackoff::new(
        Duration::from_secs(3),
        Duration::from_secs(60),
        NetworkBackoff::salt_from_str(session_id),
    )
}

/// Transcribes one inbound Telegram voice message. Returns `None` when the
/// update must be skipped (no API key, download or transcription failure).
#[allow(clippy::too_many_arguments)]
async fn transcribe_voice_update<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    network: &OutboundNetwork,
    token: &str,
    chat_id: i64,
    session_id_str: &str,
    logger: &mut BridgeLogger,
    from_name: &str,
    file_id: &str,
) -> Option<String> {
    logger.log(
        "RECV_TG_VOICE",
        session_id_str,
        &format!("from={} file_id={}", from_name, file_id),
    );

    let settings = app.state::<crate::config::settings::SettingsState>();
    let cfg = settings.read().await;
    let api_key = cfg.gemini_api_key.clone();
    let model_raw = cfg.gemini_model.clone();
    drop(cfg);

    let model = if model_raw.is_empty() {
        "gemini-2.5-flash".to_string()
    } else {
        model_raw
    };

    if api_key.is_empty() {
        log::warn!("[bridge] Voice message received but no Gemini API key configured");
        let _ = api::send_message(
            network,
            token,
            chat_id,
            "Cannot transcribe voice: Gemini API key not configured",
        )
        .await;
        return None;
    }

    let file_path = match api::get_file(network, token, file_id).await {
        Ok(fp) => fp,
        Err(e) => {
            logger.log(
                "VOICE_ERR",
                session_id_str,
                &format!("get_file failed: {}", e),
            );
            let _ = api::send_message(
                network,
                token,
                chat_id,
                &format!("Failed to get voice file: {}", e),
            )
            .await;
            return None;
        }
    };

    let audio_bytes = match api::download_file(network, token, &file_path).await {
        Ok(bytes) => bytes,
        Err(e) => {
            logger.log(
                "VOICE_ERR",
                session_id_str,
                &format!("download failed: {}", e),
            );
            let _ = api::send_message(
                network,
                token,
                chat_id,
                &format!("Failed to download voice: {}", e),
            )
            .await;
            return None;
        }
    };

    match crate::commands::voice::transcribe_audio_with_network(
        network,
        &audio_bytes,
        "audio/ogg",
        &api_key,
        &model,
    )
    .await
    {
        Ok(text) => {
            logger.log(
                "VOICE_OK",
                session_id_str,
                &format!("transcribed {} chars", text.len()),
            );
            let _ =
                api::send_message(network, token, chat_id, &format!("Transcribed: {}", text)).await;
            Some(text)
        }
        Err(e) => {
            logger.log(
                "VOICE_ERR",
                session_id_str,
                &format!("transcription failed: {}", e),
            );
            let _ = api::send_message(
                network,
                token,
                chat_id,
                &format!("Transcription failed: {}", e),
            )
            .await;
            None
        }
    }
}
