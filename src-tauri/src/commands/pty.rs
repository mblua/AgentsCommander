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
/// `async` and doing the PTY reads inside `spawn_blocking`, because this path deliberately
/// goes through `PtyManager::screen_rows_since` - taking the manager mutex, the route registry
/// and, on the local backend, a child liveness probe - while a session may be producing heavy
/// output. The engine's own tick avoids all of that; a debounced preview can afford it, and
/// blocking the async runtime on it could not.
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

/// #1171 - which agents a watcher reaches, and which of them it is out of budget for.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherReachEntry {
    pub agent_id: String,
    pub agent_label: String,
    pub command_stem: String,
    /// False when this watcher reaches the agent but falls outside its 8-watcher budget.
    pub in_budget: bool,
}

/// #1171 - resolve a CANDIDATE selector against the configured agents.
///
/// This exists so the Settings UI never reimplements stem normalization. There is exactly one
/// stem rule in the tree (`command_executable_basename`), the catalog rejects prefix matching
/// in writing because `pi` and `agent` false-match under it, and the frontend's existing
/// `starts_with` rule in `suggestedContextRegex` must not be ported. Asking the backend is the
/// only way to keep that true.
///
/// The candidate replaces the saved entry of the same id before resolving, so the budget is
/// computed against the map the user is about to save rather than the one on disk.
#[tauri::command]
pub async fn preview_watcher_reach(
    settings: State<'_, crate::config::settings::SettingsState>,
    watcher_id: String,
    commands: Option<Vec<String>>,
) -> Result<Vec<WatcherReachEntry>, String> {
    use crate::config::settings::{WatcherConfig, WatcherEntry, WatcherMode};
    use crate::pty::watchers::{resolve_watchers, WatcherAgent};

    let settings = settings.read().await;
    let agents: Vec<WatcherAgent> = settings
        .agents
        .iter()
        .map(|agent| WatcherAgent {
            id: agent.id.clone(),
            command: agent.command.clone(),
        })
        .collect();

    // Keep whatever the saved entry already says, and override only the selector: a preview
    // must not silently re-enable a disabled watcher or change its mode.
    let mut candidate = match settings.watchers.get(&watcher_id).and_then(|e| e.valid()) {
        Some(saved) => saved.clone(),
        None => WatcherConfig {
            enabled: true,
            mode: WatcherMode::Occurrence,
            pattern: String::new(),
            commands: None,
            dedupe: Default::default(),
            dedupe_window_ms: 2000,
            captured_against: None,
        },
    };
    candidate.commands = commands;
    candidate.enabled = true;

    let mut watchers = settings.watchers.clone();
    watchers.insert(watcher_id.clone(), WatcherEntry::Valid(candidate));
    let labels: std::collections::HashMap<&str, (&str, &str)> = settings
        .agents
        .iter()
        .map(|agent| {
            (
                agent.id.as_str(),
                (agent.label.as_str(), agent.command.as_str()),
            )
        })
        .collect();

    let (resolved, _notices) = resolve_watchers(&agents, &watchers);

    let mut out: Vec<WatcherReachEntry> = Vec::new();
    for (agent_id, resolution) in &resolved {
        let running = resolution.running.iter().any(|w| w.id == watcher_id);
        let over_budget = resolution.over_budget.iter().any(|id| id == &watcher_id);
        if !running && !over_budget {
            continue;
        }
        let (label, command) = labels.get(agent_id.as_str()).copied().unwrap_or(("", ""));
        out.push(WatcherReachEntry {
            agent_id: agent_id.clone(),
            agent_label: label.to_string(),
            command_stem: crate::config::coding_agents_catalog::command_executable_basename(
                command,
            )
            .unwrap_or_default(),
            in_budget: running,
        });
    }
    // `resolve_watchers` returns a HashMap, so a stable order has to be imposed here rather
    // than assumed: the Settings list must not reshuffle between keystrokes.
    out.sort_by(|left, right| {
        left.agent_label
            .cmp(&right.agent_label)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    Ok(out)
}

#[cfg(test)]
mod watcher_preview_tests {
    use super::*;
    use crate::config::settings::{
        AgentConfig, AppSettings, SettingsState, WatcherConfig, WatcherEntry, WatcherMode,
    };
    use crate::errors::AppError;
    use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
    use crate::pty::watchers::{FrameStamp, ScreenFrame, ScreenRowsSince};

    fn agent(id: &str, label: &str, command: &str) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            label: label.to_string(),
            command: command.to_string(),
            color: "#fff".to_string(),
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            backend: Default::default(),
        }
    }

    fn watcher(commands: Option<&[&str]>) -> WatcherEntry {
        WatcherEntry::Valid(WatcherConfig {
            enabled: true,
            mode: WatcherMode::Occurrence,
            pattern: "x".to_string(),
            commands: commands.map(|list| list.iter().map(|s| s.to_string()).collect()),
            dedupe: Default::default(),
            dedupe_window_ms: 2000,
            captured_against: None,
        })
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

    /// 9.4.58 - `preview_watcher_reach` names the agents a selector reaches, resolving stems
    /// through the ONE rule in the tree rather than a second one written in TypeScript.
    #[tokio::test]
    async fn preview_reach_reports_the_agents_a_selector_reaches() {
        let app = settings_app(AppSettings {
            agents: vec![
                agent("a1", "Claude", "claude"),
                agent("a2", "Claude Sandbox", r"C:\rt\claude.cmd"),
                agent("a3", "Codex", "codex"),
                agent("a4", "Pi via Claude", "pi --provider claude"),
            ],
            ..AppSettings::default()
        });

        let reach = preview_watcher_reach(
            app.state(),
            "w".to_string(),
            Some(vec!["claude".to_string()]),
        )
        .await
        .expect("reach");

        let ids: Vec<&str> = reach.iter().map(|entry| entry.agent_id.as_str()).collect();
        assert_eq!(ids, vec!["a1", "a2"]);
        assert!(reach.iter().all(|entry| entry.in_budget));
        assert_eq!(reach[0].agent_label, "Claude");
        assert_eq!(reach[1].command_stem, "claude");
    }

    /// ...and it marks a watcher that reaches an agent but falls outside its budget, so the
    /// user sees "not running on <agent>" in the row where they made the choice instead of in
    /// a log line they will never read.
    #[tokio::test]
    async fn preview_reach_marks_a_watcher_that_is_out_of_budget() {
        let mut settings = AppSettings {
            agents: vec![agent("a1", "Claude", "claude")],
            ..AppSettings::default()
        };
        for i in 0..8 {
            settings.watchers.insert(format!("aaa-{i}"), watcher(None));
        }
        let app = settings_app(settings);

        let reach = preview_watcher_reach(app.state(), "zzz-mine".to_string(), None)
            .await
            .expect("reach");

        assert_eq!(reach.len(), 1);
        assert_eq!(reach[0].agent_id, "a1");
        assert!(
            !reach[0].in_budget,
            "eight watchers already run on this agent, so the ninth is configured but idle"
        );
    }

    /// A selector no agent has is not an error: it simply reaches nobody, and Settings shows
    /// "reaches 0 agents" so the typo is visible where it was made.
    #[tokio::test]
    async fn preview_reach_of_a_stem_no_agent_has_is_empty_and_not_an_error() {
        let app = settings_app(AppSettings {
            agents: vec![agent("a1", "Claude", "claude")],
            ..AppSettings::default()
        });

        let reach = preview_watcher_reach(
            app.state(),
            "w".to_string(),
            Some(vec!["gemni".to_string()]),
        )
        .await
        .expect("reach");

        assert!(reach.is_empty());
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
