use crate::filesystem::phone_mailbox as mailbox_disk;
use crate::phone::mailbox::handoff::PURGE_WG_ACTION;
use crate::phone::mailbox::poller::{PurgeDecision, PurgeGatePeer, PurgeGateSession};
use crate::phone::mailbox::settle::filter_sessions_by_fqn;
use crate::phone::mailbox::teardown;
use crate::phone::types::OutboxMessage;
use crate::session::session::{SessionInfo, SessionStatus};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

pub(crate) fn peer_fqns(wg_dir: &Path, agent_name: &str, project: &str, room: &str) -> Vec<String> {
    let mut peer_fqns: Vec<String> = Vec::new();
    if let Ok(entries) = mailbox_disk::mailbox_directory_paths(wg_dir) {
        for path in entries.flatten() {
            let name = match path.file_name().and_then(|name| name.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let agent = match name.strip_prefix("__agent_") {
                Some(a) => a.to_string(),
                None => continue,
            };
            if agent == agent_name {
                continue;
            }
            if !mailbox_disk::mailbox_path_is_dir(&path) {
                continue;
            }
            peer_fqns.push(format!("{}:{}/{}", project, room, agent));
        }
    }
    peer_fqns.sort();

    peer_fqns
}

pub(crate) struct PeerLiveInfo {
    fqn: String,
    all_session_ids: Vec<String>,
    live_sessions: Vec<(Uuid, bool)>, // (session_id, mirror_idle)
}

pub(crate) fn peer_live_info(
    peer_fqns: &[String],
    sessions: &[SessionInfo],
    pty_live: &HashSet<Uuid>,
    project: &str,
    room: &str,
    msg: &OutboxMessage,
) -> Result<(Vec<PeerLiveInfo>, Vec<Uuid>), serde_json::Value> {
    let mut peer_infos: Vec<PeerLiveInfo> = Vec::with_capacity(peer_fqns.len());
    let mut all_live_session_ids: Vec<Uuid> = Vec::new();

    for fqn in peer_fqns {
        let matched: Vec<&SessionInfo> = filter_sessions_by_fqn(sessions, fqn);
        let all_session_ids: Vec<String> = matched.iter().map(|s| s.id.clone()).collect();

        let mut live_sessions: Vec<(Uuid, bool)> = Vec::new();
        for s in &matched {
            // Guard B (root): corrupted-state assertion (F-12).
            if s.is_root_agent
                || crate::config::root_agent::root_agent_path_requires_protection(
                    &s.working_directory,
                )
            {
                let response = serde_json::json!({
                    "action": PURGE_WG_ACTION,
                    "workgroup": format!("{}:{}", project, room),
                    "status": "failed_root_guard",
                    "requested_by": msg.from,
                    "offending_session_id": s.id,
                    "offending_working_directory": s.working_directory,
                    "peers": [],
                });
                return Err(response);
            }

            let sid = match Uuid::parse_str(&s.id) {
                Ok(id) => id,
                Err(_) => continue,
            };
            let is_live = !matches!(s.status, SessionStatus::Exited(_)) && pty_live.contains(&sid);
            if is_live {
                let mirror_idle =
                    !matches!(s.status, SessionStatus::Active | SessionStatus::Running)
                        || s.waiting_for_input;
                live_sessions.push((sid, mirror_idle));
                all_live_session_ids.push(sid);
            }
        }

        peer_infos.push(PeerLiveInfo {
            fqn: fqn.clone(),
            all_session_ids,
            live_sessions,
        });
    }

    Ok((peer_infos, all_live_session_ids))
}

pub(crate) fn gate_peers(
    peer_infos: &[PeerLiveInfo],
    readiness_map: &HashMap<Uuid, crate::pty::idle_detector::PurgeReadiness>,
) -> Vec<PurgeGatePeer> {
    peer_infos
        .iter()
        .map(|info| PurgeGatePeer {
            fqn: info.fqn.clone(),
            all_session_ids: info.all_session_ids.clone(),
            live: info
                .live_sessions
                .iter()
                .map(|(sid, mirror_idle)| PurgeGateSession {
                    session_id: *sid,
                    readiness: readiness_map.get(sid).copied().unwrap_or(
                        crate::pty::idle_detector::PurgeReadiness {
                            session_id: *sid,
                            activity_age: None,
                            watcher_idle: false,
                            last_resize_age: None,
                            resize_grace: crate::session::profile::IdleTuning::DEFAULT.resize_grace,
                            idle_threshold: crate::session::profile::IdleTuning::DEFAULT
                                .idle_threshold,
                            silence_age: None,
                        },
                    ),
                    mirror_idle: *mirror_idle,
                })
                .collect(),
        })
        .collect()
}

pub(crate) fn dry_run_response(
    project: &str,
    room: &str,
    msg: &OutboxMessage,
    quiet: Duration,
    decision: &PurgeDecision,
) -> serde_json::Value {
    let status = if decision.passed {
        "dry_run_ready"
    } else {
        "dry_run_blocked"
    };
    // (N-3) The dry-run path reports `purgeable` and the diagnostic
    // fields, NOT `outcome` (which is the gate's internal "busy"/
    // "skipped"/"untracked" vocabulary and contradicts `would_purge:
    // true` when every peer is "skipped" because it has no live
    // session). An operator reads `purgeable: true`, not "skipped".
    serde_json::json!({
        "action": PURGE_WG_ACTION,
        "workgroup": format!("{}:{}", project, room),
        "status": status,
        "requested_by": msg.from,
        "quiet_period_ms": quiet.as_millis() as u64,
        "dry_run": true,
        "would_purge": decision.passed,
        "peers": decision.peers.iter().map(|p| serde_json::json!({
            "name": p.fqn,
            "purgeable": p.purgeable,
            "idle_ms": p.idle_ms,
            "silence_ms": p.silence_ms,
            "watcher_idle": p.watcher_idle,
            "mirror_idle": p.mirror_idle,
            "resize_settled": p.resize_settled,
            "session_ids": p.session_ids,
        })).collect::<Vec<_>>(),
    })
}

pub(crate) fn busy_response(
    project: &str,
    room: &str,
    msg: &OutboxMessage,
    quiet: Duration,
    decision: &PurgeDecision,
) -> serde_json::Value {
    serde_json::json!({
        "action": PURGE_WG_ACTION,
        "workgroup": format!("{}:{}", project, room),
        "status": "rejected_busy",
        "requested_by": msg.from,
        "quiet_period_ms": quiet.as_millis() as u64,
        "peers": decision.peers.iter().map(|p| serde_json::json!({
            "name": p.fqn,
            "outcome": p.outcome,
            "purgeable": p.purgeable,
            "idle_ms": p.idle_ms,
            "silence_ms": p.silence_ms,
            "watcher_idle": p.watcher_idle,
            "mirror_idle": p.mirror_idle,
            "resize_settled": p.resize_settled,
            "session_ids": p.session_ids,
        })).collect::<Vec<_>>(),
    })
}

pub(crate) fn completion_response(
    project: &str,
    room: &str,
    msg: &OutboxMessage,
    quiet: Duration,
    decision: &PurgeDecision,
    snapshot: &teardown::PurgeSnapshot,
) -> serde_json::Value {
    let mut closed_ids = Vec::new();
    let mut already_closed_ids = Vec::new();
    let mut failed_ids = Vec::new();
    let mut reasons = Vec::new();
    for (sid, outcome) in &snapshot.results {
        match outcome {
            teardown::TeardownOutcome::Closed => closed_ids.push(sid.to_string()),
            teardown::TeardownOutcome::AlreadyGone => {
                already_closed_ids.push(sid.to_string());
            }
            _ => {
                failed_ids.push(sid.to_string());
                reasons.push(serde_json::json!({"session_id": sid, "reason": outcome.reason()}));
            }
        }
    }
    let current_id = match snapshot.phase {
        teardown::PurgePhase::Started(sid)
            if !snapshot.results.iter().any(|(id, _)| *id == sid) =>
        {
            Some(sid)
        }
        _ => None,
    };
    let unattempted_ids: Vec<String> = snapshot
        .ids
        .iter()
        .filter(|sid| {
            Some(**sid) != current_id && !snapshot.results.iter().any(|(done, _)| done == *sid)
        })
        .map(Uuid::to_string)
        .collect();
    let uncertain = snapshot.phase != teardown::PurgePhase::Stopped || snapshot.reason.is_some();
    let status = if uncertain || !failed_ids.is_empty() {
        "partial_failure"
    } else {
        "purged"
    };
    serde_json::json!({
        "action": PURGE_WG_ACTION,
        "workgroup": format!("{}:{}", project, room),
        "status": status,
        "requested_by": msg.from,
        "quiet_period_ms": quiet.as_millis() as u64,
        "dry_run": false,
        "purged": closed_ids.len(),
        "already_closed": already_closed_ids.len(),
        "failed": failed_ids.len(),
        "session_ids": closed_ids,
        "already_closed_ids": already_closed_ids,
        "failed_ids": failed_ids,
        "current_uncertain_id": current_id,
        "unattempted_ids": unattempted_ids,
        "reason": snapshot.reason,
        "failure_reasons": reasons,
        "outcome_unknown": uncertain,
        "may_complete_later": uncertain && snapshot.started
            && snapshot.phase != teardown::PurgePhase::Stopped,
        "peers": decision.peers.iter().map(|p| {
            let current = current_id.is_some_and(|id| p.session_ids.contains(&id.to_string()));
            let outcome = if current {
                "uncertain"
            } else if failed_ids.iter().any(|f| p.session_ids.contains(f)) {
                "failed"
            } else if already_closed_ids.iter().any(|c| p.session_ids.contains(c))
                && !closed_ids.iter().any(|c| p.session_ids.contains(c))
            {
                "already_closed"
            } else if closed_ids.iter().any(|c| p.session_ids.contains(c)) {
                "closed"
            } else {
                "no_match"
            };
            serde_json::json!({
                "name": p.fqn,
                "outcome": outcome,
                "session_ids": p.session_ids,
                "purgeable": p.purgeable,
            })
        }).collect::<Vec<_>>(),
    })
}
