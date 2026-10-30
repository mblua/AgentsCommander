mod target;

use super::poller::{owned_replacement_launch_recipe, read_guarded_agent_state, MailboxPoller};
#[cfg(test)]
use super::pty_claim::MailboxTestHooks;
use super::pty_claim::{refresh_pty_claim_for_resume, PtyClaimRefresh};
use super::pty_observation::PtyAuthorityState;
use super::settle::{
    observe_pty_spawn_scope, select_persistent_exited_pty_candidate,
    session_has_current_pty_submission_provenance, PtySpawnStep,
};
use crate::config::settings::SettingsState;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    pub(crate) async fn validate_claimed_api_authority<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
    ) -> Result<
        Option<crate::config::teams::VerifiedPtyInputRoute>,
        crate::api::auth::FreshRegistryError,
    > {
        let Some(client_store) = client_store else {
            return Ok(None);
        };
        let (Some(client_id), Some(generation)) = (
            claimed.authority_client_id.as_deref(),
            claimed.authority_client_generation.as_deref(),
        ) else {
            return Ok(None);
        };
        let Some(fresh) = client_store
            .load_active_binding_fresh_offloaded(
                client_id.to_string(),
                generation.to_string(),
                crate::api::auth::SCOPE_PTY_INPUT,
            )
            .await?
        else {
            return Ok(None);
        };
        Ok(self
            .validate_claimed_api_authority_with_fresh(app, claimed, &fresh)
            .await)
    }

    pub(crate) async fn validate_claimed_api_authority_with_fresh<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        fresh: &crate::api::auth::ApiClientFreshGuard,
    ) -> Option<crate::config::teams::VerifiedPtyInputRoute> {
        let client_id = claimed.authority_client_id.as_deref()?;
        let generation = claimed.authority_client_generation.as_deref()?;
        let session_id =
            crate::phone::types::parse_canonical_uuid_v4(&claimed.authority_session_id).ok()?;
        if fresh.client.client_id != client_id
            || !fresh.client.has_scope(crate::api::auth::SCOPE_PTY_INPUT)
            || fresh.client.bound_session_id.as_deref()
                != Some(claimed.authority_session_id.as_str())
            || fresh.client.credential_generation.as_deref() != Some(generation)
        {
            return None;
        }
        let session = {
            let manager = app.pty_sessions();
            let guard = manager.read().await;
            guard.get_session(session_id).await
        }?;
        if matches!(session.status, SessionStatus::Exited(_))
            || session.backend_kind != SessionBackendKind::ContainerTransport
        {
            return None;
        }
        let root =
            crate::path_identity::verify_directory(Path::new(&session.working_directory)).ok()?;
        let bound_root =
            crate::path_identity::verify_directory(Path::new(&fresh.client.bound_root)).ok()?;
        if !crate::path_identity::same_object(&root, &bound_root) {
            return None;
        }
        let (binding, route_backend, route_identities, live) = {
            let manager = app.pty_routes();
            let manager = manager.lock().unwrap_or_else(|error| error.into_inner());
            (
                manager.container_backend().credential_binding(session_id),
                manager.backend_kind(session_id),
                manager.route_identities(session_id),
                manager.has_session(session_id),
            )
        };
        let binding = binding?;
        if !live
            || route_backend != Some(SessionBackendKind::ContainerTransport)
            || binding.client_id != client_id
            || binding.credential_generation != generation
            || binding.bound_session_id != claimed.authority_session_id
            || binding.bound_root_object_id != root.object_id
            || !crate::api::auth::constant_time_eq(
                &binding.credential_token_hash,
                &fresh.presented_token_hash,
            )
        {
            return None;
        }
        let in_memory_paths = {
            let settings = app.pty_settings();
            let paths = settings.read().await.project_paths.clone();
            paths
        };
        let mut paths =
            match crate::config::settings::read_pty_input_project_paths_strict_offloaded().await {
                Ok(paths) => paths.unwrap_or(in_memory_paths),
                Err(_) => return None,
            };
        let sender = crate::config::teams::verify_pty_input_coordinator_root(Path::new(
            &session.working_directory,
        ))
        .ok()?;
        if let Some(project_path) = sender.ac_root_identity.canonical_path.parent() {
            let project = project_path
                .to_str()
                .map(crate::path_utils::normalize_windows_verbatim_path)?;
            if !paths.contains(&project) {
                paths.push(project);
            }
        }
        let route = crate::config::teams::verify_pty_input_route(
            Path::new(&session.working_directory),
            false,
            &claimed.target_fqn,
            &paths,
        )
        .ok()?;
        let route_matches = route_identities.is_some_and(|(cwd, replica, _)| {
            cwd.as_ref()
                .is_some_and(|saved| crate::path_identity::same_object(saved, &root))
                && replica.as_ref().is_some_and(|saved| {
                    crate::path_identity::same_object(saved, &route.sender.replica_identity)
                })
        });
        if !route_matches
            || route.sender.canonical_fqn != claimed.sender_fqn
            || route.sender.authority_fingerprint != claimed.sender_identity_fingerprint
            || route.target.authority_fingerprint != claimed.target_identity_fingerprint
        {
            return None;
        }
        Some(route)
    }

    async fn pty_spawn_authority_is_current<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        expected_sender: &crate::config::teams::VerifiedPtyInputIdentity,
        expected_target: &crate::config::teams::VerifiedPtyInputIdentity,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
    ) -> Result<bool, crate::api::auth::FreshRegistryError> {
        let current = if source_plane == crate::phone::types::PtyInputSourcePlane::HostCli {
            self.validate_claimed_host_authority(app, claimed).await
        } else {
            self.validate_claimed_api_authority(app, claimed, api_client_store)
                .await?
        };
        Ok(current.is_some_and(|route| {
            route.sender.canonical_fqn == expected_sender.canonical_fqn
                && route.sender.authority_fingerprint == expected_sender.authority_fingerprint
                && crate::path_identity::same_object(
                    &route.sender.replica_identity,
                    &expected_sender.replica_identity,
                )
                && route.target.canonical_fqn == expected_target.canonical_fqn
                && route.target.authority_fingerprint == expected_target.authority_fingerprint
                && crate::path_identity::same_object(
                    &route.target.replica_identity,
                    &expected_target.replica_identity,
                )
        }))
    }

    async fn owned_pty_recipe_is_current<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        expected: &crate::commands::session::OwnedReplacementCreateRequest,
    ) -> bool {
        let Some(agent) = expected.agent_id.as_deref() else {
            return false;
        };
        let settings = app.state::<SettingsState>().read().await.clone();
        let Ok(Some(spawn)) = crate::commands::session::build_configured_agent_spawn_for_cwd(
            &settings,
            agent,
            &expected.cwd,
            None,
        ) else {
            return false;
        };
        if SessionBackendKind::from(&spawn.backend) != SessionBackendKind::LocalProcess {
            return false;
        }
        let mut current = expected.clone();
        current.shell = spawn.shell.clone();
        current.shell_args = spawn.shell_args.clone();
        current.agent_label = Some(spawn.trusted_agent_label.clone());
        current.resolved_spawn = Some(spawn);
        current.host_shell = Some(crate::pty::backend::ResolvedAgentHostShell {
            program: settings.default_shell,
            args: settings.default_shell_args,
        });
        owned_replacement_launch_recipe(&current) == owned_replacement_launch_recipe(expected)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Keep the actual PTY source, proof, executor and fixture borrow contract together"
    )]
    async fn spawn_owned_pty_replacement<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        request: crate::commands::session::OwnedReplacementCreateRequest,
        source: (
            &crate::api::message_store::ClaimedPtyInputOperation,
            &crate::phone::types::PtyInputResult,
            &crate::config::teams::VerifiedPtyInputIdentity,
            Option<&Arc<crate::api::auth::ApiClientStore>>,
        ),
        target: (&crate::config::teams::VerifiedPtyInputIdentity, Uuid),
        prepared: Option<crate::session::selection::PreparedOwnedReplacement>,
        target_ownership: &crate::api::message_store::PtyInputTargetOwnership<'_>,
        source_executor: &mut crate::session::selection::SourceExecutorGuard,
        #[cfg(test)] fixture_lease: Option<&Arc<crate::commands::session::FixturePtyCutLease>>,
    ) -> Result<PtySpawnStep, crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;
        use crate::session::selection::ReplacementAdmissionStep;
        let (claimed, initial, sender, api_client_store) = source;
        let (target, previous_id) = target;
        let Some(coordinator) = app.try_state::<crate::session::selection::SelectionCoordinator>()
        else {
            return Ok(PtySpawnStep::NeedReacquire(prepared));
        };
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let prepared = match prepared {
            Some(prepared) => prepared,
            None => {
                let Some(prepared) = Self::prepare_pty_replacement(
                    &coordinator,
                    &manager,
                    &request,
                    claimed,
                    initial,
                    target,
                    previous_id,
                )
                .await?
                else {
                    return Ok(PtySpawnStep::NeedReacquire(None));
                };
                prepared
            }
        };
        Self::validate_pty_replacement_launch(&prepared, &request)?;
        let tracker = pty
            .lock()
            .map_err(|_| C::StoreTransient)?
            .response_close_state();
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let admission = manager
            .with_current_carrier_snapshot(&permit, |current| {
                coordinator.admit_owned_replacement(&permit, current, &tracker, &pty, prepared)
            })
            .await;
        drop(permit);
        let continuation = match admission {
            Ok(ReplacementAdmissionStep::Waiting(prepared)) => {
                return Ok(PtySpawnStep::NeedReacquire(Some(prepared)))
            }
            Ok(ReplacementAdmissionStep::Accepted(continuation)) => continuation,
            Err(_) => return Err(C::StoreTransient),
        };
        if !source_executor.bind(&continuation) {
            return Ok(PtySpawnStep::RetainedUnowned);
        }
        let validation_messages = Arc::clone(&self.messages);
        let validation_app = app.clone();
        let validation_claim = claimed.clone();
        let validation_initial = initial.clone();
        let validation_sender = sender.clone();
        let validation_target = target.clone();
        let validation_api = api_client_store.cloned();
        let validation_request = request.clone();
        let continuation = continuation.with_source_validation(Arc::new(move || {
            let messages = Arc::clone(&validation_messages);
            let app = validation_app.clone();
            let claimed = validation_claim.clone();
            let initial = validation_initial.clone();
            let sender = validation_sender.clone();
            let target = validation_target.clone();
            let api = validation_api.clone();
            let request = validation_request.clone();
            Box::pin(async move {
                use crate::session::selection::OwnedSourceStop as Stop;
                let delivery = MailboxPoller::delivery_only(messages);
                let store = app
                    .try_state::<crate::api::message_store::MessageStoreState>()
                    .and_then(|state| state.store.as_ref().ok().cloned())
                    .ok_or(Stop::StoreTransient)?;
                match refresh_pty_claim_for_resume(&store, &initial, &claimed).await {
                    Ok(PtyClaimRefresh::Current) => {}
                    Ok(PtyClaimRefresh::Terminal) => return Err(Stop::Terminal),
                    Err(C::AuthorityChanged) => return Err(Stop::AuthorityChanged),
                    Err(C::Expired) => return Err(Stop::Expired),
                    Err(C::LeaseLost) => return Err(Stop::LeaseLost),
                    Err(_) => return Err(Stop::StoreTransient),
                }
                match delivery
                    .pty_spawn_authority_is_current(
                        &app,
                        &claimed,
                        &sender,
                        &target,
                        claimed.source_plane,
                        api.as_ref(),
                    )
                    .await
                {
                    Ok(true) => {
                        if delivery.owned_pty_recipe_is_current(&app, &request).await {
                            Ok(())
                        } else {
                            Err(Stop::AuthorityChanged)
                        }
                    }
                    Ok(false) => Err(Stop::AuthorityChanged),
                    Err(_) => Err(Stop::StoreTransient),
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
        #[cfg(test)]
        if !continuation
            .fixture_reached(crate::session::selection::ReplacementFixtureCut::SourceCoreEntered)
            .await
        {
            return Ok(PtySpawnStep::RetainedUnowned);
        }
        #[cfg(test)]
        let fixture_created = if let Some(lease) = fixture_lease {
            let Some(control) = self
                .test_hooks
                .as_ref()
                .and_then(MailboxTestHooks::replacement_fixture_control)
            else {
                return Ok(PtySpawnStep::RetainedUnowned);
            };
            let result = coordinator
                .submit_inline_replacement_fixture(
                    crate::commands::session::FixtureReplacementInput {
                        request: request.clone(),
                        continuation: continuation.clone().with_fixture_control(control.clone()),
                        lease: crate::commands::session::FixtureReplacementLease::PtyCutShared(
                            lease.clone(),
                        ),
                    },
                    control,
                )
                .await;
            let Some(result) = result else {
                return Ok(PtySpawnStep::RetainedUnowned);
            };
            if result.ordinary_lease.is_some() {
                return Ok(PtySpawnStep::RetainedUnowned);
            }
            Some(result.outcome)
        } else {
            None
        };
        #[cfg(test)]
        let created = match fixture_created {
            Some(created) => created,
            None => {
                crate::commands::session::create_owned_response_replacement(
                    app,
                    request,
                    continuation.clone(),
                    target_ownership,
                )
                .await
            }
        };
        #[cfg(not(test))]
        let created = crate::commands::session::create_owned_response_replacement(
            app,
            request,
            continuation.clone(),
            target_ownership,
        )
        .await;
        Self::owned_pty_spawn_outcome(created, continuation)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn spawn_pty_input_target<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        matching: &[SessionInfo],
        verified_sender: &crate::config::teams::VerifiedPtyInputIdentity,
        verified_target: &crate::config::teams::VerifiedPtyInputIdentity,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
        target_ownership: &crate::api::message_store::PtyInputTargetOwnership<'_>,
        initial: &crate::phone::types::PtyInputResult,
        prepared_replacement: Option<crate::session::selection::PreparedOwnedReplacement>,
        source_executor: &mut crate::session::selection::SourceExecutorGuard,
        #[cfg(test)] fixture_lease: Option<&Arc<crate::commands::session::FixturePtyCutLease>>,
    ) -> Result<PtySpawnStep, crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;

        if verified_target.canonical_fqn != claimed.target_fqn
            || !target_ownership.proves(&verified_target.canonical_fqn)
        {
            return Err(C::AuthorityChanged);
        }
        let target_root = verified_target.replica_root.clone();
        let target_root_text = target_root.to_str().ok_or(C::UnsafePath)?.to_string();
        let settings_state = app.state::<SettingsState>();
        let settings = settings_state.read().await.clone();
        let (agent_id, spawn, exited) = Self::resolve_pty_target_spawn(
            &target_root,
            &target_root_text,
            &settings,
            claimed,
            matching,
            verified_target,
        )?;
        // #1271 - same-snapshot host shell: built from the SAME cloned
        // AppSettings that resolved the spawn above (mailbox.rs:5420), before
        // any await, so the pair can never mix across a configuration change.
        let resolved_agent_host_shell = Some(crate::pty::backend::ResolvedAgentHostShell {
            program: settings.default_shell.clone(),
            args: settings.default_shell_args.clone(),
        });
        let expected_backend = SessionBackendKind::from(&spawn.backend);
        let carried = exited.map(|session| {
            (
                session.telegram_bot_id.clone(),
                session.communication.clone(),
            )
        });
        Self::require_pty_spawn_authority(
            self.pty_spawn_authority_is_current(
                app,
                claimed,
                verified_sender,
                verified_target,
                source_plane,
                api_client_store,
            )
            .await,
        )?;
        // No destroy/create/native preparation has started. Return ownership
        // to dispatch rather than waiting under its borrowed target guards.
        use crate::config::sessions_persistence::CreateCarrierScope;
        let (scope, retained) = observe_pty_spawn_scope(app, verified_target).await;
        if !matches!(scope, CreateCarrierScope::Foreign { .. }) {
            let Some(retained_id) = retained else {
                return Ok(PtySpawnStep::NeedReacquire(prepared_replacement));
            };
            let exited_id = Self::retained_pty_target_id(expected_backend, exited, retained_id)?;
            let (_, local_name) = crate::config::teams::split_project_prefix(&claimed.target_fqn);
            return self
                .spawn_owned_pty_replacement(
                    app,
                    Self::pty_replacement_request(
                        spawn,
                        agent_id,
                        &target_root_text,
                        local_name,
                        resolved_agent_host_shell,
                    ),
                    (claimed, initial, verified_sender, api_client_store),
                    (verified_target, exited_id),
                    prepared_replacement,
                    target_ownership,
                    source_executor,
                    #[cfg(test)]
                    fixture_lease,
                )
                .await;
        }
        drop(prepared_replacement);
        if let Some(exited) = exited {
            let exited_id = crate::phone::types::parse_canonical_uuid_v4(&exited.id)
                .map_err(|_| C::InconsistentSession)?;
            let destroy_result =
                crate::commands::session::background_destroy_session_inner(app, exited_id).await;
            let session_manager = app
                .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                .inner()
                .clone();
            let remaining = {
                let guard = session_manager.read().await;
                guard.list_sessions().await
            };
            let selected_survived = remaining.iter().any(|session| session.id == exited.id);
            let live_appeared = remaining
                .iter()
                .any(|session| Self::is_live_pty_target(session, verified_target));
            let route_or_spawn_survived = {
                let manager = app.state::<Arc<Mutex<PtyManager>>>();
                let manager = manager.lock().unwrap_or_else(|error| error.into_inner());
                manager.backend_kind(exited_id).is_some()
                    || manager.has_pending_spawn_for_replica(&verified_target.replica_identity)
            };
            if selected_survived || live_appeared || route_or_spawn_survived {
                return Err(C::SessionRace);
            }
            if destroy_result.is_err() {
                log::debug!(
                    "[pty-input] exited destroy reported failure but relist proved removal id={}",
                    claimed.injection_id
                );
            }
        }
        Self::require_pty_spawn_authority(
            self.pty_spawn_authority_is_current(
                app,
                claimed,
                verified_sender,
                verified_target,
                source_plane,
                api_client_store,
            )
            .await,
        )?;
        let session_manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .inner()
            .clone();
        let pty_manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let (_, local_name) = crate::config::teams::split_project_prefix(&claimed.target_fqn);
        let created = crate::commands::session::create_session_inner_with_pty_target_ownership(
            app,
            &session_manager,
            &pty_manager,
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            target_root_text,
            Some(local_name.to_string()),
            Some(agent_id),
            Some(spawn.trusted_agent_label.clone()),
            false,
            Vec::new(),
            exited.is_none(),
            Some(spawn),
            resolved_agent_host_shell,
            None,
            crate::commands::session::CreateSelectionIntent::Background,
            target_ownership,
        )
        .await;
        let Some((info, created_id)) = Self::resolve_created_pty_target(
            created,
            &session_manager,
            &pty_manager,
            verified_target,
            expected_backend,
            &settings,
        )
        .await?
        else {
            return Ok(PtySpawnStep::RetainedUnowned);
        };
        if let Some((telegram_bot_id, communication)) = carried {
            if telegram_bot_id.is_some() {
                self.attach_persisted_telegram_for_wake(
                    app,
                    created_id,
                    telegram_bot_id.as_deref(),
                )
                .await;
            }
            if let Some(communication) = communication {
                let restored = {
                    let guard = session_manager.read().await;
                    guard
                        .restore_communication(created_id, communication.clone())
                        .await
                };
                if restored {
                    crate::session::selection::publish_session_communication(
                        app,
                        created_id,
                        Some(&communication),
                    );
                }
            }
        }
        Ok(PtySpawnStep::Created(info))
    }
}
