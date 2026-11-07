use std::sync::{Arc, OnceLock};

use crate::app_run::exit::ExitEvents;
use crate::{
    emit_session_idle_edge, loops, CoManagedSupervisorHandle, IdleDetector, SessionManager,
};
use tauri::{AppHandle, Manager, Runtime};

impl<R: Runtime> ExitEvents for AppHandle<R> {
    fn terminal_attached(&self, session_id: String) {
        let _ = tauri::Emitter::emit(
            self,
            "terminal_attached",
            serde_json::json!({ "sessionId": session_id }),
        );
    }
}

impl IdleDetector {
    pub(crate) fn for_app_run(
        owners: &crate::AppOwners,
        response_close: &crate::pty::manager::ResponseCloseState,
        co_managed_supervisor_for_idle: CoManagedSupervisorHandle,
        app_handle_lock: &Arc<OnceLock<tauri::AppHandle>>,
    ) -> Arc<IdleDetector> {
        let handle_for_idle = Arc::clone(app_handle_lock);
        let handle_for_busy = Arc::clone(app_handle_lock);
        let response_close_for_idle = response_close.clone();
        let response_close_for_busy = response_close.clone();
        let idle_detector = IdleDetector::with_activity_coalescer(
            Arc::clone(&owners.activity),
            move |id| {
                response_close_for_idle.observe_detector(id, false, std::time::Instant::now());
                log::debug!("[idle] >>> EMIT session_idle for {}", &id.to_string()[..8]);
                if let Some(app) = handle_for_idle.get() {
                    // #2232 phase 7: the Co-managed decision travels in the same
                    // payload, emitted synchronously and FIRST (epic 3.6). A
                    // separate later event would always paint waiting before red.
                    // `is_armed` is lock-free and never reads settings or disk.
                    let _ = emit_session_idle_edge(
                        app,
                        &co_managed_supervisor_for_idle.armed,
                        Some(&co_managed_supervisor_for_idle),
                        id,
                    );
                    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                    let mgr_clone = session_mgr.inner().clone();
                    let app_for_idle = app.clone();
                    // #1682 - read the control-write mark on the watcher thread, at
                    // the edge instant. `Option<Duration>` is `Copy`, so it moves
                    // into the spawned task with no clone and no guard held.
                    let control_write_age = app
                        .try_state::<Arc<crate::pty::idle_detector::IdleDetector>>()
                        .and_then(|detector| detector.control_write_age(id));
                    tauri::async_runtime::spawn(async move {
                        let mgr = mgr_clone.read().await;
                        mgr.mark_idle(id).await;
                        crate::config::sessions_persistence::persist_current_state_prune_dormant(
                            &mgr,
                        )
                        .await;
                        // #1682 - a busy->idle edge on an armed session stamps
                        // `tooling.lastAgentMessageAt`. A no-op unless a message write reached
                        // an arming site for this session in this process (R7 and R8 arm with
                        // nothing submitted), the user has no recent unsubmitted input in it,
                        // and no recent control write of ours is what re-opened the busy period.
                        crate::commands::session::record_agent_turn_completed(
                            &app_for_idle,
                            &mgr,
                            id,
                            control_write_age,
                        )
                        .await;
                        if let Some(scheduler) =
                            app_for_idle.try_state::<Arc<loops::scheduler::LoopScheduler>>()
                        {
                            scheduler
                                .inner()
                                .on_session_idle(app_for_idle.clone(), id)
                                .await;
                        }
                    });
                }
            },
            move |id| {
                response_close_for_busy.observe_detector(id, true, std::time::Instant::now());
                log::debug!("[idle] >>> EMIT session_busy for {}", &id.to_string()[..8]);
                if let Some(app) = handle_for_busy.get() {
                    let _ = tauri::Emitter::emit(
                        app,
                        "session_busy",
                        serde_json::json!({ "id": id.to_string() }),
                    );
                    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                    let mgr_clone = session_mgr.inner().clone();
                    tauri::async_runtime::spawn(async move {
                        let mgr = mgr_clone.read().await;
                        mgr.mark_busy(id).await;
                        crate::config::sessions_persistence::persist_current_state_prune_dormant(
                            &mgr,
                        )
                        .await;
                    });
                }
            },
        );
        response_close.bind_publisher(crate::pty::manager::ResponseClosePublisher::Detector);
        let response_close_for_output = response_close.clone();
        if !idle_detector.bind_raw_output_observer(
            response_close.wiring_identity(),
            move |id, now| {
                response_close_for_output.observe_raw_output(id, now);
            },
        ) {
            response_close.fail_closed();
        }
        response_close.bind_publisher(crate::pty::manager::ResponseClosePublisher::RawOutput);
        idle_detector
    }
}
