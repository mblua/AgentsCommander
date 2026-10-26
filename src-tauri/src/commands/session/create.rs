use crate::commands::session::lifecycle::LifecycleEntryAuthority;
use crate::config::agent_command::AgentSpawnCommand;
use crate::pty::backend::PtyViewport;
use crate::pty::backend::ResolvedAgentHostShell;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::selection::SelectionCause;
use crate::session::selection::SelectionTransaction;
use crate::session::session::SessionCommunication;
use crate::session::session::SessionInfo;
use crate::session::session::SessionRepo;
use core::create_session_inner_core;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Manager;
mod admission;
mod context_prep;
mod contracts;
mod core;
mod effects;
mod launch;
mod publication;
#[cfg(test)]
pub(crate) use admission::acquire_create_publication_region;
#[cfg(test)]
pub(crate) use admission::note_coordinator_create_on_app;
#[cfg(test)]
pub(crate) use admission::try_acquire_create_publication_region;
#[cfg(test)]
#[cfg(target_os = "windows")]
pub(crate) use admission::with_create_resource_region;
#[cfg(test)]
#[cfg(target_os = "windows")]
pub(crate) use admission::CreateResourceAdmission;
#[cfg(test)]
pub(crate) use admission::OwnedCreateAdmission;
pub(crate) use launch::build_configured_agent_spawn_for_cwd;
pub(crate) use launch::compute_profile_outdated;
pub(crate) use launch::resolve_configured_agent_spawn_for_cwd;
pub(crate) use launch::resolve_create_session_cwd;
#[cfg(test)]
pub(crate) use launch::resolve_launch_auto_self_clear;
#[cfg(test)]
pub(crate) use launch::resolve_launch_idle_tuning;

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

pub use contracts::CreateSelectionIntent;
pub(crate) use contracts::{
    CreateCompletion, DeferredCreateOutput, InitialLastPrompt, OwnedReplacementCreateRequest,
    OwnedReplacementCreateStep,
};
#[cfg(test)]
pub(crate) use publication::register_session_samplers;
pub(crate) use publication::{purge_session_side_state, reset_substantive_input};
