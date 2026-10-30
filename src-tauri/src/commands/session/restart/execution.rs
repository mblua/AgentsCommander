mod selection;
mod source;

use super::context::{ensure_lifecycle_root_directory, LifecycleExecutionContext};
use super::settlement::{
    accept_restart_deferred_completion, accept_restart_teardown, finalize_restart_deferred,
    publish_restart_after_warnings, publish_restart_before_warnings, teardown_old_for_restart,
};
use crate::commands::session::agent_command::{
    carry_communication_for_restart, effective_restart_requested_profile,
    restart_skip_auto_resume_with_intent,
};
use crate::commands::session::create::{create_session_inner_impl, CreateSelectionIntent};
use crate::commands::session::lifecycle::retire_lifecycle_entry_protection;
#[cfg(test)]
use crate::commands::session::restart_reader_seam;
use crate::config::settings::SettingsState;
use crate::pty::backend::ResolvedAgentHostShell;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::selection::{RestartJobRequest, SelectionCause, SelectionTransaction};
use crate::session::session::{SessionInfo, SessionStatus};
use crate::session::warnings::emit_session_warning;
use std::sync::{Arc, Mutex};
use tauri::Manager;

pub(crate) async fn execute_restart_transaction_in_context<R: tauri::Runtime>(
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
    // 1. Read config from existing session BEFORE destroying it.
    let source = read_restart_source(transaction, &session_mgr, request.session_id).await?;
    let (input, continuation) = source.into_preparation(request);
    let source::RestartContinuation {
        uuid,
        name,
        git_repos,
        is_root_agent,
        telegram_bot_id,
        old_status,
        old_selected,
        activate_after,
        intent,
        enforcement,
    } = continuation;
    let prepared = prepare_restart(transaction, context, &settings, input).await;
    let PreparedRestart {
        shell,
        shell_args,
        cwd,
        selected_agent_id,
        agent_label,
        resolved_spawn,
        resolved_agent_host_shell,
        restart_start_fresh,
        carried_communication,
    } = prepared?;

    let retirement = begin_restart_retirement(transaction, context).await?;

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

pub(crate) struct RestartSource {
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    name: String,
    stored_agent_id: Option<String>,
    stored_agent_label: Option<String>,
    git_repos: Vec<crate::session::session::SessionRepo>,
    is_root_agent: bool,
    telegram_bot_id: Option<String>,
    stored_requested_profile: Option<String>,
    stored_start_fresh: bool,
    stored_communication: Option<crate::session::session::SessionCommunication>,
    old_status: SessionStatus,
    old_selected: bool,
}

pub(crate) struct RestartPreparation {
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    stored_agent_id: Option<String>,
    stored_agent_label: Option<String>,
    is_root_agent: bool,
    stored_requested_profile: Option<String>,
    stored_start_fresh: bool,
    stored_communication: Option<crate::session::session::SessionCommunication>,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
    communication_override: Option<crate::session::session::SessionCommunication>,
    enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
}

struct PreparedRestart {
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    selected_agent_id: Option<String>,
    agent_label: Option<String>,
    resolved_spawn: Option<crate::config::agent_command::AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
    restart_start_fresh: bool,
    carried_communication: Option<crate::session::session::SessionCommunication>,
}

async fn read_restart_source<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    uuid: uuid::Uuid,
) -> Result<RestartSource, String> {
    let mgr = session_mgr.read().await;
    let session = match mgr.get_session(uuid).await {
        Some(session) => session,
        None => {
            transaction.observe_lifecycle_missing_source();
            return Err("Session not found".to_string());
        }
    };
    let old_selected = mgr.selection_payload().await.id() == Some(uuid);
    Ok(RestartSource {
        shell: session.shell.clone(),
        shell_args: session.shell_args.clone(),
        cwd: session.working_directory.clone(),
        name: session.name.clone(),
        stored_agent_id: session.agent_id.clone(),
        stored_agent_label: session.agent_label.clone(),
        git_repos: session.git_repos.clone(),
        is_root_agent: session.is_root_agent
            || crate::config::root_agent::is_root_agent_path(&session.working_directory),
        telegram_bot_id: session.telegram_bot_id.clone(),
        stored_requested_profile: session.requested_profile.clone(),
        // (#630/#631) honor an unconsumed durable fresh intent
        stored_start_fresh: session.start_fresh_on_restore,
        // (#747) candidate raised-hand carry across the reopen
        stored_communication: session.communication.clone(),
        old_status: session.status.clone(),
        old_selected,
    })
}

async fn prepare_restart<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    context: &LifecycleExecutionContext<'_>,
    settings: &SettingsState,
    input: RestartPreparation,
) -> Result<PreparedRestart, String> {
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
    let app = transaction.app();
    let RestartPreparation {
        shell,
        shell_args,
        cwd,
        stored_agent_id,
        stored_agent_label,
        is_root_agent,
        stored_requested_profile,
        stored_start_fresh,
        stored_communication,
        agent_id,
        requested_profile,
        skip_auto_resume,
        communication_override,
        enforcement,
    } = input;
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
        crate::config::sessions_persistence::enforce_creation_gate(settings, &cwd, enforcement)
            .await?;

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
        let selection::RestartSelection {
            selected_agent_id,
            resolved_spawn,
            resolved_agent_host_shell,
        } = {
            let cfg = settings
                .current_read()
                .await
                .map_err(|error| error.to_string())?;
            selection::resolve_restart_selection(
                &cfg,
                &cwd,
                requested_agent_id.as_deref(),
                stored_agent_id.as_deref(),
                selected_requested_profile.as_deref(),
            )?
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

        Ok(PreparedRestart {
            shell,
            shell_args,
            cwd,
            selected_agent_id,
            agent_label,
            resolved_spawn,
            resolved_agent_host_shell,
            restart_start_fresh,
            carried_communication,
        })
    }
    .await;
    if let Some(preparation) = preparation {
        preparation.complete(prepared.as_ref().map(|_| None).map_err(Clone::clone));
    }
    match prepared {
        Ok(prepared) => Ok(prepared),
        Err(error) => {
            transaction.observe_lifecycle_preparation_failure();
            Err(error)
        }
    }
}

async fn begin_restart_retirement<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    context: &LifecycleExecutionContext<'_>,
) -> Result<Option<crate::session::selection::LifecycleEffectGuard>, String> {
    let app = transaction.app();
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

    Ok(retirement)
}
