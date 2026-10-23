use crate::commands::session::create_session_inner_core;
use crate::commands::session::LifecycleEntryAuthority;
use crate::commands::session::OwnedReplacementCreateRequest;
use crate::commands::session::OwnedReplacementCreateStep;
use crate::config::agent_command::AgentSpawnCommand;
use crate::config::coordinator_clocks::ClearedCloseMarkers;
use crate::pty::backend::PtyViewport;
use crate::pty::backend::ResolvedAgentHostShell;
use crate::pty::manager::PtyManager;
use crate::resource_monitor::AgentLaunchPermit;
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::PendingCreateBinding;
use crate::session::manager::SessionManager;
use crate::session::selection::SelectionCause;
use crate::session::selection::SelectionTransaction;
use crate::session::selection::TrustedCreateIntent;
use crate::session::session::SessionCommunication;
use crate::session::session::SessionInfo;
use crate::session::session::SessionRepo;
use crate::session::warnings::SessionWarning;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use uuid::Uuid;
mod admission;
mod context_prep;
mod launch;
#[cfg(test)]
pub(crate) use admission::acquire_create_publication_region;
pub(crate) use admission::acquire_lifecycle_create_publication_region;
pub(crate) use admission::enforce_owned_unarchived_for_spawn;
pub(crate) use admission::revalidate_owned_create_effect;
#[cfg(test)]
pub(crate) use admission::try_acquire_create_publication_region;
pub(crate) use admission::with_create_resource_region;
pub(crate) use admission::CoordinatorCreateTicket;
pub(crate) use admission::CreateCapacityContext;
pub(crate) use admission::CreateResourceAdmission;
pub(crate) use admission::OwnedCreateAdmission;
pub(crate) use admission::OwnedCreateTargetLease;
pub(crate) use context_prep::materialize_replica_context;
pub(crate) use context_prep::notify_replica_context_failure;
pub(crate) use context_prep::ReplicaContextPreparation;
pub(crate) use launch::build_configured_agent_spawn_for_cwd;
pub(crate) use launch::compute_profile_outdated;
pub(crate) use launch::resolve_configured_agent_spawn_for_cwd;
pub(crate) use launch::resolve_create_session_cwd;
pub(crate) use launch::resolve_launch_auto_self_clear;
pub(crate) use launch::resolve_launch_idle_tuning;

/// #1032 + #1171 - start sampling a freshly spawned session, with both engines.
///
/// **Sessions with no agent are never registered** with the scraper or the quota engine, and
/// that rule lives HERE, once: a plain shell costs neither anything, ever. #2879 - the watcher
/// engine is asked BEFORE that rule, with the optional agent, because its internal activity
/// consumer may observe a Codex session that has no configured agent; it registers nothing
/// when there is neither an agent nor a consumer. `try_state` and not `state`, because
/// `state` panics when unmanaged and a test app manages neither engine - an absent engine is
/// simply the feature being off.
///
/// There is no race with the screen parser: `PtyManager::spawn` has already returned `Ok` at
/// the call site and the parser is registered during the spawn. The scraper's first sample is
/// 5 s away and the first watcher tick is 200 ms away, and neither can arrive before the
/// parser exists.
pub(crate) fn register_session_samplers<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: Uuid,
    agent_id: Option<String>,
) {
    if let Some(watchers) = app.try_state::<Arc<crate::pty::watchers::WatcherEngine>>() {
        watchers.register_session(id, agent_id.clone());
    }
    let Some(agent_id) = agent_id else {
        return;
    };
    if let Some(scraper) = app.try_state::<Arc<crate::pty::context_scrape::ContextScraper>>() {
        scraper.register_session(id, agent_id.clone());
    }
    if let Some(quota) = app.try_state::<Arc<crate::pty::agent_quota::AgentQuotaEngine>>() {
        quota.register_session(id, agent_id);
    }
}

/// #1171 - purge every per-session side structure a DESTROYED session leaves behind.
///
/// One helper with two call sites, because the per-session side-state purge is already
/// duplicated in this file: the destroy loop and `publish_restart_destroyed` are parallel
/// copies of each other, and adding a third thing to purge to only one of them is how a leak
/// gets written.
///
/// **The order is load-bearing.** The engine is retired FIRST, so no tick still in flight can
/// republish this session's status and recreate the entry step 2 removes. `WatcherHistory`
/// creates entries only from the engine's publish, and the engine publishes only while holding
/// its registration lock, so once `retire_session` returns nothing can bring the entry back.
///
/// **Destroyed sessions only.** A session that exits on its own is NOT destroyed: the engine
/// stops sampling it and its buffer stays, which is the case that matters - an API error or a
/// CLI crash is exactly when the evidence is worth keeping. Root-agent sessions retained as
/// `Exited` keep theirs too, because their row is still in the list.
pub(crate) fn purge_session_side_state<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    if let Some(quota) = app.try_state::<Arc<crate::pty::agent_quota::AgentQuotaEngine>>() {
        quota.retire_session(session_id);
    }
    if let Some(watchers) = app.try_state::<Arc<crate::pty::watchers::WatcherEngine>>() {
        watchers.retire_session(session_id);
    }
    if let Some(history) = app.try_state::<crate::pty::watchers::history::WatcherHistoryState>() {
        history.purge(session_id);
    }
    reset_substantive_input(app, session_id);
}

/// #871 - clear a session's substantive-input marker. (#2336) Also clears the
/// session's typing-hold entry, so the destroy and restart teardown paths drop
/// the manual padlock, the natural clock and every counted held ID with the
/// tracker they already reset.
///
/// Extracted by #1171 so it has ONE definition rather than the two inline copies the destroy
/// and restart cleanups each carried. It is a map removal, so calling it twice for the same
/// session is a no-op and not a second effect - which is what lets the destroy path keep
/// resetting every id it publishes, retained-exited ones included, while the destroyed-only
/// purge below also resets the ones it owns.
pub(crate) fn reset_substantive_input<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    if let Some(activity) = app.try_state::<crate::pty::input_activity::SubstantiveInputState>() {
        activity
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .reset(session_id);
    }
    if let Some(hold) = app.try_state::<crate::pty::input_activity::TypingHoldState>() {
        hold.lock()
            .unwrap_or_else(|error| error.into_inner())
            .reset(session_id);
    }
}

pub(crate) async fn rollback_pre_created_session<R: tauri::Runtime>(
    _app: &AppHandle<R>,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: &Arc<Mutex<PtyManager>>,
    id: Uuid,
    reason: &str,
) {
    log::warn!(
        "[session] Rolling back pre-created session {} after setup failure: {}",
        id,
        reason
    );
    // The reserved create ticket or inline transaction owns the one common
    // backend-kind-aware rollback after this setup body returns. Killing here
    // would race that finalizer and perform the same teardown twice.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateSelectionIntent {
    User,
    Background,
    Suppress,
}

impl CreateSelectionIntent {
    pub(crate) fn trusted(self) -> Option<TrustedCreateIntent> {
        match self {
            Self::User => Some(TrustedCreateIntent::User),
            Self::Background => Some(TrustedCreateIntent::Background),
            Self::Suppress => None,
        }
    }
}

pub(crate) struct DeferredCreateOutput {
    pub(crate) info: SessionInfo,
    pub(crate) binding: PendingCreateBinding,
    pub(crate) warnings: Vec<SessionWarning>,
    // Keep admission through the enclosing retained transaction's finalization.
    pub(crate) _create_admission: Arc<Mutex<OwnedCreateAdmission>>,
    pub(crate) effective_continuation: bool,
}

#[derive(Clone)]
pub(crate) struct InitialLastPrompt {
    pub(crate) text: Option<String>,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    effective_profile: Option<String>,
}

impl InitialLastPrompt {
    pub(crate) fn from_persisted(
        row: &crate::config::sessions_persistence::PersistedSession,
    ) -> Self {
        Self {
            text: row.last_prompt.clone(),
            agent_id: row.agent_id.clone(),
            requested_profile: row.requested_profile.clone(),
            effective_profile: row.effective_profile.clone(),
        }
    }

    pub(crate) fn compatible(&self, row: &crate::session::session::Session) -> bool {
        self.agent_id.is_some()
            && self.agent_id == row.agent_id
            && self.requested_profile == row.requested_profile
            && self.effective_profile == row.effective_profile
    }
}

pub(crate) enum CreateCompletion {
    Retained,
    Finalized(SessionInfo),
    Deferred(DeferredCreateOutput),
}

impl CreateCompletion {
    pub(crate) async fn into_finalized(self) -> Result<SessionInfo, String> {
        match self {
            Self::Retained => std::future::pending().await,
            Self::Finalized(info) => Ok(info),
            Self::Deferred(_) => Err("create unexpectedly deferred finalization".to_string()),
        }
    }
}

pub(crate) fn release_resource_launch_permit(
    monitor: &Arc<ResourceMonitorState>,
    permit: &mut Option<AgentLaunchPermit>,
) {
    if let Some(permit) = permit.take() {
        monitor.release_unregistered_permit(permit);
    }
}

pub(crate) async fn create_owned_response_replacement<R: tauri::Runtime>(
    app: &AppHandle<R>,
    request: OwnedReplacementCreateRequest,
    continuation: crate::session::selection::OwnedReplacementContinuation,
    target: &crate::api::message_store::PtyInputTargetOwnership<'_>,
) -> Result<OwnedReplacementCreateStep, String> {
    create_owned_response_replacement_in_context(app, request, continuation, target, None).await
}

// An inline caller must supply its actual current selection turn and cause.
// Absence selects the existing external finalization ticket, never an inferred
// restore or lifecycle capability.
pub(crate) async fn create_owned_response_replacement_in_context<R: tauri::Runtime>(
    app: &AppHandle<R>,
    request: OwnedReplacementCreateRequest,
    continuation: crate::session::selection::OwnedReplacementContinuation,
    target: &crate::api::message_store::PtyInputTargetOwnership<'_>,
    inline: Option<(&SelectionTransaction<R>, SelectionCause)>,
) -> Result<OwnedReplacementCreateStep, String> {
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .inner()
        .clone();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let completion = create_session_inner_impl(
        app,
        &manager,
        &pty,
        request.shell,
        request.shell_args,
        request.cwd,
        request.name,
        request.agent_id,
        request.agent_label,
        request.skip_tooling_save,
        request.git_repos,
        false,
        request.resolved_spawn,
        request.host_shell,
        None,
        CreateSelectionIntent::Background,
        None,
        None,
        inline.as_ref().map(|(transaction, _)| *transaction),
        inline.map(|(_, cause)| cause),
        false,
        Some(target),
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
        None,
        None,
        Some(continuation.clone()),
        None,
    )
    .await;
    let completion = match completion {
        Ok(completion) => completion,
        Err(error) => {
            let manager = manager.read().await.clone();
            continuation
                .rollback_failed_create(&manager, &pty, &error)
                .await;
            return Err(error);
        }
    };
    match completion {
        CreateCompletion::Finalized(info) => Ok(OwnedReplacementCreateStep::Finalized(info)),
        CreateCompletion::Retained => Ok(OwnedReplacementCreateStep::Retained),
        CreateCompletion::Deferred(_) => Err("create unexpectedly deferred finalization".into()),
    }
}

// Shared by Tauri command + restore path; collapsing args would force a context struct.
#[allow(clippy::too_many_arguments)]
pub async fn create_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
    git_repos: Vec<SessionRepo>,
    skip_auto_resume: bool,
    resolved_spawn: Option<AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    // #973 - the size the view has already fitted to, when the caller has a view at all.
    // `None` keeps AC's historical 120x30. See `PtyViewport`.
    viewport: Option<PtyViewport>,
    selection_intent: CreateSelectionIntent,
) -> Result<SessionInfo, String> {
    create_session_inner_impl(
        app,
        session_mgr,
        pty_mgr,
        shell,
        shell_args,
        cwd,
        session_name,
        agent_id,
        agent_label,
        skip_tooling_save,
        git_repos,
        skip_auto_resume,
        resolved_spawn,
        resolved_agent_host_shell,
        viewport,
        selection_intent,
        None,
        None,
        None,
        None,
        false,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
        None,
        None,
        None,
        None,
    )
    .await?
    .into_finalized()
    .await
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "Preserve PTY create result layout and unchanged completion consumers"
)]
pub(crate) enum PtyTargetCreateStep {
    Finalized(SessionInfo),
    Retained,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_session_inner_with_pty_target_ownership<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
    git_repos: Vec<SessionRepo>,
    skip_auto_resume: bool,
    resolved_spawn: Option<AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    viewport: Option<PtyViewport>,
    selection_intent: CreateSelectionIntent,
    target_ownership: &crate::api::message_store::PtyInputTargetOwnership<'_>,
) -> Result<PtyTargetCreateStep, String> {
    let completion = create_session_inner_impl(
        app,
        session_mgr,
        pty_mgr,
        shell,
        shell_args,
        cwd,
        session_name,
        agent_id,
        agent_label,
        skip_tooling_save,
        git_repos,
        skip_auto_resume,
        resolved_spawn,
        resolved_agent_host_shell,
        viewport,
        selection_intent,
        None,
        None,
        None,
        None,
        false,
        Some(target_ownership),
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
        None,
        None,
        None,
        None,
    )
    .await?;
    match completion {
        CreateCompletion::Finalized(info) => Ok(PtyTargetCreateStep::Finalized(info)),
        CreateCompletion::Retained => Ok(PtyTargetCreateStep::Retained),
        CreateCompletion::Deferred(_) => Err("create unexpectedly deferred finalization".into()),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_session_inner_for_restore<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
    git_repos: Vec<SessionRepo>,
    skip_auto_resume: bool,
    resolved_spawn: Option<AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    viewport: Option<PtyViewport>,
    pending_start_fresh: Option<bool>,
    pending_communication: Option<SessionCommunication>,
    coding_agent_binding: Option<crate::session::manager::CodingAgentBinding>,
    initial_last_prompt: Option<&crate::config::sessions_persistence::PersistedSession>,
) -> Result<SessionInfo, String> {
    create_session_inner_impl(
        transaction.app(),
        session_mgr,
        pty_mgr,
        shell,
        shell_args,
        cwd,
        session_name,
        agent_id,
        agent_label,
        skip_tooling_save,
        git_repos,
        skip_auto_resume,
        resolved_spawn,
        resolved_agent_host_shell,
        viewport,
        CreateSelectionIntent::Suppress,
        pending_start_fresh,
        pending_communication,
        Some(transaction),
        Some(SelectionCause::Restore),
        false,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
        coding_agent_binding,
        None,
        None,
        initial_last_prompt.map(InitialLastPrompt::from_persisted),
    )
    .await?
    .into_finalized()
    .await
}

/// #552 Coordinator create/reopen: seed the badge clock and clear both close markers.
/// #552 seed the badge clock for a coordinator's first spawn so it shows 0m
/// immediately, and clear any "auto-closed" marker. An auto-closed coordinator
/// is DESTROYED, so its reopen flows through this create path (the "create
/// in-place" branch of handleReplicaClick), NOT restart_session_inner.
/// seed_if_absent never overwrites, so a respawn does NOT reset the badge.
/// (#2413) Returns the create's marker ticket (`None` = no clocks state); the
/// caller ends it with `succeeded()` or `failed()` so a failed create puts the
/// cleared markers back.
pub(crate) fn note_coordinator_create_on_app<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    cwd: &str,
) -> Option<CoordinatorCreateTicket<R>> {
    let clocks = app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()?;
    {
        // agent_fqn_from_path returns String (teams.rs:80), not Option.
        let fqn = crate::config::teams::agent_fqn_from_path(cwd);
        let now = chrono::Utc::now();
        // Held through the emits below: a restore can never emit its timestamp
        // after this create's clear has emitted null.
        let _order = CLOSE_MARKER_EVENT_ORDER
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (seeded, cleared_auto, cleared_manual) = {
            let mut g = clocks.lock().unwrap_or_else(|e| e.into_inner());
            let seeded = g.seed_if_absent(&fqn, now);
            let auto_before = g.auto_closed_at(&fqn);
            let manual_before = g.manually_closed_at(&fqn);
            let cleared_auto = g.clear_auto_closed(&fqn);
            let cleared_manual = g.clear_manually_closed(&fqn);
            g.begin_coordinator_create(
                &fqn,
                ClearedCloseMarkers {
                    auto_closed_at: auto_before.filter(|_| cleared_auto),
                    manually_closed_at: manual_before.filter(|_| cleared_manual),
                },
            );
            (seeded, cleared_auto, cleared_manual)
        };
        if seeded {
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
        if cleared_manual {
            let _ = app.emit(
                "coordinator_manual_close_changed",
                serde_json::json!({ "replicaPath": cwd, "manuallyClosedAt": null }),
            );
        }
        Some(CoordinatorCreateTicket {
            app: app.clone(),
            cwd: cwd.to_string(),
            fqn,
            done: false,
            session_may_exist: false,
        })
    }
}

/// (#2413) Orders close-marker changes made by coordinator creates with their
/// events. The create's clear and a failed create's restore each hold it from
/// the clocks change through their emits, so the UI receives their events in
/// the same order as the clock changes. Taken before the clocks lock, never
/// held across an `.await`.
pub(crate) static CLOSE_MARKER_EVENT_ORDER: Mutex<()> = Mutex::new(());

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_session_inner_impl<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
    git_repos: Vec<SessionRepo>,
    skip_auto_resume: bool,
    resolved_spawn: Option<AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    viewport: Option<PtyViewport>,
    selection_intent: CreateSelectionIntent,
    pending_start_fresh: Option<bool>,
    pending_communication: Option<SessionCommunication>,
    inline_transaction: Option<&SelectionTransaction<R>>,
    inline_cause: Option<SelectionCause>,
    defer_inline_finalization: bool,
    preheld_target: Option<&crate::api::message_store::PtyInputTargetOwnership<'_>>,
    enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
    coding_agent_binding: Option<crate::session::manager::CodingAgentBinding>,
    lifecycle_domain: Option<&LifecycleEntryAuthority>,
    owned_replacement: Option<crate::session::selection::OwnedReplacementContinuation>,
    initial_last_prompt: Option<InitialLastPrompt>,
) -> Result<CreateCompletion, String> {
    if inline_transaction.is_some_and(|transaction| {
        transaction.has_restore_context() && !transaction.has_active_restore_capability()
    }) {
        if let Some(domain) = lifecycle_domain {
            domain
                .interrupted
                .store(true, std::sync::atomic::Ordering::Release);
        }
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    if preheld_target.is_some()
        || inline_transaction.is_some_and(|transaction| {
            transaction.has_accepted_lifecycle_core() || transaction.has_active_restore_capability()
        })
        || lifecycle_domain.is_some()
    {
        // Accepted lifecycle jobs already own the one outer create core.
        // The PTY producer owns its entire operation, target stripe/file and
        // exact target guard. Its borrowed proof stays in that task's scope.
        return create_session_inner_core(
            app,
            session_mgr,
            pty_mgr,
            shell,
            shell_args,
            cwd,
            session_name,
            agent_id,
            agent_label,
            skip_tooling_save,
            git_repos,
            skip_auto_resume,
            resolved_spawn,
            resolved_agent_host_shell,
            viewport,
            selection_intent,
            pending_start_fresh,
            pending_communication,
            inline_transaction,
            inline_cause,
            defer_inline_finalization,
            preheld_target,
            enforcement,
            coding_agent_binding,
            lifecycle_domain,
            owned_replacement,
            initial_last_prompt,
        )
        .await;
    }
    let app = app.clone();
    let session_mgr = session_mgr.clone();
    let pty_mgr = pty_mgr.clone();
    let inline_transaction = inline_transaction.cloned();
    crate::config::sessions_persistence::run_owned_persist_core(async move {
        create_session_inner_core(
            &app,
            &session_mgr,
            &pty_mgr,
            shell,
            shell_args,
            cwd,
            session_name,
            agent_id,
            agent_label,
            skip_tooling_save,
            git_repos,
            skip_auto_resume,
            resolved_spawn,
            resolved_agent_host_shell,
            viewport,
            selection_intent,
            pending_start_fresh,
            pending_communication,
            inline_transaction.as_ref(),
            inline_cause,
            defer_inline_finalization,
            None,
            enforcement,
            coding_agent_binding,
            None,
            owned_replacement,
            initial_last_prompt,
        )
        .await
    })
    .await
}

pub(crate) fn resolve_create_destination<R: tauri::Runtime>(
    cwd: &str,
    lifecycle_domain: Option<&LifecycleEntryAuthority>,
    preheld_target: Option<&crate::api::message_store::PtyInputTargetOwnership<'_>>,
    inline_transaction: Option<&SelectionTransaction<R>>,
) -> Result<
    (
        Option<String>,
        Option<crate::path_identity::VerifiedPathIdentity>,
    ),
    String,
> {
    let destination_proof = (|| {
        let cwd_path = std::path::Path::new(cwd);
        let create_target_key = crate::config::teams::pty_input_create_gate_key_from_cwd(cwd_path)
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        let create_target_replica_identity = if create_target_key.is_some() {
            let anchor = crate::config::teams::strict_wg_replica_anchor_from_cwd(cwd_path)
                .map_err(|_| "targetCreateGateUnavailable".to_string())?
                .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
            Some(
                crate::path_identity::verify_directory(&anchor)
                    .map_err(|_| "targetCreateGateUnavailable".to_string())?,
            )
        } else {
            None
        };
        if let Some(domain) = lifecycle_domain {
            if !domain.validates_destination(cwd, create_target_key.as_deref()) {
                return Err("invalidTargetCreateOwnership".to_string());
            }
        }
        if let Some(preheld) = preheld_target {
            let target_key = create_target_key
                .as_deref()
                .ok_or_else(|| "invalidTargetCreateOwnership".to_string())?;
            if !preheld.proves(target_key) {
                return Err("invalidTargetCreateOwnership".to_string());
            }
        }
        Ok::<_, String>((create_target_key, create_target_replica_identity))
    })();
    match destination_proof {
        Ok(proof) => Ok(proof),
        Err(error) => {
            if let Some(domain) = lifecycle_domain {
                domain
                    .interrupted
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            if let Some(transaction) = inline_transaction {
                transaction.invalidate_restore_ownership();
            }
            Err(error)
        }
    }
}

pub(crate) async fn acquire_create_target_lease<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    target_key: Option<&str>,
    replica: Option<&crate::path_identity::VerifiedPathIdentity>,
    has_preheld_target: bool,
    has_lifecycle_domain: bool,
) -> Result<Option<OwnedCreateTargetLease>, String> {
    Ok(if !has_preheld_target && !has_lifecycle_domain {
        if let Some(target_key) = target_key {
            let gate = app
                .try_state::<crate::api::message_store::PtyInputTargetGateState>()
                .and_then(|state| state.gate.as_ref().ok().map(Arc::clone))
                .or_else(|| {
                    app.try_state::<crate::api::message_store::MessageStoreState>()
                        .and_then(|state| state.target_gate.as_ref().ok().map(Arc::clone))
                })
                .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
            let stripe = gate
                .acquire_target_lock(target_key)
                .await
                .map_err(|_| "targetCreateGateUnavailable".to_string())?;
            let exact = gate.acquire_exact(target_key).await;
            Some(OwnedCreateTargetLease {
                key: target_key.to_owned(),
                gate,
                replica: replica
                    .cloned()
                    .ok_or_else(|| "targetCreateGateUnavailable".to_string())?,
                guards: Some((exact, stripe)),
            })
        } else {
            None
        }
    } else {
        None
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "P05 retains distinct result, marker, admission and original cleanup ownership \
              without a context bundle."
)]
pub(crate) async fn finish_create_core_result<R: tauri::Runtime>(
    marker: Option<CoordinatorCreateTicket<R>>,
    mut result: Result<CreateCompletion, String>,
    owned_replacement: Option<&crate::session::selection::OwnedReplacementContinuation>,
    create_owner: &Arc<Mutex<OwnedCreateAdmission>>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    has_preheld_target: bool,
    inline_pending_binding: &Mutex<Option<crate::session::manager::PendingCreateBinding>>,
    inline_transaction: Option<&SelectionTransaction<R>>,
) -> Result<CreateCompletion, String> {
    if let Some(t) = marker {
        if result.is_err() {
            t.failed();
        } else if matches!(
            &result,
            Ok(CreateCompletion::Finalized(_)) | Ok(CreateCompletion::Deferred(_))
        ) {
            t.succeeded();
        }
    }
    if let (Err(error), Some(continuation)) = (&result, owned_replacement) {
        let waiting = continuation.source_stop().is_none()
            && create_owner
                .lock()
                .ok()
                .is_some_and(|owner| owner.blocked_bound);
        if !waiting {
            let manager = session_mgr.read().await.clone();
            continuation
                .rollback_failed_create(&manager, pty_mgr, error)
                .await;
        }
    }
    if let Some(continuation) = owned_replacement {
        if continuation.source_stop().is_some() {
            // Validation failure is not a setup Err or whole-source stopped
            // proof. Retain the real pending owner and its work.
            result = Ok(CreateCompletion::Retained);
        } else if create_owner
            .lock()
            .ok()
            .is_some_and(|owner| owner.blocked_bound)
        {
            result = Ok(CreateCompletion::Retained);
        }
    }
    if has_preheld_target
        && create_owner
            .lock()
            .ok()
            .is_some_and(|owner| owner.blocked_bound)
    {
        result = Ok(CreateCompletion::Retained);
    }
    if result.is_err() && owned_replacement.is_none() {
        let binding = {
            inline_pending_binding
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
        };
        if let (Some(transaction), Some(binding)) = (inline_transaction, binding) {
            transaction.rollback_inline_create(binding).await;
        }
    }
    result
}
