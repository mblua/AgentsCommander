use super::handoff::{
    anti_spoof_accept, canonicalize_msg_from_in_place, PURGE_WG_ACTION, RAISE_HAND_ACTION,
    SELF_CLEAR_ACTION, SELF_RESTART_ACTION, SELF_SWITCH_ACTION,
};
use super::notice::{
    derive_project_from_outbox_path, sender_name_for_session_cwd,
    sender_name_for_session_cwd_with_root_flag, validate_coordinator_to_root_route,
    validate_root_sender_payload, validate_root_sender_route, OutboxOrigin,
};
use super::poller::{MailboxPoller, OutboxAuthenticationFacts, OutboxStage, ValidatedWake};
use super::pty_observation::{PtyAuthorityState, PtyReadinessState};
use super::settle::session_has_current_pty_submission_provenance;
use crate::config::teams;
#[cfg(test)]
use crate::phone::mailbox::classify_outbox_document;
#[cfg(test)]
use crate::phone::mailbox::OutboxClassification;
use crate::phone::types::OutboxMessage;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use crate::MasterToken;
use std::path::Path;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn wait_for_pty_input_ready<S: PtyReadinessState + ?Sized>(
        &self,
        app: &S,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        verified_target: &crate::config::teams::VerifiedPtyInputIdentity,
        session_id: Uuid,
        spawned: bool,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
        heartbeat: &crate::api::message_store::PreparationHeartbeatGuard,
    ) -> Result<(), crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        loop {
            let expires = crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at)
                .map_err(|_| C::StoreCorrupt)?;
            if heartbeat.failed() {
                return Err(if expires <= chrono::Utc::now() {
                    C::Expired
                } else {
                    C::LeaseLost
                });
            }
            if expires <= chrono::Utc::now() {
                return Err(C::Expired);
            }
            if app
                .pty_restore()
                .is_some_and(|flag| flag.0.load(std::sync::atomic::Ordering::SeqCst))
            {
                return Err(C::RestoreInProgress);
            }
            if app
                .pty_purge()
                .is_some_and(|guard| guard.blocks_agent(&claimed.target_fqn))
            {
                return Err(C::PurgeInProgress);
            }
            let menu_guard = app.pty_menu();
            if menu_guard
                .as_ref()
                .is_some_and(|g| g.is_blocked(session_id))
            {
                return Err(C::MenuGuardBlocked);
            }
            let authority = if source_plane == crate::phone::types::PtyInputSourcePlane::HostCli {
                self.validate_claimed_host_authority(app, claimed).await
            } else {
                self.validate_claimed_api_authority(app, claimed, api_client_store)
                    .await
                    .map_err(|_| C::StoreTransient)?
            };
            if !authority.is_some_and(|route| {
                route.sender.authority_fingerprint == claimed.sender_identity_fingerprint
                    && route.target.authority_fingerprint == verified_target.authority_fingerprint
                    && crate::path_identity::same_object(
                        &route.target.replica_identity,
                        &verified_target.replica_identity,
                    )
            }) {
                return Err(C::AuthorityChanged);
            }
            let session = {
                let manager = app.pty_sessions();
                let guard = manager.read().await;
                guard.get_session(session_id).await
            }
            .ok_or(C::SessionRace)?;
            let current_target = crate::config::teams::verify_pty_input_replica_cwd(Path::new(
                &session.working_directory,
            ))
            .map_err(|_| C::AuthorityChanged)?;
            let settings = app.pty_settings().read().await.clone();
            if current_target.canonical_fqn != claimed.target_fqn
                || current_target.authority_fingerprint != verified_target.authority_fingerprint
                || !crate::path_identity::same_object(
                    &current_target.replica_identity,
                    &verified_target.replica_identity,
                )
                || !session_has_current_pty_submission_provenance(
                    &SessionInfo::from(&session),
                    &settings,
                )
                || matches!(session.status, SessionStatus::Exited(_))
            {
                return Err(C::SessionRace);
            }
            let route_valid = {
                let manager = app.pty_routes();
                let manager = manager.lock().unwrap_or_else(|error| error.into_inner());
                manager.has_session(session_id)
                    && manager.backend_kind(session_id) == Some(session.backend_kind)
                    && manager
                        .route_identities(session_id)
                        .is_some_and(|(cwd, replica, _)| {
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
            if !route_valid {
                return Err(C::SessionRace);
            }
            let readiness = app
                .pty_readiness()
                .purge_readiness(&[session_id])
                .into_iter()
                .next()
                .ok_or(C::UntrackedReadiness)?;
            let activity_required = readiness
                .idle_threshold
                .checked_add(std::time::Duration::from_secs(2))
                .ok_or(C::UntrackedReadiness)?;
            let activity_age = readiness.activity_age.ok_or(C::UntrackedReadiness)?;
            let resize_ready = match readiness.last_resize_age {
                None => true,
                Some(age) => readiness
                    .resize_grace
                    .checked_add(std::time::Duration::from_secs(2))
                    .is_some_and(|required| age >= required),
            };
            if session.waiting_for_input
                && readiness.watcher_idle
                && activity_age >= activity_required
                && resize_ready
            {
                return Ok(());
            }
            if !spawned && !session.waiting_for_input {
                return Err(C::Busy);
            }
            if std::time::Instant::now() >= deadline {
                return Err(C::ReadinessTimeout);
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    /// Process a single outbox message file.
    /// `origin`: the directory the file was found in (#2232 phase 7). The
    /// master/root-token origin is `OutboxOrigin::Master`.
    #[cfg(test)]
    pub(crate) async fn process_message<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        origin: OutboxOrigin,
    ) -> Result<(), String> {
        let content = match classify_outbox_document(path) {
            OutboxClassification::Standard(content) => content,
            OutboxClassification::PrivilegedCandidate { bytes, identity } => {
                if origin == OutboxOrigin::CoManaged {
                    let _ = Self::reject_raw_file(
                        path,
                        "Co-managed origin may carry only a wake; privileged PTY input is rejected",
                    );
                    return Ok(());
                }
                self.process_pty_input_file(
                    app,
                    path,
                    origin == OutboxOrigin::Master,
                    &bytes,
                    &identity,
                )
                .await;
                return Ok(());
            }
            OutboxClassification::InvalidDocument => {
                self.reject_malformed_pty_candidate(path);
                return Ok(());
            }
        };
        self.process_message_content(app, path, origin, &content, None)
            .await
    }

    /// (#1399) `deferred_tail`: `Some(slot)` when the caller owns a worker pool
    /// and wants the delivery tail detached: the validated message is parked in
    /// `slot` and `Ok(())` is returned WITHOUT delivering. `None` runs the tail
    /// inline (the `#[cfg(test)]` `process_message` entry point). Every earlier
    /// rejection return is unaffected because the success type stays `()`.
    pub(crate) async fn process_message_content<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        origin: OutboxOrigin,
        content: &str,
        deferred_tail: Option<&mut Option<ValidatedWake>>,
    ) -> Result<(), String> {
        let is_app_outbox = origin == OutboxOrigin::Master;
        let (msg, outbox_project) = match self
            .parse_and_normalize_outbox(app, path, origin, content)
            .await?
        {
            std::ops::ControlFlow::Continue(value) => value,
            std::ops::ControlFlow::Break(result) => return result,
        };
        let authentication = match self.authenticate_outbox(app, path, &msg).await? {
            std::ops::ControlFlow::Continue(value) => value,
            std::ops::ControlFlow::Break(result) => return result,
        };
        let () = match self
            .dispatch_outbox_self_operation(
                app,
                path,
                &msg,
                origin,
                is_app_outbox,
                authentication.4,
            )
            .await?
        {
            std::ops::ControlFlow::Continue(value) => value,
            std::ops::ControlFlow::Break(result) => return result,
        };
        let () = match self
            .authorize_outbox_route(app, path, &msg, origin, &outbox_project, authentication)
            .await?
        {
            std::ops::ControlFlow::Continue(value) => value,
            std::ops::ControlFlow::Break(result) => return result,
        };
        let () = match self
            .validate_outbox_action_and_mode(app, path, &msg, is_app_outbox)
            .await?
        {
            std::ops::ControlFlow::Continue(value) => value,
            std::ops::ControlFlow::Break(result) => return result,
        };
        self.deliver_outbox_tail(
            app,
            path,
            msg,
            content,
            (origin, deferred_tail),
            authentication,
        )
        .await
    }

    async fn parse_and_normalize_outbox<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        path: &Path,
        origin: OutboxOrigin,
        content: &str,
    ) -> OutboxStage<(OutboxMessage, Option<String>)> {
        // `let mut msg`: §AR2-norm below mutates `msg.from` / `msg.to` in place
        // as the SINGLE POINT OF TRUTH for canonicalization. Downstream code
        // (routing, action dispatch, injection, archival) reads the canonical
        // form without re-mutation.
        let mut msg: OutboxMessage = serde_json::from_str(content)
            .map_err(|e| format!("Failed to parse outbox message: {}", e))?;

        if let Some(raw) = msg.request_id.as_deref() {
            let parsed = Uuid::parse_str(raw).map_err(|_| "invalid_request_id".to_string())?;
            if raw != parsed.hyphenated().to_string() {
                return Err("invalid_request_id".to_string());
            }
        }

        log::info!(
            "[mailbox] Processing message {} from='{}' to='{}' mode='{}'",
            msg.id,
            msg.from,
            msg.to,
            msg.mode
        );

        let outbox_project = match derive_project_from_outbox_path(path) {
            Ok(project) => project,
            Err(reason) => {
                return Ok(std::ops::ControlFlow::Break(
                    self.reject_message(path, &msg, &reason).await,
                ));
            }
        };

        // For repo outboxes (not app-outbox), validate that msg.from matches the outbox owner.
        // This prevents tokenless spoofing: a message in repo X's outbox must claim to be from repo
        // X.
        //
        // §DR5 lenient fallback: if `msg.from` is unqualified (legacy) and its LOCAL
        // part matches `expected_from`'s local part, accept — §AR2-norm below then
        // upgrades `msg.from` to the canonical FQN. Cross-project qualified names
        // are rejected (different `project:` prefix).
        //
        // `expected_from` is hoisted (§DR2-3) so §AR2-norm can upgrade `msg.from`
        // even when this block has returned to outer scope.
        let mut expected_from: Option<String> = None;
        if origin == OutboxOrigin::Replica {
            let outbox_dir = path.parent().unwrap_or(Path::new(""));
            // outbox_dir is <repo>/.agentscommander/outbox — go up 2 levels to get the repo path
            if let Some(repo_path) = outbox_dir.parent().and_then(|p| p.parent()) {
                let derived = sender_name_for_session_cwd(&repo_path.to_string_lossy());
                if !anti_spoof_accept(&msg.from, &derived) {
                    return Ok(std::ops::ControlFlow::Break(
                        self.reject_message(
                            path,
                            &msg,
                            &format!(
                                "Outbox-sender mismatch: outbox belongs to '{}' but message claims \
                                '{}'",
                                derived, msg.from
                            ),
                        )
                        .await,
                    ));
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
            log::debug!(
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
        // layout (`<project>/<workspace>/wg-*/__agent_*/<local-dir>/outbox/<file>.json`),
        // include the derived `<project>` in the in-memory `paths` slice so qualified
        // WG-peer FQNs written by `cli::send` (which performs the symmetric walk-up
        // augmentation — see #228 Step 1) resolve here too. Without this, the daemon
        // re-rejects with `UnknownQualified` even though the CLI side succeeded.
        // settings.json is NOT mutated. In-memory, this-message only. See #228 D1-a.
        match self
            .normalize_outbox_recipient(app, path, &mut msg, &outbox_project)
            .await?
        {
            std::ops::ControlFlow::Continue(()) => {}
            std::ops::ControlFlow::Break(result) => {
                return Ok(std::ops::ControlFlow::Break(result))
            }
        }
        Ok(std::ops::ControlFlow::Continue((msg, outbox_project)))
    }

    async fn normalize_outbox_recipient<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        path: &Path,
        msg: &mut OutboxMessage,
        outbox_project: &Option<String>,
    ) -> OutboxStage {
        if !msg.to.is_empty() {
            let mut paths = {
                let cfg = app.pty_settings();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            if let Some(root_project) = outbox_project.as_ref() {
                let canon_root_project =
                    crate::filesystem::projects::canonical_refresh_project(Path::new(root_project))
                        .ok();
                let already_present = paths.iter().any(|p| match &canon_root_project {
                    Some(canon_target) => {
                        crate::filesystem::projects::canonical_refresh_project(Path::new(p))
                            .ok()
                            .as_ref()
                            == Some(canon_target)
                    }
                    None => p == root_project,
                });
                if !already_present {
                    paths.push(root_project.clone());
                }
            }
            match crate::config::teams::resolve_agent_target(&msg.to, &paths) {
                Ok(fqn) => {
                    if fqn != msg.to {
                        log::debug!("[mailbox] canonicalized msg.to '{}' → '{}'", msg.to, fqn);
                        msg.to = fqn;
                    }
                }
                Err(e) => {
                    return Ok(std::ops::ControlFlow::Break(
                        self.reject_message(path, msg, &format!("Unresolvable target: {}", e))
                            .await,
                    ));
                }
            }
        }

        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn authenticate_outbox_session<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        path: &Path,
        msg: &OutboxMessage,
        token_str: &str,
    ) -> OutboxStage<(bool, bool)> {
        let token_belongs_to_root_agent;
        let saw_session_token;
        let session_mgr = app.pty_sessions();
        let mgr = session_mgr.read().await;

        if let Ok(token_uuid) = Uuid::parse_str(token_str) {
            match mgr.find_by_token(token_uuid).await {
                None => {
                    // Token is stale/invalid. Env-only credentials cannot be refreshed into
                    // an already-running child process, so reject instead of injecting a new token.
                    drop(mgr);
                    if let Some(session_id) = self.find_active_session(app, &msg.from).await {
                        log::warn!(
                            "[mailbox] Stale token from '{}' matches active session {}, but \
                            env-only credentials cannot be refreshed in-place",
                            msg.from,
                            session_id
                        );
                    }
                    return Ok(std::ops::ControlFlow::Break(
                        self.reject_message(
                            path,
                            msg,
                            "Invalid session token. Env-only credentials cannot be refreshed into \
                            a live process; restart or respawn the sender session.",
                        )
                        .await,
                    ));
                }
                Some(session) => {
                    saw_session_token = true;
                    token_belongs_to_root_agent = session.is_root_agent
                        || crate::config::root_agent::is_root_agent_path(
                            &session.working_directory,
                        );
                    // Anti-spoofing: verify msg.from matches the token's session CWD.
                    // Post-§AR2-norm, msg.from is canonical FQN (or legacy unqualified
                    // if expected_from was unavailable). Session-derived name uses the
                    // canonical helper; comparison is exact equality.
                    let session_name = sender_name_for_session_cwd_with_root_flag(
                        &session.working_directory,
                        token_belongs_to_root_agent,
                    );
                    if session_name != msg.from {
                        log::warn!(
                            "[mailbox] Token-root mismatch: token session='{}' but from='{}'",
                            session_name,
                            msg.from
                        );
                        return Ok(std::ops::ControlFlow::Break(
                            self.reject_message(
                                path,
                                msg,
                                &format!(
                                    "Token-root mismatch: session is '{}' but message claims '{}'",
                                    session_name, msg.from
                                ),
                            )
                            .await,
                        ));
                    }
                }
            }
        } else {
            // Token is not a valid UUID. Env-only credentials cannot be refreshed
            // into an already-running child process, so reject instead of injecting.
            drop(mgr);
            if let Some(session_id) = self.find_active_session(app, &msg.from).await {
                log::warn!(
                    "[mailbox] Malformed token from '{}' matches active session {}, but env-only \
                    credentials cannot be refreshed in-place",
                    msg.from,
                    session_id
                );
            }
            return Ok(std::ops::ControlFlow::Break(self
                .reject_message(
                    path,
                    msg,
                    "Malformed session token. Env-only credentials cannot be refreshed into a live \
                    process; restart or respawn the sender session.",
                )
                .await));
        }
        Ok(std::ops::ControlFlow::Continue((
            token_belongs_to_root_agent,
            saw_session_token,
        )))
    }

    async fn authenticate_outbox<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
    ) -> OutboxStage<OutboxAuthenticationFacts> {
        // Check if token is the master token or root token (bypasses anti-spoofing + team
        // validation)
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

        let root_agent_claim = msg.from == crate::config::root_agent::ROOT_AGENT_SENDER;
        let root_agent_recipient = crate::config::root_agent::is_root_agent_target(&msg.to);
        let mut token_belongs_to_root_agent = false;
        let mut saw_session_token = false;

        if !is_master {
            if let Some(token_str) = msg.token.as_deref() {
                match self
                    .authenticate_outbox_session(app, path, msg, token_str)
                    .await?
                {
                    std::ops::ControlFlow::Continue((root, saw)) => {
                        token_belongs_to_root_agent = root;
                        saw_session_token = saw;
                    }
                    std::ops::ControlFlow::Break(result) => {
                        return Ok(std::ops::ControlFlow::Break(result))
                    }
                }
            }
        }
        Ok(std::ops::ControlFlow::Continue((
            is_master,
            root_agent_claim,
            root_agent_recipient,
            token_belongs_to_root_agent,
            saw_session_token,
        )))
    }

    async fn dispatch_outbox_self_operation<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        origin: OutboxOrigin,
        is_app_outbox: bool,
        saw_session_token: bool,
    ) -> OutboxStage {
        // ── #2232 phase 7: the Co-managed origin may express exactly one thing ──
        // A wake carrying a pointer. `action` and `command` are rejected here,
        // before the pre-routing dispatch below, because `purge-wg` and
        // `close-session` are destructive verbs and this origin is written by
        // the in-process supervisor, not by a token-bearing caller. The same
        // fields from a replica outbox keep today's behaviour untouched.
        if origin == OutboxOrigin::CoManaged && (msg.action.is_some() || msg.command.is_some()) {
            log::warn!(
                "[mailbox] Co-managed origin rejected a message carrying action={:?} command={:?} \
                msg={}",
                msg.action,
                msg.command,
                msg.id
            );
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(
                    path,
                    msg,
                    "Co-managed origin may carry only a wake; action and command are rejected",
                )
                .await,
            ));
        }

        // ── #617 self-clear: token-authorized self-operation ──
        // Dispatched BEFORE the team-routing chain below. self-clear clears the
        // CALLER'S OWN context; it is authorized solely by session-token ownership
        // (proved just above: find_by_token + from==session_name anti-spoof), not by
        // "can A reach B" team rules. Routing it through can_communicate would wrongly
        // reject valid callers in no shared team. close-session stays in the
        // post-routing action block because it IS a cross-agent operation.
        if matches!(
            msg.action.as_deref(),
            Some(SELF_CLEAR_ACTION | super::PLAIN_SELF_CLEAR_ACTION)
        ) {
            return Ok(std::ops::ControlFlow::Break(
                self.handle_self_clear(app, path, msg, is_app_outbox).await,
            ));
        }
        if msg.action.as_deref() == Some(SELF_SWITCH_ACTION) {
            return Ok(std::ops::ControlFlow::Break(
                self.handle_self_handoff_switch(app, path, msg, is_app_outbox)
                    .await,
            ));
        }
        // (#1632) Same pre-routing block, same reason: self-handoff-and-restart operates on the
        // CALLER'S OWN session and is authorized by token ownership, not by team reachability.
        if msg.action.as_deref() == Some(SELF_RESTART_ACTION) {
            return Ok(std::ops::ControlFlow::Break(
                self.handle_self_restart(app, path, msg, is_app_outbox)
                    .await,
            ));
        }
        if msg.action.as_deref() == Some(RAISE_HAND_ACTION) {
            return Ok(std::ops::ControlFlow::Break(
                self.handle_raise_hand(app, path, msg, is_app_outbox).await,
            ));
        }

        // (#885 F-7) `purge-wg` dispatches pre-routing, after the anti-spoof block.
        // `msg.from` is anti-spoof-verified ONLY inside the `if !is_master` block
        // above, so it is unverified both for a tokenless message AND for a
        // master-token message. `saw_session_token` is the single bit that proves
        // `msg.from` was checked against a live session's CWD. It is NOT disjoined
        // with `is_master`: a master-token message would sail through with an
        // attacker-chosen `msg.from`, and `verified_wg_coordinator_target` would
        // resolve against ANY workgroup on disk. Root has no workgroup.
        if msg.action.as_deref() == Some(PURGE_WG_ACTION) {
            if !saw_session_token {
                return Ok(std::ops::ControlFlow::Break(
                    self.reject_message(path, msg, "purge-room requires a session token")
                        .await,
                ));
            }
            return Ok(std::ops::ControlFlow::Break(
                self.handle_purge_wg(app, path, msg, is_app_outbox).await,
            ));
        }

        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn authorize_comanaged_outbox<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        path: &Path,
        msg: &OutboxMessage,
        root_agent_recipient: bool,
    ) -> OutboxStage {
        // #2232 phase 7: no token, no master path, no anti-spoof. The FQN in
        // `from` is the credential and the destination check is the CLI's:
        // Root through the verified-coordinator validator, everything else
        // through `can_communicate`. An unreachable destination is rejected.
        let mut paths = {
            let cfg = app.pty_settings();
            let c = cfg.read().await;
            c.project_paths.clone()
        };
        // The queue lives under a room root, whose grandparent is the project
        // directory; include it so a qualified coordinator FQN resolves even
        // when the project is not in settings (session-derived rooms).
        if let Some(project_dir) = crate::config::co_managed::room_root_for_path(path)
            .and_then(|room_root| room_root.parent().map(|ac_root| ac_root.to_path_buf()))
            .and_then(|ac_root| ac_root.parent().map(|project| project.to_path_buf()))
        {
            let canon_project = std::fs::canonicalize(&project_dir).ok();
            let already_present = paths.iter().any(|p| match &canon_project {
                Some(canon_target) => std::fs::canonicalize(p).ok().as_ref() == Some(canon_target),
                None => std::path::Path::new(p) == project_dir,
            });
            if !already_present {
                paths.push(project_dir.to_string_lossy().to_string());
            }
        }

        if root_agent_recipient {
            if let Err(reason) = validate_coordinator_to_root_route(&msg.from, &paths) {
                return Ok(std::ops::ControlFlow::Break(
                    self.reject_message(path, msg, reason).await,
                ));
            }
        } else {
            let discovered_teams = teams::discover_teams();
            if !self.can_reach(&msg.from, &msg.to, &discovered_teams) {
                log::warn!(
                    "[mailbox] Co-managed routing check FAILED: '{}' cannot reach '{}'",
                    msg.from,
                    msg.to
                );
                return Ok(std::ops::ControlFlow::Break(
                    self.reject_message(path, msg, "Sender cannot reach destination")
                        .await,
                ));
            }
        }
        log::debug!(
            "[mailbox] Co-managed routing check passed: '{}' -> '{}'",
            msg.from,
            msg.to
        );
        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn authorize_root_sender_outbox<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        path: &Path,
        msg: &OutboxMessage,
        outbox_project: &Option<String>,
        authentication: (bool, bool, bool),
    ) -> OutboxStage {
        let (is_master, saw_session_token, token_belongs_to_root_agent) = authentication;
        let mut paths = {
            let cfg = app.pty_settings();
            let c = cfg.read().await;
            c.project_paths.clone()
        };
        if let Some(root_project) = outbox_project.as_ref() {
            let canon_root_project = std::fs::canonicalize(root_project).ok();
            let already_present = paths.iter().any(|p| match &canon_root_project {
                Some(canon_target) => std::fs::canonicalize(p).ok().as_ref() == Some(canon_target),
                None => p == root_project,
            });
            if !already_present {
                paths.push(root_project.clone());
            }
        }

        if let Err(reason) = validate_root_sender_route(
            &msg.to,
            &paths,
            is_master,
            saw_session_token,
            token_belongs_to_root_agent,
        ) {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(path, msg, reason).await,
            ));
        }
        if let Err(reason) = validate_root_sender_payload(msg) {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(path, msg, &reason).await,
            ));
        }
        log::debug!(
            "[mailbox] Root Agent routing check passed: '{}' -> '{}'",
            msg.from,
            msg.to
        );
        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn authorize_root_recipient_outbox<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        path: &Path,
        msg: &OutboxMessage,
        outbox_project: &Option<String>,
    ) -> OutboxStage {
        // #293 — coordinator → root recipient.
        //
        // Build the same effective_project_paths slice the root-sender
        // branch uses (settings + the outbox file's WG-replica project
        // walk-up) so verified_wg_coordinator_target sees the project
        // where the outbox lives.
        let mut paths = {
            let cfg = app.pty_settings();
            let c = cfg.read().await;
            c.project_paths.clone()
        };
        if let Some(root_project) = outbox_project.as_ref() {
            let canon_root_project = std::fs::canonicalize(root_project).ok();
            let already_present = paths.iter().any(|p| match &canon_root_project {
                Some(canon_target) => std::fs::canonicalize(p).ok().as_ref() == Some(canon_target),
                None => p == root_project,
            });
            if !already_present {
                paths.push(root_project.clone());
            }
        }

        // Master/root token does NOT bypass the verified-coordinator check
        // for root-recipient: the URI is meaningful only when paired with
        // a real coordinator identity, and the verified check is cheap.
        if let Err(reason) = validate_coordinator_to_root_route(&msg.from, &paths) {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(path, msg, reason).await,
            ));
        }
        log::debug!(
            "[mailbox] Coordinator→Root routing check passed: '{}' -> '{}'",
            msg.from,
            msg.to
        );
        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn authorize_outbox_route<S: PtyAuthorityState + ?Sized>(
        &self,
        app: &S,
        path: &Path,
        msg: &OutboxMessage,
        origin: OutboxOrigin,
        outbox_project: &Option<String>,
        authentication: OutboxAuthenticationFacts,
    ) -> OutboxStage {
        let (
            is_master,
            root_agent_claim,
            root_agent_recipient,
            token_belongs_to_root_agent,
            saw_session_token,
        ) = authentication;
        if origin == OutboxOrigin::CoManaged {
            return self
                .authorize_comanaged_outbox(app, path, msg, root_agent_recipient)
                .await;
        } else if root_agent_claim {
            return self
                .authorize_root_sender_outbox(
                    app,
                    path,
                    msg,
                    outbox_project,
                    (is_master, saw_session_token, token_belongs_to_root_agent),
                )
                .await;
        } else if root_agent_recipient {
            return self
                .authorize_root_recipient_outbox(app, path, msg, outbox_project)
                .await;
        } else if is_master {
            log::debug!(
                "[mailbox] Master token used — bypassing team validation for {} -> {}",
                msg.from,
                msg.to
            );
        } else {
            let discovered_teams = teams::discover_teams();
            if !self.can_reach(&msg.from, &msg.to, &discovered_teams) {
                log::warn!(
                    "[mailbox] Routing check FAILED: '{}' cannot reach '{}'",
                    msg.from,
                    msg.to
                );
                return Ok(std::ops::ControlFlow::Break(
                    self.reject_message(path, msg, "Sender cannot reach destination")
                        .await,
                ));
            }
            log::debug!(
                "[mailbox] Routing check passed: '{}' -> '{}'",
                msg.from,
                msg.to
            );
        }

        Ok(std::ops::ControlFlow::Continue(()))
    }
}
