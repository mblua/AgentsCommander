use super::*;
use crate::commands::project_settings::{get_project_groups_inner, PROJECT_GROUPS_UPDATED_EVENT};
use crate::config::project_settings::{WorkgroupGroup, WorkgroupGroupsConfig};
use crate::config::settings::AppSettings;
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::web::broadcast::{WsBroadcaster, WsOutMsg};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Build a minimal `WsState` backed by `settings`, plus a broadcast receiver
/// for asserting web events emitted during dispatch.
fn ws_state_for(settings: AppSettings) -> (WsState, tokio::sync::mpsc::Receiver<WsOutMsg>) {
    let app = crate::test_support::test_builder()
        .manage(crate::session::warnings::new_session_warning_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");
    let app_handle = app.handle().clone();
    let broadcaster = WsBroadcaster::new();
    let receiver = broadcaster.subscribe();
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let idle_detector = IdleDetector::new(|_| {}, |_| {});
    let git_watcher = GitWatcher::new(Arc::clone(&session_mgr), app_handle.clone());
    let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
        Arc::new(Mutex::new(HashMap::new())),
        idle_detector,
        git_watcher,
        Some(broadcaster.clone()),
        None,
    )));
    let settings = crate::config::settings::new_settings_state(settings);
    let state = WsState {
        session_mgr,
        pty_mgr,
        settings,
        broadcaster,
        app_handle,
    };
    (state, receiver)
}

fn test_agent(id: &str) -> crate::config::settings::AgentConfig {
    crate::config::settings::AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: id.to_string(),
        command: id.to_string(),
        color: "#10b981".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }
}

/// #2306 P2 - a real `settings.json` + `settings.local.json` pair whose
/// overlay owns the top-level `agents` key, loaded through the production
/// loader so the ownership is genuine.
fn overlay_owned_agents_settings(dir: &std::path::Path) -> AppSettings {
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&json!({
            "defaultShell": "test-shell",
            "defaultShellArgs": [],
            "agents": [
                { "id": "base-a", "label": "A", "command": "claude", "color": "#111111" },
                { "id": "base-b", "label": "B", "command": "claude", "color": "#222222" }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("settings.50.personal.no-git.json"),
        serde_json::to_string_pretty(&json!({
            "agents": [
                { "id": "ov-a", "label": "A", "command": "claude", "color": "#111111", "order": 1 },
                { "id": "ov-b", "label": "B", "command": "claude", "color": "#222222" }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    crate::config::settings::load_settings_from_path(&dir.join("settings.json"))
}

mod agent_update;
mod coding_agent;
#[cfg(windows)]
mod room_idle;
mod routes;
mod transport_parity;
