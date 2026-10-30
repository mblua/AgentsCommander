use super::handoff::{
    is_permanent_delivery_error, resolve_remote_pty_command, wake_spawn_skip_auto_resume,
    SettleAction, FRESH_IDLE_GUARD,
};
use super::notice::{
    container_body_override_for_delivery, format_wake_content, is_container_backend_session,
    ResolvedWakeAgentCommand, WakeContent, WakeDeliveryOrigin,
};
use super::poller::{reserve_response_request_at_write, MailboxPoller};
use super::pty_claim::{response_diag_debug_only, ResponseWakeWork};
#[cfg(test)]
use super::pty_claim::{MailboxSpawnCall, MailboxTestEvent};
use super::settle::{
    classify_injection_error, live_settle_action, live_wake_route, settle_tick, wake_settle_ready,
    LiveWakeRoute, STARTUP_SETTLE_THRESHOLD,
};
use crate::phone::types::OutboxMessage;
use crate::pty::inject::SettleReadiness;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
#[cfg(test)]
use crate::session::session::SessionRepo;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn spawn_wake_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        resolved_command: &ResolvedWakeAgentCommand,
        cwd: String,
        session_name: String,
        spawn_with_resume: bool,
        skip_tooling_save: bool,
        spawn_shell: String,
        spawn_args: Vec<String>,
        spawn_label: Option<String>,
        resolved_spawn: Option<crate::config::agent_command::AgentSpawnCommand>,
        resolved_agent_host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>,
    ) -> Result<SessionInfo, String> {
        #[cfg(not(test))]
        let _ = msg;
        let skip_auto_resume = wake_spawn_skip_auto_resume(spawn_with_resume);
        let cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);

        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let call = MailboxSpawnCall {
                to: msg.to.clone(),
                session_name: session_name.clone(),
                cwd: cwd.clone(),
                shell: spawn_shell.clone(),
                shell_args: spawn_args.clone(),
                skip_auto_resume,
                skip_tooling_save,
            };
            {
                let mut calls = hooks.spawn_calls.lock().unwrap();
                calls.push(call.clone());
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Spawn(call));
            }

            let spawn_gate = hooks.internal_spawn_gate.lock().unwrap().take();
            if let Some(spawn_gate) = spawn_gate {
                hooks.internal_spawn_started.notify_one();
                let _ = spawn_gate.await;
            }

            let spawn_is_coordinator = *hooks.spawn_is_coordinator.lock().unwrap();
            // #2411 Mirror production create: the coordinator clear runs before the
            // create can fail, on both the failure and the success path.
            // #2413 The ticket ends like production: `failed()` on the create error.
            let ticket = if spawn_is_coordinator {
                crate::commands::session::note_coordinator_create_on_app(app, &cwd)
            } else {
                None
            };
            if let Some(msg) = hooks.internal_spawn_error.lock().unwrap().take() {
                if let Some(t) = ticket {
                    t.failed();
                }
                return Err(msg);
            }
            if let Some(t) = ticket {
                t.succeeded();
            }
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let session = mgr
                .create_session(
                    spawn_shell.clone(),
                    spawn_args.clone(),
                    cwd,
                    resolved_spawn
                        .as_ref()
                        .map(|spawn| spawn.trusted_agent_id.clone())
                        .or_else(|| resolved_command.agent_id.clone()),
                    spawn_label.clone(),
                    Vec::<SessionRepo>::new(),
                    spawn_is_coordinator,
                    crate::pty::backend::SessionBackendKind::LocalProcess,
                )
                .await
                .map_err(|e| e.to_string())?;
            mgr.rename_session(session.id, session_name)
                .await
                .map_err(|e| e.to_string())?;
            mgr.mark_idle(session.id).await;
            let inserted = mgr
                .get_session(session.id)
                .await
                .ok_or_else(|| format!("Session {} not found after test spawn", session.id))?;
            drop(mgr);
            super::tests::CodexClearScannerFixture::prepare_spawned_clear_session(
                app,
                hooks,
                inserted.id,
            )?;
            return Ok(SessionInfo::from(&inserted));
        }

        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
        crate::commands::session::create_session_inner(
            app,
            session_mgr.inner(),
            pty_mgr.inner(),
            spawn_shell,
            spawn_args,
            cwd,
            Some(session_name),                // readable name, no [temp] prefix
            resolved_command.agent_id.clone(), // links to agent config
            spawn_label,                       // human-readable label
            // #1635-2 D5: dispatch-flag wakes suppress lastCodingAgent persistence
            skip_tooling_save,
            Vec::new(),       // git_repos
            skip_auto_resume, // see deliver_wake top
            resolved_spawn,
            // #1271 - the configured host shell paired with the resolved agent,
            // built from the same settings snapshot that produced the spawn.
            resolved_agent_host_shell,
            // #973 - headless caller: no terminal to measure, keep 120x30.
            None,
            crate::commands::session::CreateSelectionIntent::Background,
        )
        .await
    }

    pub(crate) async fn attach_persisted_telegram_for_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        bot_id: Option<&str>,
    ) {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let bot_id = bot_id.map(str::to_string);
            {
                let mut calls = hooks.attach_calls.lock().unwrap();
                calls.push((session_id, bot_id.clone()));
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Attach { session_id, bot_id });
            }
            return;
        }

        #[cfg(not(test))]
        {
            crate::commands::session::attach_persisted_telegram_if_configured(
                app, session_id, bot_id,
            )
            .await;
        }
        #[cfg(test)]
        {
            let _ = (app, session_id, bot_id);
        }
    }

    pub(crate) async fn wait_for_spawned_wake_idle<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
    ) -> Result<SettleReadiness, String> {
        #[cfg(test)]
        if self.test_hooks.is_some() {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.mark_idle(session_id).await;
            // #2586 - the hooked path never runs the settle loop, so it has no
            // readiness to report; `Unknown` keeps it from fabricating `Ready`.
            return Ok(SettleReadiness::Unknown);
        }

        // #611: require SUSTAINED idle before injecting on the COLD-SPAWN path. A
        // freshly spawned agent (notably Claude) can hit a quiet window >
        // idle_threshold mid-startup and be marked idle before its TUI input/paste
        // state is stable; injecting then lands the body but the submit \r can be
        // dropped (written yet never sent). The settle lets late startup renders
        // finish first. Cold-spawn seeds fresh (None) and treats busy as "still
        // starting, keep waiting" (busy_means_inject = false). #1001 PR2 adds the
        // sibling live-inject gate `settle_live_before_inject`; both share the loop
        // below via `settle_until_ready`.
        self.settle_until_ready(
            app,
            session_id,
            std::time::Duration::from_secs(90),
            std::time::Duration::from_millis(2000),
            std::time::Duration::from_millis(500),
            None,
        )
        .await
    }

    /// (#1001 PR2 / B) Shared settle loop for both wake-inject paths. Waits until
    /// the session is safe to inject: sustained idle for `settle`, or - when
    /// `busy_means_inject` (live path) - a busy/mid-turn session injects at once
    /// (bias-to-deliver). `initial_idle_since` seeds the settle: the live path
    /// credits real idle age so a ready session is not delayed; cold-spawn passes
    /// None. Never drops a delivery: on `max_wait` it injects anyway. Errs only if
    /// the session was destroyed mid-settle, or (#1388/E8) has already exited. It
    /// gates on `wake_settle_ready` (idle AND rendered), not on idle alone, so a
    /// child that paints nothing no longer settles faster than a healthy one.
    /// It reads and decides BEFORE sleeping,
    /// so a seeded already-ready session injects with zero added latency; the
    /// cold-spawn sustained-idle requirement is unchanged.
    pub(crate) async fn settle_until_ready<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        max_wait: std::time::Duration,
        settle: std::time::Duration,
        poll: std::time::Duration,
        initial_idle_since: Option<std::time::Instant>,
    ) -> Result<SettleReadiness, String> {
        let start = std::time::Instant::now();
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mut idle_since = initial_idle_since;

        loop {
            // #1388: first condition, read in its own scope so the std Mutex guard is
            // dropped before the await below - the guard is not Send, so this is enforced
            // by the compiler rather than by convention. `try_state` (not `state`) mirrors
            // the IdleDetector read in `settle_live_before_inject` (:7719): a missing
            // manager is "no claim", not "not rendered", and must not gate.
            let rendered = match app.try_state::<Arc<Mutex<PtyManager>>>() {
                Some(pty_mgr) => match pty_mgr.lock() {
                    Ok(pty) => pty.has_rendered_visible_content(session_id),
                    Err(_) => true,
                },
                None => true,
            };

            // Read the flag under a short-lived lock; never hold it across the
            // pure decision or the sleep below.
            let waiting = {
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    // #1388/E8: a child that already exited can never satisfy the render
                    // condition, so without this it would hold the single global delivery
                    // lane for the full 90s and delay shutdown with it. Treated like the
                    // vanished case below: stop settling and let the caller's existing
                    // error path run.
                    Some(s) if matches!(s.status, SessionStatus::Exited(_)) => {
                        return Err(format!(
                            "Session {} exited before message injection",
                            session_id
                        ));
                    }
                    Some(s) => s.waiting_for_input,
                    None => {
                        return Err(format!(
                            "Session {} was destroyed before message injection",
                            session_id
                        ));
                    }
                }
            };
            let menu_blocked = app
                .try_state::<Arc<crate::pty::menu_guard::MenuGuard>>()
                .is_some_and(|g| g.is_blocked(session_id));
            let ready = !menu_blocked && wake_settle_ready(waiting, rendered);

            let was_settling = idle_since.is_some();
            let (next_idle_since, action) = settle_tick(
                ready,
                idle_since,
                std::time::Instant::now(),
                settle,
                start.elapsed(),
                max_wait,
            );
            idle_since = next_idle_since;

            match action {
                SettleAction::InjectNow => {
                    if menu_blocked {
                        return Err(format!(
                            "{}: session {} is blocked by interactive menu",
                            crate::pty::menu_guard::ERR_MENU_GUARD_DEFERRED,
                            session_id
                        ));
                    }
                    // #2586 D3 - readiness exists only on this `Ok` branch.
                    if start.elapsed() >= max_wait {
                        log::warn!(
                            "[mailbox] wake: timeout waiting for session {} to reach sustained \
                            idle; injecting anyway (waiting_for_input={}, rendered={})",
                            session_id,
                            waiting,
                            rendered
                        );
                        return Ok(SettleReadiness::TimedOut);
                    }
                    return Ok(SettleReadiness::Ready);
                }
                SettleAction::Wait => {
                    if was_settling && !ready {
                        log::info!(
                            "[mailbox] wake: session {} left the settle window during settle \
                            (waiting_for_input={}, rendered={}); re-waiting",
                            session_id,
                            waiting,
                            rendered
                        );
                    }
                    tokio::time::sleep(poll).await;
                }
            }
        }
    }

    /// (#1001 PR2 / B, grinch P1) Settle a LIVE session to paste-ready before a
    /// wake inject. The live-inject arm used to go straight to inject with no gate.
    /// The MEASURED live-path drop is the still-starting case (P2), closed by the
    /// alive_age Starting route below; the established fresh-idle guard here is
    /// DEFENSIVE retention (bias-to-deliver, cannot drop), NOT a measured-drop gate
    /// (the ~75% was PR1's cold-spawn measurement, not this path). Every tick reads ONE
    /// atomic `purge_readiness` snapshot and decides from the real-time
    /// `activity_age` (plus the per-session `idle_threshold` and the resize-freeze
    /// `last_resize_age`) via `live_settle_action` - NOT from the lagged
    /// `SessionManager.waiting_for_input`, whose ~500ms idle-flip lag would let a
    /// busy fast-path fire inside the fresh-idle window (grinch P1). A busy
    /// (mid-turn) or long-idle session injects at once; only a freshly-idle one
    /// waits out `FRESH_IDLE_GUARD`. Best-effort: any missing state falls through
    /// to the inject, which surfaces the real state.
    ///
    /// (#1001 PR2 P2 / option-a) That `activity_age` gate can't tell a STARTING
    /// session (emitting startup output) from a mid-turn one - both are
    /// `activity_age < idle_threshold` - so on a still-starting candidate the busy
    /// fast-path injected into a not-paste-ready TUI and dropped (still-starting
    /// 83%). So this first routes on `alive_age` (`live_wake_route`): a candidate
    /// younger than `STARTUP_SETTLE_THRESHOLD` takes the #611 sustained-idle settle
    /// (waits out startup churn); only an established candidate uses the
    /// `activity_age` fast-path below.
    pub(crate) async fn settle_live_before_inject<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
    ) -> SettleReadiness {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            hooks.settle_calls.lock().unwrap().push(session_id);
            let remove_session = hooks
                .remove_session_on_settle
                .lock()
                .unwrap()
                .remove(&session_id);
            if remove_session {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                mgr.destroy_session(session_id)
                    .await
                    .expect("remove test session after logical-command preflight");
            }
            // hooked tests exercise the inject wiring, not real timers
            return SettleReadiness::Unknown;
        }

        let Some(idle) = app.try_state::<std::sync::Arc<crate::pty::idle_detector::IdleDetector>>()
        else {
            // #2586 - no detector means no settle gate ran.
            return SettleReadiness::Unknown;
        };

        // (#1001 PR2 P2 / option-a) Classify starting vs established by alive_age
        // ONCE, at entry: alive_age at wake time is the time since PTY spawn, which
        // distinguishes a still-starting candidate (startup churn) from an
        // established one. alive_age only grows and the Starting branch is safe
        // regardless of a later crossing, so a single read suffices - re-classifying
        // per tick could switch a still-churning session onto the fast-path
        // mid-startup and re-open the drop.
        if live_wake_route(idle.alive_age(session_id), STARTUP_SETTLE_THRESHOLD)
            == LiveWakeRoute::Starting
        {
            // Still-starting LIVE candidate: in the same not-paste-ready state as a
            // cold spawn, so wait out startup churn with the #611 sustained-idle
            // settle (the gate PR1 measured at 0% drop) using the cold-spawn params
            // - notably the 90s cap, since a cold agent can take >10s to become
            // paste-ready (measured ~12-16s for Claude, startup_probe), well past
            // the live path's 10s cap. Best-effort: a destroy mid-settle just falls
            // through to the inject's own race path, with no readiness (#2586 D3:
            // readiness exists only on the `Ok` branch).
            return self
                .settle_until_ready(
                    app,
                    session_id,
                    std::time::Duration::from_secs(90),
                    std::time::Duration::from_millis(2000),
                    std::time::Duration::from_millis(500),
                    None,
                )
                .await
                .unwrap_or(SettleReadiness::Unknown);
        }

        // Established: gate on the real-time activity_age snapshot (grinch P1).
        let max_wait = std::time::Duration::from_secs(10);
        let poll = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();
        let menu_guard = app.try_state::<Arc<crate::pty::menu_guard::MenuGuard>>();

        loop {
            if menu_guard
                .as_ref()
                .is_some_and(|g| g.is_blocked(session_id))
            {
                if start.elapsed() >= max_wait {
                    return SettleReadiness::TimedOut;
                }
                tokio::time::sleep(poll).await;
                continue;
            }
            let Some(r) = idle.purge_readiness(&[session_id]).into_iter().next() else {
                return SettleReadiness::Unknown; // no snapshot: proceed to inject (best-effort)
            };
            let settle_live = r.idle_threshold + FRESH_IDLE_GUARD;
            match live_settle_action(
                r.activity_age,
                r.last_resize_age,
                r.resize_grace,
                r.idle_threshold,
                settle_live,
                start.elapsed(),
                max_wait,
            ) {
                SettleAction::InjectNow => {
                    if start.elapsed() >= max_wait {
                        log::warn!(
                            "[mailbox] wake: live settle for {} hit the {}s cap; injecting anyway",
                            session_id,
                            max_wait.as_secs()
                        );
                        return SettleReadiness::TimedOut;
                    }
                    return SettleReadiness::Ready;
                }
                SettleAction::Wait => tokio::time::sleep(poll).await,
            }
        }
    }

    /// Inject a message into a session's PTY stdin.
    /// `interactive` = true (all remaining callers): live interactive `wake`
    /// delivery — plain message only, no response markers, no watcher.
    /// `interactive` = false is currently unreachable (the former
    /// `wake-and-sleep` non-interactive path was removed in 0.7.0). The
    /// `use_markers=true` branch below is retained for future non-interactive
    /// consumers; see _plans/delete-modes.md §2.4.
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    pub(crate) async fn inject_into_pty<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        msg: &OutboxMessage,
        interactive: bool,
        origin: WakeDeliveryOrigin,
        comanaged: bool,
        settle: SettleReadiness,
    ) -> Result<(), String> {
        self.inject_into_pty_with_response(
            app,
            session_id,
            msg,
            interactive,
            origin,
            comanaged,
            settle,
            (None, None),
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Keep the original peer injection flags, settle policy and borrowed response \
            context"
    )]
    pub(crate) async fn inject_into_pty_with_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        msg: &OutboxMessage,
        interactive: bool,
        origin: WakeDeliveryOrigin,
        comanaged: bool,
        settle: SettleReadiness,
        context: (
            Option<&ResponseWakeWork>,
            Option<&crate::session::selection::OwnedReplacementContinuation>,
        ),
    ) -> Result<(), String> {
        let (response, owned_replacement) = context;
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

        // Remote logical actions use the same canonical text-block injector as
        // messages. Resolve against the actual session shell before the busy
        // check, and never synthesize provider text from the wire value.
        if let Some(ref command) = msg.command {
            let (shell, waiting_for_input) = {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions
                    .iter()
                    .find(|session| session.id == session_id.to_string())
                {
                    Some(session) => (session.shell.clone(), session.waiting_for_input),
                    None => {
                        return Err(format!(
                            "Session not found: {} - cannot execute logical remote command '{}'",
                            session_id, command
                        ));
                    }
                }
            };
            let resolved = resolve_remote_pty_command(&shell, command)?;
            if !waiting_for_input {
                return Err(format!(
                    "Cannot execute remote command '{}': agent is busy (not idle)",
                    command
                ));
            }

            // Submit the resolved static text through the canonical injector.
            // The accepted idle-to-final-Enter race is the same one as normal
            // message delivery; delayed double Enter remains the shared defense.
            // #2336 - this is a peer wake (logical remote command), so it passes
            // through the typing hold like a standard wake.
            let source = self.codex_clear_source.as_deref().filter(|_| {
                command == "clear"
                    && Path::new(shell.trim())
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .is_some_and(|stem| stem.to_lowercase().starts_with("codex"))
            });
            let observation = source.map(|_| {
                let mut observer = self.delivery_poller();
                observer.clear_reservation = self.clear_reservation.clone();
                Box::new(move || observer.begin_clear_observation()) as Box<dyn FnOnce() + Send>
            });
            crate::pty::inject::inject_peer_wake_with_current_facts(
                app,
                session_id,
                resolved.text,
                &msg.id,
                settle,
                observation,
                |session, current| {
                    if owned_replacement.is_none()
                        && crate::config::sessions_persistence::unconfirmed_replacement_work_waits(
                            &msg.to, session.id,
                        )
                    {
                        return Err(crate::pty::inject::OWNED_WORK_SCOPE_WAIT.into());
                    }
                    if owned_replacement.is_some_and(|continuation| {
                        !continuation.pre_write_work_matches(current, pty_mgr.inner())
                    }) {
                        return Err("replacement work binding requires recapture".into());
                    }
                    if let Some(source) = source {
                        if msg.command.as_deref() != Some("clear")
                            || resolve_remote_pty_command(&session.shell, "clear")? != resolved
                        {
                            return Err("codex_clear_execution_error: provider changed".into());
                        }
                        super::clear_attempt::persist_attempt(source, msg, session.id)
                            .map_err(|error| format!("codex_clear_execution_error: {error}"))?;
                    }
                    Ok(())
                },
            )
            .await
            .map_err(|e| {
                if e == crate::pty::inject::OWNED_WORK_SCOPE_WAIT {
                    return e;
                }
                if !crate::pty::menu_guard::is_typing_hold_deferred_error(&e)
                    && (is_permanent_delivery_error(&e) || self.is_terminal_clear_error(msg, &e))
                {
                    // Terminal rejection of a previously held wake: drop its
                    // count. A transient failure keeps it, because the message
                    // stays queued and may be deferred again.
                    crate::commands::pty::clear_held_wake(app, session_id, &msg.id);
                }
                let (level, outcome) = classify_injection_error(&e);
                log::log!(
                    level,
                    "[mailbox] PTY injection {} msg={} session={} logical={} resolved={}: {}",
                    outcome,
                    msg.id,
                    session_id,
                    command,
                    resolved.text,
                    e
                );
                e
            })?;

            log::info!(
                "[mailbox] logical PTY action executed msg={} session={} logical={} resolved={}",
                msg.id,
                session_id,
                command,
                resolved.text
            );

            // Logical clear is a fresh-conversation boundary regardless of
            // whether the provider text was /clear or Pi /new. Compact keeps
            // the conversation and does not stamp.
            if resolved.logical.creates_fresh_boundary() {
                crate::commands::pty::stamp_fresh_boundary_to_session(app, session_id).await;
            }

            let _ = tauri::Emitter::emit(
                app,
                "message_delivered",
                serde_json::json!({
                    "id": msg.id,
                    "from": msg.from,
                    "to": msg.to,
                    "mode": msg.mode,
                    "command": command,
                    "injected": true
                }),
            );

            // Logical actions keep the live child environment. If the message
            // has a follow-up body, inject it only after the agent becomes idle.
            // Credentials remain environment-only. Detached work never blocks
            // the delivery pipeline.
            let app_clone = app.clone();
            let msg_clone = msg.clone();
            let command_owned = command.clone();
            let resolved_text = resolved.text;
            tauri::async_runtime::spawn(async move {
                if !msg_clone.body.is_empty() {
                    if let Err(e) =
                        Self::inject_followup_after_idle_static(&app_clone, session_id, &msg_clone)
                            .await
                    {
                        log::warn!(
                            "[mailbox] Failed to inject follow-up after logical {} resolved as {} \
                            for session {}: {}",
                            command_owned,
                            resolved_text,
                            session_id,
                            e
                        );
                    }
                }
            });

            return Ok(());
        }

        // ── Standard message path ──
        // Only use response markers for non-interactive sessions
        let use_markers = msg.get_output && !interactive;
        let body_override = if is_container_backend_session(app, session_id)? {
            let sender_root = if origin == WakeDeliveryOrigin::FilesystemPoller
                && crate::phone::messaging::parse_file_notification(&msg.body).is_some()
            {
                Some(
                    self.resolve_repo_path(&msg.from, app)
                        .await
                        .ok_or_else(|| {
                            format!(
                        "Cannot resolve sender path for '{}' during container file delivery",
                        msg.from
                    )
                        })?,
                )
            } else {
                None
            };
            container_body_override_for_delivery(
                origin,
                &msg.body,
                sender_root.as_deref().map(Path::new),
            )?
        } else {
            None
        };
        let body = body_override.as_deref().unwrap_or(&msg.body);

        // Interactive and marker-less paths share the minimal PTY wrap via
        // `format_pty_wrap` (single source with `PTY_WRAP_FIXED` used by the
        // CLI clamp). Only the `--get-output` + `request_id` case wraps the
        // payload with response markers.
        let payload = match (use_markers, msg.request_id.as_ref()) {
            (true, Some(rid)) if comanaged => {
                let display_from =
                    crate::phone::messaging::compose_sender_for_comanaged_origin(&msg.from);
                crate::phone::messaging::format_pty_wrap_with_markers(&display_from, body, rid)
            }
            (true, Some(rid)) => {
                crate::phone::messaging::format_pty_wrap_with_markers(&msg.from, body, rid)
            }
            _ => format_wake_content(WakeContent::Peer {
                from: &msg.from,
                body,
                origin,
                comanaged,
            }),
        };

        // Register response watcher only for non-interactive sessions
        if use_markers {
            if let Some(ref rid) = msg.request_id {
                // Response file goes to the SENDER's .agentscommander/responses/
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let response_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let mgr = pty_mgr
                        .lock()
                        .map_err(|e| format!("PTY lock failed: {}", e))?;
                    mgr.register_response_watcher(session_id, rid.clone(), response_dir);
                    drop(mgr);
                }
            }
        }

        // SECURITY: this `first_100` log MUST NOT see credential values.
        // Credentials are env-only and must never be routed through PTY payloads.
        // Keep this log limited to standard message payloads.
        log::debug!(
            "[mailbox] Injecting into PTY session={} msg={} payload_len={} first_100={:?}",
            session_id,
            msg.id,
            payload.len(),
            payload.chars().take(100).collect::<String>()
        );
        // #2336 - a standard peer wake is the other gated delivery path; the
        // detached logical-command follow-up above stays on the plain injector.
        let prepared = self
            .prepare_response_request(app, msg, session_id, comanaged, response)
            .await;
        let attempt = response.and_then(|work| match work.attempt.lock() {
            Ok(attempt) => attempt.clone(),
            Err(_) => {
                response_diag_local!("inject", "attempt_lock_failure", work.diagnostic_message_id);
                None
            }
        });
        response_diag_debug_only(|| {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag \
                site={} outcome=observed stage=inject_presence id=unknown member={} prepared={} \
                attempt={} work={}",
                line!(),
                session_id,
                prepared.is_some(),
                attempt.is_some(),
                response.is_some()
            );
        });
        let mut diagnostic_prewrite_reached = false;
        crate::pty::inject::inject_peer_wake_with_current_facts(
            app,
            session_id,
            &payload,
            &msg.id,
            settle,
            None,
            |session, current| {
                response_diag_debug_only(|| {
                    diagnostic_prewrite_reached = true;
                });
                response_diag_local!(
                    "prewrite",
                    "callback_reached",
                    Uuid::parse_str(&msg.id).ok()
                );
                if owned_replacement.is_none()
                    && crate::config::sessions_persistence::unconfirmed_replacement_work_waits(
                        &msg.to, session.id,
                    )
                {
                    response_diag_local!(
                        "prewrite",
                        "owned_replacement_wait",
                        Uuid::parse_str(&msg.id).ok()
                    );
                    return Err(crate::pty::inject::OWNED_WORK_SCOPE_WAIT.into());
                }
                if owned_replacement.is_some_and(|continuation| {
                    !continuation.pre_write_work_matches(current, pty_mgr.inner())
                }) {
                    response_diag_local!(
                        "prewrite",
                        "owned_replacement_mismatch",
                        Uuid::parse_str(&msg.id).ok()
                    );
                    return Err("replacement work binding requires recapture".into());
                }
                if let (Some(prepared), Some(attempt), Some(work)) =
                    (prepared.as_ref(), attempt.as_ref(), response)
                {
                    reserve_response_request_at_write(
                        app, current, session, prepared, work, attempt,
                    );
                }
                Ok(())
            },
        )
        .await
        .map_err(|e| {
            response_diag_local!(
                "inject",
                if diagnostic_prewrite_reached {
                    "failure_after_prewrite"
                } else {
                    "failure_before_prewrite"
                },
                Uuid::parse_str(&msg.id).ok()
            );
            if e == crate::pty::inject::OWNED_WORK_SCOPE_WAIT {
                return e;
            }
            if !crate::pty::menu_guard::is_typing_hold_deferred_error(&e)
                && is_permanent_delivery_error(&e)
            {
                crate::commands::pty::clear_held_wake(app, session_id, &msg.id);
            }
            let (level, outcome) = classify_injection_error(&e);
            log::log!(
                level,
                "[mailbox] PTY injection {} session={} msg={}: {}",
                outcome,
                session_id,
                msg.id,
                e
            );
            e
        })?;

        response_diag_local!(
            "inject",
            if diagnostic_prewrite_reached {
                "success_prewrite_reached"
            } else {
                "success_prewrite_not_reached"
            },
            Uuid::parse_str(&msg.id).ok()
        );
        log::info!(
            "[mailbox] PTY injection SUCCESS session={} msg={}",
            session_id,
            msg.id
        );
        // (#756) AC-injected message CONTENT creates a post-boundary transcript:
        // drop any pending fresh intent (record + mirror). Bare logical action
        // text (/clear, Pi /new, or /compact) never reaches this line.
        crate::commands::pty::note_post_boundary_content_to_session(app, session_id).await;
        let _ = tauri::Emitter::emit(
            app,
            "message_delivered",
            serde_json::json!({
                "id": msg.id,
                "from": msg.from,
                "to": msg.to,
                "mode": msg.mode,
                "injected": true
            }),
        );
        Ok(())
    }

    /// Wait for agent to become idle after a remote command, then inject body as follow-up.
    /// Static method — can be spawned as a detached task without borrowing self.
    async fn inject_followup_after_idle_static<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        let max_wait = std::time::Duration::from_secs(30);
        let poll = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();

        // Wait for idle (waiting_for_input = true)
        loop {
            if start.elapsed() >= max_wait {
                return Err(format!(
                    "Timeout waiting for agent to become idle after remote command ({}s)",
                    max_wait.as_secs()
                ));
            }
            tokio::time::sleep(poll).await;

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == session_id.to_string()) {
                Some(s) if s.waiting_for_input => break,
                Some(_) => {} // busy — keep polling
                None => {
                    return Err(format!(
                        "Session {} destroyed before follow-up could be injected",
                        session_id
                    ))
                }
            }
        }

        // Inject the follow-up body as a standard interactive message.
        // Note: same TOCTOU race as the command path — agent could become busy
        // between the idle check above and this write. Acceptable for this use case.
        let payload = crate::phone::messaging::format_pty_wrap(&msg.from, &msg.body);
        crate::pty::inject::inject_text_into_session(app, session_id, &payload).await?;
        // A follow-up body after logical clear is post-boundary content.
        crate::commands::pty::note_post_boundary_content_to_session(app, session_id).await;
        Ok(())
    }
}
