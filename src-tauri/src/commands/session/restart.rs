pub(crate) mod context;
mod settlement;
pub(crate) use context::LifecycleExecutionContext;
pub(crate) use settlement::{
    accept_restart_deferred_completion, accept_restart_teardown,
    complete_owned_replacement_retirement, finalize_restart_deferred,
    publish_restart_after_warnings, publish_restart_before_warnings, teardown_old_for_restart,
};

pub(crate) use super::agent_command::carry_communication_for_restart;
pub(crate) use super::agent_command::effective_create_skip_auto_resume;
pub(crate) use super::agent_command::effective_restart_requested_profile;
#[cfg(test)]
pub(crate) use super::agent_command::effective_restart_skip_auto_resume;
pub(crate) use super::agent_command::matched_selection;
pub(crate) use super::agent_command::mirror_forces_fresh;
pub(crate) use super::agent_command::resolve_restart_selected_agent_id;
pub(crate) use super::agent_command::restart_skip_auto_resume_with_intent;
pub(crate) use super::agent_command::MatchedSelection;
use crate::commands::session::execute_restart_transaction_in_context;
use crate::commands::session::prepare_restore_lifecycle_authority;
use crate::commands::session::LifecycleEntryAuthority;
use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::selection::LifecycleExecutorState;
use crate::session::selection::SelectionCoordinator;
use crate::session::selection::SelectionTransaction;
use crate::session::selection::TrustedRestartIntent;
use crate::session::session::SessionCommunication;
use crate::session::session::SessionInfo;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Manager;
use uuid::Uuid;

pub(crate) use crate::session::selection::RestartJobRequest;

/// Restart a session: destroy the existing one and recreate it with the same
/// configuration but a fresh PTY. By default suppresses provider auto-resume
/// (true user-intent restart — fresh conversation).
///
/// Callers that are instead *waking* a previously-deferred session pass
/// `skip_auto_resume = Some(false)`. A session is "deferred" (PTY `Exited(0)`
/// at startup) for any of the following reasons:
///
///   1. `restoreCoordinatorWakeState = false` (the new-policy default per
///      issue #248) defers every persisted session regardless of agent type.
///   2. `restoreCoordinatorWakeState = true` defers all non-coordinator team
///      members (these are never auto-woken on startup under the new policy).
///   3. `restoreCoordinatorWakeState = true` defers coordinators whose PTY
///      status was `Exited(_)` at the prior shutdown (the state-sensitive
///      branch added by issue #248).
///   4. The user explicitly closed the session during the prior run.
///
/// In all four cases, `skip_auto_resume = Some(false)` allows the next PTY
/// spawn to inject `claude --continue`, `codex resume --last`, or
/// `antigravity --continue` so the prior conversation continues.
///
/// The restarted session is automatically activated, Telegram bridges are
/// re-attached, and state is persisted.
#[allow(clippy::too_many_arguments)]
pub async fn restart_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
) -> Result<SessionInfo, String> {
    restart_session_inner_with_activation(
        app,
        session_mgr,
        pty_mgr,
        settings,
        uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn restart_session_inner_with_activation<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
    activate_after: bool,
) -> Result<SessionInfo, String> {
    restart_session_inner_with_intent(
        app,
        session_mgr,
        pty_mgr,
        settings,
        uuid,
        agent_id,
        requested_profile,
        skip_auto_resume,
        activate_after,
        TrustedRestartIntent::User,
        None,
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn restart_session_inner_with_intent<R: tauri::Runtime>(
    app: &AppHandle<R>,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: &Arc<Mutex<PtyManager>>,
    _settings: &SettingsState,
    uuid: Uuid,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume: Option<bool>,
    activate_after: bool,
    intent: TrustedRestartIntent,
    communication_override: Option<SessionCommunication>,
    enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
) -> Result<SessionInfo, String> {
    let restarted = app
        .state::<SelectionCoordinator>()
        .restart_lifecycle(
            app.clone(),
            RestartJobRequest {
                session_id: uuid,
                agent_id,
                requested_profile,
                skip_auto_resume,
                activate_after,
                intent,
                communication_override,
                enforcement,
            },
        )
        .await?;

    // #2232 phase 4 section 5.2: the restart replaces the session UUID, so
    // `execute_restart_transaction` owns the reader lifecycle on both the
    // success and failure paths (release the old id, then raise the
    // replacement's own demands). Nothing reader-related is done here.

    Ok(restarted)
}

#[expect(
    dead_code,
    reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
)]
pub(crate) async fn execute_restart_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RestartJobRequest,
) -> Result<SessionInfo, String> {
    if !transaction.has_active_restore_capability() {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let (authority, root_directory) =
        prepare_restore_lifecycle_authority(transaction, Some(request.session_id), false).await?;
    execute_restart_transaction_in_context(
        transaction,
        request,
        &LifecycleExecutionContext::RestoreEntry {
            authority: &authority,
            root_directory: root_directory.as_deref(),
        },
    )
    .await
}

pub(crate) use crate::session::selection::LifecycleEntryOutcome;

pub(crate) async fn execute_external_restart_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RestartJobRequest,
    authority: &LifecycleEntryAuthority,
) -> LifecycleEntryOutcome {
    let result = execute_restart_transaction_in_context(
        transaction,
        request,
        &LifecycleExecutionContext::ExternalEntry(authority),
    )
    .await;
    if authority
        .interrupted
        .load(std::sync::atomic::Ordering::Acquire)
    {
        LifecycleEntryOutcome::Interrupted
    } else {
        LifecycleEntryOutcome::Returned(result)
    }
}

impl SelectionCoordinator {
    pub(crate) async fn restart_lifecycle<R: tauri::Runtime>(
        &self,
        app: AppHandle<R>,
        request: RestartJobRequest,
    ) -> Result<SessionInfo, String> {
        let executor = app
            .try_state::<LifecycleExecutorState<SelectionTransaction<R>>>()
            .map(|state| Arc::clone(state.get()));
        self.restart_lifecycle_with_executor(app, request, executor)
            .await
    }
}
