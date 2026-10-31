use super::handoff::{resolve_remote_pty_command, wake_action_for, WakeAction};
use super::notice::{
    InternalNoticeGuard, InternalSystemNotice, InternalSystemTarget, ResolvedWakeSpawnPlan,
    WakeDeliveryOrigin,
};
use super::poller::{
    MailboxPoller, OwnedPeerWakeExecution, OwnedPeerWakeStep, PeerWakeFallback, SpawnedPeerWake,
};
#[cfg(test)]
use super::pty_claim::MailboxTestHooks;
use super::pty_claim::{ResponseWakeWork, WakeDelivery};
use super::settle::err_is_pty_session_missing;
use crate::phone::types::OutboxMessage;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionCommunicationKind;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Manager;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

async fn prepare_replacement_context(
    execution: &OwnedPeerWakeExecution,
    continuation: &crate::session::selection::OwnedReplacementContinuation,
    manager: &SessionManager,
    pty: &Arc<Mutex<PtyManager>>,
) -> Result<bool, String> {
    use crate::session::selection::LifecycleEffectSlot;
    if let Some(spawn) = execution.request.resolved_spawn.as_ref() {
        if !continuation.validate_pre_effect_region(manager, pty).await {
            return Ok(false);
        }
        let Some(effect) = continuation.start_effect(LifecycleEffectSlot::ContextPreparation)
        else {
            return Ok(false);
        };
        let prepared = crate::config::agent_command::prepare_agent_spawn_command(spawn);
        effect.complete(prepared.as_ref().map(|_| None).map_err(Clone::clone));
        if let Err(error) = prepared {
            continuation
                .rollback_failed_create(manager, pty, &error)
                .await;
            return Err(error);
        }
        if !continuation.preserve_context_prerequisite() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn replacement_delivery(execution: &OwnedPeerWakeExecution) -> OutboxMessage {
    let mut delivered = execution.message.clone();
    if execution.flags_present {
        if let Some(spawn) = execution.request.resolved_spawn.as_ref() {
            delivered.effective_agent_id = Some(spawn.trusted_agent_id.clone());
            delivered.effective_profile = Some(spawn.profile_resolution.effective_profile.clone());
            delivered.profile_fallback_applied = spawn.profile_resolution.fallback_applied;
        } else {
            delivered.effective_agent_id = execution.resolved_command.agent_id.clone();
        }
    }
    delivered
}

fn replacement_target_ownership(
    execution: &OwnedPeerWakeExecution,
) -> Result<crate::api::message_store::PtyInputTargetOwnership<'_>, String> {
    execution
        .gate
        .target_ownership(
            &execution.message.to,
            execution.stripe.as_ref().ok_or("missingTargetStripe")?,
            execution.exact.as_ref().ok_or("missingTargetOwnership")?,
        )
        .map_err(|_| "invalidTargetCreateOwnership".to_string())
}

#[cfg(test)]
enum PeerReplacementFixture {
    NotUsed,
    Retained,
    Created(Box<Result<crate::commands::session::OwnedReplacementCreateStep, String>>),
}

#[cfg(test)]
async fn execute_peer_replacement_fixture<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    execution: &mut OwnedPeerWakeExecution,
    continuation: &crate::session::selection::OwnedReplacementContinuation,
    fixture: Option<Arc<crate::session::selection::ReplacementFixtureControl>>,
) -> PeerReplacementFixture {
    let Some(control) = fixture else {
        return PeerReplacementFixture::NotUsed;
    };
    let (Some(exact), Some(stripe)) = (execution.exact.take(), execution.stripe.take()) else {
        return PeerReplacementFixture::Retained;
    };
    let Some(coordinator) = app.try_state::<crate::session::selection::SelectionCoordinator>()
    else {
        return PeerReplacementFixture::Retained;
    };
    let input = crate::commands::session::FixtureReplacementInput {
        request: execution.request.clone(),
        continuation: continuation.clone().with_fixture_control(control.clone()),
        lease: crate::commands::session::FixtureReplacementLease::OrdinaryMoved(
            crate::commands::session::FixturePtyCutLease {
                gate: execution.gate.clone(),
                target: execution.message.to.clone(),
                exact,
                stripe,
            },
        ),
    };
    let Some(result) = coordinator
        .submit_inline_replacement_fixture(input, control)
        .await
    else {
        return PeerReplacementFixture::Retained;
    };
    let Some(lease) = result.ordinary_lease else {
        return PeerReplacementFixture::Retained;
    };
    if !Arc::ptr_eq(&lease.gate, &execution.gate) || lease.target != execution.message.to {
        return PeerReplacementFixture::Retained;
    }
    execution.exact = Some(lease.exact);
    execution.stripe = Some(lease.stripe);
    PeerReplacementFixture::Created(Box::new(result.outcome))
}

impl MailboxPoller {
    pub(crate) async fn execute_owned_peer_replacement<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        execution: &mut OwnedPeerWakeExecution,
    ) -> Result<OwnedPeerWakeStep, String> {
        use crate::session::selection::LifecycleEffectSlot;
        #[cfg(test)]
        let fixture_control = self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control);
        #[cfg(test)]
        let fixture_continuation = match &fixture_control {
            Some(control) => execution
                .continuation
                .clone()
                .with_fixture_control(control.clone()),
            None => execution.continuation.clone(),
        };
        #[cfg(test)]
        let continuation = &fixture_continuation;
        #[cfg(not(test))]
        let continuation = &execution.continuation;
        #[cfg(test)]
        if !continuation
            .fixture_reached(crate::session::selection::ReplacementFixtureCut::SourceCoreEntered)
            .await
        {
            return Ok(OwnedPeerWakeStep::Retained);
        }
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        if !continuation.validate_source().await {
            return Ok(OwnedPeerWakeStep::Retained);
        }
        if !prepare_replacement_context(execution, continuation, &manager, &pty).await? {
            return Ok(OwnedPeerWakeStep::Retained);
        }
        #[cfg(test)]
        let fixture = fixture_control.filter(|control| {
            !control
                .ticket_only
                .load(std::sync::atomic::Ordering::Acquire)
        });
        #[cfg(test)]
        let fixture_created =
            match execute_peer_replacement_fixture(app, execution, continuation, fixture).await {
                PeerReplacementFixture::Retained => return Ok(OwnedPeerWakeStep::Retained),
                PeerReplacementFixture::Created(created) => Some(*created),
                PeerReplacementFixture::NotUsed => None,
            };
        #[cfg(test)]
        let created = match fixture_created {
            Some(created) => created,
            None => {
                let proof = replacement_target_ownership(execution)?;
                crate::commands::session::create_owned_response_replacement(
                    app,
                    execution.request.clone(),
                    continuation.clone(),
                    &proof,
                )
                .await
            }
        };
        #[cfg(not(test))]
        let created = {
            let proof = replacement_target_ownership(execution)?;
            crate::commands::session::create_owned_response_replacement(
                app,
                execution.request.clone(),
                continuation.clone(),
                &proof,
            )
            .await
        };
        match created {
            Ok(crate::commands::session::OwnedReplacementCreateStep::Finalized(_)) => {}
            Ok(crate::commands::session::OwnedReplacementCreateStep::Retained) => {
                return Ok(OwnedPeerWakeStep::Retained)
            }
            Err(error) => return Err(error),
        }
        let Some(info) = continuation.refresh_completion_info(&manager, &pty).await else {
            return Ok(OwnedPeerWakeStep::Retained);
        };
        let id = continuation.binding().map(|binding| binding.session_id());
        let Some(id) = id else {
            return Ok(OwnedPeerWakeStep::Retained);
        };
        // Start the work receipt only after its initial current-B checks. A
        // failed preflight has reached no bridge, settle or input effect.
        let Some(effect) = continuation.start_effect(LifecycleEffectSlot::WorkTransfer) else {
            return Ok(OwnedPeerWakeStep::Retained);
        };
        if info.telegram_bot_id.is_some() {
            self.attach_persisted_telegram_for_wake(app, id, info.telegram_bot_id.as_deref())
                .await;
        }
        let settle = match self.wait_for_spawned_wake_idle(app, id).await {
            Ok(settle) => settle,
            Err(error) => {
                effect.complete(Err(error.clone()));
                continuation.note_retained_work(error);
                return Ok(OwnedPeerWakeStep::Retained);
            }
        };
        if continuation
            .refresh_completion_info(&manager, &pty)
            .await
            .is_none()
        {
            // The bridge/settle calls returned; no input was attempted. Keep
            // their known incomplete-work result instead of an unwind receipt.
            effect.complete(Err("replacement work stopped before input".into()));
            return Ok(OwnedPeerWakeStep::Retained);
        }
        let injected = self
            .inject_wake_into_pty(
                app,
                id,
                &execution.message,
                execution.delivery,
                settle,
                (execution.response.as_ref(), Some(continuation)),
            )
            .await;
        if let Err(error) = injected {
            effect.complete(Err(error.clone()));
            continuation.note_retained_work(error);
            return Ok(OwnedPeerWakeStep::Retained);
        }
        drop(execution.exact.take());
        drop(execution.stripe.take());
        continuation
            .finish_observed_work_transfer(&manager, &pty, effect, true)
            .await;
        Ok(OwnedPeerWakeStep::Delivered(replacement_delivery(
            execution,
        )))
    }

    async fn try_inject_peer_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        delivery: (WakeDeliveryOrigin, bool),
        response: Option<&ResponseWakeWork>,
        session_id: Uuid,
        dispatch: (bool, bool),
    ) -> Result<Option<OutboxMessage>, String> {
        let (origin, comanaged) = delivery;
        let (logical_command, flags_present) = dispatch;
        if logical_command {
            let shell = {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                mgr.get_session(session_id)
                    .await
                    .map(|session| session.shell)
            };
            let Some(shell) = shell else {
                log::warn!(
                    "[mailbox] wake: candidate {} vanished during logical-command preflight, \
                    trying next",
                    session_id
                );
                return Ok(None);
            };
            if let Some(wire_value) = msg.command.as_deref() {
                resolve_remote_pty_command(&shell, wire_value)?;
            }
        }

        // (#1001 PR2 / B) Settle the live session to paste-ready before
        // injecting. The MEASURED live-path drop is the still-starting
        // case (100% here / 83% baseline, P2), which the alive_age
        // Starting route closes via the #611 settle. The established
        // fresh-idle guard is DEFENSIVE retention (bias-to-deliver, cannot
        // drop - the cap always injects), not a measured-drop closer; the
        // ~75% figure was PR1's COLD-SPAWN fresh-idle measurement, not this
        // established path. Busy/mid-turn and long-idle sessions inject at
        // once; only a freshly-idle established one waits the guard.
        // Best-effort - a vanished session just falls through to the
        // inject's own race handling below.
        let settle = self.settle_live_before_inject(app, session_id).await;
        match self
            .inject_wake_into_pty(
                app,
                session_id,
                msg,
                (origin, comanaged),
                settle,
                (response, None),
            )
            .await
        {
            Ok(()) => {
                // #1635-2 D1: a live target cannot apply dispatch
                // flags — deliver as today and annotate the receipt.
                // A plain wake stays byte-identical (no annotation).
                let mut delivered = msg.clone();
                if flags_present {
                    delivered.dispatch_not_applied = Some("live-target".to_string());
                }
                Ok(Some(delivered))
            }
            Err(e) if err_is_pty_session_missing(&e) => {
                // Race: PTY died between `find_live_candidates`
                // probe and `PtyManager::write`. Load-bearing
                // safety net for the dropped per-iteration
                // re-read (grinch G.M2). Try the next candidate.
                // Flag the race for the post-loop AC5 promotion
                // (grinch HIGH-1) — if every Inject candidate
                // races to dead, the spawn-persistent fall-through
                // must still set spawn_with_resume.
                log::warn!(
                    "[mailbox] wake: candidate {} died after liveness probe ({}), trying next",
                    session_id,
                    e
                );
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) async fn select_peer_wake_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        delivery: (WakeDeliveryOrigin, bool),
        response: Option<&ResponseWakeWork>,
        dispatch: (bool, bool),
    ) -> Result<std::ops::ControlFlow<OutboxMessage, PeerWakeFallback>, String> {
        let (logical_command, flags_present) = dispatch;
        // Whether the spawn-fallback should allow provider auto-resume.
        // Default false: cold wake — no SessionManager record at this CWD.
        // Promoted to true in two paths below: (a) RespawnExited deferred-
        // destroy block, (b) phantoms-only fall-through (every CWD match
        // was a desync phantom — preserve any on-disk transcript via
        // auto-resume). See issue #223 round-1 resolution.
        //
        // MUST NOT be re-derived after `destroy_session_inner` runs: post-
        // destroy, the candidate would vanish from list_sessions and the
        // value would silently flip, regressing the deferred-non-coord
        // wake by losing `--continue`. Set the flag inside the candidate
        // loop only. See plan §4.5.a / round-1 G7 / round-3 R3.2.
        let mut spawn_with_resume = false;
        let mut pending_exited_destroy: Option<Uuid> = None;
        let mut pending_exited_telegram_bot_id: Option<String> = None;
        let mut pending_exited_communication: Option<
            crate::session::session::SessionCommunication,
        > = None;
        // HIGH-1 (Step-7 review): symmetric AC5 protection. Tracks whether
        // every iter'd Inject candidate hit the `err_is_pty_session_missing`
        // race arm — if so, the post-loop fall-through must still promote
        // spawn_with_resume so the on-disk transcript isn't abandoned. Same
        // outcome as grinch G.H2's phantoms-only path, just a different
        // upstream cause (race-killed siblings vs. desync phantoms).
        let mut lost_inject_to_race = false;

        // Issue #223: enumerate ALL viable CWD candidates (PTY-liveness filtered)
        // with their captured status, plus a had_any_match flag for the phantoms-
        // only AC5-preservation path (grinch G.H2). Iterate so a stale-Running
        // phantom no longer blocks delivery to the live Idle recipient at the
        // same CWD; defer Exited destroys until later Inject attempts fail
        // (grinch G.H1) — preserves AC5's "first Exited wins respawn slot".
        //
        // #293: Root Agent recipient uses the `is_root_agent` flag lookup,
        // not CWD-FQN match, since `agent_fqn_from_path(ac-root-agent)` does
        // not produce `ROOT_AGENT_SENDER`. Exited records are filtered out by
        // `find_root_session_candidate` so the deferred-destroy block never
        // fires on a user-launched session.
        let (candidates, had_any_match) =
            if crate::config::root_agent::is_root_agent_target(&msg.to) {
                match self.find_root_session_candidate(app).await {
                    Some(pair) => (vec![pair], true),
                    None => (Vec::new(), false),
                }
            } else {
                self.find_live_candidates(app, &msg.to).await
            };

        for &(session_id, ref status) in &candidates {
            log::debug!(
                "[mailbox] wake: candidate {} captured-status={:?}",
                session_id,
                status
            );

            match wake_action_for(status) {
                WakeAction::Inject => {
                    match self
                        .try_inject_peer_candidate(
                            app,
                            msg,
                            delivery,
                            response,
                            session_id,
                            (logical_command, flags_present),
                        )
                        .await?
                    {
                        Some(delivered) => return Ok(std::ops::ControlFlow::Break(delivered)),
                        None => lost_inject_to_race = true,
                    }
                }
                WakeAction::RespawnExited => {
                    // Defer the destroy: an Inject candidate later in the list
                    // may succeed and avoid destroy+spawn entirely (grinch
                    // G.H1). Only the FIRST Exited's id is remembered —
                    // preserves AC5's "first Exited wins the respawn slot"
                    // semantic.
                    if pending_exited_destroy.is_none() {
                        pending_exited_destroy = Some(session_id);
                        let (bot_id, communication) = {
                            let session_mgr =
                                app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                            let mgr = session_mgr.read().await;
                            mgr.get_session(session_id)
                                .await
                                .map(|s| (s.telegram_bot_id.clone(), s.communication.clone()))
                                .unwrap_or((None, None))
                        };
                        pending_exited_telegram_bot_id = bot_id;
                        // (#747) carry a restored raised hand across the
                        // destroy+respawn; the wake injection is not user input
                        // and must not clear it (#676).
                        pending_exited_communication = communication;
                        spawn_with_resume = true;
                        log::debug!(
                            "[mailbox] wake: deferring Exited destroy for {} (status={:?}) pending \
                            later Inject success",
                            session_id,
                            status
                        );
                    }
                    continue;
                }
            }
        }

        // No Inject returned Ok. Three cases enable auto-resume on the spawn-
        // persistent fall-through:
        //   1. A deferred Exited destroy is pending (spawn_with_resume true).
        //   2. Candidate list empty BUT records existed in the manager — i.e.,
        //      every CWD-match was a non-Exited phantom. Spawning cold would
        //      abandon any on-disk transcript at the matched CWD (grinch G.H2
        //      AC5 micro-regression).
        //   3. Every viable Inject candidate died mid-flight (all hit the
        //      `err_is_pty_session_missing` race arm). Symmetric to case 2 —
        //      same AC5 outcome since cold spawn would abandon the transcript
        //      (grinch HIGH-1, Step-7 review).
        if pending_exited_destroy.is_none()
            && (candidates.is_empty() && had_any_match || lost_inject_to_race)
        {
            spawn_with_resume = true;
            log::debug!(
                "[mailbox] wake: all CWD candidates for '{}' are phantoms or lost to race; \
                enabling auto-resume on spawn-persistent",
                msg.to
            );
        }

        Ok(std::ops::ControlFlow::Continue((
            spawn_with_resume,
            pending_exited_destroy,
            pending_exited_telegram_bot_id,
            pending_exited_communication,
        )))
    }

    async fn spawn_peer_wake_fallback<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        plan: ResolvedWakeSpawnPlan,
        spawn_with_resume: bool,
        flags_present: bool,
    ) -> Result<SpawnedPeerWake, String> {
        let ResolvedWakeSpawnPlan {
            resolved_command,
            cwd,
            session_name,
            spawn_shell,
            spawn_args,
            spawn_label,
            configured_spawn,
            resolved_agent_host_shell,
        } = plan;
        let spawn_source = resolved_command.source.clone();
        let spawn_raw = resolved_command.raw_command.clone();

        log::info!(
            "[mailbox] wake: spawning '{}' from {}: raw_command={:?}, shell={:?}, args={:?}",
            msg.to,
            spawn_source,
            spawn_raw,
            spawn_shell,
            spawn_args
        );

        if let Some(spawn) = configured_spawn.as_ref() {
            crate::config::agent_command::prepare_agent_spawn_command(spawn)?;
        }
        // #1635-2 D4: `configured_spawn` is moved into the spawn below; keep a
        // clone so the delivered-receipt annotation can read the trusted agent
        // id and the profile resolution that actually launched.
        let configured_spawn_for_annotation = configured_spawn.clone();
        let info = self
            .spawn_wake_session(
                app,
                msg,
                &resolved_command,
                cwd,
                session_name,
                spawn_with_resume,
                flags_present, // D5: dispatch-flag wakes suppress tooling writes
                spawn_shell.clone(),
                spawn_args.clone(),
                spawn_label,
                configured_spawn,
                resolved_agent_host_shell,
            )
            .await
            .map_err(|e| {
                format!(
                    "Failed to spawn session for '{}': command from {} resolved raw_command={:?}, \
                    shell={:?}, args={:?}: {}",
                    msg.to, spawn_source, spawn_raw, spawn_shell, spawn_args, e
                )
            })?;

        let session_id =
            Uuid::parse_str(&info.id).map_err(|e| format!("Failed to parse session id: {}", e))?;

        Ok((
            session_id,
            configured_spawn_for_annotation,
            resolved_command,
        ))
    }

    async fn finish_peer_wake_fallback<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        delivery: (WakeDeliveryOrigin, bool),
        response: Option<&ResponseWakeWork>,
        restored: (
            Option<String>,
            Option<Uuid>,
            Option<crate::session::session::SessionCommunication>,
        ),
        spawned: (bool, SpawnedPeerWake),
    ) -> Result<OutboxMessage, String> {
        let (origin, comanaged) = delivery;
        let (
            pending_exited_telegram_bot_id,
            mut pending_exited_orphan_to_clear,
            mut pending_exited_communication,
        ) = restored;
        let (flags_present, (session_id, configured_spawn_for_annotation, resolved_command)) =
            spawned;
        if pending_exited_telegram_bot_id.is_some() {
            self.attach_persisted_telegram_for_wake(
                app,
                session_id,
                pending_exited_telegram_bot_id.as_deref(),
            )
            .await;
        }

        // (#747) Single-carrier rule. If the deferred destroy failed, the
        // orphan Exited record still holds the restored hand while the carry
        // below plants it on the new session. Clear the orphan's copy now
        // (the spawn succeeded, so the live session is the carrier) and tell
        // the frontend; otherwise the stale orphan badge survives attending
        // the live session, and deduplicate (name+cwd, first-kept) can
        // persist the orphan's Exited+hand row, resurrecting an already
        // attended hand on the next restart.
        if let Some(orphan_id) = pending_exited_orphan_to_clear.take() {
            let cleared = {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                mgr.clear_communication_if_kind(orphan_id, SessionCommunicationKind::RaiseHand)
                    .await
            };
            if cleared {
                crate::session::selection::publish_session_communication(app, orphan_id, None);
            }
        }

        // (#747) Re-apply a raised hand carried from the destroyed dormant
        // record. spawn_with_resume is true on this path (set in the
        // RespawnExited arm), so the resumed conversation keeps its pending
        // user-attention marker until real user input.
        if let Some(communication) = pending_exited_communication.take() {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let restored = {
                let mgr = session_mgr.read().await;
                mgr.restore_communication(session_id, communication.clone())
                    .await
            };
            if restored {
                crate::session::selection::publish_session_communication(
                    app,
                    session_id,
                    Some(&communication),
                );
            }
        }

        let settle = self.wait_for_spawned_wake_idle(app, session_id).await?;

        // Inject message — interactive mode (session persists, user sees reply instructions)
        self.inject_wake_into_pty(
            app,
            session_id,
            msg,
            (origin, comanaged),
            settle,
            (response, None),
        )
        .await?;

        // #1635-2 D4: the delivered JSON reports the effective agent and
        // profile when dispatch flags were honored on this spawn. When the
        // agent is not configured (no trusted spawn), only the resolved agent
        // id is annotated; profile fields stay absent.
        let mut delivered = msg.clone();
        if flags_present {
            if let Some(spawn) = configured_spawn_for_annotation.as_ref() {
                delivered.effective_agent_id = Some(spawn.trusted_agent_id.clone());
                delivered.effective_profile =
                    Some(spawn.profile_resolution.effective_profile.clone());
                delivered.profile_fallback_applied = spawn.profile_resolution.fallback_applied;
            } else {
                delivered.effective_agent_id = resolved_command.agent_id.clone();
            }
        }
        Ok(delivered)
    }

    pub(crate) async fn deliver_peer_wake_fallback<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        delivery: (WakeDeliveryOrigin, bool),
        response: Option<&ResponseWakeWork>,
        dispatch: (bool, bool),
        fallback: PeerWakeFallback,
    ) -> Result<OutboxMessage, String> {
        let (logical_command, flags_present) = dispatch;
        let (
            spawn_with_resume,
            pending_exited_destroy,
            pending_exited_telegram_bot_id,
            pending_exited_communication,
        ) = fallback;
        // (#747) Set only when the deferred destroy FAILS: the orphan Exited
        // record then still holds the restored hand and must be cleared once
        // the replacement spawn succeeds (single-carrier rule, 5d).
        let mut pending_exited_orphan_to_clear: Option<Uuid> = None;
        // No viable Inject candidate succeeded, so the fallback would spawn a
        // persistent session. Root Agent remains user-launched only.
        log::info!(
            "[mailbox] wake: no active session for '{}', spawning persistent session",
            msg.to
        );
        if crate::config::root_agent::is_root_agent_target(&msg.to) {
            let restoring = app
                .state::<Arc<crate::RestoreInProgress>>()
                .0
                .load(std::sync::atomic::Ordering::SeqCst);
            return if restoring {
                Err(format!(
                    "Root Agent session not yet restored for '{}'; daemon restart in progress — \
                    will retry.",
                    msg.to
                ))
            } else {
                Err(format!(
                    "No live Root Agent session for '{}'. The Root Agent must be running locally \
                    to receive messages — ask the user to launch it.",
                    msg.to
                ))
            };
        }

        // A logical action must be mapped against the exact carried spawn shell
        // before an Exited record is destroyed or provider directories are made.
        let mut spawn_plan = if logical_command {
            let plan = self.resolve_wake_spawn_plan(app, msg).await?;
            if let Some(wire_value) = msg.command.as_deref() {
                resolve_remote_pty_command(&plan.spawn_shell, wire_value)?;
            }
            Some(plan)
        } else {
            None
        };

        if let Some(exited_id) = pending_exited_destroy {
            log::debug!(
                "[mailbox] wake: executing deferred destroy for Exited candidate {}",
                exited_id
            );
            if let Err(e) = self.destroy_exited_wake_session(app, exited_id).await {
                // Best-effort. Orphan SessionManager record lingers until AC3's
                // runtime-dedup path (`#223-fu1`) drains it. spawn-persistent
                // below still fires with the correct spawn_with_resume flag.
                // (grinch G.M5 option (c).)
                log::error!(
                    "[mailbox] wake: failed to destroy exited session {} (orphan will linger until \
                    AC3): {}",
                    exited_id,
                    e
                );
                // (#747) The orphan Exited record still holds its restored
                // raised hand. Do NOT clear it here: if the spawn below also
                // fails, the orphan must stay the sole carrier (the hand is
                // not lost with the failed wake). Remember it so 5d moves the
                // hand to the new session once the spawn succeeds.
                pending_exited_orphan_to_clear = Some(exited_id);
            }
        }

        // Normal messages retain their original ordering: resolve the plan only
        // after any deferred Exited destroy. Logical actions already carry the
        // read-only plan proven safe above.
        if spawn_plan.is_none() {
            spawn_plan = Some(self.resolve_wake_spawn_plan(app, msg).await?);
        }
        let plan = spawn_plan
            .ok_or_else(|| "Internal error: wake spawn plan was not resolved".to_string())?;
        let spawned = self
            .spawn_peer_wake_fallback(app, msg, plan, spawn_with_resume, flags_present)
            .await?;
        self.finish_peer_wake_fallback(
            app,
            msg,
            delivery,
            response,
            (
                pending_exited_telegram_bot_id,
                pending_exited_orphan_to_clear,
                pending_exited_communication,
            ),
            (flags_present, spawned),
        )
        .await
    }

    /// Deliver a validated AgentsCommander-generated notice through the existing wake,
    /// resume, background-spawn, settle, and canonical injection plumbing. No serialized
    /// message field can select this path.
    pub(crate) async fn deliver_internal_system_notice<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
        cancellation: CancellationToken,
        guard: InternalNoticeGuard,
    ) -> Result<(), String> {
        self.deliver_wake_engine(
            app,
            WakeDelivery::InternalSystem {
                target,
                notice,
                cancellation,
                guard,
            },
        )
        .await
        .map(|_| ())
    }
}
