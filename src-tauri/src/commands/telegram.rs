use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::sessions_persistence::persist_current_state_result;
use crate::config::settings::SettingsState;
use crate::network::OutboundNetwork;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::telegram::bridge::{self, ReaderDest, SessionReaderKind};
#[cfg(test)]
pub(crate) use crate::telegram::manager::release_all_reader_demands;
use crate::telegram::manager::{DemandEpoch, ReaderConsumer, ReaderEntry, TelegramBridgeState};
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
// Eight parameters since #2454 split the identity slice from `shell_args` (D3-g).
#[allow(clippy::too_many_arguments)]
pub(crate) fn derive_reader(
    shell: &str,
    shell_args: &[String],
    identity_args: &[String],
    cwd: &str,
    backend_kind: SessionBackendKind,
    agent_kind: Option<CodingAgentKind>,
    resolved_claude_projects_dir: Option<PathBuf>,
    effective_codex_home: Option<&str>,
) -> Result<Option<SessionReaderKind>, String> {
    let attach_time = chrono::Utc::now();
    // #2454: the EFFECTIVE argv names the transcript id AC minted; the
    // CONFIGURED `shell_args` alone still decide the projects directory.
    let transcript_id = crate::commands::session::transcript_id_from_args(identity_args);

    match agent_kind {
        Some(CodingAgentKind::Claude) => match backend_kind {
            SessionBackendKind::LocalProcess => match resolved_claude_projects_dir.or_else(|| {
                crate::commands::session::resolve_claude_projects_dir(shell, shell_args, cwd)
            }) {
                Some(p) => Ok(Some(SessionReaderKind::Claude {
                    project_dir: p,
                    transcript_id,
                })),
                None => Err("Cannot resolve Claude projects dir".to_string()),
            },
            SessionBackendKind::ContainerTransport => match resolved_claude_projects_dir {
                Some(p) => Ok(Some(SessionReaderKind::Claude {
                    project_dir: p,
                    transcript_id,
                })),
                None => Err("Cannot resolve Claude projects dir for container session; \
                CLAUDE_CONFIG_DIR is not mapped into the replica mount"
                    .to_string()),
            },
        },
        Some(CodingAgentKind::Codex) => {
            let root = match backend_kind {
                SessionBackendKind::LocalProcess => {
                    let effective_home = effective_codex_home.map(Path::new);
                    super::codex_resolver::resolve_codex_sessions_root_with_effective_home(
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
                None if backend_kind == SessionBackendKind::ContainerTransport => Err("Cannot \
                resolve Codex sessions root for container session; CODEX_HOME is not mapped into \
                the replica mount"
                    .to_string()),
                None => Err("Cannot resolve Codex sessions root (~/.codex/sessions/ \
                missing)"
                    .to_string()),
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
        session
            .effective_shell_args
            .as_deref()
            .unwrap_or(&session.shell_args),
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

/// True while `session_id` already holds a **Room** demand (#2456).
///
/// An in-memory lookup under the bridge lock, the same query as the fast path
/// in [`raise_reader_demand`], so the re-raise tick can skip a satisfied
/// session before paying for readiness resolution.
pub(crate) async fn holds_room_reader_demand<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> bool {
    let Some(tg_state) = app.try_state::<TelegramBridgeState>() else {
        return false;
    };
    let tg = tg_state.lock().await;
    tg.reader_demands(session_id)
        .contains(&ReaderConsumer::Room)
}

/// The Room demand's release epoch for `session_id` (#2516), read under the
/// bridge lock; [`DemandEpoch::ZERO`] when the bridge state is unmanaged.
pub(crate) async fn room_demand_epoch<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> DemandEpoch {
    let Some(tg_state) = app.try_state::<TelegramBridgeState>() else {
        return DemandEpoch::ZERO;
    };
    let tg = tg_state.lock().await;
    tg.demand_epoch(session_id, ReaderConsumer::Room)
}

/// [`raise_reader_demand_guarded`] with no epoch guard: the Bot path.
pub(crate) async fn raise_reader_demand<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    consumer: ReaderConsumer,
    dest: Option<ReaderDest>,
) -> bool {
    raise_reader_demand_guarded(app, session_id, consumer, dest, None).await
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
///
/// With `expected` set (#2516), the raise aborts and returns `false`, adding
/// no demand, opening no capture slot and spawning no reader, when a release
/// of this `(session_id, consumer)` demand has happened since the caller
/// sampled `expected`. The check is the first statement under each bridge
/// lock, so compare and mutation share one critical section.
pub(crate) async fn raise_reader_demand_guarded<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    consumer: ReaderConsumer,
    dest: Option<ReaderDest>,
    expected: Option<DemandEpoch>,
) -> bool {
    let Some(tg_state) = app.try_state::<TelegramBridgeState>() else {
        return false;
    };
    // Fast path taken without resolving anything: the reader is already there.
    {
        let mut tg = tg_state.lock().await;
        if expected.is_some_and(|e| tg.demand_epoch(session_id, consumer) != e) {
            return false;
        }
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
    if expected.is_some_and(|e| tg.demand_epoch(session_id, consumer) != e) {
        return false;
    }
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
        crate::telegram::manager::release_all_reader_demands(app, session_id).await;
        return false;
    }
    true
}

#[cfg(test)]
pub(crate) mod reader_demand_seam;

/// Test-only failure injection for `attach_telegram_bot_by_id`'s
/// `sessions.json` persistence step (phase 4 attach-rollback coverage).
///
/// Keyed by session id and consumed on first use: arming a failure for one
/// test cannot leak into another test that attaches a different session.
#[cfg(test)]
pub(crate) mod attach_persistence_seam;

/// Test-only failure injection for `telegram_detach`'s `sessions.json`
/// persistence step (phase 4 test 21).
///
/// Keyed by session id and consumed on first use: arming a failure for one
/// test cannot leak into another test that detaches a different session.
#[cfg(test)]
pub(crate) mod detach_persistence_seam;

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
        effective_shell_args,
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
            session.effective_shell_args.clone(),
            session.working_directory.clone(),
            session.backend_kind,
            session.resolved_claude_projects_dir.clone(),
            session.effective_codex_home.clone(),
        )
    };

    let reader = match derive_reader(
        &shell,
        &shell_args,
        effective_shell_args.as_deref().unwrap_or(&shell_args),
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
        let cfg = settings
            .current_read()
            .await
            .map_err(|error| error.to_string())?;
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
        let persist_result = persist_current_state_result(&mgr).await;
        #[cfg(test)]
        let persist_result = if attach_persistence_seam::take(session_id) {
            Err("synthetic attach persistence failure".to_string())
        } else {
            persist_result
        };
        if let Err(e) = persist_result {
            mgr.set_telegram_bot_id(session_id, None).await;
            let shutdown = tg.detach(session_id).ok();
            let err_msg = format!(
                "Telegram bridge attached but sessions.json could not be persisted; rolled back \
                live bridge for session {}: {}",
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
            // #2232 phase 4 section 5: this attach raises the Bot demand only
            // **after** persistence succeeds, so the rollback has no demand of
            // its own to release. Releasing here would tear down the demand of
            // an earlier, still-valid attach (an idempotent repeat attach lands
            // in this branch), which section 5 forbids: a rollback releases Bot
            // only when that attach raised it.
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
    _tg_mgr: State<'_, TelegramBridgeState>,
    session_id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    detach_telegram_inner(&app, uuid).await
}

/// Detach the bot side of a session and release its Bot demand.
///
/// Split out of the command so phase 4 test 21 can drive it with a plain
/// `AppHandle` while still exercising the real persistence path and the
/// `#[cfg(test)]` failure seam in it.
pub(crate) async fn detach_telegram_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    uuid: Uuid,
) -> Result<(), String> {
    let session_id = uuid.to_string();
    let tg_mgr = app.state::<TelegramBridgeState>();
    let mut shutdown = Some({
        let mut tg = tg_mgr.lock().await;
        tg.detach(uuid).map_err(|e| e.to_string())?
    });

    let persist_result = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;

        mgr.set_telegram_bot_id(uuid, None).await;
        let persist_result = persist_current_state_result(&mgr).await;
        #[cfg(test)]
        let persist_result = if detach_persistence_seam::take(uuid) {
            Err("synthetic detach persistence failure".to_string())
        } else {
            persist_result
        };
        persist_result
    };
    if let Some(shutdown) = shutdown.take() {
        shutdown.spawn_wait_or_abort();
    }

    // #2232 phase 4 section 5: a live detach releases the **bot** demand even
    // when `sessions.json` persistence fails, so the reader stops sending and
    // a Room demand, if any, keeps capturing. The persistence error and event
    // below are unchanged.
    release_reader_demand(app, uuid, ReaderConsumer::Bot).await;

    if let Err(e) = persist_result {
        let err_msg = format!(
            "Telegram bridge detached live, but sessions.json could not be persisted for session \
            {}: {}",
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
        return Err(err_msg);
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

    let chat_id = updates.last().map(|u| u.chat_id).ok_or_else(|| {
        "No messages found. Send any message to your bot in Telegram first, then \
        click Test again."
            .to_string()
    })?;

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

/// Send original plain text, discarding remote errors at the CLI boundary.
pub(crate) async fn perform_send_message(
    network: &OutboundNetwork,
    bot: &TelegramBotConfig,
    text: &str,
) -> Result<(), &'static str> {
    crate::telegram::api::send_message(network, &bot.token, bot.chat_id, text)
        .await
        .map_err(|_| "Telegram send failed; delivery may be uncertain")
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
    let (bytes, size) =
        crate::telegram::attachments::prepare_telegram_attachment(path, SEND_DOCUMENT_MAX_BYTES)
            .await?;

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
    let cfg = settings
        .current_read()
        .await
        .map_err(|error| error.to_string())?;
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
mod tests;
