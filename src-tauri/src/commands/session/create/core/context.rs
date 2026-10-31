use super::backend;
use crate::commands::session::agent_command::executable_basename;
use crate::commands::session::create::admission::acquire_lifecycle_create_publication_region;
use crate::commands::session::create::admission::release_resource_launch_permit;
use crate::commands::session::create::admission::revalidate_owned_create_effect;
use crate::commands::session::create::admission::with_create_resource_region;
use crate::commands::session::create::context_prep::notify_replica_context_failure;
use crate::commands::session::create::contracts::ContextReady;
use crate::commands::session::create::contracts::CreateBodyContext;
use crate::commands::session::create::contracts::CreateCompletion;
use crate::commands::session::create::contracts::CreateSelectionIntent;
use crate::commands::session::create::contracts::PendingLaunch;
use crate::commands::session::create::effects::start_create_effect;
use crate::commands::session::create::effects::CreateEffectStart;
use crate::commands::session::create::launch::resolve_launch_auto_self_clear;
use crate::commands::session::create::publication::rollback_pre_created_session;
use crate::commands::session::resume_args::inject_antigravity_resume;
use crate::commands::session::resume_args::inject_codex_resume;
use crate::commands::session::resume_args::maybe_inject_muse_resume;
use crate::commands::session::resume_args::maybe_inject_pi_resume;
use crate::commands::session::resume_probe::claude_resume_probe_target_for_kind;
use crate::commands::session::resume_probe::effective_codex_home_for_backend;
use crate::commands::session::resume_probe::injected_claude_config_dir_for_copy;
use crate::commands::session::resume_probe::should_inject_continue;
use crate::commands::session::resume_probe::should_inject_fresh_session_id;
use crate::config::settings::SettingsState;
use crate::pty::backend::SessionBackendKind;
use crate::pty::container_paths::CLAUDE_CONFIG_DIR_KEY;
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::session::selection::SelectionCause;
use crate::session::session::SessionCommunication;
use crate::session::session::SessionStatus;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

pub(crate) async fn prepare<'a: 'b, 'b, R: tauri::Runtime>(
    ctx: &'b CreateBodyContext<'a, R>,
    input: PendingLaunch,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        app,
        session_mgr,
        pty_mgr,
        cwd,
        resolved_spawn,
        ..
    } = ctx;
    let mut pending = input;
    let id = pending.session.id;
    let effective_continuation = false;
    // #930 - resolve the host-credential copy-in plan for container coding agents
    // BEFORE the resume probe and the container env translation, so both consumers
    // observe the CLAUDE_CONFIG_DIR we may inject below. Gated by the global setting
    // (default on) and the per-agent profile descriptor; None when off,
    // non-container, an unrecognized agent, or the host file is absent. The plan is
    // pure (env read + file-exists check); the actual copy runs later in the
    // container backend spawn (spawn_runtime_backed), preserving copy-after-seed.
    let spawn_cwd = pending
        .container_path_context
        .as_ref()
        .map(|context| context.host_root.clone())
        .unwrap_or_else(|| cwd.to_owned());
    let container_credential = if pending.session.backend_kind
        == SessionBackendKind::ContainerTransport
    {
        let settings_state = app.state::<SettingsState>();
        let copy_enabled = {
            let cfg = match settings_state.current_read().await {
                Ok(cfg) => cfg,
                Err(error) => {
                    let error = error.to_string();
                    release_resource_launch_permit(
                        &pending.resource_monitor,
                        &mut pending.resource_permit,
                    );
                    drop(pending.mgr);
                    rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                    return Err(error);
                }
            };
            cfg.container_credentials_from_host
        };
        if copy_enabled {
            pending
                .agent_kind
                .and_then(|k| k.profile().container_credential)
                .and_then(|src| crate::pty::container_credentials::resolve_plan(&src, &spawn_cwd))
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

    let session_env_warnings = Vec::new();
    let input = ContextReady {
        effective_continuation,
        spawn_cwd,
        container_credential,
        injected_claude_config_dir,
        session_env_warnings,
        pending,
    };
    prepare_resume(ctx, input).await
}

pub(crate) async fn prepare_resume<'a: 'b, 'b, R: tauri::Runtime>(
    ctx: &'b CreateBodyContext<'a, R>,
    mut input: ContextReady,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        shell,
        cwd,
        resolved_spawn,
        inline_transaction,
        owned_replacement,
        lifecycle_domain,
        create_owner,
        ..
    } = ctx;
    // Resolve and inspect Claude transcript storage only for a canonically
    // detected Claude launch. Provider-looking Pi option values must not cause
    // any Claude path resolution, filesystem check, warning, or stored memo.
    let is_claude = input.pending.agent_kind == Some(CodingAgentKind::Claude);
    let resume_probe = claude_resume_probe_target_for_kind(
        input.pending.agent_kind,
        input.pending.session.backend_kind,
        input
            .pending
            .container_path_context
            .as_ref()
            .map(|context| &context.map),
        resolved_spawn,
        input.injected_claude_config_dir.as_deref(),
        shell,
        &input.pending.shell_args,
        cwd,
    );
    let resolved_claude_projects_dir = resume_probe
        .as_ref()
        .and_then(|probe| probe.host_probe_path.clone());
    let carrier_projects = match with_create_resource_region(
        create_owner,
        &mut input.pending.resource_permit,
        acquire_lifecycle_create_publication_region(
            create_owner,
            &input.pending.mgr,
            lifecycle_domain,
            inline_transaction,
        ),
    )
    .await
    {
        Ok(permit) => permit,
        Err(error) => {
            release_resource_launch_permit(
                &input.pending.resource_monitor,
                &mut input.pending.resource_permit,
            );
            return Err(error);
        }
    };
    input
        .pending
        .mgr
        .set_pending_resolved_claude_projects_dir_with_carrier_permit(
            &carrier_projects,
            input.pending.pending_binding,
            resolved_claude_projects_dir.clone(),
        )
        .await
        .map_err(|error| error.to_string())?;
    drop(carrier_projects);
    if let Some(continuation) = owned_replacement {
        if !continuation.validate_source().await {
            release_resource_launch_permit(
                &input.pending.resource_monitor,
                &mut input.pending.resource_permit,
            );
            return Ok(CreateCompletion::Retained);
        }
    }
    input.pending.session.resolved_claude_projects_dir = resolved_claude_projects_dir.clone();
    let claude_project_exists = resume_probe
        .as_ref()
        .and_then(|probe| probe.host_probe_path.as_ref())
        .is_some_and(|path| crate::config::agent_command::resume_host_directory_is_dir(path));
    if let Some(warning) = resume_probe
        .as_ref()
        .and_then(|probe| probe.warning.clone())
    {
        input.session_env_warnings.push(warning);
    }
    let will_inject_continue = should_inject_continue(
        is_claude,
        input.pending.skip_auto_resume,
        claude_project_exists,
        &input.pending.full_cmd,
    );
    log_resume_decision(
        cwd,
        &input.pending,
        resume_probe.as_ref(),
        &resolved_claude_projects_dir,
        claude_project_exists,
        will_inject_continue,
    );
    apply_resume_args(
        shell,
        resolved_spawn,
        &mut input,
        is_claude,
        will_inject_continue,
    );

    prepare_instructions(ctx, input).await
}

pub(crate) async fn prepare_instructions<'a: 'b, 'b, R: tauri::Runtime>(
    ctx: &'b CreateBodyContext<'a, R>,
    mut input: ContextReady,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        app,
        session_mgr,
        pty_mgr,
        shell,
        cwd,
        inline_transaction,
        owned_replacement,
        create_owner,
        ..
    } = ctx;
    let id = input.pending.session.id;
    // #529 - resolve the instructions filename from the configured coding agent
    // (falling back to detection for ad-hoc launches), plus the union of every
    // configured agent's filename for cleanup. Computed under a single settings
    // read guard that is dropped before any filesystem I/O (no guard across the
    // materialize call).
    let (target_filename, managed_filenames, auto_self_clear) = {
        let settings_state = app.state::<SettingsState>();
        let cfg = match settings_state.current_read().await {
            Ok(cfg) => cfg,
            Err(error) => {
                let error = error.to_string();
                release_resource_launch_permit(
                    &input.pending.resource_monitor,
                    &mut input.pending.resource_permit,
                );
                drop(input.pending.mgr);
                rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                return Err(error);
            }
        };
        let managed = crate::config::agent_command::managed_instructions_filenames(&cfg);
        let target = crate::config::agent_command::resolve_target_filename(
            input.pending.agent_id.as_deref(),
            &cfg,
            input.pending.context_target,
        );
        // Resolve under the same guard; no second lock is held across I/O.
        // Eligibility is an explicit direct-shell capability, independent of
        // CodingAgentKind provider tuning.
        let auto_self_clear =
            resolve_launch_auto_self_clear(&cfg, shell, cwd, input.pending.is_coordinator);
        (target, managed, auto_self_clear)
    };

    if let Some(ref target_filename) = target_filename {
        if owned_replacement.is_some() {
            revalidate_owned_create_effect(
                create_owner,
                &mut input.pending.resource_permit,
                &input.pending.resource_monitor,
                &input.pending.mgr,
                pty_mgr,
            )
            .await?;
        }
        let context_effect = match start_create_effect(
            inline_transaction,
            owned_replacement,
            crate::session::selection::LifecycleEffectSlot::ContextPreparation,
        ) {
            CreateEffectStart::Started(effect) => effect,
            CreateEffectStart::Retain => {
                release_resource_launch_permit(
                    &input.pending.resource_monitor,
                    &mut input.pending.resource_permit,
                );
                return Ok(CreateCompletion::Retained);
            }
        };
        let context_result = run_context_work(
            ctx,
            ContextWorkInput {
                target_filename,
                managed_filenames: &managed_filenames,
                container_repos: &input.pending.container_repos,
                skip_auto_resume: input.pending.skip_auto_resume,
                is_coordinator: input.pending.is_coordinator,
                auto_self_clear,
                context_effect,
            },
        )
        .await;
        let context_result = match context_result {
            Ok(result) => result,
            Err(error) => {
                let error = format!("replica context blocking preparation failed: {error}");
                release_resource_launch_permit(
                    &input.pending.resource_monitor,
                    &mut input.pending.resource_permit,
                );
                drop(input.pending.mgr);
                rollback_pre_created_session(app, session_mgr, pty_mgr, id, &error).await;
                return Err(error);
            }
        };
        match context_result {
            Ok(_) => {}
            Err(e) => {
                notify_replica_context_failure(app, owned_replacement, &e);
                release_resource_launch_permit(
                    &input.pending.resource_monitor,
                    &mut input.pending.resource_permit,
                );
                drop(input.pending.mgr);
                rollback_pre_created_session(app, session_mgr, pty_mgr, id, &e).await;
                return Err(e);
            }
        }
    }

    backend::spawn(ctx, input).await
}

fn inject_claude_continue_args(
    shell: &str,
    agent_id: Option<&str>,
    shell_args: &mut Vec<String>,
) -> bool {
    let mut effective_continuation = false;
    // #260: Claude's resume flag from the CodingAgentProfile. resume_tokens
    // is a 1-element const for Claude, so [0] is provably in bounds.
    let continue_flag = CodingAgentKind::Claude.profile().resume_tokens[0];
    if let Some(aid) = agent_id {
        if executable_basename(shell) == "cmd" {
            if let Some(last) = shell_args.last_mut() {
                if executable_basename(last) == "claude" || last.to_lowercase().contains("claude") {
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

    effective_continuation
}

fn inject_claude_fresh_args(shell: &str, agent_id: Option<&str>, shell_args: &mut Vec<String>) {
    if let Some(aid) = agent_id {
        let fresh_session_id = Uuid::new_v4();
        if executable_basename(shell) == "cmd" {
            if let Some(last) = shell_args.last_mut() {
                if executable_basename(last) == "claude" || last.to_lowercase().contains("claude") {
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

fn inject_other_provider_resume_args(
    agent_kind: Option<CodingAgentKind>,
    resolved_spawn: Option<&crate::config::agent_command::AgentSpawnCommand>,
    skip_auto_resume: bool,
    shell: &str,
    agent_id: Option<&str>,
    shell_args: &mut Vec<String>,
) -> bool {
    let mut effective_continuation = false;
    if maybe_inject_pi_resume(
        agent_kind,
        resolved_spawn,
        skip_auto_resume,
        shell,
        shell_args,
    ) {
        effective_continuation = true;
        if let Some(spawn) = resolved_spawn {
            log::info!(
                "Auto-injected Pi `--continue` for trusted agent '{}'",
                spawn.trusted_agent_id
            );
        }
    }

    if agent_kind == Some(CodingAgentKind::Codex) && !skip_auto_resume {
        if let Some(aid) = agent_id {
            if inject_codex_resume(shell, shell_args) {
                effective_continuation = true;
                log::info!("Auto-injected `codex resume --last` for agent '{}'", aid);
            }
        }
    }

    if agent_kind == Some(CodingAgentKind::Antigravity) && !skip_auto_resume {
        if let Some(aid) = agent_id {
            if inject_antigravity_resume(shell, shell_args) {
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
        resolved_spawn,
        skip_auto_resume,
        shell,
        shell_args,
    ) {
        effective_continuation = true;
        if let Some(spawn) = resolved_spawn {
            log::info!(
                "Auto-injected Muse `resume --last` for trusted agent '{}'",
                spawn.trusted_agent_id
            );
        }
    }

    effective_continuation
}

struct ContextWorkInput<'a> {
    target_filename: &'a str,
    managed_filenames: &'a [String],
    container_repos: &'a Option<crate::pty::container_repos::RepoMountResolution>,
    skip_auto_resume: bool,
    is_coordinator: bool,
    auto_self_clear: bool,
    context_effect: Option<crate::session::selection::LifecycleEffectGuard>,
}

async fn run_context_work<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    input: ContextWorkInput<'_>,
) -> Result<Result<Option<String>, String>, impl std::fmt::Display> {
    let ContextWorkInput {
        target_filename,
        managed_filenames,
        container_repos,
        skip_auto_resume,
        is_coordinator,
        auto_self_clear,
        context_effect,
    } = input;
    let &CreateBodyContext {
        cwd, coordinator, ..
    } = ctx;
    #[cfg(test)]
    let owned_replacement = ctx.owned_replacement;
    #[cfg(test)]
    let context_fixture = owned_replacement.and_then(|owned| owned.fixture_native(false));
    #[cfg(test)]
    let native_await_panic =
        owned_replacement.and_then(|owned| owned.fixture_take_native_await_panic(false));
    let context_native = coordinator.run_blocking_seed_work({
        let cwd = cwd.to_owned();
        // #1172 D5: capture the FINAL fresh-versus-resume decision. `skip_auto_resume`
        // is a `mut` parameter of this function whose only mutation is the #756 mirror
        // at :1470, ~500 lines above, so the value read here is final. `true` means AC
        // is deliberately NOT resuming: a fresh conversation begins.
        let start_fresh = skip_auto_resume;
        let target_filename = target_filename.to_owned();
        let managed_filenames = managed_filenames.to_vec();
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
            let context_result = ctx_file::materialize_agent_context_file_with_filename_activated(
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
                effect.complete(context_result.as_ref().map(|_| None).map_err(Clone::clone));
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
        crate::session::selection::await_fixture_native(context_native, native_await_panic).await;
    #[cfg(not(test))]
    let context_result = context_native.await;

    context_result
}

pub(crate) struct BindTargetInput<'a> {
    pub target: &'a Option<crate::path_identity::VerifiedPathIdentity>,
    pub selection_intent: CreateSelectionIntent,
    pub mgr: &'a SessionManager,
    pub resource_monitor: &'a Arc<ResourceMonitorState>,
    pub resource_permit: &'a mut Option<crate::resource_monitor::AgentLaunchPermit>,
    pub prepared_binding: crate::session::manager::PendingCreateBinding,
    pub carrier_u0: crate::session::manager::CarrierMutationPermit,
}

pub(crate) async fn bind_and_check_target<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    input: BindTargetInput<'_>,
) -> Result<Option<crate::session::manager::CarrierMutationPermit>, String> {
    let BindTargetInput {
        target,
        selection_intent,
        mgr,
        resource_monitor,
        resource_permit,
        prepared_binding,
        carrier_u0,
    } = input;
    let &CreateBodyContext {
        pty_mgr,
        inline_transaction,
        inline_cause,
        create_ticket,
        ..
    } = ctx;
    #[cfg(test)]
    let owned_replacement = ctx.owned_replacement;
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
            release_resource_launch_permit(resource_monitor, resource_permit);
            return Ok(None);
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
    if target.is_some()
        && matches!(
            selection_intent,
            CreateSelectionIntent::Background | CreateSelectionIntent::Suppress
        )
        && !matches!(inline_cause, Some(SelectionCause::Restart(_)))
    {
        let target = target
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
            release_resource_launch_permit(resource_monitor, resource_permit);
            return Err("sessionRace".to_string());
        }
    }
    Ok(Some(carrier_u0))
}

pub(crate) async fn apply_profile_metadata<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    mgr: &SessionManager,
    carrier_u1: &crate::session::manager::CarrierMutationPermit,
    pending_binding: crate::session::manager::PendingCreateBinding,
    session: &mut crate::session::session::Session,
    container_path_context: Option<&crate::commands::session::resume_probe::ContainerPathContext>,
) -> Result<(), String> {
    let resolved_spawn = ctx.resolved_spawn;
    if let Some(spawn) = resolved_spawn {
        let effective_codex_home = effective_codex_home_for_backend(
            session.backend_kind,
            container_path_context.as_ref().map(|context| &context.map),
            spawn,
        );
        mgr.set_pending_profile_metadata_with_carrier_permit(
            carrier_u1,
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

    Ok(())
}

pub(crate) async fn apply_pending_options<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    mgr: &SessionManager,
    carrier_u0: &crate::session::manager::CarrierMutationPermit,
    pending_binding: crate::session::manager::PendingCreateBinding,
    session: &mut crate::session::session::Session,
    pending_communication: Option<&SessionCommunication>,
) -> Result<(), String> {
    let pending_start_fresh = ctx.pending_start_fresh;
    if let Some(value) = pending_start_fresh {
        mgr.set_pending_start_fresh_on_restore_with_carrier_permit(
            carrier_u0,
            pending_binding,
            value,
        )
        .await
        .map_err(|error| error.to_string())?;
        session.start_fresh_on_restore = value;
    }
    if let Some(communication) = pending_communication.cloned() {
        mgr.set_pending_communication_with_carrier_permit(
            carrier_u0,
            pending_binding,
            communication.clone(),
        )
        .await
        .map_err(|error| error.to_string())?;
        session.communication = Some(communication);
    }

    Ok(())
}

pub(crate) fn pending_is_observed<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    pending_binding: crate::session::manager::PendingCreateBinding,
) -> bool {
    let &CreateBodyContext {
        owned_replacement,
        inline_transaction,
        ..
    } = ctx;
    if owned_replacement
        .as_ref()
        .is_some_and(|continuation| !continuation.mark_pending(pending_binding))
    {
        return false;
    }
    if inline_transaction
        .is_some_and(|transaction| !transaction.observe_entry_pending_binding(pending_binding))
    {
        return false;
    }
    true
}

fn log_resume_decision(
    cwd: &str,
    pending: &PendingLaunch,
    resume_probe: Option<&crate::commands::session::resume_probe::ResumeProbeTarget>,
    resolved_claude_projects_dir: &Option<std::path::PathBuf>,
    claude_project_exists: bool,
    will_inject_continue: bool,
) {
    if let Some(probe) = resume_probe {
        log::info!(
            "[session] claude-resume-decision {} cwd={:?} projects_dir={:?} filesystem={} \
             exists={} skip_auto_resume={} -> inject_continue={}",
            &pending.session.id.to_string()[..8],
            cwd,
            resolved_claude_projects_dir,
            probe.filesystem,
            claude_project_exists,
            pending.skip_auto_resume,
            will_inject_continue
        );
    } else if let Some(kind) = pending.agent_kind {
        log::info!(
            "[session] resume-decision {} agent={} cwd={:?} skip_auto_resume={}",
            &pending.session.id.to_string()[..8],
            kind.as_str(),
            cwd,
            pending.skip_auto_resume
        );
    }
}

fn apply_resume_args(
    shell: &str,
    resolved_spawn: Option<&crate::config::agent_command::AgentSpawnCommand>,
    input: &mut ContextReady,
    is_claude: bool,
    will_inject_continue: bool,
) {
    if will_inject_continue {
        input.effective_continuation |= inject_claude_continue_args(
            shell,
            input.pending.agent_id.as_deref(),
            &mut input.pending.shell_args,
        );
    }

    // (#756) Rider: on Claude spawns where AC suppresses resume, mint the fresh
    // session's identity at spawn. Uuid::new_v4() satisfies the "unused UUID"
    // requirement (Claude errors on a colliding --session-id; v4 collision is
    // negligible and a fresh one is minted per spawn). Mutually exclusive with
    // the --continue block above by construction (will_inject_continue requires
    // skip_auto_resume=false). `full_cmd` predates both blocks' mutations, so
    // the veto scan sees user-configured identity flags, never AC's own.
    if should_inject_fresh_session_id(
        is_claude,
        input.pending.skip_auto_resume,
        &input.pending.full_cmd,
    ) {
        inject_claude_fresh_args(
            shell,
            input.pending.agent_id.as_deref(),
            &mut input.pending.shell_args,
        );
    }

    input.effective_continuation |= inject_other_provider_resume_args(
        input.pending.agent_kind,
        resolved_spawn,
        input.pending.skip_auto_resume,
        shell,
        input.pending.agent_id.as_deref(),
        &mut input.pending.shell_args,
    );
}
