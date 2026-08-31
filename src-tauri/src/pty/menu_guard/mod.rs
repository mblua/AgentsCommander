//! #1646 / #1647 - Proactive detection subsystem for terminal blocking menus (e.g. folder trust dialogs).
//!
//! Scans terminal output frames periodically for known blocking menu patterns. When detected,
//! marks the session as blocked, updates the session's communication state with a warning notification,
//! and notifies the frontend so the user can be directed to the terminal.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;
use uuid::Uuid;

use crate::config::settings::{BlockingMenuConfig, BlockingMenuEntry, SettingsState};
use crate::pty::manager::PtyManager;
use crate::pty::watchers::frame::{logical_rows, LogicalRow};
use crate::pty::watchers::{FrameStamp, ScreenRowsSince};
use crate::session::manager::SessionManager;

pub const ERR_MENU_GUARD_DEFERRED: &str = "menu_guard_deferred";

pub fn is_menu_guard_deferred_error(e: &str) -> bool {
    e.starts_with(ERR_MENU_GUARD_DEFERRED)
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
    sessions: Mutex<HashMap<Uuid, MenuGuardSessionState>>,
    compiled_patterns: Mutex<HashMap<String, Result<regex::Regex, String>>>,
    next_episode_id: AtomicU64,
}

impl Default for MenuGuard {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            compiled_patterns: Mutex::new(HashMap::new()),
            next_episode_id: AtomicU64::new(1),
        }
    }
}

impl MenuGuard {
    pub fn new() -> Self {
        Self::default()
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

        match matched_entry {
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
        }
    }

    pub fn resolve_current_episode(&self, session_id: Uuid) -> bool {
        let mut sessions_guard = match self.sessions.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(state) = sessions_guard.get_mut(&session_id) {
            state.suppressed_episode_id = Some(state.episode_id);
            state.is_blocked = false;
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

    pub async fn scan_tick<R: tauri::Runtime>(&self, app: &tauri::AppHandle<R>) {
        let Some(settings_state) = app.try_state::<SettingsState>() else {
            return;
        };
        let settings = settings_state.read().await.clone();

        let Some(session_mgr_state) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        else {
            return;
        };

        if !settings.menu_guard_enabled {
            let blocked_sessions: Vec<Uuid> = {
                let mut sessions = match self.sessions.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                let mut ids = Vec::new();
                for (id, state) in sessions.iter_mut() {
                    if state.is_blocked || state.matched_pattern.is_some() {
                        state.is_blocked = false;
                        state.matched_pattern = None;
                        state.matched_notification = None;
                        state.suppressed_episode_id = None;
                        ids.push(*id);
                    }
                }
                ids
            };
            if !blocked_sessions.is_empty() {
                let session_mgr = session_mgr_state.read().await;
                for id in blocked_sessions {
                    if session_mgr.clear_blocked_menu(id).await {
                        crate::session::selection::publish_session_communication(app, id, None);
                    }
                }
            }
            return;
        }

        let Some(pty_mgr_state) = app.try_state::<Arc<std::sync::Mutex<PtyManager>>>() else {
            return;
        };

        let session_mgr = session_mgr_state.read().await;
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
                .and_then(|agent| agent.blocking_menus.clone())
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
                            if let Some((true, comm)) = session_mgr
                                .set_blocked_menu(session_uuid, msg, chrono::Utc::now())
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
                        && session_mgr.clear_blocked_menu(session_uuid).await
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
mod tests {
    use super::*;
    use crate::config::settings::default_blocking_menus_for_command;

    #[test]
    fn test_menu_guard_pattern_matching() {
        let guard = MenuGuard::new();
        let session_id = Uuid::new_v4();

        let pi_entries = default_blocking_menus_for_command("pi");
        let codex_entries = default_blocking_menus_for_command("codex");

        let pi_row = vec![LogicalRow {
            start: 0,
            end: 0,
            text: "  Trust project folder? (y/n)".to_string(),
        }];
        let eval_pi = guard.evaluate_logical_rows(session_id, &pi_row, &pi_entries);
        assert!(eval_pi.is_blocked);
        assert!(eval_pi.should_notify);
        assert!(guard.is_blocked(session_id));
        assert_eq!(
            eval_pi.matched_notification.as_deref(),
            Some("pi is waiting for you to answer the folder-trust menu in this terminal")
        );

        let codex_session_id = Uuid::new_v4();
        let codex_row = vec![LogicalRow {
            start: 0,
            end: 0,
            text: "Do you trust the contents of this directory? [Y/n]".to_string(),
        }];
        let eval_codex = guard.evaluate_logical_rows(codex_session_id, &codex_row, &codex_entries);
        assert!(eval_codex.is_blocked);
        assert!(eval_codex.should_notify);
        assert!(guard.is_blocked(codex_session_id));
        assert_eq!(
            eval_codex.matched_notification.as_deref(),
            Some("codex is waiting for you to answer the folder-trust menu in this terminal")
        );
    }

    #[test]
    fn test_menu_guard_episode_suppression_and_rearm() {
        let guard = MenuGuard::new();
        let session_id = Uuid::new_v4();
        let entries = default_blocking_menus_for_command("pi");

        let menu_rows = vec![LogicalRow {
            start: 0,
            end: 0,
            text: "Trust project folder?".to_string(),
        }];
        let clean_rows = vec![LogicalRow {
            start: 0,
            end: 0,
            text: "Normal output".to_string(),
        }];

        // 1. First appearance -> blocked
        let eval1 = guard.evaluate_logical_rows(session_id, &menu_rows, &entries);
        assert!(eval1.is_blocked);
        assert!(eval1.should_notify);
        assert!(guard.is_blocked(session_id));
        let episode_1 = eval1.episode_id;

        // 2. User resolves the episode
        let resolved = guard.resolve_current_episode(session_id);
        assert!(resolved);
        assert!(!guard.is_blocked(session_id));

        // 3. Same menu still visible on next tick -> suppressed, not blocked
        let eval2 = guard.evaluate_logical_rows(session_id, &menu_rows, &entries);
        assert!(!eval2.is_blocked);
        assert!(!eval2.should_notify);
        assert!(!guard.is_blocked(session_id));
        assert_eq!(eval2.episode_id, episode_1);

        // 4. Menu disappears -> clears state, returns should_clear_notification
        let eval3 = guard.evaluate_logical_rows(session_id, &clean_rows, &entries);
        assert!(!eval3.is_blocked);
        assert!(eval3.should_clear_notification);
        assert!(!guard.is_blocked(session_id));

        // 5. Menu reappears -> new episode, re-armed and blocked!
        let eval4 = guard.evaluate_logical_rows(session_id, &menu_rows, &entries);
        assert!(eval4.is_blocked);
        assert!(eval4.should_notify);
        assert!(guard.is_blocked(session_id));
        assert_ne!(eval4.episode_id, episode_1);
    }

    #[test]
    fn test_menu_guard_invalid_regex_tolerance() {
        let guard = MenuGuard::new();
        let session_id = Uuid::new_v4();

        let entries = vec![
            BlockingMenuEntry::Valid(BlockingMenuConfig {
                pattern: "[unclosed regex".to_string(),
                notification: "bad regex".to_string(),
                enabled: true,
                captured_against: None,
            }),
            BlockingMenuEntry::Valid(BlockingMenuConfig {
                pattern: "valid match".to_string(),
                notification: "good regex".to_string(),
                enabled: true,
                captured_against: None,
            }),
        ];

        let rows = vec![LogicalRow {
            start: 0,
            end: 0,
            text: "some valid match text".to_string(),
        }];

        // Should skip invalid regex without panic and match the valid one
        let eval = guard.evaluate_logical_rows(session_id, &rows, &entries);
        assert!(eval.is_blocked);
        assert_eq!(eval.matched_notification.as_deref(), Some("good regex"));
    }
}
