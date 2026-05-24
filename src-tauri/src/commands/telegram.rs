use std::path::Path;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::telegram::bridge::SessionReaderKind;
use crate::telegram::manager::TelegramBridgeState;
use crate::telegram::types::BridgeInfo;
use crate::session::profile::CodingAgentKind;

/// Derive which session-reader pipeline to spawn for a given session.
///
/// - `Ok(Some(kind))` — agent detected and resolver succeeded → caller spawns
///   the reader.
/// - `Ok(None)` — `agent_kind` is `None` (plain shell) → caller falls back to
///   PTY mode.
/// - `Err(message)` — agent detected but resolver returned None → caller logs +
///   emits `telegram_bridge_error` + early-returns with its contractual success
///   value (or `Err` for `telegram_attach`).
///
/// #260: agent selection is `Option<CodingAgentKind>`. Mutual exclusion is now
/// structural (an enum is one variant or none), so the pre-#260
/// `debug_assert!(kinds_set <= 1, …)` guard was removed.
pub(crate) fn derive_reader(
    shell: &str,
    shell_args: &[String],
    cwd: &str,
    agent_kind: Option<CodingAgentKind>,
) -> Result<Option<SessionReaderKind>, String> {
    let attach_time = chrono::Utc::now();

    match agent_kind {
        Some(CodingAgentKind::Claude) => {
            match crate::commands::session::resolve_claude_projects_dir(shell, shell_args, cwd) {
                Some(p) => Ok(Some(SessionReaderKind::Claude { project_dir: p })),
                None => Err("Cannot resolve Claude projects dir".to_string()),
            }
        }
        Some(CodingAgentKind::Codex) => {
            match crate::commands::codex_resolver::resolve_codex_sessions_root(
                shell, shell_args, cwd,
            ) {
                Some(root) => Ok(Some(SessionReaderKind::Codex {
                    search_root: root,
                    cwd: cwd.to_string(),
                    attach_time,
                })),
                None => Err(
                    "Cannot resolve Codex sessions root (~/.codex/sessions/ missing)".to_string(),
                ),
            }
        }
        Some(CodingAgentKind::Gemini) => {
            // H1 softened contract: spawn the watcher whenever `~/.gemini/`
            // exists; the cwd-to-slug lookup is deferred to the watcher's
            // per-poll `lookup_chats_dir_for_cwd`. Loud abort only if Gemini
            // was never installed on this machine.
            match crate::commands::gemini_resolver::resolve_gemini_home(shell, shell_args, cwd) {
                Some(home) => Ok(Some(SessionReaderKind::Gemini {
                    gemini_home: home,
                    cwd: cwd.to_string(),
                    attach_time,
                })),
                None => Err(
                    "Cannot resolve Gemini home (~/.gemini/ missing — Gemini never installed)"
                        .to_string(),
                ),
            }
        }
        None => Ok(None), // No agent detected — caller falls back to PTY mode.
    }
}

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

    // Extract the fields the resolver needs and drop the SessionManager read guard
    // BEFORE invoking `derive_reader` — the resolver does blocking filesystem I/O
    // (`which::which` walks `%PATH%`, opens wrapper scripts) that can take hundreds
    // of milliseconds. Holding a `tokio::sync::RwLock` read guard across that would
    // starve concurrent writers (create_session, restart_session, switch_session).
    let (agent_kind, shell, shell_args, working_directory) = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let session = mgr.get_session(uuid).await.ok_or("Session not found")?;
        (
            session.agent_kind,
            session.shell.clone(),
            session.shell_args.clone(),
            session.working_directory.clone(),
        )
    };

    let reader = match derive_reader(&shell, &shell_args, &working_directory, agent_kind) {
        Ok(r) => r,
        Err(reason) => {
            let err_msg = format!(
                "Telegram bridge: {} for session {} (shell={:?}). Bridge inactive.",
                reason, uuid, shell
            );
            log::error!("{}", err_msg);
            let _ = app.emit(
                "telegram_bridge_error",
                serde_json::json!({
                    "sessionId": session_id,
                    "error": err_msg,
                }),
            );
            return Err(err_msg);
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
        .attach(uuid, &bot, pty_arc, app.clone(), reader)
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

    crate::telegram::api::send_message(&client, &token, chat_id, "agentscommander connected")
        .await
        .map_err(|e| e.to_string())?;

    Ok(chat_id)
}

/// Telegram `sendPhoto` upper bound. Files at or below this size with a
/// supported image extension take the inline-photo path; anything larger
/// falls back to `sendDocument`.
const SEND_PHOTO_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Hard cap for `telegram_send_image`. Telegram Bot API allows
/// `sendDocument` up to 50 MB without the local-bot-API server.
const SEND_DOCUMENT_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// Telegram caption hard cap. The Bot API counts UTF-16 code units, not
/// Rust chars or UTF-8 bytes; non-BMP chars (emoji) encode to 2 units, so a
/// char-count truncation can produce up to 2x the limit and the server
/// returns `Bad Request: caption is too long`.
const CAPTION_MAX_UTF16_UNITS: usize = 1024;

/// Extensions Telegram renders inline via `sendPhoto`. GIF is intentionally
/// excluded because `sendPhoto` strips animation.
const PHOTO_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Photo,
    Document,
}

/// Deterministic endpoint selection. Photo path requires BOTH size <= 10 MB
/// AND a known photo extension; everything else routes to document.
/// `ext` must already be lowercased by the caller.
pub(crate) fn choose_endpoint(size: u64, ext: &str) -> Endpoint {
    if size <= SEND_PHOTO_MAX_BYTES && PHOTO_EXTENSIONS.contains(&ext) {
        Endpoint::Photo
    } else {
        Endpoint::Document
    }
}

/// Map a (lowercased) file extension to the explicit `Content-Type` we pass
/// to Telegram. Unknown extensions fall back to `application/octet-stream`.
pub(crate) fn extension_to_mime(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

/// Trim leading/trailing whitespace, then truncate to Telegram's 1024
/// UTF-16-code-unit cap. If the cut lands inside a surrogate pair, drop the
/// dangling high surrogate so `String::from_utf16_lossy` does not emit
/// `U+FFFD`.
pub(crate) fn truncate_caption(input: &str) -> String {
    let t = input.trim();
    let units: Vec<u16> = t.encode_utf16().collect();
    if units.len() <= CAPTION_MAX_UTF16_UNITS {
        return t.to_string();
    }
    let mut cut = units;
    cut.truncate(CAPTION_MAX_UTF16_UNITS);
    if matches!(cut.last(), Some(&u) if (0xD800..=0xDBFF).contains(&u)) {
        cut.pop();
    }
    String::from_utf16_lossy(&cut)
}

/// Send a local image (or generic file) through a configured Telegram bot.
///
/// v1 contract:
///   - `path` must point to an existing regular file; symlinks are rejected.
///   - Files <= 10 MB with extension in `PHOTO_EXTENSIONS` use `sendPhoto`.
///   - Everything else uses `sendDocument`, up to a 50 MB hard cap enforced
///     both at the metadata check and at the read/allocation boundary.
///   - `caption` is trimmed and clamped to `CAPTION_MAX_UTF16_UNITS` UTF-16
///     code units (Telegram's true unit). Empty captions are dropped.
#[tauri::command]
pub async fn telegram_send_image(
    settings: State<'_, SettingsState>,
    bot_id: String,
    path: String,
    caption: Option<String>,
) -> Result<(), String> {
    let cfg = settings.read().await;
    let bot = cfg
        .telegram_bots
        .iter()
        .find(|b| b.id == bot_id)
        .ok_or_else(|| format!("Bot not found: {}", bot_id))?
        .clone();
    drop(cfg);

    let p = Path::new(&path);
    let lmeta = tokio::fs::symlink_metadata(p)
        .await
        .map_err(|e| format!("stat failed: {}", e))?;
    if lmeta.file_type().is_symlink() {
        return Err(format!("Symlinks are not supported in v1: {}", path));
    }
    if !lmeta.is_file() {
        return Err(format!("Not a regular file: {}", path));
    }
    let size = lmeta.len();
    if size == 0 {
        return Err(format!("File is empty: {}", path));
    }
    if size > SEND_DOCUMENT_MAX_BYTES {
        return Err(format!(
            "File exceeds Telegram 50 MB limit ({} bytes): {}",
            size, path
        ));
    }

    let f = tokio::fs::File::open(p)
        .await
        .map_err(|e| format!("open failed: {}", e))?;
    let mut bytes: Vec<u8> = Vec::with_capacity(size as usize);
    f.take(SEND_DOCUMENT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| format!("read failed: {}", e))?;
    if bytes.len() as u64 > SEND_DOCUMENT_MAX_BYTES {
        return Err(format!("File grew past 50 MB during read: {}", path));
    }

    let filename = match p.file_name().and_then(|s| s.to_str()) {
        Some(name) => name.to_string(),
        None => {
            log::warn!(
                "telegram_send_image: non-UTF8 filename for {}, sending as 'image'",
                path
            );
            "image".to_string()
        }
    };

    let caption_trimmed: Option<String> = caption.as_deref().map(truncate_caption);
    let caption_ref: Option<&str> = caption_trimmed.as_deref().filter(|s| !s.is_empty());

    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let endpoint = choose_endpoint(size, &ext);
    let mime = extension_to_mime(&ext);

    log::info!(
        "telegram_send_image: bot={} path={} size={} ext={} endpoint={:?} mime={}",
        bot_id, path, size, ext, endpoint, mime
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let result = match endpoint {
        Endpoint::Photo => crate::telegram::api::send_photo(
            &client,
            &bot.token,
            bot.chat_id,
            bytes,
            &filename,
            mime,
            caption_ref,
        )
        .await
        .map_err(|e| e.to_string()),
        Endpoint::Document => crate::telegram::api::send_document(
            &client,
            &bot.token,
            bot.chat_id,
            bytes,
            &filename,
            mime,
            caption_ref,
        )
        .await
        .map_err(|e| e.to_string()),
    };

    if let Err(ref e) = result {
        log::error!("telegram_send_image failed: {}", e);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_photo_under_limit() {
        assert_eq!(choose_endpoint(9 * 1024 * 1024, "png"), Endpoint::Photo);
    }

    #[test]
    fn endpoint_document_size_kickout() {
        assert_eq!(
            choose_endpoint(11 * 1024 * 1024, "png"),
            Endpoint::Document
        );
    }

    #[test]
    fn endpoint_document_extension_kickout() {
        assert_eq!(
            choose_endpoint(5 * 1024 * 1024, "gif"),
            Endpoint::Document
        );
    }

    #[test]
    fn truncate_caption_ascii() {
        let s = "a".repeat(2048);
        let out = truncate_caption(&s);
        assert_eq!(out.encode_utf16().count(), 1024);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn truncate_caption_emoji_surrogate() {
        let s: String = "\u{1F600}".repeat(600);
        assert_eq!(s.encode_utf16().count(), 1200);
        let out = truncate_caption(&s);
        let units = out.encode_utf16().count();
        assert!(units <= 1024, "expected <= 1024 UTF-16 units, got {}", units);
        assert!(
            !out.contains('\u{FFFD}'),
            "truncate_caption emitted U+FFFD: dangling surrogate not dropped"
        );
    }
}
