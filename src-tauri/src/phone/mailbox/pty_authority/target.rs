use super::{
    owned_replacement_launch_recipe, read_guarded_agent_state,
    select_persistent_exited_pty_candidate, session_has_current_pty_submission_provenance, Arc,
    MailboxPoller, Mutex, Path, PtyManager, PtySpawnStep, SessionBackendKind, SessionInfo,
    SessionManager, SessionStatus, Uuid,
};

impl MailboxPoller {
    pub(crate) async fn prepare_pty_replacement(
        coordinator: &crate::session::selection::SelectionCoordinator,
        manager: &SessionManager,
        request: &crate::commands::session::OwnedReplacementCreateRequest,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        initial: &crate::phone::types::PtyInputResult,
        target: &crate::config::teams::VerifiedPtyInputIdentity,
        previous_id: Uuid,
    ) -> Result<
        Option<crate::session::selection::PreparedOwnedReplacement>,
        crate::phone::types::PtyInputReasonCode,
    > {
        use crate::phone::types::PtyInputReasonCode as C;
        use crate::session::selection::{OwnedReplacementWork, ResolvedReplacement};
        let directory = crate::config::config_dir().ok_or(C::UnsafePath)?;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let captured = manager
            .with_current_carrier_snapshot(&permit, |current| {
                let carrier =
                    crate::config::sessions_persistence::retained_carrier_for_replacement(
                        &permit,
                        &directory,
                        &target.canonical_fqn,
                        previous_id,
                        current,
                    )?;
                let row = current
                    .rows
                    .iter()
                    .find(|row| row.id == previous_id)?
                    .clone();
                Some((carrier, row))
            })
            .await;
        drop(permit);
        let Some((carrier, previous)) = captured else {
            return Ok(None);
        };
        let work = OwnedReplacementWork::PtyDurableReference {
            version: initial.version,
            injection_id: claimed.injection_id.clone(),
            op_id: claimed.op_id.clone(),
            sender: claimed.sender_fqn.clone(),
            target: claimed.target_fqn.clone(),
            source_plane: match claimed.source_plane {
                crate::phone::types::PtyInputSourcePlane::HostCli => "host_cli",
                crate::phone::types::PtyInputSourcePlane::ContainerApi => "container_api",
            }
            .into(),
            payload_sha256: initial.payload_sha256.clone().ok_or(C::AuthorityChanged)?,
            payload_bytes: initial.payload_bytes.ok_or(C::AuthorityChanged)?,
            issued_at: initial.issued_at.clone().ok_or(C::AuthorityChanged)?,
            expires_at: claimed.expires_at.clone(),
            queued_at: initial.queued_at.clone().ok_or(C::AuthorityChanged)?,
            lease_owner: claimed.lease_owner.clone(),
            attempt: claimed.attempt,
            authority_session_id: claimed.authority_session_id.clone(),
            authority_client_id: claimed.authority_client_id.clone(),
            authority_client_generation: claimed.authority_client_generation.clone(),
            sender_identity_fingerprint: claimed.sender_identity_fingerprint.clone(),
            target_identity_fingerprint: claimed.target_identity_fingerprint.clone(),
            requested_agent_id: claimed.requested_agent_id.clone(),
        };
        let cwd_identity = crate::path_identity::verify_directory(Path::new(&request.cwd))
            .map_err(|_| C::UnsafePath)?;
        Ok(Some(coordinator.prepare_owned_replacement(
            ResolvedReplacement {
                target: target.canonical_fqn.clone(),
                directory,
                previous,
                carrier,
                requested_agent_id: request.agent_id.clone(),
                requested_profile: None,
                shell: request.shell.clone(),
                shell_args: request.shell_args.clone(),
                launch_recipe: owned_replacement_launch_recipe(request),
                cwd: request.cwd.clone(),
                backend: SessionBackendKind::LocalProcess,
                work,
                cwd_identity: cwd_identity.into(),
                replica_identity: Some(target.replica_identity.clone().into()),
            },
            manager.reserve_carrier_create_binding(),
        )))
    }

    pub(crate) fn resolve_pty_target_spawn<'a>(
        target_root: &Path,
        target_root_text: &str,
        settings: &crate::config::settings::AppSettings,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        matching: &'a [SessionInfo],
        verified_target: &crate::config::teams::VerifiedPtyInputIdentity,
    ) -> Result<
        (
            String,
            crate::config::agent_command::AgentSpawnCommand,
            Option<&'a SessionInfo>,
        ),
        crate::phone::types::PtyInputReasonCode,
    > {
        use crate::phone::types::PtyInputReasonCode as C;
        let (config_bytes, _) = crate::path_identity::read_bounded_regular(
            &target_root.join("config.json"),
            1024 * 1024,
        )
        .map_err(|_| C::UnsafePath)?;
        let config_value = crate::path_identity::parse_json_no_duplicates(&config_bytes)
            .map_err(|_| C::UnsupportedProfile)?;
        // #2786 (C1) - the state file goes through the same guarded read as
        // config.json (regular file, no-follow, size, duplicate keys); only a
        // missing file is absence. The shared loader's policy overlays it.
        let state_value = read_guarded_agent_state(target_root)?;
        let config_value =
            crate::config::agent_config::overlay_agent_local_state(config_value, state_value);
        let local_config =
            serde_json::from_value::<crate::config::agent_config::AgentLocalConfig>(config_value)
                .map_err(|_| C::UnsupportedProfile)?;
        let exited = select_persistent_exited_pty_candidate(matching, |session| {
            crate::config::teams::verify_pty_input_replica_cwd(Path::new(
                &session.working_directory,
            ))
            .is_ok_and(|identity| {
                identity.canonical_fqn == verified_target.canonical_fqn
                    && identity.authority_fingerprint == verified_target.authority_fingerprint
            })
        });
        let mut candidates = Vec::new();
        if let Some(id) = claimed.requested_agent_id.as_ref() {
            candidates.push(id.clone());
        } else {
            if let Some(id) = exited.and_then(|session| session.agent_id.clone()) {
                candidates.push(id);
            }
            if let Some(id) = local_config.tooling.current_coding_agent.clone() {
                candidates.push(id);
            }
            if let Some(id) = local_config.tooling.last_coding_agent.clone() {
                candidates.push(id);
            }
            candidates.extend(settings.agents.iter().map(|agent| agent.id.clone()));
        }
        candidates.dedup();
        let mut resolved = None;
        for id in candidates {
            let Ok(Some(spawn)) = crate::commands::session::build_configured_agent_spawn_for_cwd(
                settings,
                &id,
                target_root_text,
                None,
            ) else {
                if claimed.requested_agent_id.is_some() {
                    return Err(C::UnsupportedProfile);
                }
                continue;
            };
            let hint =
                crate::session::profile::CodingAgentKind::detect(&spawn.shell, &spawn.shell_args);
            if crate::session::profile::detect_configured_pty_submission_agent(
                &spawn.shell,
                &spawn.shell_args,
                hint,
            )
            .is_some()
            {
                resolved = Some((id, spawn));
                break;
            }
            if claimed.requested_agent_id.is_some() {
                return Err(C::UnsupportedProfile);
            }
        }
        let (agent_id, spawn) = resolved.ok_or(C::UnsupportedProfile)?;
        Ok((agent_id, spawn, exited))
    }

    pub(crate) fn validate_created_pty_target(
        info: &SessionInfo,
        pty_manager: &Arc<Mutex<PtyManager>>,
        expected_backend: SessionBackendKind,
        verified_target: &crate::config::teams::VerifiedPtyInputIdentity,
        settings: &crate::config::settings::AppSettings,
    ) -> Result<Uuid, crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;
        let created_id = crate::phone::types::parse_canonical_uuid_v4(&info.id)
            .map_err(|_| C::InconsistentSession)?;
        let created_target =
            crate::config::teams::verify_pty_input_replica_cwd(Path::new(&info.working_directory))
                .map_err(|_| C::InconsistentSession)?;
        let route_valid = {
            let manager = pty_manager
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let identities = manager.route_identities(created_id);
            manager.has_session(created_id)
                && manager.backend_kind(created_id) == Some(expected_backend)
                && identities.is_some_and(|(cwd, replica, _)| {
                    cwd.as_ref().is_some_and(|cwd| {
                        crate::path_identity::is_verified_descendant(
                            cwd,
                            &verified_target.replica_identity,
                        )
                    }) && replica.as_ref().is_some_and(|replica| {
                        crate::path_identity::same_object(
                            replica,
                            &verified_target.replica_identity,
                        )
                    })
                })
        };
        if created_target.canonical_fqn != verified_target.canonical_fqn
            || created_target.authority_fingerprint != verified_target.authority_fingerprint
            || info.backend_kind != expected_backend
            || !session_has_current_pty_submission_provenance(info, settings)
            || !route_valid
        {
            return Err(C::InconsistentSession);
        }
        Ok(created_id)
    }
    pub(crate) fn owned_pty_spawn_outcome(
        created: Result<crate::commands::session::OwnedReplacementCreateStep, String>,
        continuation: crate::session::selection::OwnedReplacementContinuation,
    ) -> Result<PtySpawnStep, crate::phone::types::PtyInputReasonCode> {
        match created {
            Ok(crate::commands::session::OwnedReplacementCreateStep::Finalized(info)) => {
                Ok(PtySpawnStep::OwnedCreated(info, continuation))
            }
            Ok(crate::commands::session::OwnedReplacementCreateStep::Retained) => {
                Ok(PtySpawnStep::Retained {
                    continuation,
                    failed: false,
                })
            }
            Err(error) => {
                // A returned create error does not certify runtime rollback or
                // durable source handoff. Keep the accepted owner authoritative.
                continuation.note_retained_work(error);
                Ok(PtySpawnStep::Retained {
                    continuation,
                    failed: true,
                })
            }
        }
    }

    pub(crate) async fn classify_pty_spawn_failure(
        session_manager: &Arc<tokio::sync::RwLock<SessionManager>>,
        pty_manager: &Arc<Mutex<PtyManager>>,
        verified_target: &crate::config::teams::VerifiedPtyInputIdentity,
        error: &str,
    ) -> crate::phone::types::PtyInputReasonCode {
        use crate::phone::types::PtyInputReasonCode as C;
        let sessions = {
            let guard = session_manager.read().await;
            guard.list_sessions().await
        };
        let ambiguous = sessions.iter().any(|session| {
            !matches!(session.status, SessionStatus::Exited(_))
                && crate::config::teams::verify_pty_input_replica_cwd(Path::new(
                    &session.working_directory,
                ))
                .is_ok_and(|identity| {
                    identity.canonical_fqn == verified_target.canonical_fqn
                        && identity.authority_fingerprint == verified_target.authority_fingerprint
                })
        }) || pty_manager
            .lock()
            .unwrap_or_else(|lock_error| lock_error.into_inner())
            .has_pending_spawn_for_replica(&verified_target.replica_identity);
        if error == "sessionRace" {
            C::SessionRace
        } else if ambiguous {
            C::InconsistentSession
        } else {
            C::SpawnFailedSafe
        }
    }

    pub(crate) fn pty_replacement_request(
        spawn: crate::config::agent_command::AgentSpawnCommand,
        agent_id: String,
        target_root_text: &str,
        local_name: &str,
        resolved_agent_host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>,
    ) -> crate::commands::session::OwnedReplacementCreateRequest {
        crate::commands::session::OwnedReplacementCreateRequest {
            shell: spawn.shell.clone(),
            shell_args: spawn.shell_args.clone(),
            cwd: crate::path_utils::normalize_windows_verbatim_path(target_root_text),
            name: Some(local_name.to_string()),
            agent_id: Some(agent_id),
            agent_label: Some(spawn.trusted_agent_label.clone()),
            skip_tooling_save: false,
            git_repos: Vec::new(),
            resolved_spawn: Some(spawn),
            host_shell: resolved_agent_host_shell,
        }
    }
    pub(crate) fn validate_pty_replacement_launch(
        prepared: &crate::session::selection::PreparedOwnedReplacement,
        request: &crate::commands::session::OwnedReplacementCreateRequest,
    ) -> Result<(), crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;
        if !prepared.matches_launch(
            &request.shell,
            &request.shell_args,
            &request.cwd,
            &request.agent_id,
            &owned_replacement_launch_recipe(request),
        ) {
            return Err(C::AuthorityChanged);
        }
        Ok(())
    }

    pub(crate) fn is_live_pty_target(
        session: &SessionInfo,
        verified_target: &crate::config::teams::VerifiedPtyInputIdentity,
    ) -> bool {
        !matches!(session.status, SessionStatus::Exited(_))
            && crate::config::teams::verify_pty_input_replica_cwd(Path::new(
                &session.working_directory,
            ))
            .is_ok_and(|identity| {
                identity.canonical_fqn == verified_target.canonical_fqn
                    && identity.authority_fingerprint == verified_target.authority_fingerprint
            })
    }

    pub(crate) async fn resolve_created_pty_target(
        created: Result<crate::commands::session::PtyTargetCreateStep, String>,
        session_manager: &Arc<tokio::sync::RwLock<SessionManager>>,
        pty_manager: &Arc<Mutex<PtyManager>>,
        verified_target: &crate::config::teams::VerifiedPtyInputIdentity,
        expected_backend: SessionBackendKind,
        settings: &crate::config::settings::AppSettings,
    ) -> Result<Option<(SessionInfo, Uuid)>, crate::phone::types::PtyInputReasonCode> {
        let info = match created {
            Ok(crate::commands::session::PtyTargetCreateStep::Finalized(info)) => info,
            Ok(crate::commands::session::PtyTargetCreateStep::Retained) => return Ok(None),
            Err(error) => {
                return Err(Self::classify_pty_spawn_failure(
                    session_manager,
                    pty_manager,
                    verified_target,
                    &error,
                )
                .await);
            }
        };
        let created_id = Self::validate_created_pty_target(
            &info,
            pty_manager,
            expected_backend,
            verified_target,
            settings,
        )?;
        Ok(Some((info, created_id)))
    }
    pub(crate) fn require_pty_spawn_authority(
        current: Result<bool, crate::api::auth::FreshRegistryError>,
    ) -> Result<(), crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;
        match current {
            Ok(true) => Ok(()),
            Ok(false) => Err(C::AuthorityChanged),
            Err(_) => Err(C::StoreTransient),
        }
    }

    pub(crate) fn retained_pty_target_id(
        expected_backend: SessionBackendKind,
        exited: Option<&SessionInfo>,
        retained_id: Uuid,
    ) -> Result<Uuid, crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;
        if expected_backend != SessionBackendKind::LocalProcess {
            return Err(C::AuthorityChanged);
        }
        let exited = exited.ok_or(C::InconsistentSession)?;
        let exited_id = crate::phone::types::parse_canonical_uuid_v4(&exited.id)
            .map_err(|_| C::InconsistentSession)?;
        if exited_id != retained_id {
            return Err(C::InconsistentSession);
        }

        Ok(exited_id)
    }
}
