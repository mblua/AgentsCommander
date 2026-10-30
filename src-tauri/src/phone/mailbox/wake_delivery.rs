use super::handoff::resolve_remote_pty_command;
use super::notice::{
    internal_system_envelope, parse_remote_pty_command, sender_name_for_session_cwd_with_root_flag,
    OutboxOrigin, WakeDeliveryOrigin, ERR_INVALID_DISPATCH_PROFILE,
};
use super::poller::{
    owned_replacement_launch_recipe, MailboxPoller, OutboxAuthenticationFacts, OutboxStage,
    OwnedPeerTarget, OwnedPeerWakeExecution, OwnedPeerWakeStep, ValidatedWake,
};
#[cfg(test)]
use super::pty_claim::MailboxTestHooks;
use super::pty_claim::{ResponseWakeWork, WakeDelivery};
use crate::config::settings::SettingsState;
use crate::config::teams;
use crate::phone::types::OutboxMessage;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::MasterToken;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    pub(crate) async fn validate_outbox_action_and_mode<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> OutboxStage {
        // Action-based dispatch (close-session, etc.) — handled before mode-based delivery
        if let Some(ref action) = msg.action {
            match action.as_str() {
                "close-session" => {
                    // §224 G-IMPL-2 — thread `is_app_outbox` so the response-
                    // write path can skip the outbox-relative primary write
                    // when the message came from the app-outbox (master-token
                    // path). That derived path lands under
                    // <config_dir>/instances/<id>/responses/ which the CLI
                    // never polls, so writing there leaks orphan JSON files.
                    return Ok(std::ops::ControlFlow::Break(
                        self.handle_close_session(app, path, msg, is_app_outbox)
                            .await,
                    ));
                }
                _ => {
                    return Ok(std::ops::ControlFlow::Break(
                        self.reject_message(path, msg, &format!("Unknown action '{}'", action))
                            .await,
                    ));
                }
            }
        }

        // Deliver based on mode — all modes require immediate delivery or rejection
        let mode = if msg.mode.is_empty() {
            "wake"
        } else {
            msg.mode.as_str()
        };
        // Only `wake` is supported. Defensive check for malformed outbox files
        // that might arrive from external (root-token) write paths.
        if mode != "wake" {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(
                    path,
                    msg,
                    &format!("Unsupported delivery mode '{}'. Valid: wake", mode),
                )
                .await,
            ));
        }
        Ok(std::ops::ControlFlow::Continue(()))
    }

    pub(crate) async fn deliver_outbox_tail<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
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
        // #2232 phase 7: a Co-managed message is delivered inline with its
        // display attribution; it is never parked for the detached worker pool,
        // whose shared `deliver_wake` tail carries no attribution and must keep
        // its exact behaviour for every other origin.
        if origin == OutboxOrigin::CoManaged {
            return self
                .deliver_bound_outbox_tail(app, path, &validated, true)
                .await;
        }
        if let Some(slot) = deferred_tail {
            *slot = Some(validated);
            return Ok(());
        }
        // #1635-2 D4: write the annotated message so the `Delivered:` CLI
        // receipt can report the effective agent/profile. No-flag messages are
        // still byte-identical to today.
        self.deliver_bound_outbox_tail(app, path, &validated, false)
            .await
    }

    async fn deliver_bound_outbox_tail<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        validated: &ValidatedWake,
        comanaged: bool,
    ) -> Result<(), String> {
        let mut delivery = self.delivery_poller();
        delivery.codex_clear_source = Some(path.to_path_buf());
        match delivery
            .deliver_validated_wake(app, validated, comanaged)
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

    /// Deliver mode: wake — inject into the recipient's PTY for any non-Exited
    /// session; destroy and respawn if Exited; spawn persistent if none. Always
    /// delivers (no busy-gate — stdin buffer absorbs input while the agent is
    /// mid-turn).
    // #791: `pub(crate)` so the in-daemon control-plane API can funnel through
    // the SAME actuation the poller uses (no fork). The API calls this on a
    // throwaway `MailboxPoller::new()` (delivery-stateless: this method and its
    // whole `&self` callee chain read only `app.state::<...>()`, never the
    // poller's `poll_interval` / `retry_tracker` fields). See plan #791 §0.5 HIGH-1.
    pub(crate) async fn deliver_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
    ) -> Result<OutboxMessage, String> {
        self.deliver_wake_with_origin(app, msg, WakeDeliveryOrigin::FilesystemPoller)
            .await
    }

    pub(crate) async fn deliver_wake_with_origin<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
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

    /// #2232 phase 7: deliver a wake whose origin is the Co-managed provenance
    /// queue. `comanaged` is display-only and never part of routing; the
    /// existing entry points stay attribution-free.
    async fn deliver_wake_with_attribution<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
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

    /// Single private entry for every wake delivery kind. The variant is selected only by
    /// code: serialized peer messages can enter only `Peer`, while the validated target,
    /// notice, cancellation, and guard capability are required for `InternalSystem`.
    /// Returns the annotated `OutboxMessage` for the `Delivered:` receipt; the
    /// internal-system arm builds its own envelope (it holds no serialized message).
    pub(crate) async fn deliver_validated_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
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

    pub(crate) async fn deliver_wake_engine<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        delivery: WakeDelivery<'_>,
    ) -> Result<OutboxMessage, String> {
        let target_fqn = match &delivery {
            WakeDelivery::Peer { message, .. } => message.to.as_str(),
            WakeDelivery::InternalSystem { target, .. } => target.fqn(),
        };
        let _response_delivery =
            match app.try_state::<crate::api::message_store::MessageStoreState>() {
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
            } => Box::pin(self.deliver_peer_wake(app, message, origin, comanaged, response)).await,
            WakeDelivery::InternalSystem {
                target,
                notice,
                cancellation,
                guard,
            } => {
                // The internal-system envelope never carries dispatch flags, so
                // the fresh unannotated envelope is the correct delivered
                // message. Built up front because the delivery below moves
                // `target`; the delivery MUST still run (context alerts,
                // notices, cancellations, guard).
                let delivered = internal_system_envelope(&target);
                self.deliver_internal_system_wake(app, target, notice, cancellation, guard)
                    .await?;
                Ok(delivered)
            }
        }
    }

    async fn deliver_peer_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        origin: WakeDeliveryOrigin,
        comanaged: bool,
        response: Option<&ResponseWakeWork>,
    ) -> Result<OutboxMessage, String> {
        // Parse a hand-authored logical action after process_message's
        // authorization/routing gates but before any recipient actuation.
        let parsed_remote_command = msg
            .command
            .as_deref()
            .map(parse_remote_pty_command)
            .transpose()?;

        // #1635-2 D5: a wake carries dispatch flags when it names a specific
        // agent (empty string and `auto` are the no-flag shapes — the API/DB
        // path writes `preferred_agent: String::new()`) or requests a profile
        // letter. Flagged wakes suppress the spawn-time config.json writes
        // (skip_tooling_save) and populate the delivered-receipt annotations.
        let flags_present = (!msg.preferred_agent.is_empty() && msg.preferred_agent != "auto")
            || msg.requested_profile.is_some();

        // #1635-2 D6: a present requestedProfile that is not a single A-Z
        // letter is a permanent delivery error. The CLI rejects it before
        // enqueue (1635-1); this defends hand-written outbox JSON.
        if let Some(letter) = msg.requested_profile.as_deref() {
            if crate::config::settings::normalize_profile_letter(letter).is_none() {
                return Err(ERR_INVALID_DISPATCH_PROFILE.to_string());
            }
        }

        // (#885 J2) A purge is destroying this agent's record right now. A wake
        // delivered into that window falls through to spawn-persistent below and
        // would cold-spawn the agent we are purging, silently breaking the verb's
        // postcondition. This is a BACKSTOP, not the primary defense: the DB
        // dispatcher must skip its tick before leasing (see `api/dispatcher.rs`,
        // #885 F-5); reaching this Err from there would burn an attempt and can
        //   POISON the message. The one caller for which this Err is safe:
        //   - filesystem poller: non-permanent error, retried at the 3s poll
        //     interval up to MAX_DELIVERY_ATTEMPTS. Deferred, not lost.
        //   The inline API send that used to map this to a rejected outcome no
        //   longer exists (#1177), so the DB dispatcher is now the only other
        //   caller, and it is exactly the one F-5 must keep out of this window.
        if let Some(g) = app.try_state::<std::sync::Arc<crate::session::purge_guard::PurgeGuard>>()
        {
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
                        OwnedPeerWakeStep::Retained => return std::future::pending().await,
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

    async fn peer_wake_needs_owned_replacement<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        previous: Uuid,
    ) -> bool {
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
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

    async fn owned_peer_source_is_current<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        request: &crate::commands::session::OwnedReplacementCreateRequest,
        physical: &crate::path_identity::VerifiedPathIdentity,
    ) -> bool {
        if app
            .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
            .is_some_and(|purge| purge.blocks_agent(&msg.to))
        {
            return false;
        }
        let settings = app.state::<SettingsState>().read().await.clone();
        let master = msg.token.as_deref().is_some_and(|token| {
            app.try_state::<MasterToken>()
                .is_some_and(|master| master.matches(token))
                || settings.root_token.as_deref() == Some(token)
        });
        if !master {
            if let Some(token) = msg.token.as_deref() {
                let Ok(token) = Uuid::parse_str(token) else {
                    return false;
                };
                let manager = app
                    .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                    .read()
                    .await
                    .clone();
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

    async fn admit_owned_peer_replacement<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        request: &crate::commands::session::OwnedReplacementCreateRequest,
        previous_id: Uuid,
    ) -> Result<Option<OwnedPeerTarget>, String> {
        #[cfg(test)]
        if let Some(control) = self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control)
        {
            if !control.before_source_accept(&msg.id).await {
                return Ok(None);
            }
        }
        use crate::session::selection::{
            OwnedReplacementWork, ReplacementAdmissionStep, ResolvedReplacement,
        };
        let coordinator = app.state::<crate::session::selection::SelectionCoordinator>();
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let gate = app
            .try_state::<crate::api::message_store::MessageStoreState>()
            .and_then(|state| state.target_gate.as_ref().ok().cloned())
            .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        let directory =
            crate::config::config_dir().ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        let physical = crate::path_identity::verify_directory(Path::new(&request.cwd))
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        let replica = teams::strict_wg_replica_anchor_from_cwd(Path::new(&request.cwd))
            .ok()
            .flatten()
            .and_then(|root| crate::path_identity::verify_directory(&root).ok())
            .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        let payload = serde_json::to_value(msg).map_err(|error| error.to_string())?;
        let work = OwnedReplacementWork::OrdinaryOwned {
            message_id: msg.id.clone(),
            body: msg.body.clone(),
            payload: payload.clone(),
            recipient: msg.to.clone(),
            request: payload,
        };
        let mut prepared = None;
        loop {
            coordinator.wait_for_restore_release().await?;
            if !self
                .owned_peer_source_is_current(app, msg, request, &physical)
                .await
            {
                return Err("Wake source authority changed before replacement".into());
            }
            let stripe = gate
                .acquire_target_lock(&msg.to)
                .await
                .map_err(|_| "targetCreateGateUnavailable".to_string())?;
            let exact = gate.acquire_exact(&msg.to).await;
            let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
            let captured = manager
                .with_current_carrier_snapshot(&permit, |current| {
                    let scope =
                        crate::config::sessions_persistence::classify_lifecycle_carrier_scope(
                            &permit,
                            Some(&msg.to),
                            &physical.canonical_path,
                        );
                    let foreign = matches!(
                        scope,
                        crate::config::sessions_persistence::CreateCarrierScope::Foreign { .. }
                    );
                    let carrier =
                        crate::config::sessions_persistence::retained_carrier_for_replacement(
                            &permit,
                            &directory,
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
            if captured.0 {
                return Ok(None);
            }
            if prepared.is_none() {
                if let Some((carrier, previous)) = captured.1 {
                    prepared = Some(coordinator.prepare_owned_replacement(
                        ResolvedReplacement {
                            target: msg.to.clone(),
                            directory: directory.clone(),
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
                        },
                        manager.reserve_carrier_create_binding(),
                    ));
                }
            }
            if let Some(candidate) = prepared.take() {
                let tracker = pty
                    .lock()
                    .map_err(|_| "targetCreateGateUnavailable".to_string())?
                    .response_close_state();
                let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
                let admission = manager
                    .with_current_carrier_snapshot(&permit, |current| {
                        coordinator
                            .admit_owned_replacement(&permit, current, &tracker, &pty, candidate)
                    })
                    .await;
                drop(permit);
                match admission? {
                    ReplacementAdmissionStep::Accepted(continuation) => {
                        return Ok(Some(OwnedPeerTarget {
                            continuation,
                            gate,
                            exact,
                            stripe,
                            physical,
                        }))
                    }
                    ReplacementAdmissionStep::Waiting(candidate) => prepared = Some(candidate),
                }
            }
            drop(exact);
            drop(stripe);
            tokio::task::yield_now().await;
        }
    }

    async fn deliver_owned_peer_replacement<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
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
            return Ok(OwnedPeerWakeStep::Retained);
        }
        let request = crate::commands::session::OwnedReplacementCreateRequest {
            shell: plan.spawn_shell,
            shell_args: plan.spawn_args,
            cwd: crate::path_utils::normalize_windows_verbatim_path(&plan.cwd),
            name: Some(plan.session_name),
            agent_id: plan.resolved_command.agent_id.clone(),
            agent_label: plan.spawn_label,
            skip_tooling_save: dispatch.1,
            git_repos: Vec::new(),
            resolved_spawn: plan.configured_spawn,
            host_shell: plan.resolved_agent_host_shell,
        };
        let Some(target) = self
            .admit_owned_peer_replacement(app, msg, &request, previous)
            .await?
        else {
            return Ok(OwnedPeerWakeStep::Relist);
        };
        let validation_messages = Arc::clone(&self.messages);
        let validation_app = app.clone();
        let validation_message = msg.clone();
        let validation_request = request.clone();
        let physical = target.physical;
        let continuation = target
            .continuation
            .with_source_validation(Arc::new(move || {
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
            }));
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
        let mut execution = OwnedPeerWakeExecution {
            message: msg.clone(),
            delivery,
            response: response.cloned(),
            flags_present: dispatch.1,
            request,
            resolved_command: plan.resolved_command,
            continuation,
            gate: target.gate,
            exact: Some(target.exact),
            stripe: Some(target.stripe),
        };
        let completion = execution.continuation.clone();
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let app = app.clone();
        let delivery = self.delivery_poller();
        // Relocates the one existing create core around its owned work tail.
        // Common create borrows this lexical proof and never spawns another core.
        let result = crate::config::sessions_persistence::run_owned_persist_core(async move {
            let mut source = crate::session::selection::SourceExecutorGuard::unbound();
            if !source.bind(&execution.continuation) {
                return Ok(OwnedPeerWakeStep::Retained);
            }
            let result = delivery
                .execute_owned_peer_replacement(&app, &mut execution)
                .await;
            source.returned(result.is_err());
            result
        })
        .await;
        #[cfg(test)]
        if matches!(&result, Ok(OwnedPeerWakeStep::Retained)) {
            // The accepted core has actually returned; its public wake caller
            // intentionally remains pending below. This is observation only.
            let _ = completion
                .fixture_reached(
                    crate::session::selection::ReplacementFixtureCut::SourceRetainedReturn,
                )
                .await;
        }
        // The existing result channel has returned the real legacy Err to the
        // source-owning caller. The completed core has released its target
        // guards; the original full message remains with this caller.
        if result.is_err() && completion.precommit_rollback_completed() {
            if let Some(effect) = completion
                .start_effect(crate::session::selection::LifecycleEffectSlot::WorkTransfer)
            {
                completion
                    .finish_rolled_back_work(&manager, &pty, effect, true)
                    .await;
            }
        }
        result
    }
}
