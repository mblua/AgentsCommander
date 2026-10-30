use crate::app_state::ConfigSeedLockState;
use crate::commands::session::create::admission::acquire_lifecycle_create_publication_region;
use crate::commands::session::create::admission::release_resource_launch_permit;
use crate::commands::session::create::admission::revalidate_owned_create_effect;
use crate::commands::session::create::admission::with_create_resource_region;
use crate::commands::session::create::contracts::ContextReady;
use crate::commands::session::create::contracts::CreateBodyContext;
use crate::commands::session::create::contracts::CreateCompletion;
use crate::commands::session::create::contracts::PendingLaunch;
use crate::commands::session::create::contracts::PublicationInput;
use crate::commands::session::create::effects::start_create_effect;
use crate::commands::session::create::effects::CreateEffectStart;
use crate::commands::session::create::launch::resolve_launch_idle_tuning;
use crate::commands::session::create::publication::rollback_pre_created_session;
#[cfg(test)]
use crate::commands::session::seed_race_barriers;
use crate::config::settings::SettingsState;
use crate::events::pty::output_target;
use crate::pty::backend::BackendSpawnSpec;
use crate::pty::backend::LaunchWitness;
use crate::pty::backend::PtyViewport;
use crate::pty::backend::SessionBackendKind;
use crate::pty::container_paths::CLAUDE_CONFIG_DIR_KEY;
use crate::pty::manager::PtyManager;
use crate::resource_monitor::ResourceLaunchMetadata;
use crate::resource_monitor::ResourceLaunchRegistration;
use crate::resource_monitor::ResourceLogicalAgentSlot;
use crate::session::profile::idle_tuning_for;
use tauri::Manager;

use super::publication;

pub(crate) async fn spawn<'a: 'b, 'b, R: tauri::Runtime>(
    ctx: &'b CreateBodyContext<'a, R>,
    input: ContextReady,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        app,
        session_mgr,
        pty_mgr,
        owned_replacement,
        ..
    } = ctx;
    let ContextReady {
        effective_continuation,
        spawn_cwd,
        container_credential,
        injected_claude_config_dir,
        mut session_env_warnings,
        pending,
    } = input;
    let PendingLaunch {
        resource_monitor,
        mut resource_permit,
        mgr,
        spawn_mark,
        mut session,
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
        configured_identity,
        ..
    } = pending;
    let id = session.id;
    // Capture the effective arg vector BEFORE spawn so SessionInfo::from(&session)
    // (emitted at line ~439 as "session_created") carries provider resume flags.
    // Bind once, broadcast to two consumers: the store write is for later
    // `mgr.get_session` callers; the local-clone write is for the imminent emit.
    //
    // DO NOT REMOVE OR GATE THIS CAPTURE. Issue #65 regression guard: removing
    // or wrapping in a condition reintroduces the exact bug this plan fixes.
    // See _plans/bug-statusbar-dynamic-launch-args.md §10 and §15 for rationale.
    let effective = shell_args.clone();
    record_effective_args(
        ctx,
        &mgr,
        &resource_monitor,
        &mut resource_permit,
        pending_binding,
        effective.clone(),
    )
    .await?;
    if let Some(continuation) = owned_replacement {
        if !continuation.validate_source().await {
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            return Ok(CreateCompletion::Retained);
        }
    }
    session.effective_shell_args = Some(effective);

    let environment = prepare_environment(
        ctx,
        SpawnEnvironmentInput {
            session: &session,
            agent_id: &agent_id,
            agent_label: &agent_label,
            is_root_agent,
            injected_claude_config_dir: injected_claude_config_dir.as_deref(),
            container_path_context: container_path_context.as_ref(),
            resource_monitor: &resource_monitor,
            resource_permit: &mut resource_permit,
            session_env_warnings: &mut session_env_warnings,
        },
    );
    // #598 - seed the config folder before the PTY starts so the agent sees the
    // templated config. Best-effort: never aborts the spawn. This is the single
    // execution chokepoint every real spawn funnels through (create / replica /
    // other variants, delivery / mailbox / web wakes); prevalidation never hits
    // it because it discards the spawn. Per Q2 this runs on EVERY spawn,
    // including loop/scheduler resume-wakes (overwrite every spawn).
    match prepare_config_seed_stage(ctx, &mut resource_permit, &resource_monitor, &mgr, id).await? {
        None => return Ok(CreateCompletion::Retained),
        Some(Ok(())) => {}
        Some(Err(error)) => {
            release_resource_launch_permit(&resource_monitor, &mut resource_permit);
            drop(mgr);
            rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
            return Err(error);
        }
    }

    // #2124 - the idle tuning this launch hands the detector: the agent kind
    // plus the EFFECTIVE catalog entry matched to the configured command.
    // One settings snapshot, released before the blocking filesystem read;
    // no lock is held across that I/O.
    let launch_idle_tuning = {
        let settings_state = app.state::<SettingsState>();
        match load_launch_idle_tuning(settings_state.inner(), &agent_id, agent_kind).await {
            Ok(tuning) => tuning,
            Err(error) => {
                release_resource_launch_permit(&resource_monitor, &mut resource_permit);
                drop(mgr);
                rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                return Err(error);
            }
        }
    };
    let launch_witness = LaunchWitness::default();
    let spawn_spec = build_spawn_spec(
        ctx,
        SpawnSpecification {
            id,
            agent_id: &agent_id,
            agent_kind,
            shell_args: &shell_args,
            spawn_cwd: &spawn_cwd,
            container_repos: &container_repos,
            container_credential,
            environment,
            launch_idle_tuning,
            launch_witness: &launch_witness,
        },
    );
    let input = PublicationInput {
        agent_id,
        agent_label,
        agent_kind,
        is_coordinator,
        is_root_agent,
        fresh_forced_by_mirror,
        skip_auto_resume,
        configured_identity,
        effective_continuation,
        pending_binding,
        session_env_warnings,
        session,
        mgr,
        resource_permit,
        resource_monitor,
    };
    spawn_backend(ctx, input, spawn_spec, launch_witness, spawn_mark).await
}

async fn spawn_backend<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    mut input: PublicationInput,
    spawn_spec: BackendSpawnSpec,
    launch_witness: LaunchWitness,
    spawn_mark: crate::pty::manager::SpawnMark,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        app,
        session_mgr,
        pty_mgr,
        inline_transaction,
        owned_replacement,
        create_owner,
        marker_ticket,
        ..
    } = ctx;
    let id = input.session.id;
    // (#2413) Spawn fence: set BEFORE the first await of spawn, so a drop at
    // any point inside or after spawn keeps the close markers cleared.
    if let Some(t) = marker_ticket
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        t.session_may_exist = true;
    }
    if let Some(continuation) = owned_replacement {
        if !continuation.validate_source().await {
            release_resource_launch_permit(&input.resource_monitor, &mut input.resource_permit);
            return Ok(CreateCompletion::Retained);
        }
    }
    if owned_replacement.is_some() {
        revalidate_owned_create_effect(
            create_owner,
            &mut input.resource_permit,
            &input.resource_monitor,
            &input.mgr,
            pty_mgr,
        )
        .await?;
    }
    let backend_effect = match start_create_effect(
        inline_transaction,
        owned_replacement,
        crate::session::selection::LifecycleEffectSlot::BackendSpawn,
    ) {
        CreateEffectStart::Started(effect) => effect,
        CreateEffectStart::Retain => {
            release_resource_launch_permit(&input.resource_monitor, &mut input.resource_permit);
            return Ok(CreateCompletion::Retained);
        }
    };
    #[cfg(test)]
    if !backend_spawn_fixture_ready(owned_replacement).await {
        return Ok(CreateCompletion::Retained);
    }
    let spawn_result = PtyManager::spawn(pty_mgr, input.session.backend_kind, spawn_spec).await;
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
        drop(input.mgr);
        rollback_pre_created_session(app, session_mgr, pty_mgr, id, &err).await;
        return Err(err);
    }

    publication::complete(ctx, input).await
}

fn child_environment(
    recipe_env: Option<&[(String, String)]>,
    injected_claude_config_dir: Option<&str>,
) -> Vec<(String, String)> {
    let mut configured_env = recipe_env.map(|env| env.to_vec()).unwrap_or_default();
    // #930 - inject the copy directory as a host-path CLAUDE_CONFIG_DIR so the
    // container env translation below maps it to /workspace/.claude and the copied
    // token is actually read. Only present when we will copy and the user set none.
    if let Some(dir) = injected_claude_config_dir {
        configured_env.push((CLAUDE_CONFIG_DIR_KEY.to_string(), dir.to_owned()));
    }
    configured_env
}

fn translate_child_environment(
    mut configured_env: Vec<(String, String)>,
    env_remove_keys: &[String],
    backend_kind: SessionBackendKind,
    container_path_map: Option<&crate::pty::container_paths::ContainerPathMap>,
    env_unset: &mut Vec<String>,
    warnings: &mut Vec<crate::pty::container_paths::ContainerEnvWarning>,
) -> Vec<(String, String)> {
    if backend_kind == SessionBackendKind::ContainerTransport {
        if let Some(context) = container_path_map {
            let translated = crate::pty::container_backend::container_child_env(
                configured_env,
                env_remove_keys.to_vec(),
                context,
            );
            configured_env = translated.child_env;
            *env_unset = translated.env_unset;
            warnings.extend(translated.warnings);
        }
    }
    configured_env
}

fn resource_launch_metadata(
    cwd: &str,
    is_root_agent: bool,
    id: uuid::Uuid,
    name: &str,
    agent_id: &Option<String>,
    agent_label: &Option<String>,
) -> ResourceLaunchMetadata {
    // #516 - human-readable WG + agent identity for the Resource Monitor row,
    // derived from the deterministic spawn cwd (not the user-renamable
    // session.name). Root agents carry no wg-/__agent_ segments, so label them
    // explicitly with the bare replica dir name.
    let (workgroup, agent) = {
        let (wg, ag) = crate::config::teams::workgroup_and_agent_from_path(cwd);
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
    let project = crate::config::teams::project_from_path(cwd);
    ResourceLaunchMetadata {
        session_id: id,
        name: name.to_owned(),
        agent_id: agent_id.clone(),
        agent_label: agent_label.clone(),
        workgroup,
        agent,
        project,
    }
}

async fn run_config_seed_work(
    coordinator: &crate::session::selection::SelectionCoordinator,
    lock: ConfigSeedLockState,
    seed: crate::config::config_seed::ResolvedConfigSeed,
    id: uuid::Uuid,
    effect: Option<crate::session::selection::LifecycleEffectGuard>,
    #[cfg(test)] continuation: Option<&crate::session::selection::OwnedReplacementContinuation>,
) -> Result<crate::config::config_seed::ConfigSeedReport, String> {
    #[cfg(test)]
    let seed_fixture = continuation.and_then(|owned| owned.fixture_native(true));
    #[cfg(test)]
    let native_await_panic =
        continuation.and_then(|owned| owned.fixture_take_native_await_panic(true));
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
        let activation = Some(crate::config::seed_manifest::ManifestActivationToken::production());
        #[cfg(test)]
        let activation: Option<crate::config::seed_manifest::ManifestActivationToken> = None;
        let report = crate::config::config_seed::perform_config_seed_recorded(
            &seed,
            &id.to_string(),
            activation.as_ref(),
        );
        // The blocking worker records its actual report even when
        // the async receiver disappears. Native failure remains the
        // existing best-effort report, not a new spawn error.
        if let Some(effect) = effect {
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
    let native_result =
        crate::session::selection::await_fixture_native(seed_native, native_await_panic).await;
    #[cfg(not(test))]
    let native_result = seed_native.await;
    native_result.map_err(|error| error.to_string())
}

async fn load_launch_idle_tuning(
    settings: &crate::config::settings::SettingsState,
    agent_id: &Option<String>,
    agent_kind: Option<crate::session::profile::CodingAgentKind>,
) -> Result<crate::session::profile::IdleTuning, String> {
    let snapshot = settings
        .current_snapshot()
        .await
        .map_err(|error| error.to_string())?;
    let requested_agent_id = agent_id.clone();
    match tokio::task::spawn_blocking(move || {
        let catalog = crate::config::coding_agents_catalog::load_catalog_for_settings(&snapshot)
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
        Ok(tuning) => Ok(tuning),
        Err(error) => {
            log::warn!("[idle] launch idle tuning resolution failed: {error}");
            Ok(idle_tuning_for(agent_kind))
        }
    }
}

async fn prepare_config_seed_stage<'a: 'b, 'b, R: tauri::Runtime>(
    ctx: &'b CreateBodyContext<'a, R>,
    resource_permit: &mut Option<crate::resource_monitor::AgentLaunchPermit>,
    resource_monitor: &std::sync::Arc<crate::resource_monitor::ResourceMonitorState>,
    mgr: &crate::session::manager::SessionManager,
    id: uuid::Uuid,
) -> Result<Option<Result<(), String>>, String> {
    if let Some(seed) = ctx.resolved_spawn.and_then(|s| s.seed.as_ref()) {
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
        if ctx.owned_replacement.is_some() {
            revalidate_owned_create_effect(
                ctx.create_owner,
                resource_permit,
                resource_monitor,
                mgr,
                ctx.pty_mgr,
            )
            .await?;
        }
        let seed_effect = match start_create_effect(
            ctx.inline_transaction,
            ctx.owned_replacement,
            crate::session::selection::LifecycleEffectSlot::ConfigSeed,
        ) {
            CreateEffectStart::Started(effect) => effect,
            CreateEffectStart::Retain => {
                release_resource_launch_permit(resource_monitor, resource_permit);
                return Ok(None);
            }
        };
        let lock = ctx.app.state::<ConfigSeedLockState>().inner().clone();
        let seed_result = run_config_seed_work(
            ctx.coordinator,
            lock,
            seed,
            id,
            seed_effect,
            #[cfg(test)]
            ctx.owned_replacement,
        )
        .await;
        match seed_result {
            Ok(report) => {
                log::debug!("[config-seed] session {} outcome: {:?}", id, report);
            }
            Err(error) => {
                let error = format!("config seed blocking preparation failed: {error}");
                return Ok(Some(Err(error)));
            }
        }
    }

    // #1063 test-only barrier: the seed transaction (if any) has acquired and
    // released the project gate, but the create is still pending and unfinalized,
    // so a deletion's pending-inclusive snapshot must still observe it. No-op in
    // production.
    #[cfg(test)]
    {
        seed_race_barriers::hit_after_seed_before_pty(ctx.cwd).await;
    }
    Ok(Some(Ok(())))
}

#[cfg(test)]
async fn backend_spawn_fixture_ready(
    owned: Option<&crate::session::selection::OwnedReplacementContinuation>,
) -> bool {
    match owned {
        Some(owned) => {
            owned
                .fixture_reached(
                    crate::session::selection::ReplacementFixtureCut::BeforeBackendSpawn,
                )
                .await
        }
        None => true,
    }
}

struct SpawnEnvironment {
    extra_env: Vec<(String, String)>,
    configured_env: Vec<(String, String)>,
    env_remove_keys: Vec<String>,
    env_unset: Vec<String>,
    resource_registration: Option<ResourceLaunchRegistration>,
    logical_resource_slot: Option<ResourceLogicalAgentSlot>,
}
struct SpawnEnvironmentInput<'a> {
    session: &'a crate::session::session::Session,
    agent_id: &'a Option<String>,
    agent_label: &'a Option<String>,
    is_root_agent: bool,
    injected_claude_config_dir: Option<&'a str>,
    container_path_context:
        Option<&'a crate::commands::session::resume_probe::ContainerPathContext>,
    resource_monitor: &'a std::sync::Arc<crate::resource_monitor::ResourceMonitorState>,
    resource_permit: &'a mut Option<crate::resource_monitor::AgentLaunchPermit>,
    session_env_warnings: &'a mut Vec<crate::pty::container_paths::ContainerEnvWarning>,
}

fn prepare_environment<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    input: SpawnEnvironmentInput<'_>,
) -> SpawnEnvironment {
    let SpawnEnvironmentInput {
        session,
        agent_id,
        agent_label,
        is_root_agent,
        injected_claude_config_dir,
        container_path_context,
        resource_monitor,
        resource_permit,
        session_env_warnings,
    } = input;
    let &CreateBodyContext {
        cwd,
        resolved_spawn,
        ..
    } = ctx;
    let id = session.id;
    let extra_env = if agent_id.is_some() {
        crate::pty::credentials::build_credentials_env(&session.token, cwd)
    } else {
        Vec::new()
    };
    let configured_env = child_environment(
        resolved_spawn.map(|spawn| spawn.child_env.as_slice()),
        injected_claude_config_dir,
    );
    let env_remove_keys: Vec<String> = resolved_spawn
        .map(|spawn| spawn.env_remove_keys.clone())
        .unwrap_or_default();
    let mut env_unset = Vec::new();
    let mut warnings = Vec::new();
    let configured_env = translate_child_environment(
        configured_env,
        &env_remove_keys,
        session.backend_kind,
        container_path_context.as_ref().map(|context| &context.map),
        &mut env_unset,
        &mut warnings,
    );
    session_env_warnings.extend(warnings);
    let (resource_registration, logical_resource_slot): (
        Option<ResourceLaunchRegistration>,
        Option<ResourceLogicalAgentSlot>,
    ) = match session.backend_kind {
        SessionBackendKind::LocalProcess => resource_permit
            .take()
            .map(|permit| {
                let metadata = resource_launch_metadata(
                    cwd,
                    is_root_agent,
                    id,
                    &session.name,
                    agent_id,
                    agent_label,
                );
                ResourceLaunchRegistration::new(resource_monitor.as_ref().clone(), permit, metadata)
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

    SpawnEnvironment {
        extra_env,
        configured_env,
        env_remove_keys,
        env_unset,
        resource_registration,
        logical_resource_slot,
    }
}

async fn record_effective_args<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    mgr: &crate::session::manager::SessionManager,
    resource_monitor: &std::sync::Arc<crate::resource_monitor::ResourceMonitorState>,
    resource_permit: &mut Option<crate::resource_monitor::AgentLaunchPermit>,
    pending_binding: crate::session::manager::PendingCreateBinding,
    effective: Vec<String>,
) -> Result<(), String> {
    let &CreateBodyContext {
        create_owner,
        lifecycle_domain,
        inline_transaction,
        ..
    } = ctx;
    let carrier_args = match with_create_resource_region(
        create_owner,
        resource_permit,
        acquire_lifecycle_create_publication_region(
            create_owner,
            mgr,
            lifecycle_domain,
            inline_transaction,
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
    mgr.set_pending_effective_shell_args_with_carrier_permit(
        &carrier_args,
        pending_binding,
        effective,
    )
    .await
    .map_err(|error| error.to_string())?;
    drop(carrier_args);

    Ok(())
}

struct SpawnSpecification<'a> {
    id: uuid::Uuid,
    agent_id: &'a Option<String>,
    agent_kind: Option<crate::session::profile::CodingAgentKind>,
    shell_args: &'a [String],
    spawn_cwd: &'a str,
    container_repos: &'a Option<crate::pty::container_repos::RepoMountResolution>,
    container_credential: Option<crate::pty::container_credentials::ContainerCredentialPlan>,
    environment: SpawnEnvironment,
    launch_idle_tuning: crate::session::profile::IdleTuning,
    launch_witness: &'a LaunchWitness,
}

fn build_spawn_spec<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    input: SpawnSpecification<'_>,
) -> BackendSpawnSpec {
    let SpawnSpecification {
        id,
        agent_id,
        agent_kind,
        shell_args,
        spawn_cwd,
        container_repos,
        container_credential,
        environment,
        launch_idle_tuning,
        launch_witness,
    } = input;
    let &CreateBodyContext {
        app,
        shell,
        cwd,
        resolved_spawn,
        resolved_agent_host_shell,
        viewport,
        ..
    } = ctx;
    let viewport = viewport.unwrap_or(PtyViewport::DEFAULT);
    let SpawnEnvironment {
        extra_env,
        configured_env,
        env_remove_keys,
        env_unset,
        resource_registration,
        logical_resource_slot,
    } = environment;
    BackendSpawnSpec {
        id,
        agent_id: agent_id.clone(),
        // #942 - the CLI identity from the canonical detector (`agent_kind`, resolved
        // at the top of this function). The profile id above cannot stand in for it:
        // it is opaque, several profiles can drive the same CLI, and a coding-agent
        // launch can carry no profile id at all.
        coding_agent: agent_kind,
        cmd: shell.to_owned(),
        args: shell_args.to_vec(),
        // #1271 - the configured host shell paired with a resolved agent
        // command, carried as one immutable snapshot (never as loose
        // program/argument fields, so the pairing invariant cannot drift).
        resolved_agent_host_shell: resolved_agent_host_shell.cloned(),
        cwd: spawn_cwd.to_owned(),
        selected_cwd: if spawn_cwd != cwd {
            Some(cwd.to_owned())
        } else {
            None
        },
        // #973 - open the PTY at the size the terminal is actually going to be, so the
        // frontend never has to resize a child that is still starting up. Callers with no
        // view (restore loop, delivery loop, mailbox, CLI, tests) get PtyViewport::DEFAULT,
        // which is the 120x30 AC always used.
        cols: viewport.cols,
        rows: viewport.rows,
        container_image: resolved_spawn.and_then(|spawn| spawn.backend.image.clone()),
        configured_env,
        env_remove_keys,
        env_unset,
        extra_env,
        idle_tuning: launch_idle_tuning,
        output_target: output_target(app.clone()),
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
    }
}
