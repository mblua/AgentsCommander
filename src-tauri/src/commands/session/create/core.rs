use crate::commands::session::agent_command::resolve_actual_agent;
#[cfg(test)]
use crate::commands::session::coordinator_create_seam;
use crate::commands::session::create::admission::acquire_lifecycle_create_publication_region;
use crate::commands::session::create::admission::enforce_owned_unarchived_for_spawn;
use crate::commands::session::create::admission::note_coordinator_create_on_app;
use crate::commands::session::create::admission::release_resource_launch_permit;
use crate::commands::session::create::admission::with_create_resource_region;
use crate::commands::session::create::admission::CoordinatorCreateTicket;
use crate::commands::session::create::admission::CreateResourceAdmission;
use crate::commands::session::create::contracts::CreateBodyContext;
use crate::commands::session::create::contracts::CreateCompletion;
use crate::commands::session::create::contracts::CreateSelectionIntent;
use crate::commands::session::create::contracts::InitialLastPrompt;
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
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Manager;

mod backend;
mod context;
mod creation;
mod publication;

// (#2413) Close-marker ticket of this create; the body fills it, the outer
// scope ends it after the select.
// This accepted core owns every launched effect and its real result.
// Coordinator shutdown may reject finalization, but cannot drop a pending
// backend/native effect and manufacture completion for the caller.
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
    let Some(admission) = destination_admission(
        app,
        session_mgr,
        pty_mgr,
        &cwd,
        inline_transaction,
        lifecycle_domain,
        (preheld_target, owned_replacement.as_ref()),
    )
    .await?
    else {
        return Ok(CreateCompletion::Retained);
    };
    let session_label = session_name.as_deref().unwrap_or(&shell).to_string();
    let coordinator = selection_coordinator(app)?;
    let create_ticket = creation::reserve_create_ticket(
        coordinator.inner(),
        inline_transaction,
        selection_intent,
        owned_replacement.as_ref(),
        &admission.create_owner,
    )
    .await?;
    let inline_pending_binding = Arc::new(Mutex::new(None));
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
        create_owner: &admission.create_owner,
        create_manager: &admission.create_manager,
        coordinator: coordinator.inner(),
        create_ticket: &create_ticket,
        marker_ticket: &marker_ticket,
        inline_pending_for_body: inline_pending_binding.as_ref(),
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
            create_target_replica_identity: admission.create_target_replica_identity,
        },
    );
    let result = Box::pin(create_body).await;
    finish_body(&ctx, result, preheld_target.is_some()).await
}

pub(crate) async fn create_body<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    input: CreateInput,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        app,
        cwd,
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
    let (agent_id, agent_label) = resolve_launch_agent(
        ctx,
        &shell_args,
        agent_id.as_deref(),
        agent_label.as_deref(),
    )
    .await?;
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
    mut input: CreateInput,
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
        publication::mirror_fresh_intent(app, cwd, is_coordinator, input.skip_auto_resume);
    let agent_kind = CodingAgentKind::detect(shell, &input.shell_args);
    let is_agent_owned_launch = input.agent_id.is_some() || agent_kind.is_some() || is_root_agent;
    let coding_agent_binding = creation::resolve_coding_agent_binding(
        app,
        input.coding_agent_binding.clone(),
        input.agent_id.as_ref(),
    )
    .await?;
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
    let (container_path_context, container_repos) = resolve_container_launch(
        cwd,
        is_root_agent,
        backend_kind,
        &resource_monitor,
        &mut resource_permit,
    )?;
    input.skip_auto_resume = skip_auto_resume;
    input.coding_agent_binding = coding_agent_binding;
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
    mut input: CreateInput,
    mut state: LaunchState,
) -> Result<CreateCompletion, String> {
    let (pty_mgr, shell, cwd) = (ctx.pty_mgr, ctx.shell, ctx.cwd);
    let create_owner = ctx.create_owner;
    let resource_monitor = &state.resource_monitor;
    let resource_permit = &mut state.resource_permit;
    let mgr = &state.mgr;
    // Reuse retained B capability; publication must not mint another binding.
    let prepared_binding = create_owner.lock().ok().map(|owner| owner.binding);
    let Some(prepared_binding) = prepared_binding else {
        return Ok(CreateCompletion::Retained);
    };
    let carrier_u0 = pending_carrier(ctx, mgr, resource_monitor, resource_permit).await?;
    let carrier_u0 = match context::bind_and_check_target(
        ctx,
        context::BindTargetInput {
            target: &input.create_target_replica_identity,
            selection_intent: input.selection_intent,
            mgr,
            resource_monitor,
            resource_permit,
            prepared_binding,
            carrier_u0,
        },
    )
    .await?
    {
        Some(permit) => permit,
        None => return Ok(CreateCompletion::Retained),
    };
    let spawn_mark = {
        let pty = pty_mgr.lock().unwrap_or_else(|error| error.into_inner());
        pty.mark_spawning(cwd, &input.session_label)
    };
    let pending_result = mgr
        .create_pending_session_with_binding_and_carrier_permit(
            prepared_binding,
            &carrier_u0,
            shell.to_owned(),
            input.shell_args.clone(),
            cwd.to_owned(),
            input.agent_id.clone(),
            input.agent_label.clone(),
            std::mem::take(&mut input.git_repos),
            state.is_coordinator,
            state.backend_kind,
        )
        .await
        .map(|session| (session, prepared_binding))
        .map_err(|error| error.to_string());
    let (mut session, pending_binding) = match pending_result {
        Ok(created) => created,
        Err(e) => {
            drop(carrier_u0);
            release_resource_launch_permit(resource_monitor, resource_permit);
            return Err(e);
        }
    };
    if !context::pending_is_observed(ctx, pending_binding) {
        drop(carrier_u0);
        release_resource_launch_permit(resource_monitor, resource_permit);
        return Ok(CreateCompletion::Retained);
    }
    let carrier_u0 = match creation::apply_pending_binding(
        ctx,
        mgr,
        carrier_u0,
        pending_binding,
        &mut session,
        &mut input.skip_auto_resume,
        input.coding_agent_binding.take(),
    )
    .await
    {
        Ok(Some(permit)) => permit,
        result => {
            release_resource_launch_permit(resource_monitor, resource_permit);
            return result.map(|_| CreateCompletion::Retained);
        }
    };
    note_inline_pending(ctx, pending_binding);
    context::apply_pending_options(
        ctx,
        mgr,
        &carrier_u0,
        pending_binding,
        &mut session,
        input.pending_communication.as_ref(),
    )
    .await?;
    drop(carrier_u0);
    let input = creation::metadata_input(input);
    let state = creation::pending_state(state, session, pending_binding, spawn_mark);
    prepare_metadata(ctx, input, state).await
}

pub(crate) async fn prepare_metadata<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    input: MetadataInput,
    state: PendingState,
) -> Result<CreateCompletion, String> {
    let (input, mut state) = recheck_pending_archive(ctx, input, state).await?;
    let mgr = &state.mgr;
    let resource_monitor = &state.resource_monitor;
    let resource_permit = &mut state.resource_permit;
    let session = &mut state.session;
    let pending_binding = state.pending_binding;
    let carrier_u1 = match with_create_resource_region(
        ctx.create_owner,
        resource_permit,
        acquire_lifecycle_create_publication_region(
            ctx.create_owner,
            mgr,
            ctx.lifecycle_domain,
            ctx.inline_transaction,
        ),
    )
    .await
    {
        Ok(permit) => permit,
        Err(error) => {
            release_resource_launch_permit(resource_monitor, resource_permit);
            return Err(error);
        }
    };
    creation::record_pending_designation(
        mgr,
        &carrier_u1,
        pending_binding,
        session,
        state.fresh_forced_by_mirror,
        state.is_root_agent,
    )
    .await?;
    if let Some(name) = input.session_name.as_ref() {
        if let Err(e) = mgr
            .rename_pending_session_with_carrier_permit(&carrier_u1, pending_binding, name.clone())
            .await
        {
            let err = e.to_string();
            drop(carrier_u1);
            release_resource_launch_permit(resource_monitor, resource_permit);
            drop(state.mgr);
            rollback_pre_created_session(ctx.app, ctx.session_mgr, ctx.pty_mgr, session.id, &err)
                .await;
            return Err(err);
        }
        session.name = name.clone();
    }
    context::apply_profile_metadata(
        ctx,
        mgr,
        &carrier_u1,
        pending_binding,
        session,
        state.container_path_context.as_ref(),
    )
    .await?;
    let configured_identity =
        configured_prompt_identity(state.agent_kind, ctx.shell, &input.shell_args);
    let full_cmd = format!("{} {}", ctx.shell, input.shell_args.join(" "));
    let context_target = creation::context_target(state.agent_kind);
    // Single source of truth: store the identity on the SessionManager record
    // AND the local clone (the latter feeds the imminent `session_created` emit).
    mgr.set_pending_agent_kind_with_carrier_permit(&carrier_u1, pending_binding, state.agent_kind)
        .await
        .map_err(|error| error.to_string())?;
    session.agent_kind = state.agent_kind;
    creation::record_configured_spawn(
        mgr,
        &carrier_u1,
        pending_binding,
        session,
        ctx.resolved_spawn.is_some(),
    )
    .await?;
    drop(carrier_u1);
    if let Some(continuation) = ctx.owned_replacement {
        if !continuation.validate_source().await {
            release_resource_launch_permit(resource_monitor, resource_permit);
            return Ok(CreateCompletion::Retained);
        }
    }
    let input =
        creation::pending_launch(input, state, context_target, full_cmd, configured_identity);
    context::prepare(ctx, input).await
}

use creation::CreateInput;
use creation::LaunchState;
use creation::MetadataInput;
use creation::PendingState;

async fn recheck_pending_archive<R: tauri::Runtime>(
    ctx: &crate::commands::session::create::contracts::CreateBodyContext<'_, R>,
    input: MetadataInput,
    mut state: PendingState,
) -> Result<(MetadataInput, PendingState), String> {
    let session_label = input.session_label;
    let crate::commands::session::create::contracts::CreateBodyContext {
        app,
        session_mgr,
        pty_mgr,
        cwd,
        create_owner,
        create_manager,
        lifecycle_domain,
        inline_transaction,
        ..
    } = *ctx;
    let resource_permit = &mut state.resource_permit;
    // #1063 test-only barrier: the pending row now exists and is visible to a
    // deletion's pending-inclusive snapshot; pause here before any config-seed
    // project gate so a delete can interleave deterministically. No-op in production.
    #[cfg(test)]
    {
        seed_race_barriers::hit_before_project_gate(cwd).await;
    }

    if let Err(e) = with_create_resource_region(
        create_owner,
        resource_permit,
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
        release_resource_launch_permit(&state.resource_monitor, resource_permit);
        drop(state.mgr);
        rollback_pre_created_session(app, session_mgr, pty_mgr, state.session.id, &err).await;
        return Err(err);
    }

    Ok((
        MetadataInput {
            session_label,
            ..input
        },
        state,
    ))
}

struct DestinationAdmission {
    create_owner: Arc<Mutex<crate::commands::session::create::admission::OwnedCreateAdmission>>,
    create_manager: SessionManager,
    create_target_replica_identity: Option<crate::path_identity::VerifiedPathIdentity>,
}

async fn finish_body<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    result: Result<CreateCompletion, String>,
    preheld_target: bool,
) -> Result<CreateCompletion, String> {
    let marker = creation::take_marker_ticket(ctx.marker_ticket);
    finish_create_core_result(
        marker,
        result,
        ctx.owned_replacement,
        ctx.create_owner,
        ctx.session_mgr,
        ctx.pty_mgr,
        preheld_target,
        ctx.inline_pending_for_body,
        ctx.inline_transaction,
    )
    .await
}

async fn resolve_launch_agent<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    shell_args: &[String],
    agent_id: Option<&str>,
    agent_label: Option<&str>,
) -> Result<(Option<String>, Option<String>), String> {
    Ok({
        if let Some(spawn) = ctx.resolved_spawn {
            (
                Some(spawn.trusted_agent_id.clone()),
                Some(spawn.trusted_agent_label.clone()),
            )
        } else {
            let settings_state = ctx.app.state::<SettingsState>();
            let cfg = settings_state
                .current_read()
                .await
                .map_err(|error| error.to_string())?;
            resolve_actual_agent(ctx.shell, shell_args, agent_id, agent_label, &cfg)
        }
    })
}

fn resolve_container_launch(
    cwd: &str,
    is_root_agent: bool,
    backend_kind: SessionBackendKind,
    resource_monitor: &Arc<ResourceMonitorState>,
    resource_permit: &mut Option<crate::resource_monitor::AgentLaunchPermit>,
) -> Result<
    (
        Option<crate::commands::session::resume_probe::ContainerPathContext>,
        Option<crate::pty::container_repos::RepoMountResolution>,
    ),
    String,
> {
    if is_root_agent && backend_kind == SessionBackendKind::ContainerTransport {
        release_resource_launch_permit(resource_monitor, resource_permit);
        return Err("root-agent cannot use container transport".to_string());
    }
    let container_path_context = if backend_kind == SessionBackendKind::ContainerTransport {
        match container_path_context_for_cwd(cwd) {
            Ok(context) => Some(context),
            Err(err) => {
                release_resource_launch_permit(resource_monitor, resource_permit);
                return Err(err);
            }
        }
    } else {
        None
    };
    let container_repos = match creation::container_repos(container_path_context.as_ref()) {
        Ok(repos) => repos,
        Err(err) => {
            release_resource_launch_permit(resource_monitor, resource_permit);
            return Err(err);
        }
    };
    Ok((container_path_context, container_repos))
}

async fn pending_carrier<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    mgr: &SessionManager,
    resource_monitor: &Arc<ResourceMonitorState>,
    resource_permit: &mut Option<crate::resource_monitor::AgentLaunchPermit>,
) -> Result<crate::session::manager::CarrierMutationPermit, String> {
    let carrier_u0 = match with_create_resource_region(
        ctx.create_owner,
        resource_permit,
        acquire_lifecycle_create_publication_region(
            ctx.create_owner,
            mgr,
            ctx.lifecycle_domain,
            ctx.inline_transaction,
        ),
    )
    .await
    {
        Ok(permit) => permit,
        Err(error) => {
            release_resource_launch_permit(resource_monitor, resource_permit);
            return Err(error);
        }
    };
    Ok(carrier_u0)
}

async fn destination_admission<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    cwd: &str,
    inline_transaction: Option<&SelectionTransaction<R>>,
    lifecycle_domain: Option<&LifecycleEntryAuthority>,
    ownership: (
        Option<&crate::api::message_store::PtyInputTargetOwnership<'_>>,
        Option<&crate::session::selection::OwnedReplacementContinuation>,
    ),
) -> Result<Option<DestinationAdmission>, String> {
    let (preheld_target, owned_replacement) = ownership;
    let (create_target_key, create_target_replica_identity) =
        resolve_create_destination(cwd, lifecycle_domain, preheld_target, inline_transaction)?;
    let _owned_target_gate = acquire_create_target_lease(
        app,
        create_target_key.as_deref(),
        create_target_replica_identity.as_ref(),
        preheld_target.is_some(),
        lifecycle_domain.is_some(),
    )
    .await?;
    let create_manager = session_mgr.read().await.clone();
    let Some(create_owner) = creation::create_owner(
        preheld_target.is_some(),
        owned_replacement,
        pty_mgr,
        create_target_key.clone(),
        cwd,
        &create_manager,
        _owned_target_gate,
    ) else {
        return Ok(None);
    };
    Ok(Some(DestinationAdmission {
        create_target_replica_identity,
        create_manager,
        create_owner,
    }))
}

fn note_inline_pending<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    pending_binding: crate::session::manager::PendingCreateBinding,
) {
    if ctx.inline_transaction.is_some() {
        *ctx.inline_pending_for_body
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(pending_binding);
    }
}

fn selection_coordinator<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<tauri::State<'_, SelectionCoordinator>, String> {
    app.try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())
}
