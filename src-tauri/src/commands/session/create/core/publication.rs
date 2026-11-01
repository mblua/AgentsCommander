use crate::commands::session::agent_command::mirror_forces_fresh;
use crate::commands::session::agent_command::prepare_last_coding_agent_write;
use crate::commands::session::agent_command::save_last_coding_agent;
use crate::commands::session::co_managed::raise_room_reader_demand;
use crate::commands::session::create::admission::acquire_lifecycle_create_publication_region;
use crate::commands::session::create::admission::release_resource_launch_permit;
use crate::commands::session::create::admission::with_create_resource_region;
use crate::commands::session::create::contracts::CreateBodyContext;
use crate::commands::session::create::contracts::CreateCompletion;
use crate::commands::session::create::contracts::DeferredCreateOutput;
use crate::commands::session::create::contracts::PublicationInput;
use crate::commands::session::create::publication::register_session_samplers;
use crate::commands::session::lifecycle::LifecycleEntryAuthority;
use crate::commands::session::resume_probe::build_title_prompt_appendage;
use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use crate::session::selection::SelectionCause;
use crate::session::selection::SelectionCoordinator;
use crate::session::selection::SelectionTransaction;
use crate::session::session::SessionInfo;
use crate::session::warnings::SessionWarning;
use std::sync::Arc;
use tauri::AppHandle;
use tauri::Manager;

pub(crate) async fn complete<'a: 'b, 'b, R: tauri::Runtime>(
    ctx: &'b CreateBodyContext<'a, R>,
    input: PublicationInput,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        app,
        pty_mgr,
        cwd,
        inline_transaction,
        defer_inline_finalization,
        pending_start_fresh,
        initial_last_prompt,
        owned_replacement,
        lifecycle_domain,
        create_owner,
        ..
    } = ctx;
    let PublicationInput {
        resource_monitor,
        mut resource_permit,
        mgr,
        session,
        agent_id,
        agent_label,
        agent_kind: _agent_kind,
        is_coordinator,
        is_root_agent: _is_root_agent,
        fresh_forced_by_mirror,
        skip_auto_resume,
        configured_identity,
        mut effective_continuation,
        pending_binding,
        session_env_warnings,
    } = input;
    let id = session.id;
    if let Some(continuation) = owned_replacement {
        let actual = mgr.get_pending_session(pending_binding).await;
        let generation = pty_mgr
            .lock()
            .ok()
            .and_then(|pty| pty.route_identities(id).map(|route| route.2));
        if !actual
            .zip(generation)
            .is_some_and(|(row, generation)| continuation.record_launch(&row, generation))
        {
            return Ok(CreateCompletion::Retained);
        }
    }
    register_session_samplers(app, id, agent_id.clone());

    maybe_spawn_coordinator_title(app, cwd, id, agent_id.is_some(), is_coordinator);

    let finalization_warnings = session_env_warnings
        .iter()
        .map(|warning| {
            SessionWarning::new(
                id,
                warning.key.clone(),
                warning.kind,
                warning.message.clone(),
            )
        })
        .collect::<Vec<_>>();

    // 0.8.0: removed the "Show the terminal window when a session is created" branch.
    // Under the unified-window model the main window is created up-front and stays
    // visible; session creation has no window-show responsibility.

    publish_tooling(ctx, &session, &agent_id, &agent_label).await;

    // A configured selector is never evidence of AC-attributed continuation,
    // even when an injector also happened to add a control of its own.
    effective_continuation &= !configured_identity;
    let prompt_fresh = !configured_identity
        && (!effective_continuation
            || skip_auto_resume
            || pending_start_fresh == Some(true)
            || fresh_forced_by_mirror);
    let (key, initial) = prompt_initialization(cwd, &session, initial_last_prompt);
    let carrier_prompt = match with_create_resource_region(
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
    mgr.initialize_pending_prompt_with_carrier_permit(
        &carrier_prompt,
        pending_binding,
        key,
        initial,
        effective_continuation,
        prompt_fresh,
        !defer_inline_finalization,
    )
    .await?;
    drop(carrier_prompt);

    finalize(
        ctx,
        &mgr,
        pending_binding,
        finalization_warnings,
        effective_continuation,
        id,
    )
    .await
}

async fn finalize<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    mgr: &SessionManager,
    pending_binding: crate::session::manager::PendingCreateBinding,
    finalization_warnings: Vec<SessionWarning>,
    effective_continuation: bool,
    id: uuid::Uuid,
) -> Result<CreateCompletion, String> {
    let &CreateBodyContext {
        app,
        inline_transaction,
        inline_cause,
        defer_inline_finalization,
        owned_replacement,
        create_owner,
        create_ticket,
        inline_pending_for_body,
        ..
    } = ctx;
    if defer_inline_finalization {
        if create_ticket
            .lock()
            .map_err(|_| "create ticket unavailable".to_string())?
            .is_some()
            || inline_transaction.is_none()
        {
            return Err("deferred create finalization requires an inline transaction".to_string());
        }
        let info = mgr
            .get_pending_session(pending_binding)
            .await
            .map(|record| SessionInfo::from(&record))
            .ok_or_else(|| "deferred create pending record disappeared".to_string())?;
        inline_pending_for_body
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        return Ok(CreateCompletion::Deferred(DeferredCreateOutput {
            info,
            binding: pending_binding,
            warnings: finalization_warnings,
            _create_admission: create_owner.clone(),
            effective_continuation,
        }));
    }

    let finalization_ticket = create_ticket
        .lock()
        .map_err(|_| "create ticket unavailable".to_string())?
        .take();
    let finalized = if let Some(ticket) = finalization_ticket {
        ticket
            .finalize_with_owned_outcome(finalization_warnings)
            .await
    } else {
        let transaction = inline_transaction
            .ok_or_else(|| "inline create transaction is unavailable".to_string())?;
        let cause = inline_cause.ok_or_else(|| "inline create cause is unavailable".to_string())?;
        transaction
            .finalize_inline_create(
                pending_binding,
                cause,
                finalization_warnings,
                owned_replacement.cloned(),
            )
            .await
    };
    let Some(finalized) = finalized else {
        return Ok(CreateCompletion::Retained);
    };
    if finalized.is_ok() && inline_transaction.is_some() {
        inline_pending_for_body
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }
    let completion = finalized.map(CreateCompletion::Finalized);
    // #2232 phase 4 section 5.1: the Room demand may be raised only once
    // the row is **finalized and visible** to `get_session` — a
    // pending-create row is hidden, so a raise spawned earlier answers
    // "session not found" and is never retried. Detached, as before: the
    // raise resolves the reader kind and spawns the watcher under its own
    // locks, off the create path. A restart replacement is finalized later
    // by `execute_restart_transaction`, which raises its Room demand
    // synchronously after the commit (section 5.2); `SelectionCause::Restart(_)`
    // never reaches a finalize here, and the gate keeps that explicit.
    if completion.is_ok() && !matches!(inline_cause, Some(SelectionCause::Restart(_))) {
        let app_for_demand = app.clone();
        tauri::async_runtime::spawn(async move {
            raise_room_reader_demand(&app_for_demand, id).await;
        });
        if let Some(transaction) = inline_transaction {
            transaction.observe_entry_reader_demand(
                pending_binding,
                crate::session::selection::EntryReaderDemandReceipt::LegacyTaskAccepted,
            );
        }
    }
    completion
}

fn maybe_spawn_coordinator_title<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    cwd: &str,
    id: uuid::Uuid,
    has_agent: bool,
    is_coordinator: bool,
) {
    // Auto-inject optional non-credential bootstrap text for agent sessions
    // after PTY spawn. Credentials are already present in child environment
    // variables; no credentials are written through PTY.
    //
    // Currently the only bootstrap payload is the Coordinator auto-title prompt.
    if has_agent && !is_coordinator {
        log::debug!(
            "[session] No bootstrap injection for non-coordinator agent session {}",
            id
        );
    }
    if has_agent && is_coordinator {
        let auto_title_enabled = false; /*
                                            let settings_state = app.state::<SettingsState>();
                                            let cfg = settings_state.read().await;
                                            cfg.auto_generate_task_title
                                        };*/

        if auto_title_enabled {
            let app_clone = app.clone();
            let session_id = id;
            let cwd_clone = cwd.to_owned();

            tokio::spawn(async move {
                let prompt = match build_title_prompt_appendage(&cwd_clone) {
                    Ok(Some(prompt)) => {
                        log::info!(
                            "[session] Auto-title prompt built for session {}",
                            session_id
                        );
                        prompt
                    }
                    Ok(None) => {
                        log::info!(
                            "[session] Auto-title prompt skipped (gate not passed) for session {}",
                            session_id
                        );
                        return;
                    }
                    Err(e) => {
                        log::warn!(
                            "[session] Auto-title prompt skipped for session {}: {}",
                            session_id,
                            e
                        );
                        return;
                    }
                };

                if !wait_for_title_prompt_ready(&app_clone, session_id).await {
                    return;
                }

                match crate::pty::inject::inject_text_into_session(&app_clone, session_id, &prompt)
                    .await
                {
                    Ok(()) => {
                        log::info!(
                            "[session] Auto-title prompt injected for session {}",
                            session_id
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "[session] Failed to inject auto-title prompt for {}: {}",
                            session_id,
                            e
                        );
                    }
                }
            });
        }
    }
}

async fn wait_for_title_prompt_ready<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: uuid::Uuid,
) -> bool {
    let max_wait = std::time::Duration::from_secs(30);
    let poll = std::time::Duration::from_millis(500);
    let start = std::time::Instant::now();

    loop {
        if start.elapsed() >= max_wait {
            log::warn!(
                "[session] Timeout waiting for idle before auto-title prompt injection \
             for session {}",
                session_id
            );
            break;
        }
        tokio::time::sleep(poll).await;

        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        match sessions.iter().find(|s| s.id == session_id.to_string()) {
            Some(s) if s.waiting_for_input => break,
            Some(_) => {}
            None => {
                log::warn!(
                    "[session] Session {} gone before auto-title prompt injection",
                    session_id
                );
                return false;
            }
        }
    }

    true
}

async fn publish_tooling<R: tauri::Runtime>(
    ctx: &CreateBodyContext<'_, R>,
    session: &crate::session::session::Session,
    agent_id: &Option<String>,
    agent_label: &Option<String>,
) {
    let &CreateBodyContext {
        app,
        cwd,
        skip_tooling_save,
        ..
    } = ctx;
    let id = session.id;
    // Save lastCodingAgent + codingAgents (skip for temp sessions)
    if !skip_tooling_save {
        if let Some(ref aid) = agent_id {
            // Resolve label: use provided agent_label, or look up from settings by agent_id.
            // Without this fallback, callers that pass agent_id but no label (session-requests,
            // web remote) would write app: "Unknown" into the per-instance config.json.
            // #2433 - computed under the settings read lock, written after it drops.
            let write = {
                let settings = app.state::<SettingsState>();
                let write = match settings.current_read().await {
                    Ok(cfg) => Some(prepare_last_coding_agent_write(
                        aid,
                        agent_label.as_deref(),
                        &cfg,
                    )),
                    Err(error) => {
                        log::debug!("[session] skipping lastCodingAgent publication: {error}");
                        None
                    }
                };
                write
            };
            if let Some(write) = write {
                if let Err(e) = save_last_coding_agent(cwd, &id.to_string(), &write) {
                    log::warn!("Failed to save lastCodingAgent: {}", e);
                }
            }
            // #592 - persist the loaded profile content-hash so drift survives an
            // AC restart. Best-effort; never aborts the spawn. No-op off-replica.
            if let Some(hash) = session.profile_content_hash.as_deref() {
                log::debug!(
                    "[profile-hash] spawn-persist: session={} agent={} hash={} cwd={:?}",
                    id,
                    aid,
                    hash,
                    cwd,
                );
                if let Err(e) =
                    crate::config::coding_agent_profiles::set_replica_profile_content_hash(
                        std::path::Path::new(&cwd),
                        hash,
                    )
                {
                    log::warn!("Failed to persist profileContentHash: {}", e);
                }
            }
        }
    }
}

pub(crate) async fn await_startup<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    inline_transaction: Option<&SelectionTransaction<R>>,
    lifecycle_domain: Option<&LifecycleEntryAuthority>,
) -> Result<(), String> {
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
    Ok(())
}

pub(crate) fn mirror_fresh_intent<R: tauri::Runtime>(
    app: &AppHandle<R>,
    cwd: &str,
    is_coordinator: bool,
    mut skip_auto_resume: bool,
) -> (bool, bool) {
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
            let fqn = crate::config::teams::agent_fqn_from_path(cwd);
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

    (skip_auto_resume, fresh_forced_by_mirror)
}

fn prompt_initialization(
    cwd: &str,
    session: &crate::session::session::Session,
    initial_last_prompt: Option<&crate::commands::session::create::contracts::InitialLastPrompt>,
) -> (Option<crate::session::manager::PromptKey>, Option<String>) {
    let key = if session.backend_kind == crate::pty::backend::SessionBackendKind::LocalProcess {
        crate::config::sessions_persistence::prompt_key(
            cwd,
            session.agent_id.as_deref(),
            session.requested_profile.as_deref(),
            session.effective_profile.as_deref(),
        )
    } else {
        None
    };
    let initial = initial_last_prompt
        .as_ref()
        .filter(|source| source.compatible(session))
        .and_then(|source| source.text.clone());
    let revalidated = crate::config::sessions_persistence::prompt_key(
        cwd,
        session.agent_id.as_deref(),
        session.requested_profile.as_deref(),
        session.effective_profile.as_deref(),
    );
    let key = key.filter(|candidate| Some(candidate) == revalidated.as_ref());
    (key, initial)
}
