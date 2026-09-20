use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::config::sessions_persistence::persist_current_state_result;
use crate::config::settings::SettingsState;
use crate::network::OutboundNetwork;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::telegram::bridge::{self, ReaderDest, SessionReaderKind};
use crate::telegram::manager::{ReaderConsumer, ReaderEntry, TelegramBridgeState};
use crate::telegram::types::{BridgeInfo, TelegramBotConfig};

/// Derive which session-reader pipeline to spawn for a given session.
///
/// - `Ok(Some(kind))` — agent detected and resolver succeeded → caller spawns
///   the reader.
/// - `Ok(None)` - `agent_kind` is `None` (plain shell), or the recognized
///   provider has no JSONL reader (Pi, Antigravity), so the caller falls back
///   to PTY mode.
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
        Some(CodingAgentKind::Antigravity) => Ok(None),
        Some(CodingAgentKind::Pi) => Ok(None),
        // #1873 - Muse has no reader and no PTY fallback contract; every caller
        // returns before any bridge or filter is created.
        Some(CodingAgentKind::Muse) => {
            Err("Telegram bridge does not support Muse sessions".to_string())
        }
        None => Ok(None), // No agent detected - caller falls back to PTY mode.
    }
}

/// Resolve the reader pipeline of a live session, or `None` when it has none.
async fn reader_kind_for_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> Option<SessionReaderKind> {
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let session = {
        let mgr = session_mgr.read().await;
        mgr.get_session(session_id).await?
    };
    derive_reader(
        &session.shell,
        &session.shell_args,
        &session.working_directory,
        session.backend_kind,
        session.agent_kind,
        session.resolved_claude_projects_dir.clone(),
        session.effective_codex_home.as_deref(),
    )
    .ok()
    .flatten()
}

/// True while `session_id` still exists in the session manager.
///
/// Used by the detached create-time raise (section 5.1): if a destroy won the
/// race while the raise was resolving, the freshly installed reader must be
/// discarded instead of leaking for a session that is already gone.
async fn session_is_live<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) -> bool {
    let Some(session_mgr) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() else {
        return true;
    };
    let mgr = session_mgr.read().await;
    mgr.get_session(session_id).await.is_some()
}

/// Raise `consumer`'s demand on `session_id`'s transcript reader (#2232 phase 4
/// section 5).
///
/// Idempotent, and **adding a demand never restarts a running reader**: the new
/// consumer joins it and a cut is recorded at the reader's frontier. When no
/// reader is running one is spawned here, with the live `CaptureRegistry`
/// sender, which is the link that makes phases 1 and 3 reachable from
/// production.
///
/// Returns `true` when a reader is running for the session afterwards.
pub(crate) async fn raise_reader_demand<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    consumer: ReaderConsumer,
    dest: Option<ReaderDest>,
) -> bool {
    let Some(tg_state) = app.try_state::<TelegramBridgeState>() else {
        return false;
    };
    // Fast path taken without resolving anything: the reader is already there.
    {
        let mut tg = tg_state.lock().await;
        if tg.reader_is_running(session_id) {
            return tg.reader_demand_add(session_id, consumer, dest);
        }
    }

    let Some(kind) = reader_kind_for_session(app, session_id).await else {
        return false;
    };
    // Test-only rendezvous after eligibility resolution and before install
    // (phase 4 test 19): the create/destroy race is paused here.
    #[cfg(test)]
    reader_demand_seam::hit_before_install(&session_id.to_string()).await;
    let network = app.state::<OutboundNetwork>().inner().clone();

    let mut tg = tg_state.lock().await;
    // Re-check under the lock: another demand may have spawned it meanwhile.
    if tg.reader_is_running(session_id) {
        return tg.reader_demand_add(session_id, consumer, dest);
    }
    let (capture, rx) = tg.captures().open(&session_id.to_string());
    let reader_id = tg.next_reader_id();
    let spawned = bridge::spawn_reader(
        kind,
        session_id,
        dest,
        network,
        app.clone(),
        Some(capture.tx.clone()),
        rx.map(|rx| (capture.slot.clone(), rx)),
    );
    tg.reader_install(session_id, ReaderEntry::new(reader_id, spawned), consumer);
    drop(tg);

    // Section 5.1: the detached create-time raise rechecks session liveness
    // after install. If a destroy won the race while this raise was paused, the
    // reader is discarded and its capture slot closed instead of leaking until
    // shutdown. The check is deliberately outside `TelegramBridgeState`: the
    // destroy path takes the session lock and then this state, so awaiting the
    // session lock while holding this state would invert that order.
    if !session_is_live(app, session_id).await {
        release_all_reader_demands(app, session_id).await;
        return false;
    }
    true
}

#[cfg(test)]
pub(crate) mod reader_demand_seam {
    //! Test-only rendezvous inside [`super::raise_reader_demand`].
    //!
    //! Armed for one session id; the raise signals `reached` and then awaits
    //! `release`. Phase 4 test 19 uses it to destroy a session while the
    //! detached create-time raise sits between eligibility resolution and
    //! install.
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::sync::Notify;

    #[derive(Default)]
    pub(crate) struct ReaderDemandBarrier {
        pub(crate) reached: Notify,
        pub(crate) release: Notify,
    }

    type BarrierMap = Mutex<Option<HashMap<String, Arc<ReaderDemandBarrier>>>>;

    static BEFORE_INSTALL: BarrierMap = Mutex::new(None);

    pub(crate) fn install_before_install(key: &str) -> Arc<ReaderDemandBarrier> {
        let barrier = Arc::new(ReaderDemandBarrier::default());
        BEFORE_INSTALL
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashMap::new)
            .insert(key.to_string(), Arc::clone(&barrier));
        barrier
    }

    pub(crate) async fn hit_before_install(key: &str) {
        let barrier = BEFORE_INSTALL
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .and_then(|map| map.remove(key));
        if let Some(barrier) = barrier {
            barrier.reached.notify_one();
            barrier.release.notified().await;
        }
    }
}

/// Release `consumer`'s demand. The reader stops only when it was the last one.
///
/// The drain order is emitted with `TelegramBridgeState` **released**, within
/// the existing 2 s budget (section 9): awaiting inside the state lock is what
/// makes a slow chat block unrelated sessions.
pub(crate) async fn release_reader_demand<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    consumer: ReaderConsumer,
) {
    let Some(tg_state) = app.try_state::<TelegramBridgeState>() else {
        return;
    };
    let shutdown = {
        let mut tg = tg_state.lock().await;
        tg.reader_demand_release(session_id, consumer)
    };
    if let Some(shutdown) = shutdown {
        shutdown.spawn_wait_or_abort();
    }
}

/// Release **every** demand for a session. Destroy and shutdown do this; a
/// persistence rollback releases only the bot demand (section 5).
pub(crate) async fn release_all_reader_demands<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) {
    let Some(tg_state) = app.try_state::<TelegramBridgeState>() else {
        return;
    };
    let shutdown = {
        let mut tg = tg_state.lock().await;
        tg.reader_release_all(session_id)
    };
    if let Some(shutdown) = shutdown {
        shutdown.spawn_wait_or_abort();
    }
}

/// Re-anchor the reader's transcript **file** after a restart, keeping the
/// demand set untouched (section 5.2).
pub(crate) async fn reanchor_reader<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    let Some(tg_state) = app.try_state::<TelegramBridgeState>() else {
        return;
    };
    let tg = tg_state.lock().await;
    tg.reader_reanchor(session_id);
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

    // #2232 phase 4: the reader no longer lives in the bridge. The bridge owns
    // the bot-side tasks; the reader is raised as a **Bot demand** below, which
    // starts one if none is running and otherwise attaches over the live one.
    let reader_mode = reader.is_some();

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
                reader_mode,
                agent_kind,
            )
            .map_err(|e| e.to_string())?;

        mgr.set_telegram_bot_id(session_id, Some(bot.id.clone()))
            .await;
        if let Err(e) = persist_current_state_result(&mgr).await {
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
            // A rollback releases **only** the bot demand (section 5); a room
            // demand, if any, keeps the reader running.
            release_reader_demand(app, session_id, ReaderConsumer::Bot).await;
            return Err(err_msg);
        }
        info
    };

    if reader_mode {
        raise_reader_demand(
            app,
            session_id,
            ReaderConsumer::Bot,
            Some(ReaderDest {
                token: bot.token.clone(),
                chat_id: bot.chat_id,
            }),
        )
        .await;
    }

    let _ = app.emit("telegram_bridge_attached", info.clone());
    Ok(info)
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
        if let Err(e) = persist_current_state_result(&mgr).await {
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

    // #2232 phase 4: detaching releases the **bot** demand. A room demand, if
    // any, keeps the reader running with Telegram sends stopped.
    release_reader_demand(&app, uuid, ReaderConsumer::Bot).await;

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

    /// #1873 - Muse is rejected with one exact message on both backends, before
    /// `TelegramBridgeManager::attach` could create a bridge or filter.
    #[test]
    fn derive_reader_rejects_muse_for_all_backends_before_bridge_creation() {
        for backend in [
            SessionBackendKind::LocalProcess,
            SessionBackendKind::ContainerTransport,
        ] {
            for (shell, args) in [
                ("muse", Vec::<String>::new()),
                (
                    "/opt/muse/bin/muse",
                    vec!["resume".to_string(), "--last".to_string()],
                ),
            ] {
                let result = derive_reader(
                    shell,
                    &args,
                    "/srv/work/repo",
                    backend,
                    Some(CodingAgentKind::Muse),
                    None,
                    None,
                );
                let err = result.expect_err("Muse must be rejected");
                assert_eq!(
                    err,
                    "Telegram bridge does not support Muse sessions".to_string(),
                    "backend={backend:?} shell={shell:?}"
                );
            }
        }
    }

    #[test]
    fn derive_reader_antigravity_uses_pty_fallback_for_all_backends() {
        for backend in [
            SessionBackendKind::LocalProcess,
            SessionBackendKind::ContainerTransport,
        ] {
            let result = derive_reader(
                "agy",
                &["-m".to_string(), "gpt-5".to_string()],
                r"C:\Users\Test\repo",
                backend,
                Some(CodingAgentKind::Antigravity),
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
