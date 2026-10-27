use super::handoff::wake_claim_origin;
use super::notice::sender_name_for_session_cwd_with_root_flag;
use super::poller::{MailboxPoller, PreparedResponseRequest};
#[cfg(test)]
use super::pty_claim::MailboxTestHooks;
use super::pty_claim::{response_diag_debug_only, with_verified_response_pair, ResponseWakeWork};
use super::pty_observation::PtyAuthorityState;
use super::response_observation::{ResponseAdmissionState, ResponseCompletionState};
use crate::config::teams;
use crate::phone::types::OutboxMessage;
use crate::pty::backend::SessionBackendKind;
use crate::session::session::Session;
use crate::session::session::SessionStatus;
use crate::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use uuid::Uuid;

const HOST_PTY_CORE_LIMIT: usize = 8;

impl MailboxPoller {
    pub(crate) fn settle_response_source(&self, path: &Path, _success: bool) {
        let origin = wake_claim_origin(path).unwrap_or_else(|| path.to_path_buf());
        let work = self
            .response_sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&origin);
        if let Some(work) = work {
            let now = std::time::Instant::now();
            work.state.settle_terminal(&work.target, &work.key, now);
            if let Some(context) = work.return_context {
                // No AppHandle/current manager and route boundary exists here.
                // A synchronous source settlement can only terminate a return.
                let _ = work.state.settle_return_terminal(&context, now);
            }
        }
    }

    pub(crate) async fn prepare_response_request<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        session_id: Uuid,
        comanaged: bool,
        response: Option<&ResponseWakeWork>,
    ) -> Option<PreparedResponseRequest> {
        response_diag_local!("prepare", "entry", Uuid::parse_str(&msg.id).ok());
        let work = response_diag_option_local!(
            response,
            "prepare",
            "response_work_missing",
            Uuid::parse_str(&msg.id).ok()
        )?;
        if comanaged
            || msg.command.is_some()
            || msg.action.is_some()
            || msg.get_output
            || msg.pty_input.is_some()
            || msg.from.rsplit_once('/').map(|(room, _)| room)
                != msg.to.rsplit_once('/').map(|(room, _)| room)
        {
            response_diag_local!(
                "prepare",
                "excluded_message_class_or_room",
                Uuid::parse_str(&msg.id).ok()
            );
            return None;
        }
        let token = response_diag_option_local!(
            Uuid::parse_str(response_diag_option_local!(
                msg.token.as_deref(),
                "prepare",
                "token_missing",
                Uuid::parse_str(&msg.id).ok()
            )?)
            .ok(),
            "prepare",
            "token_invalid",
            Uuid::parse_str(&msg.id).ok()
        )?;
        let roots = app.pty_settings().read().await.project_paths.clone();
        response_diag_option_local!(
            teams::verified_wg_coordinator_target(&msg.from, &roots),
            "prepare",
            "configured_coordinator_rejected",
            Uuid::parse_str(&msg.id).ok()
        )?;
        let manager = app.pty_sessions().read().await.clone();
        let snapshot = manager.aggregate_snapshot().await;
        let member = response_diag_option_local!(
            snapshot.sessions.iter().find(|row| row.id == session_id),
            "prepare",
            "member_absent",
            Uuid::parse_str(&msg.id).ok()
        )?;
        if sender_name_for_session_cwd_with_root_flag(
            &member.working_directory,
            member.is_root_agent,
        ) != msg.to
        {
            response_diag_local!(
                "prepare",
                "member_target_mismatch",
                Uuid::parse_str(&msg.id).ok()
            );
            return None;
        }
        let mut senders = snapshot
            .sessions
            .iter()
            .filter(|row| row.token == token && !matches!(row.status, SessionStatus::Exited(_)));
        let coordinator = response_diag_option_local!(
            senders.next(),
            "prepare",
            "sender_missing",
            Uuid::parse_str(&msg.id).ok()
        )?;
        if senders.next().is_some()
            || sender_name_for_session_cwd_with_root_flag(
                &coordinator.working_directory,
                coordinator.is_root_agent,
            ) != msg.from
        {
            response_diag_local!(
                "prepare",
                "sender_ambiguous_or_target_mismatch",
                Uuid::parse_str(&msg.id).ok()
            );
            return None;
        }
        let coordinator_id = coordinator.id;
        manager
            .with_response_close_facts(|current| {
                let member = response_diag_option_local!(
                    current.rows.iter().find(|row| {
                        row.id == session_id
                            && row.token == member.token
                            && !current.pending_ids.contains(&row.id)
                    }),
                    "prepare",
                    "current_member_absent_or_pending",
                    Uuid::parse_str(&msg.id).ok()
                )?;
                let coordinator = response_diag_option_local!(
                    current.rows.iter().find(|row| {
                        row.id == coordinator_id
                            && row.token == token
                            && !current.pending_ids.contains(&row.id)
                    }),
                    "prepare",
                    "current_coordinator_absent_or_pending",
                    Uuid::parse_str(&msg.id).ok()
                )?;
                let route = |row: &Session, target: &str| {
                    Some(crate::pty::manager::ResponseRouteIdentity {
                        target: target.to_owned(),
                        session_id: row.id,
                        token: row.token,
                        generation: response_diag_option_local!(
                            work.state.generation(row.id),
                            "prepare",
                            "route_generation_absent",
                            Uuid::parse_str(&msg.id).ok()
                        )?,
                        carrier_version: *response_diag_option_local!(
                            current.versions.get(&row.id),
                            "prepare",
                            "carrier_version_absent",
                            Uuid::parse_str(&msg.id).ok()
                        )?,
                        backend: row.backend_kind,
                    })
                };
                let prepared = PreparedResponseRequest {
                    member: route(member, &msg.to)?,
                    coordinator: route(coordinator, &msg.from)?,
                    roots,
                };
                response_diag_local!("prepare", "prepared", Uuid::parse_str(&msg.id).ok());
                Some(prepared)
            })
            .await
    }

    async fn complete_response_request<S: ResponseCompletionState + ?Sized>(
        app: &S,
        work: &ResponseWakeWork,
    ) {
        let attempt = {
            match work.attempt.lock() {
                Ok(attempt) => attempt.clone(),
                Err(_) => {
                    response_diag_local!(
                        "complete_request",
                        "attempt_lock_error",
                        work.diagnostic_message_id
                    );
                    None
                }
            }
        };
        let Some(attempt) = attempt else {
            response_diag_local!(
                "complete_request",
                "attempt_missing",
                work.diagnostic_message_id
            );
            return;
        };
        let candidate = {
            match attempt.candidate.lock() {
                Ok(candidate) => candidate.clone(),
                Err(_) => {
                    response_diag_local!(
                        "complete_request",
                        "candidate_lock_error",
                        work.diagnostic_message_id
                    );
                    None
                }
            }
        };
        let Some(candidate) = candidate else {
            response_diag_local!(
                "complete_request",
                "candidate_missing",
                work.diagnostic_message_id
            );
            return;
        };
        let Some(dir) = crate::config::config_dir() else {
            response_diag_local!(
                "complete_request",
                "config_dir_unavailable",
                work.diagnostic_message_id
            );
            return;
        };
        let roots = {
            let settings = app.pty_settings();
            let settings = settings.read().await;
            crate::config::sessions_persistence::session_retention_project_paths(&settings).ok()
        };
        let Some(roots) = roots else {
            response_diag_local!(
                "complete_request",
                "roots_unavailable",
                work.diagnostic_message_id
            );
            return;
        };
        let manager = app.pty_sessions().read().await.clone();
        let initial = if let Some(coordinator) = app.response_selection() {
            let Some(exclusion) = coordinator.restore_mint_exclusion() else {
                response_diag_local!(
                    "complete_request",
                    "restore_exclusion_missing",
                    work.diagnostic_message_id
                );
                return;
            };
            candidate.initial.clone().with_restore_exclusion(exclusion)
        } else {
            candidate.initial.clone()
        };
        let published = crate::config::sessions_persistence::persist_initial_carrier(
            &manager, dir, roots, initial,
        )
        .await;
        response_diag_debug_only(|| {
            let outcome = match &published {
                Err(_) => "persist_initial_carrier_error",
                Ok(None) => "persist_initial_carrier_none",
                Ok(Some(_)) => "persist_initial_carrier_confirmed",
            };
            response_diag_local!("complete_request", outcome, work.diagnostic_message_id);
        });
        let Ok(Some(published)) = published else {
            response_diag_local!(
                "complete_request",
                "published_not_confirmed",
                work.diagnostic_message_id
            );
            return;
        };
        let settings = app.pty_settings();
        let settings = settings.read().await;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        if app
            .response_selection()
            .is_some_and(|coordinator| coordinator.restore_is_active())
        {
            response_diag_local!(
                "complete_request",
                "restore_active",
                work.diagnostic_message_id
            );
            return;
        }
        let pty = app.pty_routes().clone();
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                let diagnostic_pair = with_verified_response_pair(
                    current,
                    &pty,
                    &work.state,
                    &candidate.member,
                    &candidate.coordinator,
                    |member, coordinator| {
                        let Ok(_candidate_guard) = attempt.candidate.lock() else {
                            work.state.fail_closed();
                            response_diag_local!(
                                "complete_request",
                                "candidate_lock_poisoned",
                                work.diagnostic_message_id
                            );
                            return;
                        };
                        if !attempt.armed.load(std::sync::atomic::Ordering::Acquire) {
                            response_diag_local!(
                                "complete_request",
                                "attempt_disarmed",
                                work.diagnostic_message_id
                            );
                            return;
                        }
                        let now = std::time::Instant::now();
                        work.state.apply_settings(
                            settings.response_close_enabled,
                            settings.response_close_idle_seconds,
                            now,
                        );
                        work.state.publish_manager_facts(
                            candidate.member.clone(),
                            true,
                            crate::session::session::is_working(member),
                            now,
                        );
                        work.state.publish_manager_facts(
                            candidate.coordinator.clone(),
                            false,
                            crate::session::session::is_working(coordinator),
                            now,
                        );
                        let mut diagnostic_callback_reached = false;
                        let registered =
                            crate::config::sessions_persistence::register_published_carrier(
                                &permit,
                                &published,
                                current,
                                |_| {
                                    response_diag_debug_only(|| {
                                        diagnostic_callback_reached = true;
                                    });
                                    response_diag_local!(
                                        "complete_request",
                                        "register_published_carrier_callback_reached",
                                        work.diagnostic_message_id
                                    );
                                    response_diag_result_local!(
                                        work.state.register_request(
                                            crate::pty::manager::ResponseDeliveryProof {
                                                context: candidate.context.clone(),
                                                member: candidate.member.clone(),
                                                coordinator: candidate.coordinator.clone(),
                                                published_carrier_version: published.version,
                                            },
                                            now,
                                        ),
                                        "register_request",
                                        work.diagnostic_message_id
                                    )
                                    .ok()
                                },
                            );
                        response_diag_local!(
                            "complete_request",
                            if registered.is_some() {
                                "published_registration_some"
                            } else {
                                "published_registration_none"
                            },
                            work.diagnostic_message_id
                        );
                        response_diag_local!(
                            "complete_request",
                            if diagnostic_callback_reached {
                                "register_published_callback_reached"
                            } else {
                                "register_published_callback_not_reached"
                            },
                            work.diagnostic_message_id
                        );
                        if registered.is_some() {
                            let remembered =
                                crate::session::selection::remember_response_close_registration(
                                    crate::session::selection::ResponseCloseRegistration {
                                        diagnostic_last_poll_outcome: Arc::new(
                                            std::sync::atomic::AtomicU64::new(0),
                                        ),
                                        tracker: work.state.clone(),
                                        member: candidate.member.clone(),
                                        coordinator: candidate.coordinator.clone(),
                                        carrier: published.clone(),
                                    },
                                );
                            response_diag_local!(
                                "complete_request",
                                if remembered {
                                    "remember_true"
                                } else {
                                    "remember_false"
                                },
                                work.diagnostic_message_id
                            );
                            if !remembered {
                                work.state.fail_closed();
                            }
                            response_diag_local!(
                                "complete_request",
                                "disarm",
                                work.diagnostic_message_id
                            );
                            attempt
                                .armed
                                .store(false, std::sync::atomic::Ordering::Release);
                        }
                    },
                );
                response_diag_local!(
                    "complete_request",
                    if diagnostic_pair.is_some() {
                        "verified_pair_callback_reached"
                    } else {
                        "verified_pair_rejected"
                    },
                    work.diagnostic_message_id
                );
            })
            .await;
    }

    pub(crate) async fn complete_response_delivery<S>(&self, app: &S, path: &Path)
    where
        S: ResponseCompletionState + Clone + Send + Sync + 'static,
    {
        let origin = wake_claim_origin(path).unwrap_or_else(|| path.to_path_buf());
        let work = self
            .response_sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&origin);
        let Some(work) = work else {
            response_diag!("complete_delivery", "source_work_absent", None::<Uuid>);
            return;
        };
        let app = S::clone(app);
        crate::config::sessions_persistence::run_owned_persist_core(async move {
            Self::complete_response_request(&app, &work).await;
            let now = std::time::Instant::now();
            work.state.settle_terminal(&work.target, &work.key, now);
            response_diag!(
                "complete_delivery",
                "ordinary_terminal_settlement_invoked",
                work.diagnostic_message_id
            );
            let Some(context) = work.return_context else {
                response_diag!(
                    "complete_delivery",
                    "return_context_absent",
                    work.diagnostic_message_id
                );
                return;
            };
            let Some((member, coordinator)) = work.return_routes else {
                let _ = work.state.settle_return_terminal(&context, now);
                response_diag!(
                    "complete_delivery",
                    "return_routes_absent",
                    work.diagnostic_message_id
                );
                return;
            };
            let manager = app.pty_sessions().read().await.clone();
            let pty = app.pty_routes().clone();
            let settings = app.pty_settings();
            let settings = settings.read().await;
            work.state.apply_settings(
                settings.response_close_enabled,
                settings.response_close_idle_seconds,
                now,
            );
            let success = manager
                .with_response_close_facts(|current| {
                    let diagnostic_pair = with_verified_response_pair(
                        current,
                        &pty,
                        &work.state,
                        &member,
                        &coordinator,
                        |member_row, coordinator_row| {
                            let now = std::time::Instant::now();
                            work.state.publish_manager_facts(
                                member.clone(),
                                true,
                                crate::session::session::is_working(member_row),
                                now,
                            );
                            work.state.publish_manager_facts(
                                coordinator.clone(),
                                false,
                                crate::session::session::is_working(coordinator_row),
                                now,
                            );
                            response_diag_result!(
                                work.state.observe_return_success(
                                    crate::pty::manager::ReturnDeliveryProof {
                                        context: context.clone(),
                                    },
                                    now,
                                ),
                                "observe_return_success",
                                work.diagnostic_message_id
                            )
                            .is_ok()
                        },
                    );
                    response_diag!(
                        "complete_delivery",
                        if diagnostic_pair.is_some() {
                            "verified_pair_reached"
                        } else {
                            "verified_pair_rejected"
                        },
                        work.diagnostic_message_id
                    );
                    diagnostic_pair.unwrap_or(false)
                })
                .await;
            if !success {
                let _ = work.state.settle_return_terminal(&context, now);
            }
        })
        .await;
    }

    pub(crate) async fn admit_response_source<S: ResponseAdmissionState + ?Sized>(
        &self,
        app: &S,
        path: &Path,
        msg: &OutboxMessage,
        authenticated: bool,
        content: &str,
    ) -> Option<ResponseWakeWork> {
        response_diag_local!(
            "admit_source",
            if authenticated {
                "entry_authenticated_input_true"
            } else {
                "entry_authenticated_input_false"
            },
            Uuid::parse_str(&msg.id).ok()
        );
        let state = response_diag_option_local!(
            app.response_tracker(),
            "admit_source",
            "tracker_missing",
            Uuid::parse_str(&msg.id).ok()
        )?
        .clone();
        let digest = crate::phone::types::sha256_hex(content.as_bytes());
        if let Some(previous) = self
            .response_sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(path)
            .cloned()
        {
            if previous.digest != digest || previous.target != msg.to {
                state.fail_closed();
                response_diag_local!(
                    "admit_source",
                    "replay_identity_conflict",
                    Uuid::parse_str(&msg.id).ok()
                );
                return None;
            }
            response_diag_local!(
                "admit_source",
                "replay_reused",
                Uuid::parse_str(&msg.id).ok()
            );
            return Some(previous);
        }
        let key = format!("fs:{}:{}", path.to_string_lossy(), msg.id);
        let mut admission = match response_diag_result_local!(
            state.reserve_work(&msg.to, &key),
            "reserve_work",
            Uuid::parse_str(&msg.id).ok()
        ) {
            Ok(admission) => admission,
            Err(_) => {
                state.fail_closed();
                response_diag_local!(
                    "admit_source",
                    "reserve_work_error",
                    Uuid::parse_str(&msg.id).ok()
                );
                return None;
            }
        };
        if response_diag_result_local!(
            admission.accept_durable(&digest),
            "accept_durable",
            Uuid::parse_str(&msg.id).ok()
        )
        .is_err()
        {
            state.fail_closed();
            response_diag_local!(
                "admit_source",
                "accept_durable_error",
                Uuid::parse_str(&msg.id).ok()
            );
            return None;
        }
        drop(admission);
        response_diag_local!(
            "admit_source",
            "response_work_created",
            Uuid::parse_str(&msg.id).ok()
        );
        let mut work = ResponseWakeWork {
            diagnostic_message_id: if log::log_enabled!(
                target: "agentscommander_lib::response_close_diag",
                log::Level::Debug
            ) {
                Uuid::parse_str(&msg.id).ok()
            } else {
                None
            },
            state: state.clone(),
            target: msg.to.clone(),
            key,
            digest,
            attempt: Arc::default(),
            return_context: None,
            return_routes: None,
            delivery_context: None,
        };
        response_diag_local!(
            "admit_source",
            if authenticated {
                "authenticated_input_true"
            } else {
                "authenticated_input_false"
            },
            Uuid::parse_str(&msg.id).ok()
        );
        if authenticated {
            let settings = app.pty_settings();
            let settings = settings.read().await;
            state.apply_settings(
                settings.response_close_enabled,
                settings.response_close_idle_seconds,
                std::time::Instant::now(),
            );
            let coordinator =
                teams::verified_wg_coordinator_target(&msg.to, &settings.project_paths);
            drop(settings);
            let same_room = msg.from.rsplit_once('/').map(|(room, _)| room)
                == msg.to.rsplit_once('/').map(|(room, _)| room);
            response_diag_debug_only(|| {
                if coordinator.is_none() {
                    response_diag_local!(
                        "admit_source",
                        "configured_coordinator_absent",
                        Uuid::parse_str(&msg.id).ok()
                    );
                }
                if !same_room {
                    response_diag_local!(
                        "admit_source",
                        "cross_room",
                        Uuid::parse_str(&msg.id).ok()
                    );
                }
            });
            if coordinator.is_some() && same_room {
                let managers = app.pty_sessions();
                let manager = managers.read().await.clone();
                let token = msg
                    .token
                    .as_deref()
                    .and_then(|token| Uuid::parse_str(token).ok());
                let observed = manager
                    .with_response_close_facts(|snapshot| {
                        let route = |target: &str| {
                            let mut matches = snapshot.rows.iter().filter(|row| {
                                !snapshot.pending_ids.contains(&row.id)
                                    && sender_name_for_session_cwd_with_root_flag(
                                        &row.working_directory,
                                        row.is_root_agent,
                                    ) == target
                                    && !matches!(row.status, SessionStatus::Exited(_))
                            });
                            let row = response_diag_option_local!(
                                matches.next(),
                                "admit_source",
                                "route_missing",
                                Uuid::parse_str(&msg.id).ok()
                            )?;
                            if matches.next().is_some() {
                                response_diag_local!(
                                    "admit_source",
                                    "route_ambiguous",
                                    Uuid::parse_str(&msg.id).ok()
                                );
                                return None;
                            }
                            Some((
                                crate::pty::manager::ResponseRouteIdentity {
                                    target: target.to_string(),
                                    session_id: row.id,
                                    token: row.token,
                                    generation: response_diag_option_local!(
                                        state.generation(row.id),
                                        "admit_source",
                                        "generation_unavailable",
                                        Uuid::parse_str(&msg.id).ok()
                                    )?,
                                    carrier_version: *response_diag_option_local!(
                                        snapshot.versions.get(&row.id),
                                        "admit_source",
                                        "version_unavailable",
                                        Uuid::parse_str(&msg.id).ok()
                                    )?,
                                    backend: row.backend_kind,
                                },
                                row,
                            ))
                        };
                        let (member, member_row) = route(&msg.from)?;
                        let (coordinator, coordinator_row) = route(&msg.to)?;
                        if member.backend != SessionBackendKind::LocalProcess
                            || member_row.is_coordinator
                            || member_row.is_root_agent
                            || member_row.agent_kind.is_none()
                            || token != Some(member.token)
                            || !coordinator_row.is_coordinator
                            || coordinator_row.is_root_agent
                        {
                            response_diag_local!(
                                "admit_source",
                                "role_or_token_or_backend",
                                Uuid::parse_str(&msg.id).ok()
                            );
                            return None;
                        }
                        let pty = app.pty_routes().clone();
                        let diagnostic_pair = with_verified_response_pair(
                            snapshot,
                            &pty,
                            &state,
                            &member,
                            &coordinator,
                            |member_row, coordinator_row| {
                                let now = std::time::Instant::now();
                                state.publish_manager_facts(
                                    member.clone(),
                                    true,
                                    crate::session::session::is_working(member_row),
                                    now,
                                );
                                state.publish_manager_facts(
                                    coordinator.clone(),
                                    false,
                                    crate::session::session::is_working(coordinator_row),
                                    now,
                                );
                                response_diag_result_local!(
                                    state.admit_return(
                                        &member,
                                        &coordinator,
                                        work.key.clone(),
                                        work.digest.clone(),
                                        now,
                                    ),
                                    "admit_return",
                                    Uuid::parse_str(&msg.id).ok()
                                )
                                .ok()
                            },
                        );
                        response_diag_local!(
                            "admit_source",
                            if diagnostic_pair.is_some() {
                                "verified_pair_callback_reached"
                            } else {
                                "verified_pair_rejected"
                            },
                            Uuid::parse_str(&msg.id).ok()
                        );
                        diagnostic_pair
                            .flatten()
                            .map(|context| (context, member, coordinator))
                    })
                    .await;
                if let Some((context, member, coordinator)) = observed {
                    work.return_context = Some(context);
                    work.return_routes = Some((member, coordinator));
                }
            }
        }
        response_diag_local!(
            "admit_source",
            if work.return_context.is_some() {
                "return_context_present"
            } else {
                "return_context_absent"
            },
            work.diagnostic_message_id
        );
        self.response_sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(path.to_path_buf(), work.clone());
        Some(work)
    }

    pub fn new(messages: Arc<crate::config::injected_messages::MessageWarnings>) -> Self {
        let (wake_outcomes_tx, wake_outcomes_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            messages,
            host_pty_slots: Arc::new(tokio::sync::Semaphore::new(HOST_PTY_CORE_LIMIT)),
            poll_interval: std::time::Duration::from_secs(3),
            retry_tracker: HashMap::new(),
            response_sources: Arc::new(Mutex::new(HashMap::new())),
            snapshot_scanner: crate::phone::terminal_snapshot::SnapshotMailboxScanner::default(),
            live_claims: std::collections::HashSet::new(),
            wake_lanes: Arc::new(crate::phone::wake_lanes::WakeLanes::default()),
            wake_workers: tokio::task::JoinSet::new(),
            wake_outcomes_tx,
            wake_outcomes_rx,
            #[cfg(test)]
            test_hooks: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_test_hooks(test_hooks: MailboxTestHooks) -> Self {
        let messages = Arc::new(crate::config::injected_messages::MessageWarnings::new());
        let (wake_outcomes_tx, wake_outcomes_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            messages,
            host_pty_slots: Arc::new(tokio::sync::Semaphore::new(HOST_PTY_CORE_LIMIT)),
            poll_interval: std::time::Duration::from_secs(3),
            retry_tracker: HashMap::new(),
            response_sources: Arc::new(Mutex::new(HashMap::new())),
            snapshot_scanner: crate::phone::terminal_snapshot::SnapshotMailboxScanner::default(),
            live_claims: std::collections::HashSet::new(),
            wake_lanes: Arc::new(crate::phone::wake_lanes::WakeLanes::default()),
            wake_workers: tokio::task::JoinSet::new(),
            wake_outcomes_tx,
            wake_outcomes_rx,
            test_hooks: Some(test_hooks),
        }
    }

    /// (#1399 G2) A poller for the delivery tail only. Field-by-field and
    /// NEVER via `Self::new()` or struct-update over it: `new()` constructs the
    /// registering `SnapshotMailboxScanner::default()`, which writes itself
    /// into the process-global shutdown-owner slot and would leave the
    /// long-lived poller's scanner tasks unsealed and undrained at shutdown
    /// (#1403 tracks the pre-existing dispatcher case).
    pub(crate) fn delivery_only(
        messages: Arc<crate::config::injected_messages::MessageWarnings>,
    ) -> Self {
        let (wake_outcomes_tx, wake_outcomes_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            messages,
            host_pty_slots: Arc::new(tokio::sync::Semaphore::new(HOST_PTY_CORE_LIMIT)),
            poll_interval: std::time::Duration::from_secs(3),
            retry_tracker: HashMap::new(),
            response_sources: Arc::new(Mutex::new(HashMap::new())),
            snapshot_scanner:
                crate::phone::terminal_snapshot::SnapshotMailboxScanner::delivery_only(),
            live_claims: std::collections::HashSet::new(),
            wake_lanes: Arc::new(crate::phone::wake_lanes::WakeLanes::default()),
            wake_workers: tokio::task::JoinSet::new(),
            wake_outcomes_tx,
            wake_outcomes_rx,
            #[cfg(test)]
            test_hooks: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn delivery_only_for_tests() -> Self {
        Self::delivery_only(Arc::new(
            crate::config::injected_messages::MessageWarnings::new(),
        ))
    }
}

#[cfg(test)]
mod tests;
