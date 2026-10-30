use crate::pty::manager::PtyManager;
use crate::session::manager::acquire_carrier_mutation_permit;
use crate::session::manager::SessionManager;
use crate::session::selection::SelectionTransaction;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Manager;
use uuid::Uuid;

pub(crate) struct LifecycleSourceIdentity {
    id: Uuid,
    token: Uuid,
    version: u64,
    saved_cwd: String,
    directory: crate::path_identity::VerifiedPathIdentity,
    replica: Option<crate::path_identity::VerifiedPathIdentity>,
}

pub(crate) enum LifecycleTargetLease {
    Keyed {
        key: String,
        destination: crate::path_identity::VerifiedPathIdentity,
        gate: Arc<crate::api::message_store::PtyInputTargetGate>,
        exact: crate::api::message_store::PtyInputTargetGuard,
        stripe: crate::api::message_store::PtyInputStripeGuard,
    },
    VerifiedNoReplica {
        destination: crate::path_identity::VerifiedPathIdentity,
    },
    MissingSource {
        requested: Uuid,
    },
}

pub(crate) struct LifecycleEntryAuthority {
    lease: LifecycleTargetLease,
    source: Option<LifecycleSourceIdentity>,
    root_request: bool,
    operation: Uuid,
    reservation: Mutex<Option<crate::pty::manager::CreateOwnerReservation>>,
    pub(crate) interrupted: std::sync::atomic::AtomicBool,
    discard: Option<crate::config::sessions_persistence::PublishedCarrier>,
    pub(crate) tracker: Option<crate::pty::manager::ResponseCloseState>,
}

impl std::fmt::Debug for LifecycleEntryAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LifecycleEntryAuthority")
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "Preserve the same lifecycle lease and reservation authority and its drop boundaries"
)]
pub(crate) enum LifecycleAuthorityAdmission {
    Ready(LifecycleEntryAuthority),
    Waiting,
    NeedsRootDirectory,
}

impl LifecycleEntryAuthority {
    pub(crate) fn release(self) -> bool {
        let claim = match self.reservation.lock() {
            Ok(mut claim) => claim.take(),
            Err(_) => return false,
        };
        let claimed = claim.is_some();
        let tracker = self.tracker.clone();
        drop(claim);
        drop(self);
        // Drop's with_inner error permanently latches tracker failure. A
        // positive health query after actual Drop proves that release;
        // false/unknown keeps this entry unsafe without inspecting newer claims.
        !claimed || tracker.is_some_and(|tracker| tracker.is_healthy())
    }
    pub(crate) fn has_owned_claim(&self) -> bool {
        self.reservation
            .lock()
            .ok()
            .is_some_and(|claim| claim.is_some())
    }

    fn destination(&self) -> Option<&crate::path_identity::VerifiedPathIdentity> {
        match &self.lease {
            LifecycleTargetLease::Keyed { destination, .. }
            | LifecycleTargetLease::VerifiedNoReplica { destination } => Some(destination),
            LifecycleTargetLease::MissingSource { .. } => None,
        }
    }

    fn physical_identity_is_current(&self) -> bool {
        let matches = |expected: &crate::path_identity::VerifiedPathIdentity| {
            crate::path_identity::verify_directory(&expected.canonical_path).is_ok_and(|actual| {
                actual.object_id == expected.object_id
                    && actual.canonical_path == expected.canonical_path
            })
        };
        self.destination().is_some_and(matches)
            && self.source.as_ref().is_none_or(|source| {
                matches(&source.directory) && source.replica.as_ref().is_none_or(matches)
            })
    }

    pub(crate) fn target_ownership(
        &self,
    ) -> Result<Option<crate::api::message_store::PtyInputTargetOwnership<'_>>, String> {
        match &self.lease {
            LifecycleTargetLease::Keyed {
                key,
                gate,
                stripe,
                exact,
                ..
            } => gate
                .target_ownership(key, stripe, exact)
                .map(Some)
                .map_err(|_| "invalidTargetCreateOwnership".to_string()),
            _ => Ok(None),
        }
    }

    pub(crate) fn validates_destination(&self, cwd: &str, key: Option<&str>) -> bool {
        let Some(expected) = self.destination() else {
            return false;
        };
        let Ok(current) = crate::path_identity::verify_directory(Path::new(cwd)) else {
            return false;
        };
        if current.object_id != expected.object_id
            || current.canonical_path != expected.canonical_path
        {
            return false;
        }
        match &self.lease {
            LifecycleTargetLease::Keyed { key: actual, .. } => key == Some(actual.as_str()),
            LifecycleTargetLease::VerifiedNoReplica { .. } => key.is_none(),
            _ => false,
        }
    }
}

pub(crate) struct LifecycleClaim {
    reservation: Option<crate::pty::manager::CreateOwnerReservation>,
    discard: Option<crate::config::sessions_persistence::PublishedCarrier>,
}

pub(crate) struct LifecycleClaimScope<'a> {
    source: Option<&'a LifecycleSourceIdentity>,
    source_key: Option<&'a str>,
    destination: &'a crate::path_identity::VerifiedPathIdentity,
    destination_key: Option<&'a str>,
    fallback_target: &'a str,
}

pub(crate) fn capture_lifecycle_claim(
    permit: &crate::session::manager::CarrierMutationPermit,
    current: &crate::session::manager::CarrierSnapshot,
    scope: LifecycleClaimScope<'_>,
    operation: Uuid,
    tracker: &crate::pty::manager::ResponseCloseState,
    pty: &Arc<Mutex<PtyManager>>,
    coordinator: Option<&crate::session::selection::SelectionCoordinator>,
) -> Option<LifecycleClaim> {
    let LifecycleClaimScope {
        source,
        source_key,
        destination,
        destination_key,
        fallback_target,
    } = scope;
    use crate::config::sessions_persistence::{
        classify_lifecycle_carrier_scope, CreateCarrierScope,
    };
    use crate::pty::manager::{CreateAdmission, CreateClassification, ResponseRouteIdentity};
    let destination_scope =
        classify_lifecycle_carrier_scope(permit, destination_key, &destination.canonical_path);
    let source_scope = source.map(|source| {
        classify_lifecycle_carrier_scope(permit, source_key, &source.directory.canonical_path)
    });
    let (target, classification, discard) = match source_scope {
        Some(CreateCarrierScope::Protected {
            target,
            session_id,
            token,
            version,
            epoch,
            preparing_owner: None,
        }) => {
            let source = source?;
            if source.id != session_id || source.token != token || source.version != version {
                return None;
            }
            match destination_scope {
                CreateCarrierScope::Foreign { .. } => {}
                CreateCarrierScope::Protected {
                    target: other_target,
                    session_id: other_id,
                    token: other_token,
                    version: other_version,
                    epoch: other_epoch,
                    preparing_owner: None,
                } if other_target == target
                    && other_id == session_id
                    && other_token == token
                    && other_version == version
                    && other_epoch == epoch => {}
                _ => return None,
            }
            let carrier = crate::config::sessions_persistence::manual_discard_carrier(
                permit,
                &source.directory.canonical_path,
                &target,
                session_id,
                current,
            )?;
            let row = current.rows.iter().find(|row| row.id == session_id)?;
            let generation = if let Some(historical) = carrier.retired_local_generation() {
                let physical_replica = crate::session::manager::CarrierPhysicalIdentity::from(
                    source.replica.as_ref()?.clone(),
                );
                if !carrier.retired_local_replica_matches(&physical_replica) {
                    return None;
                }
                if tracker.generation(session_id).is_some()
                    || !matches!(
                        crate::session::manager::RetiredCarrierRouteObservation::from(
                            PtyManager::snapshot_route_proof(pty, session_id)
                        ),
                        crate::session::manager::RetiredCarrierRouteObservation::Absent
                    )
                    || pty.lock().ok()?.backend_kind(session_id).is_some()
                {
                    return None;
                }
                historical
            } else {
                if matches!(row.status, SessionStatus::Exited(_)) {
                    return None;
                }
                let generation = tracker.generation(session_id)?;
                let route = PtyManager::snapshot_route_proof(pty, session_id).ok()?;
                let actual_generation = pty.lock().ok()?.route_identities(session_id)?.2;
                if actual_generation != generation
                    || !route.matches_requester_route(
                        row.backend_kind,
                        &source.directory,
                        source.replica.as_ref(),
                    )
                {
                    return None;
                }
                generation
            };
            let identity = ResponseRouteIdentity {
                target: target.clone(),
                session_id,
                token,
                generation,
                backend: row.backend_kind,
                carrier_version: version,
            };
            (
                target,
                CreateClassification::Protected {
                    identity,
                    publication_epoch: epoch,
                },
                Some(carrier),
            )
        }
        None | Some(CreateCarrierScope::Foreign { .. }) => {
            let CreateCarrierScope::Foreign { publication_epoch } = destination_scope else {
                return None;
            };
            (
                fallback_target.to_owned(),
                CreateClassification::Foreign { publication_epoch },
                None,
            )
        }
        _ => return None,
    };
    let foreign = matches!(&classification, CreateClassification::Foreign { .. });
    match tracker.reserve_create_owner(&target, operation, classification) {
        Ok(CreateAdmission::Reserved(reservation)) => Some(LifecycleClaim {
            reservation: Some(reservation),
            discard,
        }),
        Err(crate::pty::manager::ResponseCloseError::Unavailable)
            if foreign
                && !tracker.target_is_closing(&target)
                && coordinator.is_some_and(|coordinator| {
                    coordinator.retains_lifecycle_operation(operation)
                }) =>
        {
            // SAME permanently failed tracker, current ALL-scope Foreign and a
            // real upper retained owner. No healthy/Protected/unknown fallback.
            Some(LifecycleClaim {
                reservation: None,
                discard: None,
            })
        }
        _ => None,
    }
}

fn missing_lifecycle_source_admission(
    operation: Uuid,
    requested_source: Option<Uuid>,
    root_request: bool,
    source_is_pending: bool,
) -> LifecycleAuthorityAdmission {
    // A pending exact UUID is present, not a missing-source validation case.
    if source_is_pending {
        return LifecycleAuthorityAdmission::Waiting;
    }
    let Some(requested) = requested_source else {
        return LifecycleAuthorityAdmission::Waiting;
    };
    LifecycleAuthorityAdmission::Ready(LifecycleEntryAuthority {
        lease: LifecycleTargetLease::MissingSource { requested },
        source: None,
        root_request,
        operation,
        reservation: Mutex::new(None),
        interrupted: std::sync::atomic::AtomicBool::new(false),
        discard: None,
        tracker: None,
    })
}

fn prepared_lifecycle_identity_matches(
    lease: &LifecycleTargetLease,
    source: Option<&LifecycleSourceIdentity>,
) -> bool {
    let physical_matches = |expected: &crate::path_identity::VerifiedPathIdentity| {
        crate::path_identity::verify_directory(&expected.canonical_path).is_ok_and(|actual| {
            actual.object_id == expected.object_id
                && actual.canonical_path == expected.canonical_path
        })
    };
    let destination_matches = match lease {
        LifecycleTargetLease::Keyed { destination, .. }
        | LifecycleTargetLease::VerifiedNoReplica { destination } => physical_matches(destination),
        LifecycleTargetLease::MissingSource { .. } => false,
    };
    destination_matches && source.is_none_or(|source| physical_matches(&source.directory))
}

pub(crate) async fn prepare_lifecycle_entry_authority<R: tauri::Runtime>(
    app: &AppHandle<R>,
    operation: Uuid,
    requested_source: Option<Uuid>,
    root_request: bool,
    root_directory: Option<&str>,
) -> Result<LifecycleAuthorityAdmission, String> {
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let source_id = if root_request {
        manager
            .list_sessions()
            .await
            .into_iter()
            .find(|row| {
                row.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&row.working_directory)
            })
            .map(|row| Uuid::parse_str(&row.id))
            .transpose()
            .map_err(|error| error.to_string())?
    } else {
        requested_source
    };
    let snapshot = manager.with_response_close_facts(Clone::clone).await;
    let source_row = source_id.and_then(|id| {
        snapshot
            .rows
            .iter()
            .find(|row| row.id == id && !snapshot.pending_ids.contains(&row.id))
    });
    if !root_request && source_row.is_none() {
        return Ok(missing_lifecycle_source_admission(
            operation,
            requested_source,
            root_request,
            requested_source.is_some_and(|id| snapshot.rows.iter().any(|row| row.id == id)),
        ));
    }
    let needs_root = root_request
        || source_row.is_some_and(|row| {
            row.is_root_agent
                || crate::config::root_agent::is_root_agent_path(&row.working_directory)
        });
    if needs_root && root_directory.is_none() {
        return Ok(LifecycleAuthorityAdmission::NeedsRootDirectory);
    }
    let cwd = if needs_root {
        root_directory.unwrap_or_default()
    } else {
        source_row
            .map(|row| row.working_directory.as_str())
            .unwrap_or_default()
    };
    let Ok(destination) = crate::path_identity::verify_directory(Path::new(cwd)) else {
        return Ok(LifecycleAuthorityAdmission::Waiting);
    };
    let key = crate::config::teams::pty_input_create_gate_key_from_cwd(Path::new(cwd))
        .map_err(|_| "targetCreateGateUnavailable".to_string())?;
    let mut source = if let Some(row) = source_row {
        let Some(version) = snapshot.versions.get(&row.id).copied() else {
            return Ok(LifecycleAuthorityAdmission::Waiting);
        };
        let Ok(directory) =
            crate::path_identity::verify_directory(Path::new(&row.working_directory))
        else {
            return Ok(LifecycleAuthorityAdmission::Waiting);
        };
        Some(LifecycleSourceIdentity {
            id: row.id,
            token: row.token,
            version,
            saved_cwd: row.working_directory.clone(),
            directory,
            replica: None,
        })
    } else {
        None
    };
    let source_key = source_row
        .map(|row| {
            crate::config::teams::pty_input_create_gate_key_from_cwd(Path::new(
                &row.working_directory,
            ))
        })
        .transpose()
        .map_err(|_| "targetCreateGateUnavailable".to_string())?
        .flatten();
    if let Some(source) = source.as_mut() {
        source.replica = match crate::config::teams::strict_wg_replica_anchor_from_cwd(Path::new(
            &source.saved_cwd,
        )) {
            Ok(Some(root)) => match crate::path_identity::verify_directory(&root) {
                Ok(identity) => Some(identity),
                Err(_) => return Ok(LifecycleAuthorityAdmission::Waiting),
            },
            Ok(None) => None,
            Err(_) => return Ok(LifecycleAuthorityAdmission::Waiting),
        };
    }
    let lease = if let Some(key) = key {
        let gate = app
            .try_state::<crate::api::message_store::PtyInputTargetGateState>()
            .and_then(|state| state.gate.as_ref().ok().map(Arc::clone))
            .or_else(|| {
                app.try_state::<crate::api::message_store::MessageStoreState>()
                    .and_then(|state| state.target_gate.as_ref().ok().map(Arc::clone))
            })
            .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        let stripe = gate
            .acquire_target_lock(&key)
            .await
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        let exact = gate.acquire_exact(&key).await;
        LifecycleTargetLease::Keyed {
            key,
            destination,
            gate,
            stripe,
            exact,
        }
    } else {
        LifecycleTargetLease::VerifiedNoReplica { destination }
    };
    // Gate acquisition may have suspended. Never admit an identity verified
    // only before those waits; cancellation drops exact then stripe normally.
    if !prepared_lifecycle_identity_matches(&lease, source.as_ref()) {
        drop(lease);
        return Ok(LifecycleAuthorityAdmission::Waiting);
    }
    let destination = match &lease {
        LifecycleTargetLease::Keyed { destination, .. }
        | LifecycleTargetLease::VerifiedNoReplica { destination } => destination,
        _ => unreachable!("prepared destination"),
    };
    let destination_key = match &lease {
        LifecycleTargetLease::Keyed { key, .. } => Some(key.as_str()),
        _ => None,
    };
    let tracker = app
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .response_close_state();
    let target = destination_key
        .map(str::to_owned)
        .or(source_key.clone())
        .unwrap_or_else(|| format!("create-cwd:{}", destination.canonical_path.display()));
    let permit = acquire_carrier_mutation_permit().await;
    let reservation = manager
        .with_current_carrier_snapshot(&permit, |current| {
            if !lifecycle_source_matches(current, source.as_ref(), root_request) {
                return None;
            }
            capture_lifecycle_claim(
                &permit,
                current,
                LifecycleClaimScope {
                    source: source.as_ref(),
                    source_key: source_key.as_deref(),
                    destination,
                    destination_key,
                    fallback_target: &target,
                },
                operation,
                &tracker,
                app.state::<Arc<Mutex<PtyManager>>>().inner(),
                app.try_state::<crate::session::selection::SelectionCoordinator>()
                    .as_deref(),
            )
        })
        .await;
    drop(permit);
    let Some(reservation) = reservation else {
        drop(lease);
        return Ok(LifecycleAuthorityAdmission::Waiting);
    };
    Ok(LifecycleAuthorityAdmission::Ready(
        LifecycleEntryAuthority {
            lease,
            source,
            root_request,
            operation,
            reservation: Mutex::new(reservation.reservation),
            interrupted: std::sync::atomic::AtomicBool::new(false),
            discard: reservation.discard,
            tracker: Some(tracker),
        },
    ))
}

pub(crate) async fn validate_lifecycle_entry_authority<R: tauri::Runtime>(
    app: &AppHandle<R>,
    authority: &mut LifecycleEntryAuthority,
) -> bool {
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    if let LifecycleTargetLease::MissingSource { requested } = &authority.lease {
        let permit = acquire_carrier_mutation_permit().await;
        return manager
            .with_current_carrier_snapshot(&permit, |current| {
                !current.rows.iter().any(|row| row.id == *requested)
            })
            .await;
    }
    let Some(destination) = authority.destination().cloned() else {
        return false;
    };
    if !authority.physical_identity_is_current() {
        return false;
    }
    let Ok(key) =
        crate::config::teams::pty_input_create_gate_key_from_cwd(&destination.canonical_path)
    else {
        return false;
    };
    if !authority.validates_destination(
        &destination.canonical_path.to_string_lossy(),
        key.as_deref(),
    ) {
        return false;
    }
    let source_key = match authority
        .source
        .as_ref()
        .map(|source| {
            crate::config::teams::pty_input_create_gate_key_from_cwd(
                &source.directory.canonical_path,
            )
        })
        .transpose()
    {
        Ok(key) => key.flatten(),
        Err(_) => return false,
    };
    let target = key
        .clone()
        .or(source_key.clone())
        .unwrap_or_else(|| format!("create-cwd:{}", destination.canonical_path.display()));
    let tracker = app
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .response_close_state();
    let permit = acquire_carrier_mutation_permit().await;
    {
        let Ok(mut owned_claim) = authority.reservation.lock() else {
            return false;
        };
        drop(owned_claim.take());
    }
    let reservation = manager
        .with_current_carrier_snapshot(&permit, |current| {
            if !lifecycle_source_matches(current, authority.source.as_ref(), authority.root_request)
            {
                return None;
            }
            capture_lifecycle_claim(
                &permit,
                current,
                LifecycleClaimScope {
                    source: authority.source.as_ref(),
                    source_key: source_key.as_deref(),
                    destination: &destination,
                    destination_key: key.as_deref(),
                    fallback_target: &target,
                },
                authority.operation,
                &tracker,
                app.state::<Arc<Mutex<PtyManager>>>().inner(),
                app.try_state::<crate::session::selection::SelectionCoordinator>()
                    .as_deref(),
            )
        })
        .await;
    drop(permit);
    let same_discard = match (
        &authority.discard,
        reservation
            .as_ref()
            .and_then(|claim| claim.discard.as_ref()),
    ) {
        (None, None) => true,
        (Some(expected), Some(actual)) => {
            expected.session_id == actual.session_id
                && expected.token == actual.token
                && expected.version == actual.version
                && expected.nonce == actual.nonce
                && expected.target == actual.target
                && expected.directory == actual.directory
        }
        _ => false,
    };
    if !same_discard {
        return false;
    }
    let Ok(mut owned_claim) = authority.reservation.lock() else {
        return false;
    };
    let admitted = reservation.is_some();
    *owned_claim = reservation.and_then(|claim| claim.reservation);
    admitted
}

/// Last prerequisite-to-teardown boundary. Re-admit the SAME claim under U/M,
/// then retire only its exact protection. No waiting or session deletion here.
pub(crate) async fn retire_lifecycle_entry_protection<R: tauri::Runtime>(
    app: &AppHandle<R>,
    authority: &LifecycleEntryAuthority,
) -> bool {
    if !authority.physical_identity_is_current() {
        return false;
    }
    let Some(destination) = authority.destination() else {
        return false;
    };
    let Ok(destination_key) =
        crate::config::teams::pty_input_create_gate_key_from_cwd(&destination.canonical_path)
    else {
        return false;
    };
    if !authority.validates_destination(
        &destination.canonical_path.to_string_lossy(),
        destination_key.as_deref(),
    ) {
        return false;
    }
    let source_key = match authority
        .source
        .as_ref()
        .map(|source| {
            crate::config::teams::pty_input_create_gate_key_from_cwd(
                &source.directory.canonical_path,
            )
        })
        .transpose()
    {
        Ok(key) => key.flatten(),
        Err(_) => return false,
    };
    let target = destination_key
        .clone()
        .or(source_key.clone())
        .unwrap_or_else(|| format!("create-cwd:{}", destination.canonical_path.display()));
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let pty = app.state::<Arc<Mutex<PtyManager>>>();
    let tracker = match pty.lock() {
        Ok(pty) => pty.response_close_state(),
        Err(_) => return false,
    };
    let permit = acquire_carrier_mutation_permit().await;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            if !lifecycle_source_matches(current, authority.source.as_ref(), authority.root_request)
            {
                return false;
            }
            let Ok(mut owned) = authority.reservation.lock() else {
                return false;
            };
            drop(owned.take());
            let Some(claim) = capture_lifecycle_claim(
                &permit,
                current,
                LifecycleClaimScope {
                    source: authority.source.as_ref(),
                    source_key: source_key.as_deref(),
                    destination,
                    destination_key: destination_key.as_deref(),
                    fallback_target: &target,
                },
                authority.operation,
                &tracker,
                pty.inner(),
                app.try_state::<crate::session::selection::SelectionCoordinator>()
                    .as_deref(),
            ) else {
                return false;
            };
            let exact = match (&authority.discard, &claim.discard) {
                (None, None) => true,
                (Some(expected), Some(actual)) => {
                    expected.session_id == actual.session_id
                        && expected.token == actual.token
                        && expected.version == actual.version
                        && expected.nonce == actual.nonce
                        && expected.target == actual.target
                }
                _ => false,
            };
            if !exact {
                return false;
            }
            if let Some(carrier) = &claim.discard {
                let Some(source) = authority.source.as_ref() else {
                    return false;
                };
                if !crate::config::sessions_persistence::retire_manual_discard_carrier(
                    &permit,
                    carrier,
                    &source.directory.canonical_path,
                    current,
                ) {
                    return false;
                }
                tracker.invalidate_manager_row(carrier.session_id, std::time::Instant::now());
                crate::session::selection::forget_response_close_registration(carrier.session_id);
            }
            *owned = claim.reservation;
            true
        })
        .await
}

pub(crate) fn lifecycle_source_matches(
    current: &crate::session::manager::CarrierSnapshot,
    source: Option<&LifecycleSourceIdentity>,
    root_request: bool,
) -> bool {
    let exact = source.is_none_or(|source| {
        current.rows.iter().any(|row| {
            row.id == source.id
                && row.token == source.token
                && current.versions.get(&row.id) == Some(&source.version)
                && !current.pending_ids.contains(&row.id)
                && row.working_directory == source.saved_cwd
        })
    });
    if !exact {
        return false;
    }
    if root_request {
        let mut roots = current.rows.iter().filter(|row| {
            !current.pending_ids.contains(&row.id)
                && (row.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&row.working_directory))
        });
        let selected = roots.next().map(|row| row.id);
        selected == source.map(|source| source.id) && roots.next().is_none()
    } else {
        true
    }
}

pub(crate) async fn prepare_restore_lifecycle_authority<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    source: Option<Uuid>,
    root_request: bool,
) -> Result<(LifecycleEntryAuthority, Option<String>), String> {
    let Some(operation) = transaction.current_restore_nonce() else {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    };
    let mut root_directory = if root_request {
        Some(crate::config::root_agent::ensure_root_agent_dir()?)
    } else {
        None
    };
    loop {
        if !transaction.has_active_restore_capability() {
            return Err(
                crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
            );
        }
        match prepare_lifecycle_entry_authority(
            transaction.app(),
            operation,
            source,
            root_request,
            root_directory.as_deref(),
        )
        .await?
        {
            LifecycleAuthorityAdmission::Ready(mut authority) => {
                if authority.discard.is_some() {
                    transaction.invalidate_restore_ownership();
                    return Err(
                        crate::session::selection::SelectionCoordinatorError::Unavailable
                            .to_string(),
                    );
                }
                if !validate_lifecycle_entry_authority(transaction.app(), &mut authority).await {
                    transaction.invalidate_restore_ownership();
                    return Err(
                        crate::session::selection::SelectionCoordinatorError::Unavailable
                            .to_string(),
                    );
                }
                return Ok((authority, root_directory));
            }
            LifecycleAuthorityAdmission::NeedsRootDirectory => {
                root_directory = Some(crate::config::root_agent::ensure_root_agent_dir()?);
            }
            LifecycleAuthorityAdmission::Waiting => {
                transaction.invalidate_restore_ownership();
                // Startup cannot wait for a protected finalizer queued behind
                // its own real barrier. No ownership is inferred from Running.
                return Err(
                    crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
                );
            }
        }
    }
}
