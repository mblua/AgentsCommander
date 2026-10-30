pub(crate) use super::restart::context::ensure_owned_root_directory;
use super::restart::context::{ensure_lifecycle_root_directory, LifecycleExecutionContext};
use crate::commands::session::attach_local_config_telegram_if_any;
use crate::commands::session::build_configured_agent_spawn_for_cwd;
use crate::commands::session::create_session_inner_impl;
use crate::commands::session::prepare_restore_lifecycle_authority;
use crate::commands::session::resolve_root_agent_command;
use crate::commands::session::restart::execution::execute_restart_transaction_in_context;
use crate::commands::session::CreateSelectionIntent;
use crate::commands::session::LifecycleEntryAuthority;
use crate::commands::session::LifecycleEntryOutcome;
use crate::commands::session::RestartJobRequest;
use crate::config::settings::SettingsState;
use crate::pty::backend::ResolvedAgentHostShell;
use crate::pty::manager::PtyManager;
use crate::session::manager::LifecycleMutations;
use crate::session::manager::SessionManager;
use crate::session::selection::LifecycleExecutorState;
use crate::session::selection::SelectionCause;
use crate::session::selection::SelectionCoordinator;
use crate::session::selection::SelectionSource;
use crate::session::selection::SelectionTransaction;
use crate::session::selection::TrustedCreateIntent;
use crate::session::selection::TrustedRestartIntent;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use crate::telegram::manager::TelegramBridgeState;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Manager;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExistingRootAction {
    ReuseLive,
    WakeDormant,
    DiscardMissingPty,
}

pub(crate) fn classify_existing_root(status: &SessionStatus, has_pty: bool) -> ExistingRootAction {
    if matches!(status, SessionStatus::Exited(_)) {
        ExistingRootAction::WakeDormant
    } else if has_pty {
        ExistingRootAction::ReuseLive
    } else {
        ExistingRootAction::DiscardMissingPty
    }
}

pub(crate) use crate::session::selection::RootJobRequest;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_root_agent_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    tg_mgr: &TelegramBridgeState,
    settings: &SettingsState,
    requested_agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume_for_new_session: bool,
) -> Result<SessionInfo, String> {
    create_root_agent_inner_with_intent(
        app,
        session_mgr,
        pty_mgr,
        tg_mgr,
        settings,
        requested_agent_id,
        requested_profile,
        skip_auto_resume_for_new_session,
        TrustedCreateIntent::User,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_root_agent_inner_with_intent(
    app: &AppHandle,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: &Arc<Mutex<PtyManager>>,
    _tg_mgr: &TelegramBridgeState,
    _settings: &SettingsState,
    requested_agent_id: Option<String>,
    requested_profile: Option<String>,
    skip_auto_resume_for_new_session: bool,
    intent: TrustedCreateIntent,
    select_after: bool,
) -> Result<SessionInfo, String> {
    app.state::<SelectionCoordinator>()
        .root_lifecycle(
            app.clone(),
            RootJobRequest {
                requested_agent_id,
                requested_profile,
                skip_auto_resume_for_new_session,
                intent,
                select_after,
            },
        )
        .await
}

pub(crate) async fn execute_root_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RootJobRequest,
) -> Result<SessionInfo, String> {
    if !transaction.has_active_restore_capability() {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let (authority, root_directory) =
        prepare_restore_lifecycle_authority(transaction, None, true).await?;
    execute_root_transaction_in_context(
        transaction,
        request,
        &LifecycleExecutionContext::RestoreEntry {
            authority: &authority,
            root_directory: root_directory.as_deref(),
        },
    )
    .await
}

pub(crate) async fn execute_external_root_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RootJobRequest,
    authority: &LifecycleEntryAuthority,
) -> LifecycleEntryOutcome {
    let result = execute_root_transaction_in_context(
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

struct RootAgentLaunch {
    shell: String,
    shell_args: Vec<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    resolved_spawn: Option<crate::config::agent_command::AgentSpawnCommand>,
    resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
}

async fn resolve_root_agent_launch(
    settings: &SettingsState,
    root_agent_path: &str,
    request: &RootJobRequest,
) -> Result<RootAgentLaunch, String> {
    let last_coding_agent = crate::config::root_agent::read_last_coding_agent(root_agent_path);
    let (shell, shell_args, agent_id, agent_label) = {
        let settings = settings
            .current_read()
            .await
            .map_err(|error| error.to_string())?;
        resolve_root_agent_command(
            &settings,
            request.requested_agent_id.as_deref(),
            last_coding_agent.as_deref(),
        )?
    };
    let (resolved_spawn, resolved_agent_host_shell) = if let Some(agent_id) = agent_id.as_deref() {
        // #1271 - build the spawn and copy the configured default host shell
        // from the SAME single guard, before any await, so the pair can never
        // mix across a configuration change (Phase 1 items 1-2; mirrors the
        // restart pattern).
        let settings = settings
            .current_read()
            .await
            .map_err(|error| error.to_string())?;
        let spawn = build_configured_agent_spawn_for_cwd(
            &settings,
            agent_id,
            root_agent_path,
            request.requested_profile.as_deref(),
        )?;
        let host_shell = Some(ResolvedAgentHostShell {
            program: settings.default_shell.clone(),
            args: settings.default_shell_args.clone(),
        });
        (spawn, host_shell)
    } else {
        (None, None)
    };
    let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
        (
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            Some(spawn.trusted_agent_label.clone()),
        )
    } else {
        (shell, shell_args, agent_label)
    };
    Ok(RootAgentLaunch {
        shell,
        shell_args,
        agent_id,
        agent_label,
        resolved_spawn,
        resolved_agent_host_shell,
    })
}

pub(crate) async fn execute_root_transaction_in_context<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: RootJobRequest,
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
    let root_agent_path = ensure_lifecycle_root_directory(transaction, context).await?;
    let existing = session_mgr
        .read()
        .await
        .list_sessions()
        .await
        .into_iter()
        .find(|session| {
            session.is_root_agent
                || crate::config::root_agent::is_root_agent_path(&session.working_directory)
        });

    if let Some(existing) = existing {
        let session_id = Uuid::parse_str(&existing.id).map_err(|error| error.to_string())?;
        session_mgr
            .read()
            .await
            .set_is_root_agent(session_id, true)
            .await;
        let has_pty = pty_mgr
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .has_session(session_id);
        match classify_existing_root(&existing.status, has_pty) {
            ExistingRootAction::ReuseLive => {
                // Reuse has no new binding/backend or replacement effects. Track
                // its actual selection/persistence/publication and final lookup.
                let publication = transaction.start_lifecycle_effect(
                    crate::session::selection::LifecycleEffectSlot::Publication,
                );
                if transaction.has_lifecycle_entry() && publication.is_none() {
                    return std::future::pending().await;
                }
                let result = async {
                    if request.select_after {
                        let decision = transaction
                            .live_decision(session_id)
                            .ok_or_else(|| "Root session has no displayable PTY".to_string())?;
                        let committed = transaction
                            .commit(
                                decision,
                                SelectionCause::UserSwitch,
                                LifecycleMutations::default(),
                            )
                            .await?;
                        if let Some(selection) = committed.selection.as_ref() {
                            transaction
                                .persist(SelectionSource::UserSwitch, Some(session_id))
                                .await;
                            transaction.publish_selection(selection);
                        }
                    }
                    session_mgr
                        .read()
                        .await
                        .get_session(session_id)
                        .await
                        .map(|session| SessionInfo::from(&session))
                        .ok_or_else(|| "Root session disappeared during reuse".to_string())
                }
                .await;
                if let Some(publication) = publication {
                    publication.complete(
                        result
                            .as_ref()
                            .map(|info| Some(info.id.clone()))
                            .map_err(Clone::clone),
                    );
                    transaction.observe_reuse_live_result(&result);
                }
                return result;
            }
            ExistingRootAction::WakeDormant | ExistingRootAction::DiscardMissingPty => {
                let wake_dormant = matches!(
                    classify_existing_root(&existing.status, has_pty),
                    ExistingRootAction::WakeDormant
                );
                let restart_intent = match request.intent {
                    TrustedCreateIntent::User => TrustedRestartIntent::User,
                    TrustedCreateIntent::Background => TrustedRestartIntent::Background,
                };
                return execute_restart_transaction_in_context(
                    transaction,
                    RestartJobRequest {
                        session_id,
                        agent_id: request.requested_agent_id,
                        requested_profile: request.requested_profile,
                        skip_auto_resume: Some(if wake_dormant {
                            false
                        } else {
                            request.skip_auto_resume_for_new_session
                        }),
                        activate_after: request.select_after,
                        intent: restart_intent,
                        communication_override: None,
                        enforcement:
                            crate::config::sessions_persistence::default_creation_gate_enforcement(
                            ),
                    },
                    context,
                )
                .await;
            }
        }
    }

    let RootAgentLaunch {
        shell,
        shell_args,
        agent_id,
        agent_label,
        resolved_spawn,
        resolved_agent_host_shell,
    } = resolve_root_agent_launch(&settings, &root_agent_path, &request).await?;
    let target_ownership = context
        .authority()
        .map(|authority| authority.target_ownership())
        .transpose()?
        .flatten();
    let info = create_session_inner_impl(
        app,
        &session_mgr,
        &pty_mgr,
        shell,
        shell_args,
        root_agent_path.clone(),
        Some(crate::config::root_agent::ROOT_AGENT_SESSION_NAME.to_string()),
        agent_id,
        agent_label,
        false,
        Vec::new(),
        request.skip_auto_resume_for_new_session,
        resolved_spawn,
        resolved_agent_host_shell,
        None,
        CreateSelectionIntent::Suppress,
        None,
        None,
        Some(transaction),
        Some(SelectionCause::SessionCreated(request.intent)),
        false,
        target_ownership.as_ref(),
        crate::config::sessions_persistence::default_creation_gate_enforcement(),
        None,
        context.authority(),
        None,
        None,
    )
    .await?
    .into_finalized()
    .await?;
    let session_id = Uuid::parse_str(&info.id).map_err(|error| error.to_string())?;
    if request.select_after {
        let decision = transaction
            .live_decision(session_id)
            .ok_or_else(|| "new Root session has no displayable PTY".to_string())?;
        let committed = transaction
            .commit(
                decision,
                SelectionCause::SessionCreated(request.intent),
                LifecycleMutations::default(),
            )
            .await?;
        if let Some(selection) = committed.selection.as_ref() {
            transaction
                .persist(SelectionSource::SessionCreated, Some(session_id))
                .await;
            transaction.publish_selection(selection);
        }
    }
    attach_local_config_telegram_if_any(app, session_id, &root_agent_path).await;
    Ok(info)
}

impl SelectionCoordinator {
    pub(crate) async fn root_lifecycle<R: tauri::Runtime>(
        &self,
        app: AppHandle<R>,
        request: RootJobRequest,
    ) -> Result<SessionInfo, String> {
        let executor = app
            .try_state::<LifecycleExecutorState<SelectionTransaction<R>>>()
            .map(|state| Arc::clone(state.get()));
        self.root_lifecycle_with_executor(app, request, executor)
            .await
    }
}
