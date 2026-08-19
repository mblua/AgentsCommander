use std::sync::{Arc, Mutex};
use std::time::Instant;

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
            return Err("purge-wg in progress for this session; input rejected".to_string());
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
            return Err("purge-wg in progress for this session; input rejected".to_string());
        }
    }
    PtyManager::write_with_permit(&permit, &data).map_err(|error| error.to_string())?;
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
        idle.record_activity_with_bytes(session_id, byte_count);
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
pub fn pty_resize(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
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
mod watcher_preview_tests {
    use super::*;
    use crate::config::settings::{AppSettings, SettingsState};
    use crate::errors::AppError;
    use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
    use crate::pty::watchers::{FrameStamp, ScreenFrame, ScreenRowsSince};

    /// One watcher row of the Settings draft. `preview_watcher_reach` takes the whole draft,
    /// so every reach test below builds a set and never a single row.
    fn draft(id: &str, enabled: bool, commands: Option<&[&str]>) -> WatcherDraftEntry {
        WatcherDraftEntry {
            id: id.to_string(),
            enabled,
            commands: commands.map(|list| list.iter().map(|s| s.to_string()).collect()),
        }
    }

    fn draft_agent(id: &str, label: &str, command: &str) -> WatcherAgentDraftEntry {
        WatcherAgentDraftEntry {
            id: id.to_string(),
            label: label.to_string(),
            command: command.to_string(),
        }
    }

    /// The row of `id` in a reach response, which is addressed by id and never by position.
    fn row<'a>(rows: &'a [WatcherReachRow], id: &str) -> &'a WatcherReachRow {
        rows.iter()
            .find(|row| row.id == id)
            .unwrap_or_else(|| panic!("no reach row for '{id}'"))
    }

    fn allocated_on(rows: &[WatcherReachRow], id: &str, agent_id: &str) -> bool {
        row(rows, id)
            .entries
            .iter()
            .find(|entry| entry.agent_id == agent_id)
            .unwrap_or_else(|| panic!("watcher '{id}' does not reach agent '{agent_id}'"))
            .allocated
    }

    fn reached_agents(rows: &[WatcherReachRow], id: &str) -> Vec<String> {
        row(rows, id)
            .entries
            .iter()
            .map(|entry| entry.agent_id.clone())
            .collect()
    }

    fn settings_app(settings: AppSettings) -> tauri::App<tauri::test::MockRuntime> {
        let state: SettingsState = Arc::new(tokio::sync::RwLock::new(settings));
        tauri::test::mock_builder()
            .manage(state)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build a watcher preview test app")
    }

    /// A backend that paints one fixed screen and refuses everything else.
    struct FixedScreenBackend {
        rows: Vec<String>,
    }

    impl PtyBackend for FixedScreenBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn spawn(
            &self,
            _spec: BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
            Box::pin(async { Ok(()) })
        }
        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            _id: Uuid,
            _data: &[u8],
        ) -> Result<(), AppError> {
            unreachable!("a preview must never write to a PTY")
        }
        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
            unreachable!("a preview must never resize a PTY")
        }
        fn kill(&self, _id: Uuid) -> Result<(), AppError> {
            unreachable!("a preview must never kill a session")
        }
        fn has_session(&self, _id: Uuid) -> bool {
            true
        }
        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }
        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }
        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::Unavailable
        }
        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }
        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }
        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
        fn screen_rows_since(&self, _id: Uuid, _seen: Option<FrameStamp>) -> ScreenRowsSince {
            ScreenRowsSince::Frame(ScreenFrame {
                rows: self.rows.clone(),
                wrapped: vec![false; self.rows.len()],
                cursor_row: 0,
                stamp: Some(FrameStamp {
                    sequence: 1,
                    rows: self.rows.len() as u16,
                    cols: 120,
                }),
            })
        }
    }

    fn pty_app(rows: &[&str]) -> (tauri::App<tauri::test::MockRuntime>, Uuid) {
        let backend = Arc::new(FixedScreenBackend {
            rows: rows.iter().map(|r| r.to_string()).collect(),
        });
        let manager = PtyManager::new_for_test(backend);
        let id = Uuid::new_v4();
        manager.record_route(id, SessionBackendKind::LocalProcess);
        let app = tauri::test::mock_builder()
            .manage(Arc::new(Mutex::new(manager)))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build a pty preview test app");
        (app, id)
    }

    /// 9.5.65 - a `session_id` that is not a UUID is an error, exactly like
    /// `get_session_context`. Everything else about this command answers with values.
    #[test]
    fn a_session_id_that_is_not_a_uuid_is_rejected() {
        let app = settings_app(AppSettings::default());

        assert!(get_watcher_activity(app.handle().clone(), "nope".into(), None).is_err());
    }

    /// 9.5.64 - with no history managed at all - a test app, a build without the engine - the
    /// command answers with the EMPTY snapshot rather than an error. `warmedUp: false` is what
    /// tells the window it is looking at a session the engine has not reached, so it shows a
    /// neutral starting state instead of "no watcher reaches this agent".
    #[test]
    fn an_unmanaged_history_answers_with_the_empty_snapshot() {
        let app = settings_app(AppSettings::default());

        let snapshot = get_watcher_activity(app.handle().clone(), Uuid::new_v4().to_string(), None)
            .expect("never an error");

        assert!(snapshot.matches.is_empty());
        assert!(!snapshot.warmed_up);
        assert!(!snapshot.truncated);
        assert_eq!(snapshot.last_seq, 0);
        assert_eq!(snapshot.possibly_missed_frames, 0);
        assert!(snapshot.active_watchers.is_empty());
    }

    /// 9.5.63 - the command hands `limit` straight to the ring, which trims from the NEW end
    /// and keeps the order.
    #[test]
    fn the_command_reads_the_ring_and_honours_the_limit() {
        use crate::pty::watchers::history::{SessionStatus, WatcherHistory, WatcherHistoryState};
        use crate::pty::watchers::WatcherMatchPayload;

        let id = Uuid::new_v4();
        let history: WatcherHistoryState = Arc::new(WatcherHistory::default());
        history.publish(id, SessionStatus::default());
        for seq in 1..=5u64 {
            history.record(
                id,
                &[WatcherMatchPayload {
                    session_id: id.to_string(),
                    seq,
                    watcher_id: "w".to_string(),
                    mode: crate::pty::watchers::WatcherMode::Occurrence,
                    at: chrono::Utc::now(),
                    captures: Vec::new(),
                    row: format!("row {seq}"),
                    row_truncated: false,
                }],
            );
        }
        let app = tauri::test::mock_builder()
            .manage(history)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build a watcher activity test app");

        let all = get_watcher_activity(app.handle().clone(), id.to_string(), None).expect("all");
        assert_eq!(all.matches.len(), 5);
        assert!(all.warmed_up);

        let recent =
            get_watcher_activity(app.handle().clone(), id.to_string(), Some(2)).expect("recent");
        assert_eq!(
            recent.matches.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![4, 5]
        );
        assert_eq!(recent.last_seq, 5);
    }

    /// 9.6.86 (first case) - **the common case**: a user writes a regex in Settings with no
    /// agent session running. It compiles and says so, and says explicitly that it did not
    /// look at anything - which is not the same as having looked and found nothing.
    #[tokio::test]
    async fn a_compile_only_preview_reports_compiles_without_sampling() {
        let app = settings_app(AppSettings::default());

        let preview = preview_watcher_pattern(app.handle().clone(), None, r"Read \((.+)\)".into())
            .await
            .expect("compile-only preview never fails");

        assert!(preview.compiles);
        assert!(preview.error.is_none());
        assert!(!preview.sampled);
        assert_eq!((preview.matched_rows, preview.total_rows), (0, 0));
        assert!(preview.samples.is_empty());
        assert!(!preview.captures_volatile);
    }

    /// 9.6.86 (fourth case) - a pattern that does not compile is a RESULT, not a command
    /// failure. The Settings control needs the message to show the user.
    #[tokio::test]
    async fn an_uncompilable_pattern_returns_a_result_rather_than_an_error() {
        let app = settings_app(AppSettings::default());

        let preview = preview_watcher_pattern(app.handle().clone(), None, r"Read \((.+".into())
            .await
            .expect("a bad pattern must not fail the command");

        assert!(!preview.compiles);
        assert!(preview.error.is_some());
        assert!(!preview.sampled);
    }

    /// 9.6.86 (second case) - with a live session, the preview reports matched rows against
    /// total LOGICAL rows and shows at most three of them.
    #[tokio::test]
    async fn a_preview_against_a_live_session_reports_matches_against_total_rows() {
        let (app, id) = pty_app(&[
            "Read (a.rs)",
            "idle",
            "Read (b.rs)",
            "Read (c.rs)",
            "Read (d.rs)",
        ]);

        let preview = preview_watcher_pattern(
            app.handle().clone(),
            Some(id.to_string()),
            r"Read \((.+)\)".into(),
        )
        .await
        .expect("preview");

        assert!(preview.sampled);
        assert_eq!(preview.total_rows, 5);
        assert_eq!(preview.matched_rows, 4);
        assert_eq!(preview.samples.len(), WATCHER_PREVIEW_SAMPLES);
        assert_eq!(preview.samples[0], "Read (a.rs)");
        assert!(
            !preview.captures_volatile,
            "the same screen twice cannot be volatile"
        );
    }

    /// 9.6.87 - **the pattern that looks fine and is not.** A regex capturing a clock matches
    /// one row of thirty, so `matchedRows` says nothing is wrong, and in `state` mode it emits
    /// five events a second forever. The two samples a second apart are what catch it.
    ///
    /// Takes about a second in real time by construction: the interval between the samples IS
    /// the measurement.
    #[tokio::test]
    async fn a_pattern_capturing_a_clock_is_reported_as_volatile() {
        struct TickingClockBackend {
            reads: std::sync::atomic::AtomicUsize,
        }

        impl PtyBackend for TickingClockBackend {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn spawn(
                &self,
                _spec: BackendSpawnSpec,
            ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
                Box::pin(async { Ok(()) })
            }
            fn write(
                &self,
                _authority: &crate::pty::manager::BackendWriteAuthority,
                _id: Uuid,
                _data: &[u8],
            ) -> Result<(), AppError> {
                unreachable!("a preview must never write to a PTY")
            }
            fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
                unreachable!("a preview must never resize a PTY")
            }
            fn kill(&self, _id: Uuid) -> Result<(), AppError> {
                unreachable!("a preview must never kill a session")
            }
            fn has_session(&self, _id: Uuid) -> bool {
                true
            }
            fn get_screen_snapshot(
                &self,
                _id: Uuid,
            ) -> Option<crate::pty::output::PtyScreenSnapshot> {
                None
            }
            fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
                None
            }
            fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
                crate::pty::context_scrape::ScreenRowsRead::Unavailable
            }
            fn register_response_watcher(
                &self,
                _session_id: Uuid,
                _request_id: String,
                _response_dir: std::path::PathBuf,
            ) {
            }
            fn terminate_job_for_session(&self, _id: Uuid) -> bool {
                false
            }
            fn kill_all_jobs(&self) -> (usize, usize) {
                (0, 0)
            }
            fn screen_rows_since(&self, _id: Uuid, _seen: Option<FrameStamp>) -> ScreenRowsSince {
                let tick = self
                    .reads
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let rows = vec!["idle".to_string(), format!("elapsed 00:0{tick}")];
                ScreenRowsSince::Frame(ScreenFrame {
                    wrapped: vec![false; rows.len()],
                    cursor_row: 0,
                    stamp: Some(FrameStamp {
                        sequence: tick as u64 + 1,
                        rows: rows.len() as u16,
                        cols: 120,
                    }),
                    rows,
                })
            }
        }

        let backend = Arc::new(TickingClockBackend {
            reads: std::sync::atomic::AtomicUsize::new(0),
        });
        let manager = PtyManager::new_for_test(backend);
        let id = Uuid::new_v4();
        manager.record_route(id, SessionBackendKind::LocalProcess);
        let app = tauri::test::mock_builder()
            .manage(Arc::new(Mutex::new(manager)))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build a clock preview test app");

        let preview = preview_watcher_pattern(
            app.handle().clone(),
            Some(id.to_string()),
            r"elapsed (\d\d:\d\d)".into(),
        )
        .await
        .expect("preview");

        assert!(preview.sampled);
        assert_eq!(
            preview.matched_rows, 1,
            "one row of two: nothing looks wrong"
        );
        assert!(
            preview.captures_volatile,
            "...and yet in state mode this would emit five events a second, forever"
        );
    }

    /// 9.6.86 (third case) - a session that cannot be read reports `sampled: false` and KEEPS
    /// the compile result. "Could not look" must never read as "looked and found nothing".
    #[tokio::test]
    async fn a_session_that_cannot_be_read_keeps_the_compile_result_and_does_not_claim_a_sample() {
        let app = settings_app(AppSettings::default());

        let preview = preview_watcher_pattern(
            app.handle().clone(),
            Some(Uuid::new_v4().to_string()),
            "Read".into(),
        )
        .await
        .expect("preview");

        assert!(preview.compiles);
        assert!(!preview.sampled);
    }

    #[tokio::test]
    async fn a_session_id_that_is_not_a_uuid_is_an_error() {
        let app = settings_app(AppSettings::default());

        assert!(preview_watcher_pattern(
            app.handle().clone(),
            Some("not-a-uuid".into()),
            "x".into()
        )
        .await
        .is_err());
    }

    /// 9.4.58a - nine enabled selectorless rows against one agent: the first eight in ID order
    /// hold a slot and the ninth does not, with nothing on disk contributing.
    ///
    /// The rows are sent in an order deliberately different from lexicographic, so an
    /// implementation that honours request order instead of `BTreeMap` key order fails here.
    /// Its failure mode is the UI telling the user that a watcher holds a slot it does not.
    #[tokio::test]
    async fn a_draft_of_nine_rows_allocates_the_first_eight_in_id_order() {
        let ids = ["w3", "w9", "w1", "w7", "w5", "w2", "w8", "w4", "w6"];
        let rows = preview_watcher_reach(
            ids.iter().map(|id| draft(id, true, None)).collect(),
            vec![draft_agent("a1", "Claude", "claude")],
        )
        .await
        .expect("reach");

        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ids,
            "one response row per request row, in request order"
        );
        for id in ["w1", "w2", "w3", "w4", "w5", "w6", "w7", "w8"] {
            assert!(allocated_on(&rows, id, "a1"), "{id} is within the budget");
        }
        assert!(
            !allocated_on(&rows, "w9", "a1"),
            "w9 sorts ninth and the agent has eight slots"
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.entries.iter().any(|entry| entry.allocated))
                .count(),
            crate::pty::watchers::WATCHERS_PER_AGENT_BUDGET,
            "no draft yields more allocated rows on one agent than that agent has slots"
        );
    }

    /// 9.4.58b - the draft is the whole input. A row the user deleted contributes nothing to
    /// any agent's budget, however many rows the saved map still holds.
    #[tokio::test]
    async fn rows_absent_from_the_draft_consume_no_budget() {
        let rows = preview_watcher_reach(
            ["w1", "w2", "w3", "w4"]
                .iter()
                .map(|id| draft(id, true, None))
                .collect(),
            vec![draft_agent("a1", "Claude", "claude")],
        )
        .await
        .expect("reach");

        assert_eq!(rows.len(), 4);
        for id in ["w1", "w2", "w3", "w4"] {
            assert!(allocated_on(&rows, id, "a1"));
        }
    }

    /// 9.4.58c - the displacement fixture: one disabled row plus eight enabled ones.
    ///
    /// This is the case a per-row forced-enabled pass gets wrong. There, `a`'s own pass
    /// displaced `i` while every other row's pass displaced nobody, so all nine reported
    /// themselves in budget on an agent with eight slots.
    #[tokio::test]
    async fn a_disabled_row_reaches_everything_and_allocates_nothing() {
        let mut watchers = vec![draft("a", false, None)];
        for id in ["b", "c", "d", "e", "f", "g", "h", "i"] {
            watchers.push(draft(id, true, None));
        }
        let rows = preview_watcher_reach(watchers, vec![draft_agent("a1", "Claude", "claude")])
            .await
            .expect("reach");

        assert_eq!(
            reached_agents(&rows, "a"),
            vec!["a1"],
            "a disabled row still reports the agents its selector reaches"
        );
        assert!(
            !allocated_on(&rows, "a", "a1"),
            "a disabled row holds no slot, and the editor must call that 'disabled', not 'budget'"
        );
        for id in ["b", "c", "d", "e", "f", "g", "h", "i"] {
            assert!(
                allocated_on(&rows, id, "a1"),
                "{id} is one of the eight enabled rows and the disabled row displaces none of them"
            );
        }
        assert_eq!(
            rows.iter()
                .filter(|row| row.entries.iter().any(|entry| entry.allocated))
                .count(),
            crate::pty::watchers::WATCHERS_PER_AGENT_BUDGET
        );
    }

    /// 9.4.58d - reach does not depend on enablement. The `entries` of a row are identical
    /// whether it is enabled or disabled, all else equal; only `allocated` moves.
    #[tokio::test]
    async fn reach_does_not_depend_on_enablement() {
        let agents = vec![
            draft_agent("a1", "Claude", "claude"),
            draft_agent("a2", "Codex", "codex"),
        ];
        let others = || {
            vec![
                draft("w1", true, Some(&["claude"])),
                draft("w2", true, None),
            ]
        };

        let mut enabled = others();
        enabled.push(draft("w3", true, Some(&["claude", "codex"])));
        let enabled = preview_watcher_reach(enabled, agents.clone())
            .await
            .expect("reach");

        let mut disabled = others();
        disabled.push(draft("w3", false, Some(&["claude", "codex"])));
        let disabled = preview_watcher_reach(disabled, agents)
            .await
            .expect("reach");

        assert_eq!(reached_agents(&enabled, "w3"), vec!["a1", "a2"]);
        assert_eq!(
            reached_agents(&disabled, "w3"),
            reached_agents(&enabled, "w3")
        );
        assert!(allocated_on(&enabled, "w3", "a1"));
        assert!(!allocated_on(&disabled, "w3", "a1"));
        assert_eq!(
            reached_agents(&disabled, "w1"),
            reached_agents(&enabled, "w1"),
            "and no other row's reach moved either"
        );
    }

    /// 9.4.58e - the selector rules, unchanged: absent reaches every agent, `[]` reaches none,
    /// a selector that does not tokenize skips the whole watcher, and the reported stem is the
    /// AGENT's. In both reach-nobody cases the response row is still present, with no entries.
    #[tokio::test]
    async fn the_selector_rules_survive_the_draft_shape() {
        let rows = preview_watcher_reach(
            vec![
                draft("all", true, None),
                draft("none", true, Some(&[])),
                draft("broken", true, Some(&["claude", "   "])),
                draft("claude-only", true, Some(&["claude"])),
                draft("typo", true, Some(&["gemni"])),
            ],
            vec![
                draft_agent("a1", "Claude", "claude"),
                draft_agent("a2", "Claude Sandbox", r"C:\rt\claude.cmd"),
                draft_agent("a3", "Codex", "codex"),
                draft_agent("a4", "Pi via Claude", "pi --provider claude"),
            ],
        )
        .await
        .expect("reach");

        assert_eq!(reached_agents(&rows, "all"), vec!["a1", "a2", "a3", "a4"]);
        assert!(
            row(&rows, "none").entries.is_empty(),
            "an empty selector reaches nobody and is the opposite of an absent one"
        );
        assert!(
            row(&rows, "broken").entries.is_empty(),
            "one unreadable selector entry skips the WHOLE watcher, never 'reaches everybody'"
        );
        assert!(row(&rows, "typo").entries.is_empty());
        assert_eq!(reached_agents(&rows, "claude-only"), vec!["a1", "a2"]);

        let claude_only = &row(&rows, "claude-only").entries;
        assert_eq!(claude_only[0].agent_label, "Claude");
        assert_eq!(
            claude_only[1].command_stem, "claude",
            "the stem reported is the agent's, resolved through the one rule in the tree"
        );
    }

    /// 9.4.58f - the agents come from the draft too. Each of these three is a case where
    /// resolving against the saved agent list would have answered about a state the user had
    /// already left, and two of the three would have over-reported.
    #[tokio::test]
    async fn the_agent_half_of_the_draft_decides_the_reach() {
        let watchers = || {
            vec![
                draft("on-claude", true, Some(&["claude"])),
                draft("on-codex", true, Some(&["codex"])),
            ]
        };

        let before = preview_watcher_reach(
            watchers(),
            vec![
                draft_agent("a1", "Claude", "claude"),
                draft_agent("a2", "Second", "claude"),
            ],
        )
        .await
        .expect("reach");
        assert_eq!(reached_agents(&before, "on-claude"), vec!["a1", "a2"]);
        assert!(row(&before, "on-codex").entries.is_empty());

        // The command changed in the draft: the agent leaves one watcher and joins the other.
        let retargeted = preview_watcher_reach(
            watchers(),
            vec![
                draft_agent("a1", "Claude", "claude"),
                draft_agent("a2", "Second", "codex"),
            ],
        )
        .await
        .expect("reach");
        assert_eq!(reached_agents(&retargeted, "on-claude"), vec!["a1"]);
        assert_eq!(reached_agents(&retargeted, "on-codex"), vec!["a2"]);

        // The agent was deleted in the draft: it is named by nobody.
        let removed =
            preview_watcher_reach(watchers(), vec![draft_agent("a1", "Claude", "claude")])
                .await
                .expect("reach");
        assert_eq!(reached_agents(&removed, "on-claude"), vec!["a1"]);

        // The agent was added in the draft: it is reached before it is ever saved.
        let added = preview_watcher_reach(
            watchers(),
            vec![
                draft_agent("a1", "Claude", "claude"),
                draft_agent("a2", "Second", "claude"),
                draft_agent("a3", "Third", "codex"),
            ],
        )
        .await
        .expect("reach");
        assert_eq!(reached_agents(&added, "on-claude"), vec!["a1", "a2"]);
        assert_eq!(reached_agents(&added, "on-codex"), vec!["a3"]);
    }

    /// 9.4.58j - the fixed points of the contract that the editor cannot produce but the
    /// command still has to define: duplicate ids, an empty id, response order, entry order.
    #[tokio::test]
    async fn the_defined_behavior_for_drafts_the_editor_cannot_produce() {
        let rows = preview_watcher_reach(
            vec![
                draft("dup", true, Some(&["codex"])),
                draft("", true, None),
                draft("dup", true, Some(&["claude"])),
            ],
            vec![
                // Labels out of alphabetical order and one pair sharing a label, so the
                // entry sort is exercised on both keys.
                draft_agent("a2", "Zed", "claude"),
                draft_agent("a3", "Alpha", "claude"),
                draft_agent("a1", "Alpha", "claude"),
            ],
        )
        .await
        .expect("reach");

        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec!["dup", "", "dup"],
            "one response row per request row, in request order, id carried back"
        );
        assert_eq!(
            reached_agents(&rows, "dup"),
            vec!["a1", "a3", "a2"],
            "entries sort by label with the agent id as tie-break"
        );
        assert_eq!(
            rows[0].entries.len(),
            rows[2].entries.len(),
            "a duplicate id is later-wins in the map and BOTH response rows report that one \
             resolution"
        );
        assert_eq!(reached_agents(&rows, ""), vec!["a1", "a3", "a2"]);
        assert!(
            allocated_on(&rows, "", "a1"),
            "an empty id is a legal key that sorts first, and is not special-cased"
        );
    }

    /// 9.4.58l - allocation is slot assignment and not a promise of output.
    ///
    /// The pattern does not travel to the reach command at all, so a row whose regex does not
    /// compile is allocated a slot and is inert. The two dimensions are asserted together, so
    /// nobody later reads `allocated` as a promise that the watcher will emit.
    #[tokio::test]
    async fn an_uncompilable_pattern_is_allocated_and_inert() {
        let app = settings_app(AppSettings::default());

        let rows = preview_watcher_reach(
            vec![draft("broken-regex", true, None)],
            vec![draft_agent("a1", "Claude", "claude")],
        )
        .await
        .expect("reach");
        assert!(
            allocated_on(&rows, "broken-regex", "a1"),
            "an enabled row within budget holds its slot whatever its pattern is"
        );

        let compile = preview_watcher_pattern(app.handle().clone(), None, "Read (".to_string())
            .await
            .expect("preview");
        assert!(
            !compile.compiles,
            "and the other dimension is answered next to it, by the row's own pattern preview"
        );
        assert!(compile.error.is_some());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::config::coordinator_clocks::{CoordinatorClocks, CoordinatorClocksState};
    use crate::session::manager::SessionManager;
    use crate::session::session::SessionRepo;

    struct FreshIntentFixture {
        app: tauri::App<tauri::test::MockRuntime>,
        session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
        clocks: CoordinatorClocksState,
        session_id: Uuid,
        fqn: String,
    }

    fn user_input_test_app(
        session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
        clocks: CoordinatorClocksState,
    ) -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .manage(session_mgr)
            .manage(clocks)
            .manage(crate::pty::input_activity::new_state())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build user input test app")
    }

    async fn fresh_intent_fixture() -> FreshIntentFixture {
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let clocks = Arc::new(Mutex::new(CoordinatorClocks::default()));
        let app = user_input_test_app(session_mgr.clone(), clocks.clone());
        let cwd = "C:/ac-test/project/.ac/wg-871-dev-team/__agent_tech-lead".to_string();
        let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
        let session = {
            let mgr = session_mgr.read().await;
            mgr.create_session(
                "codex".to_string(),
                Vec::new(),
                cwd,
                None,
                None,
                Vec::<SessionRepo>::new(),
                true,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create coordinator session")
        };

        {
            let mgr = session_mgr.read().await;
            mgr.set_start_fresh_on_restore(session.id, true).await;
        }
        {
            let mut guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
            assert!(guard.mark_start_fresh(&fqn, chrono::Utc::now()));
        }

        FreshIntentFixture {
            app,
            session_mgr,
            clocks,
            session_id: session.id,
            fqn,
        }
    }

    async fn record_fresh(f: &FreshIntentFixture) -> bool {
        let mgr = f.session_mgr.read().await;
        mgr.get_session(f.session_id)
            .await
            .expect("session should exist")
            .start_fresh_on_restore
    }

    fn mirror_fresh(f: &FreshIntentFixture) -> bool {
        f.clocks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .start_fresh_at(&f.fqn)
            .is_some()
    }

    fn inject_continue_after_restore(start_fresh_on_restore: bool) -> bool {
        !start_fresh_on_restore
    }

    #[tokio::test]
    async fn restart_non_substantive_terminal_writes_keep_restore_fresh() {
        let f = fresh_intent_fixture().await;
        for chunk in [
            b"\x1b[I".as_slice(),
            b"\x1b[A".as_slice(),
            b"\x1b]11;rgb:1234/5678/9abc\x07".as_slice(),
            b"\r".as_slice(),
        ] {
            note_user_message_to_session(
                f.app.handle(),
                f.session_id,
                UserInputSource::Terminal(chunk),
            )
            .await;
            assert!(record_fresh(&f).await);
            assert!(mirror_fresh(&f));
        }

        assert!(!inject_continue_after_restore(record_fresh(&f).await));
    }

    #[tokio::test]
    async fn restart_substantive_terminal_prompt_allows_resume_on_restore() {
        let f = fresh_intent_fixture().await;
        note_user_message_to_session(
            f.app.handle(),
            f.session_id,
            UserInputSource::Terminal(b"do the thing\r"),
        )
        .await;

        assert!(!record_fresh(&f).await);
        assert!(!mirror_fresh(&f));
        assert!(inject_continue_after_restore(record_fresh(&f).await));
    }

    #[test]
    fn boundary_metadata_failure_dominates_applied_and_unchanged_outcomes() {
        use BoundaryMetadataOutcome as O;
        assert_eq!(
            combine_boundary_metadata(O::Applied, O::Unchanged),
            O::Applied
        );
        assert_eq!(
            combine_boundary_metadata(O::Unchanged, O::Unchanged),
            O::Unchanged
        );
        assert_eq!(combine_boundary_metadata(O::Failed, O::Applied), O::Failed);
        assert_eq!(combine_boundary_metadata(O::Applied, O::Failed), O::Failed);
    }

    #[tokio::test]
    async fn restart_injected_body_allows_resume_on_restore() {
        let f = fresh_intent_fixture().await;
        note_post_boundary_content_to_session(f.app.handle(), f.session_id).await;

        assert!(!record_fresh(&f).await);
        assert!(!mirror_fresh(&f));
        assert!(inject_continue_after_restore(record_fresh(&f).await));
    }

    #[tokio::test]
    async fn ctrl_c_cancelled_terminal_line_keeps_restore_fresh() {
        let f = fresh_intent_fixture().await;
        for chunk in [
            b"do the thing".as_slice(),
            b"\x03".as_slice(),
            b"\r".as_slice(),
        ] {
            note_user_message_to_session(
                f.app.handle(),
                f.session_id,
                UserInputSource::Terminal(chunk),
            )
            .await;
        }

        assert!(record_fresh(&f).await);
        assert!(mirror_fresh(&f));
        assert!(!inject_continue_after_restore(record_fresh(&f).await));
    }
}
