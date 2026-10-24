//! #1646 / #1647 - Proactive detection subsystem for terminal blocking menus
//! (e.g. folder trust dialogs).
//!
//! Scans terminal output frames periodically for known blocking menu patterns. When detected,
//! marks the session as blocked, updates the session's communication state with a warning
//! notification, and notifies the frontend so the user can be directed to the terminal.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;
use uuid::Uuid;

use crate::config::sessions_persistence::BlockedMenuPersistOutcome;
use crate::config::settings::{
    AgentConfig, BlockingMenuConfig, BlockingMenuEntry, BlockingMenusStore, SettingsState,
};
use crate::pty::manager::PtyManager;
use crate::pty::watchers::frame::{logical_rows, LogicalRow};
use crate::pty::watchers::{FrameStamp, ScreenRowsSince};
use crate::session::manager::SessionManager;

pub const ERR_MENU_GUARD_DEFERRED: &str = "menu_guard_deferred";

pub fn is_menu_guard_deferred_error(e: &str) -> bool {
    e.starts_with(ERR_MENU_GUARD_DEFERRED)
}

/// #2336 - deferral marker for the per-session typing hold. Deliberately a
/// SEPARATE marker from the menu guard, not a reuse of it: both refusals get the
/// same recoverable "not yet" treatment at every retry boundary (lease release,
/// no attempt burn), but their causes never overlap and their lifetimes differ
/// (a blocking-menu episode versus a recent human keystroke).
pub const ERR_TYPING_HOLD_DEFERRED: &str = "typing_hold_deferred";

pub fn is_typing_hold_deferred_error(e: &str) -> bool {
    e.starts_with(ERR_TYPING_HOLD_DEFERRED)
}

#[derive(Debug, Clone)]
pub struct MenuGuardSessionState {
    pub episode_id: u64,
    pub suppressed_episode_id: Option<u64>,
    pub matched_pattern: Option<String>,
    pub matched_notification: Option<String>,
    pub is_blocked: bool,
    pub last_seen_stamp: Option<FrameStamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuGuardEvaluation {
    pub is_blocked: bool,
    pub should_notify: bool,
    pub should_clear_notification: bool,
    pub matched_notification: Option<String>,
    pub matched_pattern: Option<String>,
    pub episode_id: u64,
}

pub struct MenuGuard {
    response_close: Option<crate::pty::manager::ResponseCloseState>,
    sessions: Mutex<HashMap<Uuid, MenuGuardSessionState>>,
    compiled_patterns: Mutex<HashMap<String, Result<regex::Regex, String>>>,
    next_episode_id: AtomicU64,
    store: BlockingMenusStore,
}

impl Default for MenuGuard {
    fn default() -> Self {
        Self {
            response_close: None,
            sessions: Mutex::new(HashMap::new()),
            compiled_patterns: Mutex::new(HashMap::new()),
            next_episode_id: AtomicU64::new(1),
            store: BlockingMenusStore::shipped_only(),
        }
    }
}

impl MenuGuard {
    pub(crate) fn with_response_close(
        store: BlockingMenusStore,
        state: crate::pty::manager::ResponseCloseState,
    ) -> Self {
        state.bind_publisher(crate::pty::manager::ResponseClosePublisher::Menu);
        Self {
            store,
            response_close: Some(state),
            ..Self::default()
        }
    }
    pub(crate) fn response_close_state(&self) -> Option<&crate::pty::manager::ResponseCloseState> {
        self.response_close.as_ref()
    }
    fn publish_response_menu(&self, id: Uuid, blocked: Option<bool>) {
        if let Some(state) = &self.response_close {
            state.publish_menu(id, blocked, std::time::Instant::now());
        }
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// #1905 - production constructor: the store built from the config dir at startup.
    pub fn with_store(store: BlockingMenusStore) -> Self {
        Self {
            store,
            ..Self::default()
        }
    }

    /// #1905 - the entries evaluated for one agent's sessions (D3, layer 0 first).
    pub(crate) fn entries_for(&self, agent: &AgentConfig) -> Vec<BlockingMenuEntry> {
        self.store.resolve_for(agent)
    }

    fn get_or_compile_regex(&self, pattern: &str) -> Option<regex::Regex> {
        let mut compiled = match self.compiled_patterns.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(entry) = compiled.get(pattern) {
            return entry.as_ref().ok().cloned();
        }
        match regex::Regex::new(pattern) {
            Ok(re) => {
                compiled.insert(pattern.to_string(), Ok(re.clone()));
                Some(re)
            }
            Err(e) => {
                log::error!("[menu_guard] Invalid regex pattern '{}': {}", pattern, e);
                compiled.insert(pattern.to_string(), Err(e.to_string()));
                None
            }
        }
    }

    pub fn evaluate_logical_rows(
        &self,
        session_id: Uuid,
        logical_rows: &[LogicalRow],
        entries: &[BlockingMenuEntry],
    ) -> MenuGuardEvaluation {
        let mut matched_entry: Option<&BlockingMenuConfig> = None;
        for entry in entries {
            if let Some(config) = entry.valid() {
                if config.enabled {
                    if let Some(re) = self.get_or_compile_regex(&config.pattern) {
                        if logical_rows.iter().any(|row| re.is_match(&row.text)) {
                            matched_entry = Some(config);
                            break;
                        }
                    }
                }
            }
        }

        let mut sessions_guard = match self.sessions.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let state = sessions_guard
            .entry(session_id)
            .or_insert_with(|| MenuGuardSessionState {
                episode_id: 0,
                suppressed_episode_id: None,
                matched_pattern: None,
                matched_notification: None,
                is_blocked: false,
                last_seen_stamp: None,
            });

        let evaluation = match matched_entry {
            Some(config) => {
                let is_same_pattern = state.matched_pattern.as_deref() == Some(&config.pattern);
                if !is_same_pattern {
                    state.episode_id = self.next_episode_id.fetch_add(1, Ordering::SeqCst);
                    state.suppressed_episode_id = None;
                }
                state.matched_pattern = Some(config.pattern.clone());
                state.matched_notification = Some(config.notification.clone());

                let is_suppressed = state.suppressed_episode_id == Some(state.episode_id);
                if is_suppressed {
                    state.is_blocked = false;
                    MenuGuardEvaluation {
                        is_blocked: false,
                        should_notify: false,
                        should_clear_notification: false,
                        matched_notification: state.matched_notification.clone(),
                        matched_pattern: state.matched_pattern.clone(),
                        episode_id: state.episode_id,
                    }
                } else {
                    state.is_blocked = true;
                    MenuGuardEvaluation {
                        is_blocked: true,
                        should_notify: true,
                        should_clear_notification: false,
                        matched_notification: state.matched_notification.clone(),
                        matched_pattern: state.matched_pattern.clone(),
                        episode_id: state.episode_id,
                    }
                }
            }
            None => {
                let was_matching = state.matched_pattern.is_some()
                    || state.is_blocked
                    || state.matched_notification.is_some()
                    || state.suppressed_episode_id.is_some();
                state.suppressed_episode_id = None;
                state.matched_pattern = None;
                state.matched_notification = None;
                state.is_blocked = false;
                MenuGuardEvaluation {
                    is_blocked: false,
                    should_notify: false,
                    should_clear_notification: was_matching,
                    matched_notification: None,
                    matched_pattern: None,
                    episode_id: state.episode_id,
                }
            }
        };
        self.publish_response_menu(session_id, Some(state.is_blocked));
        evaluation
    }

    pub fn resolve_current_episode(&self, session_id: Uuid) -> bool {
        let mut sessions_guard = match self.sessions.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(state) = sessions_guard.get_mut(&session_id) {
            state.suppressed_episode_id = Some(state.episode_id);
            state.is_blocked = false;
            self.publish_response_menu(session_id, Some(false));
            true
        } else {
            false
        }
    }

    pub fn is_blocked(&self, session_id: Uuid) -> bool {
        let sessions_guard = match self.sessions.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        sessions_guard
            .get(&session_id)
            .map(|s| s.is_blocked)
            .unwrap_or(false)
    }

    async fn clear_disabled_menus<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_mgr_state: &Arc<tokio::sync::RwLock<SessionManager>>,
    ) {
        let manager = { session_mgr_state.read().await.clone() };
        let disabled_current_ids = manager
            .list_sessions()
            .await
            .into_iter()
            .filter_map(|row| Uuid::parse_str(&row.id).ok())
            .collect::<Vec<_>>();
        let blocked_sessions: Vec<Uuid> = {
            let mut sessions = match self.sessions.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let mut ids = Vec::new();
            for id in &disabled_current_ids {
                self.publish_response_menu(*id, Some(false));
            }
            for (id, state) in sessions.iter_mut() {
                if state.is_blocked || state.matched_pattern.is_some() {
                    state.is_blocked = false;
                    state.matched_pattern = None;
                    state.matched_notification = None;
                    state.suppressed_episode_id = None;
                    self.publish_response_menu(*id, Some(false));
                    ids.push(*id);
                }
            }
            ids
        };
        if !blocked_sessions.is_empty() {
            let session_mgr = { session_mgr_state.read().await.clone() };
            for id in blocked_sessions {
                if crate::config::sessions_persistence::clear_blocked_menu_and_persist(
                    &session_mgr,
                    id,
                )
                .await
                {
                    crate::session::selection::publish_session_communication(app, id, None);
                }
            }
        }
    }

    pub async fn scan_tick<R: tauri::Runtime>(&self, app: &tauri::AppHandle<R>) {
        let Some(settings_state) = app.try_state::<SettingsState>() else {
            return;
        };
        let settings = match settings_state.current_read().await {
            Ok(settings) => settings.clone(),
            Err(_) => {
                log::debug!("[MenuGuard] settings unavailable; skipping scan");
                return;
            }
        };

        let Some(session_mgr_state) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        else {
            return;
        };

        if !settings.menu_guard_enabled {
            self.clear_disabled_menus(app, session_mgr_state.inner())
                .await;
            return;
        }

        let Some(pty_mgr_state) = app.try_state::<Arc<std::sync::Mutex<PtyManager>>>() else {
            return;
        };

        let session_mgr = { session_mgr_state.read().await.clone() };
        let sessions = session_mgr.list_sessions().await;

        for session in sessions {
            let Ok(session_uuid) = Uuid::parse_str(&session.id) else {
                continue;
            };
            if matches!(
                session.status,
                crate::session::session::SessionStatus::Exited(_)
            ) {
                continue;
            }

            let entries: Vec<BlockingMenuEntry> = session
                .agent_id
                .as_deref()
                .and_then(|aid| settings.agents.iter().find(|a| a.id == aid))
                .map(|agent| self.entries_for(agent))
                .unwrap_or_default();

            let last_seen_stamp = {
                let sessions_guard = match self.sessions.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                sessions_guard
                    .get(&session_uuid)
                    .and_then(|s| s.last_seen_stamp)
            };

            let screen_read = {
                let pty_guard = match pty_mgr_state.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                pty_guard.screen_rows_since(session_uuid, last_seen_stamp)
            };

            match screen_read {
                ScreenRowsSince::Unchanged => {}
                ScreenRowsSince::Frame(frame) => {
                    let stamp = frame.stamp;
                    {
                        let mut sessions_guard = match self.sessions.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        let state = sessions_guard.entry(session_uuid).or_insert_with(|| {
                            MenuGuardSessionState {
                                episode_id: 0,
                                suppressed_episode_id: None,
                                matched_pattern: None,
                                matched_notification: None,
                                is_blocked: false,
                                last_seen_stamp: None,
                            }
                        });
                        state.last_seen_stamp = stamp;
                    }
                    let rows = logical_rows(&frame);
                    let eval = self.evaluate_logical_rows(session_uuid, &rows, &entries);
                    if eval.should_notify {
                        if let Some(msg) = eval.matched_notification {
                            // #1669: the transition must reach sessions.json so a disk-reading CLI
                            // (`list-peers`) can see the blocked terminal. Only a REAL transition
                            // writes: `Unchanged` is the steady state on this 250 ms loop and does
                            // no save and no settings read. A failed save is logged inside the
                            // helper and does NOT suppress this publish; see its doc comment for
                            // why there is no rollback.
                            if let BlockedMenuPersistOutcome::Changed(comm) =
                                crate::config::sessions_persistence::set_blocked_menu_and_persist(
                                    &session_mgr,
                                    session_uuid,
                                    msg,
                                    chrono::Utc::now(),
                                    &settings,
                                )
                                .await
                            {
                                crate::session::selection::publish_session_communication(
                                    app,
                                    session_uuid,
                                    Some(&comm),
                                );
                            }
                        }
                    } else if eval.should_clear_notification
                        && crate::config::sessions_persistence::clear_blocked_menu_and_persist(
                            &session_mgr,
                            session_uuid,
                        )
                        .await
                    {
                        crate::session::selection::publish_session_communication(
                            app,
                            session_uuid,
                            None,
                        );
                    }
                }
                ScreenRowsSince::Gone => {
                    let mut sessions_guard = match self.sessions.lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    sessions_guard.remove(&session_uuid);
                    self.publish_response_menu(session_uuid, None);
                }
                ScreenRowsSince::Missing => {}
            }
        }
    }

    pub fn start(
        self: &Arc<Self>,
        app: tauri::AppHandle,
        shutdown: crate::shutdown::ShutdownSignal,
    ) {
        let this = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.token().cancelled() => break,
                    _ = interval.tick() => {
                        this.scan_tick(&app).await;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests;
