use super::*;
use super::{
    api_server_probe_addr, api_server_status, build_web_server_owned_status,
    classify_virtual_interface, ensure_web_remote_open_allowed, is_tcp_socket_listening,
    map_web_server_interfaces, mint_api_client_with_path, move_coding_agent_command_with_saver,
    move_coding_agent_inner_with_saver, persist_coding_agent_env_settings_update,
    persist_coding_agent_profiles_update, persist_narrow_settings_update_with_saver,
    persist_protected_settings_update_with_saver, persist_settings_draft_update_with_saver,
    purge_sessions_after_settings_update_in_dir, reorder_coding_agent_command_with_saver,
    reorder_coding_agent_inner_with_saver, resolve_web_server_owned_status,
    select_current_bind_failure, set_rail_collapse_inner_with_saver, settings_snapshot_from,
    start_api_server, stop_web_server_handle, web_remote_url, web_server_probe_addr,
    WebServerOwnershipState, AGENT_ORDER_OVERLAY_PINNED, MINT_API_CLIENT_DEFAULT_TTL_HOURS,
    MINT_API_CLIENT_MAX_TTL_DAYS, MINT_API_CLIENT_NOTE,
};
#[cfg(windows)]
use super::{build_profile_assignment_target, canonical_compare_key};
use crate::api::auth;
use crate::config::coding_agent_mutations::AgentMoveDirection;
use crate::config::coding_agent_profiles::SelectionWriteIntent;
use crate::config::sessions_persistence::{session_retention_project_paths, PersistedSession};
use crate::config::settings::{
    AgentConfig, AppSettings, CodingAgentEnv, CodingAgentEnvSource, MainWindowDisplayState,
    ProfileCellConfig, SettingsState, WindowGeometry,
};
use crate::session::manager::SessionManager;
use crate::{
    ApiServerHandle, ApiServerTask, WebServerHandle, WebServerLifecycle, WEB_SERVER_START_CANCELLED,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio_util::sync::CancellationToken;

// ── #1077 SettingsSnapshot / resolution report ───────────────────────
fn settings_with_agents(ids: &[&str]) -> AppSettings {
    AppSettings {
        agents: ids
            .iter()
            .enumerate()
            .map(|(index, id)| agent_with_order(id, Some(index as u32)))
            .collect(),
        ..AppSettings::default()
    }
}

/// Base `settings.json` + `settings.local.json` whose local overlay owns the
/// top-level `agents` key; returns the base path. The caller loads through
/// the production loader so ownership is genuine.
fn seed_overlay_agents_fixture(dir: &Path, local_agents: &Value) -> PathBuf {
    let path = dir.join("settings.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "defaultShell": "test-shell",
            "defaultShellArgs": [],
            "rootToken": "base-token",
            "agents": [
                { "id": "base-a", "label": "Base A", "command": "claude", "color": "#111111" },
                { "id": "base-b", "label": "Base B", "command": "claude", "color": "#222222" }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("settings.50.personal.no-git.json"),
        serde_json::to_string_pretty(&json!({ "agents": local_agents })).unwrap(),
    )
    .unwrap();
    path
}

fn agent_with_order(id: &str, order: Option<u32>) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: format!("{id}-label"),
        command: format!("{id}-cmd"),
        color: "#000000".to_string(),
        order,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    }
}

mod snapshot;

fn settings_with_single_agent() -> AppSettings {
    AppSettings {
        agents: vec![AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "agent-0".to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: "Codex".to_string(),
            command: "codex".to_string(),
            color: "#000000".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }],
        ..AppSettings::default()
    }
}

#[test]
#[cfg(windows)]
fn canonical_compare_key_converts_verbatim_unc() {
    assert_eq!(
        canonical_compare_key(Path::new(r"\\?\UNC\server\share\Repo")),
        "//server/share/repo"
    );
}

#[test]
#[cfg(windows)]
fn profile_assignment_target_paths_convert_verbatim_unc() {
    let live_by_cwd = BTreeMap::new();
    let identity = crate::config::replica_identity::WgReplicaIdentity {
        agent_name: "dev".to_string(),
        ac_root: PathBuf::from(r"\\?\UNC\server\share\repo\.ac"),
        matrix_dir: PathBuf::from(r"\\?\UNC\server\share\repo\.ac\_agent_dev"),
        identity: "../../_agent_dev".to_string(),
    };

    let target = build_profile_assignment_target(
        Path::new(r"\\?\UNC\server\share\repo\.ac\wg-1\__agent_dev"),
        &identity,
        &crate::config::coding_agent_profiles::ReplicaSelectionState::Unlocked {
            configuration_ref: None,
            profile_override_source: None,
            identity: "../../_agent_dev".to_string(),
            pair: None,
            warning: None,
        },
        &live_by_cwd,
    );

    assert_eq!(target.workgroup_path, r"\\server\share\repo\.ac\wg-1");
    assert_eq!(
        target.replica_path,
        r"\\server\share\repo\.ac\wg-1\__agent_dev"
    );
}

fn state_for(settings: AppSettings) -> SettingsState {
    crate::config::settings::new_settings_state(settings)
}

fn write_settings_file(dir: &Path, settings: &AppSettings) {
    let json = serde_json::to_vec_pretty(settings).expect("serialize settings");
    std::fs::write(dir.join("settings.json"), json).expect("write settings.json");
}

fn write_sessions_file(dir: &Path, sessions: &[PersistedSession]) {
    let json = serde_json::to_vec_pretty(sessions).expect("serialize sessions");
    std::fs::write(dir.join("sessions.json"), json).expect("write sessions.json");
}

fn read_sessions_file(dir: &Path) -> Vec<PersistedSession> {
    let json = std::fs::read_to_string(dir.join("sessions.json")).expect("read sessions.json");
    serde_json::from_str(&json).expect("parse sessions.json")
}

fn settings_payload_without_keys(settings: &AppSettings, keys: &[&str]) -> AppSettings {
    let mut value = serde_json::to_value(settings).expect("serialize settings payload");
    let object = value
        .as_object_mut()
        .expect("settings payload must serialize to an object");
    for key in keys {
        assert!(
            object.remove(*key).is_some(),
            "settings payload has no `{key}` key to strip; the A7 gate would be a no-op"
        );
    }
    serde_json::from_value(value).expect("deserialize stale settings payload")
}

fn assert_single_project(settings: &AppSettings, project: &str) {
    assert_eq!(settings.project_paths, vec![project.to_string()]);
    assert_eq!(settings.project_path.as_deref(), Some(project));
}

/// #1077: create a real AC project dir and return its display-canonical path,
/// so the six-field decoder validates it and the SELECTED runtime path (in the
/// preserving writer's fresh-decoded return) equals the stored string.
fn real_ac_project(parent: &Path, name: &str) -> String {
    let dir = parent.join(name);
    std::fs::create_dir_all(dir.join(".ac")).unwrap();
    canonical_display(&dir)
}

/// #1077: the display-canonical form of an existing directory - what the
/// six-field decoder selects. Storing this instead of the raw temp path keeps
/// runtime-path assertions platform-robust (equal to raw on Windows, the
/// symlink-resolved canonical on Linux/CI, which the preserving writer publishes).
fn canonical_display(dir: &Path) -> String {
    crate::config::projects::display_canonical(
        &std::fs::canonicalize(dir).unwrap().to_string_lossy(),
    )
}

/// #1077: seed a whole, valid AppSettings object on disk with `keys` removed
/// (to simulate an absent-key scenario without tripping the preserve writer's
/// whole-object gate).
fn write_settings_file_without_keys(dir: &Path, settings: &AppSettings, keys: &[&str]) {
    let mut value = serde_json::to_value(settings).expect("serialize settings");
    let object = value.as_object_mut().expect("settings object");
    for key in keys {
        object.remove(*key);
    }
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_vec_pretty(&value).expect("serialize settings json"),
    )
    .expect("write settings.json");
}

// ─────────────────────────────────────────────────────────────────────────
// #1737 D14 - the two payload-constructed `AppSettings` carry-overs, and D13's
// narrowing on the three other production writers of `codingAgentProfiles`.
// Plan section 11, P1 to P7.
// ─────────────────────────────────────────────────────────────────────────
mod local_overlay_1737;

/// #2133 (P4) - every test injects `<tempdir>/settings.json`; none resolves the config dir.
mod agent_help_2133;

mod agent_order;
mod api_server;
mod coding_agent_install;
mod e2432_p02_source_api_tests;
mod hash_activation;
mod issue_3083_tests;
mod p02_wire_schema_tests;
mod placement;
mod profile_assignment;
mod project_reconcile;
mod room_idle_confirmation;
mod selection_api;
mod settings_draft;
mod settings_update;
mod web_server;
