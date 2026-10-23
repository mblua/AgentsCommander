//! The run-event handler of `run()`: window teardown and the exit path.

use std::sync::{Arc, Mutex};

use tauri::Manager;

use crate::{
    combined_shutdown_retained_diagnostics_with_scanner, commands, phone, quit_gate_window_gone,
    resource_monitor, sessions_persistence, shutdown_persistence_allowed, DetachedSessionsState,
    PtyManager, QuitGate, SessionManager, ShutdownSignal, SpecBoardState, TauriQuitHost,
    TelegramBridgeState, SHUTDOWN_CLEANUP_BUDGET_SECS,
};

/// Locals `run()` keeps alive for the run-event handler.
pub(super) struct ExitCtx {
    pub(super) detached_set: DetachedSessionsState,
    pub(super) spec_board_state: SpecBoardState,
    pub(super) session_mgr_for_exit: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(super) coordinator_clocks_for_exit:
        crate::config::coordinator_clocks::CoordinatorClocksState,
    pub(super) shutdown_for_exit: ShutdownSignal,
    pub(super) selection_coordinator_for_exit: crate::session::selection::SelectionCoordinator,
    pub(super) tg_mgr_for_exit: TelegramBridgeState,
    pub(super) resource_monitor_for_exit: Arc<resource_monitor::ResourceMonitorState>,
    pub(super) ui_automation_state_for_exit: crate::testability::ui_automation::UiAutomationState,
}

pub(super) fn on_run_event(app_handle: &tauri::AppHandle, event: tauri::RunEvent, ctx: &ExitCtx) {
    let ExitCtx {
        detached_set,
        spec_board_state,
        session_mgr_for_exit,
        coordinator_clocks_for_exit,
        shutdown_for_exit,
        selection_coordinator_for_exit,
        tg_mgr_for_exit,
        resource_monitor_for_exit,
        ui_automation_state_for_exit,
    } = ctx;
    match event {
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Destroyed,
            ..
        } => {
            // #2296 - a gate window that dies without answering must not
            // hold a quit round open. Synchronous: it only drops the
            // registration, clears busy and (if that gate was still
            // unanswered) aborts the round with reason `destroyed`.
            if let Some(gate) = app_handle.try_state::<Arc<QuitGate>>() {
                let gate = Arc::clone(gate.inner());
                quit_gate_window_gone(&gate, &TauriQuitHost::new(app_handle.clone()), &label);
            }
            // #1363 - a destroyed window's terminal-output attachments are released
            // here, in the backend, without any frontend cooperation. It is what keeps
            // a window that died without detaching from leaving a session emitting to
            // a webview that is gone, and it is why the frontend close hook does not
            // need to block the close.
            if let Some(pty_mgr) = app_handle
                .try_state::<Arc<Mutex<crate::pty::manager::PtyManager>>>()
                .map(|state| Arc::clone(state.inner()))
            {
                let destroyed = label.clone();
                tauri::async_runtime::spawn(async move {
                    pty_mgr
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .release_window_attachments(&destroyed);
                });
            }
            if label == "spec-board" {
                let state = spec_board_state.clone();
                tauri::async_runtime::spawn(async move {
                    commands::spec_board::spec_board_close_all(state).await;
                });
            }
            // #566 - when the main window is destroyed (any close path:
            // X, Alt+F4, programmatic, silent- or confirm-quit), close the
            // Resource Monitor window so it cannot orphan and keep the app
            // alive. No-op if it was never opened or already closed.
            if label == "main" {
                if let Some(rm) = app_handle.get_webview_window("resource-monitor") {
                    // G4: log on failure rather than swallow. A swallowed
                    // error would hide the exact orphan bug this fixes;
                    // mirrors the FE quit path's console.warn.
                    if let Err(e) = rm.destroy() {
                        log::warn!("[shutdown] RM window destroy failed: {e}");
                    }
                }
                // #2563 - same orphan class for detached terminals when
                // main is destroyed without a quit round (OS logoff,
                // programmatic destroy). Leaves `was_detached` intact.
                let mut destroyed = 0usize;
                for (label, window) in app_handle.webview_windows() {
                    if !label.starts_with("terminal-") {
                        continue;
                    }
                    if let Err(e) = window.destroy() {
                        log::warn!("[shutdown] detached window destroy failed: {label}: {e}");
                    }
                    destroyed += 1;
                }
                if destroyed > 0 {
                    log::info!(
                        "[shutdown] main destroyed: destroyed {} detached terminal \
                            window(s)",
                        destroyed
                    );
                }
            }
            // Detached-window destroyed (by any mechanism — X, Alt+F4, programmatic).
            // Two jobs:
            //   1) Clear from `DetachedSessionsState` — switch_session needs an
            //      accurate view of which sessions have live windows.
            //   2) Emit `terminal_attached` — frontend stores subscribed to this event
            //      clear the id from `sessionsStore.detachedIds` (Phase 2+ only;
            //      Phase 1 has no subscriber — the event is harmlessly dropped).
            //
            // DELIBERATELY ABSENT: we do NOT call `SessionManager::set_was_detached`
            // here. That mutation is reserved for `detach_terminal_inner` (→true)
            // and `attach_terminal` (→false) under Fix A (plan §A3.2 / NEW-3).
            // Mirroring the clear here would reintroduce NEW-1: A3.7 quit path
            // destroys every detached window → Destroyed fires N times → all
            // `Session::was_detached` flipped to false → `persist_current_state`
            // on `RunEvent::Exit` writes was_detached=false for every session →
            // restart restores nothing detached. See plan §10 rule.
            if let Some(id_no_dashes) = label.strip_prefix("terminal-") {
                if id_no_dashes.len() == 32 {
                    let formatted = format!(
                        "{}-{}-{}-{}-{}",
                        &id_no_dashes[0..8],
                        &id_no_dashes[8..12],
                        &id_no_dashes[12..16],
                        &id_no_dashes[16..20],
                        &id_no_dashes[20..32],
                    );
                    if let Ok(uuid) = uuid::Uuid::parse_str(&formatted) {
                        {
                            let mut set = detached_set.lock().unwrap();
                            set.remove(&uuid);
                        }
                        let _ = tauri::Emitter::emit(
                            app_handle,
                            "terminal_attached",
                            serde_json::json!({ "sessionId": formatted }),
                        );
                    }
                }
            }
            // #714 screenshot overlay destroyed (user close, crash, or our
            // own teardown): clear capture state and destroy sibling
            // overlays. Idempotent — a no-op once state is already Idle.
            if label.starts_with("screenshot-overlay-") {
                let label = label.to_string();
                let app = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    crate::screenshot::handle_overlay_window_destroyed(app, label).await;
                });
            }
        }
        tauri::RunEvent::Exit => {
            // Cancel all active Telegram bridges before general shutdown
            let bridge_shutdowns = {
                let mut tg = tauri::async_runtime::block_on(tg_mgr_for_exit.lock());
                tg.cancel_all()
            };
            for shutdown in bridge_shutdowns {
                shutdown.abort_now();
            }

            // #1149 - close every open activity interval here. `trigger()`
            // below stops the IdleDetector before anything else, so this
            // is the only position where the session map is still
            // populated AND the detector is still alive. Without it a
            // clean exit would drop every open interval, which is the
            // defect this issue names first.
            //
            // Reaching the manager needs the outer lock, and `block_on`
            // is forbidden on this path, so take it with `try_read`, clone
            // the manager out (it is an `Arc` over its own state) and drop
            // the guard before spinning.
            let manager_for_activity = session_mgr_for_exit
                .try_read()
                .ok()
                .map(|guard| guard.clone());
            let working_snapshot = match manager_for_activity {
                Some(manager) => {
                    // Bounded at 500 ms, and it holds no lock the writer
                    // needs while it sleeps, so it can never starve the
                    // writer it waits on. It can only fail to observe a
                    // gap, and that failure is already correct: the
                    // consumer closes every open interval at `app_stop`'s
                    // timestamp regardless of enumeration.
                    let deadline =
                        std::time::Instant::now() + std::time::Duration::from_millis(500);
                    loop {
                        if let Some(rows) = manager.try_snapshot_working_sessions() {
                            break Some(rows);
                        }
                        if std::time::Instant::now() >= deadline {
                            break None;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                }
                // The outer lock has no production writer, so unreachable
                // in practice; falls through to the degraded path.
                None => None,
            };
            let activity_batch = match &working_snapshot {
                Some(rows) => {
                    let mut batch: Vec<_> = rows
                        .iter()
                        .map(|row| {
                            crate::config::activity_log::build_idle_from_snapshot(
                                row,
                                crate::config::activity_log::IdleReason::AppStop,
                            )
                        })
                        .collect();
                    batch.push(crate::config::activity_log::build_app_stop(
                        true,
                        rows.len(),
                    ));
                    batch
                }
                // The degraded path is a designed outcome, not a fallback
                // to optimise away: the spin may legitimately exhaust
                // under teardown load, and the enumerated records are pure
                // precision on top of a close that happens either way.
                None => vec![crate::config::activity_log::build_app_stop(false, 0)],
            };
            // One open, write and close for all N+1 lines.
            crate::config::activity_log::append_batch(&activity_batch);

            // #632 B1 - trigger background-task shutdown FIRST so the resource
            // watchdog stops dispatching NEW ticks and the idle detectors stop.
            // (An already-dispatched spawn_blocking kill_group still runs;
            // safety there rests on B2b's bounded set + kill_group's
            // Terminating/Terminated idempotency guard, not on trigger().)
            let snapshot_scanner_shutdown =
                phone::mailbox::MailboxPoller::active_terminal_snapshot_shutdown_owner();
            if let Some(owner) = &snapshot_scanner_shutdown {
                owner.seal();
            }
            log::info!("[shutdown] Triggering background task shutdown (async, not awaited)...");
            shutdown_for_exit.trigger();
            let scanner_shutdown = match snapshot_scanner_shutdown {
                Some(owner) => tauri::async_runtime::block_on(owner.seal_and_drain_until(
                    tokio::time::Instant::now()
                        + std::time::Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS),
                )),
                None => phone::terminal_snapshot::SnapshotScannerDrainResult {
                    terminal: true,
                    ..Default::default()
                },
            };
            log::info!(
                "[shutdown] terminal snapshot scanner drained joined={} aborted={} \
                    terminal={}",
                scanner_shutdown.joined,
                scanner_shutdown.aborted,
                scanner_shutdown.terminal
            );

            let context_alert_monitor = app_handle
                .try_state::<Arc<crate::session::context_alerts::ContextAlertMonitor>>()
                .map(|monitor| Arc::clone(monitor.inner()));
            if let Some(monitor) = context_alert_monitor.as_ref() {
                monitor.request_close();
            }

            let selection_shutdown =
                tauri::async_runtime::block_on(selection_coordinator_for_exit.close_and_join());
            log::info!("[shutdown] selection coordinator joined before global cleanup");

            // Selection shutdown seals and accounts for session preparation before
            // the alert actor consumes its sole join handle.
            if let Some(monitor) = context_alert_monitor {
                if let Err(error) = tauri::async_runtime::block_on(monitor.close_and_join()) {
                    log::warn!("[shutdown] context alert monitor join failed: {}", error);
                }
            }

            // #632 A - kill every agent's Job Object: atomically terminates
            // each jobbed session's whole descendant tree via the job handle.
            // This is the durable, orphan-free guarantee for jobbed sessions.
            let pty_mgr = app_handle.state::<Arc<Mutex<PtyManager>>>();
            let pty_lock_budget = std::time::Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS);
            let container_backend = {
                let deadline = std::time::Instant::now() + pty_lock_budget;
                loop {
                    match pty_mgr.try_lock() {
                        Ok(guard) => break Some(guard.container_backend()),
                        Err(std::sync::TryLockError::Poisoned(error)) => {
                            break Some(error.into_inner().container_backend());
                        }
                        Err(std::sync::TryLockError::WouldBlock)
                            if std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                        Err(std::sync::TryLockError::WouldBlock) => break None,
                    }
                }
            };
            let mut container_shutdown = match container_backend {
                Some(container_backend) => {
                    container_backend.stop_all_started_containers_blocking(pty_lock_budget)
                }
                None => {
                    log::error!(
                        "[shutdown] PTY owner lock reached the global cleanup deadline \
                            before container ownership transfer"
                    );
                    crate::pty::container_backend::ContainerShutdownReport {
                        terminal: false,
                        retained: vec!["reason=global-pty-owner state=retained".to_string()],
                    }
                }
            };
            let jobs = {
                let deadline = std::time::Instant::now() + pty_lock_budget;
                loop {
                    match pty_mgr.try_lock() {
                        Ok(guard) => break Some(guard.kill_all_jobs()),
                        Err(std::sync::TryLockError::Poisoned(error)) => {
                            break Some(error.into_inner().kill_all_jobs());
                        }
                        Err(std::sync::TryLockError::WouldBlock)
                            if std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                        Err(std::sync::TryLockError::WouldBlock) => break None,
                    }
                }
            };
            let (jobs_killed, jobless_sessions) = match jobs {
                Some(counts) => counts,
                None => {
                    log::error!(
                        "[shutdown] PTY owner lock reached the job cleanup deadline \
                            state=retained"
                    );
                    container_shutdown.terminal = false;
                    container_shutdown
                        .retained
                        .push("reason=global-job-owner state=retained".to_string());
                    (0, 0)
                }
            };
            log::info!(
                "[shutdown] terminated {jobs_killed} agent job object(s); \
                    {jobless_sessions} session(s) had no job"
            );

            // #632 B2+B4 - run the identity reaper for accounting and as the
            // backstop for job-less sessions, TIME-BOXED. Fresh-only targets
            // (B2a) make it near-instant for jobbed sessions (their live tree is
            // already dead). Abandoning it on timeout is safe for jobbed
            // sessions; the job-less warning below covers the MED-2 residual.
            let rm_for_cleanup = resource_monitor_for_exit.clone();
            let cleanup = crate::shutdown::run_time_boxed(
                std::time::Duration::from_secs(SHUTDOWN_CLEANUP_BUDGET_SECS),
                move || {
                    rm_for_cleanup
                        .kill_all_owned_groups(resource_monitor::ResourceKillReason::AppShutdown)
                },
            );
            match &cleanup {
                Ok(results) => {
                    for result in results {
                        if result.quarantined {
                            log::warn!(
                                "[shutdown] resource group {} quarantined during \
                                    cleanup: {}",
                                result.session_id,
                                result.message
                            );
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => log::warn!(
                    "[shutdown] resource cleanup exceeded \
                        {SHUTDOWN_CLEANUP_BUDGET_SECS}s budget; proceeding to exit (job \
                        objects already terminated jobbed trees)"
                ),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => log::error!(
                    "[shutdown] resource cleanup thread panicked before completing; \
                        jobbed trees were still terminated by the job kill"
                ),
            }
            // #632 MED-2 - job-less sessions are reaper-only; if the reaper did
            // not finish, their trees may be orphaned. Make it visible.
            if cleanup.is_err() && jobless_sessions > 0 {
                log::warn!(
                    "[shutdown] {jobless_sessions} session(s) had no Job Object and the \
                        reaper did not finish in budget; their process trees may be \
                        orphaned"
                );
            }

            if shutdown_persistence_allowed(
                selection_shutdown.persistence_safe,
                container_shutdown.terminal,
            ) {
                log::info!("[shutdown] Persisting session state...");
                let mgr_clone = session_mgr_for_exit.clone();
                tauri::async_runtime::block_on(async move {
                    let mgr = mgr_clone.read().await;
                    sessions_persistence::persist_current_state(&mgr).await;
                });
                log::info!("[shutdown] Session state persisted, process exiting");
            } else {
                let retained = combined_shutdown_retained_diagnostics_with_scanner(
                    scanner_shutdown.retained,
                    selection_shutdown.retained,
                    container_shutdown.retained,
                );
                log::error!(
                    "[shutdown] final session persistence skipped because cleanup \
                        ownership is retained work=[{}]",
                    retained.join(", ")
                );
            }

            // #552 flush the coordinator badge / auto-closed store on clean
            // exit (the 60s tick can leave up to one tick of recency
            // unpersisted). Sync snapshot + save; best-effort on poison.
            {
                let snap = coordinator_clocks_for_exit
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .snapshot();
                if let Err(e) = crate::config::coordinator_clocks::save_map(&snap) {
                    log::warn!("[coordinator-clocks] exit flush failed: {}", e);
                }
            }

            // Issue #231 + grinch G-LOW (#246): remove daemon.pid AFTER
            // persist_current_state so a concurrent CLI invocation never
            // observes NoPidFile while sessions.json is being rewritten.
            // Still runs before process exit — subsequent CLI invocations
            // see NoPidFile (not StalePidFile) once we return.
            crate::config::daemon_pid::remove_pid_file();
            ui_automation_state_for_exit.cleanup_session_file();
        }
        _ => {}
    }
}
