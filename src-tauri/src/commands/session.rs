use crate::app_state::DetachedSessionsState;
use crate::config::agent_config;
use crate::config::coordinator_clocks::CoordinatorClocksState;
use crate::config::sessions_persistence::persist_current_state;
#[cfg(test)]
use crate::config::settings::AppSettings;
use crate::config::settings::SettingsState;
#[cfg(test)]
use crate::pty::backend::SessionBackendKind;
use crate::pty::backend::{PtyViewport, ResolvedAgentHostShell};
use crate::pty::manager::PtyManager;
#[cfg(test)]
use crate::resource_monitor::ResourceLimits;
use crate::session::manager::SessionManager;
#[cfg(test)]
use crate::session::profile::CodingAgentKind;
#[cfg(test)]
use crate::session::selection::TrustedRestartIntent;
use crate::session::selection::{
    SelectionCause, SelectionCoordinator, SelectionRequest, SelectionTransaction,
};
use crate::session::session::{SessionInfo, SessionRepo, SessionStatus};
use crate::session::warnings::emit_session_warning;
use crate::telegram::manager::TelegramBridgeState;
pub(crate) use resume_args::trusted_muse_auto_resume_spawn;
#[cfg(test)]
use resume_args::{
    inject_antigravity_resume, inject_codex_resume, maybe_inject_muse_resume,
    maybe_inject_pi_resume,
};
#[cfg(test)]
use resume_args::{
    inject_pi_resume, pi_has_explicit_session_control, pi_is_non_conversation_invocation,
};
#[cfg(test)]
use resume_probe::{claude_projects_dir_for_config_dir, resume_probe_target_for_config_dir};
#[cfg(test)]
use resume_probe::{
    claude_resume_probe_target_for_kind, container_path_context_for_cwd,
    injected_claude_config_dir_for_copy, should_inject_continue, should_inject_fresh_session_id,
};
pub(crate) use resume_probe::{resolve_claude_projects_dir, transcript_id_from_args};
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
pub(crate) mod selection_executor;
mod telegram;
#[cfg(test)]
use agent_command::coding_agent_descriptor;
pub(crate) use agent_command::executable_basename;
#[cfg(test)]
use agent_command::prepare_last_coding_agent_write;
#[cfg(test)]
use agent_command::resolve_actual_agent;
#[cfg(test)]
use agent_command::resolve_agent_command;
#[cfg(test)]
use agent_command::resolve_agent_from_shell;
pub(crate) use agent_command::resolve_root_agent_command;
#[cfg(test)]
use agent_command::save_last_coding_agent;
pub(crate) use co_managed::co_managed_capturable_for_session;
pub(crate) use co_managed::co_managed_effective_state_for_session;
pub(crate) use co_managed::raise_room_reader_demand;
pub(crate) use co_managed::raise_room_reader_demand_in;
pub(crate) use co_managed::release_room_reader_demand;
#[cfg(test)]
use create::acquire_create_publication_region;
pub(crate) use create::build_configured_agent_spawn_for_cwd;
use create::compute_profile_outdated;
pub(crate) use create::create_owned_response_replacement;
#[cfg(test)]
use create::create_owned_response_replacement_in_context;
pub use create::create_session_inner;
pub(crate) use create::create_session_inner_for_restore;
use create::create_session_inner_impl;
pub(crate) use create::create_session_inner_with_pty_target_ownership;
#[cfg(test)]
pub(crate) use create::note_coordinator_create_on_app;
pub(crate) use create::purge_session_side_state;
#[cfg(test)]
pub(crate) use create::register_session_samplers;
use create::reset_substantive_input;
pub(crate) use create::resolve_configured_agent_spawn_for_cwd;
pub(crate) use create::resolve_create_session_cwd;
#[cfg(test)]
use create::resolve_launch_auto_self_clear;
#[cfg(test)]
use create::resolve_launch_idle_tuning;
#[cfg(test)]
use create::try_acquire_create_publication_region;
#[cfg(test)]
use create::with_create_resource_region;
#[cfg(test)]
use create::CreateResourceAdmission;
pub use create::CreateSelectionIntent;
use create::InitialLastPrompt;
#[cfg(test)]
use create::OwnedCreateAdmission;
pub(crate) use create::PtyTargetCreateStep;
pub(crate) use create::{OwnedReplacementCreateRequest, OwnedReplacementCreateStep};
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
pub use destroy::CoordinatorCloseOutcome;
pub(crate) use destroy::DestroyOutcome;
pub(crate) use destroy::DestroyRequest;
pub use destroy::DestructionSource;
#[cfg(test)]
pub(crate) use lifecycle::prepare_lifecycle_entry_authority;
use lifecycle::prepare_restore_lifecycle_authority;
use lifecycle::retire_lifecycle_entry_protection;
#[cfg(test)]
pub(crate) use lifecycle::LifecycleAuthorityAdmission;
pub(crate) use lifecycle::LifecycleEntryAuthority;
use restart::accept_restart_deferred_completion;
use restart::accept_restart_teardown;
pub(crate) use restart::carry_communication_for_restart;
use restart::context::ensure_lifecycle_root_directory;
use restart::effective_create_skip_auto_resume;
use restart::effective_restart_requested_profile;
#[cfg(test)]
use restart::effective_restart_skip_auto_resume;
use restart::finalize_restart_deferred;
pub(crate) use restart::matched_selection;
#[cfg(test)]
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

async fn publish_legacy_destroy_outcome<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    source: &DestructionSource,
    outcome: &DestroyOutcome,
    committed: &crate::session::manager::CommitResult,
    retained_marked: &std::collections::HashSet<uuid::Uuid>,
) {
    for session_id in outcome
        .destroyed_ids
        .iter()
        .chain(outcome.retained_exited_ids.iter())
        .copied()
    {
        transaction.publish_destroyed(session_id);
        let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
        if let Some(window) = transaction.app().get_webview_window(&label) {
            if let Err(error) = window.destroy() {
                log::warn!(
                    "[session] detached window destroy failed session={} source={:?}: {}",
                    session_id,
                    source,
                    error
                );
            }
        }
        reset_substantive_input(transaction.app(), session_id);
    }

    // #1171 - a SEPARATE loop over `destroyed_ids` only, rather than a membership test inside
    // the loop above. That loop iterates `destroyed_ids.chain(retained_exited_ids)`, so a
    // `contains` check would be O(n squared) and, worse, would express a business rule as a
    // condition inside a loop that does something else. A separate loop makes "destroyed only"
    // structural.
    //
    // Root-agent sessions retained as `Exited` are deliberately NOT purged: they stay in the
    // manager and their row is still in the list, so their post-mortem view still has a place
    // to be shown.
    for session_id in outcome.destroyed_ids.iter().copied() {
        purge_session_side_state(transaction.app(), session_id);
        // #2232 phase 4 section 5: destroy releases **both** demands, which
        // cancels the reader and closes the session's `CaptureRegistry` entry.
        crate::telegram::manager::release_all_reader_demands(transaction.app(), session_id).await;
    }

    for row in committed.changed_rows.iter().filter(|row| {
        Uuid::parse_str(&row.id)
            .ok()
            .is_some_and(|id| retained_marked.contains(&id))
    }) {
        transaction.publish_created(row);
    }
    for session_id in &committed.cleared_raise_hand_ids {
        transaction.publish_communication_cleared(*session_id);
    }
    if let Some(selection) = committed.selection.as_ref() {
        transaction.publish_selection(selection);
    }
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
