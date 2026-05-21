use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::Manager;
use uuid::Uuid;

use crate::config::agent_config::AgentLocalConfig;
use crate::config::settings::SettingsState;
use crate::config::teams;
use crate::phone::types::OutboxMessage;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionStatus;
use crate::{AppOutbox, MasterToken};

/// If `outbox_file` lives at
/// `<project_dir>/.ac-new/wg-<N>-*/__agent_*/<local-dir>/outbox/<file>.json`,
/// return `project_dir` as a UTF-8 `String`. Otherwise, `None`.
///
/// Mirrors the WG-replica walk-up in `cli::send::derive_root_project_dir` so
/// the mailbox can resolve qualified WG-peer FQNs against the same root-walk-up
/// source the sending CLI used (and the same source `list_peers::detect_wg_replica`
/// uses to report siblings as `reachable: true`). In-memory only — settings.json
/// is NOT mutated. See #228 D1-a.
///
/// The path layout is fixed by `MailboxPoller::poll` which constructs outbox
/// paths as `Path::new(<all_paths-entry>).join(agent_local_dir_name()).join("outbox")`.
/// We walk ancestors: `<file>.json` → `outbox` → `<local-dir>` → `<__agent_*>` →
/// `<wg-*>` → `<.ac-new>` → `<project_dir>`.
///
/// Uses `to_str()?` (NOT `to_string_lossy()`) for parity with
/// `list_peers::detect_wg_replica`. Keep this in lockstep with
/// `cli::send::derive_root_project_dir`.
fn derive_project_from_outbox_path(outbox_file: &Path) -> Option<String> {
    let canon = std::fs::canonicalize(outbox_file).ok()?;
    // canon = <project>/.ac-new/wg-*/__agent_*/<local-dir>/outbox/<file>.json
    let outbox_dir = canon.parent()?; // .../outbox
    if outbox_dir.file_name().and_then(|n| n.to_str()) != Some("outbox") {
        return None;
    }
    let local_dir = outbox_dir.parent()?; // .../<local-dir>
    let agent_dir = local_dir.parent()?; // .../__agent_*
    let agent_name = agent_dir.file_name().and_then(|n| n.to_str())?;
    if !agent_name.starts_with("__agent_") {
        return None;
    }
    let wg_dir = agent_dir.parent()?; // .../wg-*
    let wg_name = wg_dir.file_name().and_then(|n| n.to_str())?;
    if !wg_name.starts_with("wg-") {
        return None;
    }
    let ac_new_dir = wg_dir.parent()?; // .../.ac-new
    if ac_new_dir.file_name().and_then(|n| n.to_str()) != Some(".ac-new") {
        return None;
    }
    let project_dir = ac_new_dir.parent()?; // .../<project>
    Some(project_dir.to_str()?.to_string())
}

/// Tracks delivery attempts for a single outbox message.
struct RetryState {
    attempt_count: u32,
    logged: bool,
}

const MAX_DELIVERY_ATTEMPTS: u32 = 10;
const ERR_UNRESOLVABLE_AGENT: &str = "Could not resolve inbox for agent";

/// §DR5 anti-spoof accept rule. Outbox-sender check passes when `msg_from`
/// equals `expected_from` exactly, OR when `msg_from` is unqualified (legacy)
/// AND its local part matches `expected_from`'s local part. Qualified-but-
/// wrong-project `msg_from` is always rejected.
pub(crate) fn anti_spoof_accept(msg_from: &str, expected_from: &str) -> bool {
    if msg_from == expected_from {
        return true;
    }
    let (_, exp_local) = crate::config::teams::split_project_prefix(expected_from);
    let (msg_proj, msg_local) = crate::config::teams::split_project_prefix(msg_from);
    msg_proj.is_none() && exp_local == msg_local
}

/// §AR2-norm step (1): upgrade a legacy-unqualified `msg_from` to
/// `expected_from` when expected_from is FQN. Returns true if the upgrade
/// happened. No-op when msg_from is already qualified, expected_from is
/// None, or expected_from itself is unqualified.
pub(crate) fn canonicalize_msg_from_in_place(
    msg_from: &mut String,
    expected_from: Option<&str>,
) -> bool {
    let Some(exp) = expected_from else {
        return false;
    };
    let (exp_proj, _) = crate::config::teams::split_project_prefix(exp);
    if exp_proj.is_none() {
        return false;
    }
    let (msg_proj, _) = crate::config::teams::split_project_prefix(msg_from);
    if msg_proj.is_some() {
        return false;
    }
    *msg_from = exp.to_string();
    true
}

/// Decision made by `deliver_wake` for an existing session.
#[derive(Debug, PartialEq)]
pub(crate) enum WakeAction {
    /// Session is live — inject into stdin, regardless of whether the agent
    /// is waiting for input or mid-turn. Bias toward delivery.
    Inject,
    /// Session is Exited — destroy it and fall through to spawn a fresh one.
    RespawnExited,
}

/// Pure decision given a session's status. Extracted so the decision table is
/// unit-testable without a tauri runtime. `deliver_wake` calls this and acts
/// on the result; any future restoration of a busy-gate would require editing
/// this fn (and its tests below), not a lone `if` inside `deliver_wake`.
pub(crate) fn wake_action_for(status: &SessionStatus) -> WakeAction {
    if matches!(status, SessionStatus::Exited(_)) {
        WakeAction::RespawnExited
    } else {
        WakeAction::Inject
    }
}

/// `skip_auto_resume` value used by `deliver_wake`'s spawn-fallback. Inverts
/// the positive-form `spawn_with_resume` flag so call sites read naturally.
///
/// Pinned via this helper to fence against a future refactor that "simplifies"
/// `!spawn_with_resume` to `spawn_with_resume` and silently regresses #82.
/// See plan §8.2 / round-2 R2.7 / round-3 R3.2.
pub(crate) fn wake_spawn_skip_auto_resume(spawn_with_resume: bool) -> bool {
    !spawn_with_resume
}

/// Decide whether a session is a viable candidate for the mailbox to attempt
/// delivery to. Pure function — unit-testable without a tauri runtime.
///
/// Rules:
/// - `Exited(_)` records are KEPT even with `has_pty == false` — the wake
///   contract documents the respawn path: "if Exited, respawn". The respawn
///   path does NOT need a live PTY (it calls `destroy_session_inner` then
///   `create_session_inner`).
/// - All other statuses (`Active`/`Running`/`Idle`) require `has_pty == true`.
///   A SessionManager record with one of these statuses but no PtyManager
///   entry is a desync phantom — the inject path is guaranteed to fail with
///   `AppError::SessionNotFound`, so the router must skip it (issue #223).
///
/// Exhaustive `match` (not `matches!`) — a future `SessionStatus` variant
/// forces a deliberate compile-error decision rather than silently routing
/// to the `has_pty` branch. (dev-rust R1.B7 / grinch G.M6.)
pub(crate) fn is_viable_wake_candidate(status: &SessionStatus, has_pty: bool) -> bool {
    match status {
        SessionStatus::Exited(_) => true,
        SessionStatus::Active | SessionStatus::Running | SessionStatus::Idle => has_pty,
    }
}

/// Substring sniff for the `AppError::SessionNotFound` formatted message that
/// bubbles up through `pty::inject::inject_text_into_session` as a `String`.
///
/// Tight coupling: the error string format is `"Session not found: {uuid}"`
/// (see `errors.rs:5`). `inject_text_into_session` wraps it as `"PTY write
/// failed: Session not found: {uuid}"`. The substring `"Session not found:"`
/// covers both wrappings. Pinned by the
/// `err_is_pty_session_missing_matches_actual_apperror_format` test below.
///
/// If a future PR introduces typed-error plumbing through `inject_into_pty`,
/// replace this sniff with a typed match.
pub(crate) fn err_is_pty_session_missing(e: &str) -> bool {
    e.contains("Session not found:")
}

/// §224 D.3 — pure filter: session infos by exact-FQN match on
/// `working_directory`. Extracted from `find_all_sessions` so the predicate
/// can be unit-tested without a live `SessionManager` / `AppHandle`.
/// Defensive regression guard: no future change can accidentally re-introduce
/// a `was_active` / `waiting_for_input` / `status` gate without breaking
/// these tests.
pub(crate) fn filter_sessions_by_fqn<'a>(
    sessions: &'a [crate::session::session::SessionInfo],
    target: &str,
) -> Vec<&'a crate::session::session::SessionInfo> {
    sessions
        .iter()
        .filter(|s| crate::config::teams::agent_fqn_from_path(&s.working_directory) == target)
        .collect()
}

/// §224 A.2.5 — outcome of the daemon-restart race guard wait loop.
///
/// The production caller (`handle_close_session`) inlines the wait loop
/// because the natural probe closure captures `&self`, `app`, and `target`
/// across an `.await`, which is awkward to pass through this helper's
/// `FnMut() -> Future` shape without boxing. The helper is retained as the
/// canonical executable specification of the wait semantics for D.5a unit
/// tests — any future divergence between the inline loop and this helper is
/// a regression.
///
/// §224 G-IMPL-5 (NIT, accepted) — LOAD-BEARING COMMENT. The inlined wait
/// loop in `handle_close_session` has no direct unit test on Windows
/// (D.5b is `#[ignore]`'d pending the cross-process FS enumeration
/// investigation). Equivalence between the two implementations is
/// enforced ONLY by this comment + the helper's D.5a unit tests. Do NOT
/// change the inlined loop's semantics without updating this helper to
/// match, and vice versa.
#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub(crate) enum RestoreWaitOutcome {
    /// Deadline elapsed with the restore flag still set; caller should report
    /// `status="restore_in_progress"` to the user.
    StillInProgress,
    /// Flag cleared with the probe still returning empty — there is no live
    /// session for this FQN. Caller falls through to the `no_match` path.
    NoMatch,
}

/// §224 A.2.5 — poll `probe` for a non-empty result, OR for `flag` to clear,
/// until `deadline`. Pure async helper extracted so the race-guard logic can
/// be unit-tested without a live `SessionManager` / `AppHandle`.
///
/// Returns:
///   * `Ok(non_empty_session_ids)` — a session appeared during the wait.
///   * `Err(StillInProgress)` — deadline elapsed, flag still set.
///   * `Err(NoMatch)` — flag cleared with empty result throughout (final
///     probe also empty).
#[allow(dead_code)]
pub(crate) async fn wait_for_restore_or_session<F, Fut>(
    flag: &std::sync::atomic::AtomicBool,
    mut probe: F,
    deadline: std::time::Instant,
    poll: std::time::Duration,
) -> Result<Vec<Uuid>, RestoreWaitOutcome>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Vec<Uuid>>,
{
    use std::sync::atomic::Ordering;
    loop {
        if std::time::Instant::now() >= deadline {
            return if flag.load(Ordering::SeqCst) {
                Err(RestoreWaitOutcome::StillInProgress)
            } else {
                Err(RestoreWaitOutcome::NoMatch)
            };
        }
        tokio::time::sleep(poll).await;
        let ids = probe().await;
        if !ids.is_empty() {
            return Ok(ids);
        }
        if !flag.load(Ordering::SeqCst) {
            // Flag cleared. One final probe to catch a last-tick insertion
            // by the restore task before its guard dropped.
            let ids = probe().await;
            return if ids.is_empty() {
                Err(RestoreWaitOutcome::NoMatch)
            } else {
                Ok(ids)
            };
        }
    }
}

/// The MailboxPoller runs as a background tokio task. It polls outbox directories
/// for all known agent repos, validates messages, and delivers them according to mode.
pub struct MailboxPoller {
    poll_interval: std::time::Duration,
    retry_tracker: HashMap<PathBuf, RetryState>,
}

impl Default for MailboxPoller {
    fn default() -> Self {
        Self::new()
    }
}

impl MailboxPoller {
    pub fn new() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(3),
            retry_tracker: HashMap::new(),
        }
    }

    /// Start the poller as a background task.
    pub fn start(mut self, app: tauri::AppHandle, shutdown: crate::shutdown::ShutdownSignal) {
        tauri::async_runtime::spawn(async move {
            // Initial poll without delay (matches original behavior)
            if let Err(e) = self.poll(&app).await {
                log::warn!("MailboxPoller error: {}", e);
            }
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.token().cancelled() => {
                        log::info!("[MailboxPoller] Shutdown signal received, stopping");
                        break;
                    }
                    _ = tokio::time::sleep(self.poll_interval) => {
                        if let Err(e) = self.poll(&app).await {
                            log::warn!("MailboxPoller error: {}", e);
                        }
                    }
                }
            }
        });
    }

    /// One poll cycle: scan all repo outbox dirs, process each message.
    async fn poll(&mut self, app: &tauri::AppHandle) -> Result<(), String> {
        let settings = app.state::<SettingsState>();
        let repo_paths = {
            let cfg = settings.read().await;
            cfg.project_paths.clone()
        };

        // Also scan CWDs of active sessions for repos not in settings
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let session_dirs = {
            let mgr = session_mgr.read().await;
            mgr.get_sessions_working_dirs().await
        };

        let mut all_paths: Vec<String> = repo_paths;
        for (_, dir) in &session_dirs {
            if !all_paths.contains(dir) {
                all_paths.push(dir.clone());
            }
        }

        // Include the instance-private app outbox
        let app_outbox = app.state::<AppOutbox>();
        let app_outbox_path = app_outbox.path().to_string();

        // Collect all outbox directories to scan
        let mut outbox_dirs: Vec<PathBuf> = all_paths
            .iter()
            .map(|p| {
                Path::new(p)
                    .join(crate::config::agent_local_dir_name())
                    .join("outbox")
            })
            .collect();
        outbox_dirs.push(PathBuf::from(&app_outbox_path));

        for outbox_dir in &outbox_dirs {
            if !outbox_dir.is_dir() {
                continue;
            }

            let entries: Vec<PathBuf> = match std::fs::read_dir(outbox_dir) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                    .filter(|p| {
                        // Skip files in subdirectories (delivered/, rejected/)
                        p.parent() == Some(outbox_dir.as_path())
                    })
                    .collect(),
                Err(_) => continue,
            };

            let is_app_outbox = outbox_dir.as_path() == Path::new(&app_outbox_path);
            for path in entries {
                match self.process_message(app, &path, is_app_outbox).await {
                    Ok(()) => {
                        self.retry_tracker.remove(&path);
                    }
                    Err(e) => {
                        let is_permanent = e.contains(ERR_UNRESOLVABLE_AGENT);
                        let should_reject = is_permanent || {
                            let state =
                                self.retry_tracker
                                    .entry(path.clone())
                                    .or_insert(RetryState {
                                        attempt_count: 0,
                                        logged: false,
                                    });
                            state.attempt_count += 1;

                            if !state.logged {
                                log::warn!(
                                    "Failed to process outbox message {:?} (attempt {}): {}",
                                    path,
                                    state.attempt_count,
                                    e
                                );
                                state.logged = true;
                            } else {
                                log::debug!(
                                    "Retry {} for outbox message {:?}: {}",
                                    state.attempt_count,
                                    path,
                                    e
                                );
                            }

                            state.attempt_count >= MAX_DELIVERY_ATTEMPTS
                        };

                        if should_reject {
                            let reason = if is_permanent {
                                e.clone()
                            } else {
                                let attempts = self
                                    .retry_tracker
                                    .get(&path)
                                    .map(|s| s.attempt_count)
                                    .unwrap_or(0);
                                format!(
                                    "Undeliverable after {} attempts. Last error: {}",
                                    attempts, e
                                )
                            };

                            // §130-stuck-file: on read failure (e.g. non-UTF-8 non-BOM file),
                            // fall back to `reject_raw_file` so the file is moved to `rejected/`
                            // instead of looping forever with `attempt_count >= MAX`.
                            let rejected = match read_text_bom_tolerant(&path) {
                                Ok(content) => {
                                    if let Ok(msg) = serde_json::from_str::<OutboxMessage>(&content)
                                    {
                                        self.reject_message(&path, &msg, &reason).await.is_ok()
                                    } else {
                                        Self::reject_raw_file(&path, &reason).is_ok()
                                    }
                                }
                                Err(_) => Self::reject_raw_file(&path, &reason).is_ok(),
                            };

                            if rejected {
                                self.retry_tracker.remove(&path);
                            } else {
                                log::error!(
                                    "Failed to reject outbox message {:?} — will retry",
                                    path
                                );
                            }
                        }
                    }
                }
            }
        }

        // Prune tracker entries for files that no longer exist
        self.retry_tracker.retain(|path, _| path.exists());

        // Poll session-requests directory (from create-agent CLI)
        self.poll_session_requests(app).await;

        Ok(())
    }

    /// Process a single outbox message file.
    /// `is_app_outbox`: true if the message came from the instance-private outbox (master token path).
    async fn process_message(
        &self,
        app: &tauri::AppHandle,
        path: &Path,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let content = read_text_bom_tolerant(path)
            .map_err(|e| format!("Failed to read outbox file: {}", e))?;

        // `let mut msg`: §AR2-norm below mutates `msg.from` / `msg.to` in place
        // as the SINGLE POINT OF TRUTH for canonicalization. Downstream code
        // (routing, action dispatch, injection, archival) reads the canonical
        // form without re-mutation.
        let mut msg: OutboxMessage = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse outbox message: {}", e))?;

        log::info!(
            "[mailbox] Processing message {} from='{}' to='{}' mode='{}'",
            msg.id,
            msg.from,
            msg.to,
            msg.mode
        );

        // For repo outboxes (not app-outbox), validate that msg.from matches the outbox owner.
        // This prevents tokenless spoofing: a message in repo X's outbox must claim to be from repo X.
        //
        // §DR5 lenient fallback: if `msg.from` is unqualified (legacy) and its LOCAL
        // part matches `expected_from`'s local part, accept — §AR2-norm below then
        // upgrades `msg.from` to the canonical FQN. Cross-project qualified names
        // are rejected (different `project:` prefix).
        //
        // `expected_from` is hoisted (§DR2-3) so §AR2-norm can upgrade `msg.from`
        // even when this block has returned to outer scope.
        let mut expected_from: Option<String> = None;
        if !is_app_outbox {
            let outbox_dir = path.parent().unwrap_or(Path::new(""));
            // outbox_dir is <repo>/.agentscommander/outbox — go up 2 levels to get the repo path
            if let Some(repo_path) = outbox_dir.parent().and_then(|p| p.parent()) {
                let derived =
                    crate::config::teams::agent_fqn_from_path(&repo_path.to_string_lossy());
                if !anti_spoof_accept(&msg.from, &derived) {
                    return self
                        .reject_message(
                            path,
                            &msg,
                            &format!(
                                "Outbox-sender mismatch: outbox belongs to '{}' but message claims '{}'",
                                derived, msg.from
                            ),
                        )
                        .await;
                }
                expected_from = Some(derived);
            }
        }

        // ── §AR2-norm — SINGLE POINT OF TRUTH: msg.from / msg.to canonicalization ──
        //
        // Runs AFTER anti-spoof and BEFORE token validation / routing / action dispatch.
        // Every downstream read of msg.from / msg.to sees the canonical FQN (or bare
        // legacy form when canonicalization wasn't possible). Downstream code MUST
        // NOT re-mutate msg.from or msg.to.

        // (1) Upgrade a legacy-unqualified msg.from to the anti-spoof-derived
        // expected_from FQN (closes grinch §G5: resolve_repo_path(&msg.from)
        // for response-dir lookup now sees a canonical input).
        let original_from_for_log = msg.from.clone();
        if canonicalize_msg_from_in_place(&mut msg.from, expected_from.as_deref()) {
            log::info!(
                "[mailbox] canonicalized legacy msg.from '{}' → '{}'",
                original_from_for_log,
                msg.from
            );
        }

        // (2) Canonicalize msg.to via the shared resolver. Empty `to` is allowed for
        // action-dispatch paths (e.g. close-session may set an empty to); skip in
        // that case. Reject-on-ambiguity semantics match the CLI (Decision 2 rule 2c).
        //
        // §AR2-norm D1-a augmentation: when the outbox file lives under a WG-replica
        // layout (`<project>/.ac-new/wg-*/__agent_*/<local-dir>/outbox/<file>.json`),
        // include the derived `<project>` in the in-memory `paths` slice so qualified
        // WG-peer FQNs written by `cli::send` (which performs the symmetric walk-up
        // augmentation — see #228 Step 1) resolve here too. Without this, the daemon
        // re-rejects with `UnknownQualified` even though the CLI side succeeded.
        // settings.json is NOT mutated. In-memory, this-message only. See #228 D1-a.
        if !msg.to.is_empty() {
            let mut paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            if let Some(root_project) = derive_project_from_outbox_path(path) {
                let canon_root_project = std::fs::canonicalize(&root_project).ok();
                let already_present = paths.iter().any(|p| match &canon_root_project {
                    Some(canon_target) => {
                        std::fs::canonicalize(p).ok().as_ref() == Some(canon_target)
                    }
                    None => p == &root_project,
                });
                if !already_present {
                    paths.push(root_project);
                }
            }
            match crate::config::teams::resolve_agent_target(&msg.to, &paths) {
                Ok(fqn) => {
                    if fqn != msg.to {
                        log::info!("[mailbox] canonicalized msg.to '{}' → '{}'", msg.to, fqn);
                        msg.to = fqn;
                    }
                }
                Err(e) => {
                    return self
                        .reject_message(path, &msg, &format!("Unresolvable target: {}", e))
                        .await;
                }
            }
        }

        // Check if token is the master token or root token (bypasses anti-spoofing + team validation)
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

        if is_master {
            log::info!(
                "[mailbox] Master token used — bypassing team validation for {} → {}",
                msg.from,
                msg.to
            );
        } else {
            // Validate session token if present (anti-spoofing)
            if let Some(ref token_str) = msg.token {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;

                if let Ok(token_uuid) = Uuid::parse_str(token_str) {
                    match mgr.find_by_token(token_uuid).await {
                        None => {
                            // Token is stale/invalid. Env-only credentials cannot be refreshed into
                            // an already-running child process, so reject instead of injecting a new token.
                            drop(mgr);
                            if let Some(session_id) = self.find_active_session(app, &msg.from).await
                            {
                                log::warn!(
                                    "[mailbox] Stale token from '{}' matches active session {}, but env-only credentials cannot be refreshed in-place",
                                    msg.from,
                                    session_id
                                );
                            }
                            return self
                                .reject_message(
                                    path,
                                    &msg,
                                    "Invalid session token. Env-only credentials cannot be refreshed into a live process; restart or respawn the sender session.",
                                )
                                .await;
                        }
                        Some(session) => {
                            // Anti-spoofing: verify msg.from matches the token's session CWD.
                            // Post-§AR2-norm, msg.from is canonical FQN (or legacy unqualified
                            // if expected_from was unavailable). Session-derived name uses the
                            // canonical helper; comparison is exact equality.
                            let session_name = crate::config::teams::agent_fqn_from_path(
                                &session.working_directory,
                            );
                            if session_name != msg.from {
                                log::warn!(
                                    "[mailbox] Token-root mismatch: token session='{}' but from='{}'",
                                    session_name, msg.from
                                );
                                return self.reject_message(
                                    path,
                                    &msg,
                                    &format!(
                                        "Token-root mismatch: session is '{}' but message claims '{}'",
                                        session_name, msg.from
                                    ),
                                )
                                .await;
                            }
                        }
                    }
                } else {
                    // Token is not a valid UUID. Env-only credentials cannot be refreshed
                    // into an already-running child process, so reject instead of injecting.
                    drop(mgr);
                    if let Some(session_id) = self.find_active_session(app, &msg.from).await {
                        log::warn!(
                            "[mailbox] Malformed token from '{}' matches active session {}, but env-only credentials cannot be refreshed in-place",
                            msg.from,
                            session_id
                        );
                    }
                    return self
                        .reject_message(
                            path,
                            &msg,
                            "Malformed session token. Env-only credentials cannot be refreshed into a live process; restart or respawn the sender session.",
                        )
                        .await;
                }
            }

            // Validate peer visibility (team membership) — skipped for master token
            let discovered_teams = teams::discover_teams();
            if !self.can_reach(&msg.from, &msg.to, &discovered_teams) {
                log::warn!(
                    "[mailbox] Routing check FAILED: '{}' cannot reach '{}'",
                    msg.from,
                    msg.to
                );
                return self
                    .reject_message(path, &msg, "Sender cannot reach destination")
                    .await;
            }
            log::info!(
                "[mailbox] Routing check passed: '{}' → '{}'",
                msg.from,
                msg.to
            );
        }

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
                    return self.handle_close_session(app, path, &msg, is_app_outbox).await;
                }
                _ => {
                    return self
                        .reject_message(path, &msg, &format!("Unknown action '{}'", action))
                        .await;
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
            return self
                .reject_message(
                    path,
                    &msg,
                    &format!("Unsupported delivery mode '{}'. Valid: wake", mode),
                )
                .await;
        }
        self.deliver_wake(app, &msg).await?;

        // Move to delivered/ with token stripped
        self.move_to_delivered(path, &msg).await
    }

    /// Deliver mode: wake — inject into the recipient's PTY for any non-Exited
    /// session; destroy and respawn if Exited; spawn persistent if none. Always
    /// delivers (no busy-gate — stdin buffer absorbs input while the agent is
    /// mid-turn).
    async fn deliver_wake(
        &self,
        app: &tauri::AppHandle,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        // Whether the spawn-fallback should allow provider auto-resume.
        // Default false: cold wake — no SessionManager record at this CWD.
        // Promoted to true in two paths below: (a) RespawnExited deferred-
        // destroy block, (b) phantoms-only fall-through (every CWD match
        // was a desync phantom — preserve any on-disk transcript via
        // auto-resume). See issue #223 round-1 resolution.
        //
        // MUST NOT be re-derived after `destroy_session_inner` runs: post-
        // destroy, the candidate would vanish from list_sessions and the
        // value would silently flip, regressing the deferred-non-coord
        // wake by losing `--continue`. Set the flag inside the candidate
        // loop only. See plan §4.5.a / round-1 G7 / round-3 R3.2.
        let mut spawn_with_resume = false;
        let mut pending_exited_destroy: Option<Uuid> = None;
        // HIGH-1 (Step-7 review): symmetric AC5 protection. Tracks whether
        // every iter'd Inject candidate hit the `err_is_pty_session_missing`
        // race arm — if so, the post-loop fall-through must still promote
        // spawn_with_resume so the on-disk transcript isn't abandoned. Same
        // outcome as grinch G.H2's phantoms-only path, just a different
        // upstream cause (race-killed siblings vs. desync phantoms).
        let mut lost_inject_to_race = false;

        // Issue #223: enumerate ALL viable CWD candidates (PTY-liveness filtered)
        // with their captured status, plus a had_any_match flag for the phantoms-
        // only AC5-preservation path (grinch G.H2). Iterate so a stale-Running
        // phantom no longer blocks delivery to the live Idle recipient at the
        // same CWD; defer Exited destroys until later Inject attempts fail
        // (grinch G.H1) — preserves AC5's "first Exited wins respawn slot".
        let (candidates, had_any_match) = self.find_live_candidates(app, &msg.to).await;

        for &(session_id, ref status) in &candidates {
            log::info!(
                "[mailbox] wake: candidate {} captured-status={:?}",
                session_id,
                status
            );

            match wake_action_for(status) {
                WakeAction::Inject => {
                    match self.inject_into_pty(app, session_id, msg, true).await {
                        Ok(()) => return Ok(()),
                        Err(e) if err_is_pty_session_missing(&e) => {
                            // Race: PTY died between `find_live_candidates`
                            // probe and `PtyManager::write`. Load-bearing
                            // safety net for the dropped per-iteration
                            // re-read (grinch G.M2). Try the next candidate.
                            // Flag the race for the post-loop AC5 promotion
                            // (grinch HIGH-1) — if every Inject candidate
                            // races to dead, the spawn-persistent fall-through
                            // must still set spawn_with_resume.
                            lost_inject_to_race = true;
                            log::warn!(
                                "[mailbox] wake: candidate {} died after liveness probe ({}), trying next",
                                session_id,
                                e
                            );
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                }
                WakeAction::RespawnExited => {
                    // Defer the destroy: an Inject candidate later in the list
                    // may succeed and avoid destroy+spawn entirely (grinch
                    // G.H1). Only the FIRST Exited's id is remembered —
                    // preserves AC5's "first Exited wins the respawn slot"
                    // semantic.
                    if pending_exited_destroy.is_none() {
                        pending_exited_destroy = Some(session_id);
                        spawn_with_resume = true;
                        log::info!(
                            "[mailbox] wake: deferring Exited destroy for {} (status={:?}) pending later Inject success",
                            session_id,
                            status
                        );
                    }
                    continue;
                }
            }
        }

        // No Inject returned Ok. Three cases enable auto-resume on the spawn-
        // persistent fall-through:
        //   1. A deferred Exited destroy is pending (spawn_with_resume true).
        //   2. Candidate list empty BUT records existed in the manager — i.e.,
        //      every CWD-match was a non-Exited phantom. Spawning cold would
        //      abandon any on-disk transcript at the matched CWD (grinch G.H2
        //      AC5 micro-regression).
        //   3. Every viable Inject candidate died mid-flight (all hit the
        //      `err_is_pty_session_missing` race arm). Symmetric to case 2 —
        //      same AC5 outcome since cold spawn would abandon the transcript
        //      (grinch HIGH-1, Step-7 review).
        if pending_exited_destroy.is_none()
            && (candidates.is_empty() && had_any_match || lost_inject_to_race)
        {
            spawn_with_resume = true;
            log::info!(
                "[mailbox] wake: all CWD candidates for '{}' are phantoms or lost to race; enabling auto-resume on spawn-persistent",
                msg.to
            );
        }

        if let Some(exited_id) = pending_exited_destroy {
            log::info!(
                "[mailbox] wake: executing deferred destroy for Exited candidate {}",
                exited_id
            );
            if let Err(e) = crate::commands::session::destroy_session_inner(app, exited_id).await {
                // Best-effort. Orphan SessionManager record lingers until AC3's
                // runtime-dedup path (`#223-fu1`) drains it. spawn-persistent
                // below still fires with the correct spawn_with_resume flag.
                // (grinch G.M5 option (c).)
                log::error!(
                    "[mailbox] wake: failed to destroy exited session {} (orphan will linger until AC3): {}",
                    exited_id,
                    e
                );
            }
        }

        // ── No viable Inject candidate succeeded — spawn a persistent one ──
        log::info!(
            "[mailbox] wake: no active session for '{}', spawning persistent session",
            msg.to
        );

        let agent_command = self.resolve_agent_command(app, msg).await;
        let (shell, shell_args) = agent_command.ok_or_else(|| {
            format!("No agent command resolved for '{}' — cannot spawn session. Configure lastCodingAgent or agents in settings.", msg.to)
        })?;

        let dest_path = self.resolve_repo_path(&msg.to, app).await;
        let cwd = match dest_path {
            Some(path) => path,
            None => {
                // Fallback: for WG agents (wg-name/agent), derive path from sibling session CWDs
                self.resolve_wg_path_from_sessions(app, &msg.to)
                    .await
                    .ok_or_else(|| {
                        format!(
                            "Cannot resolve repo path for '{}' — cannot spawn session",
                            msg.to
                        )
                    })?
            }
        };

        // Resolve agent_id and label from lastCodingAgent config
        let (agent_id, agent_label) = self.resolve_agent_id_and_label(app, &cwd).await;

        // §AR2-session-name: strip optional `<project>:` prefix from the display
        // name so the sidebar label stays short (e.g. "wg-1-devs/tech-lead" not
        // "proj-a:wg-1-devs/tech-lead"). The canonical FQN stays recoverable via
        // `agent_fqn_from_path(&cwd)` at any list-sessions time.
        let session_name = {
            let (_, local) = crate::config::teams::split_project_prefix(&msg.to);
            local.to_string()
        };

        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

        let info = crate::commands::session::create_session_inner(
            app,
            session_mgr.inner(),
            pty_mgr.inner(),
            shell,
            shell_args,
            cwd,
            Some(session_name), // readable name, no [temp] prefix
            agent_id,           // links to agent config
            agent_label,        // human-readable label
            false,              // skip_tooling_save = false → persist lastCodingAgent
            Vec::new(),         // git_repos
            wake_spawn_skip_auto_resume(spawn_with_resume), // see deliver_wake top
        )
        .await
        .map_err(|e| format!("Failed to spawn session for '{}': {}", msg.to, e))?;

        let session_id =
            Uuid::parse_str(&info.id).map_err(|e| format!("Failed to parse session id: {}", e))?;

        // Wait for agent to boot and become idle (ready for input)
        let max_wait = std::time::Duration::from_secs(90);
        let poll = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() >= max_wait {
                log::warn!(
                    "[mailbox] wake: timeout waiting for session {} to become idle",
                    session_id
                );
                break; // inject anyway as fallback
            }
            tokio::time::sleep(poll).await;

            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == session_id.to_string()) {
                Some(s) if s.waiting_for_input => {
                    log::info!(
                        "[mailbox] wake: session {} is idle, injecting message",
                        session_id
                    );
                    drop(mgr);
                    break;
                }
                Some(_) => {} // still booting
                None => {
                    return Err(format!(
                        "Session {} was destroyed before message injection",
                        session_id
                    ));
                }
            }
            drop(mgr);
        }

        // Inject message — interactive mode (session persists, user sees reply instructions)
        self.inject_into_pty(app, session_id, msg, true).await
    }

    /// Inject a message into a session's PTY stdin.
    /// `interactive` = true (all remaining callers): live interactive `wake`
    /// delivery — plain message only, no response markers, no watcher.
    /// `interactive` = false is currently unreachable (the former
    /// `wake-and-sleep` non-interactive path was removed in 0.7.0). The
    /// `use_markers=true` branch below is retained for future non-interactive
    /// consumers; see _plans/delete-modes.md §2.4.
    async fn inject_into_pty(
        &self,
        app: &tauri::AppHandle,
        session_id: Uuid,
        msg: &OutboxMessage,
        interactive: bool,
    ) -> Result<(), String> {
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

        // ── Remote command path: delegate to the canonical injector ──
        // `/clear` and `/compact` are submitted to the agent's PTY exactly the
        // same way as a normal message body: through `inject_text_into_session`,
        // which owns shell detection and the agent-specific double-Enter safety
        // net (see pty/inject.rs). The injector — not this branch — appends the
        // Enter(s); we pass just the `/command` text.
        if let Some(ref command) = msg.command {
            const ALLOWED_COMMANDS: &[&str] = &["clear", "compact"];
            if !ALLOWED_COMMANDS.contains(&command.as_str()) {
                return Err(format!("Unsupported remote command '{}'", command));
            }

            // Precondition: agent must be idle (waiting_for_input) AND the
            // shell must be a coding-agent CLI that owns explicit-Enter
            // handling in the canonical injector. The shell check guards
            // against silent failure on non-agent shells (plain bash/pwsh)
            // and on cmd-wrapped Codex sessions: under R1 the injector
            // sends ZERO carriage returns when `needs_explicit_enter` is
            // false, so writing `/clear` into such a shell would leave the
            // text un-submitted and let subsequent user input concatenate
            // with it. Reject explicitly instead — closes grinch's G1 + G3.
            // Removing this reject requires extending `needs_explicit_enter`
            // to recognize the rejected case as agent-aware first; see
            // `#233-followup-cmd-wrapper`.
            {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    None => {
                        return Err(format!(
                            "Session {} not found — cannot execute remote command '{}'",
                            session_id, command
                        ))
                    }
                    Some(s) if !crate::pty::inject::needs_explicit_enter(&s.shell) => {
                        return Err(format!(
                            "Cannot execute remote command '/{}': session shell '{}' is not a coding-agent CLI (Claude / Codex / Gemini). cmd / pwsh wrappers around an agent are tracked separately as #233-followup-cmd-wrapper.",
                            command, s.shell
                        ))
                    }
                    Some(s) if !s.waiting_for_input => {
                        return Err(format!(
                            "Cannot execute remote command '{}': agent is busy (not idle)",
                            command
                        ))
                    }
                    _ => {} // idle agent-shell — proceed
                }
            }

            // Submit `/<command>` via the canonical text-block injector.
            //
            // TOCTOU note (round-2 correction): the gap between the idle check
            // above and the FINAL `\r` is now ~2 s (text → 1500 ms → \r →
            // 500 ms → \r inside `inject_text_into_session`), not microseconds
            // as the original direct-write path. Two things can interleave
            // inside that window:
            //   1. User keystrokes from xterm.js — they take the path
            //      `frontend → invoke("pty_write") → commands/pty.rs::pty_write
            //      → PtyManager::write` (raw bytes, bypasses the injector). A
            //      user typing between the staggered Enters concatenates with
            //      the un-Enter'd `/<command>`.
            //   2. The idle detector flipping `waiting_for_input` to false
            //      based on independent PTY output, leaving the second `\r`
            //      to land on a busy agent (harmless — at worst an extra
            //      Enter on empty input, per pty/inject.rs:78-79).
            // We accept this race because the standard-message path at
            // mailbox.rs:972 already exhibits it identically and the
            // staggered double-Enter is the empirically-tuned defense
            // against the dominant single-`\r`-eaten failure mode that
            // motivated #233.
            let cmd_text = format!("/{}", command); // NO trailing \r — the injector adds it
            crate::pty::inject::inject_text_into_session(app, session_id, &cmd_text)
                .await
                .map_err(|e| {
                    log::error!(
                        "[mailbox] PTY injection FAILED for command '/{}' session={} msg={}: {}",
                        command,
                        session_id,
                        msg.id,
                        e
                    );
                    e
                })?;

            log::info!(
                "Executed remote command '{}' on session {} (from: {})",
                command,
                session_id,
                msg.from
            );

            let _ = tauri::Emitter::emit(
                app,
                "message_delivered",
                serde_json::json!({
                    "id": msg.id,
                    "from": msg.from,
                    "to": msg.to,
                    "mode": msg.mode,
                    "command": command,
                    "injected": true
                }),
            );

            // Post-command background work:
            //  - `/clear` and `/compact` both keep the still-live child process environment.
            //  - Credentials are env-only; nothing is re-sent through the PTY here.
            //  - If the message has a follow-up body, inject it after the agent becomes idle.
            // Never block the delivery pipeline — spawn as a detached task.
            let app_clone = app.clone();
            let msg_clone = msg.clone();
            let command_owned = command.clone();
            tauri::async_runtime::spawn(async move {
                if !msg_clone.body.is_empty() {
                    if let Err(e) =
                        Self::inject_followup_after_idle_static(&app_clone, session_id, &msg_clone)
                            .await
                    {
                        log::warn!(
                            "[mailbox] Failed to inject follow-up after /{} for session {}: {}",
                            command_owned,
                            session_id,
                            e
                        );
                    }
                }
            });

            return Ok(());
        }

        // ── Standard message path ──
        // Only use response markers for non-interactive sessions
        let use_markers = msg.get_output && !interactive;

        // Interactive and marker-less paths share the minimal PTY wrap via
        // `format_pty_wrap` (single source with `PTY_WRAP_FIXED` used by the
        // CLI clamp). Only the `--get-output` + `request_id` case wraps the
        // payload with response markers.
        let payload = match (use_markers, msg.request_id.as_ref()) {
            (true, Some(rid)) => format!(
                "\n[Message from {}] {}\n(Reply between markers: %%AC_RESPONSE::{}::START%% ... %%AC_RESPONSE::{}::END%%)\n\r",
                msg.from, msg.body, rid, rid
            ),
            _ => crate::phone::messaging::format_pty_wrap(&msg.from, &msg.body),
        };

        // Register response watcher only for non-interactive sessions
        if use_markers {
            if let Some(ref rid) = msg.request_id {
                // Response file goes to the SENDER's .agentscommander/responses/
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let response_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let mgr = pty_mgr
                        .lock()
                        .map_err(|e| format!("PTY lock failed: {}", e))?;
                    mgr.register_response_watcher(session_id, rid.clone(), response_dir);
                    drop(mgr);
                }
            }
        }

        // SECURITY: this `first_100` log MUST NOT see credential values.
        // Credentials are env-only and must never be routed through PTY payloads.
        // Keep this log limited to standard message payloads.
        log::debug!(
            "[mailbox] Injecting into PTY session={} msg={} payload_len={} first_100={:?}",
            session_id,
            msg.id,
            payload.len(),
            payload.chars().take(100).collect::<String>()
        );
        crate::pty::inject::inject_text_into_session(app, session_id, &payload)
            .await
            .map_err(|e| {
                log::error!(
                    "[mailbox] PTY injection FAILED session={} msg={}: {}",
                    session_id,
                    msg.id,
                    e
                );
                e
            })?;

        log::info!(
            "[mailbox] PTY injection SUCCESS session={} msg={}",
            session_id,
            msg.id
        );
        let _ = tauri::Emitter::emit(
            app,
            "message_delivered",
            serde_json::json!({
                "id": msg.id,
                "from": msg.from,
                "to": msg.to,
                "mode": msg.mode,
                "injected": true
            }),
        );
        Ok(())
    }

    /// Wait for agent to become idle after a remote command, then inject body as follow-up.
    /// Static method — can be spawned as a detached task without borrowing self.
    async fn inject_followup_after_idle_static(
        app: &tauri::AppHandle,
        session_id: Uuid,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        let max_wait = std::time::Duration::from_secs(30);
        let poll = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();

        // Wait for idle (waiting_for_input = true)
        loop {
            if start.elapsed() >= max_wait {
                return Err(format!(
                    "Timeout waiting for agent to become idle after remote command ({}s)",
                    max_wait.as_secs()
                ));
            }
            tokio::time::sleep(poll).await;

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == session_id.to_string()) {
                Some(s) if s.waiting_for_input => break,
                Some(_) => {} // busy — keep polling
                None => {
                    return Err(format!(
                        "Session {} destroyed before follow-up could be injected",
                        session_id
                    ))
                }
            }
        }

        // Inject the follow-up body as a standard interactive message.
        // Note: same TOCTOU race as the command path — agent could become busy
        // between the idle check above and this write. Acceptable for this use case.
        let payload = crate::phone::messaging::format_pty_wrap(&msg.from, &msg.body);
        crate::pty::inject::inject_text_into_session(app, session_id, &payload).await
    }

    /// Find the best session for a given agent name (matches by working directory).
    /// Prefers active/running non-temp sessions over idle/exited ones.
    ///
    /// Used ONLY by stale-token logging (mailbox.rs:456, 501). Routing now uses
    /// `find_live_candidates` — do not add new callers. (grinch G.L5.)
    async fn find_active_session(&self, app: &tauri::AppHandle, agent_name: &str) -> Option<Uuid> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        log::info!(
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
        let mut matches: Vec<&crate::session::session::SessionInfo> = sessions
            .iter()
            .filter(|s| {
                crate::config::teams::agent_fqn_from_path(&s.working_directory) == agent_name
            })
            .collect();

        if matches.is_empty() {
            log::warn!("[mailbox] No session matched for '{}'", agent_name);
            return None;
        }

        log::info!(
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
        log::info!(
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
    ///   preserves on-disk Claude/Codex/Gemini transcript). (grinch G.H2.)
    async fn find_live_candidates(
        &self,
        app: &tauri::AppHandle,
        agent_name: &str,
    ) -> (Vec<(Uuid, SessionStatus)>, bool /* had_any_match */) {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        let mut matches: Vec<&crate::session::session::SessionInfo> = sessions
            .iter()
            .filter(|s| {
                crate::config::teams::agent_fqn_from_path(&s.working_directory) == agent_name
            })
            .collect();

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

        // Take the PtyManager lock ONCE and probe each candidate.
        let pty = pty_mgr.lock().unwrap();
        let viable: Vec<(Uuid, SessionStatus)> = matches
            .iter()
            .filter_map(|s| Uuid::parse_str(&s.id).ok().map(|id| (id, *s)))
            .filter_map(|(id, s)| {
                let has_pty = pty.has_session(id);
                let viable = is_viable_wake_candidate(&s.status, has_pty);
                if !viable {
                    // Phantom skip — observable in prod logs to scope the AC3
                    // follow-up. (dev-rust R1.B4.)
                    log::warn!(
                        "[mailbox] skipping desync phantom: id={} status={:?} has_pty={} name='{}'",
                        id,
                        s.status,
                        has_pty,
                        s.name
                    );
                    None
                } else {
                    Some((id, s.status.clone()))
                }
            })
            .collect();
        drop(pty);

        log::info!(
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

    /// Find ALL sessions matching an agent name (by working directory).
    /// Returns all matching session UUIDs, not just the "best" one.
    ///
    /// §AR2-G2: exact-FQN filter (same simplification as `find_active_session`).
    async fn find_all_sessions(&self, app: &tauri::AppHandle, agent_name: &str) -> Vec<Uuid> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        filter_sessions_by_fqn(&sessions, agent_name)
            .into_iter()
            .filter_map(|s| Uuid::parse_str(&s.id).ok())
            .collect()
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
    async fn handle_close_session(
        &self,
        app: &tauri::AppHandle,
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
                            "Not authorized: '{}' is not a coordinator of '{}' team",
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
            if restore_flag
                .0
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let poll = std::time::Duration::from_millis(100);
                // We can't pass `self.find_all_sessions(...)` as a closure
                // because of the `&self` capture + `async` future shape, so
                // inline the wait loop instead of going through the pure helper.
                // The helper's logic is unit-tested separately (D.5a).
                loop {
                    if std::time::Instant::now() >= deadline {
                        if restore_flag
                            .0
                            .load(std::sync::atomic::Ordering::SeqCst)
                        {
                            restore_in_progress_result = true;
                        }
                        break;
                    }
                    tokio::time::sleep(poll).await;
                    session_ids = self.find_all_sessions(app, target).await;
                    if !session_ids.is_empty() {
                        break;
                    }
                    if !restore_flag
                        .0
                        .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        // Flag cleared mid-wait. One final probe (the restore
                        // task may have just inserted our target as its last
                        // act before the guard dropped), then fall through.
                        session_ids = self.find_all_sessions(app, target).await;
                        break;
                    }
                }
            }
        }

        let force = msg.force.unwrap_or(false);
        let timeout_secs = msg.timeout_secs.unwrap_or(30);

        // §224 A.2 — gate the kill-loop log on non-empty session_ids so we
        // don't emit a misleading "force-killing 0 session(s)" line on the
        // no_match / restore_in_progress paths.
        if !session_ids.is_empty() {
            log::info!(
                "[mailbox] close-session: {} {} session(s) for '{}' (requested by '{}', timeout={}s)",
                if force {
                    "force-killing"
                } else {
                    "gracefully closing"
                },
                session_ids.len(),
                target,
                msg.from,
                timeout_secs
            );
        }

        let mut closed_ids: Vec<String> = Vec::new();
        for sid in &session_ids {
            let success = if force {
                self.force_close_session(app, *sid).await
            } else {
                self.graceful_close_session(app, *sid, timeout_secs).await
            };
            if success {
                closed_ids.push(sid.to_string());
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
            // §224 review fix: snapshot under the read guard, drop the guard,
            // THEN write to disk. Avoids holding the outer SessionManager
            // RwLock across the blocking `std::fs::rename` inside
            // `save_sessions`, which would block any writer (destroy_session,
            // create_session, restore-loop spawn) for the duration of the
            // disk I/O.
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let snapshot = {
                let mgr = session_mgr.read().await;
                crate::config::sessions_persistence::snapshot_sessions(&mgr).await
            };
            if let Err(e) = crate::config::sessions_persistence::save_sessions(&snapshot) {
                log::warn!(
                    "[mailbox] close-session: failed to persist cleaned sessions.json after no_match: {}",
                    e
                );
            }
        }

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
            let status = if restore_in_progress_result {
                "restore_in_progress"
            } else if session_ids.is_empty() {
                "no_match"
            } else if closed_ids.is_empty() {
                "already_closed"
            } else {
                "closed"
            };
            let response = serde_json::json!({
                "action": "close-session",
                "target": target,
                "status": status,
                "sessions_closed": closed_ids.len(),
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
                    return self.move_to_delivered(path, msg).await;
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
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let response_path = responses_dir.join(format!("{}.json", rid));
                    if let Err(e) = std::fs::write(&response_path, &json) {
                        log::warn!(
                            "[mailbox] Failed to write close-session response to outbox-relative path {:?}: {}",
                            response_path, e
                        );
                    }
                } else {
                    log::warn!(
                        "[mailbox] close-session: cannot derive outbox-relative responses dir from message path {:?}",
                        path
                    );
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
                        "[mailbox] Failed to write close-session response to resolved-sender path: {}",
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

        // Move original message to delivered/
        self.move_to_delivered(path, msg).await
    }

    /// Force-close a session immediately via destroy_session_inner.
    async fn force_close_session(&self, app: &tauri::AppHandle, sid: Uuid) -> bool {
        match crate::commands::session::destroy_session_inner(app, sid).await {
            Ok(()) => {
                log::info!("[mailbox] close-session: force-destroyed session {}", sid);
                true
            }
            Err(e) => {
                log::warn!(
                    "[mailbox] close-session: failed to force-destroy session {}: {}",
                    sid,
                    e
                );
                false
            }
        }
    }

    /// Gracefully close a session: inject exit command, poll for Exited, fallback to force on timeout.
    async fn graceful_close_session(
        &self,
        app: &tauri::AppHandle,
        sid: Uuid,
        timeout_secs: u32,
    ) -> bool {
        // Get session info to determine agent type
        let exit_cmd = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == sid.to_string()) {
                Some(s) => Self::resolve_exit_command(&s.shell, &s.shell_args),
                None => {
                    log::warn!(
                        "[mailbox] close-session: session {} not found for graceful close",
                        sid
                    );
                    return false;
                }
            }
        };

        log::info!(
            "[mailbox] close-session: injecting '{}' into session {}",
            exit_cmd.escape_debug(),
            sid
        );

        // Inject exit command into PTY.
        // Clone the Arc so the State borrow is released, then lock+write+drop guard before any .await.
        let pty_arc = app
            .state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>()
            .inner()
            .clone();
        let inject_result = match pty_arc.lock() {
            Ok(mgr) => {
                let res = mgr
                    .write(sid, exit_cmd.as_bytes())
                    .map_err(|e| e.to_string());
                drop(mgr);
                res
            }
            Err(e) => Err(format!("PTY lock failed: {}", e)),
        };
        if let Err(e) = inject_result {
            log::warn!(
                "[mailbox] close-session: PTY inject failed for {}: {}, falling back to force",
                sid,
                e
            );
            return self.force_close_session(app, sid).await;
        }

        // Poll for SessionStatus::Exited
        let timeout = std::time::Duration::from_secs(timeout_secs as u64);
        let poll_interval = std::time::Duration::from_secs(1);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() >= timeout {
                log::warn!(
                    "[mailbox] close-session: graceful timeout ({}s) for session {}, falling back to force",
                    timeout_secs, sid
                );
                return self.force_close_session(app, sid).await;
            }

            tokio::time::sleep(poll_interval).await;

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == sid.to_string()) {
                Some(s) if matches!(s.status, SessionStatus::Exited(_)) => {
                    log::info!("[mailbox] close-session: session {} exited gracefully", sid);
                    drop(mgr);
                    // Clean up the exited session
                    return self.force_close_session(app, sid).await;
                }
                None => {
                    // Session already removed
                    log::info!("[mailbox] close-session: session {} already gone", sid);
                    return true;
                }
                _ => {} // still running, keep polling
            }
        }
    }

    /// Determine the exit command to inject based on the session's shell/agent type.
    /// Claude Code -> "/exit\r", generic shell/codex -> "exit\r"
    fn resolve_exit_command(shell: &str, shell_args: &[String]) -> String {
        let full_cmd = format!("{} {}", shell, shell_args.join(" "));
        let basenames: Vec<String> = full_cmd
            .split_whitespace()
            .map(crate::commands::session::executable_basename)
            .collect();

        if basenames.iter().any(|b| b == "claude" || b == "aider") {
            "/exit\r".to_string()
        } else {
            // Codex, generic shell, and other CLIs
            "exit\r".to_string()
        }
    }

    /// Resolve the full filesystem path for an agent name.
    ///
    /// §AR2-G4: collector pattern. For qualified inputs, an FQN matches at most
    /// one CWD/path/team-member entry per iteration (by construction) — the
    /// dedupe is defense-in-depth for redundant registrations. For unqualified
    /// inputs (legacy), local-part matches across multiple projects return
    /// `None` rather than arbitrarily picking one.
    ///
    /// §AR2-G3: WG fallback seed honors the target project filter. Combined
    /// with §DR2-4 composition: `matches.push(...); break;` within a single
    /// `rp` iteration (FQN can only match one replica dir per project) while
    /// the outer loop continues so cross-project ambiguity is still detected.
    async fn resolve_repo_path(&self, agent_name: &str, app: &tauri::AppHandle) -> Option<String> {
        let (target_project, target_local) = crate::config::teams::split_project_prefix(agent_name);
        let is_qualified = target_project.is_some();
        let mut matches: Vec<String> = Vec::new();

        let record_match = |path_str: &str, out: &mut Vec<String>| {
            if !out.iter().any(|m| m == path_str) {
                out.push(path_str.to_string());
            }
        };

        let hits_agent = |cwd: &str| -> bool {
            let path_fqn = crate::config::teams::agent_fqn_from_path(cwd);
            if is_qualified {
                path_fqn == agent_name
            } else {
                let (_, path_local) = crate::config::teams::split_project_prefix(&path_fqn);
                path_local == target_local
            }
        };

        // Loop 1: session CWDs
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_working_dirs().await;
        for (_, cwd) in &dirs {
            if hits_agent(cwd) {
                record_match(cwd, &mut matches);
            }
        }
        drop(mgr);

        // Loop 2: settings project_paths
        let settings = app.state::<SettingsState>();
        let cfg = settings.read().await;
        for rp in &cfg.project_paths {
            if hits_agent(rp) {
                record_match(rp, &mut matches);
            }
        }

        // Loop 3: discovered team member paths. Short-circuit by project when
        // the target is qualified (team.project matches target_project).
        let discovered_teams = teams::discover_teams();
        for team in &discovered_teams {
            if let Some(want) = target_project {
                if team.project != want {
                    continue;
                }
            }
            for agent_path in team.agent_paths.iter().flatten() {
                let path_str = agent_path.to_string_lossy().to_string();
                if hits_agent(&path_str) {
                    record_match(&path_str, &mut matches);
                }
            }
        }

        // Loop 4: WG replica fallback. Scan `.ac-new/<wg>/__agent_<short>` under
        // project_paths (base + immediate non-dot children), honoring the target
        // project filter. §DR2-4 composition: push + break within a single `rp`
        // (an FQN matches at most one replica dir per project) but continue the
        // outer loop so ambiguity across projects is detected.
        if target_local.starts_with("wg-") {
            if let Some((wg_name, agent_short)) = target_local.split_once('/') {
                let replica_dir = format!("__agent_{}", agent_short);

                let project_matches = |dir_name: &str| -> bool {
                    match target_project {
                        Some(want) => dir_name == want,
                        None => true,
                    }
                };

                for rp in &cfg.project_paths {
                    let base = std::path::Path::new(rp);
                    if !base.is_dir() {
                        continue;
                    }
                    let base_name = base.file_name().and_then(|n| n.to_str()).unwrap_or("");

                    let mut dirs_to_check: Vec<std::path::PathBuf> = Vec::new();
                    if project_matches(base_name) {
                        dirs_to_check.push(base.to_path_buf());
                    }
                    if let Ok(entries) = std::fs::read_dir(base) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if !p.is_dir() {
                                continue;
                            }
                            let dir_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            if dir_name.starts_with('.') {
                                continue;
                            }
                            if !project_matches(dir_name) {
                                continue;
                            }
                            dirs_to_check.push(p);
                        }
                    }

                    for dir in dirs_to_check {
                        let candidate = dir.join(".ac-new").join(wg_name).join(&replica_dir);
                        if candidate.is_dir() {
                            record_match(&candidate.to_string_lossy(), &mut matches);
                            // Within a single `rp`, first hit is the unique hit —
                            // an FQN matches one replica dir per project. Continue
                            // OUTER loop to detect cross-project ambiguity (§DR2-4).
                            break;
                        }
                    }
                }
            }
        }

        match matches.len() {
            0 => None,
            1 => Some(matches.pop().unwrap()),
            _ => {
                log::warn!(
                    "[mailbox] resolve_repo_path('{}'): {} candidates, refusing arbitrary pick: {:?}",
                    agent_name, matches.len(), matches
                );
                None
            }
        }
    }

    // Shadow `agent_name_from_path` removed — all mailbox call sites now use
    // `crate::config::teams::agent_fqn_from_path` per §AR2 (§DR2 consolidation).

    /// Check if sender can reach destination via team membership.
    /// Only agents in the same team can communicate — no parent directory fallback.
    fn can_reach(&self, from: &str, to: &str, discovered_teams: &[teams::DiscoveredTeam]) -> bool {
        crate::config::teams::can_communicate(from, to, discovered_teams)
    }

    /// Resolve which agent CLI to spawn when `deliver_wake` needs a new
    /// persistent session for the destination agent.
    async fn resolve_agent_command(
        &self,
        app: &tauri::AppHandle,
        msg: &OutboxMessage,
    ) -> Option<(String, Vec<String>)> {
        let settings = app.state::<SettingsState>();
        let cfg = settings.read().await;

        // If specific agent requested
        if msg.preferred_agent != "auto" {
            if let Some(agent) = cfg.agents.iter().find(|a| a.id == msg.preferred_agent) {
                return Some((agent.command.clone(), vec![]));
            }
        }

        // Try lastCodingAgent from destination's config.json
        if let Some(dest_path) = self.resolve_repo_path(&msg.to, app).await {
            let config_path = Path::new(&dest_path)
                .join(crate::config::agent_local_dir_name())
                .join("config.json");
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(local_config) = serde_json::from_str::<AgentLocalConfig>(&content) {
                    if let Some(last_agent) = local_config.tooling.last_coding_agent.as_deref() {
                        if let Some(agent) = cfg.agents.iter().find(|a| a.id == last_agent) {
                            return Some((agent.command.clone(), vec![]));
                        }
                    }
                }
            }
        }

        // Fallback: use sender's agent if known
        if let Some(ref sender_agent) = msg.sender_agent {
            if let Some(agent) = cfg.agents.iter().find(|a| a.id == *sender_agent) {
                return Some((agent.command.clone(), vec![]));
            }
        }

        // Last resort: use first configured agent
        cfg.agents.first().map(|a| (a.command.clone(), vec![]))
    }

    /// Resolve agent_id and agent_label for a destination agent.
    /// Reads lastCodingAgent from the dest's config.json, then looks up the label in settings.
    async fn resolve_agent_id_and_label(
        &self,
        app: &tauri::AppHandle,
        cwd: &str,
    ) -> (Option<String>, Option<String>) {
        let config_path = std::path::Path::new(cwd)
            .join(crate::config::agent_local_dir_name())
            .join("config.json");

        let last_agent_id = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|content| serde_json::from_str::<AgentLocalConfig>(&content).ok())
            .and_then(|cfg| cfg.tooling.last_coding_agent);

        let Some(ref agent_id) = last_agent_id else {
            return (None, None);
        };

        let settings = app.state::<SettingsState>();
        let cfg = settings.read().await;
        let label = cfg
            .agents
            .iter()
            .find(|a| a.id == *agent_id)
            .map(|a| a.label.clone());

        (last_agent_id, label)
    }

    /// Fallback path resolution for WG agents: find a sibling session in the same WG,
    /// derive the WG directory from its CWD, and construct the target agent path.
    ///
    /// §4.4: peel optional `<project>:` prefix before splitting the local part.
    /// If the target is qualified, the returned candidate must also be in the
    /// same project (checked via derived FQN).
    async fn resolve_wg_path_from_sessions(
        &self,
        app: &tauri::AppHandle,
        agent_name: &str,
    ) -> Option<String> {
        let (target_project, local) = crate::config::teams::split_project_prefix(agent_name);
        let (wg_name, agent_short) = local.split_once('/')?;
        if !wg_name.starts_with("wg-") {
            return None;
        }

        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_working_dirs().await;
        drop(mgr);

        let wg_marker = format!("/{}/", wg_name);
        for (_, cwd) in &dirs {
            let normalized = cwd.replace('\\', "/");
            if let Some(wg_pos) = normalized.rfind(&wg_marker) {
                let wg_dir = &normalized[..wg_pos + 1 + wg_name.len()];
                let candidate = format!("{}/__agent_{}", wg_dir, agent_short);
                if std::path::Path::new(&candidate).is_dir() {
                    // If target is qualified, enforce same project on the candidate.
                    if let Some(want) = target_project {
                        let candidate_fqn = crate::config::teams::agent_fqn_from_path(&candidate);
                        let (cand_project, _) =
                            crate::config::teams::split_project_prefix(&candidate_fqn);
                        if cand_project != Some(want) {
                            continue;
                        }
                    }
                    log::info!(
                        "[mailbox] wake: resolved WG agent path from sibling session: {}",
                        candidate
                    );
                    return Some(
                        std::path::PathBuf::from(&candidate)
                            .to_string_lossy()
                            .to_string(),
                    );
                }
            }
        }

        None
    }

    /// Move an outbox message to outbox/delivered/ with token stripped.
    async fn move_to_delivered(&self, path: &Path, msg: &OutboxMessage) -> Result<(), String> {
        let delivered_dir = path.parent().ok_or("No parent dir")?.join("delivered");
        std::fs::create_dir_all(&delivered_dir)
            .map_err(|e| format!("Failed to create delivered dir: {}", e))?;

        // Strip token before storing
        let mut stripped = msg.clone();
        stripped.token = None;

        let dest = delivered_dir.join(format!("{}.json", msg.id));
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {}", e))?;
        std::fs::write(&dest, json)
            .map_err(|e| format!("Failed to write delivered file: {}", e))?;

        // Remove original
        std::fs::remove_file(path).map_err(|e| format!("Failed to remove outbox file: {}", e))?;

        log::info!("[mailbox] Message {} moved to delivered/", msg.id);
        Ok(())
    }

    /// Reject a message: move to outbox/rejected/ with reason, and notify the sender.
    async fn reject_message(
        &self,
        path: &Path,
        msg: &OutboxMessage,
        reason: &str,
    ) -> Result<(), String> {
        let rejected_dir = path.parent().ok_or("No parent dir")?.join("rejected");
        std::fs::create_dir_all(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {}", e))?;

        // Write reason file FIRST — the CLI polls for this file to detect rejection
        let reason_path = rejected_dir.join(format!("{}.reason.txt", msg.id));
        std::fs::write(&reason_path, reason)
            .map_err(|_| "Failed to write reason file".to_string())?;

        // Then write the stripped message JSON
        let mut stripped = msg.clone();
        stripped.token = None;

        let dest = rejected_dir.join(format!("{}.json", msg.id));
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {}", e))?;
        std::fs::write(&dest, json).map_err(|e| format!("Failed to write rejected file: {}", e))?;

        // Remove original
        std::fs::remove_file(path).map_err(|e| format!("Failed to remove outbox file: {}", e))?;

        log::warn!(
            "[mailbox] Message {} moved to rejected/: {}",
            msg.id,
            reason
        );
        Ok(())
    }

    /// Reject a raw file that cannot be parsed as OutboxMessage.
    fn reject_raw_file(path: &Path, reason: &str) -> Result<(), String> {
        let rejected_dir = path.parent().ok_or("No parent dir")?.join("rejected");
        std::fs::create_dir_all(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {}", e))?;

        let filename = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("unknown.json");

        let dest = rejected_dir.join(filename);
        std::fs::rename(path, &dest)
            .or_else(|_| std::fs::copy(path, &dest).and_then(|_| std::fs::remove_file(path)))
            .map_err(|e| format!("Failed to move file to rejected: {}", e))?;

        let stem = Path::new(filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let reason_path = rejected_dir.join(format!("{}.reason.txt", stem));
        std::fs::write(&reason_path, reason)
            .map_err(|_| "Failed to write reason file".to_string())?;

        log::warn!("Rejected raw file {:?}: {}", path, reason);
        Ok(())
    }

    /// Poll ~/.agentscommander/session-requests/ for launch requests from the CLI.
    async fn poll_session_requests(&self, app: &tauri::AppHandle) {
        let config_dir = match crate::config::config_dir() {
            Some(d) => d,
            None => return,
        };
        let requests_dir = config_dir.join("session-requests");
        if !requests_dir.is_dir() {
            return;
        }

        let entries: Vec<PathBuf> = match std::fs::read_dir(&requests_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                .collect(),
            Err(_) => return,
        };

        for path in entries {
            let content = match read_text_bom_tolerant(&path) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("[session-requests] Failed to read {:?}: {}", path, e);
                    continue;
                }
            };

            let request: crate::cli::create_agent::SessionRequest =
                match serde_json::from_str(&content) {
                    Ok(r) => r,
                    Err(e) => {
                        log::warn!("[session-requests] Failed to parse {:?}: {}", path, e);
                        // Delete malformed file
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                };

            log::info!(
                "[session-requests] Processing: name='{}' cwd='{}' agent='{}'",
                request.session_name,
                request.cwd,
                request.agent_id
            );

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

            match crate::commands::session::create_session_inner(
                app,
                session_mgr.inner(),
                pty_mgr.inner(),
                request.shell.clone(),
                request.shell_args.clone(),
                request.cwd.clone(),
                Some(request.session_name.clone()),
                Some(request.agent_id.clone()),
                None,       // No agent label — auto-detected from shell
                false,      // Persist tooling
                Vec::new(), // git_repos
                true,       // skip_auto_resume = true → CLI session-request is a fresh create
            )
            .await
            {
                Ok(info) => {
                    log::info!(
                        "[session-requests] Created session '{}' (id={})",
                        request.session_name,
                        info.id
                    );
                }
                Err(e) => {
                    log::error!(
                        "[session-requests] Failed to create session '{}': {}",
                        request.session_name,
                        e
                    );
                }
            }

            // Delete processed request file regardless of success/failure
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Read a file as UTF-8 string, tolerant of UTF-8 / UTF-16 LE / UTF-16 BE BOMs.
/// Logs a warning when a BOM is detected so users see which tool is writing
/// odd encoding into outbox / session-requests (typically PowerShell on Windows).
fn read_text_bom_tolerant(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read file: {}", e))?;

    if bytes.starts_with(&[0xFF, 0xFE]) {
        log::warn!(
            "[bom] UTF-16 LE BOM detected in {:?} — decoding to UTF-8 (writer should use UTF-8 without BOM)",
            path
        );
        let u16_data: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&u16_data))
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        log::warn!(
            "[bom] UTF-16 BE BOM detected in {:?} — decoding to UTF-8 (writer should use UTF-8 without BOM)",
            path
        );
        let u16_data: Vec<u16> = bytes[2..]
            .chunks_exact(2)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    // ── §224 D.5a — wait_for_restore_or_session unit tests ──

    // ── §224 D.3 — filter_sessions_by_fqn pure-predicate tests ──

    fn make_session_info(
        id: &str,
        name: &str,
        cwd: &str,
        status: crate::session::session::SessionStatus,
        waiting_for_input: bool,
    ) -> crate::session::session::SessionInfo {
        crate::session::session::SessionInfo {
            id: id.into(),
            name: name.into(),
            shell: "claude".into(),
            shell_args: vec![],
            effective_shell_args: None,
            created_at: "2026-05-16T00:00:00Z".into(),
            working_directory: cwd.into(),
            status,
            waiting_for_input,
            pending_review: false,
            last_prompt: None,
            agent_id: None,
            agent_label: None,
            git_repos: vec![],
            workgroup_brief: None,
            is_coordinator: false,
            token: "t".into(),
            agent_kind: Some(crate::session::profile::CodingAgentKind::Claude),
            was_detached: false,
            detached_geometry: None,
        }
    }

    /// §224 regression: an idle session with `waiting_for_input=true` (the bug
    /// user's exact state) must still pass the predicate. The predicate is
    /// FQN-only by design.
    #[test]
    fn filter_sessions_by_fqn_includes_idle_waiting_session() {
        let target = "proj:wg-1-devs/alice";
        let s = make_session_info(
            "uuid-1",
            "alice",
            r"C:\proj\.ac-new\wg-1-devs\__agent_alice",
            crate::session::session::SessionStatus::Idle,
            true,
        );
        let pool = vec![s];
        let hits = filter_sessions_by_fqn(&pool, target);
        assert_eq!(
            hits.len(),
            1,
            "idle/waiting session must match FQN-only predicate"
        );
    }

    #[test]
    fn filter_sessions_by_fqn_rejects_non_matching_cwd() {
        let target = "proj:wg-1-devs/alice";
        let s = make_session_info(
            "uuid-1",
            "bob",
            r"C:\proj\.ac-new\wg-1-devs\__agent_bob",
            crate::session::session::SessionStatus::Active,
            false,
        );
        let pool = vec![s];
        let hits = filter_sessions_by_fqn(&pool, target);
        assert!(hits.is_empty(), "bob's session must not match alice's FQN");
    }

    #[test]
    fn filter_sessions_by_fqn_matches_regardless_of_was_detached_or_status() {
        // §224 — the active vs detached vs waiting-for-input mix should not
        // matter; only the FQN of working_directory.
        let target = "proj:wg-1-devs/alice";
        let mut active = make_session_info(
            "uuid-a",
            "alice-active",
            r"C:\proj\.ac-new\wg-1-devs\__agent_alice",
            crate::session::session::SessionStatus::Active,
            false,
        );
        active.was_detached = true;
        let idle = make_session_info(
            "uuid-b",
            "alice-idle",
            r"C:\proj\.ac-new\wg-1-devs\__agent_alice",
            crate::session::session::SessionStatus::Idle,
            true,
        );
        let exited = make_session_info(
            "uuid-c",
            "alice-exited",
            r"C:\proj\.ac-new\wg-1-devs\__agent_alice",
            crate::session::session::SessionStatus::Exited(0),
            false,
        );
        let pool = vec![active, idle, exited];
        let hits = filter_sessions_by_fqn(&pool, target);
        assert_eq!(hits.len(), 3, "all three must match the same FQN");
    }

    #[tokio::test]
    async fn wait_returns_session_when_probe_succeeds_mid_wait() {
        let flag = AtomicBool::new(true);
        let counter = std::sync::Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let result = wait_for_restore_or_session(
            &flag,
            || {
                let counter = counter_clone.clone();
                async move {
                    let n = counter.fetch_add(1, Ordering::SeqCst);
                    if n >= 2 {
                        vec![Uuid::nil()]
                    } else {
                        vec![]
                    }
                }
            },
            deadline,
            std::time::Duration::from_millis(50),
        )
        .await;
        assert!(result.is_ok(), "probe should have produced a hit");
        assert_eq!(result.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn wait_returns_still_in_progress_when_flag_never_clears() {
        let flag = AtomicBool::new(true);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        let result = wait_for_restore_or_session(
            &flag,
            || async { vec![] },
            deadline,
            std::time::Duration::from_millis(50),
        )
        .await;
        assert_eq!(result, Err(RestoreWaitOutcome::StillInProgress));
    }

    #[tokio::test]
    async fn wait_returns_no_match_when_flag_clears_with_empty_result() {
        let flag = std::sync::Arc::new(AtomicBool::new(true));
        let flag_clone = flag.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            flag_clone.store(false, Ordering::SeqCst);
        });
        let result = wait_for_restore_or_session(
            &flag,
            || async { vec![] },
            deadline,
            std::time::Duration::from_millis(50),
        )
        .await;
        assert_eq!(result, Err(RestoreWaitOutcome::NoMatch));
    }

    #[test]
    fn wake_action_running_injects() {
        assert_eq!(wake_action_for(&SessionStatus::Running), WakeAction::Inject);
    }

    // ── wake_spawn_skip_auto_resume tests (issue #82, plan §8.2) ──

    #[test]
    fn wake_spawn_skip_auto_resume_skips_when_cold() {
        // Cold wake (no prior session, or race fallthrough) — suppress resume.
        assert!(wake_spawn_skip_auto_resume(false));
    }

    #[test]
    fn wake_spawn_skip_auto_resume_allows_when_known_state() {
        // Known-state wake (RespawnExited match) — allow `--continue` /
        // codex `resume --last` / gemini `--resume latest`.
        assert!(!wake_spawn_skip_auto_resume(true));
    }

    #[test]
    fn wake_action_active_injects() {
        assert_eq!(wake_action_for(&SessionStatus::Active), WakeAction::Inject);
    }

    #[test]
    fn wake_action_idle_injects() {
        assert_eq!(wake_action_for(&SessionStatus::Idle), WakeAction::Inject);
    }

    #[test]
    fn wake_action_exited_respawns() {
        assert_eq!(
            wake_action_for(&SessionStatus::Exited(0)),
            WakeAction::RespawnExited
        );
        // Non-zero exit codes take the same path.
        assert_eq!(
            wake_action_for(&SessionStatus::Exited(1)),
            WakeAction::RespawnExited
        );
        assert_eq!(
            wake_action_for(&SessionStatus::Exited(-1)),
            WakeAction::RespawnExited
        );
    }

    // ── is_viable_wake_candidate tests (issue #223) ──

    #[test]
    fn is_viable_wake_candidate_keeps_live_running() {
        assert!(is_viable_wake_candidate(&SessionStatus::Running, true));
    }

    #[test]
    fn is_viable_wake_candidate_keeps_live_idle() {
        assert!(is_viable_wake_candidate(&SessionStatus::Idle, true));
    }

    #[test]
    fn is_viable_wake_candidate_keeps_live_active() {
        assert!(is_viable_wake_candidate(&SessionStatus::Active, true));
    }

    /// Issue #223 — primary regression guard. A SessionManager record with
    /// status=Running but no PtyManager entry (the phantom in the log) MUST be
    /// skipped — otherwise inject_into_pty fails with `Session not found:` and
    /// the router has no fallback.
    #[test]
    fn is_viable_wake_candidate_skips_phantom_running_no_pty() {
        assert!(!is_viable_wake_candidate(&SessionStatus::Running, false));
    }

    #[test]
    fn is_viable_wake_candidate_skips_phantom_idle_no_pty() {
        assert!(!is_viable_wake_candidate(&SessionStatus::Idle, false));
    }

    #[test]
    fn is_viable_wake_candidate_skips_phantom_active_no_pty() {
        assert!(!is_viable_wake_candidate(&SessionStatus::Active, false));
    }

    /// AC5 regression guard. Deferred-non-coord sessions have status=Exited(0)
    /// and NO PTY (see lib.rs:614). They MUST remain candidates so the
    /// documented respawn path runs and `--continue` is injected on the new
    /// session.
    #[test]
    fn is_viable_wake_candidate_keeps_exited_without_pty_for_respawn() {
        assert!(is_viable_wake_candidate(&SessionStatus::Exited(0), false));
        assert!(is_viable_wake_candidate(&SessionStatus::Exited(1), false));
        assert!(is_viable_wake_candidate(&SessionStatus::Exited(-1), false));
    }

    /// Stale-PTY-instance edge case: status=Exited with a leftover PtyInstance
    /// entry. Still a candidate — RespawnExited will destroy the leftover.
    #[test]
    fn is_viable_wake_candidate_keeps_exited_with_stale_pty() {
        assert!(is_viable_wake_candidate(&SessionStatus::Exited(0), true));
    }

    /// A PtyManager entry that EXISTS but whose underlying child has exited is
    /// STILL treated as viable by this layer (we cannot detect child liveness
    /// without surfacing portable_pty::Child::wait()). Documented here so a
    /// future PR closing the AC3 gap (PTY-exit hook in `#223-fu1`) knows this
    /// test must be updated to assert the new contract. (dev-rust R1.E3.)
    #[test]
    fn is_viable_wake_candidate_accepts_mapped_pty_regardless_of_child_state() {
        // status=Running + has_pty=true → viable, even if the child is secretly dead.
        // The router relies on `#223-fu1` to keep status in sync with reality.
        assert!(is_viable_wake_candidate(&SessionStatus::Running, true));
    }

    // ── err_is_pty_session_missing tests (issue #223) ──

    #[test]
    fn err_is_pty_session_missing_matches_inject_wrap() {
        // Exact shape emitted by pty::inject::inject_text_into_session
        // (pty/inject.rs:67) when PtyManager::write returns SessionNotFound.
        let e = "PTY write failed: Session not found: 2ced5ccf-1234-5678-9abc-def012345678";
        assert!(err_is_pty_session_missing(e));
    }

    #[test]
    fn err_is_pty_session_missing_matches_bare_form() {
        // Matches even without the inject_text_into_session wrapping, in case
        // a future call site propagates the inner error directly.
        let e = "Session not found: abcdef";
        assert!(err_is_pty_session_missing(e));
    }

    #[test]
    fn err_is_pty_session_missing_rejects_unrelated_errors() {
        assert!(!err_is_pty_session_missing("PTY error: broken pipe"));
        assert!(!err_is_pty_session_missing(
            "Failed to read outbox file: foo"
        ));
        assert!(!err_is_pty_session_missing(""));
    }

    /// Pins the actual `AppError::SessionNotFound` Display format. If a future
    /// refactor changes `errors.rs:5` to anything other than `"Session not
    /// found: {0}"`, this test fails with a clear message — the sniff is the
    /// load-bearing safety net for the candidate loop. (grinch G.M3.)
    #[test]
    fn err_is_pty_session_missing_matches_actual_apperror_format() {
        use crate::errors::AppError;
        let id = uuid::Uuid::new_v4();
        let raw = AppError::SessionNotFound(id.to_string()).to_string();
        assert!(
            err_is_pty_session_missing(&raw),
            "AppError::SessionNotFound display changed to {:?}; \
             err_is_pty_session_missing substring sniff broken — \
             update both the sniff and this test",
            raw
        );
    }

    // ── Anti-spoof / canonicalization pure-logic tests (AR2-tests 22, 23 + DR2-5) ──

    /// §DR7 / AR2-tests #22: legacy-unqualified msg.from is accepted when its
    /// local part matches the anti-spoof expected_from (local-only fallback).
    #[test]
    fn anti_spoof_legacy_msg_from_accepted_by_local_match() {
        // expected_from derived from repo path (canonical FQN).
        let expected = "proj-a:wg-1-devs/tech-lead";
        // msg.from is legacy unqualified with same local part.
        let msg_from = "wg-1-devs/tech-lead";
        assert!(anti_spoof_accept(msg_from, expected));
    }

    /// §DR7 / AR2-tests #23: qualified-but-wrong-project msg.from is rejected.
    /// A naïve suffix match would accept this — the LOCAL-only fallback rejects.
    #[test]
    fn anti_spoof_cross_project_qualified_msg_from_rejected() {
        let expected = "proj-a:wg-1-devs/tech-lead";
        let spoofed = "proj-b:wg-1-devs/tech-lead";
        assert!(!anti_spoof_accept(spoofed, expected));
    }

    /// Exact-FQN match is trivially accepted (baseline).
    #[test]
    fn anti_spoof_exact_fqn_match_accepted() {
        let expected = "proj-a:wg-1-devs/tech-lead";
        assert!(anti_spoof_accept(expected, expected));
    }

    /// §DR2-5 / AR2-tests #25: §AR2-norm step (1) upgrades a legacy-unqualified
    /// msg.from to the anti-spoof expected_from FQN. Without this upgrade,
    /// grinch §G5's `resolve_repo_path(&msg.from)` response-dir lookup could
    /// receive an ambiguous-local-part input.
    #[test]
    fn process_message_canonicalizes_legacy_msg_from() {
        let mut msg_from = "wg-1-devs/tech-lead".to_string();
        let expected_from = "proj-a:wg-1-devs/tech-lead";
        let upgraded = canonicalize_msg_from_in_place(&mut msg_from, Some(expected_from));
        assert!(upgraded);
        assert_eq!(msg_from, "proj-a:wg-1-devs/tech-lead");
    }

    /// Already-qualified msg.from is NOT overwritten by canonicalization.
    #[test]
    fn canonicalize_noop_for_already_qualified_msg_from() {
        let mut msg_from = "proj-a:wg-1-devs/tech-lead".to_string();
        let upgraded = canonicalize_msg_from_in_place(
            &mut msg_from,
            Some("proj-b:wg-1-devs/tech-lead"), // different project — don't overwrite!
        );
        assert!(!upgraded);
        assert_eq!(msg_from, "proj-a:wg-1-devs/tech-lead");
    }

    /// No expected_from (app outbox path) → canonicalization is a no-op.
    #[test]
    fn canonicalize_noop_when_expected_from_absent() {
        let mut msg_from = "wg-1-devs/alice".to_string();
        let upgraded = canonicalize_msg_from_in_place(&mut msg_from, None);
        assert!(!upgraded);
        assert_eq!(msg_from, "wg-1-devs/alice");
    }

    // ── Full mailbox-pipeline tests marked [INT] — placeholders for a future
    // two-project fixture harness. Acknowledged to ship with the fix per
    // tech-lead's must-apply directive; the pure-logic tests above cover the
    // §G1, §G2, §G5 regression surface at the unit level, and §AR2-G1's
    // close-session resolver gate is covered by the §AR2-shared resolver
    // tests (config::teams::tests::resolve_agent_target_*). ──

    /// §G9#1 / AR2-tests #17 — close-session with unqualified target from a
    /// direct outbox write MUST NOT destroy sessions in an unauthorized
    /// project. Covered at the resolver layer by
    /// `resolve_agent_target_rejects_ambiguous` and
    /// `resolve_agent_target_two_level_scan` in `config/teams.rs::tests`;
    /// `handle_close_session`'s §AR2-G1 gate calls `resolve_agent_target`
    /// before authorization, so rejecting Ambiguous at that layer blocks the
    /// attack before any session is touched. Full end-to-end fixture needs a
    /// Tauri `AppHandle` harness — follow-up.
    #[test]
    #[ignore = "integration: needs two-project Tauri AppHandle fixture"]
    fn close_session_rejects_direct_outbox_write_with_unqualified_target() {
        // Full-pipeline assertion stub; logic-layer coverage lives in:
        //   - config::teams::tests::resolve_agent_target_rejects_ambiguous
        //   - config::teams::tests::resolve_agent_target_two_level_scan
        //   - config::teams::tests::is_coordinator_rejects_legacy_unqualified_from
    }

    /// §G9#2 / AR2-tests #18 — wake with ambiguous unqualified `to` MUST be
    /// rejected, not silently routed. Covered at the resolver layer by the
    /// same `resolve_agent_target_rejects_ambiguous` test; `process_message`
    /// calls `resolve_agent_target` on `msg.to` at §AR2-norm before mode
    /// dispatch, so ambiguous `msg.to` becomes a rejected outbox message.
    /// Full end-to-end fixture needs an AppHandle harness.
    #[test]
    #[ignore = "integration: needs two-project Tauri AppHandle fixture"]
    fn deliver_wake_rejects_unqualified_to_with_cross_project_matches() {
        // Full-pipeline assertion stub; logic-layer coverage lives in:
        //   - config::teams::tests::resolve_agent_target_rejects_ambiguous
        //   - config::teams::tests::resolve_agent_target_two_level_scan
    }

    /// §G9#3 / AR2-tests #19 — `resolve_repo_path` WG fallback with a
    /// qualified target honors `target_project` (no cross-project leak even
    /// when the base dir `rp` is another project's root). Logic covered by
    /// the `project_matches` closure + `dirs_to_check` seeding in the
    /// §AR2-G3 block; a full fixture-based test needs filesystem setup
    /// under an AppHandle harness.
    #[test]
    #[ignore = "integration: needs filesystem fixture + AppHandle"]
    fn resolve_repo_path_wg_fallback_honors_target_project() {}

    /// §G9#4 / AR2-tests #20 — `resolve_repo_path` with an unqualified target
    /// matching multiple projects returns `None` (refuses arbitrary pick).
    /// §AR2-G4 collector pattern logic is covered by inspection — the
    /// `matches.len()` match arm returns None on `_`. Full integration test
    /// needs AppHandle + session-CWDs fixture.
    #[test]
    #[ignore = "integration: needs filesystem fixture + AppHandle"]
    fn resolve_repo_path_returns_none_on_ambiguous_unqualified() {}

    /// §G9#8 / AR2-tests #21 — a session spawned by `deliver_wake` from an
    /// FQN `msg.to` has a sidebar `Session.name` WITHOUT the `:` prefix.
    /// §AR2-session-name handles this at mailbox.rs (spawn path) via
    /// `split_project_prefix(&msg.to).1`. The logic is one line; a full
    /// integration test needs a Tauri runtime.
    #[test]
    #[ignore = "integration: needs Tauri runtime + session manager fixture"]
    fn deliver_wake_spawned_session_name_has_no_colon() {}

    /// §G9#9 / AR2-tests #24 — full round-trip CLI send → mailbox route →
    /// reply. Intentionally integration-level; no unit scaffolding.
    #[test]
    #[ignore = "integration: full CLI + mailbox + two-project fixture"]
    fn resolve_to_target_round_trip_integration() {}

    // ── #228 D1-a — derive_project_from_outbox_path tests ──

    #[test]
    fn derive_project_from_outbox_path_walks_up_wg_replica_layout() {
        let temp = tempfile::TempDir::new().unwrap();
        let project_dir = temp.path().join("proj-x");
        let outbox = project_dir
            .join(".ac-new")
            .join("wg-1-devs")
            .join("__agent_alice")
            .join(".agentscommander") // matches agent_local_dir_name() shape
            .join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        let file = outbox.join("msg-42.json");
        std::fs::write(&file, "{}").unwrap();

        let got =
            derive_project_from_outbox_path(&file).expect("should derive project from WG layout");
        let expected = std::fs::canonicalize(&project_dir)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(got, expected);
    }

    #[test]
    fn derive_project_from_outbox_path_rejects_non_wg_layout() {
        let temp = tempfile::TempDir::new().unwrap();
        // A path with the right tail but missing the `.ac-new` ancestor.
        let outbox = temp
            .path()
            .join("random")
            .join(".agentscommander")
            .join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        let file = outbox.join("msg.json");
        std::fs::write(&file, "{}").unwrap();
        assert!(derive_project_from_outbox_path(&file).is_none());
    }

    #[test]
    fn derive_project_from_outbox_path_rejects_app_outbox_layout() {
        // App-outbox lives at <config_dir>/app-outbox/<file>.json — no
        // `__agent_*` / `wg-*` ancestors. Helper must return None so app-outbox
        // messages are NOT augmented (they have their own master-token bypass).
        let temp = tempfile::TempDir::new().unwrap();
        let app_outbox = temp.path().join("config").join("app-outbox");
        std::fs::create_dir_all(&app_outbox).unwrap();
        let file = app_outbox.join("msg.json");
        std::fs::write(&file, "{}").unwrap();
        assert!(derive_project_from_outbox_path(&file).is_none());
    }

    // ── Issue #223 deliver_wake integration placeholders ──

    /// Issue #223 — full regression test the user explicitly requested:
    ///   "two CWD-matching records, one dead PTY, one live — assert delivery
    ///   reaches live."
    /// Full-pipeline assertion needs a Tauri AppHandle harness with both
    /// SessionManager and PtyManager state primed. Logic coverage lives in the
    /// `is_viable_wake_candidate_*` unit tests above. Once a shared
    /// AppHandle fixture exists (see existing #[ignore] stubs), un-ignore.
    #[test]
    #[ignore = "integration: needs Tauri AppHandle + Session/Pty fixtures"]
    fn deliver_wake_routes_to_live_when_phantom_present() {
        // Fixture shape:
        //   1. SessionManager has two records, same working_directory:
        //      A: id=A, status=Running, name="phantom"      → NO PtyManager entry
        //      B: id=B, status=Idle,    name="live"         → has PtyManager entry
        //   2. Call deliver_wake(msg{to=agent}).
        //   3. Assert inject_text_into_session was called with B (NOT A).
        //   4. Assert delivery returned Ok.
    }

    /// Issue #223 — secondary fallback test:
    ///   "If first live candidate dies between liveness probe and inject,
    ///   router must try next candidate within the SAME delivery attempt."
    #[test]
    #[ignore = "integration: needs Tauri AppHandle + Session/Pty fixtures"]
    fn deliver_wake_falls_back_when_first_inject_race_kills_pty() {
        // Fixture shape:
        //   1. SessionManager has two records, same working_directory, both
        //      status=Running, both initially with PtyManager entries.
        //   2. Patch PtyManager::write so the first call returns SessionNotFound.
        //   3. Call deliver_wake(msg{to=agent}).
        //   4. Assert two inject attempts inside one delivery, second succeeds.
    }

    /// Issue #223 — AC5 regression guard:
    ///   Wake delivery to a deferred-non-coord session (status=Exited(0), no
    ///   PTY) MUST still take the RespawnExited path, NOT silently fall through
    ///   to cold spawn-persistent.
    #[test]
    #[ignore = "integration: needs Tauri AppHandle + Session/Pty fixtures"]
    fn deliver_wake_respawns_exited_deferred_session_with_resume_flag() {
        // Fixture shape:
        //   1. SessionManager has one record: status=Exited(0), no PtyManager
        //      entry (deferred-non-coord shape).
        //   2. Call deliver_wake(msg{to=agent}).
        //   3. Assert destroy_session_inner ran for the exited id.
        //   4. Assert create_session_inner ran with skip_auto_resume=false
        //      (spawn_with_resume=true → wake_spawn_skip_auto_resume(true)=false).
    }

    /// Issue #223 — HIGH-1 (Step-7 review): symmetric to the phantoms-only
    /// AC5 fall-through (grinch G.H2). When every viable Inject candidate
    /// races to dead between liveness probe and `PtyManager::write`, the
    /// post-loop branch MUST still promote `spawn_with_resume`. Otherwise
    /// cold spawn-persistent runs and the on-disk transcript at the matched
    /// CWD is silently abandoned — same AC5 outcome as the phantoms-only
    /// case, just with a race-killed-siblings cause instead of desync.
    /// Plausible triggers: system shutdown mid-wake, OOM kill of sibling
    /// PTYs, container restart, Windows logoff with sibling sessions.
    #[test]
    #[ignore = "integration: needs Tauri AppHandle + Session/Pty fixtures"]
    fn deliver_wake_promotes_resume_when_all_live_candidates_race_to_dead() {
        // Fixture shape:
        //   1. SessionManager has two records, same working_directory, both
        //      status=Running, both initially with PtyManager entries — so
        //      `find_live_candidates` returns both as viable with
        //      had_any_match=true.
        //   2. Patch PtyManager::write so BOTH attempts return SessionNotFound
        //      (simulates the race-window: sibling PTYs die between probe
        //      and write).
        //   3. Call deliver_wake(msg{to=agent}).
        //   4. Assert two inject attempts occurred inside one delivery, both
        //      failing the `err_is_pty_session_missing` substring sniff and
        //      hitting the `continue` arm.
        //   5. Assert `pending_exited_destroy` stayed None (neither candidate
        //      was Exited).
        //   6. Load-bearing assertion: spawn-persistent ran with
        //      `wake_spawn_skip_auto_resume(true) == false` (i.e.,
        //      skip_auto_resume=false → `--continue` / `resume --last` /
        //      `--resume latest` IS injected). Without the new
        //      `lost_inject_to_race` flag, this would silently cold-spawn
        //      (skip_auto_resume=true) and abandon the transcript.
    }

    // ── BOM-tolerant reader tests (issue #130) ──

    use std::io::Write;
    use tempfile::NamedTempFile;

    // Includes a non-BMP codepoint (😀 U+1F600 → surrogate pair D83D DE00) so the
    // UTF-16 LE/BE BOM tests exercise surrogate-pair decoding, not just BMP.
    const SAMPLE_JSON: &str = r#"{"id":"abc","kind":"ping","emoji":"😀"}"#;

    fn write_temp(bytes: &[u8]) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("tempfile");
        f.write_all(bytes).expect("write");
        f.flush().expect("flush");
        f
    }

    #[test]
    fn bom_tolerant_reads_plain_utf8() {
        let f = write_temp(SAMPLE_JSON.as_bytes());
        let got = read_text_bom_tolerant(f.path()).expect("read");
        assert_eq!(got, SAMPLE_JSON);
        // Parses as JSON.
        let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
    }

    #[test]
    fn bom_tolerant_strips_utf8_bom() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(SAMPLE_JSON.as_bytes());
        let f = write_temp(&bytes);
        let got = read_text_bom_tolerant(f.path()).expect("read");
        assert_eq!(got, SAMPLE_JSON);
        let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
    }

    #[test]
    fn bom_tolerant_decodes_utf16_le_bom() {
        let mut bytes = vec![0xFF, 0xFE];
        for u in SAMPLE_JSON.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let f = write_temp(&bytes);
        let got = read_text_bom_tolerant(f.path()).expect("read");
        assert_eq!(got, SAMPLE_JSON);
        let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
    }

    #[test]
    fn bom_tolerant_decodes_utf16_be_bom() {
        let mut bytes = vec![0xFE, 0xFF];
        for u in SAMPLE_JSON.encode_utf16() {
            bytes.extend_from_slice(&u.to_be_bytes());
        }
        let f = write_temp(&bytes);
        let got = read_text_bom_tolerant(f.path()).expect("read");
        assert_eq!(got, SAMPLE_JSON);
        let _: serde_json::Value = serde_json::from_str(&got).expect("parse");
    }

    #[test]
    fn bom_tolerant_rejects_invalid_utf8_no_bom() {
        // Lone continuation byte 0x80 is invalid UTF-8 and not a recognized BOM.
        let f = write_temp(&[0x80, 0x81, 0x82]);
        let err = read_text_bom_tolerant(f.path()).expect_err("must err");
        assert!(err.contains("Invalid UTF-8"), "unexpected error: {}", err);
    }

    #[test]
    fn bom_tolerant_empty_file_returns_empty_string() {
        let f = write_temp(&[]);
        let got = read_text_bom_tolerant(f.path()).expect("empty ok");
        assert_eq!(got, "");
        // Serde fails with a parse error (NOT a read error) — confirms the failure
        // surfaces at the parser, which is what the callsites wrap with their own
        // context strings.
        let parse_err = serde_json::from_str::<serde_json::Value>(&got).expect_err("must err");
        assert!(
            parse_err.is_eof(),
            "expected EOF parse error, got: {}",
            parse_err
        );
    }

    /// §130-stuck-file regression: when the reject path receives a file whose
    /// bytes are non-UTF-8 and have no BOM (e.g. PowerShell `Set-Content
    /// -Encoding ANSI` from a CP1252 locale), `read_text_bom_tolerant` returns
    /// `Err` every poll cycle. Before this fix, the reject branch was guarded
    /// by `if let Ok(content) = ...` and dropped to `else { false }` on Err —
    /// the file stayed in the source dir at `attempt_count >= MAX`, looping
    /// forever. The new `Err(_) => reject_raw_file(...)` arm closes that gap.
    /// This test drives the fallback directly: an unreadable file is moved to
    /// `rejected/` and a reason file is written, exactly as the new branch does.
    #[test]
    fn reject_raw_file_moves_unreadable_outbox_file_to_rejected_dir() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let outbox = tmp.path();
        let stuck = outbox.join("stuck.json");
        // CP1252 high-byte sequence — invalid UTF-8, no BOM.
        std::fs::write(&stuck, [0x80, 0x81, 0x82]).expect("write stuck file");

        // Precondition: this is exactly the input shape that hits the new Err arm.
        let read_err = read_text_bom_tolerant(&stuck).expect_err("read must err");
        assert!(
            read_err.contains("Invalid UTF-8"),
            "unexpected error: {}",
            read_err
        );

        // Drive the new fallback path.
        MailboxPoller::reject_raw_file(
            &stuck,
            "Undeliverable after 10 attempts. Last error: Failed to read outbox file: Invalid UTF-8",
        )
        .expect("reject_raw_file ok");

        assert!(
            !stuck.exists(),
            "original file should be moved out of source dir"
        );
        let rejected_dir = outbox.join("rejected");
        assert!(rejected_dir.is_dir(), "rejected/ should be created");
        assert!(
            rejected_dir.join("stuck.json").is_file(),
            "file should be moved to rejected/stuck.json"
        );
        let reason_file = rejected_dir.join("stuck.reason.txt");
        assert!(reason_file.is_file(), "reason file should be in rejected/");
        let reason = std::fs::read_to_string(&reason_file).expect("read reason");
        assert!(
            reason.contains("Undeliverable"),
            "reason content: {}",
            reason
        );
    }
}
