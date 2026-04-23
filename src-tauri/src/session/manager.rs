use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::session::{Session, SessionInfo, SessionRepo, SessionStatus};
use crate::errors::AppError;

pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<Uuid, Session>>>,
    active_session: Arc<RwLock<Option<Uuid>>>,
    order: Arc<RwLock<Vec<Uuid>>>,
    next_number: Arc<RwLock<u32>>,
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
            is_claude: false,
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
        let mut active = self.active_session.write().await;
        if *active == Some(id) {
            *active = None;
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

    pub async fn set_is_claude(&self, id: Uuid, val: bool) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.is_claude = val;
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
            let new_val =
                crate::config::teams::is_coordinator_for_cwd(&s.working_directory, teams);
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
}
