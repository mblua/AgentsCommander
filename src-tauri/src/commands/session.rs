use crate::config::agent_command::AgentSpawnCommand;
use crate::config::agent_config;
use crate::config::coordinator_clocks::CoordinatorClocksState;
use crate::config::sessions_persistence::persist_current_state;
#[cfg(test)]
use crate::config::settings::AppSettings;
use crate::config::settings::SettingsState;
use crate::pty::backend::{
    BackendSpawnSpec, LaunchWitness, PtyViewport, ResolvedAgentHostShell, SessionBackendKind,
};
use crate::pty::container_paths::{ContainerEnvWarning, CLAUDE_CONFIG_DIR_KEY};
use crate::pty::manager::PtyManager;
use crate::pty::output::PtyOutputTarget;
use crate::resource_monitor::{
    ResourceLaunchMetadata, ResourceLaunchRegistration, ResourceLimits, ResourceLogicalAgentSlot,
    ResourceMonitorState,
};
use crate::session::manager::SessionManager;
use crate::session::profile::{idle_tuning_for, CodingAgentKind};
#[cfg(test)]
use crate::session::selection::TrustedRestartIntent;
use crate::session::selection::{
    SelectionCause, SelectionCoordinator, SelectionRequest, SelectionTransaction,
};
use crate::session::session::{SessionCommunication, SessionInfo, SessionRepo, SessionStatus};
use crate::session::warnings::{emit_session_warning, SessionWarning};
use crate::telegram::manager::TelegramBridgeState;
use crate::DetachedSessionsState;
pub(crate) use resume_args::trusted_muse_auto_resume_spawn;
use resume_args::{
    antigravity_tokens_have_resume, codex_has_explicit_subcommand, codex_tokens_have_resume,
    expand_env_var_refs, inject_antigravity_resume, inject_codex_resume, maybe_inject_muse_resume,
    maybe_inject_pi_resume, pi_has_explicit_session_control,
};
#[cfg(test)]
use resume_args::{inject_pi_resume, pi_is_non_conversation_invocation};
use resume_probe::{
    build_title_prompt_appendage, claude_resume_probe_target_for_kind, configured_prompt_identity,
    container_path_context_for_cwd, effective_codex_home_for_backend,
    injected_claude_config_dir_for_copy, should_inject_continue, should_inject_fresh_session_id,
};
#[cfg(test)]
use resume_probe::{claude_projects_dir_for_config_dir, resume_probe_target_for_config_dir};
pub(crate) use resume_probe::{resolve_claude_projects_dir, transcript_id_from_args};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use uuid::Uuid;

mod agent_command;
mod co_managed;
mod create;
mod destroy;
mod lifecycle;
mod restart;
mod restart_resume;
mod resume_args;
mod resume_probe;
mod root_agent;
mod telegram;
#[cfg(test)]
use agent_command::coding_agent_descriptor;
pub(crate) use agent_command::executable_basename;
use agent_command::prepare_last_coding_agent_write;
use agent_command::resolve_actual_agent;
#[cfg(test)]
use agent_command::resolve_agent_command;
#[cfg(test)]
use agent_command::resolve_agent_from_shell;
pub(crate) use agent_command::resolve_root_agent_command;
use agent_command::save_last_coding_agent;
pub(crate) use co_managed::co_managed_capturable_for_session;
pub(crate) use co_managed::co_managed_effective_state_for_session;
pub(crate) use co_managed::raise_room_reader_demand;
pub(crate) use co_managed::raise_room_reader_demand_in;
pub(crate) use co_managed::release_room_reader_demand;
#[cfg(test)]
use create::acquire_create_publication_region;
use create::acquire_create_target_lease;
use create::acquire_lifecycle_create_publication_region;
pub(crate) use create::build_configured_agent_spawn_for_cwd;
use create::compute_profile_outdated;
pub(crate) use create::create_owned_response_replacement;
#[cfg(test)]
use create::create_owned_response_replacement_in_context;
pub use create::create_session_inner;
pub(crate) use create::create_session_inner_for_restore;
use create::create_session_inner_impl;
pub(crate) use create::create_session_inner_with_pty_target_ownership;
use create::enforce_owned_unarchived_for_spawn;
use create::finish_create_core_result;
pub(crate) use create::note_coordinator_create_on_app;
pub(crate) use create::purge_session_side_state;
pub(crate) use create::register_session_samplers;
use create::release_resource_launch_permit;
use create::reset_substantive_input;
pub(crate) use create::resolve_configured_agent_spawn_for_cwd;
use create::resolve_create_destination;
pub(crate) use create::resolve_create_session_cwd;
use create::resolve_launch_auto_self_clear;
use create::resolve_launch_idle_tuning;
use create::revalidate_owned_create_effect;
use create::rollback_pre_created_session;
#[cfg(test)]
use create::try_acquire_create_publication_region;
use create::with_create_resource_region;
pub(crate) use create::CoordinatorCreateTicket;
use create::CreateCapacityContext;
use create::CreateCompletion;
use create::CreateResourceAdmission;
pub use create::CreateSelectionIntent;
use create::DeferredCreateOutput;
use create::InitialLastPrompt;
use create::OwnedCreateAdmission;
pub(crate) use create::PtyTargetCreateStep;
use create::CLOSE_MARKER_EVENT_ORDER;
pub(crate) use destroy::background_destroy_session_inner;
use destroy::count_working_members;
pub use destroy::destroy_session_inner;
#[cfg(test)]
pub(crate) use destroy::destroy_sessions_with_source;
#[cfg(test)]
use destroy::detach_response_close_telegram;
pub(crate) use destroy::execute_destroy_transaction;
use destroy::execute_destroy_transaction_common;
use destroy::execute_manual_coordinator_destroy;
pub(crate) use destroy::execute_response_close_transaction;
pub(crate) use destroy::reconcile_response_closes;
pub use destroy::CoordinatorCloseOutcome;
pub(crate) use destroy::DestroyOutcome;
pub(crate) use destroy::DestroyRequest;
pub use destroy::DestructionSource;
pub(crate) use lifecycle::prepare_lifecycle_entry_authority;
use lifecycle::prepare_restore_lifecycle_authority;
use lifecycle::retire_lifecycle_entry_protection;
pub(crate) use lifecycle::validate_lifecycle_entry_authority;
pub(crate) use lifecycle::LifecycleAuthorityAdmission;
pub(crate) use lifecycle::LifecycleEntryAuthority;
use restart::accept_restart_deferred_completion;
use restart::accept_restart_teardown;
pub(crate) use restart::carry_communication_for_restart;
pub(crate) use restart::complete_owned_replacement_retirement;
use restart::effective_create_skip_auto_resume;
use restart::effective_restart_requested_profile;
#[cfg(test)]
use restart::effective_restart_skip_auto_resume;
pub(crate) use restart::execute_external_restart_transaction;
use restart::finalize_restart_deferred;
pub(crate) use restart::matched_selection;
use restart::mirror_forces_fresh;
use restart::publish_restart_after_warnings;
use restart::publish_restart_before_warnings;
pub(crate) use restart::resolve_restart_selected_agent_id;
pub use restart::restart_session_inner;
pub use restart::restart_session_inner_with_activation;
pub(crate) use restart::restart_session_inner_with_intent;
use restart::restart_skip_auto_resume_with_intent;
use restart::teardown_old_for_restart;
pub(crate) use restart::LifecycleEntryOutcome;
use restart::LifecycleExecutionContext;
pub(crate) use restart::MatchedSelection;
pub(crate) use restart::RestartJobRequest;
#[cfg(test)]
pub(crate) use restart_resume::partition_restart_resume_ready;
pub(crate) use restart_resume::record_agent_turn_completed;
pub(crate) use restart_resume::restart_resume_prompt_for;
#[cfg(test)]
pub(crate) use restart_resume::restart_resume_session_is_ready;
pub(crate) use restart_resume::run_restart_resume;
pub(crate) use restart_resume::RestartResumeTarget;
#[cfg(test)]
use root_agent::classify_existing_root;
pub(crate) use root_agent::create_root_agent_inner;
use root_agent::ensure_lifecycle_root_directory;
pub(crate) use root_agent::ensure_owned_root_directory;
pub(crate) use root_agent::execute_external_root_transaction;
pub(crate) use root_agent::execute_root_transaction;
#[cfg(test)]
use root_agent::ExistingRootAction;
pub(crate) use root_agent::RootJobRequest;
pub(crate) use telegram::attach_local_config_telegram_if_any;
pub(crate) use telegram::attach_persisted_telegram_if_configured;
pub(crate) use telegram::preserve_deferred_telegram_intent_if_valid;

#[cfg(test)]
pub(crate) mod room_raise_seam;

/// #1063: test-only finite barriers that pause a real session create at the two
/// points a deletion race needs: after the pending row is inserted and before the
/// config-seed project gate (`before_project_gate`), and after the seed transaction
/// and before PTY spawn/finalization (`after_seed_before_pty`). Barriers are keyed
/// by the create's normalized working directory, so concurrently running create
/// tests never take each other's barrier, and they exist only in `#[cfg(test)]`
/// builds, so production create is unchanged and stays non-emitting.
#[cfg(test)]
pub(crate) mod seed_race_barriers;

/// (#2413) Test-only coordinator override for `create_session_inner_impl`.
/// `is_coordinator` comes from `discover_teams()`, which reads the real
/// settings, so a lib test cannot make a temp cwd a coordinator otherwise.
/// Keyed by the normalized cwd, like `seed_race_barriers`.
#[cfg(test)]
pub(crate) mod coordinator_create_seam;

/// Test-only rendezvous inside `execute_restart_transaction`, after the old
/// id's reader demands are released and before the replacement is created
/// (#2232 phase 4 test 9).
///
/// Test 9 needs to prove the post-commit sweep: an old-id raise installed
/// *after* the early release but *before* the commit must be gone once the
/// commit has removed the old row. This barrier makes that window
/// deterministic instead of timing-dependent.
#[cfg(test)]
pub(crate) mod restart_reader_seam;

/// Core session creation logic shared by the Tauri command and the restore path.
/// Creates a session record, spawns a PTY, and emits the session_created event.
/// Auto-detects agent from shell command if not provided, and auto-injects provider-specific
/// resume flags (Claude/Pi/Antigravity `--continue`, Codex `resume --last`, and for a
/// trusted empty-argv local macOS/Linux Muse recipe `resume --last`, #1873)
/// when appropriate.
/// If `skip_tooling_save` is true, skips writing to the repo's config.json (for temp sessions).
///
/// `skip_auto_resume` controls provider auto-resume injection:
/// - `true` — suppress all provider auto-resume. Use this for any "fresh
///   create" call site (UI/CLI/root-agent create, mailbox wake-from-cold
///   meaning no SessionManager record at this CWD, `restart_session` with
///   default semantics from `effective_restart_skip_auto_resume`).
/// - `false` — allow provider auto-resume. Use this only for paths restoring
///   a session AC already knows about (the startup-restore loop in `lib.rs`,
///   the wake-from-known-state branch in `mailbox::deliver_wake` — any
///   `RespawnExited` match, today driven exclusively by deferred-non-coord
///   `Exited(0)` records — and `restart_session` when its caller passes
///   `Some(false)`).
///
/// Owned launch inputs retained before a response replacement starts effects.
/// The executor alone supplies app state and a real lexical target proof.
#[derive(Debug, Clone)]
pub(crate) struct OwnedReplacementCreateRequest {
    pub(crate) shell: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) cwd: String,
    pub(crate) name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) skip_tooling_save: bool,
    pub(crate) git_repos: Vec<SessionRepo>,
    pub(crate) resolved_spawn: Option<AgentSpawnCommand>,
    pub(crate) host_shell: Option<ResolvedAgentHostShell>,
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "Preserve owned finalization result layout; boxing adds an unnecessary allocation"
)]
pub(crate) enum OwnedReplacementCreateStep {
    Finalized(SessionInfo),
    Retained,
}

/// Owns the original locks only across the admitted test create cut. These
/// values never enter the retained operation record.
#[cfg(test)]
pub(crate) struct FixturePtyCutLease {
    pub(crate) gate: Arc<crate::api::message_store::PtyInputTargetGate>,
    pub(crate) target: String,
    pub(crate) exact: crate::api::message_store::PtyInputTargetGuard,
    pub(crate) stripe: crate::api::message_store::PtyInputStripeGuard,
}

#[cfg(test)]
pub(crate) enum FixtureReplacementLease {
    OrdinaryMoved(FixturePtyCutLease),
    PtyCutShared(Arc<FixturePtyCutLease>),
}

#[cfg(test)]
pub(crate) struct FixtureReplacementInput {
    pub(crate) request: OwnedReplacementCreateRequest,
    pub(crate) continuation: crate::session::selection::OwnedReplacementContinuation,
    pub(crate) lease: FixtureReplacementLease,
}

#[cfg(test)]
pub(crate) struct FixtureReplacementResult {
    pub(crate) outcome: Result<OwnedReplacementCreateStep, String>,
    pub(crate) ordinary_lease: Option<FixturePtyCutLease>,
}

#[cfg(test)]
impl std::fmt::Debug for FixtureReplacementResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixtureReplacementResult")
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl std::fmt::Debug for FixtureReplacementInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixtureReplacementInput")
            .field("continuation", &self.continuation)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) async fn execute_inline_replacement_fixture<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    input: FixtureReplacementInput,
) -> FixtureReplacementResult {
    let FixtureReplacementInput {
        request,
        continuation,
        lease,
    } = input;
    let observed = continuation.clone();
    if !observed
        .fixture_reached(crate::session::selection::ReplacementFixtureCut::BeforeSourceValidation)
        .await
    {
        return FixtureReplacementResult {
            outcome: Ok(OwnedReplacementCreateStep::Retained),
            ordinary_lease: match lease {
                FixtureReplacementLease::OrdinaryMoved(lease) => Some(lease),
                FixtureReplacementLease::PtyCutShared(lease) => {
                    drop(lease);
                    None
                }
            },
        };
    }
    let borrowed = match &lease {
        FixtureReplacementLease::OrdinaryMoved(lease) => lease,
        FixtureReplacementLease::PtyCutShared(lease) => lease.as_ref(),
    };
    let mut outcome =
        match borrowed
            .gate
            .target_ownership(&borrowed.target, &borrowed.stripe, &borrowed.exact)
        {
            Ok(proof) => {
                create_owned_response_replacement_in_context(
                    transaction.app(),
                    request,
                    continuation,
                    &proof,
                    Some((
                        transaction,
                        SelectionCause::SessionCreated(
                            crate::session::selection::TrustedCreateIntent::Background,
                        ),
                    )),
                )
                .await
            }
            Err(_) => Ok(OwnedReplacementCreateStep::Retained),
        };
    if !observed
        .fixture_reached(
            crate::session::selection::ReplacementFixtureCut::AdapterOutcomeBeforeLeaseDrop,
        )
        .await
        && outcome.is_ok()
    {
        outcome = Ok(OwnedReplacementCreateStep::Retained);
    }
    let ordinary_lease = match lease {
        FixtureReplacementLease::OrdinaryMoved(lease) => Some(lease),
        FixtureReplacementLease::PtyCutShared(lease) => {
            drop(lease);
            None
        }
    };
    FixtureReplacementResult {
        outcome,
        ordinary_lease,
    }
}

/// (#2413) Test-only pause in a failed create's restore, after the clocks lock
/// is dropped and before its events are emitted. Keyed by FQN.
#[cfg(test)]
pub(crate) mod restore_emit_seam;

#[allow(clippy::too_many_arguments)]
async fn create_session_inner_core<R: tauri::Runtime>(
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
    mut skip_auto_resume: bool,
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
    {
        let manager = session_mgr.read().await;
        crate::config::sessions_persistence::initialize_prompt_retention(&manager).await;
    }
    // #1327 - startup coding-agent updates gate: no session may open (GUI,
    // restore, restart, root agent, web, phone) before the startup update run
    // finishes or times out. Absent in tests (mock app, nothing managed) and in
    // non-GUI contexts -> no-op.
    if let Some(gate) = app.try_state::<Arc<crate::agent_update::AgentUpdateGate>>() {
        gate.wait_until_done().await;
    }
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
    if !inline_transaction.is_some_and(|transaction| transaction.has_active_restore_capability()) {
        if let Some(coordinator) = app.try_state::<SelectionCoordinator>() {
            coordinator.wait_for_restore_release().await?;
        }
    }
    let cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);
    let (create_target_key, create_target_replica_identity) =
        resolve_create_destination(&cwd, lifecycle_domain, preheld_target, inline_transaction)?;
    let _owned_target_gate = acquire_create_target_lease(
        app,
        create_target_key.as_deref(),
        create_target_replica_identity.as_ref(),
        preheld_target.is_some(),
        lifecycle_domain.is_some(),
    )
    .await?;
    let create_manager = session_mgr.read().await.clone();
    let (create_nonce, create_binding) = match owned_replacement.as_ref() {
        Some(continuation) => match continuation.binding() {
            Some(binding) => (continuation.operation(), binding),
            None => return Ok(CreateCompletion::Retained),
        },
        None => (
            Uuid::new_v4(),
            create_manager.reserve_carrier_create_binding(),
        ),
    };
    let create_owner = Arc::new(Mutex::new(OwnedCreateAdmission {
        preheld_target: preheld_target.is_some(),
        owned_replacement: owned_replacement.clone(),
        replacement_pty: owned_replacement.as_ref().map(|_| pty_mgr.clone()),
        tracker: pty_mgr
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .response_close_state(),
        target: create_target_key
            .clone()
            .unwrap_or_else(|| format!("create-cwd:{}", cwd)),
        cwd: PathBuf::from(&cwd),
        nonce: create_nonce,
        binding: create_binding,
        reservation: None,
        failed_foreign: false,
        owned_target: _owned_target_gate,
        capacity: None,
        resource: None,
        blocked_bound: false,
        capacity_error: None,
    }));
    let session_label = session_name.as_deref().unwrap_or(&shell).to_string();
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    let create_ticket = Arc::new(Mutex::new(match inline_transaction {
        Some(_) => None,
        None => Some(match selection_intent.trusted() {
            Some(intent) => coordinator
                .reserve_create(intent)
                .await
                .map_err(|error| error.to_string())?,
            None => coordinator
                .reserve_suppressed_create()
                .await
                .map_err(|error| error.to_string())?,
        }),
    }));
    if let Some(continuation) = owned_replacement.as_ref() {
        if let Some(ticket) = create_ticket
            .lock()
            .map_err(|_| "create ticket unavailable".to_string())?
            .as_mut()
        {
            ticket.retain_owned_replacement(continuation.clone())?;
        }
    }
    if inline_transaction.is_none() {
        create_owner
            .lock()
            .map_err(|_| "targetCreateGateUnavailable".to_string())?
            .capacity = Some(CreateCapacityContext {
            ticket: create_ticket.clone(),
            coordinator: coordinator.inner().clone(),
        });
    }
    let inline_pending_binding = Arc::new(Mutex::new(None));
    let inline_pending_for_body = Arc::clone(&inline_pending_binding);
    // (#2413) Close-marker ticket of this create; the body fills it, the outer
    // scope ends it after the select.
    let marker_ticket = Mutex::new(None::<CoordinatorCreateTicket<R>>);
    let create_body = async {
        // §1295 5.1a creation gate: FIRST statement of create_body, BEFORE the
        // existing `enforce_unarchived_for_spawn` (which the archive-gate source
        // test :7438 counts separately). Reading the SAME normalized `cwd` string
        // the archive gate receives; no resource permit and no pending row exist
        // yet, so a rejection leaves zero RAM/disk residue and needs no
        // `rollback_pre_created_session`. (dev-rust E: the two call strings
        // at :1443/:1708 are untouched.)
        crate::config::sessions_persistence::enforce_creation_gate(app, &cwd, enforcement).await?;
        enforce_owned_unarchived_for_spawn(
            app,
            &cwd,
            &session_label,
            &create_owner,
            &create_manager,
            lifecycle_domain,
            inline_transaction,
        )
        .await?;
        // An unarchived path has no native activation/admission callback.
        // It still reserves healthy ownership before the first create effect.
        let initial_admission = acquire_lifecycle_create_publication_region(
            &create_owner,
            &create_manager,
            lifecycle_domain,
            inline_transaction,
        )
        .await?;
        drop(initial_admission);
        let (agent_id, agent_label) = {
            if let Some(spawn) = resolved_spawn.as_ref() {
                (
                    Some(spawn.trusted_agent_id.clone()),
                    Some(spawn.trusted_agent_label.clone()),
                )
            } else {
                let settings_state = app.state::<SettingsState>();
                let cfg = settings_state
                    .current_read()
                    .await
                    .map_err(|error| error.to_string())?;
                resolve_actual_agent(
                    &shell,
                    &shell_args,
                    agent_id.as_deref(),
                    agent_label.as_deref(),
                    &cfg,
                )
            }
        };

        // Recompute is_coordinator from the current team snapshot. One source of truth:
        // every caller of create_session_inner gets the same computation.
        let teams = tokio::task::spawn_blocking(crate::config::teams::discover_teams)
            .await
            .map_err(|e| e.to_string())?;
        let is_coordinator = crate::config::teams::is_coordinator_for_cwd(&cwd, &teams);
        #[cfg(test)]
        let is_coordinator = is_coordinator || coordinator_create_seam::forced(&cwd);
        let is_root_agent = crate::config::root_agent::is_root_agent_path(&cwd);
        #[cfg(test)]
        let is_root_agent = is_root_agent || coordinator_create_seam::forced_root(&cwd);

        if is_coordinator {
            *marker_ticket.lock().unwrap_or_else(|e| e.into_inner()) =
                note_coordinator_create_on_app(app, &cwd);
        }

        // (#756) Durable fresh-intent mirror, consumed at the create path: the
        // caller requested resume (#599 reopen passes skipAutoResume=false), but
        // this cwd's coordinator carries a pending fresh boundary (restart or
        // successful logical clear (/clear or Pi /new) whose record died with
        // an auto/manual close). Force a fresh spawn BEFORE any provider resume
        // injection below (Claude/Pi/Antigravity --continue, Codex resume --last);
        // provider-agnostic by construction. The mirror is deliberately NOT cleared
        // here: only post-boundary content (typed input or AC-injected content)
        // drops it, so repeated close/reopen cycles with no content stay fresh.
        let mut fresh_forced_by_mirror = false;
        if is_coordinator && !skip_auto_resume {
            if let Some(clocks) =
                app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
            {
                let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
                let pending = {
                    let guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
                    guard.start_fresh_at(&fqn)
                };
                if mirror_forces_fresh(skip_auto_resume, is_coordinator, pending.is_some()) {
                    log::info!(
                        "[session] fresh-intent mirror pending for '{}' (boundary {}): forcing \
                     skip_auto_resume (#756)",
                        fqn,
                        pending.map(|d| d.to_rfc3339()).unwrap_or_default()
                    );
                    skip_auto_resume = true;
                    fresh_forced_by_mirror = true;
                }
            }
        }

        let agent_kind = CodingAgentKind::detect(&shell, &shell_args);
        let is_agent_owned_launch = agent_id.is_some() || agent_kind.is_some() || is_root_agent;
        let coding_agent_binding = match coding_agent_binding.clone() {
            Some(binding) => Some(binding),
            None => match agent_id.as_ref() {
                Some(original_agent_id) => {
                    let settings_state = app.state::<SettingsState>();
                    let cfg = settings_state
                        .current_read()
                        .await
                        .map_err(|error| error.to_string())?;
                    let id = Uuid::new_v4().to_string();
                    let seed = match crate::config::settings::bootstrap_binding_revision(
                        &cfg,
                        original_agent_id,
                    ) {
                        Ok(identity_revision) => Some(serde_json::json!({
                            "identityRevision": identity_revision,
                            "originalAgentId": original_agent_id,
                        })),
                        Err(error) => {
                            log::warn!(
                                "[session] binding seed unavailable for '{}': {}",
                                original_agent_id,
                                error
                            );
                            None
                        }
                    };
                    Some(crate::session::manager::CodingAgentBinding {
                        id,
                        seed,
                        origin: None,
                    })
                }
                None => None,
            },
        };
        let resource_monitor = app.state::<Arc<ResourceMonitorState>>().inner().clone();
        let mut resource_permit = if is_agent_owned_launch {
            let cfg = app
                .state::<SettingsState>()
                .current_snapshot()
                .await
                .map_err(|error| error.to_string())?;
            resource_monitor.try_reserve_agent_slot(ResourceLimits::from(&cfg))?
        } else {
            None
        };

        create_owner
            .lock()
            .map_err(|_| "targetCreateGateUnavailable".to_string())?
            .resource = Some(Arc::new(Mutex::new(CreateResourceAdmission {
            monitor: resource_monitor.clone(),
            settings: app.state::<SettingsState>().inner().clone(),
            permit: None,
            needed: is_agent_owned_launch,
            in_region: false,
            released: false,
        })));

        // Clone the manager handle so the outer RwLock guard drops before spawn awaits.
        let mgr = session_mgr.read().await.clone();
        let backend_kind = resolved_spawn
            .as_ref()
            .map(|spawn| SessionBackendKind::from(&spawn.backend))
            .unwrap_or_default();
        if is_root_agent && backend_kind == SessionBackendKind::ContainerTransport {
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Err("root-agent cannot use container transport".to_string());
        }
        let container_path_context = if backend_kind == SessionBackendKind::ContainerTransport {
            match container_path_context_for_cwd(&cwd) {
                Ok(context) => Some(context),
                Err(err) => {
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    return Err(err);
                }
            }
        } else {
            None
        };
        // #935 - resolve the container's read-write repo bind mounts from its own
        // per-agent config.json repos[] (plan Sec 2/4), on the CANONICAL host root.
        // This runs BEFORE create_session below, so an inadmissible entry (an escape
        // signature: a repos[] rewrite reaching a sibling replica or messaging/) hard-
        // fails the spawn here with no session record, token, or container created
        // (plan Sec 4.3/4.4). None for local-process sessions.
        let container_repos = match container_path_context.as_ref() {
            Some(context) => {
                match crate::pty::container_repos::resolve_repo_mounts(std::path::Path::new(
                    &context.host_root,
                )) {
                    Ok(resolution) => Some(resolution),
                    Err(err) => {
                        release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                        return Err(err);
                    }
                }
            }
            None => None,
        };
        // Attach the B capability already retained before create effects to
        // this ticket/inline owner. No binder, default insert, or queue
        // admission may mint another binding in the publication region.
        let prepared_binding = create_owner.lock().ok().map(|owner| owner.binding);
        let Some(prepared_binding) = prepared_binding else {
            return Ok(CreateCompletion::Retained);
        };
        let carrier_u0 = match with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            acquire_lifecycle_create_publication_region(
                &create_owner,
                &mgr,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            Ok(permit) => permit,
            Err(error) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(error);
            }
        };
        let mut inline_reserved = None;
        if let Some(ticket) = create_ticket
            .lock()
            .map_err(|_| "create ticket unavailable".to_string())?
            .as_mut()
        {
            #[cfg(test)]
            if owned_replacement
                .as_ref()
                .is_some_and(|owned| owned.fixture_bind_twice())
            {
                assert_eq!(ticket.bind_prepared(prepared_binding)?, prepared_binding);
            }
            ticket.bind_prepared(prepared_binding)?;
        } else if let Some(transaction) = inline_transaction {
            #[cfg(test)]
            if owned_replacement
                .as_ref()
                .is_some_and(|owned| owned.fixture_bind_twice())
            {
                assert_eq!(
                    transaction
                        .prepare_inline_create_binding(prepared_binding, &mut inline_reserved)?,
                    prepared_binding
                );
            }
            transaction.prepare_inline_create_binding(prepared_binding, &mut inline_reserved)?;
        } else {
            return Err("inline create transaction is unavailable".to_string());
        }
        if let Some(transaction) = inline_transaction {
            if !transaction.retain_entry_create_binding(prepared_binding) {
                drop(carrier_u0);
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
        }
        // #1101: the restart replacement create runs while `teardown_old_for_restart`
        // has released the old PTY but RETAINED its canonical manager row (removed only
        // at the atomic commit's `mutations.remove(uuid)`, which runs after this gate).
        // For a live-session restart that row is non-Exited, so `appeared_session` is a
        // false positive and would refuse every restart of a live WG replica session.
        // Restart passes `preheld_target = None` and acquires its own exact target lock
        // (`_owned_target_gate`), so it is already serialized and is the authoritative
        // replacement; this cold-spawn dedup heuristic is redundant and wrong for it.
        // Skip the gate only for the restart cause. The wake path (inline_cause = None)
        // is unchanged and keeps its dedup.
        if create_target_replica_identity.is_some()
            && matches!(
                selection_intent,
                CreateSelectionIntent::Background | CreateSelectionIntent::Suppress
            )
            && !matches!(inline_cause, Some(SelectionCause::Restart(_)))
        {
            let target = create_target_replica_identity
                .as_ref()
                .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
            let appeared_session = mgr.list_sessions().await.into_iter().any(|session| {
                !matches!(session.status, SessionStatus::Exited(_))
                    && crate::path_identity::verify_directory(std::path::Path::new(
                        &session.working_directory,
                    ))
                    .is_ok_and(|cwd_identity| {
                        crate::path_identity::is_verified_descendant(&cwd_identity, target)
                    })
            });
            let appeared_spawn = pty_mgr
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .has_pending_spawn_for_replica(target);
            if appeared_session || appeared_spawn {
                drop(carrier_u0);
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err("sessionRace".to_string());
            }
        }
        let spawn_mark = {
            let pty = pty_mgr.lock().unwrap_or_else(|error| error.into_inner());
            pty.mark_spawning(&cwd, &session_label)
        };
        let pending_result = mgr
            .create_pending_session_with_binding_and_carrier_permit(
                prepared_binding,
                &carrier_u0,
                shell.clone(),
                shell_args.clone(),
                cwd.clone(),
                agent_id.clone(),
                agent_label.clone(),
                git_repos,
                is_coordinator,
                backend_kind,
            )
            .await
            .map(|session| (session, prepared_binding))
            .map_err(|error| error.to_string());
        let (mut session, pending_binding) = match pending_result {
            Ok(created) => created,
            Err(e) => {
                drop(carrier_u0);
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(e);
            }
        };
        if owned_replacement
            .as_ref()
            .is_some_and(|continuation| !continuation.mark_pending(pending_binding))
        {
            drop(carrier_u0);
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Ok(CreateCompletion::Retained);
        }
        if inline_transaction
            .is_some_and(|transaction| !transaction.observe_entry_pending_binding(pending_binding))
        {
            drop(carrier_u0);
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Ok(CreateCompletion::Retained);
        }
        let previous = match owned_replacement.as_ref() {
            Some(continuation) => match continuation.previous_for_launch(&carrier_u0) {
                Some(previous) => Some(previous),
                None => {
                    drop(carrier_u0);
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    return Ok(CreateCompletion::Retained);
                }
            },
            None => None,
        };
        if let Some(previous) = previous {
            // Carry is local launch input only. FinalizedOwned performs the
            // exact persisted carry after B's real finalization/version receipt.
            // The producer requests resume; A's unconsumed durable fresh intent
            // still determines the actual runtime argv before launch.
            skip_auto_resume = previous.start_fresh_on_restore;
            session.telegram_bot_id = previous.telegram_bot_id;
            session.communication = previous.communication;
            session.start_fresh_on_restore = previous.start_fresh_on_restore;
        }
        if let Err(error) = mgr
            .install_pending_coding_agent_binding_with_carrier_permit(
                &carrier_u0,
                pending_binding,
                coding_agent_binding,
            )
            .await
        {
            drop(carrier_u0);
            if owned_replacement.is_none() {
                if let Some(transaction) = inline_transaction {
                    transaction.rollback_inline_create(pending_binding).await;
                }
            }
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Err(error);
        }
        if inline_transaction.is_some() {
            *inline_pending_for_body
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(pending_binding);
        }
        if let Some(value) = pending_start_fresh {
            mgr.set_pending_start_fresh_on_restore_with_carrier_permit(
                &carrier_u0,
                pending_binding,
                value,
            )
            .await
            .map_err(|error| error.to_string())?;
            session.start_fresh_on_restore = value;
        }
        if let Some(communication) = pending_communication.clone() {
            mgr.set_pending_communication_with_carrier_permit(
                &carrier_u0,
                pending_binding,
                communication.clone(),
            )
            .await
            .map_err(|error| error.to_string())?;
            session.communication = Some(communication);
        }

        drop(carrier_u0);

        // #1063 test-only barrier: the pending row now exists and is visible to a
        // deletion's pending-inclusive snapshot; pause here before any config-seed
        // project gate so a delete can interleave deterministically. No-op in production.
        #[cfg(test)]
        {
            seed_race_barriers::hit_before_project_gate(&cwd).await;
        }

        if let Err(e) = with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            enforce_owned_unarchived_for_spawn(
                app,
                &cwd,
                &session_label,
                &create_owner,
                &create_manager,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            let err = e.to_string();
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            drop(mgr);
            rollback_pre_created_session(app, session_mgr, pty_mgr, session.id, &err).await;
            return Err(err);
        }

        let carrier_u1 = match with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            acquire_lifecycle_create_publication_region(
                &create_owner,
                &mgr,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            Ok(permit) => permit,
            Err(error) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(error);
            }
        };
        // (#756) Propagate the mirror-forced intent onto the NEW record: the
        // startup-restore path reads ONLY the record, so without this an app close
        // after the forced-fresh reopen would resume the pre-boundary conversation.
        // No persist call here: the command, restart and both restore callers
        // persist after inner returns; the wake/loop/web creates do not, which is
        // fine because the mirror is already persisted and re-consumed by the
        // guard above on every later create, and the record stamp reaches disk
        // with the next persist-any.
        if fresh_forced_by_mirror {
            mgr.set_pending_start_fresh_on_restore_with_carrier_permit(
                &carrier_u1,
                pending_binding,
                true,
            )
            .await
            .map_err(|error| error.to_string())?;
        }

        if is_root_agent {
            mgr.set_pending_is_root_agent_with_carrier_permit(&carrier_u1, pending_binding, true)
                .await
                .map_err(|error| error.to_string())?;
            session.is_root_agent = true;
        }

        if let Some(name) = session_name {
            if let Err(e) = mgr
                .rename_pending_session_with_carrier_permit(
                    &carrier_u1,
                    pending_binding,
                    name.clone(),
                )
                .await
            {
                let err = e.to_string();
                drop(carrier_u1);
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                drop(mgr);
                rollback_pre_created_session(app, session_mgr, pty_mgr, session.id, &err).await;
                return Err(err);
            }
            session.name = name;
        }

        let id = session.id;
        if let Some(spawn) = resolved_spawn.as_ref() {
            let effective_codex_home = effective_codex_home_for_backend(
                session.backend_kind,
                container_path_context.as_ref().map(|context| &context.map),
                spawn,
            );
            mgr.set_pending_profile_metadata_with_carrier_permit(
                &carrier_u1,
                pending_binding,
                Some(spawn.profile_resolution.requested_profile.clone()),
                Some(spawn.profile_resolution.effective_profile.clone()),
                spawn.profile_resolution.fallback_chain.clone(),
                spawn.profile_resolution.fallback_applied,
                spawn.match_tier.clone(),
                spawn.original_profile_letter.clone(),
                effective_codex_home.clone(),
                Some(spawn.profile_content_hash.clone()),
            )
            .await
            .map_err(|error| error.to_string())?;
            session.requested_profile = Some(spawn.profile_resolution.requested_profile.clone());
            session.effective_profile = Some(spawn.profile_resolution.effective_profile.clone());
            session.profile_fallback_chain = spawn.profile_resolution.fallback_chain.clone();
            session.profile_fallback_applied = spawn.profile_resolution.fallback_applied;
            session.match_tier = spawn.match_tier.clone();
            session.original_profile_letter = spawn.original_profile_letter.clone();
            session.effective_codex_home = effective_codex_home;
            session.profile_content_hash = Some(spawn.profile_content_hash.clone());
        }

        let mut shell_args = shell_args;
        let mut effective_continuation = false;
        let configured_identity = configured_prompt_identity(agent_kind, &shell, &shell_args);
        let full_cmd = format!("{} {}", shell, shell_args.join(" "));
        // #260: single detector (session/profile.rs). Replaces the old
        // starts_with triple; this is the SAME call `strip_auto_injected_args`
        // makes, so the persisted recipe and the runtime identity cannot disagree.
        let context_target = match agent_kind {
            Some(CodingAgentKind::Claude) => {
                Some(crate::config::session_context::ManagedContextTarget::Claude)
            }
            Some(CodingAgentKind::Codex) => {
                Some(crate::config::session_context::ManagedContextTarget::Codex)
            }
            Some(CodingAgentKind::Antigravity) => {
                Some(crate::config::session_context::ManagedContextTarget::Antigravity)
            }
            Some(CodingAgentKind::Pi) => {
                Some(crate::config::session_context::ManagedContextTarget::Pi)
            }
            // #1873 - Muse has no managed context target; a configured agent still
            // resolves its instructions filename (AGENTS.md) via `agent_command`.
            Some(CodingAgentKind::Muse) => None,
            None => None,
        };

        // Single source of truth: store the identity on the SessionManager record
        // AND the local clone (the latter feeds the imminent `session_created` emit).
        mgr.set_pending_agent_kind_with_carrier_permit(&carrier_u1, pending_binding, agent_kind)
            .await
            .map_err(|error| error.to_string())?;
        session.agent_kind = agent_kind;
        let trusted_configured_spawn = resolved_spawn.is_some();
        mgr.set_pending_trusted_configured_spawn_with_carrier_permit(
            &carrier_u1,
            pending_binding,
            trusted_configured_spawn,
        )
        .await
        .map_err(|error| error.to_string())?;
        session.trusted_configured_spawn = trusted_configured_spawn;

        drop(carrier_u1);
        if let Some(continuation) = owned_replacement.as_ref() {
            if !continuation.validate_source().await {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
        }

        // #930 - resolve the host-credential copy-in plan for container coding agents
        // BEFORE the resume probe and the container env translation, so both consumers
        // observe the CLAUDE_CONFIG_DIR we may inject below. Gated by the global setting
        // (default on) and the per-agent profile descriptor; None when off,
        // non-container, an unrecognized agent, or the host file is absent. The plan is
        // pure (env read + file-exists check); the actual copy runs later in the
        // container backend spawn (spawn_runtime_backed), preserving copy-after-seed.
        let spawn_cwd = container_path_context
            .as_ref()
            .map(|context| context.host_root.clone())
            .unwrap_or_else(|| cwd.clone());
        let container_credential = if session.backend_kind == SessionBackendKind::ContainerTransport
        {
            let settings_state = app.state::<SettingsState>();
            let copy_enabled = {
                let cfg = match settings_state.current_read().await {
                    Ok(cfg) => cfg,
                    Err(error) => {
                        let error = error.to_string();
                        release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                        drop(mgr);
                        rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                        return Err(error);
                    }
                };
                cfg.container_credentials_from_host
            };
            if copy_enabled {
                agent_kind
                    .and_then(|k| k.profile().container_credential)
                    .and_then(|src| {
                        crate::pty::container_credentials::resolve_plan(&src, &spawn_cwd)
                    })
            } else {
                None
            }
        } else {
            None
        };
        // #930 - when we WILL copy host creds and the user configured no
        // CLAUDE_CONFIG_DIR (respecting an explicit value OR an explicit removal),
        // default it to the copy directory so a default-on container authenticates with
        // zero env rows. Injected as a host path; the container env translation maps it
        // to /workspace/.claude and the resume probe then treats state as durable.
        let user_has_claude_config_dir = resolved_spawn
            .as_ref()
            .map(|spawn| {
                spawn.effective_env_value(CLAUDE_CONFIG_DIR_KEY).is_some()
                    || spawn.env_remove_keys.iter().any(|remove| {
                        crate::config::settings::normalize_env_key_for_platform(remove)
                            == crate::config::settings::normalize_env_key_for_platform(
                                CLAUDE_CONFIG_DIR_KEY,
                            )
                    })
            })
            .unwrap_or(false);
        let injected_claude_config_dir = injected_claude_config_dir_for_copy(
            container_credential.as_ref(),
            user_has_claude_config_dir,
        );

        // Resolve and inspect Claude transcript storage only for a canonically
        // detected Claude launch. Provider-looking Pi option values must not cause
        // any Claude path resolution, filesystem check, warning, or stored memo.
        let is_claude = agent_kind == Some(CodingAgentKind::Claude);
        let resume_probe = claude_resume_probe_target_for_kind(
            agent_kind,
            session.backend_kind,
            container_path_context.as_ref().map(|context| &context.map),
            resolved_spawn.as_ref(),
            injected_claude_config_dir.as_deref(),
            &shell,
            &shell_args,
            &cwd,
        );
        let resolved_claude_projects_dir = resume_probe
            .as_ref()
            .and_then(|probe| probe.host_probe_path.clone());
        let carrier_projects = match with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            acquire_lifecycle_create_publication_region(
                &create_owner,
                &mgr,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            Ok(permit) => permit,
            Err(error) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(error);
            }
        };
        mgr.set_pending_resolved_claude_projects_dir_with_carrier_permit(
            &carrier_projects,
            pending_binding,
            resolved_claude_projects_dir.clone(),
        )
        .await
        .map_err(|error| error.to_string())?;
        drop(carrier_projects);
        if let Some(continuation) = owned_replacement.as_ref() {
            if !continuation.validate_source().await {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
        }
        session.resolved_claude_projects_dir = resolved_claude_projects_dir.clone();
        let claude_project_exists = resume_probe
            .as_ref()
            .and_then(|probe| probe.host_probe_path.as_ref())
            .is_some_and(|path| path.is_dir());
        let mut session_env_warnings: Vec<ContainerEnvWarning> = Vec::new();
        if let Some(warning) = resume_probe
            .as_ref()
            .and_then(|probe| probe.warning.clone())
        {
            session_env_warnings.push(warning);
        }
        let will_inject_continue = should_inject_continue(
            is_claude,
            skip_auto_resume,
            claude_project_exists,
            &full_cmd,
        );
        if let Some(probe) = resume_probe.as_ref() {
            log::info!(
                "[session] claude-resume-decision {} cwd={:?} projects_dir={:?} filesystem={} \
                 exists={} skip_auto_resume={} -> inject_continue={}",
                &id.to_string()[..8],
                cwd,
                resolved_claude_projects_dir,
                probe.filesystem,
                claude_project_exists,
                skip_auto_resume,
                will_inject_continue
            );
        } else if let Some(kind) = agent_kind {
            log::info!(
                "[session] resume-decision {} agent={} cwd={:?} skip_auto_resume={}",
                &id.to_string()[..8],
                kind.as_str(),
                cwd,
                skip_auto_resume
            );
        }
        if will_inject_continue {
            // #260: Claude's resume flag from the CodingAgentProfile. resume_tokens
            // is a 1-element const for Claude, so [0] is provably in bounds.
            let continue_flag = CodingAgentKind::Claude.profile().resume_tokens[0];
            if let Some(ref aid) = agent_id {
                if executable_basename(&shell) == "cmd" {
                    if let Some(last) = shell_args.last_mut() {
                        if executable_basename(last) == "claude"
                            || last.to_lowercase().contains("claude")
                        {
                            *last = format!("{} {}", last, continue_flag);
                            effective_continuation = true;
                            log::info!(
                                "Auto-injected --continue for agent '{}' \
                                 (prior conversation exists, cmd path)",
                                aid
                            );
                        }
                    }
                } else {
                    shell_args.push(continue_flag.to_string());
                    effective_continuation = true;
                    log::info!(
                        "Auto-injected --continue for agent '{}' (prior conversation exists)",
                        aid
                    );
                }
            }
        }

        // (#756) Rider: on Claude spawns where AC suppresses resume, mint the fresh
        // session's identity at spawn. Uuid::new_v4() satisfies the "unused UUID"
        // requirement (Claude errors on a colliding --session-id; v4 collision is
        // negligible and a fresh one is minted per spawn). Mutually exclusive with
        // the --continue block above by construction (will_inject_continue requires
        // skip_auto_resume=false). `full_cmd` predates both blocks' mutations, so
        // the veto scan sees user-configured identity flags, never AC's own.
        if should_inject_fresh_session_id(is_claude, skip_auto_resume, &full_cmd) {
            if let Some(ref aid) = agent_id {
                let fresh_session_id = Uuid::new_v4();
                if executable_basename(&shell) == "cmd" {
                    if let Some(last) = shell_args.last_mut() {
                        if executable_basename(last) == "claude"
                            || last.to_lowercase().contains("claude")
                        {
                            *last = format!("{} --session-id {}", last, fresh_session_id);
                            log::info!(
                            "Auto-injected --session-id {} for agent '{}' (fresh spawn, cmd path, \
                             #756)",
                            fresh_session_id,
                            aid
                        );
                        }
                    }
                } else {
                    shell_args.push("--session-id".to_string());
                    shell_args.push(fresh_session_id.to_string());
                    log::info!(
                        "Auto-injected --session-id {} for agent '{}' (fresh spawn, #756)",
                        fresh_session_id,
                        aid
                    );
                }
            }
        }

        if maybe_inject_pi_resume(
            agent_kind,
            resolved_spawn.as_ref(),
            skip_auto_resume,
            &shell,
            &mut shell_args,
        ) {
            effective_continuation = true;
            if let Some(spawn) = resolved_spawn.as_ref() {
                log::info!(
                    "Auto-injected Pi `--continue` for trusted agent '{}'",
                    spawn.trusted_agent_id
                );
            }
        }

        if agent_kind == Some(CodingAgentKind::Codex) && !skip_auto_resume {
            if let Some(ref aid) = agent_id {
                if inject_codex_resume(&shell, &mut shell_args) {
                    effective_continuation = true;
                    log::info!("Auto-injected `codex resume --last` for agent '{}'", aid);
                }
            }
        }

        if agent_kind == Some(CodingAgentKind::Antigravity) && !skip_auto_resume {
            if let Some(ref aid) = agent_id {
                if inject_antigravity_resume(&shell, &mut shell_args) {
                    effective_continuation = true;
                    log::info!("Auto-injected `agy --continue` for agent '{}'", aid);
                }
            }
        }

        // #1873 - Muse workspace-latest resume: only a trusted, exact, empty-argv,
        // local macOS/Linux recipe with resume intent receives `resume --last`,
        // appended to the LOCAL effective vector only. Fresh intent, configured
        // arguments, wrappers, containers, Windows and ad-hoc launches stay plain.
        if maybe_inject_muse_resume(
            agent_kind,
            resolved_spawn.as_ref(),
            skip_auto_resume,
            &shell,
            &mut shell_args,
        ) {
            effective_continuation = true;
            if let Some(spawn) = resolved_spawn.as_ref() {
                log::info!(
                    "Auto-injected Muse `resume --last` for trusted agent '{}'",
                    spawn.trusted_agent_id
                );
            }
        }

        // #529 - resolve the instructions filename from the configured coding agent
        // (falling back to detection for ad-hoc launches), plus the union of every
        // configured agent's filename for cleanup. Computed under a single settings
        // read guard that is dropped before any filesystem I/O (no guard across the
        // materialize call).
        let (target_filename, managed_filenames, auto_self_clear): (
            Option<String>,
            Vec<String>,
            bool,
        ) = {
            let settings_state = app.state::<SettingsState>();
            let cfg = match settings_state.current_read().await {
                Ok(cfg) => cfg,
                Err(error) => {
                    let error = error.to_string();
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                    return Err(error);
                }
            };
            let managed = crate::config::agent_command::managed_instructions_filenames(&cfg);
            let target = crate::config::agent_command::resolve_target_filename(
                agent_id.as_deref(),
                &cfg,
                context_target,
            );
            // Resolve under the same guard; no second lock is held across I/O.
            // Eligibility is an explicit direct-shell capability, independent of
            // CodingAgentKind provider tuning.
            let auto_self_clear =
                resolve_launch_auto_self_clear(&cfg, &shell, &cwd, is_coordinator);
            (target, managed, auto_self_clear)
        };

        if let Some(ref target_filename) = target_filename {
            if owned_replacement.is_some() {
                revalidate_owned_create_effect(
                    &create_owner,
                    &mut resource_permit,
                    &resource_monitor,
                    &mgr,
                    pty_mgr,
                )
                .await?;
            }
            let context_effect = inline_transaction
                .and_then(|transaction| {
                    transaction.start_lifecycle_effect(
                        crate::session::selection::LifecycleEffectSlot::ContextPreparation,
                    )
                })
                .or_else(|| {
                    owned_replacement.as_ref().and_then(|continuation| {
                        continuation.start_effect(
                            crate::session::selection::LifecycleEffectSlot::ContextPreparation,
                        )
                    })
                });
            if (owned_replacement.is_some()
                || inline_transaction.is_some_and(|transaction| transaction.has_lifecycle_entry()))
                && context_effect.is_none()
            {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
            #[cfg(test)]
            let context_fixture = owned_replacement
                .as_ref()
                .and_then(|owned| owned.fixture_native(false));
            #[cfg(test)]
            let native_await_panic = owned_replacement
                .as_ref()
                .and_then(|owned| owned.fixture_take_native_await_panic(false));
            let context_native = coordinator.run_blocking_seed_work({
                let cwd = cwd.clone();
                // #1172 D5: capture the FINAL fresh-versus-resume decision. `skip_auto_resume`
                // is a `mut` parameter of this function whose only mutation is the #756 mirror
                // at :1470, ~500 lines above, so the value read here is final. `true` means AC
                // is deliberately NOT resuming: a fresh conversation begins.
                let start_fresh = skip_auto_resume;
                let target_filename = target_filename.clone();
                let managed_filenames = managed_filenames.clone();
                let container_repos = container_repos.clone();
                move || {
                    #[cfg(test)]
                    if let Some(control) = context_fixture.as_ref() {
                        control.enter();
                    }
                    // #1065 Stage F: production records the session-spawn context
                    // read/sync and self-heal under the per-project gate (acquired
                    // inside the blocking worker so the 10-second poll never blocks a
                    // Tokio worker); a `#[cfg(test)]` lib build stays non-emitting.
                    #[cfg(not(test))]
                    let activation =
                        Some(crate::config::seed_manifest::ManifestActivationToken::production());
                    #[cfg(test)]
                    let activation: Option<
                        crate::config::seed_manifest::ManifestActivationToken,
                    > = None;
                    use crate::config::session_context as ctx_file;
                    let context_result =
                        ctx_file::materialize_agent_context_file_with_filename_activated(
                            &cwd,
                            &target_filename,
                            &managed_filenames,
                            is_coordinator,
                            auto_self_clear,
                            container_repos.as_ref(),
                            activation.as_ref(),
                        );
                    // #1172 - rotate the origin Agent Matrix's `memory/` for the fresh session
                    // about to start, so it begins with a clean write target and the previous
                    // session's memory is preserved under `memory-archive/memory_<ts>/`.
                    //
                    // Two gates, both load-bearing (D5):
                    //   - `start_fresh`: a RESUME never rotates. This chokepoint also
                    //     serves the app-startup restore path
                    //     (`create_session_inner_for_restore`, :1239), which continues an
                    //     existing conversation; emptying `memory/` under a resumed agent
                    //     is the one outcome the user ruled out.
                    //   - `is_ok()`: a launch that is about to roll back (:2016-2036)
                    //     leaves no rotation behind.
                    //
                    // Never fails a launch: `rotate_origin_memory_at_spawn` returns `()`
                    // and every error path inside it warns and returns.
                    if start_fresh && context_result.is_ok() {
                        crate::config::agent_memory::rotate_origin_memory_at_spawn(&cwd);
                    }
                    if let Some(effect) = context_effect {
                        effect
                            .complete(context_result.as_ref().map(|_| None).map_err(Clone::clone));
                    }
                    #[cfg(test)]
                    if let Some(control) = context_fixture.as_ref() {
                        control.returned(format!("{context_result:?}"));
                    }
                    context_result
                }
            });
            #[cfg(test)]
            let context_result =
                crate::session::selection::await_fixture_native(context_native, native_await_panic)
                    .await;
            #[cfg(not(test))]
            let context_result = context_native.await;
            let context_result = match context_result {
                Ok(result) => result,
                Err(error) => {
                    let error = format!("replica context blocking preparation failed: {error}");
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                    return Err(error);
                }
            };
            match context_result {
                Ok(_) => {}
                Err(e) => {
                    log::error!("Replica context validation failed: {}", e);
                    use tauri_plugin_dialog::DialogExt;
                    // #537 facet (b) - the old copy blamed "context files missing",
                    // but the real cause is usually a transient config.json lock
                    // during replica identity repair. State what actually failed;
                    // the interpolated error carries the precise, retry-suggesting
                    // detail from format_publish_error.
                    let dialog_msg = format!(
                        "Cannot launch session - failed to update replica config:\n\n{}",
                        e
                    );
                    let dialog_title = "Session Launch Error";
                    #[cfg(test)]
                    let notification_observed = owned_replacement.as_ref().is_some_and(|owned| {
                        owned.fixture_assert_context_failure_observer(
                            &e,
                            dialog_title,
                            &dialog_msg,
                        );
                        owned.fixture_observe_context_failure(&e, dialog_title, &dialog_msg)
                    });
                    #[cfg(not(test))]
                    let notification_observed = false;
                    if !notification_observed {
                        app.dialog()
                            .message(&dialog_msg)
                            .title(dialog_title)
                            .show(|_| {});
                    }
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &e).await;
                    return Err(e);
                }
            }
        }

        // Capture the effective arg vector BEFORE spawn so SessionInfo::from(&session)
        // (emitted at line ~439 as "session_created") carries provider resume flags.
        // Bind once, broadcast to two consumers: the store write is for later
        // `mgr.get_session` callers; the local-clone write is for the imminent emit.
        //
        // DO NOT REMOVE OR GATE THIS CAPTURE. Issue #65 regression guard: removing
        // or wrapping in a condition reintroduces the exact bug this plan fixes.
        // See _plans/bug-statusbar-dynamic-launch-args.md §10 and §15 for rationale.
        let effective = shell_args.clone();
        let carrier_args = match with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            acquire_lifecycle_create_publication_region(
                &create_owner,
                &mgr,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            Ok(permit) => permit,
            Err(error) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(error);
            }
        };
        mgr.set_pending_effective_shell_args_with_carrier_permit(
            &carrier_args,
            pending_binding,
            effective.clone(),
        )
        .await
        .map_err(|error| error.to_string())?;
        drop(carrier_args);
        if let Some(continuation) = owned_replacement.as_ref() {
            if !continuation.validate_source().await {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
        }
        session.effective_shell_args = Some(effective);

        let extra_env = if agent_id.is_some() {
            crate::pty::credentials::build_credentials_env(&session.token, &cwd)
        } else {
            Vec::new()
        };
        let mut configured_env: Vec<(String, String)> = resolved_spawn
            .as_ref()
            .map(|spawn| spawn.child_env.clone())
            .unwrap_or_default();
        // #930 - inject the copy directory as a host-path CLAUDE_CONFIG_DIR so the
        // container env translation below maps it to /workspace/.claude and the copied
        // token is actually read. Only present when we will copy and the user set none.
        if let Some(dir) = injected_claude_config_dir.as_ref() {
            configured_env.push((CLAUDE_CONFIG_DIR_KEY.to_string(), dir.clone()));
        }
        let env_remove_keys: Vec<String> = resolved_spawn
            .as_ref()
            .map(|spawn| spawn.env_remove_keys.clone())
            .unwrap_or_default();
        let mut env_unset: Vec<String> = Vec::new();
        if session.backend_kind == SessionBackendKind::ContainerTransport {
            if let Some(context) = container_path_context.as_ref() {
                let translated = crate::pty::container_backend::container_child_env(
                    configured_env,
                    env_remove_keys.clone(),
                    &context.map,
                );
                configured_env = translated.child_env;
                env_unset = translated.env_unset;
                session_env_warnings.extend(translated.warnings);
            }
        }
        let (resource_registration, logical_resource_slot): (
            Option<ResourceLaunchRegistration>,
            Option<ResourceLogicalAgentSlot>,
        ) = match session.backend_kind {
            SessionBackendKind::LocalProcess => resource_permit
                .take()
                .map(|permit| {
                    // #516 - human-readable WG + agent identity for the Resource Monitor row,
                    // derived from the deterministic spawn cwd (not the user-renamable
                    // session.name). Root agents carry no wg-/__agent_ segments, so label them
                    // explicitly with the bare replica dir name.
                    let (workgroup, agent) = {
                        let (wg, ag) = crate::config::teams::workgroup_and_agent_from_path(&cwd);
                        if wg.is_some() {
                            (wg, ag)
                        } else if is_root_agent {
                            let bare = cwd
                                .replace('\\', "/")
                                .rsplit('/')
                                .find(|s| !s.is_empty())
                                .map(|s| s.to_string());
                            (Some("Root agent".to_string()), bare)
                        } else {
                            (None, None)
                        }
                    };
                    // #566 - project folder name for the Resource Monitor row, derived from
                    // the same deterministic spawn cwd as the (workgroup, agent) pair above.
                    let project = crate::config::teams::project_from_path(&cwd);
                    ResourceLaunchRegistration::new(
                        resource_monitor.as_ref().clone(),
                        permit,
                        ResourceLaunchMetadata {
                            session_id: id,
                            name: session.name.clone(),
                            agent_id: agent_id.clone(),
                            agent_label: agent_label.clone(),
                            workgroup,
                            agent,
                            project,
                        },
                    )
                })
                .map(|registration| (Some(registration), None))
                .unwrap_or((None, None)),
            SessionBackendKind::ContainerTransport => (
                None,
                resource_permit
                    .take()
                    .and_then(|permit| resource_monitor.hold_logical_agent_slot(permit)),
            ),
        };

        // #598 - seed the config folder before the PTY starts so the agent sees the
        // templated config. Best-effort: never aborts the spawn. This is the single
        // execution chokepoint every real spawn funnels through (create / replica /
        // other variants, delivery / mailbox / web wakes); prevalidation never hits
        // it because it discards the spawn. Per Q2 this runs on EVERY spawn,
        // including loop/scheduler resume-wakes (overwrite every spawn).
        if let Some(seed) = resolved_spawn.as_ref().and_then(|s| s.seed.as_ref()) {
            // grinch HIGH-1: serialize the seed swap for ALL dests (not just
            // `.claude`). Two same-replica spawns can run concurrently (e.g. a
            // delivery tick + a mailbox wake) holding only `session_mgr.read()`,
            // with no global spawn lock. Without serialization, spawn B's
            // prefix-sweep of stale scratch (`clear_stale_seed_scratch`) could
            // delete spawn A's in-flight temp/trash mid-swap and lose the config
            // (breaking H1's "dest always fully-old or fully-new" + M3 isolation).
            // Under this lock any other-id scratch is truly stale, so the
            // leak-reclaim sweep stays safe. Clone the Arc out of State first so the
            // owned guard does not borrow a State temporary (E0716).
            let seed = seed.clone();
            if owned_replacement.is_some() {
                revalidate_owned_create_effect(
                    &create_owner,
                    &mut resource_permit,
                    &resource_monitor,
                    &mgr,
                    pty_mgr,
                )
                .await?;
            }
            let seed_effect = inline_transaction
                .and_then(|transaction| {
                    transaction.start_lifecycle_effect(
                        crate::session::selection::LifecycleEffectSlot::ConfigSeed,
                    )
                })
                .or_else(|| {
                    owned_replacement.as_ref().and_then(|continuation| {
                        continuation.start_effect(
                            crate::session::selection::LifecycleEffectSlot::ConfigSeed,
                        )
                    })
                });
            if (inline_transaction.is_some_and(|transaction| transaction.has_lifecycle_entry())
                || owned_replacement.is_some())
                && seed_effect.is_none()
            {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
            let lock = app.state::<crate::ConfigSeedLockState>().inner().clone();
            #[cfg(test)]
            let seed_fixture = owned_replacement
                .as_ref()
                .and_then(|owned| owned.fixture_native(true));
            #[cfg(test)]
            let native_await_panic = owned_replacement
                .as_ref()
                .and_then(|owned| owned.fixture_take_native_await_panic(true));
            let seed_native = coordinator.run_blocking_seed_work(move || {
                #[cfg(test)]
                if let Some(control) = seed_fixture.as_ref() {
                    control.enter();
                }
                let _seed_guard = lock.blocking_lock_owned();
                // #1065 Stage F: production records the config-seed publication
                // under the per-project gate, acquired AFTER this global lock
                // (plan section 6.3 lock order) and dropped before returning; a
                // `#[cfg(test)]` lib build stays non-emitting.
                #[cfg(not(test))]
                let activation =
                    Some(crate::config::seed_manifest::ManifestActivationToken::production());
                #[cfg(test)]
                let activation: Option<
                    crate::config::seed_manifest::ManifestActivationToken,
                > = None;
                let report = crate::config::config_seed::perform_config_seed_recorded(
                    &seed,
                    &id.to_string(),
                    activation.as_ref(),
                );
                // The blocking worker records its actual report even when
                // the async receiver disappears. Native failure remains the
                // existing best-effort report, not a new spawn error.
                if let Some(effect) = seed_effect {
                    effect.complete(Ok(Some(format!("{report:?}"))));
                }
                drop(_seed_guard);
                #[cfg(test)]
                if let Some(control) = seed_fixture.as_ref() {
                    control.returned(format!("{report:?}"));
                }
                report
            });
            #[cfg(test)]
            let seed_result =
                crate::session::selection::await_fixture_native(seed_native, native_await_panic)
                    .await;
            #[cfg(not(test))]
            let seed_result = seed_native.await;
            match seed_result {
                Ok(report) => {
                    log::debug!("[config-seed] session {} outcome: {:?}", id, report);
                }
                Err(error) => {
                    let error = format!("config seed blocking preparation failed: {error}");
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                    return Err(error);
                }
            }
        }

        // #1063 test-only barrier: the seed transaction (if any) has acquired and
        // released the project gate, but the create is still pending and unfinalized,
        // so a deletion's pending-inclusive snapshot must still observe it. No-op in
        // production.
        #[cfg(test)]
        {
            seed_race_barriers::hit_after_seed_before_pty(&cwd).await;
        }

        let viewport = viewport.unwrap_or(PtyViewport::DEFAULT);
        // #2124 - the idle tuning this launch hands the detector: the agent kind
        // plus the EFFECTIVE catalog entry matched to the configured command.
        // One settings snapshot, released before the blocking filesystem read;
        // no lock is held across that I/O.
        let launch_idle_tuning = {
            let settings_state = app.state::<SettingsState>();
            let snapshot = match settings_state.current_snapshot().await {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    let error = error.to_string();
                    release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                    drop(mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                    return Err(error);
                }
            };
            let requested_agent_id = agent_id.clone();
            match tokio::task::spawn_blocking(move || {
                let catalog =
                    crate::config::coding_agents_catalog::load_catalog_for_settings(&snapshot)
                        .unwrap_or_default();
                resolve_launch_idle_tuning(
                    &snapshot,
                    requested_agent_id.as_deref(),
                    agent_kind,
                    &catalog,
                )
            })
            .await
            {
                Ok(tuning) => tuning,
                Err(error) => {
                    log::warn!("[idle] launch idle tuning resolution failed: {error}");
                    idle_tuning_for(agent_kind)
                }
            }
        };
        let launch_witness = LaunchWitness::default();
        let spawn_spec = BackendSpawnSpec {
            id,
            agent_id: agent_id.clone(),
            // #942 - the CLI identity from the canonical detector (`agent_kind`, resolved
            // at the top of this function). The profile id above cannot stand in for it:
            // it is opaque, several profiles can drive the same CLI, and a coding-agent
            // launch can carry no profile id at all.
            coding_agent: agent_kind,
            cmd: shell.clone(),
            args: shell_args.clone(),
            // #1271 - the configured host shell paired with a resolved agent
            // command, carried as one immutable snapshot (never as loose
            // program/argument fields, so the pairing invariant cannot drift).
            resolved_agent_host_shell: resolved_agent_host_shell.clone(),
            cwd: spawn_cwd.clone(),
            selected_cwd: if spawn_cwd != cwd {
                Some(cwd.clone())
            } else {
                None
            },
            // #973 - open the PTY at the size the terminal is actually going to be, so the
            // frontend never has to resize a child that is still starting up. Callers with no
            // view (restore loop, delivery loop, mailbox, CLI, tests) get PtyViewport::DEFAULT,
            // which is the 120x30 AC always used.
            cols: viewport.cols,
            rows: viewport.rows,
            container_image: resolved_spawn
                .as_ref()
                .and_then(|spawn| spawn.backend.image.clone()),
            configured_env,
            env_remove_keys,
            env_unset,
            extra_env,
            idle_tuning: launch_idle_tuning,
            output_target: PtyOutputTarget::from_app_handle(app.clone()),
            resource_registration,
            logical_resource_slot,
            container_credential,
            container_repo_mounts: container_repos
                .as_ref()
                .map(|resolution| {
                    resolution
                        .mounts()
                        .map(
                            |(host, container)| crate::pty::container_repos::ContainerRepoMount {
                                host_path: host.to_path_buf(),
                                container_path: container.to_string(),
                            },
                        )
                        .collect()
                })
                .unwrap_or_default(),
            launch_witness: launch_witness.clone(),
        };
        // (#2413) Spawn fence: set BEFORE the first await of spawn, so a drop at
        // any point inside or after spawn keeps the close markers cleared.
        if let Some(t) = marker_ticket
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            t.session_may_exist = true;
        }
        if let Some(continuation) = owned_replacement.as_ref() {
            if !continuation.validate_source().await {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Ok(CreateCompletion::Retained);
            }
        }
        if owned_replacement.is_some() {
            revalidate_owned_create_effect(
                &create_owner,
                &mut resource_permit,
                &resource_monitor,
                &mgr,
                pty_mgr,
            )
            .await?;
        }
        let backend_effect = inline_transaction
            .and_then(|transaction| {
                transaction.start_lifecycle_effect(
                    crate::session::selection::LifecycleEffectSlot::BackendSpawn,
                )
            })
            .or_else(|| {
                owned_replacement.as_ref().and_then(|continuation| {
                    continuation
                        .start_effect(crate::session::selection::LifecycleEffectSlot::BackendSpawn)
                })
            });
        if (inline_transaction.is_some_and(|transaction| transaction.has_lifecycle_entry())
            || owned_replacement.is_some())
            && backend_effect.is_none()
        {
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Ok(CreateCompletion::Retained);
        }
        #[cfg(test)]
        if let Some(owned) = owned_replacement.as_ref() {
            if !owned
                .fixture_reached(
                    crate::session::selection::ReplacementFixtureCut::BeforeBackendSpawn,
                )
                .await
            {
                return Ok(CreateCompletion::Retained);
            }
        }
        let spawn_result = PtyManager::spawn(pty_mgr, session.backend_kind, spawn_spec).await;
        if let Some(effect) = backend_effect {
            effect.complete(
                spawn_result
                    .as_ref()
                    .map(|_| Some(id.to_string()))
                    .map_err(|error| error.to_string()),
            );
        }
        drop(spawn_mark);
        if let Err(e) = spawn_result {
            if !launch_witness.launched() {
                if let Some(t) = marker_ticket
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_mut()
                {
                    t.session_may_exist = false;
                }
            } else {
                log::warn!(
                    "[coordinator-create] spawn failed after process launch; close markers stay \
                     cleared session={}",
                    id
                );
            }
            let err = e.to_string();
            drop(mgr);
            rollback_pre_created_session(app, session_mgr, pty_mgr, id, &err).await;
            return Err(err);
        }

        if let Some(continuation) = owned_replacement.as_ref() {
            let actual = mgr.get_pending_session(pending_binding).await;
            let generation = pty_mgr
                .lock()
                .ok()
                .and_then(|pty| pty.route_identities(id).map(|route| route.2));
            if !actual
                .zip(generation)
                .is_some_and(|(row, generation)| continuation.record_launch(&row, generation))
            {
                return Ok(CreateCompletion::Retained);
            }
        }
        register_session_samplers(app, id, agent_id.clone());

        // Auto-inject optional non-credential bootstrap text for agent sessions
        // after PTY spawn. Credentials are already present in child environment
        // variables; no credentials are written through PTY.
        //
        // Currently the only bootstrap payload is the Coordinator auto-title prompt.
        if agent_id.is_some() && !is_coordinator {
            log::debug!(
                "[session] No bootstrap injection for non-coordinator agent session {}",
                id
            );
        }
        if agent_id.is_some() && is_coordinator {
            let auto_title_enabled = false; /*
                                                let settings_state = app.state::<SettingsState>();
                                                let cfg = settings_state.read().await;
                                                cfg.auto_generate_task_title
                                            };*/

            if auto_title_enabled {
                let app_clone = app.clone();
                let session_id = id;
                let cwd_clone = cwd.clone();

                tokio::spawn(async move {
                    let prompt = match build_title_prompt_appendage(&cwd_clone) {
                        Ok(Some(prompt)) => {
                            log::info!(
                                "[session] Auto-title prompt built for session {}",
                                session_id
                            );
                            prompt
                        }
                        Ok(None) => {
                            log::info!(
                            "[session] Auto-title prompt skipped (gate not passed) for session {}",
                            session_id
                        );
                            return;
                        }
                        Err(e) => {
                            log::warn!(
                                "[session] Auto-title prompt skipped for session {}: {}",
                                session_id,
                                e
                            );
                            return;
                        }
                    };

                    let max_wait = std::time::Duration::from_secs(30);
                    let poll = std::time::Duration::from_millis(500);
                    let start = std::time::Instant::now();

                    loop {
                        if start.elapsed() >= max_wait {
                            log::warn!(
                            "[session] Timeout waiting for idle before auto-title prompt injection \
                             for session {}",
                            session_id
                        );
                            break;
                        }
                        tokio::time::sleep(poll).await;

                        let session_mgr =
                            app_clone.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                        let mgr = session_mgr.read().await;
                        let sessions = mgr.list_sessions().await;
                        match sessions.iter().find(|s| s.id == session_id.to_string()) {
                            Some(s) if s.waiting_for_input => break,
                            Some(_) => {}
                            None => {
                                log::warn!(
                                    "[session] Session {} gone before auto-title prompt injection",
                                    session_id
                                );
                                return;
                            }
                        }
                    }

                    match crate::pty::inject::inject_text_into_session(
                        &app_clone, session_id, &prompt,
                    )
                    .await
                    {
                        Ok(()) => {
                            log::info!(
                                "[session] Auto-title prompt injected for session {}",
                                session_id
                            );
                        }
                        Err(e) => {
                            log::warn!(
                                "[session] Failed to inject auto-title prompt for {}: {}",
                                session_id,
                                e
                            );
                        }
                    }
                });
            }
        }

        let finalization_warnings = session_env_warnings
            .iter()
            .map(|warning| {
                SessionWarning::new(
                    id,
                    warning.key.clone(),
                    warning.kind,
                    warning.message.clone(),
                )
            })
            .collect::<Vec<_>>();

        // 0.8.0: removed the "Show the terminal window when a session is created" branch.
        // Under the unified-window model the main window is created up-front and stays
        // visible; session creation has no window-show responsibility.

        // Save lastCodingAgent + codingAgents (skip for temp sessions)
        if !skip_tooling_save {
            if let Some(ref aid) = agent_id {
                // Resolve label: use provided agent_label, or look up from settings by agent_id.
                // Without this fallback, callers that pass agent_id but no label (session-requests,
                // web remote) would write app: "Unknown" into the per-instance config.json.
                // #2433 - computed under the settings read lock, written after it drops.
                let write = {
                    let settings = app.state::<SettingsState>();
                    let write = match settings.current_read().await {
                        Ok(cfg) => Some(prepare_last_coding_agent_write(
                            aid,
                            agent_label.as_deref(),
                            &cfg,
                        )),
                        Err(error) => {
                            log::debug!("[session] skipping lastCodingAgent publication: {error}");
                            None
                        }
                    };
                    write
                };
                if let Some(write) = write {
                    if let Err(e) = save_last_coding_agent(&cwd, &id.to_string(), &write) {
                        log::warn!("Failed to save lastCodingAgent: {}", e);
                    }
                }
                // #592 - persist the loaded profile content-hash so drift survives an
                // AC restart. Best-effort; never aborts the spawn. No-op off-replica.
                if let Some(hash) = session.profile_content_hash.as_deref() {
                    log::debug!(
                        "[profile-hash] spawn-persist: session={} agent={} hash={} cwd={:?}",
                        id,
                        aid,
                        hash,
                        cwd,
                    );
                    if let Err(e) =
                        crate::config::coding_agent_profiles::set_replica_profile_content_hash(
                            std::path::Path::new(&cwd),
                            hash,
                        )
                    {
                        log::warn!("Failed to persist profileContentHash: {}", e);
                    }
                }
            }
        }

        // A configured selector is never evidence of AC-attributed continuation,
        // even when an injector also happened to add a control of its own.
        effective_continuation &= !configured_identity;
        let prompt_fresh = !configured_identity
            && (!effective_continuation
                || skip_auto_resume
                || pending_start_fresh == Some(true)
                || fresh_forced_by_mirror);
        let key = if session.backend_kind == crate::pty::backend::SessionBackendKind::LocalProcess {
            crate::config::sessions_persistence::prompt_key(
                &cwd,
                session.agent_id.as_deref(),
                session.requested_profile.as_deref(),
                session.effective_profile.as_deref(),
            )
        } else {
            None
        };
        let initial = initial_last_prompt
            .as_ref()
            .filter(|source| source.compatible(&session))
            .and_then(|source| source.text.clone());
        let revalidated = crate::config::sessions_persistence::prompt_key(
            &cwd,
            session.agent_id.as_deref(),
            session.requested_profile.as_deref(),
            session.effective_profile.as_deref(),
        );
        let key = key.filter(|candidate| Some(candidate) == revalidated.as_ref());
        let carrier_prompt = match with_create_resource_region(
            &create_owner,
            &mut resource_permit,
            acquire_lifecycle_create_publication_region(
                &create_owner,
                &mgr,
                lifecycle_domain,
                inline_transaction,
            ),
        )
        .await
        {
            Ok(permit) => permit,
            Err(error) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                return Err(error);
            }
        };
        mgr.initialize_pending_prompt_with_carrier_permit(
            &carrier_prompt,
            pending_binding,
            key,
            initial,
            effective_continuation,
            prompt_fresh,
            !defer_inline_finalization,
        )
        .await?;
        drop(carrier_prompt);

        if defer_inline_finalization {
            if create_ticket
                .lock()
                .map_err(|_| "create ticket unavailable".to_string())?
                .is_some()
                || inline_transaction.is_none()
            {
                return Err(
                    "deferred create finalization requires an inline transaction".to_string(),
                );
            }
            let info = mgr
                .get_pending_session(pending_binding)
                .await
                .map(|record| SessionInfo::from(&record))
                .ok_or_else(|| "deferred create pending record disappeared".to_string())?;
            inline_pending_for_body
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            return Ok(CreateCompletion::Deferred(DeferredCreateOutput {
                info,
                binding: pending_binding,
                warnings: finalization_warnings,
                _create_admission: create_owner.clone(),
                effective_continuation,
            }));
        }

        let finalization_ticket = create_ticket
            .lock()
            .map_err(|_| "create ticket unavailable".to_string())?
            .take();
        let finalized = if let Some(ticket) = finalization_ticket {
            ticket
                .finalize_with_owned_outcome(finalization_warnings)
                .await
        } else {
            let transaction = inline_transaction
                .ok_or_else(|| "inline create transaction is unavailable".to_string())?;
            let cause =
                inline_cause.ok_or_else(|| "inline create cause is unavailable".to_string())?;
            transaction
                .finalize_inline_create(
                    pending_binding,
                    cause,
                    finalization_warnings,
                    owned_replacement.clone(),
                )
                .await
        };
        let Some(finalized) = finalized else {
            return Ok(CreateCompletion::Retained);
        };
        if finalized.is_ok() && inline_transaction.is_some() {
            inline_pending_for_body
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
        }
        let completion = finalized.map(CreateCompletion::Finalized);
        // #2232 phase 4 section 5.1: the Room demand may be raised only once
        // the row is **finalized and visible** to `get_session` — a
        // pending-create row is hidden, so a raise spawned earlier answers
        // "session not found" and is never retried. Detached, as before: the
        // raise resolves the reader kind and spawns the watcher under its own
        // locks, off the create path. A restart replacement is finalized later
        // by `execute_restart_transaction`, which raises its Room demand
        // synchronously after the commit (section 5.2); `SelectionCause::Restart(_)`
        // never reaches a finalize here, and the gate keeps that explicit.
        if completion.is_ok() && !matches!(inline_cause, Some(SelectionCause::Restart(_))) {
            let app_for_demand = app.clone();
            tauri::async_runtime::spawn(async move {
                raise_room_reader_demand(&app_for_demand, id).await;
            });
            if let Some(transaction) = inline_transaction {
                transaction.observe_entry_reader_demand(
                    pending_binding,
                    crate::session::selection::EntryReaderDemandReceipt::LegacyTaskAccepted,
                );
            }
        }
        completion
    };
    // This accepted core owns every launched effect and its real result.
    // Coordinator shutdown may reject finalization, but cannot drop a pending
    // backend/native effect and manufacture completion for the caller.
    let result = create_body.await;
    let marker = marker_ticket
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    finish_create_core_result(
        marker,
        result,
        owned_replacement.as_ref(),
        &create_owner,
        session_mgr,
        pty_mgr,
        preheld_target.is_some(),
        inline_pending_binding.as_ref(),
        inline_transaction,
    )
    .await
}

/// Create a new session. Optionally override shell/args/cwd/name (for action buttons).
/// Falls back to settings defaults when not provided.
// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn create_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    _tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    shell: Option<String>,
    shell_args: Option<Vec<String>>,
    cwd: Option<String>,
    session_name: Option<String>,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    git_repos: Option<Vec<SessionRepo>>,
    skip_auto_resume: Option<bool>,
    // #973 - the terminal size the view has already fitted to. Optional: an older frontend,
    // or any caller that has no tile to measure, simply omits it and gets AC's 120x30.
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<SessionInfo, String> {
    let cfg = settings
        .current_read()
        .await
        .map_err(|error| error.to_string())?;

    let cwd = resolve_create_session_cwd(cwd);

    let resolved_spawn = if let Some(aid) = agent_id.as_deref() {
        build_configured_agent_spawn_for_cwd(&cfg, aid, &cwd, requested_profile.as_deref())?
    } else {
        None
    };

    let (shell, shell_args, agent_label, resolved_agent_host_shell) =
        if let Some(spawn) = resolved_spawn.as_ref() {
            // #1271 - keep the resolved agent executable and its logical argv as
            // the command-to-run, and carry a copy of the configured default host
            // shell (program + args from the SAME immutable config snapshot, before
            // `drop(cfg)`) separately to the backend. The backend launches a
            // non-direct Windows agent command through this host shell instead of
            // the unconditional `cmd.exe /C` fallback.
            (
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                Some(spawn.trusted_agent_label.clone()),
                Some(ResolvedAgentHostShell {
                    program: cfg.default_shell.clone(),
                    args: cfg.default_shell_args.clone(),
                }),
            )
        } else {
            let s = shell.unwrap_or_else(|| cfg.default_shell.clone());
            let sa = shell_args.unwrap_or_else(|| cfg.default_shell_args.clone());
            let al = agent_id.as_ref().and_then(|aid| {
                cfg.agents
                    .iter()
                    .find(|a| a.id == *aid)
                    .map(|a| a.label.clone())
            });
            (s, sa, al, None)
        };

    log::info!(
        "[session] FINAL resolved: shell={:?}, args={:?}, label={:?}",
        shell,
        shell_args,
        agent_label
    );

    drop(cfg);

    let info = create_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        shell,
        shell_args,
        cwd.clone(),
        session_name,
        agent_id,
        agent_label,
        false, // persist tooling
        git_repos.unwrap_or_default(),
        // #599 R1: default true (fresh create); reopen of a closed coordinator
        // passes Some(false) so the prior conversation resumes.
        effective_create_skip_auto_resume(skip_auto_resume),
        resolved_spawn,
        resolved_agent_host_shell,
        // #973 - the only caller that has a terminal to measure.
        match (cols, rows) {
            (Some(c), Some(r)) => Some(PtyViewport::from_fit(c, r)),
            _ => None,
        },
        CreateSelectionIntent::User,
    )
    .await?;

    // Auto-attach Telegram bot if repo has .agentscommander/config.json
    let id = Uuid::parse_str(&info.id).unwrap();
    attach_local_config_telegram_if_any(&app, id, &cwd).await;

    Ok(info)
}

#[tauri::command]
pub async fn destroy_session(
    app: AppHandle,
    _session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    _pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    _tg_mgr: State<'_, TelegramBridgeState>,
    _detached: State<'_, DetachedSessionsState>,
    id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    destroy_session_inner(&app, uuid).await
}

/// #588 Manually close a coordinator (and, when the cascade setting is on, its
/// team). Sets the MANUALLY-CLOSED marker on the coordinator regardless of the
/// cascade setting. When cascade is on and confirmation has not been given and at
/// least one team member is working, closes NOTHING and reports `closed:false` so
/// the FE can show the confirmation modal.
#[tauri::command]
pub async fn close_coordinator(
    app: AppHandle,
    id: String,
    confirmed: bool,
) -> Result<CoordinatorCloseOutcome, String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();

    // Snapshot coordinator facts BEFORE destroying (destroy removes the record).
    let (is_coordinator, cwd) = {
        let mgr = session_mgr.read().await;
        let s = mgr
            .get_session(uuid)
            .await
            .ok_or_else(|| "Session not found".to_string())?;
        (s.is_coordinator, s.working_directory.clone())
    };

    // Defensive: a non-coordinator target is a plain destroy (no marker/cascade).
    if !is_coordinator {
        destroy_session_inner(&app, uuid).await?;
        return Ok(CoordinatorCloseOutcome {
            closed: true,
            working_count: 0,
        });
    }

    let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
    let team_key = match fqn.rsplit_once('/') {
        Some((team, _agent)) => team.to_string(),
        None => String::new(),
    };

    // E0716: bind the State to a local FIRST (matches auto_close.rs:115). The
    // guard-bound-then-used-next-line form does NOT compile (the temporary State is
    // dropped at the `;`, so the read guard dangles). `bool` is Copy.
    let settings = app.state::<SettingsState>();
    let cascade = settings
        .current_read()
        .await
        .map_err(|error| error.to_string())?
        .coordinator_cascade_close_enabled;
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

    // LIVE-PTY members of THIS team, excluding the coordinator (decision 1; mirrors
    // auto_close.rs:136-142). agent_team_members() includes EXITED/dormant records
    // (exited sessions persist until destroy), so we filter on has_session to a live
    // set ONCE and share it between the working-count gate and the destroy loop;
    // dormant rows are never reaped. The std Mutex guard is taken AFTER the tokio
    // read await and dropped at the block end, so no lock is held across an await.
    let live_ids: Vec<Uuid> = {
        let mgr = session_mgr.read().await;
        let members = mgr.agent_team_members().await;
        let pm = pty_mgr.lock().unwrap_or_else(|e| e.into_inner());
        members
            .into_iter()
            .filter(|(mid, tk)| {
                *mid != uuid && !team_key.is_empty() && *tk == team_key && pm.has_session(*mid)
            })
            .map(|(mid, _)| mid)
            .collect()
    };

    // Confirmation gate: cascade ON, not yet confirmed, >=1 LIVE member working.
    // Only the tokio read guard is held across the get_session awaits.
    if cascade && !confirmed {
        let mgr = session_mgr.read().await;
        let mut member_states: Vec<(bool, bool)> = Vec::with_capacity(live_ids.len());
        for mid in &live_ids {
            // live_ids is pre-filtered to live PTYs, so `live` is true; the waiting
            // flag is the authoritative session record (absent -> treat as idle so a
            // vanished member never forces a modal).
            let waiting = mgr
                .get_session(*mid)
                .await
                .map(|s| s.waiting_for_input)
                .unwrap_or(true);
            member_states.push((true, waiting));
        }
        let working_count = count_working_members(&member_states);
        if working_count > 0 {
            return Ok(CoordinatorCloseOutcome {
                closed: false,
                working_count,
            });
        }
    }

    // Submit the complete cascade as one destruction transaction. The full
    // planned set is excluded from fallback ranking, member failures are known
    // before the one final manual selection, and no doomed sibling can become an
    // intermediate canonical target.
    execute_manual_coordinator_destroy(&app, uuid, live_ids, cascade).await?;

    // Set the manual marker on the coordinator FQN and persist immediately (this
    // command is not on the auto-close tick, so flush_clocks won't run).
    if let Some(clocks) = app.try_state::<CoordinatorClocksState>() {
        let now = chrono::Utc::now();
        let newly = clocks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mark_manually_closed(&fqn, now);
        if newly {
            let _ = app.emit(
                "coordinator_manual_close_changed",
                serde_json::json!({ "replicaPath": cwd, "manuallyClosedAt": now.to_rfc3339() }),
            );
            // also clear any stale auto-closed pill on the FE (mark_* cleared it
            // server-side via the mutual-exclusion transition).
            let _ = app.emit(
                "coordinator_auto_close_changed",
                serde_json::json!({ "replicaPath": cwd, "autoClosedAt": null }),
            );
        }
        let snapshot = { clocks.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
        if let Err(e) = crate::config::coordinator_clocks::save_map(&snapshot) {
            log::warn!("[manual-close] clocks save failed: {}", e);
        }
    }

    Ok(CoordinatorCloseOutcome {
        closed: true,
        working_count: 0,
    })
}

/// #1171 - this covers BOTH restart paths: `execute_restart_transaction`'s success path and
/// `finalize_failed_restart`. A restart is a normal flow - it is in the invoke handler, the
/// mailbox calls it, and a bulk settings save can restart sessions - so without the purge here
/// every restart would orphan up to 500 ring entries under an id that is already gone from the
/// session list, unreachable from the UI.
fn publish_restart_destroyed<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) {
    transaction.publish_destroyed(session_id);
    let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
    if let Some(window) = transaction.app().get_webview_window(&label) {
        if let Err(error) = window.destroy() {
            log::warn!(
                "[restart] detached window destroy failed session={}: {}",
                session_id,
                error
            );
        }
    }
    purge_session_side_state(transaction.app(), session_id);
}

async fn execute_restart_transaction_in_context<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RestartJobRequest,
    context: &LifecycleExecutionContext<'_>,
) -> Result<SessionInfo, String> {
    if matches!(context, LifecycleExecutionContext::RestoreEntry { .. })
        && !transaction.has_active_restore_capability()
    {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let app = transaction.app();
    let session_mgr = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .inner()
        .clone();
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let settings = app.state::<SettingsState>().inner().clone();
    let RestartJobRequest {
        session_id: uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
        activate_after,
        intent,
        communication_override,
        enforcement,
    } = request;
    // 1. Read config from existing session BEFORE destroying it
    let (
        shell,
        shell_args,
        cwd,
        name,
        stored_agent_id,
        stored_agent_label,
        git_repos,
        is_root_agent,
        telegram_bot_id,
        stored_requested_profile,
        stored_start_fresh,
        stored_communication,
        old_status,
        old_selected,
    ) = {
        let mgr = session_mgr.read().await;
        let session = match mgr.get_session(uuid).await {
            Some(session) => session,
            None => {
                transaction.observe_lifecycle_missing_source();
                return Err("Session not found".to_string());
            }
        };
        let old_selected = mgr.selection_payload().await.id() == Some(uuid);
        (
            session.shell.clone(),
            session.shell_args.clone(),
            session.working_directory.clone(),
            session.name.clone(),
            session.agent_id.clone(),
            session.agent_label.clone(),
            session.git_repos.clone(),
            session.is_root_agent
                || crate::config::root_agent::is_root_agent_path(&session.working_directory),
            session.telegram_bot_id.clone(),
            session.requested_profile.clone(),
            session.start_fresh_on_restore, // (#630/#631) honor an unconsumed durable fresh intent
            session.communication.clone(),  // (#747) candidate raised-hand carry across the reopen
            session.status.clone(),
            old_selected,
        )
    };

    let preparation = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::ContextPreparation);
    if transaction.has_lifecycle_entry() && preparation.is_none() {
        if let Some(authority) = context.authority() {
            authority
                .interrupted
                .store(true, std::sync::atomic::Ordering::Release);
        }
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let prepared = async {
        let cwd = if is_root_agent {
            ensure_lifecycle_root_directory(transaction, context).await?
        } else {
            cwd
        };
        let cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);
        crate::config::archive_gate::probe_spawn_refusal(app, &cwd).await?;
        // §1295 5.1b / B2: the creation gate runs AFTER `probe_spawn_refusal` but
        // WELL BEFORE `teardown_old_for_restart` (below). A rejected restart of a
        // live outside-roots session therefore DECLINES: the old session stays
        // running (live PTY untouched), `Err(sessionCreateBlocked: ...)` propagates,
        // and no `finalize_failed_restart` runs. Root-agent restarts already
        // normalized `cwd` to `ensure_root_agent_dir()` above, which the gate
        // exempts by rule 1.
        crate::config::sessions_persistence::enforce_creation_gate(app, &cwd, enforcement).await?;

        // 2. Strip auto-injected args before restart so the new session starts from the saved
        // recipe.
        let clean_args =
            crate::config::sessions_persistence::strip_auto_injected_args(&shell, &shell_args);

        let requested_agent_id = agent_id;
        let selected_requested_profile =
            effective_restart_requested_profile(requested_profile, stored_requested_profile);
        // #537 read-side / #2434: resolve the launch agent through the one matched
        // selection drift also reads, and resolve its spawn under a single settings
        // read guard. No await is held across the guard; it is dropped at the end of
        // this block. The #1271 host-shell snapshot is copied from the SAME guard so
        // program and args can never pair across a configuration change.
        let (selected_agent_id, resolved_spawn, resolved_agent_host_shell) = {
            let cfg = settings
                .current_read()
                .await
                .map_err(|error| error.to_string())?;
            let selection = matched_selection(
                &cfg,
                &cwd,
                requested_agent_id.as_deref(),
                stored_agent_id.as_deref(),
                selected_requested_profile.as_deref(),
            );
            let (selected_agent_id, resolved_spawn) = match selection {
                MatchedSelection::NoAgent => (None, None),
                MatchedSelection::Unresolved { reference } => {
                    return Err(format!(
                    "unresolved_coding_agent_reference: '{}' matches no configured coding agent",
                    reference
                ));
                }
                MatchedSelection::Agent {
                    agent_id,
                    profile_letter,
                    authoritative,
                    tier,
                    original_letter,
                } => {
                    let spawn = resolve_configured_agent_spawn_for_cwd(
                        &cfg,
                        &agent_id,
                        &cwd,
                        profile_letter.as_deref(),
                        authoritative,
                    )?
                    .map(|mut spawn| {
                        // A matched restart records the ORIGINAL request; the matched
                        // letter lives in `effective_profile`, its provenance in the
                        // #2451 carriers.
                        if authoritative {
                            if let Some(original) = selected_requested_profile.as_ref() {
                                spawn.profile_resolution.requested_profile = original.clone();
                            }
                        }
                        spawn.match_tier = tier.map(|tier| tier.as_wire_str().to_string());
                        spawn.original_profile_letter = original_letter;
                        spawn
                    });
                    (Some(agent_id), spawn)
                }
            };
            let resolved_agent_host_shell = if resolved_spawn.is_some() {
                Some(ResolvedAgentHostShell {
                    program: cfg.default_shell.clone(),
                    args: cfg.default_shell_args.clone(),
                })
            } else {
                None
            };
            (selected_agent_id, resolved_spawn, resolved_agent_host_shell)
        };
        // #2434 - prepare (isolated CODEX_HOME, OpenCode config dir) outside the
        // settings guard and BEFORE any teardown: a failure aborts the restart and
        // leaves the old session running.
        if let Some(spawn) = resolved_spawn.as_ref() {
            crate::config::agent_command::prepare_agent_spawn_command(spawn)?;
        }
        let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
            (
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                Some(spawn.trusted_agent_label.clone()),
            )
        } else {
            (shell, clean_args, stored_agent_label)
        };

        // (#630/#631) Fresh on an explicit "Restart Session" (None -> true), OR if the
        // session still carries an unconsumed durable fresh intent. Root agents are
        // scoped out (separate restore path ignores the marker, §8): they are never
        // stamped below, so `stored_start_fresh` is false for root and this reduces to
        // today's `effective_restart_skip_auto_resume`.
        let restart_start_fresh =
            restart_skip_auto_resume_with_intent(stored_start_fresh, skip_auto_resume);
        let carried_communication = communication_override
            .or_else(|| carry_communication_for_restart(stored_communication, restart_start_fresh));

        Ok::<_, String>((
            shell,
            shell_args,
            cwd,
            selected_agent_id,
            agent_label,
            resolved_spawn,
            resolved_agent_host_shell,
            restart_start_fresh,
            carried_communication,
        ))
    }
    .await;
    if let Some(preparation) = preparation {
        preparation.complete(prepared.as_ref().map(|_| None).map_err(Clone::clone));
    }
    let (
        shell,
        shell_args,
        cwd,
        selected_agent_id,
        agent_label,
        resolved_spawn,
        resolved_agent_host_shell,
        restart_start_fresh,
        carried_communication,
    ) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            transaction.observe_lifecycle_preparation_failure();
            return Err(error);
        }
    };

    let retirement = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::Retirement);
    if transaction.has_lifecycle_entry() && retirement.is_none() {
        if let Some(authority) = context.authority() {
            authority
                .interrupted
                .store(true, std::sync::atomic::Ordering::Release);
        }
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    if let Some(authority) = context.authority() {
        if !retire_lifecycle_entry_protection(app, authority).await {
            transaction.invalidate_restore_ownership();
            authority
                .interrupted
                .store(true, std::sync::atomic::Ordering::Release);
            // External adapters suppress this internal exit via Interrupted.
            // Restore unwinds its actual executor/barrier with Unavailable.
            return Err(
                crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
            );
        }
    }

    // 3. Release the old runtime slot while retaining its canonical manager row.
    // Dormant rows skip teardown and remain retryable until replacement success.
    let old_was_dormant = matches!(old_status, SessionStatus::Exited(_));
    let teardown = teardown_old_for_restart(transaction, uuid, old_was_dormant).await;
    if let Some(retirement) = retirement {
        retirement.complete(match &teardown {
            Ok(old_lost) => Ok(Some(format!("old_lost={old_lost}"))),
            Err(error) => Err(format!("old_lost={}: {}", error.old_lost, error.message)),
        });
    }
    let old_lost =
        accept_restart_teardown(transaction, teardown, uuid, old_selected, intent).await?;

    // Test-only rendezvous after the old id's early release and before the
    // replacement is created (phase 4 test 9).
    #[cfg(test)]
    restart_reader_seam::hit_after_old_release(&uuid.to_string()).await;

    // 4. Spawn the replacement as a pending, unannounced row.
    let binding_manager = session_mgr.read().await.clone();
    let coding_agent_binding = binding_manager.coding_agent_binding(uuid).await;
    let target_ownership = context
        .authority()
        .map(|authority| authority.target_ownership())
        .transpose()?
        .flatten();
    let completion = create_session_inner_impl(
        app,
        &session_mgr,
        &pty_mgr,
        shell,
        shell_args,
        cwd.clone(),
        Some(name),
        selected_agent_id,
        agent_label,
        false,
        git_repos,
        restart_start_fresh,
        resolved_spawn,
        resolved_agent_host_shell,
        None,
        CreateSelectionIntent::Suppress,
        (!is_root_agent).then_some(restart_start_fresh),
        carried_communication,
        Some(transaction),
        Some(SelectionCause::Restart(intent)),
        true,
        target_ownership.as_ref(),
        enforcement,
        coding_agent_binding,
        context.authority(),
        None,
        None,
    )
    .await;
    let deferred = accept_restart_deferred_completion(
        transaction,
        completion,
        context,
        uuid,
        old_lost,
        old_selected,
        intent,
    )
    .await?;
    let (new_uuid, committed, session_info) = finalize_restart_deferred(
        transaction,
        &deferred,
        uuid,
        old_lost,
        old_selected,
        activate_after,
        intent,
        restart_start_fresh,
    )
    .await?;
    let publication = publish_restart_before_warnings(
        transaction,
        uuid,
        new_uuid,
        deferred.binding,
        &committed,
        &session_info,
        old_lost,
    )
    .await?;
    for warning in deferred.warnings {
        emit_session_warning(app, warning);
    }

    publish_restart_after_warnings(
        transaction,
        new_uuid,
        session_info,
        restart_start_fresh,
        telegram_bot_id.as_deref(),
        &cwd,
        publication,
    )
    .await
}

// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn restart_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    _tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    id: String,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
) -> Result<SessionInfo, String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    restart_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        settings.inner(),
        uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
    )
    .await
}

#[tauri::command]
pub async fn switch_session(
    app: AppHandle,
    _session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    detached: State<'_, DetachedSessionsState>,
    id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    // If this session is detached, focus its window instead of switching the main terminal
    let is_detached = {
        let detached_set = detached.lock().unwrap();
        detached_set.contains(&uuid)
    };
    if is_detached {
        let label = format!("terminal-{}", id.replace('-', ""));
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.set_focus();
        }
        return Ok(());
    }

    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    coordinator
        .transition(SelectionRequest::user_switch(uuid))
        .await?;

    Ok(())
}

#[tauri::command]
pub async fn rename_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    id: String,
    name: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    let mgr = session_mgr.read().await;
    mgr.rename_session(uuid, name.clone())
        .await
        .map_err(|e| e.to_string())?;

    // Persist after rename
    persist_current_state(&mgr).await;

    let _ = app.emit(
        "session_renamed",
        serde_json::json!({ "id": id, "name": name }),
    );

    Ok(())
}

#[tauri::command]
pub async fn list_sessions(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
) -> Result<Vec<SessionInfo>, String> {
    let mut infos = { session_mgr.read().await.list_sessions().await };
    // #592 - recompute drift per session against current settings (settings-aware,
    // unlike the `From<&Session>` path which always emits `profile_outdated=false`).
    let cfg = settings
        .current_read()
        .await
        .map_err(|error| error.to_string())?;
    for info in infos.iter_mut() {
        info.profile_outdated = compute_profile_outdated(&cfg, info);
    }
    Ok(infos)
}

#[tauri::command]
pub async fn set_last_prompt(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    id: String,
    text: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let mgr = session_mgr.read().await;
    mgr.set_last_prompt(uuid, text.clone()).await;
    crate::config::sessions_persistence::persist_current_state(&mgr).await;
    let _ = app.emit(
        "last_prompt",
        serde_json::json!({ "sessionId": id, "text": text }),
    );
    Ok(())
}

/// A reused Root is already public: serialize the absent-only seed and event
/// with every external prompt setter, then persist after releasing the permit.
pub(crate) async fn seed_root_last_prompt<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    id: Uuid,
    persisted: &crate::config::sessions_persistence::PersistedSession,
) {
    if persisted.start_fresh_on_restore {
        return;
    }
    let Some(prompt) = persisted.last_prompt.clone() else {
        return;
    };
    let manager = session_mgr.read().await;
    crate::config::sessions_persistence::initialize_prompt_retention(&manager).await;
    let Some(row) = manager.get_session(id).await else {
        return;
    };
    if !InitialLastPrompt::from_persisted(persisted).compatible(&row) {
        return;
    }
    let key = crate::config::sessions_persistence::prompt_key(
        &row.working_directory,
        row.agent_id.as_deref(),
        row.requested_profile.as_deref(),
        row.effective_profile.as_deref(),
    );
    let verified = crate::config::sessions_persistence::prompt_key(
        &row.working_directory,
        row.agent_id.as_deref(),
        row.requested_profile.as_deref(),
        row.effective_profile.as_deref(),
    );
    let Some(key) = key.filter(|key| Some(key) == verified.as_ref()) else {
        return;
    };
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let changed = manager
        .seed_if_absent_with_carrier_permit(&permit, id, &key, prompt.clone())
        .await;
    if changed {
        let _ = app.emit(
            "last_prompt",
            serde_json::json!({ "sessionId": id.to_string(), "text": prompt }),
        );
    }
    drop(permit);
    if changed {
        crate::config::sessions_persistence::persist_current_state(&manager).await;
    }
}

#[tauri::command]
pub async fn get_active_session(
    coordinator: State<'_, SelectionCoordinator>,
) -> Result<crate::session::selection::SessionSelection, String> {
    coordinator.snapshot().await
}

/// Create, wake, or reuse a root agent session.
#[tauri::command]
pub async fn create_root_agent_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    agent_id: Option<String>,
    requested_profile: Option<String>,
) -> Result<SessionInfo, String> {
    create_root_agent_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        tg_mgr.inner(),
        settings.inner(),
        agent_id,
        requested_profile,
        true, // skip_auto_resume = true → fresh create, no `--continue` injection
    )
    .await
}

/// #1646 / #1647 - Resolves a blocking menu episode for a session.
/// Clears the blocked menu in SessionManager and publishes the updated communication state.
/// #1669 - the clear is persisted through the same atomic helper `scan_tick` uses,
/// so a disk-reading CLI stops reporting `blockedMenu` as soon as the user
/// resolves the menu. The scan loop cannot do this: `resolve_current_episode`
/// suppresses the episode, and the suppressed arm of `evaluate_logical_rows`
/// returns BOTH `should_notify: false` and `should_clear_notification: false`.
#[tauri::command]
pub async fn resolve_blocking_menu<R: Runtime>(
    app: AppHandle<R>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    menu_guard: State<'_, Arc<crate::pty::menu_guard::MenuGuard>>,
    id: String,
) -> Result<(), String> {
    let session_id = Uuid::parse_str(&id).map_err(|e| format!("Invalid session ID: {e}"))?;
    menu_guard.resolve_current_episode(session_id);
    let mgr = { session_mgr.read().await.clone() };
    crate::config::sessions_persistence::clear_blocked_menu_and_persist(&mgr, session_id).await;
    crate::session::selection::publish_session_communication(&app, session_id, None);
    Ok(())
}

/// #1682 - the stamp for the session's replica, for a terminal that just mounted
/// and missed the `session_agent_message` event. `None` covers every unavailable
/// case: unknown session, plain shell with no `agent_id`, missing or unparseable
/// config, missing key.
#[tauri::command]
pub async fn get_last_agent_message(
    session_mgr: tauri::State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
) -> Result<Option<String>, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let Some(session) = session_mgr.read().await.get_session(uuid).await else {
        return Ok(None);
    };
    if session.agent_id.is_none() {
        return Ok(None);
    }
    Ok(agent_config::read_last_agent_message_at(
        &session.working_directory,
    ))
}

#[cfg(test)]
mod tests;

/// #2232 phase 4: the reader supervisor, its demands and the production wiring.
///
/// These tests drive the same SCC-member entry points production uses —
/// `raise_room_reader_demand_in`, `release_room_reader_demand` and the
/// supervisor in `commands::telegram` — over the phase-2 room fixture.
#[cfg(test)]
pub(crate) mod reader_demand_tests;

/// #2265 phase 2, test 15: the gathering function answers from an SCC member.
///
/// A later refactor that moves the body back into the Co-managed command module breaks
/// these tests rather than only the module-cycle gate.
#[cfg(test)]
pub(crate) mod co_managed_tests;
