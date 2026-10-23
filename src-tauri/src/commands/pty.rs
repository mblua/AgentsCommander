use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::voice::tracker::VoiceTrackingState;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyScreenSnapshotPayload {
    pub session_id: String,
    pub data: Vec<u8>,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
    pub sequence: u64,
}

/// (#871) Classifies user-input notifications so fresh-intent clearing can be
/// gated on substantive post-boundary submissions.
pub(crate) enum UserInputSource<'a> {
    /// xterm terminal keystrokes from the Tauri `pty_write` command.
    Terminal(&'a [u8]),
    /// Web UI raw keystrokes from binary frames or the web command path.
    Web(&'a [u8]),
    /// A complete submitted message, always substantive.
    CompleteMessage,
}

#[tauri::command]
pub async fn pty_write(
    app: AppHandle,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    voice_tracker: State<'_, VoiceTrackingState>,
    session_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

    // (#885 J1) Keystrokes into a session being purged would flip it busy
    // between the readiness snapshot and its destroy. Scoped to the purge's
    // target set, so typing in unrelated sessions is unaffected.
    if let Some(g) = app.try_state::<std::sync::Arc<crate::session::purge_guard::PurgeGuard>>() {
        if g.blocks_session(uuid) {
            return Err("purge-room in progress for this session; input rejected".to_string());
        }
    }

    // Flag if user typed while voice recording is active for this session.
    // Lock scope closes before pty_mgr lock to avoid holding both.
    {
        let mut tracker = voice_tracker.lock().unwrap();
        if tracker.is_recording(uuid) {
            tracker.mark_typed(uuid);
        }
    }

    let permit = PtyManager::acquire_input_writer(pty_mgr.inner(), uuid)
        .await
        .map_err(|error| error.to_string())?;
    // Purge may have started while this writer waited behind another complete
    // submission. Recheck at the serialized boundary.
    if let Some(g) = app.try_state::<std::sync::Arc<crate::session::purge_guard::PurgeGuard>>() {
        if g.blocks_session(uuid) {
            return Err("purge-room in progress for this session; input rejected".to_string());
        }
    }
    PtyManager::write_with_permit(&permit, &data).map_err(|error| error.to_string())?;
    record_typing_hold_keystroke(&app, uuid, &data);
    if log::log_enabled!(log::Level::Debug) {
        let kind = match data.as_slice() {
            b"\x1b[I" => "focus-in",
            b"\x1b[O" => "focus-out",
            [] => "empty",
            [0x1b, ..] => "escape-prefixed",
            [0x00..=0x1f | 0x7f, ..] => "control-prefixed",
            _ => "other",
        };
        log::debug!(
            "[pty-input] session={} source=desktop bytes={} kind={} busy_path=mark_successful_pty_write_busy",
            uuid,
            data.len(),
            kind
        );
    }
    mark_successful_pty_write_busy(&app, uuid, data.len()).await;
    drop(permit);

    // #552 user input -> silence touch (+ badge reset if coordinator). Resolves
    // all state from `app`, so the same helper serves Telegram and web.
    note_user_message_to_session(&app, uuid, UserInputSource::Terminal(&data)).await;

    Ok(())
}

pub(crate) async fn mark_successful_pty_write_busy<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    byte_count: usize,
) {
    if let Some(idle) = app.try_state::<Arc<crate::pty::idle_detector::IdleDetector>>() {
        // #1682 - every caller of this helper is a PTY write that is not, by
        // itself, a delivered message: xterm and web keystrokes, the injection
        // text write (whose own arming block clears the mark a few lines later)
        // and the graceful-close `exit`. Marking here rather than in
        // `note_user_message_to_session` is what lets the detector test
        // `idle_set` BEFORE the write is recorded; by the time this helper
        // returns, the session is busy and that question can no longer be asked.
        idle.record_control_write_activity(session_id, byte_count);
    }
    if let Some(sessions) =
        app.try_state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>()
    {
        let manager = {
            let guard = sessions.read().await;
            guard.clone()
        };
        manager.mark_busy(session_id).await;
    }
}

/// #1682 - arming an agent turn cancels any standing control-write mark for this
/// session. Called from TWO of the three arming sites: `note_user_message_to_session`
/// here, and `pty::inject::inject_text_into_session_impl`. The third,
/// `SessionManager::prepare_pty_input_boundary`, clears through
/// `IdleDetector::clear_control_write` directly, because `session::manager` holds the
/// detector as a parameter and takes no `AppHandle`. A no-op when the detector is not
/// managed, which is the same `try_state` fail-open the neighbouring helper uses.
pub(crate) fn clear_control_write_mark<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    if let Some(idle) = app.try_state::<Arc<crate::pty::idle_detector::IdleDetector>>() {
        idle.clear_control_write(session_id);
    }
}

/// (#1793) True iff this session has emitted PRINTABLE PTY output strictly
/// after `since`. The restart-resume pass pairs this with `waiting_for_input`
/// to tell "the agent painted its prompt and went quiet" apart from "the agent
/// has printed nothing at all since spawn": `IdleDetector::register_session`
/// SEEDS `activity[id]` at spawn (#260), so the idle flag on its own reaches
/// `true` for a session that never printed.
///
/// Lives in this module, not in `commands::session`, purely for the module
/// graph: this file already resolves the detector through `try_state` and
/// already owns the arc to `pty::idle_detector`, while `commands::session` does
/// not. Calling the detector from there would add a module arc for one call.
///
/// Fails CLOSED when the detector is not managed, DELIBERATELY the opposite
/// direction from the neighbouring `try_state` helpers in this file: a missing
/// detector costs them a skipped bookkeeping update, but would cost this an
/// unrequested write into a user's agent.
pub(crate) fn session_printed_since<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    since: Instant,
) -> bool {
    match app.try_state::<Arc<crate::pty::idle_detector::IdleDetector>>() {
        Some(idle) => idle.has_printable_activity_since(session_id, since),
        None => false,
    }
}

/// #552 Record a real user message to `session_id`: always reset the auto-close
/// silence clock; if the session is a coordinator, reset its badge clock and
/// emit `coordinator_clock_updated` (and clear any "auto-closed" marker).
/// Resolves all state from `app`, so every user-input surface (xterm
/// `pty_write`, Telegram inbound, web UI) can call it with its source tag.
/// Injection / auto-resume MUST NOT call this (they are not user messages).
///
/// Generic over the Tauri runtime so callers holding either a concrete
/// `AppHandle` or a generic `AppHandle<R>` (e.g. the Telegram bridge) can reuse it.
pub(crate) async fn note_user_message_to_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    source: UserInputSource<'_>,
) {
    let (substantive, source_class): (bool, &'static str) = match source {
        UserInputSource::Terminal(data) => {
            (classify_substantive(app, session_id, data), "terminal")
        }
        UserInputSource::Web(data) => (classify_substantive(app, session_id, data), "web"),
        UserInputSource::CompleteMessage => (true, "message"),
    };

    // (a) auto-close silence: any user message keeps the team alive.
    if let Some(idle) = app.try_state::<Arc<crate::pty::idle_detector::IdleDetector>>() {
        idle.touch_silence(session_id);
    }

    // (#2232 phase 3, section 10.1) The automatic-action budget recharge has
    // exactly one call site, this one, and it reuses the source filter computed
    // one screen above rather than adding an event: `coordinator_clock_updated`
    // carries no source and fires for Telegram and web alike, so the specified
    // exclusion was unbuildable through it. Injection and auto-resume never
    // reach this function, so they cannot recharge.
    if recharges_capture_budget(substantive, &source) {
        if let Some(room_root) = room_root_of_orchestrator(app, session_id).await {
            crate::capture::state::recharge_in_memory(&room_root);
            crate::capture::state::schedule_flush(&room_root);
        }
    }

    // (#630/#631 + #698) Apply both user-input state transitions and persist them
    // atomically. On the FIRST substantive post-boundary submission we re-arm the
    // resume intent, and every user write still lowers any visible raise-hand
    // communication. This is the unified user-input choke point
    // (xterm/Telegram/web); injection and auto-resume never call it, and it runs
    // before the coordinator-only early return below so non-coordinator members
    // re-arm too when the write is substantive.
    //
    // `clear_user_input_transitions_and_persist_result` flips BOTH fields in one
    // SessionManager critical section and runs the mutation + snapshot + save
    // under a single global save lock, so no concurrent persist can snapshot a
    // half-applied state or write an intermediate one (MEDIUM grinch fix). The
    // clear event is emitted only after persistence succeeds, so `list-sessions`
    // and the UI agree.
    let manager = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let guard = mgr.read().await;
        guard.clone()
    };
    // #1682 - a substantive message arms this session; the busy->idle edges that
    // follow stamp `tooling.lastAgentMessageAt`. Non-substantive writes (arrow
    // keys, focus reports, a bare Enter, a cancelled line) never arm.
    if substantive {
        manager.arm_agent_turn(session_id).await;
        clear_control_write_mark(app, session_id);
    }
    let cleared =
        match crate::config::sessions_persistence::clear_user_input_transitions_and_persist_result(
            &manager,
            session_id,
            substantive,
        )
        .await
        {
            Ok(cleared) => cleared,
            Err(e) => {
                log::error!(
                    "Failed to persist user-input session state transitions: {}",
                    e
                );
                // The in-memory clear still applied; only the snapshot failed.
                // Suppress the clear event so the live UI does not diverge from the
                // durable file (the next persist will reconcile disk).
                crate::config::sessions_persistence::ClearedUserInputTransitions::default()
            }
        };

    if cleared.cleared_start_fresh {
        log::info!(
            "[session-state] {} fresh intent cleared: substantive {} input (#871)",
            &session_id.to_string()[..8],
            source_class
        );
    } else if !substantive {
        log::debug!(
            "[session-state] {} non-substantive {} write; fresh intent preserved (#871)",
            &session_id.to_string()[..8],
            source_class
        );
    }

    if cleared.cleared_raise_hand {
        crate::session::selection::publish_session_communication(app, session_id, None);
    }

    // (b) badge: reset only when the typed-to session is a coordinator.
    let cwd = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let cwd = mgr.read().await.coordinator_cwd(session_id).await;
        cwd
    };
    let Some(cwd) = cwd else { return };
    let Some(clocks) = app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
    else {
        return;
    };

    // agent_fqn_from_path returns String (teams.rs:80), not Option.
    let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
    let now = chrono::Utc::now();
    let (changed, cleared_auto, cleared_fresh) = {
        let mut guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
        let changed = guard.note_user_message(&fqn, now);
        // #552 a real user message reopens the coordinator -> clear any
        // "auto-closed" marker (idempotent; no-op if not marked).
        let cleared_auto = guard.clear_auto_closed(&fqn);
        // (#871/#756) Drop the fresh-intent mirror only on a substantive
        // post-boundary submission. Non-substantive terminal writes leave it set
        // so an app restart still restores fresh.
        let cleared_fresh = if substantive {
            guard.clear_start_fresh(&fqn)
        } else {
            false
        };
        (changed, cleared_auto, cleared_fresh)
    };
    if changed {
        let _ = app.emit(
            "coordinator_clock_updated",
            serde_json::json!({ "replicaPath": cwd, "lastUserMessageAt": now.to_rfc3339() }),
        );
    }
    if cleared_auto {
        let _ = app.emit(
            "coordinator_auto_close_changed",
            serde_json::json!({ "replicaPath": cwd, "autoClosedAt": null }),
        );
    }
    if cleared_fresh {
        // (#756) Persist immediately: this transition must survive an app close
        // inside the 60s flush tick window (mirrors close_coordinator's
        // explicit save; the exit flush in lib.rs only covers clean exits).
        let snapshot = { clocks.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
        if let Err(e) = crate::config::coordinator_clocks::save_map(&snapshot) {
            log::warn!("[coordinator-clocks] fresh-intent clear save failed: {}", e);
        }
    }
}

/// (#2232 phase 3) Does this user input recharge the room's automatic-action
/// budget?
///
/// Human keystrokes only. Web and inbound Telegram (`CompleteMessage`) are
/// excluded **by construction**, so anyone with chat access, including a bot,
/// or any client of the web server, cannot keep an automatic chain alive
/// forever; that is what keeps the bound a terminating construction. It is a
/// technical decision and is revisable by the user. `substantive` is the
/// existing `classify_substantive` criterion, not a second one.
fn recharges_capture_budget(substantive: bool, source: &UserInputSource<'_>) -> bool {
    substantive && matches!(source, UserInputSource::Terminal(_))
}

/// (#2232 phase 3) The room root of the orchestrator terminal `session_id` was
/// typed into, or `None` when the session is not an orchestrator.
///
/// The resolution is async and happens **before** the leaf call, so
/// `capture::state::recharge_in_memory` stays a synchronous, lock-free leaf and
/// `commands::pty` keeps an SCC-to-leaf arc that cannot grow the SCC.
async fn room_root_of_orchestrator<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> Option<std::path::PathBuf> {
    let cwd = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let cwd = mgr.read().await.coordinator_cwd(session_id).await;
        cwd
    };
    crate::config::co_managed::room_root_for_path(std::path::Path::new(&cwd?))
}

/// (#871) Run the substantive-submission classifier for a raw keystroke chunk.
/// Locks the managed tracker briefly with no await held. Fail-open to true if
/// the tracker state is absent, preserving the historical clear contract.
fn classify_substantive<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    data: &[u8],
) -> bool {
    let Some(state) = app.try_state::<crate::pty::input_activity::SubstantiveInputState>() else {
        return true;
    };
    let mut tracker = state.lock().unwrap_or_else(|e| e.into_inner());
    tracker.feed(session_id, data)
}

/// #2336 - record a desktop keystroke for the typing hold. ONLY `pty_write`
/// calls this: web writes and every injection path must not arm the natural
/// window. Called while the per-session writer permit is still held, so a peer
/// injection that acquires the permit next observes the key.
///
/// The web write paths (`web/commands.rs` dispatch `pty_write`, and the
/// binary-frame handler in `web/mod.rs`) call `mark_successful_pty_write_busy`
/// and `note_user_message_to_session(UserInputSource::Web(..))` but never this
/// recorder, so web input cannot arm or refresh the natural hold. Accepted
/// evidence for that property, per the review fallback: this source review plus
/// `typing_hold_user_message_sources_do_not_arm_hold` in this module. Driving
/// the real web dispatcher needs a Wry `WsState` fixture and web-module
/// internals, outside this phase's approved file inventory.
fn response_close_hold_publisher<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Option<crate::pty::manager::ResponseCloseState> {
    app.try_state::<crate::pty::manager::ResponseCloseState>()
        .map(|state| state.inner().clone())
}

fn record_typing_hold_keystroke<R: tauri::Runtime>(
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
async fn typing_hold_window<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<Duration, String> {
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
fn typing_hold_snapshot<R: tauri::Runtime>(
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

/// #2336 - confirm a session exists before reading or flipping its padlock, so
/// an invalid or absent session errors instead of silently touching nothing.
async fn ensure_session_exists<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> Result<(), String> {
    let Some(sessions) =
        app.try_state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>()
    else {
        return Err("session manager is unavailable".to_string());
    };
    let manager = {
        let guard = sessions.read().await;
        guard.clone()
    };
    if manager.get_session(session_id).await.is_some() {
        Ok(())
    } else {
        Err(format!("Session not found: {}", session_id))
    }
}

/// #2336 - read the per-session typing-hold snapshot. `closed` is the effective
/// hold (manual or natural window); `heldCount` is the unique deferred peer wake
/// message count. Pure in-memory read: no filesystem, DB or source-side
/// reconciliation, and no mutation of the count.
#[tauri::command]
pub async fn get_typing_hold(
    app: AppHandle,
    session_id: String,
) -> Result<crate::pty::input_activity::TypingHoldSnapshot, String> {
    let session_id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    ensure_session_exists(&app, session_id).await?;
    let window = typing_hold_window(&app).await?;
    Ok(typing_hold_snapshot(&app, session_id, window))
}

/// #2336 - flip one session's manual padlock and return the post-toggle
/// snapshot. An effective-closed session releases (suppressing the current
/// natural window so the next poll may deliver); an open one takes the manual
/// hold. Per-session only: an unknown session errors and leaves every other
/// session untouched.
#[tauri::command]
pub async fn toggle_typing_hold(
    app: AppHandle,
    session_id: String,
) -> Result<crate::pty::input_activity::TypingHoldSnapshot, String> {
    let session_id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    ensure_session_exists(&app, session_id).await?;
    let window = typing_hold_window(&app).await?;
    match app.try_state::<crate::pty::input_activity::TypingHoldState>() {
        Some(state) => {
            let publisher = response_close_hold_publisher(&app);
            let _admission = match &publisher {
                Some(publisher) => publisher
                    .reserve_hold_change(session_id)
                    .map_err(|_| "response_close_deferred".to_string())?,
                None => None,
            };
            let mut hold = state.lock().unwrap_or_else(|error| error.into_inner());
            let was_closed = hold.snapshot(session_id, window).closed;
            let snapshot = hold.toggle_manual(session_id, window);
            if let Some(publisher) = publisher {
                let now = std::time::Instant::now();
                publisher.refresh_hold_snapshot(session_id, window, snapshot.closed, now);
                publisher.publish_hold(
                    session_id,
                    if was_closed {
                        None
                    } else {
                        now.checked_add(window.saturating_mul(2))
                    },
                    true,
                    now,
                );
            }
            Ok(snapshot)
        }
        None => Err("typing hold state is unavailable".to_string()),
    }
}

/// (#756) Record an AC-driven fresh-conversation boundary for `session_id`:
/// write the coordinator-clocks mirror (`start_fresh_at`, persisted
/// immediately) and THEN stamp the durable record intent
/// (`start_fresh_on_restore = true`, persisted under the sessions save lock).
/// Mirror-first (section 19.3): the death-between-halves residue must fail
/// FRESH, never resurrect. The intent survives record destruction (idle
/// auto-close, manual close). Call only after a successful logical clear
/// injection (/clear or Pi /new; C1 remote action, C2 self-clear phase 1). The restart
/// path (C3) stamps the record itself and calls only the mirror half.
/// Root-agent sessions skip the record half (the root restore path ignores the
/// marker, #630 scope; mirrors the restart site's exclusion in
/// commands/session.rs); the mirror half self-gates on coordinators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundaryMetadataOutcome {
    Applied,
    Unchanged,
    Failed,
}

fn combine_boundary_metadata(
    first: BoundaryMetadataOutcome,
    second: BoundaryMetadataOutcome,
) -> BoundaryMetadataOutcome {
    use BoundaryMetadataOutcome as O;
    match (first, second) {
        (O::Failed, _) | (_, O::Failed) => O::Failed,
        (O::Applied, _) | (_, O::Applied) => O::Applied,
        (O::Unchanged, O::Unchanged) => O::Unchanged,
    }
}

pub(crate) async fn stamp_fresh_boundary_to_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> BoundaryMetadataOutcome {
    // (#756, section 19.3) MIRROR-FIRST: if the app dies between the halves,
    // the residue (mirror=Some, record=false) forces fresh on every reopen
    // path and self-heals (E3 re-propagates; typed input or injected content
    // clears both). Record-first residue (record=true, mirror=None) would let
    // a later record destroy resurrect the pre-boundary conversation: the
    // exact #756 bug.
    let mirror = write_start_fresh_mirror_outcome(app, session_id, true).await;
    if let Some(state) = app.try_state::<crate::pty::input_activity::SubstantiveInputState>() {
        state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reset(session_id);
    }
    let manager = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let guard = mgr.read().await;
        guard.clone()
    };
    // Single-clone lookup; DUAL root predicate mirrors the restart path so the
    // two exclusions never disagree.
    let is_root = manager
        .get_session(session_id)
        .await
        .map(|s| {
            s.is_root_agent || crate::config::root_agent::is_root_agent_path(&s.working_directory)
        })
        .unwrap_or(false);
    let record = if is_root {
        BoundaryMetadataOutcome::Unchanged
    } else {
        match crate::config::sessions_persistence::set_start_fresh_and_persist_result(
            &manager, session_id,
        )
        .await
        {
            Ok(true) => {
                log::info!(
                    "[session-state] {} fresh-boundary stamped (record, #756)",
                    &session_id.to_string()[..8]
                );
                BoundaryMetadataOutcome::Applied
            }
            Ok(false) => BoundaryMetadataOutcome::Unchanged,
            Err(_) => {
                log::error!(
                    "[session-state] fresh-boundary stamp persist failed session={} code=boundary_metadata_failed",
                    session_id
                );
                BoundaryMetadataOutcome::Failed
            }
        }
    };
    combine_boundary_metadata(mirror, record)
}

/// (#756) Drop the durable fresh intent after AC successfully injected message
/// CONTENT into `session_id` (standard mailbox body, follow-up after a remote
/// command, phase-2 handoff prompts, loop prompts). The injected body creates a
/// post-boundary transcript, so provider resume becomes safe and desirable
/// again; a lingering stamp would wipe a live autonomous conversation on the
/// next reopen. DELIBERATELY NARROW: must NOT reuse
/// `note_user_message_to_session`, whose injection-exclusion protects
/// silence/badge/auto-close semantics (see its doc comment above); this helper
/// touches ONLY the fresh intent (mirror first, then record; section 19.3).
/// Never call it for bare logical-action text (/clear, Pi /new, or /compact).
pub(crate) async fn note_post_boundary_content_to_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) -> BoundaryMetadataOutcome {
    // (#756, section 19.3) MIRROR-FIRST: the drop residue (mirror=None,
    // record=true) only mis-freshes the record-alive restore until the next
    // heal; record-first residue (record=false, mirror=Some) would wrongly
    // force-fresh BOTH reopen paths.
    let mirror = write_start_fresh_mirror_outcome(app, session_id, false).await;
    let manager = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let guard = mgr.read().await;
        guard.clone()
    };
    let record = match crate::config::sessions_persistence::clear_start_fresh_and_persist_result(
        &manager, session_id,
    )
    .await
    {
        Ok(true) => {
            log::info!(
                "[session-state] {} fresh intent dropped (post-boundary content, #756)",
                &session_id.to_string()[..8]
            );
            BoundaryMetadataOutcome::Applied
        }
        Ok(false) => BoundaryMetadataOutcome::Unchanged,
        Err(_) => {
            log::error!(
                "[session-state] post-boundary-content drop persist failed session={} code=boundary_metadata_failed",
                session_id
            );
            BoundaryMetadataOutcome::Failed
        }
    };
    combine_boundary_metadata(mirror, record)
}

/// (#756) Mirror half: write the coordinator-clocks `start_fresh_at` for the
/// session's cwd. Returns false without touching anything for non-coordinators
/// (`coordinator_cwd` -> None; root agents land here too) or when the value is
/// already in the target state. Persists the clocks file immediately on a real
/// transition: these boundaries are rare and must survive an app close inside
/// the 60s flush tick (same discipline as close_coordinator's explicit save).
pub(crate) async fn write_start_fresh_mirror_for_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    on: bool,
) -> bool {
    write_start_fresh_mirror_outcome(app, session_id, on).await == BoundaryMetadataOutcome::Applied
}

async fn write_start_fresh_mirror_outcome<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    on: bool,
) -> BoundaryMetadataOutcome {
    let cwd = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let cwd = mgr.read().await.coordinator_cwd(session_id).await;
        cwd
    };
    let Some(cwd) = cwd else {
        return BoundaryMetadataOutcome::Unchanged;
    };
    let Some(clocks) = app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
    else {
        return BoundaryMetadataOutcome::Unchanged;
    };
    // agent_fqn_from_path returns String (teams.rs:80), not Option.
    let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
    let changed = {
        let mut guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            guard.mark_start_fresh(&fqn, chrono::Utc::now())
        } else {
            guard.clear_start_fresh(&fqn)
        }
    };
    if changed {
        log::info!(
            "[coordinator-clocks] start_fresh_at {} for '{}' (#756)",
            if on { "set" } else { "cleared" },
            fqn
        );
        let snapshot = { clocks.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
        if crate::config::coordinator_clocks::save_map(&snapshot).is_err() {
            log::warn!(
                "[coordinator-clocks] start_fresh_at save failed code=boundary_metadata_failed"
            );
            return BoundaryMetadataOutcome::Failed;
        }
        BoundaryMetadataOutcome::Applied
    } else {
        BoundaryMetadataOutcome::Unchanged
    }
}

#[tauri::command]
pub fn pty_resize<R: tauri::Runtime>(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    webview: tauri::Webview<R>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    // #1439: the only record of who drove a grid change; the incident was
    // diagnosable only by inference because no pty resize was ever logged.
    log::info!(
        "[pty] resize session={uuid} cols={cols} rows={rows} from={}",
        webview.label()
    );
    pty_mgr
        .lock()
        .unwrap()
        .resize(uuid, cols, rows)
        .map_err(|e| e.to_string())
}

/// #955/#956 - the backend half of the snapshot round-trip measurement.
///
/// The frontend logs the whole trip (`[terminal] snapshot <id> settled in Nms`). This logs
/// what the backend spent inside it, and between them they say WHERE the time went. Read
/// the three numbers together:
///
/// - `handler_ms` - everything this function did, including waiting for both mutexes. If
///   this is milliseconds and the frontend says seconds, the backend is not the problem.
/// - `lock_ms` - just the wait for the `PtyManager` mutex, so backend contention cannot
///   hide inside the total.
/// - **the timestamp of this line.** This is a SYNC tauri command, so it runs on the main
///   thread: a line that appears late is a request that queued before the handler ever
///   started, which is a different bug from a response that came back late. Compare it
///   against the session's `[pty] Spawned session ...` line.
///
/// Fires once per terminal attach. Never on the PTY hot path.
#[tauri::command]
pub fn get_screen_snapshot(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    session_id: String,
) -> Result<Option<PtyScreenSnapshotPayload>, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let started = Instant::now();

    let (snapshot, lock_ms) = {
        let pty_mgr = pty_mgr
            .lock()
            .map_err(|_| "PtyManager lock poisoned".to_string())?;
        let lock_ms = started.elapsed().as_secs_f64() * 1000.0;
        (pty_mgr.get_screen_snapshot(uuid), lock_ms)
    };

    log::info!(
        "[pty] screen-snapshot session={} handler_ms={:.3} lock_ms={:.3} found={} bytes={} sequence={}",
        session_id,
        started.elapsed().as_secs_f64() * 1000.0,
        lock_ms,
        snapshot.is_some(),
        snapshot.as_ref().map(|s| s.data.len()).unwrap_or(0),
        snapshot.as_ref().map(|s| s.sequence).unwrap_or(0)
    );

    let Some(snapshot) = snapshot else {
        return Ok(None);
    };

    Ok(Some(PtyScreenSnapshotPayload {
        session_id,
        data: snapshot.data,
        rows: Some(snapshot.rows),
        cols: Some(snapshot.cols),
        sequence: snapshot.sequence,
    }))
}

/// The terminal-output attach.
///
/// The window label comes from Tauri, from the calling webview, so a frontend can only ever
/// attach the window it runs in and the label can be neither forged nor misattributed.
#[tauri::command]
pub(crate) fn activate_terminal_output<R: tauri::Runtime>(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    webview: tauri::Webview<R>,
    session_id: String,
    // Optional on purpose, which is how a Tauri command argument gets a default: an
    // attribute on the parameter is not in scope here. Without the default, a missing or
    // misspelled argument makes Tauri reject the command, and the frontend fails closed with
    // no retry, so the terminal panel stays blank until the user reselects the session by
    // hand. The default is `true`, which is what the only caller sends: the client always
    // resets before it writes the seed, so a duplicate is impossible, and the failure the
    // other default produces instead is the silent content gap the plan rules to be the
    // worse of the two.
    include_history: Option<bool>,
) -> Result<Option<PtyScreenSnapshotPayload>, String> {
    let parsed = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    let route = {
        let manager = pty_mgr
            .lock()
            .map_err(|_| "PtyManager lock poisoned".to_string())?;
        manager
            .terminal_output_route(parsed)
            .map_err(|error| error.to_string())?
    };
    let snapshot = route
        .activate_terminal_output(webview.label(), include_history.unwrap_or(true))
        .map_err(|error| error.code().to_string())?;
    Ok(snapshot.map(|snapshot| PtyScreenSnapshotPayload {
        session_id,
        data: snapshot.data,
        rows: Some(snapshot.rows),
        cols: Some(snapshot.cols),
        sequence: snapshot.sequence,
    }))
}

/// Releases this window's attachment.
///
/// A session that is already gone maps to `Ok(())` on purpose: window close races session
/// destroy, so the frontend detaches a destroyed session on every normal teardown, and an
/// error there would turn routine shutdown into error spam and invite a retry. Nothing else is
/// mapped, so a poisoned lock or a genuine routing failure still surfaces.
#[tauri::command]
pub(crate) fn detach_terminal_output<R: tauri::Runtime>(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    webview: tauri::Webview<R>,
    session_id: String,
) -> Result<(), String> {
    let session_id = Uuid::parse_str(&session_id).map_err(|error| error.to_string())?;
    let route = {
        let manager = pty_mgr
            .lock()
            .map_err(|_| "PtyManager lock poisoned".to_string())?;
        match manager.terminal_output_route(session_id) {
            Ok(route) => route,
            Err(crate::errors::AppError::SessionNotFound(_)) => return Ok(()),
            Err(error) => return Err(error.to_string()),
        }
    };
    route.detach_terminal_output(webview.label());
    Ok(())
}

/// #1032 - the last context reading for a session, for a frontend that just mounted and
/// missed the `session_context` event.
///
/// `None` covers every unavailable case there is - no regex, no match, a truncated row, a
/// session that is over, a scraper that is not managed - and NEVER means 0.
#[tauri::command]
pub fn get_session_context(app: AppHandle, session_id: String) -> Result<Option<u8>, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let Some(scraper) = app.try_state::<Arc<crate::pty::context_scrape::ContextScraper>>() else {
        return Ok(None);
    };
    Ok(scraper.last_reading(uuid))
}

/// #2566 - every published weekly-quota reading, keyed by AGENT id, for a frontend that just mounted and
/// missed the `agent_quota` events. Agents sharing one command share one value. A `None` value and an ABSENT
/// key both mean unavailable, NEVER 0 and NEVER 100, and are interchangeable to the caller: an agent
/// published as null and still configured keeps a `Some(None)` row, one never published has no row, and p2
/// renders the plain chip for both. An unmanaged engine answers an EMPTY map, not an error. Generic over `R`
/// so a `tauri::test` mock app can call it, as `get_watcher_activity` does.
#[tauri::command]
pub fn get_agent_quota_readings<R: tauri::Runtime>(
    app: AppHandle<R>,
) -> Result<std::collections::HashMap<String, Option<u8>>, String> {
    let Some(engine) = app.try_state::<Arc<crate::pty::agent_quota::AgentQuotaEngine>>() else {
        return Ok(std::collections::HashMap::new());
    };
    Ok(engine.published())
}

/// #1171 - one session's watcher activity, for the window on mount and on every poll.
///
/// SYNCHRONOUS, and it takes exactly one per-session mutex. That is possible only because the
/// engine publishes `activeWatchers`, `possiblyMissedFrames` and `warmedUp` into the history at
/// the end of each tick, instead of this command resolving settings and the session manager
/// itself - which would put a read of the session lock on the window's polling path.
///
/// A session with no buffer returns an EMPTY snapshot, not `None` and not an error: the window
/// distinguishes its four states from the values here, with no nullability to reason about.
#[tauri::command]
pub fn get_watcher_activity<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    limit: Option<usize>,
) -> Result<crate::pty::watchers::history::WatcherActivitySnapshot, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let Some(history) = app.try_state::<crate::pty::watchers::history::WatcherHistoryState>()
    else {
        return Ok(crate::pty::watchers::history::WatcherActivitySnapshot::empty());
    };
    Ok(history.snapshot(uuid, limit))
}

/// #1171 - what a pattern does, before it is saved.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherPatternPreview {
    pub compiles: bool,
    pub error: Option<String>,
    /// False when no session was given, or the session had no readable frame. This is what
    /// distinguishes "matched nothing" from "could not look".
    pub sampled: bool,
    pub matched_rows: usize,
    pub total_rows: usize,
    /// Up to 3 matched logical rows, each truncated to 256 bytes.
    pub samples: Vec<String>,
    /// True when the captures of the lowest match differed between the two samples taken about
    /// a second apart. A pattern that captures a clock or a token counter matches one row of
    /// thirty and still emits five events a second in `state` mode, and `matchedRows` alone
    /// cannot say so.
    pub captures_volatile: bool,
}

/// How many matched rows the preview shows.
const WATCHER_PREVIEW_SAMPLES: usize = 3;

/// #1171 - compile a candidate pattern and, optionally, run it against a live session.
///
/// `session_id: None` compiles only. That is the COMMON case: a user opens Settings and writes
/// a regex with no agent session running, and without this the only signal for a syntax error
/// would be the absence of activations.
///
/// `async` and doing the PTY reads inside `spawn_blocking`, because this path goes through
/// `PtyManager::screen_rows_since` and therefore takes the manager mutex and the route
/// registry - "the one every terminal write, resize and kill locks on" - while a session may
/// be producing heavy output. The engine's own tick avoids both by holding its backend `Arc`;
/// a preview debounced at 300 ms can afford them, and blocking the async runtime on them could
/// not. (No child liveness probe is involved: the watcher seam deliberately has none, see
/// `local_backend.rs`'s `screen_rows_since`.)
#[tauri::command]
pub async fn preview_watcher_pattern<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: Option<String>,
    pattern: String,
) -> Result<WatcherPatternPreview, String> {
    let compiled = match crate::pty::watchers::pattern::compile(&pattern) {
        Ok(compiled) => compiled,
        Err(error) => {
            // A pattern that does not compile is a RESULT, not a command failure: the Settings
            // control needs the message to show, not an exception to swallow.
            return Ok(WatcherPatternPreview {
                compiles: false,
                error: Some(error),
                sampled: false,
                matched_rows: 0,
                total_rows: 0,
                samples: Vec::new(),
                captures_volatile: false,
            });
        }
    };

    let mut preview = WatcherPatternPreview {
        compiles: true,
        error: None,
        sampled: false,
        matched_rows: 0,
        total_rows: 0,
        samples: Vec::new(),
        captures_volatile: false,
    };

    let Some(session_id) = session_id else {
        return Ok(preview);
    };
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let Some(pty_mgr) = app.try_state::<Arc<Mutex<PtyManager>>>() else {
        return Ok(preview);
    };
    let pty_mgr = Arc::clone(pty_mgr.inner());

    let first = read_watcher_preview_frame(Arc::clone(&pty_mgr), uuid).await;
    let Some(first) = first else {
        return Ok(preview);
    };

    let logical = crate::pty::watchers::frame::logical_rows(&first);
    let regex = compiled.regex();
    let matched: Vec<&crate::pty::watchers::frame::LogicalRow> = logical
        .iter()
        .filter(|row| regex.is_match(&row.text))
        .collect();

    preview.sampled = true;
    preview.total_rows = logical.len();
    preview.matched_rows = matched.len();
    preview.samples = matched
        .iter()
        .take(WATCHER_PREVIEW_SAMPLES)
        .map(|row| crate::pty::watchers::truncate_row(&row.text).0)
        .collect();

    let lowest_captures = |rows: &[&crate::pty::watchers::frame::LogicalRow]| {
        rows.last()
            .and_then(|row| regex.captures(&row.text))
            .map(|found| {
                found
                    .iter()
                    .skip(1)
                    .map(|group| group.map(|m| m.as_str().to_string()))
                    .collect::<Vec<_>>()
            })
    };
    let before = lowest_captures(&matched);

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    if let Some(second) = read_watcher_preview_frame(pty_mgr, uuid).await {
        let logical = crate::pty::watchers::frame::logical_rows(&second);
        let matched: Vec<&crate::pty::watchers::frame::LogicalRow> = logical
            .iter()
            .filter(|row| regex.is_match(&row.text))
            .collect();
        let after = lowest_captures(&matched);
        preview.captures_volatile = before.is_some() && after.is_some() && before != after;
    }

    Ok(preview)
}

/// Read one session's frame off the async runtime.
///
/// `seen: None` on purpose: the preview always wants the rows, never an `Unchanged`.
async fn read_watcher_preview_frame(
    pty_mgr: Arc<Mutex<PtyManager>>,
    id: Uuid,
) -> Option<crate::pty::watchers::ScreenFrame> {
    tokio::task::spawn_blocking(move || {
        let mgr = pty_mgr.lock().ok()?;
        match mgr.screen_rows_since(id, None) {
            crate::pty::watchers::ScreenRowsSince::Frame(frame) => Some(frame),
            _ => None,
        }
    })
    .await
    .ok()
    .flatten()
}

/// #1171 - one watcher row of the draft the Settings modal holds in memory.
///
/// Only the three fields `reaches` and the budget depend on (plan 4.8). `pattern`, `mode`,
/// `dedupe` and `capturedAgainst` take part in neither and are deliberately not sent: the row
/// already shows its pattern, and `preview_watcher_pattern` answers compilability separately,
/// so carrying it here would inflate every debounced payload to restate an answer that is
/// already on screen next to this one.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherDraftEntry {
    pub id: String,
    pub enabled: bool,
    #[serde(default)]
    pub commands: Option<Vec<String>>,
}

/// #1171 - one agent row of the same draft.
///
/// The modal edits agents and watchers in ONE store and one Save writes both, so resolving
/// against the SAVED agent list would answer about a state the user has already left. Two of
/// the three agent edits over-report that way: deleting an agent leaves it named in a reach
/// list it will not be in, and changing an agent's `command` leaves a watcher reported as
/// reaching it under the old stem. Only adding an agent under-reports.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherAgentDraftEntry {
    pub id: String,
    pub label: String,
    pub command: String,
}

/// #1171 - one agent that a draft row's selector reaches.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherReachEntry {
    pub agent_id: String,
    pub agent_label: String,
    pub command_stem: String,
    /// Whether this row is enabled in the draft AND holds one of this agent's 8 slots once
    /// every other ENABLED row of the draft is counted. It is membership of the engine's own
    /// `running` list, and NOT a promise that the watcher will emit anything: a resolved
    /// watcher whose pattern does not compile is allocated a slot and is inert. Compilability
    /// is a separate dimension, answered per row by `preview_watcher_pattern`, and this field
    /// deliberately does not restate it. A disabled row is always false here, and the editor,
    /// which owns `enabled`, must say "disabled" rather than "budget".
    pub allocated: bool,
}

/// #1171 - the reach of one draft row.
///
/// Exactly one per requested row, in request order. It carries `id` back because the editor
/// filters unrecognised rows out of the request, so its table positions do not match the
/// response positions.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherReachRow {
    pub id: String,
    /// Every agent this row's SELECTOR reaches, whether or not the row is enabled. Reach is a
    /// property of the selector alone; `allocated` is where enablement and budget land.
    pub entries: Vec<WatcherReachEntry>,
}

/// #1171 - resolve the WHOLE Settings draft, watchers and agents, and report per row which
/// agents its selector reaches and which of those it holds a slot on.
///
/// This exists so the Settings UI reimplements neither stem normalization nor the budget rule.
/// There is exactly one stem rule in the tree (`command_executable_basename`), the catalog
/// rejects prefix matching in writing because `pi` and `agent` false-match under it, and the
/// frontend's existing `starts_with` rule in `suggestedContextRegex` must not be ported.
/// Neither the `BTreeMap` key order nor the number 8 is written a second time in TypeScript.
///
/// It takes the whole draft and not one row because allocation is a property of the SET:
/// resolution walks the map in key order and takes the first 8 that reach an agent. A command
/// receiving a single row could only answer by inventing what the rest of the set is, and the
/// only set available to it is the saved one, which is not the one the user is editing. With
/// an empty saved map, adding nine rows before Save would make all nine report that they run,
/// and then only eight would - a positive claim about a watcher that will not run, which is
/// the opposite of the fail-closed direction this feature takes everywhere else.
///
/// Both halves come from the draft and nothing comes from disk: no settings are read and **no
/// lock is taken**, so a preview can never contend with a save.
#[tauri::command]
pub async fn preview_watcher_reach(
    watchers: Vec<WatcherDraftEntry>,
    agents: Vec<WatcherAgentDraftEntry>,
) -> Result<Vec<WatcherReachRow>, String> {
    // Synchronous CPU over an input the caller controls: one pass is O(agents x total selector
    // ENTRIES), and nothing bounds either, so one row carrying ten thousand selector entries
    // costs what ten thousand rows carrying one each cost. Off the async worker, following
    // `preview_watcher_pattern` above. No cap is introduced here on purpose: the engine
    // already runs exactly this resolution over exactly this data every 200 ms, so a payload
    // big enough to make two debounced passes expensive is already costing five times as much
    // per second inside the tick, and a cap only on the preview would feel like a fix while
    // leaving the cost where it is.
    //
    // The command owns its inputs and takes no lock, so the move is a wrapper and nothing else.
    tokio::task::spawn_blocking(move || resolve_draft_reach(&watchers, &agents))
        .await
        .map_err(|e| e.to_string())
}

/// The two passes behind `preview_watcher_reach`, pure and off the runtime.
///
/// Reach and allocation are different questions, and each gets its own pass over the SAME
/// draft, so no counterfactual budget is computed and no set of answers can disagree with
/// itself:
///
/// - **Pass A, every row forced enabled**, supplies `entries`. Reach does not depend on any
///   other row, so forcing enablement changes no row's answer but its own presence, and a
///   disabled row still shows the agents its selector reaches - the state where the control is
///   needed most.
/// - **Pass B, every row at its real draft `enabled`**, is the engine's own resolution and
///   supplies `allocated`.
///
/// Running one forced-enabled pass PER ROW instead would let nine rows all report a slot on an
/// agent that has eight, because each row's own pass silently displaces a different one.
///
/// Fixed points that the editor cannot produce but the contract still defines: a duplicate id
/// means the later row wins when the map is built and both response rows report that one
/// resolution, and an empty id is a legal key that sorts first and is not special-cased.
fn resolve_draft_reach(
    watchers: &[WatcherDraftEntry],
    agents: &[WatcherAgentDraftEntry],
) -> Vec<WatcherReachRow> {
    use crate::config::settings::{WatcherConfig, WatcherEntry, WatcherMode};
    use crate::pty::watchers::{resolve_watchers, WatcherAgent};
    use std::collections::BTreeMap;

    let resolution_agents: Vec<WatcherAgent> = agents
        .iter()
        .map(|agent| WatcherAgent {
            id: agent.id.clone(),
            command: agent.command.clone(),
        })
        .collect();

    // `mode`, `pattern`, `dedupe` and `dedupeWindowMs` take no part in resolution and do not
    // travel, so they are placeholders here and are never read back out.
    let draft_map = |force_enabled: bool| -> BTreeMap<String, WatcherEntry> {
        watchers
            .iter()
            .map(|row| {
                (
                    row.id.clone(),
                    WatcherEntry::Valid(WatcherConfig {
                        enabled: force_enabled || row.enabled,
                        mode: WatcherMode::Occurrence,
                        pattern: String::new(),
                        commands: row.commands.clone(),
                        dedupe: Default::default(),
                        dedupe_window_ms: 0,
                        captured_against: None,
                    }),
                )
            })
            .collect()
    };

    let (reach_pass, _notices) = resolve_watchers(&resolution_agents, &draft_map(true));
    let (allocation_pass, _notices) = resolve_watchers(&resolution_agents, &draft_map(false));

    // Keyed by agent id and not iterated straight off `agents`, so a draft that names the same
    // agent twice produces one entry rather than two, matching the map `resolve_watchers`
    // itself builds.
    let mut agent_meta: BTreeMap<String, (String, String)> = BTreeMap::new();
    for agent in agents {
        let stem =
            crate::config::coding_agents_catalog::command_executable_basename(&agent.command)
                .unwrap_or_default();
        agent_meta.insert(agent.id.clone(), (agent.label.clone(), stem));
    }

    let mut rows = Vec::with_capacity(watchers.len());
    for row in watchers {
        let mut entries: Vec<WatcherReachEntry> = Vec::new();
        for (agent_id, (label, stem)) in &agent_meta {
            // Reach is `running` OR `over_budget`: together they hold everything whose selector
            // matches this agent.
            let reaches = reach_pass.get(agent_id).is_some_and(|resolution| {
                resolution.running.iter().any(|w| w.id == row.id)
                    || resolution.over_budget.iter().any(|id| id == &row.id)
            });
            if !reaches {
                continue;
            }
            let allocated = allocation_pass
                .get(agent_id)
                .is_some_and(|resolution| resolution.running.iter().any(|w| w.id == row.id));
            entries.push(WatcherReachEntry {
                agent_id: agent_id.clone(),
                agent_label: label.clone(),
                command_stem: stem.clone(),
                allocated,
            });
        }
        // `resolve_watchers` returns a `HashMap`, so a stable order has to be imposed here
        // rather than assumed: the Settings list must not reshuffle between keystrokes.
        entries.sort_by(|left, right| {
            left.agent_label
                .cmp(&right.agent_label)
                .then_with(|| left.agent_id.cmp(&right.agent_id))
        });
        rows.push(WatcherReachRow {
            id: row.id.clone(),
            entries,
        });
    }
    rows
}

#[cfg(test)]
mod tests;
