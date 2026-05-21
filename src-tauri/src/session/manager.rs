use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::session::{Session, SessionInfo, SessionRepo, SessionStatus};
use super::profile::CodingAgentKind;
use crate::config::settings::WindowGeometry;
use crate::errors::AppError;

pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<Uuid, Session>>>,
    active_session: Arc<RwLock<Option<Uuid>>>,
    order: Arc<RwLock<Vec<Uuid>>>,
    next_number: Arc<RwLock<u32>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            active_session: Arc::new(RwLock::new(None)),
            order: Arc::new(RwLock::new(Vec::new())),
            next_number: Arc::new(RwLock::new(1)),
        }
    }

    // Session record is created with the full set of fields up front; splitting
    // into a builder would just defer the same parameter list.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_session(
        &self,
        shell: String,
        shell_args: Vec<String>,
        working_directory: String,
        agent_id: Option<String>,
        agent_label: Option<String>,
        git_repos: Vec<SessionRepo>,
        is_coordinator: bool,
    ) -> Result<Session, AppError> {
        let id = Uuid::new_v4();

        let mut num = self.next_number.write().await;
        let name = format!("Session {}", *num);
        *num += 1;

        let session = Session {
            id,
            name,
            shell,
            shell_args,
            effective_shell_args: None,
            created_at: chrono::Utc::now(),
            working_directory,
            status: SessionStatus::Running,
            waiting_for_input: false,
            pending_review: false,
            last_prompt: None,
            agent_id,
            agent_label,
            git_repos,
            is_coordinator,
            git_repos_gen: 0,
            token: Uuid::new_v4(),
            agent_kind: None,
            was_detached: false,
            detached_geometry: None,
        };

        self.sessions.write().await.insert(id, session.clone());
        self.order.write().await.push(id);

        // Auto-activate if no active session
        let mut active = self.active_session.write().await;
        if active.is_none() {
            *active = Some(id);
            let mut sessions = self.sessions.write().await;
            if let Some(s) = sessions.get_mut(&id) {
                s.status = SessionStatus::Active;
            }
        }

        Ok(session)
    }

    pub async fn destroy_session(&self, id: Uuid) -> Result<Option<Uuid>, AppError> {
        let mut sessions = self.sessions.write().await;
        if sessions.remove(&id).is_none() {
            return Err(AppError::SessionNotFound(id.to_string()));
        }

        let mut order = self.order.write().await;
        order.retain(|&oid| oid != id);

        let mut active = self.active_session.write().await;
        let mut new_active = None;

        if *active == Some(id) {
            // Switch to the next available session
            *active = order.first().copied();
            new_active = *active;

            if let Some(next_id) = *active {
                if let Some(s) = sessions.get_mut(&next_id) {
                    s.status = SessionStatus::Active;
                }
            }
        }

        Ok(new_active)
    }

    pub async fn switch_session(&self, id: Uuid) -> Result<(), AppError> {
        let mut sessions = self.sessions.write().await;
        if !sessions.contains_key(&id) {
            return Err(AppError::SessionNotFound(id.to_string()));
        }

        let mut active = self.active_session.write().await;

        // Deactivate the current session
        if let Some(old_id) = *active {
            if let Some(old) = sessions.get_mut(&old_id) {
                if old.status == SessionStatus::Active {
                    log::info!(
                        "[session-state] {} '{}': Active → Running (deactivated)",
                        &old_id.to_string()[..8],
                        old.name
                    );
                    old.status = SessionStatus::Running;
                }
            }
        }

        // Activate the new session
        if let Some(s) = sessions.get_mut(&id) {
            log::info!(
                "[session-state] {} '{}': {:?} → Active (switched to)",
                &id.to_string()[..8],
                s.name,
                s.status
            );
            s.status = SessionStatus::Active;
        }
        *active = Some(id);

        Ok(())
    }

    /// Set the active session WITHOUT mutating its status. Use when the target
    /// session is dormant (`Exited(_)`) and we want it "selected but not running"
    /// — e.g. the persisted-active session was deferred at startup under the
    /// issue #248 policy. The previously-active session (if any) is demoted
    /// Active → Running per the standard `switch_session` semantics. The newly-
    /// active session's status is left untouched.
    ///
    /// Use `switch_session` for the live-selection case (status flips to
    /// `Active`); use `set_active_only` for the dormant-selection case (status
    /// preserved).
    pub async fn set_active_only(&self, id: Uuid) -> Result<(), AppError> {
        let mut sessions = self.sessions.write().await;
        if !sessions.contains_key(&id) {
            return Err(AppError::SessionNotFound(id.to_string()));
        }

        let mut active = self.active_session.write().await;

        // Deactivate the current session — same demotion logic as switch_session.
        if let Some(old_id) = *active {
            if let Some(old) = sessions.get_mut(&old_id) {
                if old.status == SessionStatus::Active {
                    log::info!(
                        "[session-state] {} '{}': Active → Running (deactivated for set_active_only)",
                        &old_id.to_string()[..8],
                        old.name
                    );
                    old.status = SessionStatus::Running;
                }
            }
        }

        // Set active WITHOUT touching the new candidate's status.
        *active = Some(id);
        if let Some(s) = sessions.get_mut(&id) {
            log::info!(
                "[session-state] {} '{}': {:?} (status preserved) → selected via set_active_only",
                &id.to_string()[..8],
                s.name,
                s.status
            );
        }

        Ok(())
    }

    pub async fn rename_session(&self, id: Uuid, name: String) -> Result<(), AppError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&id)
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
        session.name = name;
        Ok(())
    }

    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.read().await;
        let order = self.order.read().await;

        order
            .iter()
            .filter_map(|id| sessions.get(id).map(SessionInfo::from))
            .collect()
    }

    pub async fn get_active(&self) -> Option<Uuid> {
        *self.active_session.read().await
    }

    /// Clear the active session and demote the previously-active session back
    /// to Running if it is still present.
    pub async fn clear_active(&self) {
        let old_id = {
            let mut active = self.active_session.write().await;
            active.take()
        };
        if let Some(old_id) = old_id {
            let mut sessions = self.sessions.write().await;
            if let Some(old) = sessions.get_mut(&old_id) {
                if old.status == SessionStatus::Active {
                    old.status = SessionStatus::Running;
                }
            }
        }
    }

    pub async fn get_session(&self, id: Uuid) -> Option<Session> {
        self.sessions.read().await.get(&id).cloned()
    }

    pub async fn get_shell(&self, id: Uuid) -> Option<String> {
        self.sessions.read().await.get(&id).map(|s| s.shell.clone())
    }

    pub async fn mark_exited(&self, id: Uuid, code: i32) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            log::info!(
                "[session-state] {} '{}': {:?} → Exited({})",
                &id.to_string()[..8],
                s.name,
                s.status,
                code
            );
            s.status = SessionStatus::Exited(code);
        }
    }

    /// Clear the active session if it matches the given ID.
    /// Used during restore to prevent deferred (Exited) sessions from
    /// blocking auto-activation of subsequent sessions.
    pub async fn clear_active_if(&self, id: Uuid) {
        let cleared_id = {
            let mut active = self.active_session.write().await;
            if *active == Some(id) {
                active.take()
            } else {
                None
            }
        };
        if let Some(old_id) = cleared_id {
            let mut sessions = self.sessions.write().await;
            if let Some(old) = sessions.get_mut(&old_id) {
                if old.status == SessionStatus::Active {
                    old.status = SessionStatus::Running;
                }
            }
        }
    }

    pub async fn mark_idle(&self, id: Uuid) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            log::info!(
                "[session-state] {} '{}': waiting_for_input {} → true",
                &id.to_string()[..8],
                s.name,
                s.waiting_for_input
            );
            s.waiting_for_input = true;
            if matches!(s.status, SessionStatus::Running) {
                s.status = SessionStatus::Idle;
            }
        }
    }

    pub async fn mark_busy(&self, id: Uuid) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            log::info!(
                "[session-state] {} '{}': waiting_for_input {} → false",
                &id.to_string()[..8],
                s.name,
                s.waiting_for_input
            );
            s.waiting_for_input = false;
            if matches!(s.status, SessionStatus::Idle) {
                s.status = SessionStatus::Running;
            }
        }
    }

    pub async fn set_last_prompt(&self, id: Uuid, prompt: String) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.last_prompt = Some(prompt);
        }
    }

    /// Set the resolved coding-agent identity. Called once by
    /// `create_session_inner` immediately after `CodingAgentKind::detect`.
    pub async fn set_agent_kind(&self, id: Uuid, kind: Option<CodingAgentKind>) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.agent_kind = kind;
        }
    }

    /// Set `was_detached` on the session. Authoritative store for persistence under
    /// Fix A (plan §A3.2). Mutated ONLY by `detach_terminal_inner` (→true) and
    /// `attach_terminal` (→false). See plan §10 rule — the `WindowEvent::Destroyed`
    /// handler must NOT call this.
    pub async fn set_was_detached(&self, id: Uuid, detached: bool) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.was_detached = detached;
        }
    }

    /// Record the detached window's last-known geometry. Called by the frontend on
    /// drag/resize via the `set_detached_geometry` Tauri command. Read at spawn
    /// time by `detach_terminal_inner` (including the Phase 3 restore path).
    pub async fn set_detached_geometry(&self, id: Uuid, geometry: WindowGeometry) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.detached_geometry = Some(geometry);
        }
    }

    /// Register the effective arg vector actually handed to portable-pty
    /// at spawn time. Called by `create_session_inner` immediately before
    /// `pty_mgr.spawn`. Idempotent — callers write the final vec once per
    /// session lifetime. Overwrites on re-call (defensive; not expected in
    /// normal flow).
    pub async fn set_effective_shell_args(&self, id: Uuid, args: Vec<String>) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.effective_shell_args = Some(args);
        }
    }

    /// Overwrite `git_repos` atomically. Bumps `git_repos_gen`. Invariant:
    /// callers preserve insertion order (replica config.json `repos` array order).
    pub async fn set_git_repos(&self, id: Uuid, repos: Vec<SessionRepo>) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.git_repos = repos;
            s.git_repos_gen = s.git_repos_gen.wrapping_add(1);
        }
    }

    /// Compare-and-swap variant for the watcher. Only writes if `expected_gen` still
    /// matches `git_repos_gen`. On mismatch a concurrent refresh has landed; the watcher
    /// discards its stale detection to prevent emit reordering (see §2.1.d / Grinch #14).
    /// Returns true on successful write.
    pub async fn set_git_repos_if_gen(
        &self,
        id: Uuid,
        repos: Vec<SessionRepo>,
        expected_gen: u64,
    ) -> bool {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            if s.git_repos_gen == expected_gen {
                s.git_repos = repos;
                s.git_repos_gen = s.git_repos_gen.wrapping_add(1);
                return true;
            }
        }
        false
    }

    /// Snapshot the current `git_repos_gen` for a session. Used by watchers to capture
    /// generation at the start of a poll so `set_git_repos_if_gen` can detect a race.
    pub async fn get_git_repos_gen(&self, id: Uuid) -> Option<u64> {
        let sessions = self.sessions.read().await;
        sessions.get(&id).map(|s| s.git_repos_gen)
    }

    /// Overwrite `is_coordinator`. Use after a team-config refresh.
    pub async fn set_is_coordinator(&self, id: Uuid, is_coordinator: bool) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.is_coordinator = is_coordinator;
        }
    }

    /// Recompute `is_coordinator` for every session using the current team snapshot.
    /// Returns the list of (session_id, new_value) pairs whose flag actually changed,
    /// so callers can emit a single event batch.
    pub async fn refresh_coordinator_flags(
        &self,
        teams: &[crate::config::teams::DiscoveredTeam],
    ) -> Vec<(Uuid, bool)> {
        let mut sessions = self.sessions.write().await;
        let mut changes = Vec::new();
        for (id, s) in sessions.iter_mut() {
            let new_val = crate::config::teams::is_coordinator_for_cwd(&s.working_directory, teams);
            if s.is_coordinator != new_val {
                s.is_coordinator = new_val;
                changes.push((*id, new_val));
            }
        }
        changes
    }

    /// Replace `git_repos` for sessions whose name matches. Bumps `git_repos_gen` on every
    /// write so an in-flight `GitWatcher::poll` that captured the pre-refresh snapshot
    /// cannot overwrite us (see §2.1.d / Grinch #14).
    /// Returns the list of (session_id, new_repos) pairs where a write actually happened.
    pub async fn refresh_git_repos_for_sessions(
        &self,
        updates: &[(String, Vec<SessionRepo>)],
    ) -> Vec<(Uuid, Vec<SessionRepo>)> {
        let mut sessions = self.sessions.write().await;
        let mut changed = Vec::new();
        for (name, repos) in updates {
            if let Some((id, s)) = sessions.iter_mut().find(|(_, s)| &s.name == name) {
                if &s.git_repos != repos {
                    s.git_repos = repos.clone();
                    s.git_repos_gen = s.git_repos_gen.wrapping_add(1);
                    changed.push((*id, repos.clone()));
                }
            }
        }
        changed
    }

    /// Per-session view for the `GitWatcher` fan-out. Returns (session_id, repos, gen).
    /// The generation snapshot lets the watcher call `set_git_repos_if_gen` for its
    /// write, skipping the write+emit if a refresh landed during detection.
    pub async fn get_sessions_repos(&self) -> Vec<(Uuid, Vec<SessionRepo>, u64)> {
        let sessions = self.sessions.read().await;
        sessions
            .iter()
            .map(|(id, s)| (*id, s.git_repos.clone(), s.git_repos_gen))
            .collect()
    }

    /// (session_id, working_directory) view for callers that only need the CWD
    /// (e.g. mailbox outbox scanning, agent-name resolution).
    pub async fn get_sessions_working_dirs(&self) -> Vec<(Uuid, String)> {
        let sessions = self.sessions.read().await;
        sessions
            .iter()
            .map(|(id, s)| (*id, s.working_directory.clone()))
            .collect()
    }

    /// Find a session by its display name. Returns its UUID if found.
    pub async fn find_by_name(&self, name: &str) -> Option<Uuid> {
        let sessions = self.sessions.read().await;
        sessions
            .iter()
            .find(|(_, s)| s.name == name)
            .map(|(id, _)| *id)
    }

    /// Find a session by its authentication token. Linear scan — fine for 10-20 sessions.
    pub async fn find_by_token(&self, token: Uuid) -> Option<SessionInfo> {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .find(|s| s.token == token)
            .map(SessionInfo::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_effective_shell_args_writes_field() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude-mb".to_string(),
                vec!["--dangerously-skip-permissions".to_string()],
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
            )
            .await
            .expect("create_session should succeed");

        assert!(session.effective_shell_args.is_none());

        let effective = vec![
            "--dangerously-skip-permissions".to_string(),
            "--continue".to_string(),
        ];
        mgr.set_effective_shell_args(session.id, effective.clone())
            .await;

        let stored = mgr
            .get_session(session.id)
            .await
            .expect("session should still exist");
        assert_eq!(stored.effective_shell_args, Some(effective));
    }

    #[tokio::test]
    async fn set_effective_shell_args_no_op_on_missing_session() {
        let mgr = SessionManager::new();
        let missing = Uuid::new_v4();
        mgr.set_effective_shell_args(missing, vec!["--continue".to_string()])
            .await;
        assert!(mgr.get_session(missing).await.is_none());
    }

    #[tokio::test]
    async fn set_effective_shell_args_overwrites_on_recall() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude-mb".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
            )
            .await
            .expect("create_session should succeed");

        mgr.set_effective_shell_args(session.id, vec!["--continue".to_string()])
            .await;
        mgr.set_effective_shell_args(
            session.id,
            vec!["--continue".to_string(), "--debug".to_string()],
        )
        .await;

        let stored = mgr.get_session(session.id).await.unwrap();
        assert_eq!(
            stored.effective_shell_args,
            Some(vec!["--continue".to_string(), "--debug".to_string()])
        );
    }

    #[tokio::test]
    async fn clear_active_removes_active_and_demotes_status() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp".to_string(),
                None,
                None,
                Vec::new(),
                false,
            )
            .await
            .expect("create_session should succeed");

        assert_eq!(mgr.get_active().await, Some(session.id));
        mgr.clear_active().await;

        assert_eq!(mgr.get_active().await, None);
        let stored = mgr.get_session(session.id).await.unwrap();
        assert_eq!(stored.status, SessionStatus::Running);
    }

    #[tokio::test]
    async fn clear_active_if_preserves_non_matching_active_session() {
        let mgr = SessionManager::new();
        let first = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp\\one".to_string(),
                None,
                None,
                Vec::new(),
                false,
            )
            .await
            .expect("create first session");
        let second = mgr
            .create_session(
                "powershell.exe".to_string(),
                Vec::new(),
                "C:\\tmp\\two".to_string(),
                None,
                None,
                Vec::new(),
                false,
            )
            .await
            .expect("create second session");

        mgr.switch_session(second.id)
            .await
            .expect("switch to second session");
        mgr.clear_active_if(first.id).await;

        assert_eq!(mgr.get_active().await, Some(second.id));
        let first_stored = mgr.get_session(first.id).await.unwrap();
        let second_stored = mgr.get_session(second.id).await.unwrap();
        assert_eq!(first_stored.status, SessionStatus::Running);
        assert_eq!(second_stored.status, SessionStatus::Active);
    }

    // ── Issue #248 — set_active_only (Fix A) ──

    #[tokio::test]
    async fn set_active_only_preserves_dormant_status() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "claude".into(),
                vec![],
                "C:\\proj".into(),
                None,
                None,
                vec![],
                true,
            )
            .await
            .unwrap();
        // After create_session, the session is auto-activated (status = Active);
        // call mark_exited to put it in the dormant state under test.
        mgr.mark_exited(session.id, 0).await;
        // clear_active_if to drop the now-stale active pointer.
        mgr.clear_active_if(session.id).await;
        assert_eq!(mgr.get_active().await, None);

        // The behavior under test: select the dormant session without flipping
        // its status.
        mgr.set_active_only(session.id).await.unwrap();
        assert_eq!(mgr.get_active().await, Some(session.id));
        let s = mgr.get_session(session.id).await.unwrap();
        assert!(matches!(s.status, SessionStatus::Exited(0))); // PRESERVED, not Active
    }

    #[tokio::test]
    async fn set_active_only_demotes_previously_active() {
        let mgr = SessionManager::new();
        let live = mgr
            .create_session("c".into(), vec![], "C:\\a".into(), None, None, vec![], false)
            .await
            .unwrap();
        // First session auto-activates → status = Active, active_session = live.id
        assert_eq!(mgr.get_active().await, Some(live.id));
        let live_state = mgr.get_session(live.id).await.unwrap();
        assert_eq!(live_state.status, SessionStatus::Active);

        // Create + mark-exited a second session for the dormant-select scenario.
        let dormant = mgr
            .create_session("c".into(), vec![], "C:\\b".into(), None, None, vec![], false)
            .await
            .unwrap();
        mgr.mark_exited(dormant.id, 0).await;

        mgr.set_active_only(dormant.id).await.unwrap();

        // Active pointer moved.
        assert_eq!(mgr.get_active().await, Some(dormant.id));
        // Previously-active demoted: Active → Running.
        let live_after = mgr.get_session(live.id).await.unwrap();
        assert_eq!(live_after.status, SessionStatus::Running);
        // New active preserved as Exited.
        let dormant_after = mgr.get_session(dormant.id).await.unwrap();
        assert!(matches!(dormant_after.status, SessionStatus::Exited(0)));
    }

    #[tokio::test]
    async fn set_active_only_returns_session_not_found_for_unknown_id() {
        let mgr = SessionManager::new();
        let bogus = uuid::Uuid::new_v4();
        let err = mgr.set_active_only(bogus).await.unwrap_err();
        assert!(matches!(err, AppError::SessionNotFound(_)));
        // Active pointer untouched.
        assert_eq!(mgr.get_active().await, None);
    }

    // ── Issue #248 / Grinch Z9 — defer + set_active_only + list_sessions chain ──

    #[tokio::test]
    async fn issue_248_defer_set_active_only_list_sessions_chain() {
        let mgr = SessionManager::new();
        // Simulate the defer arm of lib.rs §3.4: create a session, mark_exited,
        // clear_active_if.
        let session = mgr
            .create_session(
                "claude".into(),
                vec![],
                "C:\\proj\\.ac-new\\_agent_architect".into(),
                Some("aid".into()),
                Some("Architect".into()),
                vec![],
                true, // is_coordinator
            )
            .await
            .unwrap();
        mgr.mark_exited(session.id, 0).await;
        mgr.clear_active_if(session.id).await;

        // Simulate the post-loop active-switch (§3.7) with the dormant branch.
        mgr.set_active_only(session.id).await.unwrap();

        // The wire payload (what list_sessions IPC returns to the frontend).
        let infos = mgr.list_sessions().await;
        assert_eq!(infos.len(), 1);
        let json = serde_json::to_value(&infos[0]).unwrap();

        // The critical assertion — Round-2 Z1 blocker.
        // Before Fix A, this would be `"status":"active"` and the FE would
        // render the live dot, taking the wrong click path. With Fix A, status
        // round-trips as the object form for SessionStatus::Exited.
        assert_eq!(json["status"], serde_json::json!({ "exited": 0 }));

        // Active pointer correctly reflects the selection.
        assert_eq!(mgr.get_active().await, Some(session.id));
    }

    /// #260 G1 — pins `mark_idle`'s contract: the terminal mutation the
    /// idle-detector `on_idle` callback performs. NOTE: `create_session`
    /// auto-activates the first session (status `Active`), and `mark_idle`
    /// only transitions `Running → Idle` — so demote via `clear_active` first.
    /// Without that step the status assertion below would be vacuous.
    #[tokio::test]
    async fn mark_idle_sets_waiting_for_input_and_running_to_idle() {
        let mgr = SessionManager::new();
        let session = mgr
            .create_session(
                "codex".into(),
                vec![],
                "C:\\proj".into(),
                None,
                None,
                vec![],
                false,
            )
            .await
            .unwrap();
        mgr.clear_active().await; // Active → Running
        let before = mgr.get_session(session.id).await.unwrap();
        assert_eq!(before.status, SessionStatus::Running);
        assert!(!before.waiting_for_input);

        mgr.mark_idle(session.id).await;

        let after = mgr.get_session(session.id).await.unwrap();
        assert!(
            after.waiting_for_input,
            "mark_idle must set waiting_for_input = true"
        );
        assert_eq!(
            after.status,
            SessionStatus::Idle,
            "mark_idle must transition Running → Idle"
        );
    }
}
