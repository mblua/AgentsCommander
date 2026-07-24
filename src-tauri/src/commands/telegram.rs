use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::config::sessions_persistence::{
    persist_current_state_for_startup_result, persist_current_state_result,
    StartupPersistenceOutcome,
};
use crate::config::settings::SettingsState;
use crate::network::OutboundNetwork;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::telegram::bridge::SessionReaderKind;
use crate::telegram::manager::TelegramBridgeState;
use crate::telegram::types::{BridgeInfo, TelegramBotConfig};

/// Derive which session-reader pipeline to spawn for a given session.
///
/// - `Ok(Some(kind))` — agent detected and resolver succeeded → caller spawns
///   the reader.
/// - `Ok(None)` - `agent_kind` is `None` (plain shell), or the recognized
///   provider has no JSONL reader (Pi), so the caller falls back to PTY mode.
/// - `Err(message)` — agent detected but resolver returned None → caller logs +
///   emits `telegram_bridge_error` + early-returns with its contractual success
///   value (or `Err` for `telegram_attach`).
///
/// Container sessions require spawn-time memos. They never fall back to host
/// config resolvers, because the host defaults are not the container filesystem.
///
/// #260: agent selection is `Option<CodingAgentKind>`. Mutual exclusion is now
/// structural (an enum is one variant or none), so the pre-#260
/// `debug_assert!(kinds_set <= 1, …)` guard was removed.
pub(crate) fn derive_reader(
    shell: &str,
    shell_args: &[String],
    cwd: &str,
    backend_kind: SessionBackendKind,
    agent_kind: Option<CodingAgentKind>,
    resolved_claude_projects_dir: Option<PathBuf>,
    effective_codex_home: Option<&str>,
) -> Result<Option<SessionReaderKind>, String> {
    let attach_time = chrono::Utc::now();

    match agent_kind {
        Some(CodingAgentKind::Claude) => match backend_kind {
            SessionBackendKind::LocalProcess => match resolved_claude_projects_dir.or_else(|| {
                crate::commands::session::resolve_claude_projects_dir(shell, shell_args, cwd)
            }) {
                Some(p) => Ok(Some(SessionReaderKind::Claude { project_dir: p })),
                None => Err("Cannot resolve Claude projects dir".to_string()),
            },
            SessionBackendKind::ContainerTransport => match resolved_claude_projects_dir {
                Some(p) => Ok(Some(SessionReaderKind::Claude { project_dir: p })),
                None => Err("Cannot resolve Claude projects dir for container session; CLAUDE_CONFIG_DIR is not mapped into the replica mount".to_string()),
            },
        },
        Some(CodingAgentKind::Codex) => {
            let root = match backend_kind {
                SessionBackendKind::LocalProcess => {
                    let effective_home = effective_codex_home.map(Path::new);
                    crate::commands::codex_resolver::resolve_codex_sessions_root_with_effective_home(
                        effective_home,
                        shell,
                        shell_args,
                        cwd,
                    )
                }
                SessionBackendKind::ContainerTransport => {
                    effective_codex_home.map(|home| Path::new(home).join("sessions"))
                }
            };
            match root {
                Some(root) => Ok(Some(SessionReaderKind::Codex {
                    search_root: root,
                    cwd: cwd.to_string(),
                    attach_time,
                })),
                None if backend_kind == SessionBackendKind::ContainerTransport => Err("Cannot resolve Codex sessions root for container session; CODEX_HOME is not mapped into the replica mount".to_string()),
                None => Err("Cannot resolve Codex sessions root (~/.codex/sessions/ missing)".to_string()),
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
        Some(CodingAgentKind::Pi) => Ok(None),
        None => Ok(None), // No agent detected - caller falls back to PTY mode.
    }
}

pub(crate) async fn attach_telegram_bot_by_id<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    bot_id: &str,
) -> Result<BridgeInfo, String> {
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
    let tg_mgr = app.state::<TelegramBridgeState>();
    let settings = app.state::<SettingsState>();
    let network = app.state::<OutboundNetwork>().inner().clone();

    let (
        agent_kind,
        shell,
        shell_args,
        working_directory,
        backend_kind,
        resolved_claude_projects_dir,
        effective_codex_home,
    ) = {
        let mgr = session_mgr.read().await;
        let session = mgr
            .get_session(session_id)
            .await
            .ok_or_else(|| "Session not found".to_string())?;
        (
            session.agent_kind,
            session.shell.clone(),
            session.shell_args.clone(),
            session.working_directory.clone(),
            session.backend_kind,
            session.resolved_claude_projects_dir.clone(),
            session.effective_codex_home.clone(),
        )
    };

    let reader = match derive_reader(
        &shell,
        &shell_args,
        &working_directory,
        backend_kind,
        agent_kind,
        resolved_claude_projects_dir,
        effective_codex_home.as_deref(),
    ) {
        Ok(r) => r,
        Err(reason) => {
            let err_msg = format!(
                "Telegram bridge: {} for session {} (shell={:?}). Bridge inactive.",
                reason, session_id, shell
            );
            log::error!("{}", err_msg);
            let _ = app.emit(
                "telegram_bridge_error",
                serde_json::json!({
                    "sessionId": session_id.to_string(),
                    "error": err_msg,
                }),
            );
            return Err(err_msg);
        }
    };

    let bot = {
        let cfg = settings.read().await;
        cfg.telegram_bots
            .iter()
            .find(|b| b.id == bot_id)
            .cloned()
            .ok_or_else(|| format!("Bot not found: {}", bot_id))?
    };

    let info = {
        let mgr = session_mgr.read().await;
        let mut tg = tg_mgr.lock().await;
        let info = tg
            .attach(
                session_id,
                &bot,
                pty_mgr.inner().clone(),
                network.clone(),
                app.clone(),
                reader,
            )
            .map_err(|e| e.to_string())?;

        mgr.set_telegram_bot_id(session_id, Some(bot.id.clone()))
            .await;
        if let Err(e) = persist_current_state_for_app(app, &mgr).await {
            mgr.set_telegram_bot_id(session_id, None).await;
            let shutdown = tg.detach(session_id).ok();
            let err_msg = format!(
                "Telegram bridge attached but sessions.json could not be persisted; rolled back live bridge for session {}: {}",
                session_id, e
            );
            log::error!("{}", err_msg);
            let _ = app.emit(
                "telegram_bridge_error",
                serde_json::json!({
                    "sessionId": session_id.to_string(),
                    "error": err_msg,
                }),
            );
            drop(tg);
            drop(mgr);
            if let Some(shutdown) = shutdown {
                shutdown.spawn_wait_or_abort();
            }
            return Err(err_msg);
        }
        info
    };

    let _ = app.emit("telegram_bridge_attached", info.clone());
    Ok(info)
}

async fn persist_current_state_for_app<R: tauri::Runtime>(
    app: &AppHandle<R>,
    manager: &SessionManager,
) -> Result<StartupPersistenceOutcome, String> {
    match app.try_state::<crate::shutdown::ShutdownSignal>() {
        Some(shutdown) => persist_current_state_for_startup_result(manager, shutdown.inner()).await,
        None => {
            persist_current_state_result(manager).await?;
            Ok(StartupPersistenceOutcome::Persisted)
        }
    }
}

#[tauri::command]
pub async fn telegram_attach(
    app: AppHandle,
    _tg_mgr: State<'_, TelegramBridgeState>,
    _pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    _settings: State<'_, SettingsState>,
    session_id: String,
    bot_id: String,
) -> Result<BridgeInfo, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    attach_telegram_bot_by_id(&app, uuid, &bot_id).await
}

#[tauri::command]
pub async fn telegram_detach(
    app: AppHandle,
    tg_mgr: State<'_, TelegramBridgeState>,
    session_id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let mut shutdown = Some({
        let mut tg = tg_mgr.lock().await;
        tg.detach(uuid).map_err(|e| e.to_string())?
    });

    {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;

        mgr.set_telegram_bot_id(uuid, None).await;
        if let Err(e) = persist_current_state_for_app(&app, &mgr).await {
            let err_msg = format!(
                "Telegram bridge detached live, but sessions.json could not be persisted for session {}: {}",
                uuid, e
            );
            log::error!("{}", err_msg);
            let _ = app.emit(
                "telegram_bridge_error",
                serde_json::json!({
                    "sessionId": session_id.clone(),
                    "error": err_msg,
                }),
            );
            if let Some(shutdown) = shutdown.take() {
                shutdown.spawn_wait_or_abort();
            }
            return Err(err_msg);
        }
    }
    if let Some(shutdown) = shutdown.take() {
        shutdown.spawn_wait_or_abort();
    }

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

#[cfg(test)]
mod attach_detach_persistence_tests {
    /// Issue #295 — full command regression needs a Tauri AppHandle fixture with
    /// managed SessionManager/PtyManager/TelegramBridgeState and an injectable
    /// sessions.json failure path. Keep the harness shape next to the command
    /// code so the ordering contract is explicit until such a fixture exists.
    #[test]
    #[ignore = "integration: needs Tauri AppHandle + injectable sessions persistence failure"]
    fn telegram_attach_detach_persistence_ordering_harness_note() {
        // Fixture shape:
        //   1. Start with a live session and one configured Telegram bot.
        //   2. Make attach persistence fail after TelegramBridgeManager::attach
        //      succeeds; assert telegram_attach returns Err, emits
        //      telegram_bridge_error, rolls back the live bridge, and clears
        //      Session.telegram_bot_id.
        //   3. Let attach persist successfully, then start detach after attach
        //      returns Ok; assert telegram_detach returns Ok only after the
        //      serialized snapshot no longer contains telegramBotId.
        //   4. Assert the final bridge list is empty and the final persisted
        //      row has no telegramBotId.
    }
}

/// Test bot connection: discovers chat_id from the latest message sent to the bot,
/// sends a confirmation message back, and returns the discovered chat_id.
/// The user just needs to send any message to the bot before clicking Test.
#[tauri::command]
pub async fn telegram_send_test(
    network: State<'_, OutboundNetwork>,
    token: String,
) -> Result<i64, String> {
    // Fetch recent updates to discover chat_id
    let updates = crate::telegram::api::get_updates(&network, &token, 0, 0)
        .await
        .map_err(|e| e.to_string())?;

    let chat_id = updates
        .last()
        .map(|u| u.chat_id)
        .ok_or_else(|| "No messages found. Send any message to your bot in Telegram first, then click Test again.".to_string())?;

    crate::telegram::api::send_message(&network, &token, chat_id, "agentscommander connected")
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
enum Endpoint {
    Photo,
    Document,
}

/// Deterministic endpoint selection. Photo path requires BOTH size <= 10 MB
/// AND a known photo extension; everything else routes to document.
/// `ext` must already be lowercased by the caller.
fn choose_endpoint(size: u64, ext: &str) -> Endpoint {
    if size <= SEND_PHOTO_MAX_BYTES && PHOTO_EXTENSIONS.contains(&ext) {
        Endpoint::Photo
    } else {
        Endpoint::Document
    }
}

/// Map a (lowercased) file extension to the explicit `Content-Type` we pass
/// to Telegram. Unknown extensions fall back to `application/octet-stream`.
fn extension_to_mime(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

/// True if `meta` describes a symlink or — on Windows — ANY reparse point
/// (junction, mount point, …). Mirrors the helper in `commands/role_templates.rs`;
/// keep both in sync. `FileType::is_symlink()` does not flag Windows junctions
/// or file-typed reparse points, so the raw `FILE_ATTRIBUTE_REPARSE_POINT`
/// (0x400) bit is checked as well.
fn is_link_or_reparse(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Trim leading/trailing whitespace, then truncate to Telegram's 1024
/// UTF-16-code-unit cap. If the cut lands inside a surrogate pair, drop the
/// dangling high surrogate so `String::from_utf16_lossy` does not emit
/// `U+FFFD`.
fn truncate_caption(input: &str) -> String {
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
///   - `path` must point to an existing regular file; symlinks and (on Windows)
///     reparse points / junctions are rejected.
///   - Files <= 10 MB with extension in `PHOTO_EXTENSIONS` use `sendPhoto`.
///   - Everything else uses `sendDocument`, up to a 50 MB hard cap enforced
///     both at the metadata check and at the read/allocation boundary.
///   - `caption` is trimmed and clamped to `CAPTION_MAX_UTF16_UNITS` UTF-16
///     code units (Telegram's true unit). Empty captions are dropped.
///
/// Pure helper — does NOT depend on `tauri::State`. Shared by the Tauri
/// command `telegram_send_image` and the `telegram-send-image` CLI verb so
/// validation/multipart logic lives in exactly one place.
pub(crate) async fn perform_send_image(
    network: &OutboundNetwork,
    bot: &TelegramBotConfig,
    path: &str,
    caption: Option<&str>,
) -> Result<(), String> {
    let p = Path::new(path);
    let lmeta = tokio::fs::symlink_metadata(p)
        .await
        .map_err(|e| format!("stat failed: {}", e))?;
    if is_link_or_reparse(&lmeta) {
        return Err(format!(
            "Symlinks and reparse points are not supported in v1: {}",
            path
        ));
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

    let caption_trimmed: Option<String> = caption.map(truncate_caption);
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
        bot.id,
        path,
        size,
        ext,
        endpoint,
        mime
    );

    let result = match endpoint {
        Endpoint::Photo => crate::telegram::api::send_photo(
            network,
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
            network,
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

#[tauri::command]
pub async fn telegram_send_image(
    settings: State<'_, SettingsState>,
    network: State<'_, OutboundNetwork>,
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

    perform_send_image(&network, &bot, &path, caption.as_deref()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_reader_container_claude_requires_memo() {
        let result = derive_reader(
            "claude",
            &[],
            r"C:\Users\Test\repo",
            SessionBackendKind::ContainerTransport,
            Some(CodingAgentKind::Claude),
            None,
            None,
        );

        assert!(result
            .expect_err("container Claude memo is required")
            .contains("CLAUDE_CONFIG_DIR is not mapped"));
    }

    #[test]
    fn derive_reader_container_claude_uses_memo() {
        let project_dir = PathBuf::from(r"C:\repo\.ac\wg-1\__agent\.claude\projects\-workspace");
        let result = derive_reader(
            "claude",
            &[],
            r"C:\repo\.ac\wg-1\__agent",
            SessionBackendKind::ContainerTransport,
            Some(CodingAgentKind::Claude),
            Some(project_dir.clone()),
            None,
        )
        .unwrap();

        match result {
            Some(SessionReaderKind::Claude { project_dir: got }) => assert_eq!(got, project_dir),
            other => panic!("unexpected reader: {other:?}"),
        }
    }

    #[test]
    fn derive_reader_container_codex_requires_effective_home_memo() {
        let result = derive_reader(
            "codex",
            &[],
            r"C:\Users\Test\repo",
            SessionBackendKind::ContainerTransport,
            Some(CodingAgentKind::Codex),
            None,
            None,
        );

        assert!(result
            .expect_err("container Codex memo is required")
            .contains("CODEX_HOME is not mapped"));
    }

    #[test]
    fn derive_reader_container_codex_uses_effective_home_memo() {
        let home = r"C:\repo\.ac\wg-1\__agent\.codex";
        let result = derive_reader(
            "codex",
            &[],
            r"C:\repo\.ac\wg-1\__agent",
            SessionBackendKind::ContainerTransport,
            Some(CodingAgentKind::Codex),
            None,
            Some(home),
        )
        .unwrap();

        match result {
            Some(SessionReaderKind::Codex { search_root, .. }) => {
                assert_eq!(search_root, Path::new(home).join("sessions"));
            }
            other => panic!("unexpected reader: {other:?}"),
        }
    }

    #[test]
    fn derive_reader_pi_uses_pty_fallback_for_all_backends() {
        for backend in [
            SessionBackendKind::LocalProcess,
            SessionBackendKind::ContainerTransport,
        ] {
            let result = derive_reader(
                "pi",
                &["--provider".to_string(), "claude".to_string()],
                r"C:\Users\Test\repo",
                backend,
                Some(CodingAgentKind::Pi),
                None,
                None,
            );
            assert!(result.unwrap().is_none(), "backend={backend:?}");
        }
    }

    #[test]
    fn endpoint_photo_under_limit() {
        assert_eq!(choose_endpoint(9 * 1024 * 1024, "png"), Endpoint::Photo);
    }

    #[test]
    fn endpoint_document_size_kickout() {
        assert_eq!(choose_endpoint(11 * 1024 * 1024, "png"), Endpoint::Document);
    }

    #[test]
    fn endpoint_document_extension_kickout() {
        assert_eq!(choose_endpoint(5 * 1024 * 1024, "gif"), Endpoint::Document);
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
        assert!(
            units <= 1024,
            "expected <= 1024 UTF-16 units, got {}",
            units
        );
        assert!(
            !out.contains('\u{FFFD}'),
            "truncate_caption emitted U+FFFD: dangling surrogate not dropped"
        );
    }
}
