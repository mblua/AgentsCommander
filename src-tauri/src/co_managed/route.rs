//! Co-managed routing, effect commit and trigger handling.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use tauri::Manager;
use tokio_util::sync::CancellationToken;

use super::{
    abstain_reason_label, co_managed_catalog, co_managed_jev_settings, co_managed_state_reason,
    co_managed_user_message, emit_co_managed_state, route_co_managed_wake,
    surface_co_managed_user_message, write_co_managed_message_file, CoManagedRoute,
    CoManagedSupervisorHandle, CoManagedTrigger, CO_MANAGED_MAX_ATTEMPTS,
    CO_MANAGED_RETRY_BACKOFF_MS,
};
use crate::capture;
use crate::reader_demand::{
    reader_demand_pass, recovery_pass, watch_capture_slots, READER_DEMAND_TICK_SECS,
};

/// Settings project paths plus the project derived from the room root, the
/// same effective slice the CLI (`cli::send::execute`) and the mailbox's
/// Co-managed branch build before the Root check. The queue lives under
/// `<project>/.ac/<room>`, so the project is the room root's grandparent; a
/// room whose project is not (or no longer) registered in settings must still
/// let a verified coordinator reach Root (F4).
pub(crate) fn co_managed_project_paths(settings_paths: &[String], room_root: &Path) -> Vec<String> {
    let mut paths = settings_paths.to_vec();
    let Some(project_dir) = room_root.parent().and_then(|ac_root| ac_root.parent()) else {
        return paths;
    };
    let canon_project = crate::filesystem::projects::canonical_refresh_project(project_dir).ok();
    let already_present = paths.iter().any(|p| match &canon_project {
        Some(canon_target) => {
            crate::filesystem::projects::canonical_refresh_project(Path::new(p))
                .ok()
                .as_ref()
                == Some(canon_target)
        }
        None => Path::new(p) == project_dir,
    });
    if !already_present {
        paths.push(project_dir.to_string_lossy().to_string());
    }
    paths
}

/// The authorization branch is mandatory, not a style choice: `can_communicate`
/// returns false for `Root` by its own rules, because Root belongs to no team
/// and is nobody's coordinator. Root goes through the verified-coordinator
/// validator; every other destination goes through `can_communicate`.
pub(crate) fn resolve_co_managed_route(
    outcome: capture::jev::ClassifyOutcome,
    catalog: &capture::catalog::Catalog,
    from_fqn: &str,
    project_paths: &[String],
    candidate_text: &str,
) -> CoManagedRoute {
    use capture::catalog::{Destination, Resolution};
    match outcome {
        capture::jev::ClassifyOutcome::Abstained { reason } => CoManagedRoute::User {
            reason: format!("abstained: {reason}"),
            write_candidate_file: true,
        },
        capture::jev::ClassifyOutcome::Classified {
            category,
            destination,
            score,
            runner_up,
        } => {
            let decision =
                format!("category '{category}' (noul {score:.2}, runner-up {runner_up:.2})");
            match destination {
                Destination::User => CoManagedRoute::User {
                    reason: format!("{decision} routes to the user"),
                    write_candidate_file: true,
                },
                Destination::Orchestrator => match catalog.resolve(&category) {
                    Resolution::Valid {
                        peer: Some(peer), ..
                    } => {
                        let discovered_teams = crate::config::teams::discover_teams();
                        if crate::config::teams::can_communicate(from_fqn, &peer, &discovered_teams)
                        {
                            CoManagedRoute::Wake {
                                to: peer.clone(),
                                content: candidate_text.to_string(),
                                reason: format!("{decision} routed to {peer}"),
                            }
                        } else {
                            CoManagedRoute::User {
                                reason: format!(
                                    "{decision} names peer '{peer}', which is not reachable; \
                                        nothing was routed"
                                ),
                                write_candidate_file: true,
                            }
                        }
                    }
                    _ => CoManagedRoute::User {
                        reason: format!(
                            "{decision} names an orchestrator destination without a peer; \
                                nothing was routed"
                        ),
                        write_candidate_file: true,
                    },
                },
                Destination::Root => {
                    if crate::config::teams::verified_wg_coordinator_target(from_fqn, project_paths)
                        .is_some()
                    {
                        CoManagedRoute::Wake {
                            to: crate::config::root_agent::ROOT_AGENT_SENDER.to_string(),
                            content: candidate_text.to_string(),
                            reason: format!("{decision} routed to the Root Agent"),
                        }
                    } else {
                        CoManagedRoute::User {
                            reason: format!(
                                "{decision} chose Root, but this session is not a verified room \
                                    orchestrator; nothing was routed"
                            ),
                            write_candidate_file: true,
                        }
                    }
                }
                Destination::DefaultReply => match catalog.resolve(&category) {
                    Resolution::Valid {
                        reply: Some(reply), ..
                    } => CoManagedRoute::Wake {
                        to: from_fqn.to_string(),
                        content: reply,
                        reason: format!("{decision} replied to this session"),
                    },
                    _ => CoManagedRoute::User {
                        reason: format!("{decision} has no reply; nothing was routed"),
                        write_candidate_file: true,
                    },
                },
            }
        }
    }
}

// ── the commit and its retry policy ─────────────────────────────────────────

pub(crate) enum CoManagedCommit {
    Committed(capture::state::CommittedEffect),
    Contention,
    Rejected(capture::state::AbstainReason),
}

/// The preconditions, gathered fresh immediately before each attempt. No lock
/// of ours is held across the awaits; the results travel in as a typed value.
pub(crate) async fn gather_co_managed_preconditions<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: uuid::Uuid,
    room_root: &Path,
    kind: capture::state::EffectKind,
) -> capture::state::EffectPreconditions {
    let (session_alive, unique_live_session_for_cwd) = {
        let Some(manager) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() else {
            return capture::state::EffectPreconditions {
                session_alive: false,
                session_id: session_id.to_string(),
                unique_live_session_for_cwd: false,
                no_pending_user_input: false,
                effective_ready: false,
                observed_at: Instant::now(),
                kind,
            };
        };
        let guard = manager.read().await;
        let session = guard.get_session(session_id).await;
        let anchor = session
            .as_ref()
            .map(|s| s.working_directory.clone())
            .unwrap_or_default();
        let sessions = guard.list_sessions().await;
        let unique = sessions
            .iter()
            .filter(|s| {
                s.working_directory == anchor
                    && !matches!(s.status, crate::session::session::SessionStatus::Exited(_))
            })
            .count()
            == 1;
        (session.is_some(), unique)
    };

    let no_pending_user_input =
        match app.try_state::<crate::pty::input_activity::SubstantiveInputState>() {
            Some(state) => {
                let tracker = state.lock().unwrap_or_else(|e| e.into_inner());
                !tracker.pending_within(
                    session_id,
                    crate::pty::input_activity::USER_WRITE_STAMP_WINDOW,
                )
            }
            None => true,
        };

    let effective_ready = matches!(
        crate::commands::session::co_managed_effective_state_for_session(
            app,
            room_root,
            &session_id.to_string()
        )
        .await,
        Ok(crate::config::co_managed::CoManagedState::Ready)
    );

    capture::state::EffectPreconditions {
        session_alive,
        session_id: session_id.to_string(),
        unique_live_session_for_cwd,
        no_pending_user_input,
        effective_ready,
        observed_at: Instant::now(),
        kind,
    }
}

/// `commit_effect` is synchronous and takes a blocking advisory file lock for
/// up to `LOCK_WAIT_BUDGET` (100 ms). Called directly it would park a Tokio
/// worker on every idle edge, so it runs on the blocking pool and a join error
/// is an abstention, never a silent success.
pub(crate) async fn commit_co_managed_effect(
    room_root: PathBuf,
    slot: capture::sink::CaptureSlot,
    expected_seq: u64,
    expected_key: capture::key::ConsumptionKey,
    pre: capture::state::EffectPreconditions,
    capture: Arc<capture::state::CaptureState>,
) -> Result<capture::state::CommittedEffect, capture::state::AbstainReason> {
    tauri::async_runtime::spawn_blocking(move || {
        capture::state::commit_effect(
            &room_root,
            &slot,
            expected_seq,
            &expected_key,
            &pre,
            &capture,
        )
    })
    .await
    .unwrap_or(Err(capture::state::AbstainReason::LockUnavailable(
        "commit task join failed".to_string(),
    )))
}

/// Three attempts per trigger, the first plus two retries, with a fresh full
/// snapshot before each; `PreconditionsStale` and `LockBusy` are the only
/// retryable outcomes. The ceiling is the executable form of the bound: under
/// contention the feature abstains with a reason instead of looping forever.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn commit_co_managed_with_retries<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
    session_id: uuid::Uuid,
    room_root: &Path,
    slot: &capture::sink::CaptureSlot,
    expected_seq: u64,
    expected_key: &capture::key::ConsumptionKey,
    kind: capture::state::EffectKind,
) -> CoManagedCommit {
    for attempt in 0..CO_MANAGED_MAX_ATTEMPTS {
        if attempt > 0 {
            let backoff = CO_MANAGED_RETRY_BACKOFF_MS[(attempt - 1) as usize];
            tokio::time::sleep(Duration::from_millis(backoff)).await;
        }
        let pre = gather_co_managed_preconditions(app, session_id, room_root, kind).await;
        handle.record_commit_call();
        let result = commit_co_managed_effect(
            room_root.to_path_buf(),
            slot.clone(),
            expected_seq,
            expected_key.clone(),
            pre,
            handle.capture.clone(),
        )
        .await;
        match &result {
            Ok(committed) => log::info!(
                "[co-managed] commit [{session_id}]: attempt={} committed kind={:?} routable={}",
                attempt + 1,
                committed.kind,
                committed.routable
            ),
            Err(reason) => log::info!(
                "[co-managed] commit [{session_id}]: attempt={} abstained: {}",
                attempt + 1,
                abstain_reason_label(reason)
            ),
        }
        match result {
            Ok(committed) => {
                handle.run_after_commit_hook();
                return CoManagedCommit::Committed(committed);
            }
            Err(capture::state::AbstainReason::PreconditionsStale)
            | Err(capture::state::AbstainReason::LockBusy) => {}
            Err(error) => return CoManagedCommit::Rejected(error),
        }
    }
    handle.mark_contended(&session_id.to_string(), expected_seq);
    CoManagedCommit::Contention
}

pub(crate) enum CoManagedOutcome {
    Contention,
    Done(String),
}

/// One full effect: secrets first, then classification, then the bounded
/// commit, then the action. Every path returns the reason the cycle ended.
pub(crate) async fn co_managed_cycle<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
    session_id: uuid::Uuid,
    room_root: &Path,
    slot: &capture::sink::CaptureSlot,
    expected_seq: u64,
    candidate: &Arc<capture::record::CapturedRecord>,
) -> CoManagedOutcome {
    let settings = match co_managed_jev_settings(app).await {
        Ok(settings) => settings,
        Err(error) => {
            log::debug!("[co-managed] settings unavailable: {error}");
            return CoManagedOutcome::Done("settings_unavailable".to_string());
        }
    };
    let session = {
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = manager.read().await;
        guard.get_session(session_id).await
    };
    let Some(session) = session else {
        return CoManagedOutcome::Done("session vanished".to_string());
    };
    let from_fqn = crate::config::teams::agent_fqn_from_path(&session.working_directory);
    let expected_key =
        capture::state::consumption_key(room_root, candidate.as_ref(), &handle.capture);
    let text = candidate.text.clone();
    let commit = |kind| {
        commit_co_managed_with_retries(
            app,
            handle,
            session_id,
            room_root,
            slot,
            expected_seq,
            &expected_key,
            kind,
        )
    };

    // (1) The detector runs before any write and before any network call.
    if let Some(detection) = capture::secrets::detect(&handle.capture.secret_rules, &text) {
        let reason = detection.reason();
        return report_co_managed_secret(
            (slot, expected_seq, &expected_key),
            reason,
            commit(capture::state::EffectKind::TextToUser),
            |message| surface_co_managed_user_message(app, session_id, message),
        )
        .await;
    }

    // (2) Classify through the phase-6 leaf call.
    let catalog = co_managed_catalog(room_root);
    let outcome = {
        let network = app.state::<crate::network::OutboundNetwork>();
        capture::jev::classify(
            &network,
            &settings,
            &catalog,
            &text,
            &session_id.to_string()[..8],
        )
        .await
    };

    // (3) Decide before committing, so an unreachable peer never spends budget.
    let project_paths = {
        let settings = app.state::<SettingsState>();
        let guard = match settings.current_read().await {
            Ok(guard) => guard,
            Err(error) => {
                log::debug!("[co-managed] settings unavailable: {error}");
                return CoManagedOutcome::Done("settings_unavailable".to_string());
            }
        };
        co_managed_project_paths(&guard.project_paths, room_root)
    };
    let route = resolve_co_managed_route(outcome, &catalog, &from_fqn, &project_paths, &text);
    log_co_managed_route(session_id, &route);
    let kind = route.kind();

    // (4) The bounded commit.
    match commit(kind).await {
        CoManagedCommit::Committed(committed) => {
            if !consume_co_managed_route(slot, expected_seq, &expected_key, &committed) {
                return CoManagedOutcome::Done(
                    "baseline record consumed; never routed".to_string(),
                );
            }
            perform_co_managed_route(
                app, handle, session_id, room_root, &from_fqn, candidate, route, &text,
            )
            .await
        }
        CoManagedCommit::Contention => CoManagedOutcome::Contention,
        CoManagedCommit::Rejected(reason) => {
            if matches!(reason, capture::state::AbstainReason::BudgetExhausted)
                && kind == capture::state::EffectKind::Automatic
            {
                // Budget exhaustion abstains from the automatic action and
                // falls back to the budget-free text-to-user channel, so the
                // user sees why nothing was routed and nothing is enqueued.
                return report_co_managed_budget_exhausted(
                    (slot, expected_seq, &expected_key),
                    candidate,
                    &text,
                    commit(capture::state::EffectKind::TextToUser),
                    |message| surface_co_managed_user_message(app, session_id, message),
                )
                .await;
            }
            CoManagedOutcome::Done(abstain_reason_label(&reason))
        }
    }
}

/// Consume this candidate and decide whether its committed effect can be routed.
fn consume_co_managed_route(
    slot: &capture::sink::CaptureSlot,
    expected_seq: u64,
    expected_key: &capture::key::ConsumptionKey,
    committed: &capture::state::CommittedEffect,
) -> bool {
    // A newer offer stays in the slot and re-triggers its own cycle.
    slot.try_consume(expected_seq, expected_key, expected_key.epoch);
    // Automatic baseline records are consumed without routing. TextToUser
    // effects are also non-routable, but still surface their user message.
    committed.kind != capture::state::EffectKind::Automatic || committed.routable
}

/// Surface a detected secret only after its budget-free effect is committed.
async fn report_co_managed_secret<F: std::future::Future<Output = ()>>(
    pending: (
        &capture::sink::CaptureSlot,
        u64,
        &capture::key::ConsumptionKey,
    ),
    reason: String,
    commit: impl std::future::Future<Output = CoManagedCommit>,
    surface: impl FnOnce(String) -> F,
) -> CoManagedOutcome {
    let user_message = co_managed_secret_message(&reason);
    let (slot, expected_seq, expected_key) = pending;
    match commit.await {
        CoManagedCommit::Committed(_) => {
            // Consume this exact candidate; a newer offer stays in the
            // slot and re-triggers its own cycle.
            slot.try_consume(expected_seq, expected_key, expected_key.epoch);
            surface(user_message).await;
            CoManagedOutcome::Done(reason)
        }
        CoManagedCommit::Contention => CoManagedOutcome::Contention,
        CoManagedCommit::Rejected(reason) => CoManagedOutcome::Done(abstain_reason_label(&reason)),
    }
}

fn co_managed_secret_message(reason: &str) -> String {
    co_managed_user_message(
        &format!("{reason}; no file was written and nothing was routed"),
        None,
    )
}

fn log_co_managed_route(session_id: uuid::Uuid, route: &CoManagedRoute) {
    log::info!(
        "[co-managed] route [{session_id}]: variant={} kind={:?}",
        match route {
            CoManagedRoute::User { .. } => "User",
            CoManagedRoute::Wake { .. } => "Wake",
        },
        route.kind()
    );
}

/// Report budget exhaustion after the budget-free commit, then surface its message.
async fn report_co_managed_budget_exhausted<F: std::future::Future<Output = ()>>(
    pending: (
        &capture::sink::CaptureSlot,
        u64,
        &capture::key::ConsumptionKey,
    ),
    candidate: &capture::record::CapturedRecord,
    text: &str,
    commit: impl std::future::Future<Output = CoManagedCommit>,
    surface: impl FnOnce(String) -> F,
) -> CoManagedOutcome {
    let (slot, expected_seq, expected_key) = pending;
    match commit.await {
        CoManagedCommit::Committed(_) => {
            // Consume this exact candidate; a newer offer stays
            // in the slot and re-triggers its own cycle.
            slot.try_consume(expected_seq, expected_key, expected_key.epoch);
            let message = co_managed_user_message(
                &format!(
                    "abstained: the automatic budget is exhausted ({} actions since \
                        the last recharge); the candidate was not routed",
                    capture::state::BUDGET_CAP
                ),
                Some((text, candidate.provider_final, None)),
            );
            surface(message).await;
            CoManagedOutcome::Done("BudgetExhausted".to_string())
        }
        CoManagedCommit::Contention => CoManagedOutcome::Contention,
        CoManagedCommit::Rejected(second) => CoManagedOutcome::Done(abstain_reason_label(&second)),
    }
}

/// The committed effect. A `Rejected` automatic commit falls back to the
/// budget-free text-to-user channel so the user sees the reason; a route
/// failure after the commit is reported there too (the declared residual: the
/// reservation is not rolled back).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn perform_co_managed_route<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
    session_id: uuid::Uuid,
    room_root: &Path,
    from_fqn: &str,
    candidate: &Arc<capture::record::CapturedRecord>,
    route: CoManagedRoute,
    text: &str,
) -> CoManagedOutcome {
    match route {
        CoManagedRoute::User {
            reason,
            write_candidate_file,
        } => {
            let mut message = co_managed_user_message(&reason, None);
            if write_candidate_file {
                // Except in the secret case, the candidate is also written as a
                // file in `messaging/`, and the communication carries the
                // reason, an excerpt with its real length, and the path.
                match write_co_managed_message_file(room_root, from_fqn, from_fqn, text) {
                    Ok((path, _pointer)) => {
                        handle.record_step("messaging_file");
                        message = co_managed_user_message(
                            &reason,
                            Some((text, candidate.provider_final, Some(path.as_path()))),
                        );
                    }
                    Err(error) => {
                        message.push_str(&format!("\nmessage file write failed: {error}"));
                    }
                }
            }
            surface_co_managed_user_message(app, session_id, message).await;
            CoManagedOutcome::Done(reason)
        }
        CoManagedRoute::Wake {
            to,
            content,
            reason,
        } => match route_co_managed_wake(handle, room_root, from_fqn, &to, &content) {
            Ok(warning) => {
                if let Some(warning) = warning {
                    surface_co_managed_user_message(
                        app,
                        session_id,
                        format!("Send queued; message archive warning: {warning}"),
                    )
                    .await;
                }
                CoManagedOutcome::Done(reason)
            }
            Err(error) => {
                let message = co_managed_user_message(
                    &format!("routing failed after the effect was reserved: {error}"),
                    Some((text, candidate.provider_final, None)),
                );
                surface_co_managed_user_message(app, session_id, message).await;
                CoManagedOutcome::Done(format!("route failed: {error}"))
            }
        },
    }
}

/// One supervisor input. Trigger (b) runs a cycle only when the session is
/// already idle; any other slot transition only keeps the armed flag current,
/// which is how the idle edge finds it set.
pub(crate) async fn handle_co_managed_trigger<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
    trigger: CoManagedTrigger,
) {
    let session_id = trigger.session_id();
    let id = session_id.to_string();

    let Some(registry) = app.try_state::<Arc<capture::registry::CaptureRegistry>>() else {
        log::info!("[co-managed] trigger dropped [{id}]: no capture registry in state");
        return;
    };
    let Some((slot, state, candidate)) = co_managed_trigger_candidate(&registry, handle, &id)
    else {
        return;
    };

    let session = {
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = manager.read().await;
        guard.get_session(session_id).await
    };
    let Some(session) = session else {
        log::info!("[co-managed] trigger dropped [{id}]: session is gone; flag removed");
        handle.armed.remove(&id);
        return;
    };
    let Some(room_root) =
        crate::config::co_managed::room_root_for_path(Path::new(&session.working_directory))
    else {
        log::info!(
            "[co-managed] trigger dropped [{id}]: working directory {} resolves to no room \
                root; flag removed",
            session.working_directory
        );
        handle.armed.remove(&id);
        return;
    };

    let effective =
        crate::commands::session::co_managed_effective_state_for_session(app, &room_root, &id)
            .await;
    let ready = matches!(
        effective,
        Ok(crate::config::co_managed::CoManagedState::Ready)
    );
    let was_armed = handle.armed.is_armed(&id);
    if !ready {
        let catalog_path = crate::config::co_managed::load_config(&room_root)
            .catalog_path
            .map(|path| room_root.join(path));
        log::warn!(
            "[co-managed] trigger dropped [{id}]: not ready ({}); room {}, catalog {:?}; flag \
                removed",
            co_managed_state_reason(&effective),
            room_root.display(),
            catalog_path
        );
        // #2756 U2a: the slot keeps the candidate; remember it so the next
        // reader-demand pass can re-offer it once the room is Ready.
        handle.mark_pending_recovery(&id, state.seq, Arc::clone(&candidate));
        if was_armed {
            emit_co_managed_state(
                app,
                session_id,
                false,
                Some(&co_managed_state_reason(&effective)),
            );
        }
        handle.armed.remove(&id);
        return;
    }

    // Ready with an unconsumed candidate: the idle edge must find the flag set.
    handle.armed.arm(&id);

    // Contention leaves the candidate pending. A trigger for the SAME sequence
    // must not re-run it; a new record changes the sequence and re-triggers.
    // This check sits AFTER the readiness gate (F1): a session that loses
    // readiness while contended must clear the armed flag and publish the
    // readiness reason, not keep reporting `comanaged: true` forever.
    if handle.is_contended(&id, state.seq) {
        log::info!(
            "[co-managed] trigger dropped [{id}]: already contended for sequence {}",
            state.seq
        );
        return;
    }

    let already_idle = matches!(session.status, crate::session::session::SessionStatus::Idle)
        || session.waiting_for_input;
    let is_idle_edge = matches!(trigger, CoManagedTrigger::IdleEdge(_));
    if !is_idle_edge && !already_idle {
        log::info!(
            "[co-managed] trigger dropped [{id}]: not an idle edge and the session is not idle"
        );
        return;
    }
    if !is_idle_edge {
        // Trigger (b): a record arrived while the session was already idle.
        // This transition does not coincide with the idle edge, so it carries
        // its own event before the cycle performs anything.
        emit_co_managed_state(app, session_id, true, None);
    }

    log_co_managed_cycle_start(&id, &trigger, state.seq);
    let outcome = co_managed_cycle(
        app, handle, session_id, &room_root, &slot, state.seq, &candidate,
    )
    .await;
    finish_co_managed_cycle(handle, session_id, outcome, |armed, reason| {
        emit_co_managed_state(app, session_id, armed, reason);
    });
}

fn log_co_managed_cycle_start(id: &str, trigger: &CoManagedTrigger, seq: u64) {
    log::info!(
        "[co-managed] cycle start [{id}]: trigger={} seq={}",
        match trigger {
            CoManagedTrigger::IdleEdge(_) => "IdleEdge",
            CoManagedTrigger::SlotChanged(_) => "SlotChanged",
            CoManagedTrigger::Recovery(_) => "Recovery",
        },
        seq
    );
}

/// Resolve the pending capture and remove stale armed flags.
fn co_managed_trigger_candidate(
    registry: &capture::registry::CaptureRegistry,
    handle: &CoManagedSupervisorHandle,
    id: &str,
) -> Option<(
    capture::sink::CaptureSlot,
    capture::sink::SlotState,
    Arc<capture::record::CapturedRecord>,
)> {
    let Some(slot) = registry.slot(id) else {
        log::info!(
            "[co-managed] trigger dropped [{id}]: no capture slot for the session; flag removed"
        );
        handle.armed.remove(id);
        return None;
    };
    let state = slot.snapshot();
    let Some(candidate) = state.value.record().cloned() else {
        // Consumed, invalidated or cleared: the candidate this flag described
        // is gone, so the flag goes with it.
        log::info!(
            "[co-managed] trigger dropped [{id}]: slot holds no candidate record; flag removed"
        );
        handle.armed.remove(id);
        return None;
    };
    Some((slot, state, candidate))
}

fn finish_co_managed_cycle(
    handle: &CoManagedSupervisorHandle,
    session_id: uuid::Uuid,
    outcome: CoManagedOutcome,
    emit: impl FnOnce(bool, Option<&str>),
) {
    let id = session_id.to_string();
    match outcome {
        CoManagedOutcome::Contention => {
            log::info!(
                "[co-managed] cycle end [{id}]: Contention, reason contention; \
                 candidate stays pending"
            );
            emit(false, Some("contention"));
            // The flag stays set while the unchanged candidate remains pending.
        }
        CoManagedOutcome::Done(reason) => {
            handle.clear_contended(&id);
            handle.armed.disarm(&id);
            log::info!(
                "[co-managed] cycle end [{id}]: Done({}); flag disarmed",
                capture::jev::safe_reason(&reason)
            );
            emit(false, Some(&reason));
        }
    }
}

/// Start the app-wide supervisor. The discovery tick subscribes a watcher to
/// every live session's slot; the watcher forwards each slot transition as
/// trigger (b) and keeps the armed flag current on records while the session
/// is still busy.
pub(crate) fn spawn_co_managed_supervisor<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    handle: CoManagedSupervisorHandle,
    mut triggers: tokio::sync::mpsc::UnboundedReceiver<CoManagedTrigger>,
    shutdown: CancellationToken,
) {
    tauri::async_runtime::spawn(async move {
        let mut discovery = tokio::time::interval(Duration::from_millis(500));
        discovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut reader_demand = tokio::time::interval(Duration::from_secs(READER_DEMAND_TICK_SECS));
        reader_demand.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                _ = discovery.tick() => watch_capture_slots(&app, &handle).await,
                _ = reader_demand.tick() => {
                    reader_demand_pass(&app).await;
                    recovery_pass(&app, &handle).await;
                }
                trigger = triggers.recv() => {
                    let Some(trigger) = trigger else { break };
                    handle_co_managed_trigger(&app, &handle, trigger).await;
                }
            }
        }
    });
}
