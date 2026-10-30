use super::handoff::{is_viable_root_recipient, is_viable_wake_candidate};
use super::poller::MailboxPoller;
use super::pty_observation::PtyAuthorityState;
use super::settle::filter_sessions_by_fqn;
use crate::config::settings::SettingsState;
use crate::config::teams;
use crate::filesystem::phone_mailbox as mailbox_disk;
use crate::phone::types::OutboxMessage;
use crate::session::manager::SessionManager;
use crate::session::session::SessionStatus;
use crate::MasterToken;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    /// Find the best session for a given agent name (matches by working directory).
    /// Prefers active/running non-temp sessions over idle/exited ones.
    ///
    /// Used ONLY by stale-token logging (mailbox.rs:456, 501). Routing now uses
    /// `find_live_candidates` — do not add new callers. (grinch G.L5.)
    pub(crate) async fn find_active_session<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        agent_name: &str,
    ) -> Option<Uuid> {
        let session_mgr = app.pty_sessions();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        log::debug!(
            "[mailbox] find_active_session for '{}' — {} sessions: {:?}",
            agent_name,
            sessions.len(),
            sessions
                .iter()
                .map(|s| format!(
                    "{}={} status={:?} name={}",
                    s.id, s.working_directory, s.status, s.name
                ))
                .collect::<Vec<_>>()
        );

        // §AR2-G2: exact-FQN filter. Post-§AR2-norm, `agent_name` is canonical
        // (or a legacy form that genuinely matches only one project's CWD). The
        // substring/suffix fuzziness from the pre-fix code is gone — cross-project
        // leakage is impossible at this layer.
        let mut matches = filter_sessions_by_fqn(&sessions, agent_name);

        if matches.is_empty() {
            log::warn!("[mailbox] No session matched for '{}'", agent_name);
            return None;
        }

        log::debug!(
            "[mailbox] {} CWD matches for '{}': {:?}",
            matches.len(),
            agent_name,
            matches
                .iter()
                .map(|s| format!("{}({})", s.id, s.name))
                .collect::<Vec<_>>()
        );

        // Sort: non-temp first (false < true), then Active/Running before Idle before Exited
        matches.sort_by_key(|s| {
            let is_temp = s
                .name
                .starts_with(crate::session::session::TEMP_SESSION_PREFIX);
            let status = match s.status {
                SessionStatus::Active | SessionStatus::Running => 0u8,
                SessionStatus::Idle => 1,
                SessionStatus::Exited(_) => 2,
            };
            (is_temp, status)
        });

        let best = &matches[0];
        log::debug!(
            "[mailbox] Best match for '{}': session {} (name='{}', status={:?})",
            agent_name,
            best.id,
            best.name,
            best.status
        );
        Uuid::parse_str(&best.id).ok()
    }

    /// Find ALL viable CWD-matched candidates for wake delivery, sorted by
    /// preference (Active/Running first, then Idle, then Exited; non-temp before
    /// temp). Filters out records whose `SessionStatus` is non-`Exited` but
    /// whose PtyManager entry is missing — those are desync phantoms (issue
    /// #223). `Exited(_)` candidates are RETAINED for the respawn path.
    ///
    /// Returns `(viable, had_any_match)`:
    /// - `viable`: viable candidates with captured `SessionStatus` so the caller
    ///   can decide Inject-vs-RespawnExited without a second `list_sessions`
    ///   scan per candidate (grinch G.M2 / dev-rust R1.B3 alt). The captured
    ///   status may be stale by the time the caller acts on it; the
    ///   `err_is_pty_session_missing` continue arm in `deliver_wake` is the
    ///   load-bearing safety net for that race.
    /// - `had_any_match`: true if at least one CWD-matched record existed BEFORE
    ///   the predicate filter — the caller uses this to distinguish "no record
    ///   at all" (cold spawn) from "phantoms only" (warm spawn with auto-resume,
    ///   preserves on-disk Claude/Codex/Antigravity transcript). (grinch G.H2.)
    pub(crate) async fn find_live_candidates<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        agent_name: &str,
    ) -> (Vec<(Uuid, SessionStatus)>, bool /* had_any_match */) {
        let session_mgr = app.pty_sessions();

        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        let mut matches = filter_sessions_by_fqn(&sessions, agent_name);

        let had_any_match = !matches.is_empty();
        if !had_any_match {
            return (Vec::new(), false);
        }

        // Same sort key as the legacy `find_active_session`.
        // TODO(#223-fu1): once AC3's PTY-exit hook lands and starts emitting
        // non-zero exit codes, add a tertiary tie-break (e.g. created_at) so
        // Exited-within-bucket ordering is deterministic. (grinch G.L2.)
        matches.sort_by_key(|s| {
            let is_temp = s
                .name
                .starts_with(crate::session::session::TEMP_SESSION_PREFIX);
            let status = match s.status {
                SessionStatus::Active | SessionStatus::Running => 0u8,
                SessionStatus::Idle => 1,
                SessionStatus::Exited(_) => 2,
            };
            (is_temp, status)
        });

        let candidates: Vec<(Uuid, SessionStatus, String)> = matches
            .iter()
            .filter_map(|s| {
                Uuid::parse_str(&s.id)
                    .ok()
                    .map(|id| (id, s.status.clone(), s.name.clone()))
            })
            .collect();
        drop(mgr);

        let mut viable = Vec::new();
        for (id, status, name) in candidates {
            let has_pty = self.has_pty_session_for_wake(app, id).await;
            if !is_viable_wake_candidate(&status, has_pty) {
                // Phantom skip — observable in prod logs to scope the AC3
                // follow-up. (dev-rust R1.B4.)
                log::warn!(
                    "[mailbox] skipping desync phantom: id={} status={:?} has_pty={} name='{}'",
                    id,
                    status,
                    has_pty,
                    name
                );
            } else {
                viable.push((id, status));
            }
        }

        log::debug!(
            "[mailbox] {} viable wake candidate(s) for '{}' (had_any_match={}): {:?}",
            viable.len(),
            agent_name,
            had_any_match,
            viable
                .iter()
                .map(|(id, _)| id.to_string())
                .collect::<Vec<_>>(),
        );
        (viable, had_any_match)
    }

    /// Locate the live Root Agent session for routing `msg.to == ROOT_AGENT_SENDER`.
    ///
    /// Returns the first match by `SessionManager` iteration order. Persistence
    /// dedup (`config/sessions_persistence.rs:242-329`) converges to a single
    /// root session at steady state, but multiple records may exist transiently
    /// during concurrent spawns — a defensive log fires when more than one
    /// matches so future code does not silently rely on "exactly one" as a
    /// structural invariant.
    ///
    /// Filters via `is_viable_root_recipient` (stricter than
    /// `is_viable_wake_candidate`) — Exited records are returned as `None` so
    /// the caller's no-spawn guard sees `(empty, false)` instead of triggering
    /// the destroy-and-respawn path on a user-launched session.
    pub(crate) async fn find_root_session_candidate<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
    ) -> Option<(Uuid, SessionStatus)> {
        let session_mgr = app.pty_sessions();
        let pty_mgr = app.pty_routes();

        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        let matches: Vec<_> = sessions
            .iter()
            .filter(|s| {
                s.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&s.working_directory)
            })
            .collect();
        if matches.len() > 1 {
            log::warn!(
                "[mailbox] root recipient: {} root sessions found ({}); routing to first. \
                Persistence dedup should converge.",
                matches.len(),
                matches
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        let candidate = matches.into_iter().next()?;

        let id = Uuid::parse_str(&candidate.id).ok()?;
        let pty = pty_mgr.lock().unwrap();
        let has_pty = pty.has_session(id);
        drop(pty);

        if !is_viable_root_recipient(&candidate.status, has_pty) {
            log::warn!(
                "[mailbox] root recipient: not viable (id={} status={:?} has_pty={}); preserving \
                record, no destroy",
                id,
                candidate.status,
                has_pty
            );
            return None;
        }

        Some((id, candidate.status.clone()))
    }

    /// Find ALL sessions matching an agent name (by working directory).
    /// Returns all matching session UUIDs, not just the "best" one.
    ///
    /// §AR2-G2: exact-FQN filter (same simplification as `find_active_session`).
    async fn find_all_sessions<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        agent_name: &str,
    ) -> Vec<Uuid> {
        let session_mgr = app.pty_sessions();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        filter_sessions_by_fqn(&sessions, agent_name)
            .into_iter()
            .filter_map(|s| Uuid::parse_str(&s.id).ok())
            .collect()
    }

    async fn wait_for_restored_close_targets<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        target: &str,
        restore_flag: &crate::RestoreInProgress,
        session_ids: &mut Vec<Uuid>,
    ) -> bool {
        let mut restore_in_progress_result = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let poll = std::time::Duration::from_millis(100);
        // We can't pass `self.find_all_sessions(...)` as a closure
        // because of the `&self` capture + `async` future shape, so
        // inline the wait loop instead of going through the pure helper.
        // The helper's logic is unit-tested separately (D.5a).
        loop {
            if std::time::Instant::now() >= deadline {
                if restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                    restore_in_progress_result = true;
                }
                break;
            }
            tokio::time::sleep(poll).await;
            *session_ids = self.find_all_sessions(app, target).await;
            if !session_ids.is_empty() {
                break;
            }
            if !restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                // Flag cleared mid-wait. One final probe (the restore
                // task may have just inserted our target as its last
                // act before the guard dropped), then fall through.
                *session_ids = self.find_all_sessions(app, target).await;
                break;
            }
        }
        restore_in_progress_result
    }

    /// Handle close-session action: validate coordinator auth, find target sessions, destroy them.
    ///
    /// NEW ACTION HANDLERS: resolve user-supplied target fields via
    /// `config::teams::resolve_agent_target` BEFORE privileged operations.
    /// The outbox is a trust boundary — any new destructive action must
    /// canonicalize its target here, not rely on CLI-side resolution.
    ///
    /// §224 G-IMPL-2 — `is_app_outbox`: true when the message file lives
    /// under the instance-private app-outbox (master/root-token path).
    /// Used by the response-write block (A.6) to skip the outbox-relative
    /// primary write — for app-outbox messages it would land under
    /// `<config_dir>/instances/<id>/responses/`, a directory the CLI does
    /// not poll, leaking orphan JSON files with no GC.
    pub(crate) async fn handle_close_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let raw_target = msg
            .target
            .as_deref()
            .ok_or_else(|| "close-session requires 'target' field".to_string())?;

        // §AR2-G1: resolve the target to a canonical FQN BEFORE authorization.
        // Even if the CLI skipped resolution (direct outbox write, old client,
        // hand-crafted JSON), the mailbox is the authoritative gate.
        let resolved_target = {
            let paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            match crate::config::teams::resolve_agent_target(raw_target, &paths) {
                Ok(fqn) => fqn,
                Err(e) => {
                    return self
                        .reject_message(
                            path,
                            msg,
                            &format!("close-session target unresolvable: {}", e),
                        )
                        .await;
                }
            }
        };
        let target = resolved_target.as_str();

        // Re-check master token for coordinator auth bypass (independent of routing bypass above)
        let is_master = if let Some(ref token_str) = msg.token {
            let master = app.state::<MasterToken>();
            if master.matches(token_str) {
                true
            } else {
                let settings = crate::config::settings::load_settings();
                settings.root_token.as_deref() == Some(token_str.as_str())
            }
        } else {
            false
        };

        if !is_master {
            let discovered = teams::discover_teams();
            if !teams::is_coordinator_of(&msg.from, target, &discovered) {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "Not authorized: '{}' is not an orchestrator of '{}' team",
                            msg.from, target
                        ),
                    )
                    .await;
            }
        }

        // Find all sessions for the target agent.
        //
        // §224 A.2 — empty `session_ids` is NOT an error. The pre-fix code
        // rejected with "No active session found", which conflicted with
        // `list-sessions` reporting the session as alive (ghost rows from
        // `persist_merging_failed`; see A.1). Now we fall through to a
        // successful no-op response with status="no_match".
        //
        // §224 A.2.5 — daemon-restart race guard. If the initial probe is
        // empty AND restore is still in progress, poll up to 5s for either
        // (a) the flag to clear, or (b) a matching session to appear. Three
        // outcomes:
        //   * session appears   → fall through to the kill loop, status="closed".
        //   * flag clears empty → fall through to no_match path.
        //   * 5s elapses, flag still set → restore_in_progress_result = true.
        let mut session_ids = self.find_all_sessions(app, target).await;
        let mut restore_in_progress_result = false;
        if session_ids.is_empty() {
            let restore_flag = app.state::<Arc<crate::RestoreInProgress>>();
            if restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                restore_in_progress_result = self
                    .wait_for_restored_close_targets(app, target, &restore_flag, &mut session_ids)
                    .await;
            }
        }

        let force = msg.force.unwrap_or(false);
        let timeout_secs = msg.timeout_secs.unwrap_or(30);

        log_close_session_attempt(&session_ids, target, &msg.from, force, timeout_secs);

        let mut closed_ids: Vec<String> = Vec::new();
        let mut failure = None;
        let mut outcome_unknown = false;
        let mut actively_closed = false;
        let mut may_complete_later = false;
        let budget = super::teardown::observation_budget(force, timeout_secs)
            .ok_or_else(|| "close observation budget overflow".to_string())?;
        for sid in &session_ids {
            let accepted = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let outcome = super::teardown::observe_close_attempt(
                budget,
                accepted.clone(),
                self.close_session_attempt(app, *sid, force, timeout_secs, accepted),
            )
            .await;
            if outcome.confirmed() {
                actively_closed |= matches!(outcome, super::teardown::TeardownOutcome::Closed);
                closed_ids.push(sid.to_string());
            } else {
                failure = outcome.reason().map(str::to_string);
                if let super::teardown::TeardownOutcome::Uncertain {
                    may_complete_later: pending,
                    ..
                } = outcome
                {
                    may_complete_later = pending;
                    outcome_unknown = true;
                }
                break;
            }
        }

        // §224 A.7 — active ghost cleanup. After A.2.5 has confirmed (with
        // wait-and-retry) that no session matches the target, force a
        // sessions.json rewrite from the live SessionManager to drop any
        // stale persisted entry. Without this, a user with only the ghost
        // session sees the contradiction persist across multiple
        // close-session invocations (zero lifecycle events to trigger
        // passive cleanup). Cost: one disk write per no-match call.
        //
        // Skip when A.2.5 returned restore_in_progress: the restore task
        // is itself about to write the snapshot when it completes; racing
        // that write is wasted I/O and would clobber recipes for sessions
        // whose restore is pending.
        //
        // Skip when session_ids is non-empty: that's either "closed" or
        // "already_closed", and the destroy path's downstream events will
        // trigger persist_current_state organically.
        //
        // §224 G-IMPL-4 (NIT, accepted) — snapshot vs concurrent create_session
        // is racy in the ~5-50ms window between `snapshot_sessions` and
        // `save_sessions`. If a `create_session` for any target lands in
        // that window and runs its own `persist_current_state` BEFORE our
        // `save_sessions` writes, the new session is dropped from disk
        // until the next lifecycle event re-persists. Impact: brief disk
        // inconsistency, recoverable. Accepted.
        //
        // §224 G-IMPL-6 (NIT, accepted) — A.7 also drops unrelated failed-
        // recoverable ghosts. The snapshot includes only live SessionManager
        // entries; if `sessions.json` has a failed-recoverable ghost for
        // unrelated agent X, this rewrite drops X's ghost too. This is the
        // pre-existing behavior of every `persist_current_state` caller
        // (see `persist_merging_failed` docstring); A.7 just introduces a
        // new caller outside lifecycle events. Accepted.
        if session_ids.is_empty() && !restore_in_progress_result {
            // Persist through the serialized snapshot+write path so this cleanup
            // cannot replay a stale snapshot after a completed Telegram toggle.
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            if let Err(e) =
                crate::config::sessions_persistence::persist_current_state_result(&mgr).await
            {
                log::warn!(
                    "[mailbox] close-session: failed to persist cleaned sessions.json after \
                    no_match: {}",
                    e
                );
            }
        }

        write_close_session_response(
            path,
            msg,
            is_app_outbox,
            CloseSessionOutcome {
                target,
                restore_in_progress_result,
                session_ids: &session_ids,
                closed_ids,
                failure,
                outcome_unknown,
                actively_closed,
                may_complete_later,
            },
            self.resolve_repo_path(&msg.from, app),
        )
        .await;

        // Move original message to delivered/
        self.move_to_delivered(app, path, msg).await
    }
}

fn log_close_session_attempt(
    session_ids: &[Uuid],
    target: &str,
    requested_by: &str,
    force: bool,
    timeout_secs: u32,
) {
    // §224 A.2 — gate the kill-loop log on non-empty session_ids so we
    // don't emit a misleading "force-killing 0 session(s)" line on the
    // no_match / restore_in_progress paths.
    if !session_ids.is_empty() {
        log::info!(
            "[mailbox] close-session: {} {} session(s) for '{}' (requested by '{}', \
            timeout={}s)",
            if force {
                "force-killing"
            } else {
                "gracefully closing"
            },
            session_ids.len(),
            target,
            requested_by,
            timeout_secs
        );
    }
}

struct CloseSessionOutcome<'a> {
    target: &'a str,
    restore_in_progress_result: bool,
    session_ids: &'a [Uuid],
    closed_ids: Vec<String>,
    failure: Option<String>,
    outcome_unknown: bool,
    actively_closed: bool,
    may_complete_later: bool,
}

// The sender lookup stays lazy: resolve only after the outbox-relative write,
// and never when there is no request ID or serialization fails.
async fn write_close_session_response(
    path: &std::path::Path,
    msg: &OutboxMessage,
    is_app_outbox: bool,
    outcome: CloseSessionOutcome<'_>,
    sender_path: impl std::future::Future<Output = Option<String>>,
) {
    let CloseSessionOutcome {
        target,
        restore_in_progress_result,
        session_ids,
        closed_ids,
        failure,
        outcome_unknown,
        actively_closed,
        may_complete_later,
    } = outcome;
    // Write response with details.
    //
    // §224 A.2 + A.2.5 — four terminal states, all exit-0 from the user's
    // seat:
    //   "restore_in_progress" — daemon still in startup; couldn't decide.
    //   "no_match"            — found zero live sessions matching the FQN.
    //   "already_closed"      — found some, but every one vanished before
    //                           destroy (race).
    //   "closed"              — at least one was actively killed.
    if let Some(ref rid) = msg.request_id {
        let status = if failure.is_some() {
            "timeout"
        } else if restore_in_progress_result {
            "restore_in_progress"
        } else if session_ids.is_empty() {
            "no_match"
        } else if !actively_closed {
            "already_closed"
        } else {
            "closed"
        };
        let response = serde_json::json!({
            "action": "close-session",
            "target": target,
            "status": status,
            "sessions_closed": closed_ids.len(),
            "reason": failure,
            "outcome_unknown": outcome_unknown,
            "may_complete_later": may_complete_later,
            "session_ids": closed_ids,
            "requested_by": msg.from,
        });
        let json = match serde_json::to_string_pretty(&response) {
            Ok(j) => j,
            Err(e) => {
                log::warn!(
                    "[mailbox] Failed to serialize close-session response: {}",
                    e
                );
                return;
            }
        };

        // §224 A.6 — dual-write the response:
        //
        // (1) <message_file_dir>/../responses/<rid>.json — always derivable
        //     from `path` (the queued message's file location). This is
        //     exactly `<ac_dir>/responses/<rid>.json`, the CLI's polled
        //     location (close_session.rs:194-195), so no resolve_repo_path
        //     dependency.
        //
        // (2) <resolve_repo_path(msg.from)>/<agent_local_dir>/responses/<rid>.json
        //     — preserves cross-agent delivery for cases where the sender
        //     FQN points to a different ac_dir than the outbox file's
        //     parent. Best-effort; failure does not affect (1).
        //
        // Either write succeeding is enough for the CLI to receive the
        // response.
        //
        // §224 G-IMPL-2 — skip the derived primary write (1) when the
        // message came from the app-outbox (master/root-token path).
        // For app-outbox messages the parent path is
        // `<config_dir>/instances/<id>/outbox/`, so the derived responses
        // dir resolves to `<config_dir>/instances/<id>/responses/` — a
        // directory the CLI never polls (it always polls
        // `<--root>/.<bin_stem>/responses/`). Writing there leaks orphan
        // JSON files with no GC; the resolved-sender path (2) is the only
        // useful target for the master-token case. If `resolve_repo_path`
        // also fails (msg.from not enumerable), the CLI hits the response
        // timeout and exits 2 (G-IMPL-3) — "outcome unknown".
        if !is_app_outbox {
            let outbox_relative_responses_dir = path
                .parent()
                .and_then(|p| p.parent())
                .map(|ac_dir| ac_dir.join("responses"));
            if let Some(responses_dir) = outbox_relative_responses_dir {
                let _ = mailbox_disk::ensure_mailbox_archive_directory(&responses_dir);
                let response_path = responses_dir.join(format!("{}.json", rid));
                if let Err(e) = mailbox_disk::write_mailbox_document(&response_path, &json) {
                    log::warn!(
                        "[mailbox] Failed to write close-session response to outbox-relative \
                        path {:?}: {}",
                        response_path,
                        e
                    );
                }
            } else {
                log::warn!(
                    "[mailbox] close-session: cannot derive outbox-relative responses dir from \
                    message path {:?}",
                    path
                );
            }
        }

        if let Some(sender_path) = sender_path.await {
            let responses_dir = std::path::PathBuf::from(sender_path)
                .join(crate::config::agent_local_dir_name())
                .join("responses");
            let _ = mailbox_disk::ensure_mailbox_archive_directory(&responses_dir);
            let response_path = responses_dir.join(format!("{}.json", rid));
            if let Err(e) = mailbox_disk::write_mailbox_document(&response_path, &json) {
                log::warn!(
                    "[mailbox] Failed to write close-session response to resolved-sender path: \
                    {}",
                    e
                );
            }
        } else if is_app_outbox {
            // §224 G-IMPL-2 — app-outbox call AND resolve_repo_path failed
            // means the CLI's `--root` is not enumerable in project_paths.
            // Both write paths above are unreachable; the CLI will hit its
            // response-poll timeout and exit 2 ("outcome unknown",
            // G-IMPL-3). Log explicitly so operators can debug.
            log::warn!(
                "[mailbox] close-session app-outbox response is undeliverable: \
                 resolve_repo_path(msg.from='{}') returned None — the sender's --root \
                 is not enumerable in project_paths. CLI will timeout with exit 2.",
                msg.from
            );
        }
    }
}

#[cfg(test)]
mod tests;
