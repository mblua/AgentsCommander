use crate::commands::session::agent_command::mirror_forces_fresh;
use crate::commands::session::agent_command::resolve_actual_agent;
#[cfg(test)]
use crate::commands::session::coordinator_create_seam;
use crate::commands::session::create::admission::acquire_lifecycle_create_publication_region;
use crate::commands::session::create::admission::enforce_owned_unarchived_for_spawn;
use crate::commands::session::create::admission::note_coordinator_create_on_app;
use crate::commands::session::create::admission::release_resource_launch_permit;
use crate::commands::session::create::admission::with_create_resource_region;
use crate::commands::session::create::admission::CoordinatorCreateTicket;
use crate::commands::session::create::admission::CreateCapacityContext;
use crate::commands::session::create::admission::CreateResourceAdmission;
use crate::commands::session::create::admission::OwnedCreateAdmission;
use crate::commands::session::create::contracts::CreateBodyContext;
use crate::commands::session::create::contracts::CreateCompletion;
use crate::commands::session::create::contracts::CreateSelectionIntent;
use crate::commands::session::create::contracts::InitialLastPrompt;
use crate::commands::session::create::contracts::PendingLaunch;
use crate::commands::session::create::publication::acquire_create_target_lease;
use crate::commands::session::create::publication::finish_create_core_result;
use crate::commands::session::create::publication::resolve_create_destination;
use crate::commands::session::create::publication::rollback_pre_created_session;
use crate::commands::session::lifecycle::LifecycleEntryAuthority;
use crate::commands::session::resume_probe::configured_prompt_identity;
use crate::commands::session::resume_probe::container_path_context_for_cwd;
use crate::commands::session::resume_probe::effective_codex_home_for_backend;
#[cfg(test)]
use crate::commands::session::seed_race_barriers;
use crate::config::agent_command::AgentSpawnCommand;
use crate::config::settings::SettingsState;
use crate::pty::backend::PtyViewport;
use crate::pty::backend::ResolvedAgentHostShell;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::resource_monitor::ResourceLimits;
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::session::selection::SelectionCause;
use crate::session::selection::SelectionCoordinator;
use crate::session::selection::SelectionTransaction;
use crate::session::session::SessionCommunication;
use crate::session::session::SessionRepo;
use crate::session::session::SessionStatus;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Manager;
use uuid::Uuid;

mod backend;
mod context;
mod publication;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_session_inner_core<R: tauri::Runtime>(
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
    let ctx = CreateBodyContext {
        app,
        session_mgr,
        pty_mgr,
        shell: &shell,
        cwd: &cwd,
        resolved_spawn: resolved_spawn.as_ref(),
        resolved_agent_host_shell: resolved_agent_host_shell.as_ref(),
        viewport,
        skip_tooling_save,
        inline_transaction,
        inline_cause,
        defer_inline_finalization,
        pending_start_fresh,
        initial_last_prompt: initial_last_prompt.as_ref(),
        owned_replacement: owned_replacement.as_ref(),
        lifecycle_domain,
        create_owner: &create_owner,
        create_manager: &create_manager,
        coordinator: coordinator.inner(),
        create_ticket: &create_ticket,
        marker_ticket: &marker_ticket,
        inline_pending_for_body: inline_pending_for_body.as_ref(),
    };
    let &CreateBodyContext { create_manager, .. } = &ctx;
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
            create_manager,
            lifecycle_domain,
            inline_transaction,
        )
        .await?;
        // An unarchived path has no native activation/admission callback.
        // It still reserves healthy ownership before the first create effect.
        let initial_admission = acquire_lifecycle_create_publication_region(
            &create_owner,
            create_manager,
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
                create_manager,
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

        let input = PendingLaunch {
            agent_id,
            agent_label,
            agent_kind,
            is_coordinator,
            is_root_agent,
            fresh_forced_by_mirror,
            skip_auto_resume,
            pending_binding,
            shell_args,
            container_path_context,
            container_repos,
            context_target,
            full_cmd,
            configured_identity,
            session,
            spawn_mark,
            mgr,
            resource_permit,
            resource_monitor,
        };
        context::prepare(&ctx, input).await
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
