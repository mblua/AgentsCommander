//! Validates outbox actions and modes and delivers peer wakes and cold text.
//! Owns peer replacement wake delivery, including snapshot and source-currency checks.

use super::handoff::resolve_remote_pty_command;
use super::notice::{
    internal_system_envelope, parse_remote_pty_command, sender_name_for_session_cwd_with_root_flag,
    OutboxOrigin, WakeDeliveryOrigin, ERR_INVALID_DISPATCH_PROFILE,
};
use super::poller::{
    check_wake_deadline, log_wake_ended_without_input, owned_replacement_launch_recipe,
    MailboxPoller, OutboxAuthenticationFacts, OutboxStage, OwnedPeerTarget, OwnedPeerWakeExecution,
    OwnedPeerWakeStep, RetainedExit, ValidatedWake, WakeDispatch, ERR_WAKE_DELIVERY_TIMEOUT,
    ERR_WAKE_RETAINED, WAKE_ATTEMPT,
};
#[cfg(test)]
use super::pty_claim::MailboxTestHooks;
use super::pty_claim::{ResponseWakeWork, WakeDelivery};
#[cfg(test)]
use super::tests::{fixture_capture_owner, run_owned_persist_core};
#[cfg(not(test))]
use crate::config::sessions_persistence::run_owned_persist_core;
use crate::config::sessions_persistence::PublishedCarrier;
use crate::config::teams;
use crate::path_identity::VerifiedPathIdentity;
use crate::phone::mailbox::host::MailboxHost;
use crate::phone::types::OutboxMessage;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::create_contracts::OwnedReplacementCreateRequest;
use crate::session::manager::SessionManager;
use crate::session::selection::{
    OwnedReplacementContinuation, OwnedReplacementWork, PreparedOwnedReplacement,
    ReplacementAdmissionStep, ResolvedReplacement, SelectionCoordinator, SourceExecutorGuard,
};
use crate::session::session::Session;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[cfg(test)]
mod tests;

fn owned_replacement_work(msg: &OutboxMessage) -> Result<OwnedReplacementWork, String> {
    let payload = serde_json::to_value(msg).map_err(|error| error.to_string())?;
    let work = OwnedReplacementWork::OrdinaryOwned {
        message_id: msg.id.clone(),
        body: msg.body.clone(),
        payload: payload.clone(),
        recipient: msg.to.clone(),
        request: payload,
    };
    Ok(work)
}

fn owned_replacement_directories(
    request: &OwnedReplacementCreateRequest,
) -> Result<(VerifiedPathIdentity, VerifiedPathIdentity), String> {
    let physical = crate::path_identity::verify_directory(Path::new(&request.cwd))
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    let replica = teams::strict_wg_replica_anchor_from_cwd(Path::new(&request.cwd))
        .ok()
        .flatten()
        .and_then(|root| crate::path_identity::verify_directory(&root).ok())
        .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
    Ok((physical, replica))
}

fn owned_replacement_request(
    plan: super::notice::ResolvedWakeSpawnPlan,
    skip_tooling_save: bool,
) -> (
    OwnedReplacementCreateRequest,
    super::notice::ResolvedWakeAgentCommand,
) {
    let request = OwnedReplacementCreateRequest {
        shell: plan.spawn_shell,
        shell_args: plan.spawn_args,
        cwd: crate::path_utils::normalize_windows_verbatim_path(&plan.cwd),
        name: Some(plan.session_name),
        agent_id: plan.resolved_command.agent_id.clone(),
        agent_label: plan.spawn_label,
        skip_tooling_save,
        git_repos: Vec::new(),
        resolved_spawn: plan.configured_spawn,
        host_shell: plan.resolved_agent_host_shell,
    };
    (request, plan.resolved_command)
}

async fn owned_snapshot(
    manager: &SessionManager,
    directory: &Path,
    msg: &OutboxMessage,
    physical: &VerifiedPathIdentity,
    previous_id: Uuid,
) -> (bool, Option<(PublishedCarrier, Session)>) {
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let captured = manager
        .with_current_carrier_snapshot(&permit, |current| {
            let scope = crate::config::sessions_persistence::classify_lifecycle_carrier_scope(
                &permit,
                Some(&msg.to),
                &physical.canonical_path,
            );
            let foreign = matches!(
                scope,
                crate::config::sessions_persistence::CreateCarrierScope::Foreign { .. }
            );
            let carrier = crate::config::sessions_persistence::retained_carrier_for_replacement(
                &permit,
                directory,
                &msg.to,
                previous_id,
                current,
            );
            let previous = current
                .rows
                .iter()
                .find(|row| row.id == previous_id)
                .cloned();
            let old_missing = previous.is_none();
            (foreign || old_missing, carrier.zip(previous))
        })
        .await;
    drop(permit);
    captured
}

type WakeAdmissionContext<'a> = (
    &'a SelectionCoordinator,
    &'a SessionManager,
    &'a Arc<Mutex<PtyManager>>,
);

async fn recover_peer_wake_precommit(
    (coordinator, manager, pty): WakeAdmissionContext<'_>,
    target: &str,
    previous: Uuid,
    request: &OwnedReplacementCreateRequest,
    (physical, replica): (&VerifiedPathIdentity, &VerifiedPathIdentity),
) {
    coordinator
        .recover_wake_precommit(
            target,
            previous,
            (&physical.clone().into(), &replica.clone().into()),
            &|| {
                owned_replacement_directories(request).is_ok_and(|(cwd, root)| {
                    cwd.canonical_path == physical.canonical_path
                        && cwd.object_id == physical.object_id
                        && root.canonical_path == replica.canonical_path
                        && root.object_id == replica.object_id
                })
            },
            manager,
            pty,
        )
        .await;
    #[cfg(test)]
    OwnedReplacementContinuation::fixture_after_wake_probe().await;
}

fn resolve_replacement(
    msg: &OutboxMessage,
    request: &OwnedReplacementCreateRequest,
    directory: &Path,
    (carrier, previous): (PublishedCarrier, Session),
    work: &OwnedReplacementWork,
    (physical, replica): (&VerifiedPathIdentity, &VerifiedPathIdentity),
) -> ResolvedReplacement {
    ResolvedReplacement {
        target: msg.to.clone(),
        directory: directory.to_path_buf(),
        previous,
        carrier,
        requested_agent_id: request.agent_id.clone(),
        requested_profile: msg.requested_profile.clone(),
        shell: request.shell.clone(),
        shell_args: request.shell_args.clone(),
        launch_recipe: owned_replacement_launch_recipe(request),
        cwd: request.cwd.clone(),
        backend: SessionBackendKind::LocalProcess,
        work: work.clone(),
        cwd_identity: physical.clone().into(),
        replica_identity: Some(replica.clone().into()),
    }
}

async fn admit_peer_wake_carrier(
    (coordinator, manager, pty): WakeAdmissionContext<'_>,
    candidate: PreparedOwnedReplacement,
    source: &mut SourceExecutorGuard,
) -> Result<ReplacementAdmissionStep, Option<String>> {
    let tracker = pty
        .lock()
        .map_err(|_| Some("targetCreateGateUnavailable".to_string()))?
        .response_close_state();
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    let admission = manager
        .with_current_carrier_snapshot(&permit, |current| {
            coordinator
                .admit_owned_wake_replacement(&permit, current, &tracker, pty, candidate, source)
        })
        .await;
    drop(permit);
    admission.map_err(|error| error.admission_error())
}

async fn owned_peer_completion(
    result: Result<OwnedPeerWakeStep, String>,
    completion: &OwnedReplacementContinuation,
    manager: &SessionManager,
    pty: &Arc<Mutex<PtyManager>>,
) -> Result<OwnedPeerWakeStep, String> {
    #[cfg(test)]
    if matches!(&result, Ok(OwnedPeerWakeStep::Retained(_))) {
        // The accepted core has actually returned; its public wake caller
        // now ends with an error (#3263). This is observation only.
        let _ = completion
            .fixture_reached(crate::session::selection::ReplacementFixtureCut::SourceRetainedReturn)
            .await;
    }
    completion
        .settle_wake_precommit(
            manager,
            pty,
            result.as_ref().err().map(String::as_str),
            None,
        )
        .await;
    result
}

fn log_core_end(result: &Result<OwnedPeerWakeStep, String>, id: &str, to: &str, previous: Uuid) {
    if let Err(error) = result {
        if error.starts_with(ERR_WAKE_RETAINED) {
            log_wake_ended_without_input(id, to, Some(previous), error);
        }
    }
}

fn retained_wake_error(msg: &OutboxMessage, previous: Uuid, exit: &RetainedExit) -> String {
    if exit.stage == "deadline" {
        return ERR_WAKE_DELIVERY_TIMEOUT.into();
    }
    let error = format!("{ERR_WAKE_RETAINED}: stage={} rollback=none", exit.stage);
    log_wake_ended_without_input(
        &msg.id,
        &msg.to,
        Some(exit.session.unwrap_or(previous)),
        &error,
    );
    error
}

impl MailboxPoller {
    pub(crate) async fn validate_outbox_action_and_mode<S: MailboxHost>(
        &self,
        app: &S,
        path: &Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> OutboxStage {
        if let Some(ref action) = msg.action {
            match action.as_str() {
                "close-session" => {
                    // §224 G-IMPL-2: The CLI never polls master-token instance responses.
                    return Ok(std::ops::ControlFlow::Break(
                        self.handle_close_session(app, path, msg, is_app_outbox)
                            .await,
                    ));
                }
                _ => {
                    return Ok(std::ops::ControlFlow::Break(
                        self.reject_message(path, msg, &format!("Unknown action '{action}'"))
                            .await,
                    ));
                }
            }
        }

        let mode = if msg.mode.is_empty() {
            "wake"
        } else {
            msg.mode.as_str()
        };
        // Reject malformed external root-token outbox modes; wake is the only supported mode.
        if mode != "wake" {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(
                    path,
                    msg,
                    &format!("Unsupported delivery mode '{mode}'. Valid: wake"),
                )
                .await,
            ));
        }
        Ok(std::ops::ControlFlow::Continue(()))
    }

    pub(crate) async fn deliver_outbox_tail<S: MailboxHost>(
        &self,
        app: &S,
        path: &Path,
        msg: OutboxMessage,
        content: &str,
        delivery: (OutboxOrigin, Option<&mut Option<ValidatedWake>>),
        authentication: OutboxAuthenticationFacts,
    ) -> Result<(), String> {
        let (origin, deferred_tail) = delivery;
        let (is_master, _, _, token_belongs_to_root_agent, saw_session_token) = authentication;
        let response = self
            .admit_response_source(
                app,
                path,
                &msg,
                saw_session_token
                    && !is_master
                    && !token_belongs_to_root_agent
                    && origin != OutboxOrigin::CoManaged
                    && msg.command.is_none()
                    && !msg.get_output
                    && msg.pty_input.is_none(),
                content,
            )
            .await;
        let validated = ValidatedWake {
            message: msg,
            response,
        };
        let _candidate_disposal = validated
            .response
            .as_ref()
            .map(ResponseWakeWork::candidate_disposal);
        // #2232 phase 7: Inline co-managed wakes add display attribution.
        let comanaged = origin == OutboxOrigin::CoManaged;
        if !comanaged {
            if let Some(slot) = deferred_tail {
                *slot = Some(validated);
                return Ok(());
            }
        }
        // Both inline origins bind their actual JSON without changing scanner ownership.
        let mut delivery = self.delivery_poller();
        delivery.codex_clear_source = Some(path.to_path_buf());
        match delivery
            .deliver_validated_wake(app, &validated, comanaged)
            .await
        {
            Ok(annotated) => self.move_to_delivered(app, path, &annotated).await,
            Err(error) => {
                if delivery.is_terminal_clear_error(&validated.message, &error) {
                    if let Err(publication) =
                        self.reject_message(path, &validated.message, &error).await
                    {
                        log::warn!(
                            "[mailbox] terminal clear reason {error}; rejection: {publication}"
                        );
                    }
                }
                Err(error)
            }
        }
    }

    /// Deliver to live stdin, respawn Exited or spawn if absent. Busy stdin is valid.
    // #791: The daemon reuses this path without poll interval or retry state.
    pub(crate) async fn deliver_wake<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
    ) -> Result<OutboxMessage, String> {
        self.deliver_wake_with_origin(app, msg, WakeDeliveryOrigin::FilesystemPoller)
            .await
    }

    pub(crate) async fn deliver_wake_with_origin<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        origin: WakeDeliveryOrigin,
    ) -> Result<OutboxMessage, String> {
        self.deliver_wake_engine(
            app,
            WakeDelivery::Peer {
                message: msg,
                origin,
                comanaged: false,
                response: None,
            },
        )
        .await
    }

    /// #2232 phase 7: Co-managed queue attribution affects display only.
    async fn deliver_wake_with_attribution<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        origin: WakeDeliveryOrigin,
        comanaged: bool,
        response: Option<&ResponseWakeWork>,
    ) -> Result<OutboxMessage, String> {
        self.deliver_wake_engine(
            app,
            WakeDelivery::Peer {
                message: msg,
                origin,
                comanaged,
                response,
            },
        )
        .await
    }

    /// Deliver Peer or validated InternalSystem wake and return its annotated receipt.
    pub(crate) async fn deliver_validated_wake<S: MailboxHost>(
        &self,
        app: &S,
        validated: &ValidatedWake,
        comanaged: bool,
    ) -> Result<OutboxMessage, String> {
        if !comanaged && validated.response.is_none() {
            return self.deliver_wake(app, &validated.message).await;
        }
        self.deliver_wake_with_attribution(
            app,
            &validated.message,
            WakeDeliveryOrigin::FilesystemPoller,
            comanaged,
            validated.response.as_ref(),
        )
        .await
    }

    pub(crate) async fn deliver_wake_engine<S: MailboxHost>(
        &self,
        app: &S,
        delivery: WakeDelivery<'_>,
    ) -> Result<OutboxMessage, String> {
        let target_fqn = match &delivery {
            WakeDelivery::Peer { message, .. } => message.to.as_str(),
            WakeDelivery::InternalSystem { target, .. } => target.fqn(),
        };
        let _response_delivery = match app.message_store() {
            Some(state) => match &state.store {
                Ok(store) => Some(store.begin_response_delivery(target_fqn)?),
                Err(_) => None,
            },
            None => None,
        };
        match delivery {
            WakeDelivery::Peer {
                message,
                origin,
                comanaged,
                response,
            } => {
                let delivery: std::pin::Pin<
                    Box<
                        dyn std::future::Future<Output = Result<OutboxMessage, String>> + Send + '_,
                    >,
                > = Box::pin(self.deliver_peer_wake(app, message, origin, comanaged, response));
                delivery.await
            }
            WakeDelivery::InternalSystem {
                target,
                notice,
                cancellation,
                guard,
            } => {
                // Capture the flag-free receipt before delivery consumes the target.
                let delivered = internal_system_envelope(&target);
                self.deliver_internal_system_wake(app, target, notice, cancellation, guard)
                    .await?;
                Ok(delivered)
            }
        }
    }

    async fn deliver_peer_wake<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        origin: WakeDeliveryOrigin,
        comanaged: bool,
        response: Option<&ResponseWakeWork>,
    ) -> Result<OutboxMessage, String> {
        // Parse the logical action after authorization and routing, before actuation.
        let parsed_remote_command = msg
            .command
            .as_deref()
            .map(parse_remote_pty_command)
            .transpose()?;

        // #1635-2 D5: Explicit agent/profile flags skip config.json writes and annotate receipts.
        let flags_present = (!msg.preferred_agent.is_empty() && msg.preferred_agent != "auto")
            || msg.requested_profile.is_some();

        // #1635-2 D6: Reject hand-written profiles outside single A-Z permanently.
        if let Some(letter) = msg.requested_profile.as_deref() {
            if crate::config::settings::normalize_profile_letter(letter).is_none() {
                return Err(ERR_INVALID_DISPATCH_PROFILE.to_string());
            }
        }

        // #4309: check PTY text before actuation; resolve backend-dependent text below.
        if !super::wake_injection::body_depends_on_backend(msg, origin) {
            super::wake_injection::admit_peer_text(msg, &msg.body, origin, comanaged)?;
        }

        // #885 J2: Defer filesystem wakes during purge under normal retry limits.
        // #1177: DB dispatch skips the lease here to avoid spending a delivery attempt.
        if let Some(g) = app.pty_purge() {
            if g.blocks_agent(&msg.to) {
                return Err(format!(
                    "purge-room in progress for '{}'; wake deferred",
                    msg.to
                ));
            }
        }

        loop {
            let fallback = match self
                .select_peer_wake_candidate(
                    app,
                    msg,
                    (origin, comanaged),
                    response,
                    (parsed_remote_command.is_some(), flags_present),
                )
                .await?
            {
                std::ops::ControlFlow::Break(delivered) => return Ok(delivered),
                std::ops::ControlFlow::Continue(fallback) => fallback,
            };
            // #4309: validate cold-path text before destroy, spawn or replacement.
            if super::wake_injection::body_depends_on_backend(msg, origin) {
                self.admit_cold_peer_text(app, msg, (origin, comanaged))
                    .await?;
            }
            if let Some(previous) = fallback.1 {
                if self
                    .peer_wake_needs_owned_replacement(app, msg, previous)
                    .await
                {
                    match self
                        .deliver_owned_peer_replacement(
                            app,
                            msg,
                            (origin, comanaged),
                            response,
                            (parsed_remote_command.is_some(), flags_present),
                            previous,
                        )
                        .await?
                    {
                        OwnedPeerWakeStep::Delivered(delivered) => return Ok(delivered),
                        OwnedPeerWakeStep::Relist => {
                            tokio::task::yield_now().await;
                            continue;
                        }
                        OwnedPeerWakeStep::Retained(exit) => {
                            return Err(retained_wake_error(msg, previous, &exit))
                        }
                    }
                }
            }
            return self
                .deliver_peer_wake_fallback(
                    app,
                    msg,
                    (origin, comanaged),
                    response,
                    (parsed_remote_command.is_some(), flags_present),
                    fallback,
                )
                .await;
        }
    }

    /// #4309: Inline container file notices, write local notices, and defer plan errors.
    async fn admit_cold_peer_text<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        delivery: (WakeDeliveryOrigin, bool),
    ) -> Result<(), String> {
        let (origin, comanaged) = delivery;
        let Ok(plan) = self.resolve_wake_spawn_plan(app, msg).await else {
            return Ok(());
        };
        let inline = if super::wake_injection::plan_is_container(&plan) {
            let sender_root = self.resolve_repo_path(&msg.from, app).await;
            let sender_root = sender_root.as_deref().map(Path::new);
            super::notice::container_body_override_for_delivery(origin, &msg.body, sender_root)?
        } else {
            None
        };
        let body = inline.as_deref().unwrap_or(&msg.body);
        super::wake_injection::admit_peer_text(msg, body, origin, comanaged)
    }

    async fn peer_wake_needs_owned_replacement<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        previous: Uuid,
    ) -> bool {
        let manager = app.pty_sessions().read().await.clone();
        let Some(row) = manager.get_session(previous).await else {
            return false;
        };
        let Ok(directory) =
            crate::path_identity::verify_directory(Path::new(&row.working_directory))
        else {
            return true;
        };
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        !matches!(
            crate::config::sessions_persistence::classify_lifecycle_carrier_scope(
                &permit,
                Some(&msg.to),
                &directory.canonical_path,
            ),
            crate::config::sessions_persistence::CreateCarrierScope::Foreign { .. }
        )
    }

    async fn owned_peer_source_is_current<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        request: &OwnedReplacementCreateRequest,
        physical: &VerifiedPathIdentity,
    ) -> bool {
        if app
            .pty_purge()
            .is_some_and(|purge| purge.blocks_agent(&msg.to))
        {
            return false;
        }
        let settings = app.pty_settings().read().await.clone();
        let master = msg.token.as_deref().is_some_and(|token| {
            app.master_token()
                .is_some_and(|master| master.matches(token))
                || settings.root_token.as_deref() == Some(token)
        });
        if !master {
            if let Some(token) = msg.token.as_deref() {
                let Ok(token) = Uuid::parse_str(token) else {
                    return false;
                };
                let manager = app.pty_sessions().read().await.clone();
                let Some(sender) = manager.find_by_token(token).await else {
                    return false;
                };
                if sender_name_for_session_cwd_with_root_flag(
                    &sender.working_directory,
                    sender.is_root_agent,
                ) != msg.from
                {
                    return false;
                }
            }
            if !teams::can_communicate(&msg.from, &msg.to, &teams::discover_teams()) {
                return false;
            }
        }
        let Ok(current_physical) = crate::path_identity::verify_directory(Path::new(&request.cwd))
        else {
            return false;
        };
        if !crate::path_identity::same_object(&current_physical, physical)
            || teams::is_coordinator_for_cwd(&request.cwd, &teams::discover_teams())
            || crate::config::root_agent::is_root_agent_path(&request.cwd)
        {
            return false;
        }
        let Ok(current) = self.resolve_wake_spawn_plan(app, msg).await else {
            return false;
        };
        if crate::path_utils::normalize_windows_verbatim_path(&current.cwd) != request.cwd
            || current.spawn_shell != request.shell
            || current.spawn_args != request.shell_args
            || current.resolved_command.agent_id != request.agent_id
            || current.spawn_label != request.agent_label
            || current.resolved_agent_host_shell != request.host_shell
        {
            return false;
        }
        match (
            current.configured_spawn.as_ref(),
            request.resolved_spawn.as_ref(),
        ) {
            (Some(current), Some(expected)) => {
                current.profile_content_hash == expected.profile_content_hash
                    && current.profile_resolution == expected.profile_resolution
                    && current.trusted_agent_id == expected.trusted_agent_id
                    && current.trusted_agent_label == expected.trusted_agent_label
                    && current.shell == expected.shell
                    && current.shell_args == expected.shell_args
                    && current.env_remove_keys == expected.env_remove_keys
                    && current.effective_codex_home == expected.effective_codex_home
                    && current.effective_child_env() == expected.effective_child_env()
                    && match (current.seed.as_ref(), expected.seed.as_ref()) {
                        (None, None) => true,
                        (Some(current), Some(expected)) => {
                            current.dest == expected.dest
                                && current.candidates == expected.candidates
                                && current.context == expected.context
                                && current.config_dir_warning == expected.config_dir_warning
                        }
                        _ => false,
                    }
                    && SessionBackendKind::from(&current.backend)
                        == SessionBackendKind::LocalProcess
            }
            (None, None) => true,
            _ => false,
        }
    }

    async fn admit_owned_peer_replacement<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        request: &OwnedReplacementCreateRequest,
        previous_id: Uuid,
    ) -> Result<ControlFlow<OwnedPeerWakeStep, OwnedPeerTarget>, String> {
        #[cfg(test)]
        if let Some(control) = self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control)
        {
            if !control.before_source_accept(&msg.id).await {
                return Ok(ControlFlow::Break(OwnedPeerWakeStep::Relist));
            }
        }
        let coordinator = app.selection();
        let manager = app.pty_sessions().read().await.clone();
        let pty = app.pty_routes().clone();
        let gate = app
            .message_store()
            .and_then(|state| state.target_gate.as_ref().ok().cloned())
            .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        let directory =
            crate::config::config_dir().ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        let (physical, replica) = owned_replacement_directories(request)?;
        let work = owned_replacement_work(msg)?;
        let context = (coordinator, &manager, &pty);
        let dirs = (&physical, &replica);
        let mut probe_rollback = true;
        let mut prepared = None;
        let mut source = SourceExecutorGuard::unbound();
        loop {
            coordinator.wait_for_restore_release().await?;
            if !self
                .owned_peer_source_is_current(app, msg, request, &physical)
                .await
            {
                return Err("Wake source authority changed before replacement".into());
            }
            if std::mem::take(&mut probe_rollback) {
                recover_peer_wake_precommit(context, &msg.to, previous_id, request, dirs).await;
            }
            let stripe = gate
                .acquire_target_lock(&msg.to)
                .await
                .map_err(|_| "targetCreateGateUnavailable".to_string())?;
            let exact = gate.acquire_exact(&msg.to).await;
            let captured = owned_snapshot(&manager, &directory, msg, &physical, previous_id).await;
            if captured.0 {
                return Ok(ControlFlow::Break(OwnedPeerWakeStep::Relist));
            }
            if prepared.is_none() {
                prepared = captured.1.map(|snapshot| {
                    let resolved =
                        resolve_replacement(msg, request, &directory, snapshot, &work, dirs);
                    let binding = manager.reserve_carrier_create_binding();
                    coordinator.prepare_owned_replacement(resolved, binding)
                });
            }
            if let Some(candidate) = prepared.take() {
                match admit_peer_wake_carrier(context, candidate, &mut source).await {
                    Ok(ReplacementAdmissionStep::Accepted(continuation)) => {
                        return Ok(ControlFlow::Continue(OwnedPeerTarget {
                            continuation,
                            source,
                            gate,
                            exact,
                            stripe,
                            physical,
                        }))
                    }
                    Ok(ReplacementAdmissionStep::Waiting(candidate)) => prepared = Some(candidate),
                    Err(Some(error)) => return Err(error),
                    Err(None) => {
                        return Ok(ControlFlow::Break(RetainedExit::step(
                            "source_unbound",
                            None,
                        )))
                    }
                }
            }
            drop(exact);
            drop(stripe);
            check_wake_deadline()?;
            tokio::task::yield_now().await;
        }
    }

    async fn deliver_owned_peer_replacement<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        delivery: (WakeDeliveryOrigin, bool),
        response: Option<&ResponseWakeWork>,
        dispatch: (bool, bool),
        previous: Uuid,
    ) -> Result<OwnedPeerWakeStep, String> {
        let plan = self.resolve_wake_spawn_plan(app, msg).await?;
        if let Some(command) = msg.command.as_deref() {
            resolve_remote_pty_command(&plan.spawn_shell, command)?;
        }
        if plan.configured_spawn.as_ref().is_some_and(|spawn| {
            SessionBackendKind::from(&spawn.backend) != SessionBackendKind::LocalProcess
        }) {
            return Ok(RetainedExit::step("backend_not_local", None));
        }
        // #4309: Validate the local replacement's exact body before transfer.
        super::wake_injection::admit_peer_text(msg, &msg.body, delivery.0, delivery.1)?;
        let (request, resolved_command) = owned_replacement_request(plan, dispatch.1);
        let manager = app.pty_sessions().read().await.clone();
        WakeDispatch::OwnedAdmission.mark();
        let target = match self
            .admit_owned_peer_replacement(app, msg, &request, previous)
            .await?
        {
            ControlFlow::Continue(target) => target,
            ControlFlow::Break(step) => return Ok(step),
        };
        let continuation = self.with_owned_source_validation(
            app,
            msg,
            &request,
            target.continuation,
            target.physical,
        );
        #[cfg(test)]
        let continuation = match self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control)
        {
            Some(control) => continuation.with_fixture_control(control),
            None => continuation,
        };
        #[cfg(test)]
        continuation.fixture_inserted();
        #[cfg(test)]
        fixture_capture_owner(&continuation);
        // Capturing the bound guard lets even an unpolled core record Stopped.
        let mut source = target.source;
        let mut execution = OwnedPeerWakeExecution {
            message: msg.clone(),
            delivery,
            response: response.cloned(),
            flags_present: dispatch.1,
            request,
            resolved_command,
            continuation,
            gate: target.gate,
            exact: Some(target.exact),
            stripe: Some(target.stripe),
        };
        let completion = execution.continuation.clone();
        let pty = app.pty_routes().clone();
        let app = app.clone();
        let delivery = self.delivery_poller();
        #[cfg(test)]
        if let Some(probe) = &self.clear_observation_probe {
            probe.record_owned_child(&delivery);
        }
        // Common create borrows the lexical proof inside the existing owned core.
        let (id, to) = (msg.id.clone(), msg.to.clone());
        let attempt = WAKE_ATTEMPT.try_with(Clone::clone).ok().flatten();
        let core = Box::pin(async move {
            let result = delivery
                .execute_owned_peer_replacement(&app, &pty, &mut execution)
                .await;
            #[cfg(test)]
            OwnedReplacementContinuation::fixture_before_wake_return().await;
            drop(execution);
            source.returned(result.is_err());
            drop(source);
            if matches!(&result, Ok(OwnedPeerWakeStep::Retained(_))) {
                completion.settle_wake_pre_input(&manager, &pty).await;
            }
            let result = owned_peer_completion(result, &completion, &manager, &pty).await;
            log_core_end(&result, &id, &to, previous);
            result
        });
        run_owned_persist_core(WAKE_ATTEMPT.scope(attempt, core)).await
    }

    fn with_owned_source_validation<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        request: &OwnedReplacementCreateRequest,
        continuation: OwnedReplacementContinuation,
        physical: VerifiedPathIdentity,
    ) -> OwnedReplacementContinuation {
        let validation_messages = Arc::clone(&self.messages);
        let validation_app = app.clone();
        let validation_message = msg.clone();
        let validation_request = request.clone();
        continuation.with_source_validation(Arc::new(move || {
            let messages = Arc::clone(&validation_messages);
            let app = validation_app.clone();
            let msg = validation_message.clone();
            let request = validation_request.clone();
            let physical = physical.clone();
            Box::pin(async move {
                if MailboxPoller::delivery_only(messages)
                    .owned_peer_source_is_current(&app, &msg, &request, &physical)
                    .await
                {
                    Ok(())
                } else {
                    Err(crate::session::selection::OwnedSourceStop::AuthorityChanged)
                }
            })
        }))
    }
}
