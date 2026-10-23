use crate::commands::session::attach_local_config_telegram_if_any;
use crate::commands::session::attach_persisted_telegram_if_configured;
use crate::commands::session::execute_restart_transaction_in_context;
use crate::commands::session::prepare_restore_lifecycle_authority;
use crate::commands::session::publish_restart_destroyed;
use crate::commands::session::raise_room_reader_demand;
use crate::commands::session::CreateCompletion;
use crate::commands::session::DeferredCreateOutput;
use crate::commands::session::LifecycleEntryAuthority;
use crate::config::settings::AppSettings;
use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::CommitDecision;
use crate::session::manager::LifecycleMutations;
use crate::session::manager::SessionManager;
use crate::session::selection::SelectionCause;
use crate::session::selection::SelectionCoordinator;
use crate::session::selection::SelectionSource;
use crate::session::selection::SelectionTransaction;
use crate::session::selection::TrustedRestartIntent;
use crate::session::session::SessionCommunication;
use crate::session::session::SessionCommunicationKind;
use crate::session::session::SessionInfo;
use crate::telegram::manager::TelegramBridgeState;
use crate::DetachedSessionsState;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use uuid::Uuid;

pub(crate) async fn accept_restart_deferred_completion<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    completion: Result<CreateCompletion, String>,
    context: &LifecycleExecutionContext<'_>,
    uuid: uuid::Uuid,
    old_lost: bool,
    old_selected: bool,
    intent: TrustedRestartIntent,
) -> Result<DeferredCreateOutput, String> {
    Ok(match completion {
        Ok(CreateCompletion::Deferred(deferred)) => deferred,
        Ok(CreateCompletion::Retained) => {
            if let Some(authority) = context.authority() {
                authority
                    .interrupted
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            return Err(
                crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
            );
        }
        Ok(CreateCompletion::Finalized(_)) => {
            let error = "restart replacement was published before atomic finalization".to_string();
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(error);
        }
        Err(error) => {
            if context.authority().is_some_and(|authority| {
                authority
                    .interrupted
                    .load(std::sync::atomic::Ordering::Acquire)
            }) {
                return Err(error);
            }
            if old_lost {
                if let Err(finalize_error) =
                    finalize_failed_restart(transaction, uuid, old_selected, intent).await
                {
                    return Err(format!(
                        "{}; restart failure finalization failed: {}",
                        error, finalize_error
                    ));
                }
            }
            return Err(error);
        }
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_restart_deferred<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    deferred: &DeferredCreateOutput,
    uuid: uuid::Uuid,
    old_lost: bool,
    old_selected: bool,
    activate_after: bool,
    intent: TrustedRestartIntent,
    restart_start_fresh: bool,
) -> Result<
    (
        uuid::Uuid,
        crate::session::manager::CommitResult,
        SessionInfo,
    ),
    String,
> {
    let new_uuid = match Uuid::parse_str(&deferred.info.id) {
        Ok(id) => id,
        Err(error) => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(format!(
                "restart replacement returned an invalid id: {error}"
            ));
        }
    };
    let live = match transaction.live_decision(new_uuid) {
        Some(CommitDecision::Live(live)) => live,
        _ => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err("restart replacement is not displayable".to_string());
        }
    };

    // 5. Finalize the replacement, remove the old row, and decide selection in
    // one manager commit. Neither lifecycle row was public before this point.
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(deferred.binding, live);
    mutations.remove(uuid);
    if deferred.effective_continuation && !restart_start_fresh {
        let manager = transaction.manager().await;
        if let (Some(source), Some(target)) = (
            manager.get_session(uuid).await,
            manager.get_pending_session(deferred.binding).await,
        ) {
            if source.agent_id.is_some()
                && source.agent_id == target.agent_id
                && source.requested_profile == target.requested_profile
                && source.effective_profile == target.effective_profile
                && source.working_directory == target.working_directory
                && !source.start_fresh_on_restore
                && !target.start_fresh_on_restore
            {
                mutations.transfer_prompt(uuid, deferred.binding);
            }
        }
    }
    let decision = if old_selected || activate_after {
        CommitDecision::Live(live)
    } else {
        CommitDecision::Keep
    };
    let committed = match transaction
        .commit_entry_create(
            deferred.binding,
            decision,
            SelectionCause::Restart(intent),
            mutations,
        )
        .await
    {
        Ok(committed) => committed,
        Err(error) => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(error);
        }
    };
    let session_info = committed
        .finalized_rows
        .iter()
        .find(|row| row.id == new_uuid.to_string())
        .cloned()
        .ok_or_else(|| "restart finalization did not produce the replacement row".to_string())?;
    Ok((new_uuid, committed, session_info))
}

pub(crate) async fn publish_restart_before_warnings<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    uuid: uuid::Uuid,
    new_uuid: uuid::Uuid,
    binding: crate::session::manager::PendingCreateBinding,
    committed: &crate::session::manager::CommitResult,
    session_info: &SessionInfo,
    old_lost: bool,
) -> Result<Option<crate::session::selection::LifecycleEffectGuard>, String> {
    let app = transaction.app();
    let publication = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::Publication);
    if transaction.has_lifecycle_entry() && publication.is_none() {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    // Section 5.2: the commit removed the old row. Detach its bridge (dormant
    // old) and sweep every old-id reader demand raised since teardown before
    // the replacement is published or attached; then evaluate the new uuid's
    // own Room eligibility, because the create-time raise was suppressed for
    // `SelectionCause::Restart(_)`. Exactly one demand per consumer, one reader
    // and one slot may exist.
    if !old_lost {
        detach_restart_telegram(transaction, uuid).await;
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&uuid);
    }
    crate::commands::telegram::release_all_reader_demands(app, uuid).await;
    raise_room_reader_demand(app, new_uuid).await;
    transaction.observe_entry_reader_demand(
        binding,
        crate::session::selection::EntryReaderDemandReceipt::Returned,
    );
    transaction
        .persist(SelectionSource::Restart, Some(new_uuid))
        .await;
    transaction.publish_created(session_info);
    publish_restart_destroyed(transaction, uuid);
    if let Some(selection) = committed.selection.as_ref() {
        transaction.publish_selection(selection);
    }
    Ok(publication)
}

pub(crate) async fn publish_restart_after_warnings<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    new_uuid: uuid::Uuid,
    session_info: SessionInfo,
    restart_start_fresh: bool,
    telegram_bot_id: Option<&str>,
    cwd: &str,
    publication: Option<crate::session::selection::LifecycleEffectGuard>,
) -> Result<SessionInfo, String> {
    let app = transaction.app();
    // (#756) C3: mirror the honored fresh intent into the coordinator clocks so
    // it survives record destruction (idle auto-close, manual close destroy the
    // record that carries start_fresh_on_restore). Self-gates on coordinators
    // inside the helper; root agents are never coordinators, so this is a no-op
    // for them. Self-handoff-and-switch (#668) respawns through this function
    // and inherits the mirror automatically.
    if restart_start_fresh {
        crate::commands::pty::write_start_fresh_mirror_for_session(app, new_uuid, true).await;
    }

    // 6. Re-attach Telegram bridge from live persisted intent, or fall back to repo config.
    if telegram_bot_id.is_some() {
        attach_persisted_telegram_if_configured(app, new_uuid, telegram_bot_id).await;
    } else {
        attach_local_config_telegram_if_any(app, new_uuid, cwd).await;
    }

    if let Some(publication) = publication {
        publication.complete(Ok(Some(session_info.id.clone())));
    }
    Ok(session_info)
}

/// Resolves the effective `skip_auto_resume` flag for `restart_session`.
/// Defaults to `true` (fresh conversation) to preserve existing restart-button semantics.
/// `Some(false)` is used by the deferred-wake path (ProjectPanel.handleReplicaClick)
/// to allow provider auto-resume and continue the prior conversation.
pub(crate) fn effective_restart_skip_auto_resume(requested: Option<bool>) -> bool {
    requested.unwrap_or(true)
}

/// (#630/#631) Compose the restart path's effective fresh intent: fresh on an
/// explicit "Restart Session" (`requested` defaults to `true`), OR if the session
/// still carries an unconsumed durable fresh intent (`stored_start_fresh`). A
/// deferred member reopened via Branch A passes `Some(false)`, but its persisted
/// intent wins, so it stays fresh; a normal member (`false || false`) resumes,
/// unchanged. Named so the composition is unit-tested; the call site is guarded
/// against a silent revert by the `dead_code` lint under `-D warnings` (CI runs
/// `cargo clippy --all-targets -- -D warnings`), which fails the build if
/// reverting to the inline expression leaves this function unused. Mirrors
/// `lib::skip_auto_resume_for_restore`.
pub(crate) fn restart_skip_auto_resume_with_intent(
    stored_start_fresh: bool,
    requested: Option<bool>,
) -> bool {
    stored_start_fresh || effective_restart_skip_auto_resume(requested)
}

/// (#747) Decide whether a raised hand carries across the restart
/// destroy+create boundary. Carry ONLY a visible `RaiseHand` and ONLY when the
/// conversation resumes (`restart_start_fresh == false`, i.e. the Branch-A
/// reopen of a dormant session). A fresh restart starts a new conversation;
/// the raise belonged to the old one and is dropped, consistent with
/// "destroyed sessions leave no stale raised-hand state" (#747 AC 4).
///
/// Seam map: the explicit "Restart Session" command and the bulk
/// profile-assignment restart (commands/config.rs:627-637) pass fresh and drop
/// (user-initiated resets); the sidebar dormant reopen passes `Some(false)`
/// and carries; the startup wake arm reuses this helper with the persisted
/// intent (#747 plan change 3); the agent-initiated self-clear-and-handoff
/// restart carries via its own capture/re-apply (#747 plan change 10) because
/// it is not user input. `pub(crate)` so lib.rs's wake arm shares the exact
/// same decision.
pub(crate) fn carry_communication_for_restart(
    stored: Option<SessionCommunication>,
    restart_start_fresh: bool,
) -> Option<SessionCommunication> {
    if restart_start_fresh {
        return None;
    }
    stored.filter(|c| c.kind == SessionCommunicationKind::RaiseHand && c.visible)
}

/// (#599) Resolves the effective `skip_auto_resume` for the `create_session`
/// command. Defaults to `true` (fresh conversation) so every existing
/// create-in-place / new-agent / open-agent / CLI / web call site keeps its
/// intentional "no --continue on a fresh create" semantics. The
/// reopen-of-a-closed-coordinator path (ProjectPanel.handleReplicaClick) passes
/// `Some(false)` to resume the prior conversation. Mirror of
/// `effective_restart_skip_auto_resume` so both create and restart routes share
/// the same default.
pub(crate) fn effective_create_skip_auto_resume(requested: Option<bool>) -> bool {
    requested.unwrap_or(true)
}

/// (#756) Create-path mirror consume: force a fresh spawn when the caller
/// requested resume but the cwd's coordinator carries a pending fresh boundary.
/// Named + unit-tested per the #630 pattern (the call site is guarded against a
/// silent inline revert by the `dead_code` lint under CI's `-D warnings`).
pub(crate) fn mirror_forces_fresh(
    requested_skip_auto_resume: bool,
    is_coordinator: bool,
    mirror_pending: bool,
) -> bool {
    !requested_skip_auto_resume && is_coordinator && mirror_pending
}

pub(crate) fn effective_restart_requested_profile(
    requested: Option<String>,
    stored: Option<String>,
) -> Option<String> {
    requested.or(stored)
}

/// #2434 - one matched selection, read by both the restart and the drift badge
/// so a Reload launches exactly what the badge compared against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MatchedSelection {
    /// No coding-agent reference at all: restart the saved shell. Today's path.
    NoAgent,
    /// A reference that resolved.
    Agent {
        agent_id: String,
        /// The letter to launch, straight from `AgentMatch.profile_letter`;
        /// `None` whenever no letter was chosen. An absent request stays absent.
        profile_letter: Option<String>,
        /// True only when `tier.is_some()` and `profile_letter` is `Some`: a
        /// matched letter outranks the replica pin for this spawn.
        authoritative: bool,
        tier: Option<crate::config::agent_command::MatchTier>,
        original_letter: Option<String>,
    },
    /// A reference that exists and resolves to nothing.
    Unresolved { reference: String },
}

/// #2434 - pure, read-only: pick the reference a restart of `cwd` would launch
/// (explicit request, then `tooling.currentCodingAgent`, then the session's own
/// agent) and resolve it through `resolve_portable_reference`. Never falls back
/// to another agent and never writes the reference back.
pub(crate) fn matched_selection(
    settings: &AppSettings,
    cwd: &str,
    requested_agent_id: Option<&str>,
    stored_agent_id: Option<&str>,
    source_letter: Option<&str>,
) -> MatchedSelection {
    let dir = std::path::Path::new(cwd);
    // #2786 (C1) - ONE load through the shared loader serves both
    // `currentCodingAgent` and the descriptor, so they share one snapshot.
    let config = match crate::config::agent_config::read_agent_local_config_json(dir) {
        Ok(value) => value,
        Err(e) => {
            log::warn!(
                "[agent-match] malformed '{}', no descriptor: {}",
                dir.join("config.json").display(),
                e
            );
            None
        }
    };
    let current = config
        .as_ref()
        .and_then(|value| value.get("tooling")?.get("currentCodingAgent")?.as_str())
        // An empty or whitespace-only id names nothing: it is absent, always.
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    // Only a stale `currentCodingAgent` on a session with no agent of its own
    // may degrade to the saved shell; an explicit request never does.
    let plain_shell_selection =
        requested_agent_id.is_none() && stored_agent_id.is_none() && current.is_some();
    let Some(id) = requested_agent_id
        .map(str::to_string)
        .or(current)
        .or_else(|| stored_agent_id.map(str::to_string))
    else {
        return MatchedSelection::NoAgent;
    };
    let mut reference =
        crate::config::agent_command::StoredReference::from_config(config.as_ref(), &id);
    reference.source_letter = source_letter.map(str::to_string);
    match crate::config::agent_command::resolve_portable_reference(settings, &reference) {
        crate::config::agent_command::MatchOutcome::Matched(found) => MatchedSelection::Agent {
            authoritative: found.tier.is_some() && found.profile_letter.is_some(),
            agent_id: found.agent_id,
            profile_letter: found.profile_letter,
            tier: found.tier,
            original_letter: found.original_letter,
        },
        crate::config::agent_command::MatchOutcome::NoMatch { reference } => {
            if plain_shell_selection {
                // Nothing else to launch: the session already is a plain shell.
                log::warn!(
                    "[agent-match] ignoring unresolvable currentCodingAgent '{}' in '{}'; \
                     restarting the saved shell",
                    reference,
                    cwd
                );
                MatchedSelection::NoAgent
            } else {
                MatchedSelection::Unresolved { reference }
            }
        }
    }
}

/// #537 read-side: pick which coding agent a restart should launch.
///
/// Ranks an explicit request first, then the replica's Selection-UI assignment
/// (`tooling.currentCodingAgent` in `cwd/config.json`, validated against the
/// configured agents), then the agent the live session was launched with. This
/// mirrors the wake path (#534): a fresh user selection must override stale
/// launch history. The `currentCodingAgent` is validated so an unconfigured id
/// is never passed downstream (which would make `build_configured_agent_spawn_for_cwd`
/// return `Ok(None)` and silently keep the old recipe). Root agents and plain
/// terminals have no `currentCodingAgent`, so they fall straight through to
/// `stored_agent_id` and their behavior is unchanged.
pub(crate) fn resolve_restart_selected_agent_id(
    settings: &AppSettings,
    cwd: &str,
    requested_agent_id: Option<&str>,
    stored_agent_id: Option<&str>,
) -> Option<String> {
    if let Some(requested) = requested_agent_id {
        return Some(requested.to_string());
    }
    let current_selection =
        crate::config::coding_agent_profiles::read_replica_current_coding_agent(
            std::path::Path::new(cwd),
        )
        .filter(|id| settings.agents.iter().any(|agent| agent.id == *id));
    current_selection.or_else(|| stored_agent_id.map(str::to_string))
}

#[derive(Debug, Clone)]
pub(crate) struct RestartJobRequest {
    pub session_id: Uuid,
    pub agent_id: Option<String>,
    pub requested_profile: Option<String>,
    pub skip_auto_resume: Option<bool>,
    pub activate_after: bool,
    pub intent: TrustedRestartIntent,
    pub communication_override: Option<SessionCommunication>,
    /// §1295 5.1b — creation-gate enforcement for the restart replacement.
    /// Root-agent and archived-root cwds are exempt by the gate itself, so
    /// callers pass the build default; the mailbox wake-restart passes
    /// `Enforce` explicitly (§1295 6.6 / mailbox.rs:9796).
    pub enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
}

/// Restart a session: destroy the existing one and recreate it with the same
/// configuration but a fresh PTY. By default suppresses provider auto-resume
/// (true user-intent restart — fresh conversation).
///
/// Callers that are instead *waking* a previously-deferred session pass
/// `skip_auto_resume = Some(false)`. A session is "deferred" (PTY `Exited(0)`
/// at startup) for any of the following reasons:
///
///   1. `restoreCoordinatorWakeState = false` (the new-policy default per
///      issue #248) defers every persisted session regardless of agent type.
///   2. `restoreCoordinatorWakeState = true` defers all non-coordinator team
///      members (these are never auto-woken on startup under the new policy).
///   3. `restoreCoordinatorWakeState = true` defers coordinators whose PTY
///      status was `Exited(_)` at the prior shutdown (the state-sensitive
///      branch added by issue #248).
///   4. The user explicitly closed the session during the prior run.
///
/// In all four cases, `skip_auto_resume = Some(false)` allows the next PTY
/// spawn to inject `claude --continue`, `codex resume --last`, or
/// `antigravity --continue` so the prior conversation continues.
///
/// The restarted session is automatically activated, Telegram bridges are
/// re-attached, and state is persisted.
#[allow(clippy::too_many_arguments)]
pub async fn restart_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
) -> Result<SessionInfo, String> {
    restart_session_inner_with_activation(
        app,
        session_mgr,
        pty_mgr,
        settings,
        uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn restart_session_inner_with_activation<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
    activate_after: bool,
) -> Result<SessionInfo, String> {
    restart_session_inner_with_intent(
        app,
        session_mgr,
        pty_mgr,
        settings,
        uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
        activate_after,
        TrustedRestartIntent::User,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn restart_session_inner_with_intent<R: tauri::Runtime>(
    app: &AppHandle<R>,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: &Arc<Mutex<PtyManager>>,
    _settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
    activate_after: bool,
    intent: TrustedRestartIntent,
    communication_override: Option<SessionCommunication>,
    enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
) -> Result<SessionInfo, String> {
    let restarted = app
        .state::<SelectionCoordinator>()
        .restart_lifecycle(
            app.clone(),
            RestartJobRequest {
                session_id: uuid,
                agent_id,
                requested_profile,
                skip_auto_resume,
                activate_after,
                intent,
                communication_override,
                enforcement,
            },
        )
        .await?;

    // #2232 phase 4 section 5.2: the restart replaces the session UUID, so
    // `execute_restart_transaction` owns the reader lifecycle on both the
    // success and failure paths (release the old id, then raise the
    // replacement's own demands). Nothing reader-related is done here.

    Ok(restarted)
}

pub(crate) struct RestartTeardownError {
    pub(crate) message: String,
    pub(crate) old_lost: bool,
}

pub(crate) async fn complete_owned_replacement_retirement<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) {
    crate::session::selection::forget_response_close_registration(session_id);
    detach_restart_telegram(transaction, session_id).await;
    crate::commands::telegram::release_all_reader_demands(transaction.app(), session_id).await;
    transaction
        .app()
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&session_id);
}

pub(crate) async fn detach_restart_telegram<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) {
    let bridge_shutdown = {
        let telegram = transaction.app().state::<TelegramBridgeState>();
        let mut telegram = telegram.lock().await;
        if telegram.has_bridge(session_id) {
            telegram.detach(session_id).ok()
        } else {
            None
        }
    };
    if let Some(shutdown) = bridge_shutdown {
        if let Err(error) = transaction.app().emit(
            "telegram_bridge_detached",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        ) {
            log::warn!(
                "[restart] telegram detach publication failed session={}: {}",
                session_id,
                error
            );
        }
        shutdown.spawn_wait_or_abort();
    }
}

pub(crate) async fn teardown_old_for_restart<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    old_was_dormant: bool,
) -> Result<bool, RestartTeardownError> {
    if !transaction.runtime_snapshot(session_id).has_pty {
        if old_was_dormant {
            return Ok(false);
        }
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        detach_restart_telegram(transaction, session_id).await;
        // Section 5.2: the old runtime is confirmed gone, so its reader
        // demands are released before the replacement is created. A failure to
        // create the replacement therefore leaves no old reader behind.
        crate::commands::telegram::release_all_reader_demands(transaction.app(), session_id).await;
        return Ok(true);
    }

    let resource_monitor = transaction
        .app()
        .state::<Arc<ResourceMonitorState>>()
        .inner()
        .clone();
    if resource_monitor.has_registered_group(session_id) {
        transaction
            .app()
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .publish_stop_witness(session_id, "session-restart");
        let monitor = Arc::clone(&resource_monitor);
        match tokio::task::spawn_blocking(move || {
            monitor.kill_group(
                session_id,
                crate::resource_monitor::ResourceKillReason::SessionDestroy,
            )
        })
        .await
        {
            Ok(Ok(result)) if result.quarantined => log::warn!(
                "[restart] resource cleanup quarantined session={}: {}",
                session_id,
                result.message
            ),
            Ok(Ok(_)) => {}
            Ok(Err(error)) => log::warn!(
                "[restart] resource cleanup failed session={}: {}",
                session_id,
                error
            ),
            Err(error) => log::warn!(
                "[restart] resource cleanup task failed session={}: {}",
                session_id,
                error
            ),
        }
    }

    // The std-Mutex guard from this `let` initializer is dropped at the `;` before
    // `runtime_snapshot` re-locks the same pty Mutex below. Moving this into an
    // `if let`/`match` scrutinee or an inner blocking block would hold the guard
    // across that re-lock and re-introduce the re-entrant std-Mutex deadlock fixed
    // in resource_monitor (§1295).
    let kill_result = transaction
        .app()
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .kill(session_id);
    let old_lost = !transaction.runtime_snapshot(session_id).has_pty;
    if !old_lost {
        let message = kill_result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| "old PTY remained live after restart teardown".to_string());
        return Err(RestartTeardownError {
            message,
            old_lost: false,
        });
    }
    if let Err(error) = kill_result {
        log::warn!(
            "[restart] teardown reported an error after PTY loss session={}: {}",
            session_id,
            error
        );
    }
    transaction
        .app()
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&session_id);
    detach_restart_telegram(transaction, session_id).await;
    // Section 5.2: old runtime loss is confirmed, so the old id's reader
    // demands are released before the replacement is created.
    crate::commands::telegram::release_all_reader_demands(transaction.app(), session_id).await;
    Ok(true)
}

pub(crate) async fn finalize_failed_restart<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    selected: bool,
    intent: TrustedRestartIntent,
) -> Result<(), String> {
    let publication = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::Publication);
    if transaction.has_lifecycle_entry() && publication.is_none() {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let result = async {
        detach_restart_telegram(transaction, session_id).await;
        let mut mutations = LifecycleMutations::default();
        mutations.remove(session_id);
        let committed = transaction
            .commit(
                if selected {
                    CommitDecision::Clear
                } else {
                    CommitDecision::Keep
                },
                SelectionCause::Restart(intent),
                mutations,
            )
            .await?;
        // Section 5.2: the old row is gone, so any old-id reader demand that
        // survived teardown (or was raised while it was in flight) is released and
        // its capture slot closed before the destroyed publication.
        crate::commands::telegram::release_all_reader_demands(transaction.app(), session_id).await;
        transaction
            .persist(SelectionSource::Restart, Some(session_id))
            .await;
        publish_restart_destroyed(transaction, session_id);
        if let Some(selection) = committed.selection.as_ref() {
            transaction.publish_selection(selection);
        }
        Ok(())
    }
    .await;
    if let Some(publication) = publication {
        publication.complete(result.as_ref().map(|_| None).map_err(Clone::clone));
    }
    result
}

pub(crate) enum LifecycleExecutionContext<'a> {
    ExternalEntry(&'a LifecycleEntryAuthority),
    RestoreEntry {
        authority: &'a LifecycleEntryAuthority,
        root_directory: Option<&'a str>,
    },
}
impl LifecycleExecutionContext<'_> {
    pub(crate) fn authority(&self) -> Option<&LifecycleEntryAuthority> {
        match self {
            Self::ExternalEntry(authority) => Some(authority),
            Self::RestoreEntry { authority, .. } => Some(authority),
        }
    }
}

#[expect(
    dead_code,
    reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
)]
pub(crate) async fn execute_restart_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RestartJobRequest,
) -> Result<SessionInfo, String> {
    if !transaction.has_active_restore_capability() {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let (authority, root_directory) =
        prepare_restore_lifecycle_authority(transaction, Some(request.session_id), false).await?;
    execute_restart_transaction_in_context(
        transaction,
        request,
        &LifecycleExecutionContext::RestoreEntry {
            authority: &authority,
            root_directory: root_directory.as_deref(),
        },
    )
    .await
}

#[expect(
    clippy::large_enum_variant,
    reason = "Preserve lifecycle result and Interrupted dispatch ownership without new allocation"
)]
pub(crate) enum LifecycleEntryOutcome {
    Returned(Result<SessionInfo, String>),
    Interrupted,
}

pub(crate) async fn execute_external_restart_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RestartJobRequest,
    authority: &LifecycleEntryAuthority,
) -> LifecycleEntryOutcome {
    let result = execute_restart_transaction_in_context(
        transaction,
        request,
        &LifecycleExecutionContext::ExternalEntry(authority),
    )
    .await;
    if authority
        .interrupted
        .load(std::sync::atomic::Ordering::Acquire)
    {
        LifecycleEntryOutcome::Interrupted
    } else {
        LifecycleEntryOutcome::Returned(result)
    }
}

pub(crate) async fn accept_restart_teardown<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    teardown: Result<bool, RestartTeardownError>,
    uuid: uuid::Uuid,
    old_selected: bool,
    intent: TrustedRestartIntent,
) -> Result<bool, String> {
    match teardown {
        Ok(old_lost) => Ok(old_lost),
        Err(error) => {
            if error.old_lost {
                // Section 5.2: the old PTY is already gone, so its demands must
                // not outlive the failed restart.
                crate::commands::telegram::release_all_reader_demands(transaction.app(), uuid)
                    .await;
                if let Err(finalize_error) =
                    finalize_failed_restart(transaction, uuid, old_selected, intent).await
                {
                    return Err(format!(
                        "{}; restart failure finalization failed: {}",
                        error.message, finalize_error
                    ));
                }
            }
            Err(error.message)
        }
    }
}
