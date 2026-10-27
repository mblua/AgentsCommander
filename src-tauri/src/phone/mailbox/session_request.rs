#[cfg(not(test))]
use super::handoff::SELF_CLEAR_MAX_DEFER;
#[cfg(not(test))]
use super::handoff::SELF_CLEAR_POLL;
#[cfg(not(test))]
use super::handoff::SELF_CLEAR_SETTLE;
use super::handoff::{ARCHIVE_TIMESTAMP_FORMAT, PURGE_WG_ACTION, SELF_HANDOFF_ROOT_NAME};
use super::notice::derive_project_from_outbox_path;
use super::poller::{evaluate_gate, MailboxPoller, PurgeGatePeer, PurgeGateSession};
use super::settle::{archive_root_md, capture_self_forget_summary, filter_sessions_by_fqn};
use crate::config::sessions_persistence::RaiseHandPersistOutcome;
use crate::config::settings::SettingsState;
use crate::phone::types::OutboxMessage;
use crate::pty::inject::LogicalPtyCommand;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    /// (#885) Handle `purge-wg`: purge every peer in the caller's own workgroup.
    ///
    /// Sequence is non-reorderable. Steps 1-14 per plan §5.5c, consensus round 1.
    pub(crate) async fn handle_purge_wg<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        // 1. Identity gate — already enforced by the caller (§5.5b).
        debug_assert!(
            msg.token.is_some(),
            "purge-room requires a session token; caller must enforce"
        );

        // 2. Resolve WG scope and authorization, one call (§3.6).
        let effective_paths = {
            let mut paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            match derive_project_from_outbox_path(path) {
                Ok(Some(root_project)) => {
                    let canon = crate::filesystem::projects::canonical_refresh_project(
                        std::path::Path::new(&root_project),
                    )
                    .ok();
                    let already = paths.iter().any(|p| match &canon {
                        Some(ct) => {
                            crate::filesystem::projects::canonical_refresh_project(
                                std::path::Path::new(p),
                            )
                            .ok()
                            .as_ref()
                                == Some(ct)
                        }
                        None => p == &root_project,
                    });
                    if !already {
                        paths.push(root_project);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    return self.reject_message(path, msg, &e).await;
                }
            }
            paths
        };
        let wg =
            match crate::config::teams::verified_wg_coordinator_target(&msg.from, &effective_paths)
            {
                Some(wg) => wg,
                None => {
                    return self
                        .reject_message(
                            path,
                            msg,
                            &format!(
                                "Not authorized: '{}' is not the verified orchestrator of its room",
                                msg.from
                            ),
                        )
                        .await;
                }
            };

        // 3. --wg assertion (§3.6: interlock, not selector).
        if let Some(ref t) = msg.target {
            if t != &wg.wg_name {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "purge-room: --wg assertion '{}' does not match resolved room '{}'",
                            t, wg.wg_name
                        ),
                    )
                    .await;
            }
        }

        // 4. Enumerate peers (Guard A). Only `__agent_*` dirs under the WG dir.
        //    The coordinator does not purge itself.
        let wg_dir = match wg.replica_dir.parent() {
            Some(d) => d,
            None => {
                return self
                    .reject_message(path, msg, "purge-room: cannot resolve Room directory")
                    .await;
            }
        };
        let mut peer_fqns: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(wg_dir) {
            for entry in entries.flatten() {
                let name = match entry.file_name().to_str() {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let agent = match name.strip_prefix("__agent_") {
                    Some(a) => a.to_string(),
                    None => continue,
                };
                if agent == wg.agent_name {
                    continue;
                }
                if !entry.path().is_dir() {
                    continue;
                }
                peer_fqns.push(format!("{}:{}/{}", wg.project, wg.wg_name, agent));
            }
        }
        peer_fqns.sort();

        // 5. Take the mirror snapshot ONCE (F-4). One list_sessions(), one
        //    PTY-liveness pass. Per-peer filtering reuses the pure predicate.
        let sessions: Vec<SessionInfo> = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.list_sessions().await
        };
        let pty_live: std::collections::HashSet<Uuid> = {
            let pty = app.state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>();
            let mgr = pty.lock().unwrap();
            sessions
                .iter()
                .filter_map(|s| Uuid::parse_str(&s.id).ok())
                .filter(|id| mgr.has_session(*id))
                .collect()
        };

        // 6. Restore-in-progress guard (F-2: exit 3, not 0).
        {
            let restore_flag = app.state::<Arc<crate::RestoreInProgress>>();
            if restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                let response = serde_json::json!({
                    "action": PURGE_WG_ACTION,
                    "workgroup": format!("{}:{}", wg.project, wg.wg_name),
                    "status": "restore_in_progress",
                    "requested_by": msg.from,
                    "peers": [],
                });
                return self
                    .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                    .await;
            }
        }

        // 7. Liveness and Guard B (F-3: !Exited && has_pty; F-12: root guard).
        //    Collect live session info; the readiness snapshot (step 8) is
        //    correlated into gate_peers after.
        struct PeerLiveInfo {
            fqn: String,
            all_session_ids: Vec<String>,
            live_sessions: Vec<(Uuid, bool)>, // (session_id, mirror_idle)
        }
        let mut peer_infos: Vec<PeerLiveInfo> = Vec::with_capacity(peer_fqns.len());
        let mut all_live_session_ids: Vec<Uuid> = Vec::new();

        for fqn in &peer_fqns {
            let matched: Vec<&SessionInfo> = filter_sessions_by_fqn(&sessions, fqn);
            let all_session_ids: Vec<String> = matched.iter().map(|s| s.id.clone()).collect();

            let mut live_sessions: Vec<(Uuid, bool)> = Vec::new();
            for s in &matched {
                // Guard B (root): corrupted-state assertion (F-12).
                if s.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&s.working_directory)
                {
                    let response = serde_json::json!({
                        "action": PURGE_WG_ACTION,
                        "workgroup": format!("{}:{}", wg.project, wg.wg_name),
                        "status": "failed_root_guard",
                        "requested_by": msg.from,
                        "offending_session_id": s.id,
                        "offending_working_directory": s.working_directory,
                        "peers": [],
                    });
                    return self
                        .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                        .await;
                }

                let sid = match Uuid::parse_str(&s.id) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let is_live =
                    !matches!(s.status, SessionStatus::Exited(_)) && pty_live.contains(&sid);
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

        // 8. Readiness snapshot and the four-leg gate (§2.3.3, F-1).
        let quiet = std::time::Duration::from_millis(
            msg.quiet_period_ms.unwrap_or(3000).max(
                crate::session::profile::IdleTuning::DEFAULT
                    .idle_threshold
                    .as_millis() as u64,
            ),
        );
        let readiness_map: std::collections::HashMap<
            Uuid,
            crate::pty::idle_detector::PurgeReadiness,
        > = {
            let idle = app.state::<Arc<crate::pty::idle_detector::IdleDetector>>();
            let readiness = idle.purge_readiness(&all_live_session_ids);
            readiness.into_iter().map(|r| (r.session_id, r)).collect()
        };
        // Correlate into gate_peers.
        let gate_peers: Vec<PurgeGatePeer> = peer_infos
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
                                resize_grace: crate::session::profile::IdleTuning::DEFAULT
                                    .resize_grace,
                                idle_threshold: crate::session::profile::IdleTuning::DEFAULT
                                    .idle_threshold,
                                silence_age: None,
                            },
                        ),
                        mirror_idle: *mirror_idle,
                    })
                    .collect(),
            })
            .collect();

        let decision = evaluate_gate(&gate_peers, quiet);

        // 9. Dry-run exit, BEFORE the gate rejection (F-9).
        if msg.dry_run == Some(true) {
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
            let response = serde_json::json!({
                "action": PURGE_WG_ACTION,
                "workgroup": format!("{}:{}", wg.project, wg.wg_name),
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
            });
            return self
                .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                .await;
        }

        // 10. The gate. If any peer is not purgeable: reject, destroy nothing.
        if !decision.passed {
            let response = serde_json::json!({
                "action": PURGE_WG_ACTION,
                "workgroup": format!("{}:{}", wg.project, wg.wg_name),
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
            });
            return self
                .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                .await;
        }

        // 11. Acquire the lease (F-14: clone Arc into a named local first).
        let mut target_sids: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        let mut target_fqns: std::collections::HashSet<String> = std::collections::HashSet::new();
        for peer in &gate_peers {
            for session in &peer.live {
                target_sids.insert(session.session_id);
            }
            target_fqns.insert(peer.fqn.clone());
        }
        // Also include non-live session IDs (Exited records) for destruction.
        for peer in &gate_peers {
            for sid_str in &peer.all_session_ids {
                if let Ok(sid) = Uuid::parse_str(sid_str) {
                    target_sids.insert(sid);
                }
            }
        }
        let purge_guard: Arc<crate::session::purge_guard::PurgeGuard> = app
            .state::<Arc<crate::session::purge_guard::PurgeGuard>>()
            .inner()
            .clone();
        let lease = purge_guard.acquire(target_sids, target_fqns).await;

        // 12. Destroy loop (past the commit point, §2.5; no re-check).
        let force = msg.force.unwrap_or(true);
        let timeout_secs = msg.timeout_secs.unwrap_or(5);
        let mut closed_ids: Vec<String> = Vec::new();
        let mut failed_ids: Vec<String> = Vec::new();
        let mut already_closed_ids: Vec<String> = Vec::new();
        let mut any_failed = false;

        for peer in &gate_peers {
            for sid_str in &peer.all_session_ids {
                let sid = match Uuid::parse_str(sid_str) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let ok = if force {
                    self.force_close_session(app, sid).await
                } else {
                    self.graceful_close_session(app, sid, timeout_secs).await
                };
                if ok {
                    closed_ids.push(sid_str.clone());
                } else {
                    // (#885 E-4) Post-failure re-probe: if the record is gone,
                    // the destroy failed because auto-close raced us and the
                    // session is already destroyed. That is success, not
                    // failure. Sound because `destroy_session_inner` returns
                    // Err("Session not found") ONLY from its first statement;
                    // every other failure (notably kill_group's ??) leaves
                    // the record present, so a genuine failure is always
                    // distinguishable by the record still being there.
                    let gone = {
                        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                        let mgr = session_mgr.read().await;
                        mgr.get_session(sid).await.is_none()
                    };
                    if gone {
                        already_closed_ids.push(sid_str.clone());
                    } else {
                        failed_ids.push(sid_str.clone());
                        any_failed = true;
                    }
                }
            }
        }

        // 13. Drop the lease before writing the response.
        drop(lease);

        // 14. Response + move_to_delivered.
        let status = if any_failed {
            "partial_failure"
        } else {
            "purged"
        };
        let response = serde_json::json!({
            "action": PURGE_WG_ACTION,
            "workgroup": format!("{}:{}", wg.project, wg.wg_name),
            "status": status,
            "requested_by": msg.from,
            "quiet_period_ms": quiet.as_millis() as u64,
            "dry_run": false,
            "purged": closed_ids.len(),
            "already_closed": already_closed_ids.len(),
            "failed": failed_ids.len(),
            "peers": decision.peers.iter().map(|p| {
                let outcome = if failed_ids.iter().any(|f| p.session_ids.contains(f)) {
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
        });

        self.write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
            .await
    }

    /// (#885) Write the purge response JSON and move the message to delivered/.
    /// Mirrors `handle_close_session`'s dual-write block (§224 A.6, G-IMPL-2).
    async fn write_purge_response_and_deliver<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
        response: &serde_json::Value,
    ) -> Result<(), String> {
        let json = match serde_json::to_string_pretty(response) {
            Ok(j) => j,
            Err(e) => {
                log::warn!("[mailbox] Failed to serialize purge-wg response: {}", e);
                return self.move_to_delivered(app, path, msg).await;
            }
        };

        if let Some(ref rid) = msg.request_id {
            if !is_app_outbox {
                let outbox_relative_responses_dir = path
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|ac_dir| ac_dir.join("responses"));
                if let Some(responses_dir) = outbox_relative_responses_dir {
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let response_path = responses_dir.join(format!("{}.json", rid));
                    if let Err(e) = std::fs::write(&response_path, &json) {
                        log::warn!(
                            "[mailbox] Failed to write purge-wg response to outbox-relative path \
                            {:?}: {}",
                            response_path,
                            e
                        );
                    }
                }
            }

            if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                let responses_dir = std::path::PathBuf::from(sender_path)
                    .join(crate::config::agent_local_dir_name())
                    .join("responses");
                let _ = std::fs::create_dir_all(&responses_dir);
                let response_path = responses_dir.join(format!("{}.json", rid));
                if let Err(e) = std::fs::write(&response_path, &json) {
                    log::warn!(
                        "[mailbox] Failed to write purge-wg response to resolved-sender path: {}",
                        e
                    );
                }
            }
        }

        self.move_to_delivered(app, path, msg).await
    }

    /// Returns fast: the 30s sustained-idle wait runs in a detached task so the
    /// poll loop is never blocked. Idempotent: a second request while one is
    /// pending is a no-op ("already_queued").
    pub(crate) async fn handle_self_clear<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        // 1. Resolve the caller's OWN session from the token (the sole authority).
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-clear requires a valid session token; restart or respawn the session",
                    )
                    .await;
            }
        };
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.find_by_token(token_uuid).await
        };
        let session = match session {
            Some(s) => s,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-clear: no live session owns this token; restart or respawn the \
                        session",
                    )
                    .await;
            }
        };
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(path, msg, "self-clear: internal error resolving session id")
                    .await;
            }
        };

        // Resolve before the handoff existence gate so an unsupported source
        // cannot archive or queue self-maintenance state.
        let clear_text = match crate::pty::inject::resolve_logical_command_text(
            &session.shell,
            LogicalPtyCommand::Clear,
        ) {
            Some(text) => text,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "self-handoff-and-clear: session shell '{}' has no verified \
                            logical-clear mapping. Claude / Codex / Antigravity / Cursor agent \
                            direct shells use /clear; exact Pi uses /new. cmd / pwsh outer \
                            wrappers remain unsupported.",
                            session.shell
                        ),
                    )
                    .await;
            }
        };

        // 2b. #626 existence gate - REFUSE if the agent did not write its handoff notes. Clearing
        // with
        // no SELF-HANDOFF.md would wipe context with no way to resume (the agent would post-clear
        // read a nonexistent file = blank). Queue-time intent guard (not transactional): the real
        //     read-time safety net is the handoff prompt's "if missing or empty, wait" clause
        //     (self_clear_handoff_base_prompt). Use .is_file() so a stray directory named
        //     SELF-HANDOFF.md does not pass. Runs BEFORE the idempotency insert and the archive, so
        //     nothing is queued/archived with nothing to resume.
        let handoff_path =
            std::path::Path::new(&session.working_directory).join(SELF_HANDOFF_ROOT_NAME);
        if !handoff_path.is_file() {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-clear: SELF-HANDOFF.md not found in your root ({}); \
                         write it before requesting self-handoff-and-clear.",
                        session.working_directory
                    ),
                )
                .await;
        }

        // 3. Idempotency - atomic check-and-set. insert() returns false if already present.
        let newly_inserted = {
            let pending = app.state::<Arc<crate::PendingSelfClear>>();
            let mut set = pending.0.lock().unwrap_or_else(|e| e.into_inner());
            set.insert(session_id)
        };
        let status = if newly_inserted {
            "queued"
        } else {
            "already_queued"
        };

        // 4. Spawn the deferred two-phase sustained-idle gate (detached; never blocks the poller).
        //    MED-4: gated under #[cfg(not(test))] so handle_self_clear unit tests assert
        //    queue + idempotency WITHOUT launching a live 500ms poll task. The gate's
        //    decision policy is covered separately by the pure `self_clear_gate_advance` tests.
        //    Durations are passed in so a future integration test can drive it fast.
        if newly_inserted {
            // #626 - archive SELF-FORGET.md so the next cycle starts fresh. Best-effort: a failure
            // must not block the clear/handoff. The agent already wrote SELF-HANDOFF.md
            // (existence-gated above), so SELF-FORGET.md is present iff the agent kept one. FOLD-3:
            // runs in ALL cfgs (NOT inside the cfg(not(test)) spawn) so the harness archive
            // assertion can pass. Decoupled from clear success (queue-time): an abandoned cycle
            // leaves SELF-FORGET archived but uncleared; content is preserved in
            // self-clear/<ts>_SELF-FORGET.md, re-issue continues normally.
            let root = std::path::Path::new(&session.working_directory);
            let forgotten_summary = capture_self_forget_summary(root);
            let ts = chrono::Local::now()
                .format(ARCHIVE_TIMESTAMP_FORMAT)
                .to_string();
            match archive_root_md(root, "SELF-FORGET", &ts) {
                Ok(Some(p)) => log::info!(
                    "[mailbox] self-handoff-and-clear: archived SELF-FORGET.md -> {}",
                    p.display()
                ),
                Ok(None) => {} // no SELF-FORGET.md; nothing to archive
                Err(e) => log::warn!(
                    "[mailbox] self-handoff-and-clear: SELF-FORGET.md archive failed for session \
                    {} (non-fatal): {}",
                    session_id,
                    e
                ),
            }

            #[cfg(not(test))]
            {
                let app_clone = app.clone();
                // #749 - the root travels with the task (same queue-time snapshot the
                // SELF-FORGET archive above used) so Phase 2 can archive the handoff
                // without a mid-flight session-manager lookup. A session's working
                // directory never changes while its id stays alive.
                let root = root.to_path_buf();
                tauri::async_runtime::spawn(async move {
                    Self::run_self_clear_after_sustained_idle(
                        &app_clone,
                        session_id,
                        clear_text,
                        root,
                        forgotten_summary,
                        SELF_CLEAR_SETTLE,
                        SELF_CLEAR_POLL,
                        SELF_CLEAR_MAX_DEFER,
                    )
                    .await;
                });
            }
            #[cfg(test)]
            let _ = (clear_text, &forgotten_summary);
            log::info!(
                "[mailbox] self-handoff-and-clear queued for session {} (from '{}')",
                session_id,
                msg.from
            );
        } else {
            log::info!(
                "[mailbox] self-handoff-and-clear already pending for session {} (from '{}')",
                session_id,
                msg.from
            );
        }

        // 5. Write the queue-ack response, then move the message to delivered/.
        self.write_self_clear_response(app, path, msg, session_id, status, is_app_outbox)
            .await
    }

    fn session_has_visible_raise_hand_slot(session: &SessionInfo) -> bool {
        if !session.is_coordinator || matches!(&session.status, SessionStatus::Exited(_)) {
            return false;
        }
        let Some(task_path) =
            crate::session::session::find_workgroup_task_path_for_cwd(&session.working_directory)
        else {
            return false;
        };
        let Ok(content) = std::fs::read_to_string(task_path) else {
            return false;
        };
        crate::commands::entity_creation::parse_task_title(&content).is_some()
    }

    pub(crate) async fn handle_raise_hand<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "raise-hand requires a valid session token; restart or respawn the session",
                    )
                    .await;
            }
        };
        let mgr = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = session_mgr.read().await;
            guard.clone()
        };
        let Some(session) = mgr.find_by_token(token_uuid).await else {
            return self
                .reject_message(
                    path,
                    msg,
                    "raise-hand: no live session owns this token; restart or respawn the session",
                )
                .await;
        };
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(path, msg, "raise-hand: internal error resolving session id")
                    .await;
            }
        };

        if !Self::session_has_visible_raise_hand_slot(&session) {
            return self
                .write_raise_hand_response(
                    app,
                    path,
                    msg,
                    session_id,
                    false,
                    "not_visible",
                    is_app_outbox,
                )
                .await;
        }

        // #698 - raise the hand and persist its snapshot atomically with respect
        // to all session persistence (see `raise_hand_and_persist_result`). The
        // mutation, snapshot, and save happen under one global save lock, so no
        // concurrent persist can durably write the raised state before this
        // snapshot lands. The snapshot is durable before we emit the UI event or
        // write the success response, so a peer reading `list-sessions` sees
        // `raisedHand: true` only after it survived. On persist failure the helper
        // has already rolled back the live communication under the same lock, so
        // we just reject the message rather than reporting a success that did not
        // survive.
        let outcome = crate::config::sessions_persistence::raise_hand_and_persist_result(
            &mgr,
            session_id,
            chrono::Utc::now(),
        )
        .await;
        let (raised, status, changed_communication) = match outcome {
            Ok(RaiseHandPersistOutcome::Raised(communication)) => {
                (true, "raised", Some(communication))
            }
            Ok(RaiseHandPersistOutcome::AlreadyVisible) => (true, "already_visible", None),
            Ok(RaiseHandPersistOutcome::NotRaisable) => (false, "not_visible", None),
            Err(e) => {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!("raise-hand: failed to persist state: {}", e),
                    )
                    .await;
            }
        };

        if let Some(communication) = changed_communication {
            crate::session::selection::publish_session_communication(
                app,
                session_id,
                Some(&communication),
            );
        }

        self.write_raise_hand_response(app, path, msg, session_id, raised, status, is_app_outbox)
            .await
    }
}
