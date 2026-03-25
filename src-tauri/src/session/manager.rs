use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::session::{Session, SessionInfo, SessionStatus};
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
            created_at: chrono::Utc::now(),
            working_directory,
            status: SessionStatus::Running,
            waiting_for_input: false,
            last_prompt: None,
            git_branch: None,
            token: Uuid::new_v4(),
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
                    old.status = SessionStatus::Running;
                }
            }
        }

        // Activate the new session
        if let Some(s) = sessions.get_mut(&id) {
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

    pub async fn mark_exited(&self, id: Uuid, code: i32) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.status = SessionStatus::Exited(code);
        }
    }

    pub async fn mark_idle(&self, id: Uuid) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.waiting_for_input = true;
        }
    }

    pub async fn mark_busy(&self, id: Uuid) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.waiting_for_input = false;
        }
    }

    pub async fn set_last_prompt(&self, id: Uuid, prompt: String) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.last_prompt = Some(prompt);
        }
    }

    pub async fn set_git_branch(&self, id: Uuid, branch: Option<String>) {
        let mut sessions = self.sessions.write().await;
        if let Some(s) = sessions.get_mut(&id) {
            s.git_branch = branch;
        }
    }

    pub async fn get_sessions_directories(&self) -> Vec<(Uuid, String)> {
        let sessions = self.sessions.read().await;
        sessions
            .iter()
            .map(|(id, s)| (*id, s.working_directory.clone()))
            .collect()
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
