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
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Manager;
use uuid::Uuid;

mod backend;
mod context;
mod creation;
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
    publication::await_startup(app, session_mgr, inline_transaction, lifecycle_domain).await?;
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
    let create_body = create_body(
        &ctx,
        creation::CreateInput {
            session_label,
            shell_args,
            session_name,
            agent_id,
            agent_label,
            git_repos,
            skip_auto_resume,
            pending_communication,
            enforcement,
            coding_agent_binding,
            selection_intent,
            create_target_replica_identity,
        },
    );
    // This accepted core owns every launched effect and its real result.
    // Coordinator shutdown may reject finalization, but cannot drop a pending
    // backend/native effect and manufacture completion for the caller.
    let result = Box::pin(create_body).await;
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

pub(crate) async fn create_body<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    input: CreateInput,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        app,
        shell,
        cwd,
        resolved_spawn,
        inline_transaction,
        lifecycle_domain,
        create_owner,
        create_manager,
        ..
    } = ctx;
    let CreateInput {
        session_label,
        shell_args,
        session_name,
        agent_id,
        agent_label,
        git_repos,
        skip_auto_resume,
        pending_communication,
        enforcement,
        coding_agent_binding,
        selection_intent,
        create_target_replica_identity,
    } = input;
    // §1295 5.1a creation gate: FIRST statement of create_body, BEFORE the
    // existing `enforce_unarchived_for_spawn` (which the archive-gate source
    // test :7438 counts separately). Reading the SAME normalized `cwd` string
    // the archive gate receives; no resource permit and no pending row exist
    // yet, so a rejection leaves zero RAM/disk residue and needs no
    // `rollback_pre_created_session`. (dev-rust E: the two call strings
    // at :1443/:1708 are untouched.)
    if matches!(
        enforcement,
        crate::config::sessions_persistence::CreationGateEnforcement::Enforce
    ) {
        let settings = app.state::<SettingsState>();
        crate::config::sessions_persistence::enforce_creation_gate(
            settings.inner(),
            cwd,
            enforcement,
        )
        .await?;
    }
    enforce_owned_unarchived_for_spawn(
        app,
        cwd,
        &session_label,
        create_owner,
        create_manager,
        lifecycle_domain,
        inline_transaction,
    )
    .await?;
    // An unarchived path has no native activation/admission callback.
    // It still reserves healthy ownership before the first create effect.
    let initial_admission = acquire_lifecycle_create_publication_region(
        create_owner,
        create_manager,
        lifecycle_domain,
        inline_transaction,
    )
    .await?;
    drop(initial_admission);
    let (agent_id, agent_label) = {
        if let Some(spawn) = resolved_spawn {
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
                shell,
                &shell_args,
                agent_id.as_deref(),
                agent_label.as_deref(),
                &cfg,
            )
        }
    };

    reserve_launch(
        ctx,
        CreateInput {
            session_label,
            shell_args,
            session_name,
            agent_id,
            agent_label,
            git_repos,
            skip_auto_resume,
            pending_communication,
            enforcement,
            coding_agent_binding,
            selection_intent,
            create_target_replica_identity,
        },
    )
    .await
}

pub(crate) async fn reserve_launch<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    input: CreateInput,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        app,
        session_mgr,
        shell,
        cwd,
        resolved_spawn,
        create_owner,
        marker_ticket,
        ..
    } = ctx;
    let CreateInput {
        session_label,
        shell_args,
        session_name,
        agent_id,
        agent_label,
        git_repos,
        skip_auto_resume,
        pending_communication,
        enforcement,
        coding_agent_binding,
        selection_intent,
        create_target_replica_identity,
    } = input;
    // Recompute is_coordinator from the current team snapshot. One source of truth:
    // every caller of create_session_inner gets the same computation.
    let teams = tokio::task::spawn_blocking(crate::config::teams::discover_teams)
        .await
        .map_err(|e| e.to_string())?;
    let is_coordinator = crate::config::teams::is_coordinator_for_cwd(cwd, &teams);
    #[cfg(test)]
    let is_coordinator = is_coordinator || coordinator_create_seam::forced(cwd);
    let is_root_agent = crate::config::root_agent::is_root_agent_path(cwd);
    #[cfg(test)]
    let is_root_agent = is_root_agent || coordinator_create_seam::forced_root(cwd);

    if is_coordinator {
        *marker_ticket.lock().unwrap_or_else(|e| e.into_inner()) =
            note_coordinator_create_on_app(app, cwd);
    }

    let (skip_auto_resume, fresh_forced_by_mirror) =
        publication::mirror_fresh_intent(app, cwd, is_coordinator, skip_auto_resume);

    let agent_kind = CodingAgentKind::detect(shell, &shell_args);
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
        match container_path_context_for_cwd(cwd) {
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
    let input = CreateInput {
        session_label,
        shell_args,
        session_name,
        agent_id,
        agent_label,
        git_repos,
        skip_auto_resume,
        pending_communication,
        enforcement,
        coding_agent_binding,
        selection_intent,
        create_target_replica_identity,
    };
    let state = LaunchState {
        is_coordinator,
        is_root_agent,
        fresh_forced_by_mirror,
        agent_kind,
        resource_monitor,
        resource_permit,
        mgr,
        backend_kind,
        container_path_context,
        container_repos,
    };
    create_pending(ctx, input, state).await
}

pub(crate) async fn create_pending<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    input: CreateInput,
    state: LaunchState,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        pty_mgr,
        shell,
        cwd,
        inline_transaction,
        owned_replacement,
        lifecycle_domain,
        create_owner,
        inline_pending_for_body,
        ..
    } = ctx;
    let CreateInput {
        session_label,
        shell_args,
        session_name,
        agent_id,
        agent_label,
        git_repos,
        mut skip_auto_resume,
        pending_communication,
        enforcement,
        coding_agent_binding,
        selection_intent,
        create_target_replica_identity,
    } = input;
    let LaunchState {
        is_coordinator,
        is_root_agent,
        fresh_forced_by_mirror,
        agent_kind,
        resource_monitor,
        mut resource_permit,
        mgr,
        backend_kind,
        container_path_context,
        container_repos,
    } = state;
    // Attach the B capability already retained before create effects to
    // this ticket/inline owner. No binder, default insert, or queue
    // admission may mint another binding in the publication region.
    let prepared_binding = create_owner.lock().ok().map(|owner| owner.binding);
    let Some(prepared_binding) = prepared_binding else {
        return Ok(CreateCompletion::Retained);
    };
    let carrier_u0 = match with_create_resource_region(
        create_owner,
        &mut resource_permit,
        acquire_lifecycle_create_publication_region(
            create_owner,
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
    let carrier_u0 = match context::bind_and_check_target(
        ctx,
        &create_target_replica_identity,
        selection_intent,
        &mgr,
        &resource_monitor,
        &mut resource_permit,
        prepared_binding,
        carrier_u0,
    )
    .await?
    {
        Some(permit) => permit,
        None => return Ok(CreateCompletion::Retained),
    };
    let spawn_mark = {
        let pty = pty_mgr.lock().unwrap_or_else(|error| error.into_inner());
        pty.mark_spawning(cwd, &session_label)
    };
    let pending_result = mgr
        .create_pending_session_with_binding_and_carrier_permit(
            prepared_binding,
            &carrier_u0,
            shell.to_owned(),
            shell_args.clone(),
            cwd.to_owned(),
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
    if !context::pending_is_observed(ctx, pending_binding) {
        drop(carrier_u0);
        release_resource_launch_permit(&resource_monitor, &mut resource_permit);
        return Ok(CreateCompletion::Retained);
    }
    let previous = match owned_replacement {
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
    context::apply_pending_options(
        ctx,
        &mgr,
        &carrier_u0,
        pending_binding,
        &mut session,
        pending_communication.as_ref(),
    )
    .await?;

    drop(carrier_u0);
    let input = MetadataInput {
        session_label,
        shell_args,
        session_name,
        agent_id,
        agent_label,
        skip_auto_resume,
        pending_communication,
        enforcement,
        selection_intent,
        create_target_replica_identity,
    };
    let state = PendingState {
        is_coordinator,
        is_root_agent,
        fresh_forced_by_mirror,
        agent_kind,
        resource_monitor,
        resource_permit,
        mgr,
        container_path_context,
        container_repos,
        session,
        pending_binding,
        spawn_mark,
    };
    prepare_metadata(ctx, input, state).await
}

pub(crate) async fn prepare_metadata<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    input: MetadataInput,
    state: PendingState,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        app,
        session_mgr,
        pty_mgr,
        shell,
        cwd,
        resolved_spawn,
        inline_transaction,
        owned_replacement,
        lifecycle_domain,
        create_owner,
        create_manager,
        ..
    } = ctx;
    let MetadataInput {
        session_label,
        shell_args,
        session_name,
        agent_id,
        agent_label,
        skip_auto_resume,
        pending_communication: _pending_communication,
        enforcement: _enforcement,
        selection_intent: _selection_intent,
        create_target_replica_identity: _create_target_replica_identity,
    } = input;
    let PendingState {
        is_coordinator,
        is_root_agent,
        fresh_forced_by_mirror,
        agent_kind,
        resource_monitor,
        mut resource_permit,
        mgr,
        container_path_context,
        container_repos,
        mut session,
        pending_binding,
        spawn_mark,
    } = state;

    // #1063 test-only barrier: the pending row now exists and is visible to a
    // deletion's pending-inclusive snapshot; pause here before any config-seed
    // project gate so a delete can interleave deterministically. No-op in production.
    #[cfg(test)]
    {
        seed_race_barriers::hit_before_project_gate(cwd).await;
    }

    if let Err(e) = with_create_resource_region(
        create_owner,
        &mut resource_permit,
        enforce_owned_unarchived_for_spawn(
            app,
            cwd,
            &session_label,
            create_owner,
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
        create_owner,
        &mut resource_permit,
        acquire_lifecycle_create_publication_region(
            create_owner,
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
            .rename_pending_session_with_carrier_permit(&carrier_u1, pending_binding, name.clone())
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

    context::apply_profile_metadata(
        ctx,
        &mgr,
        &carrier_u1,
        pending_binding,
        &mut session,
        container_path_context.as_ref(),
    )
    .await?;

    let configured_identity = configured_prompt_identity(agent_kind, shell, &shell_args);
    let full_cmd = format!("{} {}", shell, shell_args.join(" "));
    // #260: single detector (session/profile.rs). Replaces the old
    // starts_with triple; this is the SAME call `strip_auto_injected_args`
    // makes, so the persisted recipe and the runtime identity cannot disagree.
    let context_target = creation::context_target(agent_kind);

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
    if let Some(continuation) = owned_replacement {
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
}

use creation::CreateInput;
use creation::LaunchState;
use creation::MetadataInput;
use creation::PendingState;
