use super::{
    PreparedSnapshotPayload, TerminalSnapshotRequesterSelector, TerminalSnapshotSourcePlane,
};
use crate::config::teams::VerifiedPtyInputIdentity;
use crate::pty::backend::SessionBackendKind;
use crate::pty::context_scrape::ContextSessionLiveness;
use crate::pty::manager::{PtyManager, PtySnapshotRouteProof};
use crate::session::manager::{
    SessionManager, TerminalSnapshotRequesterFact, TerminalSnapshotSessionFact,
};
use crate::session::session::{SessionStatus, TEMP_SESSION_PREFIX};
use std::path::Path;
use std::sync::Arc;
use terminal_snapshot_renderer::{
    render_png, terminal_snapshot_json_bytes_from_model, TerminalScreenModel,
    TerminalSnapshotFormat, TerminalSnapshotPayload, TerminalSnapshotReasonCode,
};
use uuid::Uuid;

pub(crate) struct RequesterProof {
    pub(crate) fact: TerminalSnapshotRequesterFact,
    pub(crate) identity: VerifiedPtyInputIdentity,
    pub(crate) cwd_identity: crate::path_identity::VerifiedPathIdentity,
    pub(crate) route: PtySnapshotRouteProof,
    pub(crate) host_token: Option<Uuid>,
}

pub(super) async fn prove_requester(
    manager: &SessionManager,
    pty_manager: &Arc<std::sync::Mutex<PtyManager>>,
    selector: TerminalSnapshotRequesterSelector,
    source_plane: TerminalSnapshotSourcePlane,
) -> Result<RequesterProof, TerminalSnapshotReasonCode> {
    let (fact, host_confinement, host_token) = match selector {
        TerminalSnapshotRequesterSelector::Host {
            token,
            expected_root,
            claimed_from,
        } => (
            manager
                .find_unique_live_snapshot_requester_by_token(token)
                .await
                .map_err(|_| TerminalSnapshotReasonCode::RequesterUnavailable)?,
            Some((expected_root, claimed_from)),
            Some(token),
        ),
        TerminalSnapshotRequesterSelector::ApiSession(id) => (
            manager
                .live_snapshot_requester_by_id(id)
                .await
                .ok_or(TerminalSnapshotReasonCode::RequesterUnavailable)?,
            None,
            None,
        ),
    };
    let expected_backend = match source_plane {
        TerminalSnapshotSourcePlane::HostCli => SessionBackendKind::LocalProcess,
        TerminalSnapshotSourcePlane::ContainerApi => SessionBackendKind::ContainerTransport,
    };
    if fact.backend_kind != expected_backend {
        return Err(TerminalSnapshotReasonCode::RequesterUnavailable);
    }
    if (!fact.is_root_agent && !fact.is_coordinator)
        || (source_plane == TerminalSnapshotSourcePlane::ContainerApi && fact.is_root_agent)
    {
        return Err(TerminalSnapshotReasonCode::NotAuthorized);
    }
    let cwd = fact.working_directory.clone();
    let is_root = fact.is_root_agent;
    let identity_task = tokio::task::spawn_blocking(move || {
        crate::logging::catch_payload_unwind(move || {
            let cwd_identity = crate::path_identity::verify_directory(std::path::Path::new(&cwd))?;
            let identity = if is_root {
                let identity = crate::config::teams::verify_terminal_snapshot_root_identity(
                    std::path::Path::new(&cwd),
                )?;
                if !crate::path_identity::same_object(&identity.replica_identity, &cwd_identity) {
                    return Err("requester_identity_invalid".to_string());
                }
                identity
            } else {
                let identity = crate::config::teams::verify_pty_input_coordinator_root(
                    std::path::Path::new(&cwd),
                )?;
                if !crate::path_identity::same_object(&identity.replica_identity, &cwd_identity) {
                    return Err("requester_identity_invalid".to_string());
                }
                identity
            };
            Ok::<_, String>((identity, cwd_identity))
        })
    })
    .await;
    let (identity, cwd_identity) = match crate::logging::collapse_payload_task(identity_task) {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => return Err(TerminalSnapshotReasonCode::RequesterUnavailable),
        Err(_) => {
            log::error!("[terminal-snapshot] stage=requester_task code=internal");
            return Err(TerminalSnapshotReasonCode::ServiceUnavailable);
        }
    };
    let route = PtyManager::snapshot_route_proof(pty_manager, fact.id)
        .map_err(|_| TerminalSnapshotReasonCode::RequesterUnavailable)?;
    if let Some((expected_root, claimed_from)) = host_confinement {
        if !crate::path_identity::same_object(&expected_root, &identity.replica_identity)
            || claimed_from != identity.canonical_fqn
        {
            return Err(TerminalSnapshotReasonCode::RequesterUnavailable);
        }
    }
    let expected_replica = if fact.is_root_agent {
        None
    } else {
        Some(&identity.replica_identity)
    };
    if route.backend_kind() != expected_backend
        || route.liveness() != ContextSessionLiveness::Live
        || !route.matches_requester_route(expected_backend, &cwd_identity, expected_replica)
    {
        return Err(TerminalSnapshotReasonCode::RequesterUnavailable);
    }
    Ok(RequesterProof {
        fact,
        identity,
        cwd_identity,
        route,
        host_token,
    })
}

pub(super) fn augment_coordinator_project(
    project_paths: &mut Vec<String>,
    requester: &VerifiedPtyInputIdentity,
) -> Result<(), TerminalSnapshotReasonCode> {
    let project = requester
        .ac_root_identity
        .canonical_path
        .parent()
        .and_then(Path::to_str)
        .map(crate::path_utils::normalize_windows_verbatim_path)
        .ok_or(TerminalSnapshotReasonCode::ServiceUnavailable)?;
    if !project_paths.contains(&project) {
        if project_paths.len() >= 4_096 {
            return Err(TerminalSnapshotReasonCode::ServiceUnavailable);
        }
        project_paths.push(project);
    }
    Ok(())
}

pub(super) fn same_authority(
    left: &VerifiedPtyInputIdentity,
    right: &VerifiedPtyInputIdentity,
) -> bool {
    left.canonical_fqn == right.canonical_fqn
        && crate::path_identity::same_object(&left.replica_identity, &right.replica_identity)
        && left.authority_fingerprint == right.authority_fingerprint
}

pub(super) fn authority_key(identity: &VerifiedPtyInputIdentity) -> String {
    format!(
        "{:016x}:{:016x}:{}",
        identity.replica_identity.object_id.volume,
        identity.replica_identity.object_id.file,
        identity.incarnation_fingerprint
    )
}

pub(crate) struct SelectedSession {
    pub(crate) fact: TerminalSnapshotSessionFact,
    pub(crate) cwd_identity: crate::path_identity::VerifiedPathIdentity,
    pub(crate) proof: PtySnapshotRouteProof,
}

pub(super) fn select_target_session(
    facts: Vec<TerminalSnapshotSessionFact>,
    proofs: Vec<Option<PtySnapshotRouteProof>>,
    target_root: &Path,
    target_identity: &crate::path_identity::VerifiedPathIdentity,
) -> Result<SelectedSession, TerminalSnapshotReasonCode> {
    if facts.len() != proofs.len() {
        return Err(TerminalSnapshotReasonCode::Internal);
    }
    let mut eligible = Vec::new();
    let mut unavailable = false;
    for (fact, proof) in facts.into_iter().zip(proofs) {
        if matches!(fact.status, SessionStatus::Exited(_))
            || fact.name.starts_with(TEMP_SESSION_PREFIX)
        {
            continue;
        }
        let lexical_target = std::path::Path::new(&fact.working_directory).starts_with(target_root);
        let cwd_identity = match crate::path_identity::verify_directory(std::path::Path::new(
            &fact.working_directory,
        )) {
            Ok(identity)
                if crate::path_identity::is_verified_descendant(&identity, target_identity) =>
            {
                identity
            }
            Ok(_) => continue,
            Err(_) => {
                if lexical_target {
                    unavailable = true;
                }
                continue;
            }
        };
        let Some(proof) = proof else {
            unavailable = true;
            continue;
        };
        let route_matches = proof.backend_kind() == fact.backend_kind
            && crate::path_identity::same_object(proof.saved_cwd(), &cwd_identity)
            && proof
                .saved_replica()
                .is_some_and(|replica| crate::path_identity::same_object(replica, target_identity));
        if !route_matches || proof.liveness() != ContextSessionLiveness::Live {
            unavailable = true;
            continue;
        }
        eligible.push(SelectedSession {
            fact,
            cwd_identity,
            proof,
        });
    }
    eligible.sort_by(|left, right| {
        status_rank(&left.fact.status)
            .cmp(&status_rank(&right.fact.status))
            .then_with(|| right.fact.created_at.cmp(&left.fact.created_at))
            .then_with(|| left.fact.id.as_bytes().cmp(right.fact.id.as_bytes()))
    });
    if let Some(selected) = eligible.into_iter().next() {
        Ok(selected)
    } else if unavailable {
        Err(TerminalSnapshotReasonCode::SnapshotUnavailable)
    } else {
        Err(TerminalSnapshotReasonCode::TargetUnavailable)
    }
}

pub(super) fn status_rank(status: &SessionStatus) -> u8 {
    match status {
        SessionStatus::Active => 0,
        SessionStatus::Running => 1,
        SessionStatus::Idle => 2,
        SessionStatus::Exited(_) => 3,
    }
}

pub(super) fn build_payload(
    format: TerminalSnapshotFormat,
    request_id: String,
    requester: String,
    target: String,
    model: Arc<TerminalScreenModel>,
) -> Result<PreparedSnapshotPayload, TerminalSnapshotReasonCode> {
    match format {
        TerminalSnapshotFormat::Json => {
            terminal_snapshot_json_bytes_from_model(&request_id, &requester, &target, &model)
                .map_err(|error| match error {
                    terminal_snapshot_renderer::ProtocolError::TooLarge => {
                        TerminalSnapshotReasonCode::SnapshotTooLarge
                    }
                    _ => TerminalSnapshotReasonCode::Internal,
                })?;
            Ok(PreparedSnapshotPayload::Json {
                request_id,
                requester,
                target,
                model,
            })
        }
        TerminalSnapshotFormat::Png => {
            let rendered = render_png(&model).map_err(|error| match error {
                terminal_snapshot_renderer::RenderError::TooLarge => {
                    TerminalSnapshotReasonCode::SnapshotTooLarge
                }
                _ => TerminalSnapshotReasonCode::RenderFailed,
            })?;
            let metadata = rendered.metadata(request_id, requester, target, &model);
            Ok(PreparedSnapshotPayload::Png(Box::new(
                TerminalSnapshotPayload::Png {
                    metadata,
                    png: rendered.bytes,
                },
            )))
        }
    }
}

pub(super) async fn clone_session_manager(
    state: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> SessionManager {
    let guard = state.read().await;
    guard.clone()
}
