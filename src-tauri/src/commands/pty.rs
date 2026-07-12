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

    // Existing behavior: write keystrokes to the PTY FIRST (no await held here)
    // so input latency is unchanged; the #552 bookkeeping is additive and after.
    pty_mgr
        .lock()
        .unwrap()
        .write(uuid, &data)
        .map_err(|e| e.to_string())?;

    // #552 user input -> silence touch (+ badge reset if coordinator). Resolves
    // all state from `app`, so the same helper serves Telegram and web.
    note_user_message_to_session(&app, uuid, UserInputSource::Terminal(&data)).await;

    Ok(())
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
        let _ = app.emit(
            "session_communication_changed",
            serde_json::json!({ "sessionId": session_id.to_string(), "communication": null }),
        );
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
/// auto-close, manual close). Call ONLY after a SUCCESSFUL
/// `/clear` injection (C1 remote command, C2 self-clear phase-1). The restart
/// path (C3) stamps the record itself and calls only the mirror half.
/// Root-agent sessions skip the record half (the root restore path ignores the
/// marker, #630 scope; mirrors the restart site's exclusion in
/// commands/session.rs); the mirror half self-gates on coordinators.
pub(crate) async fn stamp_fresh_boundary_to_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) {
    // (#756, section 19.3) MIRROR-FIRST: if the app dies between the halves,
    // the residue (mirror=Some, record=false) forces fresh on every reopen
    // path and self-heals (E3 re-propagates; typed input or injected content
    // clears both). Record-first residue (record=true, mirror=None) would let
    // a later record destroy resurrect the pre-boundary conversation: the
    // exact #756 bug.
    write_start_fresh_mirror_for_session(app, session_id, true).await;
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
    if !is_root {
        match crate::config::sessions_persistence::set_start_fresh_and_persist_result(
            &manager, session_id,
        )
        .await
        {
            Ok(true) => log::info!(
                "[session-state] {} fresh-boundary stamped (record, #756)",
                &session_id.to_string()[..8]
            ),
            Ok(false) => {} // already stamped or unknown id
            Err(e) => log::error!(
                "[session-state] fresh-boundary stamp persist failed for {}: {}",
                session_id,
                e
            ),
        }
    }
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
/// Never call it for the bare `/clear` / `/compact` command text.
pub(crate) async fn note_post_boundary_content_to_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) {
    // (#756, section 19.3) MIRROR-FIRST: the drop residue (mirror=None,
    // record=true) only mis-freshes the record-alive restore until the next
    // heal; record-first residue (record=false, mirror=Some) would wrongly
    // force-fresh BOTH reopen paths.
    write_start_fresh_mirror_for_session(app, session_id, false).await;
    let manager = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let guard = mgr.read().await;
        guard.clone()
    };
    match crate::config::sessions_persistence::clear_start_fresh_and_persist_result(
        &manager, session_id,
    )
    .await
    {
        Ok(true) => log::info!(
            "[session-state] {} fresh intent dropped (post-boundary content, #756)",
            &session_id.to_string()[..8]
        ),
        Ok(false) => {}
        Err(e) => log::error!(
            "[session-state] post-boundary-content drop persist failed for {}: {}",
            session_id,
            e
        ),
    }
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
    let cwd = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let cwd = mgr.read().await.coordinator_cwd(session_id).await;
        cwd
    };
    let Some(cwd) = cwd else { return false };
    let Some(clocks) = app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
    else {
        return false;
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
        if let Err(e) = crate::config::coordinator_clocks::save_map(&snapshot) {
            log::warn!("[coordinator-clocks] start_fresh_at save failed: {}", e);
        }
    }
    changed
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
