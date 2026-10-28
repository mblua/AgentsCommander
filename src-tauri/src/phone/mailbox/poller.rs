use super::handoff::{release_wake_claim, ERR_OWNED_WAKE_RETAINED};
use super::notice::{
    canonical_project_refresh_path, project_refresh_priority, ProjectRefreshEventPayload,
    ProjectRefreshPollBatch, ResolvedWakeAgentCommand, RetryState, WakeDeliveryOrigin,
};
#[cfg(test)]
use super::pty_claim::MailboxTestHooks;
use super::pty_claim::{
    current_response_row, ResponseCandidateAttempt, ResponseRequestCandidate, ResponseWakeWork,
};
use crate::config::settings::SettingsState;
use crate::phone::types::OutboxMessage;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::session::Session;
use crate::session::session::SessionCommunicationKind;
use crate::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Manager;
use uuid::Uuid;

pub(crate) fn collect_project_refresh_requests(requests_dir: &Path) -> ProjectRefreshPollBatch {
    if !requests_dir.is_dir() {
        return ProjectRefreshPollBatch::default();
    }

    let mut entries: Vec<PathBuf> = match std::fs::read_dir(requests_dir) {
        Ok(rd) => rd
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect(),
        Err(e) => {
            log::warn!(
                "[project-refresh-requests] Failed to read {:?}: {}",
                requests_dir,
                e
            );
            return ProjectRefreshPollBatch::default();
        }
    };
    entries.sort();

    let mut batch = ProjectRefreshPollBatch::default();
    let mut selected_by_project: HashMap<String, (u8, ProjectRefreshEventPayload)> = HashMap::new();
    let mut project_order: Vec<String> = Vec::new();

    for path in entries {
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let content = match read_text_bom_tolerant(&path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!(
                    "[project-refresh-requests] Failed to read {:?}: {}",
                    path,
                    e
                );
                continue;
            }
        };

        let mut request: crate::cli::create_agent_matrix::ProjectRefreshRequest =
            match serde_json::from_str(&content) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!(
                        "[project-refresh-requests] Failed to parse {:?}: {}",
                        path,
                        e
                    );
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
            };

        let canonical_project_path = canonical_project_refresh_path(&request.project_path);
        request.project_path = canonical_project_path.clone();
        batch.processed_paths.push(path);

        let priority = project_refresh_priority(&request.reason);
        let payload = ProjectRefreshEventPayload {
            id: request.id,
            project_path: request.project_path,
            changed_path: request.changed_path,
            changed_name: request.changed_name,
            reason: request.reason,
        };

        match selected_by_project.get(&canonical_project_path) {
            Some((existing_priority, _)) if *existing_priority <= priority => {}
            Some(_) => {
                selected_by_project.insert(canonical_project_path, (priority, payload));
            }
            None => {
                project_order.push(canonical_project_path.clone());
                selected_by_project.insert(canonical_project_path, (priority, payload));
            }
        }
    }

    for project_path in project_order {
        if let Some((_, payload)) = selected_by_project.remove(&project_path) {
            batch.payloads.push(payload);
        }
    }

    batch
}

/// (#1399) The detached delivery tail: deliver, archive on success, and on
/// ordinary failure rename the claim back before the outcome is sent. Retained
/// uncertainty keeps its non-actionable claim for terminal settlement. Ordinary
/// failures restore the origin before the scanner drains their retry/reject report.
pub(crate) async fn deliver_claimed_wake<R: tauri::Runtime>(
    delivery: &MailboxPoller,
    app: &tauri::AppHandle<R>,
    claim: &Path,
    origin: &Path,
    validated: &ValidatedWake,
) -> Result<(), String> {
    let _candidate_disposal = validated
        .response
        .as_ref()
        .map(ResponseWakeWork::candidate_disposal);
    match delivery.deliver_validated_wake(app, validated, false).await {
        // `claim.parent()` is the outbox dir, so delivered/ is derived exactly
        // as it is today and the destination still comes from `msg.id`. The
        // delivered JSON carries the receipt annotation (#1635-2 D4).
        Ok(annotated) => match delivery.move_to_delivered(app, claim, &annotated).await {
            Ok(()) => Ok(()),
            Err(error) => {
                release_wake_claim(claim, origin);
                Err(error)
            }
        },
        Err(error) => {
            if !error.starts_with(ERR_OWNED_WAKE_RETAINED) {
                release_wake_claim(claim, origin);
            }
            Err(error)
        }
    }
}

impl MailboxPoller {
    pub(crate) fn active_terminal_snapshot_shutdown_owner(
    ) -> Option<crate::phone::terminal_snapshot::SnapshotScannerShutdownOwner> {
        crate::phone::terminal_snapshot::SnapshotScannerShutdownOwner::active()
    }
}

pub(crate) struct PreparedResponseRequest {
    pub(crate) member: crate::pty::manager::ResponseRouteIdentity,
    pub(crate) coordinator: crate::pty::manager::ResponseRouteIdentity,
    pub(crate) roots: Vec<String>,
}

pub(crate) fn reserve_response_request_at_write<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    current: &crate::session::manager::CarrierSnapshot,
    session: &Session,
    prepared: &PreparedResponseRequest,
    work: &ResponseWakeWork,
    attempt: &Arc<ResponseCandidateAttempt>,
) {
    if !attempt.armed.load(std::sync::atomic::Ordering::Acquire) {
        response_diag_local!(
            "prewrite_reserve",
            "attempt_disarmed",
            work.diagnostic_message_id
        );
        return;
    }
    if app
        .try_state::<crate::session::selection::SelectionCoordinator>()
        .is_some_and(|coordinator| coordinator.restore_is_active())
    {
        response_diag_local!(
            "prewrite_reserve",
            "restore_active",
            work.diagnostic_message_id
        );
        return;
    }
    let settings = app.state::<SettingsState>().inner().clone();
    let Ok(settings) = settings.try_read_owned() else {
        response_diag_local!(
            "prewrite_reserve",
            "settings_busy",
            work.diagnostic_message_id
        );
        return;
    };
    if settings.project_paths != prepared.roots {
        response_diag_local!(
            "prewrite_reserve",
            "roots_changed",
            work.diagnostic_message_id
        );
        return;
    }
    let Some(member) = current_response_row(current, &prepared.member) else {
        response_diag_local!(
            "prewrite_reserve",
            "current_member_missing",
            work.diagnostic_message_id
        );
        return;
    };
    let Some(coordinator) = current_response_row(current, &prepared.coordinator) else {
        response_diag_local!(
            "prewrite_reserve",
            "current_coordinator_missing",
            work.diagnostic_message_id
        );
        return;
    };
    if session.id != member.id
        || member.backend_kind != SessionBackendKind::LocalProcess
        || member.is_coordinator
        || member.is_root_agent
        || member.agent_kind.is_none()
        || member
            .communication
            .as_ref()
            .is_some_and(|communication| communication.kind == SessionCommunicationKind::CoManaged)
        || !coordinator.is_coordinator
        || coordinator.is_root_agent
    {
        response_diag_local!(
            "prewrite_reserve",
            "role_or_backend_or_communication",
            work.diagnostic_message_id
        );
        return;
    }
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let Ok(member_route) = PtyManager::snapshot_route_proof(&pty, member.id) else {
        response_diag_local!(
            "prewrite_reserve",
            "member_route_missing",
            work.diagnostic_message_id
        );
        return;
    };
    let Some(replica) = member_route.saved_replica() else {
        response_diag_local!(
            "prewrite_reserve",
            "member_replica_missing",
            work.diagnostic_message_id
        );
        return;
    };
    if member_route.backend_kind() != SessionBackendKind::LocalProcess
        || !crate::path_identity::verify_directory(std::path::Path::new(&member.working_directory))
            .is_ok_and(|actual| {
                crate::path_identity::same_object(&actual, member_route.saved_cwd())
            })
        || !member_route.matches_requester_route(
            member.backend_kind,
            member_route.saved_cwd(),
            Some(replica),
        )
    {
        response_diag_local!(
            "prewrite_reserve",
            "member_route_mismatch",
            work.diagnostic_message_id
        );
        return;
    }
    let Ok(coordinator_route) = PtyManager::snapshot_route_proof(&pty, coordinator.id) else {
        response_diag_local!(
            "prewrite_reserve",
            "coordinator_route_missing",
            work.diagnostic_message_id
        );
        return;
    };
    let Ok(authority) = PtyManager::authority_route_proof(&pty, coordinator.id) else {
        response_diag_local!(
            "prewrite_reserve",
            "coordinator_authority_missing",
            work.diagnostic_message_id
        );
        return;
    };
    let Ok(_coordinator_guard) = authority.lock_verified(
        coordinator.backend_kind,
        coordinator_route.saved_cwd(),
        coordinator_route.saved_replica(),
    ) else {
        response_diag_local!(
            "prewrite_reserve",
            "authority_lock_rejected",
            work.diagnostic_message_id
        );
        return;
    };
    if coordinator_route.backend_kind() != coordinator.backend_kind
        || !crate::path_identity::verify_directory(std::path::Path::new(
            &coordinator.working_directory,
        ))
        .is_ok_and(|actual| {
            crate::path_identity::same_object(&actual, coordinator_route.saved_cwd())
        })
        || work.state.generation(member.id) != Some(prepared.member.generation)
        || work.state.generation(coordinator.id) != Some(prepared.coordinator.generation)
    {
        response_diag_local!(
            "prewrite_reserve",
            "coordinator_route_or_generation_mismatch",
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
        prepared.member.clone(),
        true,
        crate::session::session::is_working(member),
        now,
    );
    work.state.publish_manager_facts(
        prepared.coordinator.clone(),
        false,
        crate::session::session::is_working(coordinator),
        now,
    );
    let Ok(mut candidate) = attempt.candidate.lock() else {
        work.state.fail_closed();
        response_diag_local!(
            "prewrite_reserve",
            "candidate_lock_poisoned",
            work.diagnostic_message_id
        );
        return;
    };
    if !attempt.armed.load(std::sync::atomic::Ordering::Acquire) {
        response_diag_local!(
            "prewrite_reserve",
            "attempt_disarmed_after_lock",
            work.diagnostic_message_id
        );
        return;
    }
    let Ok(context) = response_diag_result_local!(
        work.state.reserve_policy_candidate(
            prepared.member.clone(),
            prepared.coordinator.clone(),
            work.key.clone(),
            work.digest.clone(),
            now,
        ),
        "reserve_policy",
        work.diagnostic_message_id
    ) else {
        response_diag_local!(
            "prewrite_reserve",
            "reserve_policy_error",
            work.diagnostic_message_id
        );
        return;
    };

    *candidate = Some(ResponseRequestCandidate {
        context,
        member: prepared.member.clone(),
        coordinator: prepared.coordinator.clone(),
        initial: crate::config::sessions_persistence::InitialCarrierIntent::new(
            prepared.member.target.clone(),
            member.id,
            member.token,
            prepared.member.carrier_version,
            replica.canonical_path.clone(),
        ),
    });
    response_diag_local!(
        "prewrite_reserve",
        "candidate_created",
        work.diagnostic_message_id
    );
}

pub(crate) struct ValidatedWake {
    pub(crate) message: OutboxMessage,
    pub(crate) response: Option<ResponseWakeWork>,
}

// Local stage payloads carry the existing facts and fallback ownership only.
// A handled rejection remains terminal even when its archival result is Ok.
pub(crate) type OutboxStage<T = ()> = Result<std::ops::ControlFlow<Result<(), String>, T>, String>;

pub(crate) type OutboxAuthenticationFacts = (bool, bool, bool, bool, bool);

pub(crate) type PeerWakeFallback = (
    bool,
    Option<Uuid>,
    Option<String>,
    Option<crate::session::session::SessionCommunication>,
);

pub(crate) type SpawnedPeerWake = (
    Uuid,
    Option<crate::config::agent_command::AgentSpawnCommand>,
    ResolvedWakeAgentCommand,
);

#[expect(
    clippy::large_enum_variant,
    reason = "Preserve the owned delivered message and its existing receipt ownership"
)]
pub(crate) enum OwnedPeerWakeStep {
    Delivered(OutboxMessage),
    Relist,
    Retained(&'static str),
}

pub(crate) struct OwnedPeerWakeExecution {
    pub(crate) message: OutboxMessage,
    pub(crate) delivery: (WakeDeliveryOrigin, bool),
    pub(crate) response: Option<ResponseWakeWork>,
    pub(crate) flags_present: bool,
    pub(crate) request: crate::commands::session::OwnedReplacementCreateRequest,
    pub(crate) resolved_command: ResolvedWakeAgentCommand,
    pub(crate) continuation: crate::session::selection::OwnedReplacementContinuation,
    pub(crate) gate: Arc<crate::api::message_store::PtyInputTargetGate>,
    pub(crate) exact: Option<crate::api::message_store::PtyInputTargetGuard>,
    pub(crate) stripe: Option<crate::api::message_store::PtyInputStripeGuard>,
}

pub(crate) struct OwnedPeerTarget {
    pub(crate) continuation: crate::session::selection::OwnedReplacementContinuation,
    pub(crate) gate: Arc<crate::api::message_store::PtyInputTargetGate>,
    pub(crate) exact: crate::api::message_store::PtyInputTargetGuard,
    pub(crate) stripe: crate::api::message_store::PtyInputStripeGuard,
    pub(crate) physical: crate::path_identity::VerifiedPathIdentity,
}

pub(crate) fn owned_replacement_launch_recipe(
    request: &crate::commands::session::OwnedReplacementCreateRequest,
) -> serde_json::Value {
    let configured = request.resolved_spawn.as_ref().map(|spawn| {
        let seed = spawn.seed.as_ref().map(|seed| {
            serde_json::json!({
                "dest": seed.dest,
                "candidates": seed.candidates.iter().map(|candidate| serde_json::json!({
                    "tier": format!("{:?}", candidate.tier),
                    "os": candidate.os_specific, "path": candidate.path,
                })).collect::<Vec<_>>(),
                "replicaRoot": seed.context.replica_root,
                "rootKind": format!("{:?}", seed.context.root_kind),
                "acRoot": seed.context.ac_root, "matrixRoot": seed.context.matrix_root,
                "warning": seed.config_dir_warning,
            })
        });
        serde_json::json!({
            "shell": spawn.shell, "args": spawn.shell_args,
            "agentEnv": spawn.agent_env, "profileEnv": spawn.profile_env,
            "generatedEnv": spawn.generated_env, "childEnv": spawn.child_env,
            "removeEnv": spawn.env_remove_keys, "codexHome": spawn.effective_codex_home,
            "requestedProfile": spawn.profile_resolution.requested_profile,
            "effectiveProfile": spawn.profile_resolution.effective_profile,
            "fallbackChain": spawn.profile_resolution.fallback_chain,
            "fallbackApplied": spawn.profile_resolution.fallback_applied,
            "profileHash": spawn.profile_content_hash,
            "trustedId": spawn.trusted_agent_id, "trustedLabel": spawn.trusted_agent_label,
            "backend": format!("{:?}", SessionBackendKind::from(&spawn.backend)),
            "matchTier": spawn.match_tier, "originalProfile": spawn.original_profile_letter,
            "seed": seed,
        })
    });
    serde_json::json!({
        "shell": request.shell, "args": request.shell_args, "cwd": request.cwd,
        "name": request.name, "agentId": request.agent_id, "agentLabel": request.agent_label,
        "skipToolingSave": request.skip_tooling_save, "repos": request.git_repos,
        "hostShell": request.host_shell.as_ref().map(|host| (&host.program, &host.args)),
        "configured": configured,
    })
}

pub struct MailboxPoller {
    pub(crate) messages: Arc<crate::config::injected_messages::MessageWarnings>,
    pub(crate) poll_interval: std::time::Duration,
    pub(crate) retry_tracker: HashMap<PathBuf, RetryState>,
    pub(crate) response_sources: Arc<Mutex<HashMap<PathBuf, ResponseWakeWork>>>,
    pub(crate) snapshot_scanner: crate::phone::terminal_snapshot::SnapshotMailboxScanner,
    /// (#1399) Claim paths this process has handed off whose outcome has not yet
    /// been successfully settled by `drain_wake_outcomes`. Membership includes
    /// running workers, queued reports, and reports requeued after filesystem
    /// observation or mutation failed. Anything else on disk is unowned.
    pub(crate) live_claims: std::collections::HashSet<PathBuf>,
    /// (#1399) Per-target delivery lanes shared with every spawned worker.
    pub(crate) wake_lanes: Arc<crate::phone::wake_lanes::WakeLanes>,
    pub(crate) wake_workers: tokio::task::JoinSet<()>,
    /// (#1399) Unbounded is safe by construction: admission caps both running
    /// workers and owned claims at `WAKE_WORKER_LIMIT`. Each owned claim has at
    /// most one report, and a failed settlement requeues that same report, so
    /// at most 8 reports are pending while worker sends remain non-blocking.
    pub(crate) wake_outcomes_tx: tokio::sync::mpsc::UnboundedSender<(PathBuf, Result<(), String>)>,
    pub(crate) wake_outcomes_rx:
        tokio::sync::mpsc::UnboundedReceiver<(PathBuf, Result<(), String>)>,
    #[cfg(test)]
    pub(crate) test_hooks: Option<MailboxTestHooks>,
}

/// (#885) One peer's correlated gate input. Built from the three snapshots
/// (mirror `Vec<SessionInfo>`, `pty_live: HashSet<Uuid>`,
/// `Vec<PurgeReadiness>`) before `evaluate_gate` is called.
#[derive(Debug, Clone)]
pub(crate) struct PurgeGatePeer {
    pub fqn: String,
    pub all_session_ids: Vec<String>,
    pub live: Vec<PurgeGateSession>,
}

#[derive(Debug, Clone)]
pub(crate) struct PurgeGateSession {
    pub session_id: Uuid,
    pub readiness: crate::pty::idle_detector::PurgeReadiness,
    pub mirror_idle: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct PurgeDecision {
    pub passed: bool,
    pub peers: Vec<PurgePeerResult>,
}

#[derive(Debug, Clone)]
pub(crate) struct PurgePeerResult {
    pub fqn: String,
    pub purgeable: bool,
    pub outcome: &'static str,
    pub idle_ms: Option<u128>,
    pub silence_ms: Option<u128>,
    pub watcher_idle: bool,
    pub mirror_idle: bool,
    pub resize_settled: bool,
    pub session_ids: Vec<String>,
}

/// (#885) Pure gate evaluator. Called by both the dry-run and the real path,
/// so a dry-run cannot lie (§3.3). A peer is purgeable iff it has no live
/// sessions (vacuously purgeable) or ALL of its live sessions pass the
/// four-leg test:
///   1. `activity_age >= effective_quiet` (printable silence)
///   2. `watcher_idle` (watcher agreement)
///   3. `mirror_idle` (SessionManager agreement)
///   4. `resize_settled` (F-1: activity readings are trustworthy)
pub(crate) fn evaluate_gate(peers: &[PurgeGatePeer], quiet: std::time::Duration) -> PurgeDecision {
    let mut passed = true;
    let mut results = Vec::with_capacity(peers.len());

    for peer in peers {
        if peer.live.is_empty() {
            results.push(PurgePeerResult {
                fqn: peer.fqn.clone(),
                purgeable: true,
                outcome: "skipped",
                idle_ms: None,
                silence_ms: None,
                watcher_idle: false,
                mirror_idle: true,
                resize_settled: true,
                session_ids: peer.all_session_ids.clone(),
            });
            continue;
        }

        let mut peer_purgeable = true;
        let mut saw_untracked_failure = false;
        let mut saw_busy_failure = false;
        let mut saw_resize_only_failure = false;
        let mut idle_ms: Option<u128> = None;
        let mut silence_ms: Option<u128> = None;
        let mut watcher_idle = true;
        let mut mirror_idle = true;
        let mut resize_settled = true;

        for session in &peer.live {
            let r = &session.readiness;
            let effective_quiet = quiet.max(r.idle_threshold);
            let m_idle = session.mirror_idle;
            let r_settled = match r.last_resize_age {
                None => true,
                Some(a) => a >= r.resize_grace + effective_quiet,
            };
            let activity_ok = matches!(r.activity_age, Some(a) if a >= effective_quiet);
            let purgeable = activity_ok && r.watcher_idle && m_idle && r_settled;

            if !purgeable {
                peer_purgeable = false;
                if r.activity_age.is_none() {
                    saw_untracked_failure = true;
                } else if !r_settled && activity_ok && r.watcher_idle && m_idle {
                    saw_resize_only_failure = true;
                } else {
                    saw_busy_failure = true;
                }
            }

            if let Some(a) = r.activity_age {
                idle_ms = Some(idle_ms.map_or(a.as_millis(), |m: u128| m.min(a.as_millis())));
            }
            if let Some(s) = r.silence_age {
                silence_ms = Some(silence_ms.map_or(s.as_millis(), |m: u128| m.min(s.as_millis())));
            }
            if !r.watcher_idle {
                watcher_idle = false;
            }
            if !m_idle {
                mirror_idle = false;
            }
            if !r_settled {
                resize_settled = false;
            }
        }

        if !peer_purgeable {
            passed = false;
        }

        // (#885 E-5) Distinguish resize-unsettled from busy, but aggregate
        // across all live sessions for the peer. `resize_unsettled` is only
        // honest when resize is the sole failing leg for every failing live
        // session. Unknown liveness is more important than either diagnostic.
        let peer_outcome = if peer_purgeable {
            "skipped"
        } else if saw_untracked_failure {
            "untracked"
        } else if saw_busy_failure {
            "busy"
        } else if saw_resize_only_failure {
            "resize_unsettled"
        } else {
            "busy"
        };

        results.push(PurgePeerResult {
            fqn: peer.fqn.clone(),
            purgeable: peer_purgeable,
            outcome: peer_outcome,
            idle_ms,
            silence_ms,
            watcher_idle,
            mirror_idle,
            resize_settled,
            session_ids: peer.all_session_ids.clone(),
        });
    }

    PurgeDecision {
        passed,
        peers: results,
    }
}

#[cfg(test)]
impl Default for MailboxPoller {
    fn default() -> Self {
        Self::new(Arc::new(
            crate::config::injected_messages::MessageWarnings::new(),
        ))
    }
}

/// Read a file as UTF-8 string, tolerant of UTF-8 / UTF-16 LE / UTF-16 BE BOMs.
/// Logs a warning when a BOM is detected so users see which tool is writing
/// odd encoding into outbox / session-requests (typically PowerShell on Windows).
pub(crate) fn read_text_bom_tolerant(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read file: {}", e))?;

    if bytes.starts_with(&[0xFF, 0xFE]) {
        log::warn!(
            "[bom] UTF-16 LE BOM detected in {:?} — decoding to UTF-8 (writer should use UTF-8 \
            without BOM)",
            path
        );
        let u16_data: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&u16_data))
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        log::warn!(
            "[bom] UTF-16 BE BOM detected in {:?} — decoding to UTF-8 (writer should use UTF-8 \
            without BOM)",
            path
        );
        let u16_data: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&u16_data))
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        log::warn!(
            "[bom] UTF-8 BOM detected in {:?} — stripping (writer should use UTF-8 without BOM)",
            path
        );
        String::from_utf8(bytes[3..].to_vec())
            .map_err(|e| format!("Invalid UTF-8 after BOM: {}", e))
    } else {
        String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8: {}", e))
    }
}

/// #2786 (C1) - the agent state file beside `config.json` in `target_root`,
/// read through the same guard as the PTY-input `config.json` read: a regular
/// non-symlink file, at most 1 MiB, no duplicate keys. Only a missing file is
/// absence; every other failure is an error the caller propagates.
pub(crate) fn read_guarded_agent_state(
    target_root: &Path,
) -> Result<Option<serde_json::Value>, crate::phone::types::PtyInputReasonCode> {
    use crate::phone::types::PtyInputReasonCode as C;
    let state_path = target_root.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME);
    if matches!(
        std::fs::symlink_metadata(&state_path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    ) {
        return Ok(None);
    }
    let (state_bytes, _) = crate::path_identity::read_bounded_regular(&state_path, 1024 * 1024)
        .map_err(|_| C::UnsafePath)?;
    crate::path_identity::parse_json_no_duplicates(&state_bytes)
        .map(Some)
        .map_err(|_| C::UnsupportedProfile)
}
