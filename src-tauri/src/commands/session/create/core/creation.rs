use crate::commands::session::create::admission::CreateCapacityContext;
use crate::commands::session::create::contracts::CreateSelectionIntent;
use crate::config::settings::SettingsState;
use crate::pty::backend::SessionBackendKind;
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::session::session::SessionCommunication;
use crate::session::session::SessionRepo;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Manager;
use uuid::Uuid;

pub(crate) struct CreateInput {
    pub(crate) session_label: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) session_name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) git_repos: Vec<SessionRepo>,
    pub(crate) skip_auto_resume: bool,
    pub(crate) pending_communication: Option<SessionCommunication>,
    pub(crate) enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
    pub(crate) coding_agent_binding: Option<crate::session::manager::CodingAgentBinding>,
    pub(crate) selection_intent: CreateSelectionIntent,
    pub(crate) create_target_replica_identity: Option<crate::path_identity::VerifiedPathIdentity>,
}
pub(crate) struct LaunchState {
    pub(crate) is_coordinator: bool,
    pub(crate) is_root_agent: bool,
    pub(crate) fresh_forced_by_mirror: bool,
    pub(crate) agent_kind: Option<CodingAgentKind>,
    pub(crate) resource_monitor: Arc<ResourceMonitorState>,
    pub(crate) resource_permit: Option<crate::resource_monitor::AgentLaunchPermit>,
    pub(crate) mgr: SessionManager,
    pub(crate) backend_kind: SessionBackendKind,
    pub(crate) container_path_context:
        Option<crate::commands::session::resume_probe::ContainerPathContext>,
    pub(crate) container_repos: Option<crate::pty::container_repos::RepoMountResolution>,
}
pub(crate) struct PendingState {
    pub(crate) is_coordinator: bool,
    pub(crate) is_root_agent: bool,
    pub(crate) fresh_forced_by_mirror: bool,
    pub(crate) agent_kind: Option<CodingAgentKind>,
    pub(crate) resource_monitor: Arc<ResourceMonitorState>,
    pub(crate) resource_permit: Option<crate::resource_monitor::AgentLaunchPermit>,
    pub(crate) mgr: SessionManager,
    pub(crate) container_path_context:
        Option<crate::commands::session::resume_probe::ContainerPathContext>,
    pub(crate) container_repos: Option<crate::pty::container_repos::RepoMountResolution>,
    pub(crate) session: crate::session::session::Session,
    pub(crate) pending_binding: crate::session::manager::PendingCreateBinding,
    pub(crate) spawn_mark: crate::pty::manager::SpawnMark,
}
pub(crate) struct MetadataInput {
    pub(crate) session_label: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) session_name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) skip_auto_resume: bool,
}

// #260: single detector (session/profile.rs). Replaces the old
// starts_with triple; this is the SAME call `strip_auto_injected_args`
// makes, so the persisted recipe and the runtime identity cannot disagree.
pub(crate) fn context_target(
    agent_kind: Option<CodingAgentKind>,
) -> Option<crate::config::session_context::ManagedContextTarget> {
    match agent_kind {
        Some(CodingAgentKind::Claude) => {
            Some(crate::config::session_context::ManagedContextTarget::Claude)
        }
        Some(CodingAgentKind::Codex) => {
            Some(crate::config::session_context::ManagedContextTarget::Codex)
        }
        Some(CodingAgentKind::Antigravity) => {
            Some(crate::config::session_context::ManagedContextTarget::Antigravity)
        }
        Some(CodingAgentKind::Pi) => Some(crate::config::session_context::ManagedContextTarget::Pi),
        // #1873 - Muse has no managed context target; a configured agent still
        // resolves its instructions filename (AGENTS.md) via `agent_command`.
        Some(CodingAgentKind::Muse) => None,
        None => None,
    }
}

type CreateTicket =
    std::sync::Arc<std::sync::Mutex<Option<crate::session::selection::CreateFinalizationTicket>>>;

pub(crate) async fn reserve_create_ticket<R: tauri::Runtime>(
    coordinator: &crate::session::selection::SelectionCoordinator,
    inline_transaction: Option<&crate::session::selection::SelectionTransaction<R>>,
    selection_intent: CreateSelectionIntent,
    owned_replacement: Option<&crate::session::selection::OwnedReplacementContinuation>,
    create_owner: &std::sync::Arc<
        std::sync::Mutex<crate::commands::session::create::admission::OwnedCreateAdmission>,
    >,
) -> Result<CreateTicket, String> {
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
    if let Some(continuation) = owned_replacement {
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
            coordinator: coordinator.clone(),
        });
    }
    Ok(create_ticket)
}

pub(crate) async fn resolve_coding_agent_binding<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    coding_agent_binding: Option<crate::session::manager::CodingAgentBinding>,
    agent_id: Option<&String>,
) -> Result<Option<crate::session::manager::CodingAgentBinding>, String> {
    Ok(match coding_agent_binding {
        Some(binding) => Some(binding),
        None => match agent_id {
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
    })
}

pub(crate) async fn apply_pending_binding<R: tauri::Runtime>(
    ctx: &crate::commands::session::create::contracts::CreateBodyContext<'_, R>,
    mgr: &SessionManager,
    carrier_u0: crate::session::manager::CarrierMutationPermit,
    pending_binding: crate::session::manager::PendingCreateBinding,
    session: &mut crate::session::session::Session,
    skip_auto_resume: &mut bool,
    coding_agent_binding: Option<crate::session::manager::CodingAgentBinding>,
) -> Result<Option<crate::session::manager::CarrierMutationPermit>, String> {
    let owned_replacement = ctx.owned_replacement;
    let inline_transaction = ctx.inline_transaction;
    let previous = match owned_replacement {
        Some(continuation) => match continuation.previous_for_launch(&carrier_u0) {
            Some(previous) => Some(previous),
            None => {
                drop(carrier_u0);
                return Ok(None);
            }
        },
        None => None,
    };
    if let Some(previous) = previous {
        // Carry is local launch input only. FinalizedOwned performs the
        // exact persisted carry after B's real finalization/version receipt.
        // The producer requests resume; A's unconsumed durable fresh intent
        // still determines the actual runtime argv before launch.
        *skip_auto_resume = previous.start_fresh_on_restore;
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
        return Err(error);
    }
    Ok(Some(carrier_u0))
}

pub(crate) async fn record_configured_spawn(
    mgr: &SessionManager,
    carrier_u1: &crate::session::manager::CarrierMutationPermit,
    pending_binding: crate::session::manager::PendingCreateBinding,
    session: &mut crate::session::session::Session,
    trusted_configured_spawn: bool,
) -> Result<(), String> {
    mgr.set_pending_trusted_configured_spawn_with_carrier_permit(
        carrier_u1,
        pending_binding,
        trusted_configured_spawn,
    )
    .await
    .map_err(|error| error.to_string())?;
    session.trusted_configured_spawn = trusted_configured_spawn;
    Ok(())
}

pub(crate) fn metadata_input(input: CreateInput) -> MetadataInput {
    let CreateInput {
        session_label,
        shell_args,
        session_name,
        agent_id,
        agent_label,
        skip_auto_resume,
        ..
    } = input;
    MetadataInput {
        session_label,
        shell_args,
        session_name,
        agent_id,
        agent_label,
        skip_auto_resume,
    }
}

pub(crate) async fn record_pending_designation(
    mgr: &SessionManager,
    carrier_u1: &crate::session::manager::CarrierMutationPermit,
    pending_binding: crate::session::manager::PendingCreateBinding,
    session: &mut crate::session::session::Session,
    fresh_forced_by_mirror: bool,
    is_root_agent: bool,
) -> Result<(), String> {
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
            carrier_u1,
            pending_binding,
            true,
        )
        .await
        .map_err(|error| error.to_string())?;
    }

    if is_root_agent {
        mgr.set_pending_is_root_agent_with_carrier_permit(carrier_u1, pending_binding, true)
            .await
            .map_err(|error| error.to_string())?;
        session.is_root_agent = true;
    }

    Ok(())
}

pub(crate) fn create_binding(
    create_manager: &SessionManager,
    owned_replacement: Option<&crate::session::selection::OwnedReplacementContinuation>,
) -> Option<(Uuid, crate::session::manager::PendingCreateBinding)> {
    Some(match owned_replacement {
        Some(continuation) => {
            let binding = continuation.binding()?;
            (continuation.operation(), binding)
        }
        None => (
            Uuid::new_v4(),
            create_manager.reserve_carrier_create_binding(),
        ),
    })
}

pub(crate) fn create_owner(
    preheld_target: bool,
    owned_replacement: Option<&crate::session::selection::OwnedReplacementContinuation>,
    pty_mgr: &Arc<Mutex<crate::pty::manager::PtyManager>>,
    create_target_key: Option<String>,
    cwd: &str,
    create_manager: &SessionManager,
    owned_target: Option<crate::commands::session::create::admission::OwnedCreateTargetLease>,
) -> Option<Arc<Mutex<crate::commands::session::create::admission::OwnedCreateAdmission>>> {
    let (create_nonce, create_binding) = create_binding(create_manager, owned_replacement)?;
    Some(Arc::new(Mutex::new(
        crate::commands::session::create::admission::OwnedCreateAdmission {
            preheld_target,
            owned_replacement: owned_replacement.cloned(),
            replacement_pty: owned_replacement.map(|_| pty_mgr.clone()),
            tracker: pty_mgr
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .response_close_state(),
            target: create_target_key.unwrap_or_else(|| format!("create-cwd:{}", cwd)),
            cwd: std::path::PathBuf::from(cwd),
            nonce: create_nonce,
            binding: create_binding,
            reservation: None,
            failed_foreign: false,
            owned_target,
            capacity: None,
            resource: None,
            blocked_bound: false,
            capacity_error: None,
        },
    )))
}

// Resolve mounts before creating a pending session so inadmissible entries
// cannot leave a session record, token, or container behind.
pub(crate) fn container_repos(
    context: Option<&crate::commands::session::resume_probe::ContainerPathContext>,
) -> Result<Option<crate::pty::container_repos::RepoMountResolution>, String> {
    context
        .map(|context| {
            crate::pty::container_repos::resolve_repo_mounts(std::path::Path::new(
                &context.host_root,
            ))
        })
        .transpose()
}

pub(crate) fn pending_state(
    state: LaunchState,
    session: crate::session::session::Session,
    pending_binding: crate::session::manager::PendingCreateBinding,
    spawn_mark: crate::pty::manager::SpawnMark,
) -> PendingState {
    PendingState {
        is_coordinator: state.is_coordinator,
        is_root_agent: state.is_root_agent,
        fresh_forced_by_mirror: state.fresh_forced_by_mirror,
        agent_kind: state.agent_kind,
        resource_monitor: state.resource_monitor,
        resource_permit: state.resource_permit,
        mgr: state.mgr,
        container_path_context: state.container_path_context,
        container_repos: state.container_repos,
        session,
        pending_binding,
        spawn_mark,
    }
}

pub(crate) fn pending_launch(
    input: MetadataInput,
    state: PendingState,
    context_target: Option<crate::config::session_context::ManagedContextTarget>,
    full_cmd: String,
    configured_identity: bool,
) -> crate::commands::session::create::contracts::PendingLaunch {
    crate::commands::session::create::contracts::PendingLaunch {
        agent_id: input.agent_id,
        agent_label: input.agent_label,
        agent_kind: state.agent_kind,
        is_coordinator: state.is_coordinator,
        is_root_agent: state.is_root_agent,
        fresh_forced_by_mirror: state.fresh_forced_by_mirror,
        skip_auto_resume: input.skip_auto_resume,
        pending_binding: state.pending_binding,
        shell_args: input.shell_args,
        container_path_context: state.container_path_context,
        container_repos: state.container_repos,
        context_target,
        full_cmd,
        configured_identity,
        session: state.session,
        spawn_mark: state.spawn_mark,
        mgr: state.mgr,
        resource_permit: state.resource_permit,
        resource_monitor: state.resource_monitor,
    }
}

pub(crate) fn take_marker_ticket<T>(ticket: &Mutex<Option<T>>) -> Option<T> {
    ticket
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
}
