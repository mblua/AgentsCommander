use super::*;
use crate::config::coding_agent_profiles::SelectionWriteIntent;

#[tokio::test]
async fn room_idle_confirmation_writer_receipts_follow_written_effective() {
    for writer in ["whole", "draft"] {
        for (candidate_seconds, written_seconds, changed) in [
            (30, 30, true),
            (15, 15, false),
            (15, 60, true),
            (30, 15, false),
        ] {
            let current = AppSettings {
                room_idle_confirmation_seconds: 15,
                ..AppSettings::default()
            };
            let state = crate::config::settings::new_settings_state(current.clone());
            let payload = AppSettings {
                room_idle_confirmation_seconds: candidate_seconds,
                ..current
            };
            let stage = std::cell::Cell::new(0);
            let saver = |candidate: &AppSettings| {
                assert!(
                    state.try_write().is_err(),
                    "saver must run under writer lock"
                );
                assert_eq!(candidate.room_idle_confirmation_seconds, candidate_seconds);
                assert_eq!(stage.replace(1), 0);
                let mut written = candidate.clone();
                written.response_close_enabled = false;
                written.response_close_idle_seconds = written_seconds;
                written.room_idle_confirmation_seconds = written_seconds;
                Ok(written)
            };
            let publish = |written: &AppSettings| {
                assert!(
                    state.try_read().is_err(),
                    "publication precedes reader visibility"
                );
                assert_eq!(stage.replace(2), 1, "publication follows successful save");
                assert!(!written.response_close_enabled);
                assert_eq!(written.response_close_idle_seconds, written_seconds);
            };
            let (saved, receipt) = match writer {
                "whole" => super::persist_protected_settings_update_with_saver_and_publication(
                    &state, payload, saver, publish,
                )
                .await
                .unwrap(),
                "draft" => {
                    let (saved, events) =
                        super::persist_settings_draft_update_with_saver_and_publication(
                            &state, payload, saver, publish,
                        )
                        .await
                        .unwrap();
                    assert!(!events.profiles_changed);
                    assert!(events.env_agent_ids.is_empty());
                    (saved, events.room_idle_confirmation_changed)
                }
                _ => unreachable!(),
            };
            assert_eq!(stage.get(), 2);
            let live = state.read().await;
            assert!(!live.response_close_enabled);
            assert_eq!(live.response_close_idle_seconds, written_seconds);
            drop(live);
            assert_eq!(
                receipt, changed,
                "{writer}/{candidate_seconds}/{written_seconds}"
            );
            assert_eq!(saved.room_idle_confirmation_seconds, written_seconds);
            assert_eq!(
                state.read().await.room_idle_confirmation_seconds,
                written_seconds
            );
        }
    }
}

#[tokio::test]
async fn room_idle_confirmation_writer_failure_never_adopts() {
    for writer in ["whole", "draft"] {
        let state = crate::config::settings::new_settings_state(AppSettings::default());
        let before = serde_json::to_value(&*state.read().await).unwrap();
        let mut payload = state.read().await.clone();
        payload.room_idle_confirmation_seconds = 30;
        let saver = |_: &AppSettings| -> Result<AppSettings, String> {
            assert!(state.try_write().is_err());
            Err("injected I/O failure".into())
        };
        let publish = |_: &AppSettings| panic!("failed save must not publish");
        let error = match writer {
            "whole" => super::persist_protected_settings_update_with_saver_and_publication(
                &state, payload, saver, publish,
            )
            .await
            .unwrap_err(),
            "draft" => super::persist_settings_draft_update_with_saver_and_publication(
                &state, payload, saver, publish,
            )
            .await
            .unwrap_err(),
            _ => unreachable!(),
        };
        assert_eq!(error, "injected I/O failure");
        assert_eq!(serde_json::to_value(&*state.read().await).unwrap(), before);
    }
}

#[cfg(windows)]
static ROOM_IDLE_CONFIRMATION_HOTKEY: Mutex<(Vec<String>, bool)> = Mutex::new((Vec::new(), false));

#[cfg(windows)]
fn room_idle_confirmation_register_hotkey(
    _: &tauri::AppHandle,
    hotkey: &str,
) -> Result<(), String> {
    let mut state = ROOM_IDLE_CONFIRMATION_HOTKEY.lock().unwrap();
    state.0.push(hotkey.to_string());
    if state.1 {
        Err("fixture refusal".into())
    } else {
        Ok(())
    }
}

// Real persistence and both in-memory transports, isolated before config_dir's OnceLock.
#[cfg(windows)]
fn room_idle_confirmation_receipts() -> Vec<Value> {
    let mut rows = Vec::new();
    for writer in ["update_settings", "save_settings_draft"] {
        for (case, success, seconds, count) in [
            ("changed", true, 30, 1),
            ("unchanged", true, 30, 0),
            ("invalid-range", false, 30, 0),
            ("malformed-present", false, 30, 0),
            ("renewed-positive", true, 60, 1),
            ("hotkey-error", true, 90, 1),
        ] {
            for transport in ["tauri", "browser"] {
                rows.push(
                    json!({"module": "commands::config", "writer": writer, "case": case,
                    "transport": transport, "eventCount": count, "success": success,
                    "diskSeconds": seconds, "stateSeconds": seconds}),
                );
            }
        }
    }
    rows
}

#[cfg(windows)]
#[test]
fn room_idle_confirmation_routes_parent() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let root = match std::env::var_os("AC_ROOM_IDLE_TEST_EVIDENCE_DIR") {
        Some(value) if !value.is_empty() => {
            let path = std::path::PathBuf::from(value);
            assert!(
                path.is_absolute(),
                "evidence override must be absolute: {}",
                path.display()
            );
            path
        }
        _ => std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/room-idle-confirmation"),
    };
    std::fs::create_dir_all(&root)
        .unwrap_or_else(|error| panic!("create {}: {error}", root.display()));
    let run = tempfile::Builder::new()
        .prefix("native-")
        .tempdir_in(&root)
        .expect("create retained run directory")
        .keep();
    eprintln!("room idle confirmation evidence: {}", run.display());
    let fixture = run.join("fixture");
    std::fs::create_dir(&fixture).unwrap();
    let fixture = fixture.canonicalize().unwrap();
    let receipt = fixture.parent().unwrap().join("receipts.json");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "commands::config::tests::room_idle_confirmation_routes_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("AC_ROOM_IDLE_TEST_CHILD", &fixture)
        .env("AGENTSCOMMANDER_CONFIG_DIR", &fixture)
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", &fixture)
        .env("AC_ROOM_IDLE_TEST_RECEIPT", &receipt)
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(run.join("stdout.log")).unwrap())
        .stderr(std::fs::File::create(run.join("stderr.log")).unwrap())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn child in {}: {error}", run.display()));
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            break status;
        }
        if Instant::now() >= deadline {
            let killed = child.kill();
            let waited = child.wait();
            panic!(
                "child timeout; evidence {}; kill={killed:?}; wait={waited:?}",
                run.display()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        status.success(),
        "child {status}; evidence {}",
        run.display()
    );
    let bytes = std::fs::read(&receipt)
        .unwrap_or_else(|error| panic!("missing {}: {error}", receipt.display()));
    let rows: Vec<Value> = serde_json::from_slice(&bytes).expect("complete parsed receipts");
    assert_eq!(rows.len(), 24);
    // Exact ordered keys and rows reject missing, extra or duplicate case/transport evidence.
    assert_eq!(rows, room_idle_confirmation_receipts());
}

#[cfg(windows)]
fn room_idle_confirmation_fixture() -> Option<std::path::PathBuf> {
    let marker = std::env::var_os("AC_ROOM_IDLE_TEST_CHILD")?;
    let fixture = std::path::PathBuf::from(marker);
    assert!(fixture.is_absolute());
    assert_eq!(fixture.canonicalize().unwrap(), fixture);
    for key in [
        "AGENTSCOMMANDER_CONFIG_DIR",
        "AGENTSCOMMANDER_TEST_CONFIG_DIR",
    ] {
        let override_path = std::path::PathBuf::from(std::env::var_os(key).expect(key));
        assert_eq!(override_path, fixture);
        assert_eq!(override_path.canonicalize().unwrap(), fixture);
    }
    assert_eq!(
        crate::config::config_dir().unwrap().canonicalize().unwrap(),
        fixture
    );
    assert_eq!(
        std::path::PathBuf::from(std::env::var_os("AC_ROOM_IDLE_TEST_RECEIPT").unwrap()),
        fixture.parent().unwrap().join("receipts.json")
    );
    Some(fixture)
}

#[cfg(windows)]
fn room_idle_confirmation_fixture_bytes(
    fixture: &std::path::Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut bytes = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(fixture).unwrap() {
        let path = entry.unwrap().path();
        assert!(!path.is_symlink());
        if path.is_dir() {
            bytes.extend(room_idle_confirmation_fixture_bytes(&path));
        } else {
            bytes.insert(path.clone(), std::fs::read(path).unwrap());
        }
    }
    bytes
}

#[cfg(windows)]
fn room_idle_confirmation_browser_events(
    receiver: &mut tokio::sync::mpsc::Receiver<crate::web::broadcast::WsOutMsg>,
) -> Vec<Value> {
    let mut events = Vec::new();
    while let Ok(message) = receiver.try_recv() {
        let crate::web::broadcast::WsOutMsg::Text(text) = message else {
            panic!("unexpected binary frame");
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        if value["event"] == "room_idle_confirmation_updated" {
            assert_eq!(
                value,
                json!({"event":"room_idle_confirmation_updated", "payload":{}})
            );
            events.push(value);
        }
    }
    events
}

#[cfg(windows)]
async fn room_idle_confirmation_case(
    app: &tauri::AppHandle,
    settings: &SettingsState,
    writer: &str,
    case: &str,
    fixture: &std::path::Path,
    receiver: &mut tokio::sync::mpsc::Receiver<crate::web::broadcast::WsOutMsg>,
) -> Vec<Value> {
    use tauri::Listener;

    let (_, target, expected, success, count) = [
        ("changed", 30, 30, true, 1),
        ("unchanged", 30, 30, true, 0),
        ("invalid-range", 0, 30, false, 0),
        ("malformed-present", 60, 30, false, 0),
        ("renewed-positive", 60, 60, true, 1),
        ("hotkey-error", 90, 90, true, 1),
    ]
    .into_iter()
    .find(|(id, ..)| *id == case)
    .expect("unknown case");
    let path = crate::config::settings::settings_path().expect("isolated settings path");
    let valid = std::fs::read(&path).unwrap();
    if case == "malformed-present" {
        std::fs::write(&path, b"{ malformed-present").unwrap();
    }
    let before = room_idle_confirmation_fixture_bytes(fixture);
    let live_before = serde_json::to_value(&*settings.read().await).unwrap();
    let mut payload = settings.read().await.clone();
    payload.room_idle_confirmation_seconds = target;
    let (tx, rx) = std::sync::mpsc::channel();
    let observed_settings = settings.clone();
    let sessions_path = fixture.join("sessions.json");
    let listener = app.listen_any("room_idle_confirmation_updated", move |event| {
        let adopted = observed_settings
            .try_read()
            .map(|guard| guard.room_idle_confirmation_seconds)
            .ok();
        tx.send((
            event.payload().to_string(),
            adopted,
            std::fs::read(&sessions_path).unwrap(),
        ))
        .unwrap();
    });
    *ROOM_IDLE_CONFIRMATION_HOTKEY.lock().unwrap() = (Vec::new(), case == "hotkey-error");
    let (hotkey_tx, hotkey_rx) = std::sync::mpsc::channel();
    let hotkey_listener = app.listen_any("screenshot_capture_failed", move |event| {
        hotkey_tx.send(event.payload().to_string()).unwrap();
    });
    let result = match writer {
        "update_settings" => {
            super::update_settings_with_hotkey_registration(
                app.clone(),
                settings,
                payload.clone(),
                room_idle_confirmation_register_hotkey,
            )
            .await
        }
        "save_settings_draft" => {
            super::save_settings_draft_with_hotkey_registration(
                app.clone(),
                settings,
                payload.clone(),
                room_idle_confirmation_register_hotkey,
            )
            .await
        }
        _ => panic!("unknown writer"),
    };
    let succeeded = result.is_ok();
    app.unlisten(listener);
    app.unlisten(hotkey_listener);
    let calls = ROOM_IDLE_CONFIRMATION_HOTKEY.lock().unwrap().0.clone();
    let expected_calls = if success {
        vec![payload.screenshot_capture_hotkey.clone()]
    } else {
        Vec::new()
    };
    let hotkey_errors: Vec<_> = hotkey_rx.try_iter().collect();
    assert_eq!(
        (succeeded, calls, hotkey_errors.len()),
        (success, expected_calls, usize::from(case == "hotkey-error")),
        "{writer}/{case}: save outcome, hotkey calls, refusal events"
    );
    for error in hotkey_errors {
        assert_eq!(
            serde_json::from_str::<Value>(&error).unwrap(),
            json!({
                "message":"Screenshot hotkey was saved but could not be registered: fixture refusal"
            })
        );
    }
    let native: Vec<_> = rx.try_iter().collect();
    assert_eq!(native.len(), count, "{writer}/{case}: tauri");
    for (payload, adopted, sessions) in native {
        assert_eq!(
            (
                serde_json::from_str::<Value>(&payload).unwrap(),
                adopted,
                sessions,
            ),
            (json!({}), Some(expected), b"[]".to_vec()),
            "{writer}/{case}: payload, adopted state after lock release, sessions"
        );
    }
    assert_eq!(
        room_idle_confirmation_browser_events(receiver).len(),
        count,
        "{writer}/{case}: browser"
    );
    assert_eq!(
        settings.read().await.room_idle_confirmation_seconds,
        expected
    );
    if !success {
        assert_eq!(room_idle_confirmation_fixture_bytes(fixture), before);
        assert_eq!(
            serde_json::to_value(&*settings.read().await).unwrap(),
            live_before
        );
    }
    if case == "malformed-present" {
        assert_eq!(std::fs::read(&path).unwrap(), b"{ malformed-present");
        std::fs::write(&path, valid).unwrap();
    }
    let loaded = crate::config::settings::load_settings_from_path(&path);
    assert_eq!(
        (
            loaded.room_idle_confirmation_seconds,
            loaded.agents.is_empty(),
            loaded.project_paths.is_empty(),
        ),
        (expected, true, true),
        "{writer}/{case}: isolated persisted settings"
    );
    assert_eq!(std::fs::read(fixture.join("sessions.json")).unwrap(), b"[]");
    ["tauri", "browser"].into_iter().map(|transport| json!({
        "module":"commands::config", "writer":writer, "case":case, "transport":transport,
        "eventCount":count, "success":success, "diskSeconds":expected, "stateSeconds":expected
    })).collect()
}

#[cfg(windows)]
#[tokio::test]
async fn room_idle_confirmation_routes_child() {
    let Some(fixture) = room_idle_confirmation_fixture() else {
        return;
    };
    let settings = crate::config::settings::new_settings_state(AppSettings {
        agents: Vec::new(),
        project_paths: Vec::new(),
        project_path: None,
        archived_project_paths: Vec::new(),
        ..AppSettings::default()
    });
    let broadcaster = crate::web::broadcast::WsBroadcaster::new();
    let mut receiver = broadcaster.subscribe();
    let app = crate::test_support::test_builder()
        .manage(settings.clone())
        .manage(broadcaster)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build isolated app");
    let mut receipts = Vec::new();
    for writer in ["update_settings", "save_settings_draft"] {
        let seed = AppSettings {
            agents: Vec::new(),
            project_paths: Vec::new(),
            project_path: None,
            archived_project_paths: Vec::new(),
            room_idle_confirmation_seconds: 15,
            ..AppSettings::default()
        };
        let saved = crate::config::settings::save_settings(&seed).expect("seed isolated settings");
        assert!(saved.agents.is_empty());
        assert!(saved.project_paths.is_empty());
        *settings.write().await = saved;
        std::fs::write(fixture.join("sessions.json"), b"[]").unwrap();
        for case in [
            "changed",
            "unchanged",
            "invalid-range",
            "malformed-present",
            "renewed-positive",
            "hotkey-error",
        ] {
            receipts.extend(
                room_idle_confirmation_case(
                    app.handle(),
                    &settings,
                    writer,
                    case,
                    &fixture,
                    &mut receiver,
                )
                .await,
            );
        }
    }
    assert_eq!(receipts, room_idle_confirmation_receipts());
    std::fs::write(
        fixture.parent().unwrap().join("receipts.json"),
        serde_json::to_vec_pretty(&receipts).unwrap(),
    )
    .unwrap();
}

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
mod snapshot;

fn hash_activation_env(value: &str) -> Vec<CodingAgentEnv> {
    vec![CodingAgentEnv {
        key: "MODE".into(),
        value: value.into(),
        source: CodingAgentEnvSource::User,
        enabled: false,
    }]
}

fn hash_activation_state() -> (tempfile::TempDir, PathBuf, SettingsState) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut settings = settings_with_single_agent();
    settings.root_token = Some("hash-test-token".into());
    settings.agents[0].envs = hash_activation_env("live");
    crate::config::settings::normalize_agent_order(&mut settings.agents);
    crate::config::settings::validate_and_repair_settings(&mut settings).unwrap();
    settings
        .coding_agent_profiles
        .codex_home_key_by_agent
        .insert("agent-0".into(), "physical-home".into());
    let mut cell = crate::config::settings::empty_profile_cell();
    cell.command = "--profile-b".into();
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .insert("B".into(), cell);
    settings
        .coding_agent_profiles
        .default_profile_by_agent
        .insert("agent-0".into(), "B".into());
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "agent-0".into(),
            BTreeMap::from([("B".into(), "Profile sentinel".into())]),
        );
    // HOME carrier is backend-owned physical state. A caller-only value
    // is intentionally removed by the existing saver when disk has none.
    std::fs::write(&path, serde_json::to_vec_pretty(&settings).unwrap()).unwrap();
    let written =
        crate::config::settings::save_settings_to_path_preserving_project_paths(&settings, &path)
            .unwrap();
    assert_eq!(
        written
            .coding_agent_profiles
            .codex_home_key_by_agent
            .get("agent-0")
            .map(String::as_str),
        Some("physical-home")
    );
    (
        temp,
        path,
        crate::config::settings::new_settings_state(written),
    )
}

fn hash_activation_assert_written(
    written: &AppSettings,
    reload: &AppSettings,
    expected_envs: &[CodingAgentEnv],
    path: &Path,
) {
    let row = &written.agents[0];
    let physical: Value = serde_json::from_slice(
        &std::fs::read(
            path.with_file_name(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
        )
        .unwrap(),
    )
    .unwrap();
    let physical_agent: AgentConfig =
        serde_json::from_value(physical["agents"][0].clone()).unwrap();
    assert_eq!(
        physical_agent.id_hash,
        Some(crate::config::settings::configuration_identity(
            &physical_agent.command,
            &physical_agent.envs
        ))
    );
    assert_eq!(physical_agent.envs, expected_envs);
    let mut expected_debug = vec![row.command.clone()];
    for env in expected_envs {
        expected_debug.extend([env.key.clone(), env.value.clone()]);
    }
    assert_eq!(row.id_hash_debug, Some(expected_debug.clone()));
    assert_eq!(physical_agent.id_hash_debug, Some(expected_debug));
    assert_eq!(row.id, "agent-0");
    assert_eq!(row.order, Some(0));
    assert_eq!(
        written.coding_agent_profiles.default_profile_by_agent["agent-0"],
        "B"
    );
    assert_eq!(
        written.coding_agent_profiles.profile_labels_by_agent["agent-0"]["B"],
        "Profile sentinel"
    );
    assert_eq!(row.envs, expected_envs);
    assert_eq!(
        row.id_hash,
        Some(crate::config::settings::configuration_identity(
            &row.command,
            expected_envs
        ))
    );
    assert_eq!(
        serde_json::to_value(&reload.agents).unwrap(),
        serde_json::to_value(&written.agents).unwrap()
    );
    assert_eq!(
        written.coding_agent_profiles.codex_home_key_by_agent["agent-0"],
        "physical-home"
    );
}

#[tokio::test]
async fn id_hash_activation_protected_whole_uses_restored_live_env_and_real_saver() {
    let (_temp, path, state) = hash_activation_state();
    let original = state.read().await.clone();
    let mut incoming = original.clone();
    incoming.agents[0].command = "codex extra-character".into();
    incoming.agents[0].envs = hash_activation_env("client-stale");
    incoming.agents[0].id_hash = Some("forged".into());
    incoming
        .coding_agent_profiles
        .codex_home_key_by_agent
        .clear();
    let (written, _) =
        persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();
    let reload = crate::config::settings::load_settings_from_path(&path);
    hash_activation_assert_written(&written, &reload, &original.agents[0].envs, &path);
    assert_eq!(
        serde_json::to_value(&state.read().await.agents).unwrap(),
        serde_json::to_value(&written.agents).unwrap()
    );
    assert_ne!(written.agents[0].id_hash, original.agents[0].id_hash);
    let mut expected_profiles = original.coding_agent_profiles.profiles_by_agent.clone();
    for (letter, cell) in expected_profiles.get_mut("agent-0").unwrap() {
        let mut debug = vec![written.agents[0].command.clone()];
        for env in &original.agents[0].envs {
            debug.extend([env.key.clone(), env.value.clone()]);
        }
        debug.push(cell.command.clone());
        for (key, value) in &cell.env {
            debug.extend([key.clone(), value.clone()]);
        }
        cell.id_hash_debug = Some(debug);
        // Independent preimage from the final command, restored live env and
        // the original payload. Disabled env rows still participate.
        let mut preimage = written.agents[0].command.clone();
        for env in &original.agents[0].envs {
            preimage.push_str(&env.key);
            preimage.push_str(&env.value);
        }
        preimage.push_str(&cell.command);
        for (key, value) in &cell.env {
            preimage.push_str(key);
            preimage.push_str(value);
        }
        let mut chars: Vec<_> = preimage
            .chars()
            .flat_map(char::to_lowercase)
            .filter(|c| !c.is_whitespace())
            .collect();
        chars.sort_unstable();
        cell.id_hash = Some(format!(
            "cid1:{:x}",
            <super::Sha256 as super::Digest>::digest(chars.iter().collect::<String>())
        ));
        assert_eq!(
            written.coding_agent_profiles.profiles_by_agent["agent-0"][letter].id_hash,
            cell.id_hash
        );
    }
    assert_eq!(
        written.coding_agent_profiles.profiles_by_agent,
        expected_profiles
    );
    let before = serde_json::to_value(&original).unwrap();
    let after = serde_json::to_value(&written).unwrap();
    for key in ["quotaSources", "projectPaths", "rootToken"] {
        assert_eq!(after[key], before[key]);
    }
}

#[tokio::test]
async fn id_hash_activation_draft_keeps_edited_env_and_existing_events() {
    let (_temp, path, state) = hash_activation_state();
    let mut draft = state.read().await.clone();
    draft.agents[0].envs = hash_activation_env("draft-edited");
    draft.agents[0].id_hash = Some("forged".into());
    let (written, events) = persist_settings_draft_update_with_saver(&state, draft, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(candidate, &path)
    })
    .await
    .unwrap();
    let reload = crate::config::settings::load_settings_from_path(&path);
    hash_activation_assert_written(
        &written,
        &reload,
        &hash_activation_env("draft-edited"),
        &path,
    );
    assert_eq!(
        serde_json::to_value(&state.read().await.agents).unwrap(),
        serde_json::to_value(&written.agents).unwrap()
    );
    assert_eq!(events.env_agent_ids, vec!["agent-0".to_string()]);
    assert!(!events.profiles_changed);
}

#[tokio::test]
async fn id_hash_activation_narrow_env_uses_explicit_replacement_and_written() {
    let (_temp, path, state) = hash_activation_state();
    let mut captured = None;
    super::persist_coding_agent_env_settings_update_with_saver(
        &state,
        "agent-0",
        hash_activation_env("narrow-replacement"),
        true,
        |candidate| {
            let written = crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )?;
            captured = Some(written.clone());
            Ok(written)
        },
    )
    .await
    .unwrap();
    let written = captured.unwrap();
    let reload = crate::config::settings::load_settings_from_path(&path);
    hash_activation_assert_written(
        &written,
        &reload,
        &hash_activation_env("narrow-replacement"),
        &path,
    );
    assert_eq!(
        serde_json::to_value(&state.read().await.agents).unwrap(),
        serde_json::to_value(&written.agents).unwrap()
    );
    assert!(written.agents[0].isolated_home);
    let physical: Value = serde_json::from_slice(
        &std::fs::read(
            path.with_file_name(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        physical["agents"][0]["id_hash"],
        written.agents[0].id_hash.clone().unwrap()
    );
}

#[tokio::test]
async fn id_hash_activation_failed_real_save_never_adopts_whole_draft_or_narrow() {
    for route in ["whole", "draft", "narrow"] {
        let (_temp, path, state) = hash_activation_state();
        let before = serde_json::to_value(&*state.read().await).unwrap();
        let mut payload = state.read().await.clone();
        payload.agents[0].command = "codex edited".into();
        let sidecar =
            path.with_file_name(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME);
        std::fs::write(&sidecar, b"{ malformed").unwrap();
        let saver = |candidate: &AppSettings| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        };
        let failed = match route {
            "whole" => persist_protected_settings_update_with_saver(&state, payload, saver)
                .await
                .is_err(),
            "draft" => persist_settings_draft_update_with_saver(&state, payload, saver)
                .await
                .is_err(),
            _ => super::persist_coding_agent_env_settings_update_with_saver(
                &state,
                "agent-0",
                hash_activation_env("requested"),
                true,
                saver,
            )
            .await
            .is_err(),
        };
        assert!(failed, "{route}");
        assert_eq!(
            serde_json::to_value(&*state.read().await).unwrap(),
            before,
            "{route}"
        );
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"{ malformed");
    }
}

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

#[tokio::test]
async fn welcome_status_command_2736_returns_err_for_an_unavailable_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let state = state_for(AppSettings {
        project_paths: vec![temp.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    });
    let catalog_err = super::coding_agent_catalog_inner(&state)
        .await
        .expect_err("no persisted catalog");
    let welcome_err = super::coding_agent_welcome_status_inner(&state)
        .await
        .expect_err("no persisted catalog");
    assert_eq!(welcome_err, catalog_err);
}

#[tokio::test]
async fn welcome_status_command_2736_releases_the_settings_lock_before_filesystem_work() {
    let state = state_for(AppSettings::default());
    let probe = state.clone();
    let mut observed = None;
    // The loader runs AT the filesystem step; `try_write` succeeds only if
    // no settings read guard is alive at that moment.
    let rows = super::coding_agent_welcome_status_inner_with(&state, |_| {
        observed = Some(probe.try_write().is_ok());
        Ok(Vec::new())
    })
    .await;
    assert_eq!(
        observed,
        Some(true),
        "settings lock held during filesystem work"
    );
    assert_eq!(rows, Ok(Vec::new()));
}

fn install_fixture_entry_2736(
    key: &str,
    install_commands: Option<crate::config::coding_agents_catalog::InstallCommands>,
) -> crate::config::coding_agents_catalog::CodingAgentDefinition {
    crate::config::coding_agents_catalog::CodingAgentDefinition {
        key: key.to_string(),
        label: key.to_string(),
        description: String::new(),
        color: String::new(),
        command: key.to_string(),
        instructions_filename: None,
        envs: Vec::new(),
        isolated_home: false,
        config_seed: None,
        removable: true,
        update_commands: Vec::new(),
        auto_update: false,
        idle_burst: None,
        install_commands,
    }
}

fn install_mock_app_2736() -> tauri::App {
    crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app")
}

#[tokio::test]
async fn install_coding_agent_2736_rejects_an_unknown_key() {
    let app = install_mock_app_2736();
    let state = state_for(AppSettings::default());
    let result = super::install_coding_agent_inner_with(
        app.handle(),
        &state,
        "nope-2736".to_string(),
        |_| Ok(vec![install_fixture_entry_2736("claude", None)]),
    )
    .await;
    assert_eq!(result, Err("unknown coding agent 'nope-2736'".to_string()));
}

#[tokio::test]
async fn install_coding_agent_2736_rejects_a_key_with_no_install_command() {
    let app = install_mock_app_2736();
    let state = state_for(AppSettings::default());
    let result = super::install_coding_agent_inner_with(
        app.handle(),
        &state,
        "bare-2736".to_string(),
        |_| Ok(vec![install_fixture_entry_2736("bare-2736", None)]),
    )
    .await;
    assert_eq!(
        result,
        Err("no install command is available for 'bare-2736' on this platform".to_string())
    );
}

#[tokio::test]
async fn install_coding_agent_2736_reports_catalog_unavailability_verbatim() {
    let app = install_mock_app_2736();
    let temp = tempfile::tempdir().unwrap();
    let state = state_for(AppSettings {
        project_paths: vec![temp.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    });
    let catalog_err = super::coding_agent_catalog_inner(&state)
        .await
        .expect_err("no persisted catalog");
    let install_err = super::install_coding_agent_inner(app.handle(), &state, "claude".to_string())
        .await
        .expect_err("no persisted catalog");
    assert_eq!(install_err, catalog_err);
}

#[test]
fn install_coding_agent_2736_resolves_the_cwd_with_the_updater_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let with_project = AppSettings {
        project_paths: vec![temp.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    assert_eq!(super::install_cwd_for(&with_project), temp.path());

    let fallback = crate::config::config_dir().expect("test config dir");
    assert_eq!(super::install_cwd_for(&AppSettings::default()), fallback);
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

fn api_server_command_test_app(settings: AppSettings) -> tauri::App {
    let session_mgr = Arc::new(RwLock::new(SessionManager::new()));
    let store_dir = tempfile::tempdir().expect("create API store directory");
    let message_store = Arc::new(
        crate::api::message_store::MessageStore::open(
            store_dir
                .path()
                .join(crate::api::message_store::DB_FILENAME),
        )
        .expect("open API message store"),
    );
    let app = crate::test_support::test_builder()
        .manage(ApiServerHandle::default())
        .manage(crate::api::message_store::MessageStoreState::ready(
            message_store,
        ))
        .manage(store_dir)
        .manage(state_for(settings))
        .manage(Arc::clone(&session_mgr))
        .manage(crate::shutdown::ShutdownSignal::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let git_watcher =
        crate::pty::git_watcher::GitWatcher::new(Arc::clone(&session_mgr), app.handle().clone());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new(
        Arc::new(Mutex::new(HashMap::new())),
        idle_detector,
        git_watcher,
        None,
        None,
    )));
    app.manage(pty_mgr);
    app
}

#[test]
fn web_remote_url_maps_wildcards_and_preserves_concrete_ips() {
    let cases = [
        (
            "0.0.0.0",
            "http://127.0.0.1:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "::",
            "http://[::1]:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "0:0:0:0:0:0:0:0",
            "http://[::1]:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "0::0",
            "http://[::1]:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "127.0.0.1",
            "http://127.0.0.1:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "192.168.1.50",
            "http://192.168.1.50:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "::1",
            "http://[::1]:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "2001:db8::25",
            "http://[2001:db8::25]:8888/?window=browser&remoteToken=test-token",
        ),
    ];

    for (bind, expected) in cases {
        assert_eq!(web_remote_url(bind, 8888, "test-token").unwrap(), expected);
    }
}

#[test]
fn web_remote_url_rejects_invalid_bind() {
    let error = web_remote_url("not-an-ip", 8888, "test-token").unwrap_err();

    assert!(error.starts_with("Invalid web server bind address 'not-an-ip':"));
}

#[test]
fn api_server_probe_addr_maps_wildcards_to_loopback() {
    assert_eq!(
        api_server_probe_addr("0.0.0.0", 9906).unwrap().to_string(),
        "127.0.0.1:9906"
    );
    assert_eq!(
        api_server_probe_addr("127.0.0.1", 9906)
            .unwrap()
            .to_string(),
        "127.0.0.1:9906"
    );
    assert_eq!(
        api_server_probe_addr("::", 9906).unwrap().to_string(),
        "[::1]:9906"
    );
    assert_eq!(
        api_server_probe_addr("::1", 9906).unwrap().to_string(),
        "[::1]:9906"
    );
}

#[tokio::test]
async fn api_server_probe_addr_detects_wildcard_listener_via_loopback() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .expect("bind wildcard test listener");
    let port = listener.local_addr().expect("listener local addr").port();
    let probe_addr = api_server_probe_addr("0.0.0.0", port).expect("probe addr");

    assert_eq!(probe_addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert!(is_tcp_socket_listening(probe_addr).await);
}

#[test]
fn web_server_probe_addr_maps_wildcards_to_loopback() {
    assert_eq!(
        web_server_probe_addr("0.0.0.0", 8765).unwrap(),
        "127.0.0.1:8765".parse().unwrap()
    );
    assert_eq!(
        web_server_probe_addr("[::]", 8766).unwrap(),
        "[::1]:8766".parse().unwrap()
    );
    assert_eq!(
        web_server_probe_addr("192.168.1.50", 8767).unwrap(),
        "192.168.1.50:8767".parse().unwrap()
    );
    assert_eq!(
        web_server_probe_addr("[2001:db8::25]", 8768).unwrap(),
        "[2001:db8::25]:8768".parse().unwrap()
    );
}

#[tokio::test]
async fn web_server_probe_addr_detects_wildcard_listener_via_loopback() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .expect("bind wildcard web listener");
    let port = listener.local_addr().expect("wildcard address").port();
    let probe_addr = web_server_probe_addr("0.0.0.0", port).expect("web probe addr");

    assert_eq!(probe_addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert!(is_tcp_socket_listening(probe_addr).await);
}

#[tokio::test]
async fn api_server_status_reports_running_for_managed_wildcard_server() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .expect("bind wildcard test listener");
    let bound_addr = listener.local_addr().expect("listener local addr");
    let settings = AppSettings {
        api_server_bind: "0.0.0.0".to_string(),
        api_server_port: bound_addr.port(),
        ..AppSettings::default()
    };
    let app = tauri::test::mock_builder()
        .manage(ApiServerHandle::default())
        .manage(state_for(settings))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");

    let shutdown = CancellationToken::new();
    let task_shutdown = shutdown.clone();
    let join = tauri::async_runtime::spawn(async move {
        let _listener = listener;
        task_shutdown.cancelled().await;
    });
    app.state::<ApiServerHandle>()
        .store_if_idle(ApiServerTask::new(join, shutdown, bound_addr))
        .expect("store api server task");

    let running = api_server_status(app.state::<ApiServerHandle>(), app.state::<SettingsState>())
        .await
        .expect("status succeeds");

    assert!(running);
    assert!(app
        .state::<ApiServerHandle>()
        .shutdown_running(Duration::from_secs(1))
        .await
        .expect("shutdown stored task"));
}

#[tokio::test]
async fn start_api_server_bind_failure_returns_err_and_status_false() {
    let settings = AppSettings {
        api_server_bind: "192.0.2.1".to_string(),
        api_server_port: 9906,
        ..AppSettings::default()
    };
    let app = api_server_command_test_app(settings);

    let err = start_api_server(
        app.handle().clone(),
        app.state::<ApiServerHandle>(),
        app.state::<SettingsState>(),
        app.state::<Arc<RwLock<SessionManager>>>(),
        app.state::<Arc<Mutex<crate::pty::manager::PtyManager>>>(),
        app.state::<crate::shutdown::ShutdownSignal>(),
    )
    .await
    .expect_err("nonlocal bind should fail readiness");

    assert!(err.contains("API server bind failed"), "{err}");
    assert!(!app.state::<ApiServerHandle>().has_running().unwrap());
    assert!(
        !api_server_status(app.state::<ApiServerHandle>(), app.state::<SettingsState>())
            .await
            .expect("status succeeds")
    );
}

/// #1768 fix case. The port is held continuously from reservation to service, so nothing can
/// take it in between. **This is the discriminating test**: section 7.3 runs it against the
/// unfixed shape (mutant M1) and records it red, then against the fixed shape and records it
/// green.
#[tokio::test]
async fn api_start_server_adopts_the_prebound_listener_and_never_releases_the_port() {
    let app = api_server_command_test_app(AppSettings::default());
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve loopback port");
    let port = listener
        .local_addr()
        .expect("reserved loopback address")
        .port();
    // Mutation tripwire. This duplicate handle keeps the port bound even if the
    // implementation drops the listener it was handed, so a drop-then-rebind implementation
    // fails readiness instead of quietly passing.
    let keeper = listener.try_clone().expect("duplicate reserved listener");

    let session_mgr = app.state::<Arc<RwLock<SessionManager>>>();
    let pty_mgr = app.state::<Arc<Mutex<crate::pty::manager::PtyManager>>>();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let start = crate::api::start_server_on_listener(
        listener,
        app.handle().clone(),
        session_mgr.inner().clone(),
        pty_mgr.inner().clone(),
        shutdown.clone(),
    );

    let addr = crate::api::wait_for_startup_ready(start.readiness)
        .await
        .expect("prebound listener adopted");
    assert_eq!(addr.port(), port, "server must serve the reserved port");

    // The port assertion and a bare `TcpStream::connect` are BOTH satisfied by an
    // implementation that keeps `keeper`'s port bound and serves Axum on a different socket:
    // `keeper` is a listening socket, so the OS completes the TCP handshake into its backlog
    // even though nothing ever accepts. Only a timed application-level response proves that
    // THIS server is the thing answering on the reserved address, so that is what is asserted.
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("healthz client");
    let url = format!("http://{}/api/v1/healthz", addr);
    let body = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        client
            .get(&url)
            .send()
            .await
            .expect("healthz request")
            .text()
            .await
            .expect("healthz body")
    })
    .await
    .expect("healthz deadline on the reserved address");
    assert_eq!(
        body, "{\"ok\":true}",
        "the reserved address must be served by this server"
    );

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), start.join_handle)
        .await
        .expect("API server shutdown deadline")
        .expect("API server task");
    drop(keeper);
}

/// #1768 readiness-error **contract** test for the unchanged `Address` path. It pins the
/// wording and the shape of the bind-failure readiness error that CI logs show. It is NOT a
/// control for this change: it exercises only the `Address` path, which this plan does not
/// alter, so it passes identically on the unfixed and the fixed code. Section 7.3 M1 is the
/// red-before / green-after evidence; this test is not.
#[tokio::test]
async fn api_start_server_address_path_bind_failure_readiness_contract() {
    let app = api_server_command_test_app(AppSettings::default());
    let adversary = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("adversary loopback port");
    let port = adversary
        .local_addr()
        .expect("adversary loopback address")
        .port();

    let session_mgr = app.state::<Arc<RwLock<SessionManager>>>();
    let pty_mgr = app.state::<Arc<Mutex<crate::pty::manager::PtyManager>>>();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let start = crate::api::start_server(
        "127.0.0.1".to_string(),
        port,
        app.handle().clone(),
        session_mgr.inner().clone(),
        pty_mgr.inner().clone(),
        shutdown.clone(),
    );

    let err = crate::api::wait_for_startup_ready(start.readiness)
        .await
        .expect_err("a taken port must fail readiness");
    assert!(err.contains("API server bind failed"), "{err}");
    assert!(err.contains(&port.to_string()), "{err}");

    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), start.join_handle)
        .await
        .expect("API server shutdown deadline")
        .expect("API server task");
    drop(adversary);
}

struct MintApiClientFixture {
    _temp: tempfile::TempDir,
    path: PathBuf,
    project: PathBuf,
    settings: AppSettings,
    root: String,
    expected_fqn: String,
}

fn mint_api_client_fixture() -> MintApiClientFixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("proj-a");
    let ac_root = project.join(".ac");
    let matrix = ac_root.join("_agent_alice");
    let replica = ac_root.join("wg-1-devs").join("__agent_alice");
    std::fs::create_dir_all(&matrix).expect("create matrix");
    std::fs::create_dir_all(&replica).expect("create replica");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_alice"}"#,
    )
    .expect("write replica config");

    let root_path = std::fs::canonicalize(&replica).expect("canonical replica");
    let root = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&root_path);
    let expected_fqn = crate::config::teams::agent_fqn_from_path(&root);
    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().to_string()],
        ..AppSettings::default()
    };

    MintApiClientFixture {
        path: temp.path().join(auth::REGISTRY_FILENAME),
        _temp: temp,
        project,
        settings,
        root,
        expected_fqn,
    }
}

#[test]
fn mint_api_client_command_helper_returns_token_and_stores_hash_only_with_default_expiry() {
    let fixture = mint_api_client_fixture();
    let issued_at = "2026-01-01T00:00:00Z";
    let expected_expires_at = (chrono::DateTime::parse_from_rfc3339(issued_at)
        .unwrap()
        .with_timezone(&chrono::Utc)
        + chrono::Duration::hours(MINT_API_CLIENT_DEFAULT_TTL_HOURS))
    .to_rfc3339();

    let response = mint_api_client_with_path(
        &fixture.path,
        &fixture.settings,
        fixture.root.clone(),
        vec![
            " send ".to_string(),
            "".to_string(),
            "list-peers-lean".to_string(),
        ],
        Some("gui mint".to_string()),
        None,
        "client-1".to_string(),
        "plain-secret".to_string(),
        issued_at.to_string(),
    )
    .expect("mint succeeds");

    assert_eq!(response.client_id, "client-1");
    assert_eq!(response.token, "plain-secret");
    assert_eq!(response.bound_root, fixture.root);
    assert_eq!(response.bound_fqn, fixture.expected_fqn);
    assert_eq!(response.scopes, vec!["send", "list-peers-lean"]);
    assert_eq!(
        response.expires_at.as_deref(),
        Some(expected_expires_at.as_str())
    );
    assert_eq!(response.note, MINT_API_CLIENT_NOTE);

    let raw = std::fs::read_to_string(&fixture.path).expect("read registry");
    assert!(raw.contains("client-1"));
    assert!(raw.contains("sha256:"));
    assert!(!raw.contains("plain-secret"));

    let registry = auth::list(&fixture.path);
    assert_eq!(registry.clients.len(), 1);
    let client = &registry.clients[0];
    assert_eq!(client.client_id, "client-1");
    assert_eq!(client.label, "gui mint");
    assert_eq!(client.bound_fqn, fixture.expected_fqn);
    assert_eq!(client.bound_root, response.bound_root);
    assert_eq!(client.scopes, response.scopes);
    assert_eq!(client.expires_at, response.expires_at);
    assert_ne!(client.token_hash, "plain-secret");
}

#[test]
fn mint_api_client_rejects_unknown_root() {
    let fixture = mint_api_client_fixture();
    let unknown_root = fixture
        .project
        .join(".ac")
        .join("wg-1-devs")
        .join("__agent_missing");

    let err = mint_api_client_with_path(
        &fixture.path,
        &fixture.settings,
        unknown_root.to_string_lossy().to_string(),
        vec!["send".to_string()],
        None,
        None,
        "client-1".to_string(),
        "plain-secret".to_string(),
        "2026-01-01T00:00:00Z".to_string(),
    )
    .expect_err("unknown root should fail");

    assert!(err.contains("unknown or invalid root"), "{err}");
    assert!(!fixture.path.exists());
}

#[test]
fn mint_api_client_rejects_expiry_beyond_max() {
    let fixture = mint_api_client_fixture();
    let issued_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let over_max =
        (issued_at + chrono::Duration::days(MINT_API_CLIENT_MAX_TTL_DAYS + 1)).to_rfc3339();

    let err = mint_api_client_with_path(
        &fixture.path,
        &fixture.settings,
        fixture.root,
        vec!["send".to_string()],
        None,
        Some(over_max),
        "client-1".to_string(),
        "plain-secret".to_string(),
        issued_at.to_rfc3339(),
    )
    .expect_err("over-max expiry should fail");

    assert!(
        err.contains("expires exceeds maximum API client lifetime"),
        "{err}"
    );
    assert!(!fixture.path.exists());
}

#[test]
fn web_server_owned_status_maps_owned_running() {
    let status = build_web_server_owned_status(
        "127.0.0.1".to_string(),
        8765,
        WebServerLifecycle::Running,
        false,
        None,
    );

    assert!(!status.listening, "listening comes from the probe");
    assert!(status.owned);
    assert!(!status.external_listening);
    assert!(status.open_allowed);
    assert_eq!(status.bind, "127.0.0.1");
    assert_eq!(status.port, 8765);
    assert_eq!(status.state, WebServerOwnershipState::OwnedRunning);
}

#[test]
fn web_server_owned_status_maps_external_listener() {
    let status = build_web_server_owned_status(
        "127.0.0.1".to_string(),
        8765,
        WebServerLifecycle::Stopped,
        true,
        None,
    );

    assert!(status.listening);
    assert!(!status.owned);
    assert!(status.external_listening);
    assert!(!status.open_allowed);
    assert_eq!(status.state, WebServerOwnershipState::ExternalListening);
}

#[test]
fn web_server_owned_status_maps_stopped() {
    let status = build_web_server_owned_status(
        "127.0.0.1".to_string(),
        8765,
        WebServerLifecycle::Stopped,
        false,
        None,
    );

    assert!(!status.listening);
    assert!(!status.owned);
    assert!(!status.external_listening);
    assert!(!status.open_allowed);
    assert_eq!(status.state, WebServerOwnershipState::Stopped);
}

#[test]
fn web_server_owned_status_maps_starting_and_stopping() {
    for (lifecycle, state, listening) in [
        (
            WebServerLifecycle::Starting,
            WebServerOwnershipState::Starting,
            true,
        ),
        (
            WebServerLifecycle::Stopping,
            WebServerOwnershipState::Stopping,
            false,
        ),
    ] {
        let status =
            build_web_server_owned_status("0.0.0.0".to_string(), 8765, lifecycle, listening, None);
        assert_eq!(status.state, state);
        assert_eq!(status.listening, listening);
        assert!(status.owned);
        assert!(!status.external_listening);
        assert!(!status.open_allowed);
    }
}

// §1453 D2: el fallo viaja en el status solo si describe el target actual.
#[test]
fn web_server_owned_status_carries_matching_bind_failure() {
    let recorded = Some(crate::web::StartServerError::BindFailed {
        bind: "192.168.1.12".to_string(),
        addr: "192.168.1.12:8888".parse().unwrap(),
        detail: "os error 10049".to_string(),
    });
    let failure = select_current_bind_failure(recorded, "192.168.1.12", 8888, false)
        .expect("matching failure must survive");
    assert_eq!(failure.bind, "192.168.1.12");
    assert_eq!(failure.port, 8888);
    assert_eq!(failure.detail, "os error 10049");

    let status = build_web_server_owned_status(
        "192.168.1.12".to_string(),
        8888,
        WebServerLifecycle::Stopped,
        false,
        Some(failure),
    );
    assert_eq!(status.state, WebServerOwnershipState::Stopped);
    assert!(status.bind_failure.is_some());
}

// §1453 D2: un fallo de un target anterior (o sobre algo que SI escucha)
// no pinta el target nuevo. El caso IPv6 bracketed es el fix D-2: con el
// bind crudo guardado, matchea; con la forma canonica se perdia siempre.
#[test]
fn web_server_owned_status_drops_stale_bind_failure() {
    let recorded = || {
        Some(crate::web::StartServerError::BindFailed {
            bind: "192.168.1.12".to_string(),
            addr: "192.168.1.12:8888".parse().unwrap(),
            detail: "os error 10049".to_string(),
        })
    };
    assert!(select_current_bind_failure(recorded(), "192.168.1.12", 9999, false).is_none());
    assert!(select_current_bind_failure(recorded(), "0.0.0.0", 8888, false).is_none());
    assert!(select_current_bind_failure(None, "192.168.1.12", 8888, false).is_none());
    // guardia de listening (D2, higiene)
    assert!(select_current_bind_failure(recorded(), "192.168.1.12", 8888, true).is_none());
    // IPv6 bracketed: el crudo matchea contra el settings crudo
    let v6 = Some(crate::web::StartServerError::BindFailed {
        bind: "[::1]".to_string(),
        addr: "[::1]:8888".parse().unwrap(),
        detail: "os error 10049".to_string(),
    });
    assert!(select_current_bind_failure(v6, "[::1]", 8888, false).is_some());
}

// §1453 D3: heuristica de clasificacion sobre nombres reales de ambos mundos.
#[test]
fn classify_virtual_interface_names() {
    for physical in [
        "Ethernet",
        "Wi-Fi",
        "Local Area Connection",
        "eth0",
        "enp3s0",
        "en0",
        "wlan0",
    ] {
        assert!(
            !classify_virtual_interface(physical),
            "{physical} must be physical"
        );
    }
    for virtual_name in [
        // el nombre REAL de esta maquina, con parentesis anidados (2.4)
        "vEthernet (WSL (Hyper-V firewall))",
        "vEthernet (WSL)",
        "vEthernet (Default Switch)",
        "Tailscale",
        "VirtualBox Host-Only Network",
        "Docker0",
        "docker0",
        "tailscale0",
        "tun0",
        "utun3",
        "wg0",
        "br-1a2b3c",
        "veth42",
        "virbr0",
        "zt0",
        "WireGuard Tunnel",
    ] {
        assert!(
            classify_virtual_interface(virtual_name),
            "{virtual_name} must be virtual"
        );
    }
}

// §1453 D1: filtro (loopback, APIPA, unspecified), orden y dedup.
#[test]
fn map_web_server_interfaces_filters_and_sorts() {
    let mapped = map_web_server_interfaces(vec![
        ("Tailscale".to_string(), "100.121.138.61".parse().unwrap()),
        ("Ethernet".to_string(), "192.168.1.9".parse().unwrap()),
        ("Ethernet".to_string(), "169.254.10.20".parse().unwrap()),
        (
            "Loopback Pseudo-Interface 1".to_string(),
            "127.0.0.1".parse().unwrap(),
        ),
        ("Wi-Fi".to_string(), "192.168.1.2".parse().unwrap()),
        ("Ethernet".to_string(), "192.168.1.9".parse().unwrap()),
    ]);
    let flat: Vec<(String, String, bool)> = mapped
        .into_iter()
        .map(|i| (i.interface_name, i.address, i.is_virtual))
        .collect();
    assert_eq!(
        flat,
        vec![
            ("Ethernet".to_string(), "192.168.1.9".to_string(), false),
            ("Wi-Fi".to_string(), "192.168.1.2".to_string(), false),
            ("Tailscale".to_string(), "100.121.138.61".to_string(), true),
        ]
    );
}

// §1453 D8: el backend PUEDE devolver [] legitimamente (maquina cuyas
// unicas direcciones son loopback y APIPA). Es la precondicion del memo
// hasDetection() del frontend: lista vacia = sin evidencia, no evidencia
// de ausencia.
#[test]
fn map_web_server_interfaces_returns_empty_when_all_filtered() {
    let mapped = map_web_server_interfaces(vec![
        (
            "Loopback Pseudo-Interface 1".to_string(),
            "127.0.0.1".parse().unwrap(),
        ),
        ("Ethernet".to_string(), "169.254.10.20".parse().unwrap()),
    ]);
    assert!(mapped.is_empty());
}

async fn wait_for_web_lifecycle(
    handle: &WebServerHandle,
    expected: WebServerLifecycle,
) -> crate::WebServerLifecycleSnapshot {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = handle.snapshot();
            if snapshot.lifecycle == expected {
                return snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("web lifecycle did not converge")
}

async fn start_running_web_handle(
    handle: &WebServerHandle,
    bind: &str,
    port: u16,
) -> Arc<crate::web::WebSocketAdmission> {
    let lifecycle = handle.clone();
    let bind = bind.to_string();
    let (admission_sender, admission_receiver) = oneshot::channel();
    let waiter = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        move |generation, admission, generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(generation, bind, port));
            assert!(admission_sender.send(Arc::clone(&admission)).is_ok());
            Ok::<_, String>(Some(tauri::async_runtime::spawn(async move {
                generation_token.cancelled().await;
            })))
        },
    );
    let admission = admission_receiver.await.expect("admission exposed");
    assert_eq!(waiter.wait().await, Ok(true));
    wait_for_web_lifecycle(handle, WebServerLifecycle::Running).await;
    admission
}

#[tokio::test]
async fn stop_web_server_reports_stopped_and_waits_for_active_terminal() {
    let stopped = WebServerHandle::default();
    assert_eq!(stop_web_server_handle(&stopped).await, Ok(false));

    let handle = WebServerHandle::default();
    let admission = start_running_web_handle(&handle, "127.0.0.1", 8751).await;
    let retained = admission
        .try_acquire()
        .expect("running generation admits retained work");
    let handle_for_stop = handle.clone();
    let mut stop_task = tokio::spawn(async move { stop_web_server_handle(&handle_for_stop).await });

    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut stop_task)
            .await
            .is_err(),
        "Stop cannot report true while admitted work is retained"
    );
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
    drop(retained);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), stop_task)
            .await
            .expect("active Stop did not reach terminal")
            .expect("active Stop task panicked"),
        Ok(true)
    );
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopped);
}

#[tokio::test]
async fn stop_web_server_clears_bind_failure_published_by_late_bind() {
    let handle = WebServerHandle::default();
    let lifecycle = handle.clone();
    let failure_recorder = handle.clone();
    let (output_reached, release_output) = handle.gate_next_start_output();
    let (bind_pending_sender, bind_pending_receiver) = oneshot::channel();
    let (release_bind_sender, release_bind_receiver) = oneshot::channel();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        move |generation, _admission, _generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(
                generation,
                "127.0.0.1".to_string(),
                8753,
            ));
            bind_pending_sender
                .send(())
                .expect("report pending bind to the test");
            release_bind_receiver
                .await
                .expect("release late bind failure");
            failure_recorder.record_bind_failure(crate::web::StartServerError::BindFailed {
                bind: "127.0.0.1".to_string(),
                addr: SocketAddr::from(([127, 0, 0, 1], 8753)),
                detail: "late bind failure".to_string(),
            });
            Ok::<_, String>(None)
        },
    );
    bind_pending_receiver.await.expect("bind became pending");

    let handle_for_stop = handle.clone();
    let stop = tokio::spawn(async move { stop_web_server_handle(&handle_for_stop).await });
    wait_for_web_lifecycle(&handle, WebServerLifecycle::Stopping).await;
    release_bind_sender
        .send(())
        .expect("release pending bind after Stop linealizes");
    assert!(
        !output_reached
            .await
            .expect("late bind output reaches the supervisor"),
        "Stop must already own the lifecycle transition"
    );
    assert!(
        handle.last_bind_failure().is_some(),
        "the late bind must really publish before terminal cleanup"
    );
    release_output
        .send(())
        .expect("release late bind result processing");

    assert_eq!(
        start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );
    assert_eq!(stop.await.expect("late-bind Stop task panicked"), Ok(true));
    let status =
        resolve_web_server_owned_status(&handle, ("127.0.0.1".to_string(), 8753), |_addr| async {
            false
        })
        .await;
    assert_eq!(status.state, WebServerOwnershipState::Stopped);
    assert!(status.bind_failure.is_none());
    assert!(handle.last_bind_failure().is_none());
}

#[tokio::test]
async fn stop_web_server_timeout_preserves_stopping_and_shared_deadline() {
    let handle = WebServerHandle::default();
    let admission = start_running_web_handle(&handle, "127.0.0.1", 8752).await;
    let retained = admission
        .try_acquire()
        .expect("running generation admits retained work");
    let first = handle
        .begin_stop_with_timeout(Duration::from_millis(40))
        .expect("running generation is owned");
    drop(first);

    assert_eq!(
        stop_web_server_handle(&handle).await,
        Err("Timed out waiting for web server generation to stop".to_string())
    );
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);

    drop(retained);
    wait_for_web_lifecycle(&handle, WebServerLifecycle::Stopped).await;
}

#[tokio::test]
async fn ensure_web_remote_open_allowed_rejects_enabled_settings_without_owned_handle() {
    let settings = AppSettings {
        web_server_enabled: true,
        web_server_bind: "127.0.0.1".to_string(),
        web_server_port: 8765,
        ..AppSettings::default()
    };
    let handle = WebServerHandle::default();
    assert_eq!(
        ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
        "Web server is not owned by this app process"
    );

    let lifecycle = handle.clone();
    let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
    let (factory_release_sender, factory_release_receiver) = oneshot::channel();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        move |generation, _admission, generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(
                generation,
                "127.0.0.1".to_string(),
                8765,
            ));
            assert!(factory_entered_sender.send(()).is_ok());
            factory_release_receiver.await.expect("release factory");
            if generation_token.is_cancelled() {
                Err(WEB_SERVER_START_CANCELLED.to_string())
            } else {
                Ok(None)
            }
        },
    );
    factory_entered_receiver.await.expect("factory entered");
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Starting);
    assert_eq!(
        ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
        "Web server is not owned by this app process"
    );
    let stop = handle.begin_stop().expect("starting generation is owned");
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
    assert_eq!(
        ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
        "Web server is not owned by this app process"
    );
    assert!(factory_release_sender.send(()).is_ok());
    assert_eq!(
        start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );
    stop.wait().await.expect("stopping generation drains");

    let mismatched = WebServerHandle::default();
    start_running_web_handle(&mismatched, "127.0.0.1", 8766).await;
    assert_eq!(
        ensure_web_remote_open_allowed(&settings, &mismatched).unwrap_err(),
        "Web server is not owned by this app process"
    );
    mismatched
        .begin_stop()
        .expect("mismatched running generation is owned")
        .wait()
        .await
        .expect("mismatched generation drains");
}

#[tokio::test]
async fn ensure_web_remote_open_allowed_accepts_owned_running_handle() {
    let settings = AppSettings {
        web_server_enabled: true,
        web_server_bind: "127.0.0.1".to_string(),
        web_server_port: 8765,
        ..AppSettings::default()
    };
    let handle = WebServerHandle::default();
    start_running_web_handle(&handle, "127.0.0.1", 8765).await;

    let authorized = ensure_web_remote_open_allowed(&settings, &handle)
        .expect("matching Running generation is authorized");
    assert_eq!(authorized.lifecycle, WebServerLifecycle::Running);
    assert_eq!(authorized.endpoint, Some(("127.0.0.1".to_string(), 8765)));
    assert!(handle.snapshot_is_current(&authorized));

    let stop = handle.begin_stop().expect("running generation is owned");
    assert!(!handle.snapshot_is_current(&authorized));
    assert_eq!(
        ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
        "Web server is not owned by this app process"
    );
    stop.wait().await.expect("authorized generation drains");
}

#[tokio::test]
async fn web_server_owned_status_retries_when_starting_revision_changes() {
    let handle = WebServerHandle::default();
    let lifecycle = handle.clone();
    let endpoint_a = ("127.0.0.1".to_string(), 8811);
    let endpoint_b = ("127.0.0.1".to_string(), 8812);
    let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
    let (factory_release_sender, factory_release_receiver) = oneshot::channel();
    let endpoint_a_for_factory = endpoint_a.clone();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        move |generation, _admission, generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(
                generation,
                endpoint_a_for_factory.0,
                endpoint_a_for_factory.1,
            ));
            assert!(factory_entered_sender.send(()).is_ok());
            factory_release_receiver.await.expect("release factory");
            if generation_token.is_cancelled() {
                Err(WEB_SERVER_START_CANCELLED.to_string())
            } else {
                Ok(None)
            }
        },
    );
    factory_entered_receiver.await.expect("factory entered");
    let before = handle.snapshot();
    assert_eq!(before.lifecycle, WebServerLifecycle::Starting);
    assert_eq!(before.endpoint, Some(endpoint_a.clone()));
    let generation = before.generation.expect("starting generation id");

    let (probe_a_sender, probe_a_receiver) = oneshot::channel();
    let (probe_b_sender, probe_b_receiver) = oneshot::channel();
    let replies = Arc::new(Mutex::new(VecDeque::from([
        probe_a_receiver,
        probe_b_receiver,
    ])));
    let (calls_sender, mut calls_receiver) = mpsc::unbounded_channel();
    let status_handle = handle.clone();
    let replies_for_probe = Arc::clone(&replies);
    let status_task = tokio::spawn(async move {
        resolve_web_server_owned_status(
            &status_handle,
            ("127.0.0.1".to_string(), 8899),
            move |addr| {
                calls_sender.send(addr).expect("record status probe");
                let reply = replies_for_probe
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("configured probe response");
                async move { reply.await.expect("release status probe") }
            },
        )
        .await
    });
    assert_eq!(
        calls_receiver.recv().await.expect("first probe call"),
        web_server_probe_addr(&endpoint_a.0, endpoint_a.1).unwrap()
    );
    assert!(handle.publish_effective_endpoint(generation, endpoint_b.0.clone(), endpoint_b.1,));
    let after_publish = handle.snapshot();
    assert_eq!(after_publish.generation, before.generation);
    assert_eq!(after_publish.lifecycle, WebServerLifecycle::Starting);
    assert_eq!(after_publish.revision, before.revision + 1);
    assert_eq!(after_publish.endpoint, Some(endpoint_b.clone()));
    assert!(probe_a_sender.send(true).is_ok());
    assert_eq!(
        calls_receiver.recv().await.expect("second probe call"),
        web_server_probe_addr(&endpoint_b.0, endpoint_b.1).unwrap()
    );
    assert!(probe_b_sender.send(false).is_ok());
    let status = status_task.await.expect("status task panicked");
    assert_eq!(status.bind, endpoint_b.0);
    assert_eq!(status.port, endpoint_b.1);
    assert!(!status.listening);
    assert_eq!(status.state, WebServerOwnershipState::Starting);
    assert!(status.owned);
    assert!(!status.open_allowed);

    let stop = handle.begin_stop().expect("starting generation is owned");
    assert!(factory_release_sender.send(()).is_ok());
    assert_eq!(
        start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );
    stop.wait().await.expect("starting generation drains");
}

#[tokio::test]
async fn web_server_owned_status_retries_when_phase_changes_during_probe() {
    let handle = WebServerHandle::default();
    let endpoint = ("127.0.0.1".to_string(), 8813);
    let admission = start_running_web_handle(&handle, &endpoint.0, endpoint.1).await;
    let retained = admission
        .try_acquire()
        .expect("running generation admits retained work");
    let (running_probe_sender, running_probe_receiver) = oneshot::channel();
    let (stopping_probe_sender, stopping_probe_receiver) = oneshot::channel();
    let replies = Arc::new(Mutex::new(VecDeque::from([
        running_probe_receiver,
        stopping_probe_receiver,
    ])));
    let (calls_sender, mut calls_receiver) = mpsc::unbounded_channel();
    let status_handle = handle.clone();
    let replies_for_probe = Arc::clone(&replies);
    let status_task = tokio::spawn(async move {
        resolve_web_server_owned_status(
            &status_handle,
            ("127.0.0.1".to_string(), 8899),
            move |addr| {
                calls_sender.send(addr).expect("record status probe");
                let reply = replies_for_probe
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("configured probe response");
                async move { reply.await.expect("release status probe") }
            },
        )
        .await
    });

    let expected_probe = web_server_probe_addr(&endpoint.0, endpoint.1).unwrap();
    assert_eq!(
        calls_receiver.recv().await.expect("Running probe call"),
        expected_probe
    );
    let stop = handle.begin_stop().expect("running generation is owned");
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
    assert!(running_probe_sender.send(true).is_ok());
    assert_eq!(
        calls_receiver.recv().await.expect("Stopping probe call"),
        expected_probe
    );
    assert!(stopping_probe_sender.send(false).is_ok());
    let status = status_task.await.expect("status task panicked");
    assert_eq!(status.state, WebServerOwnershipState::Stopping);
    assert!(status.owned);
    assert!(!status.listening);
    assert!(!status.external_listening);
    assert!(!status.open_allowed);

    drop(retained);
    stop.wait().await.expect("stopping generation drains");
}

#[tokio::test]
async fn web_server_owned_status_stopping_before_endpoint_uses_configured_fallback() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind configured fallback listener");
    let fallback = listener.local_addr().expect("fallback listener address");
    let handle = WebServerHandle::default();
    let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
    let (factory_release_sender, factory_release_receiver) = oneshot::channel();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        move |_generation, _admission, _generation_token| async move {
            assert!(factory_entered_sender.send(()).is_ok());
            factory_release_receiver.await.expect("release factory");
            Err(WEB_SERVER_START_CANCELLED.to_string())
        },
    );
    factory_entered_receiver.await.expect("factory entered");
    let stop = handle.begin_stop().expect("starting generation is owned");
    let stopping = handle.snapshot();
    assert_eq!(stopping.lifecycle, WebServerLifecycle::Stopping);
    assert_eq!(stopping.endpoint, None);
    let configured = (fallback.ip().to_string(), fallback.port());

    let listening =
        resolve_web_server_owned_status(&handle, configured.clone(), super::is_tcp_listening).await;
    assert_eq!(listening.bind, configured.0);
    assert_eq!(listening.port, configured.1);
    assert!(listening.listening);
    assert_eq!(listening.state, WebServerOwnershipState::Stopping);
    assert!(listening.owned);
    assert!(!listening.external_listening);
    assert!(!listening.open_allowed);

    let failed_probe =
        resolve_web_server_owned_status(&handle, configured, |_addr| async { false }).await;
    assert!(!failed_probe.listening);
    assert_eq!(failed_probe.state, WebServerOwnershipState::Stopping);
    assert!(failed_probe.owned);
    assert!(!failed_probe.external_listening);
    assert!(!failed_probe.open_allowed);

    assert!(factory_release_sender.send(()).is_ok());
    assert_eq!(
        start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );
    stop.wait().await.expect("stopping generation drains");
}

#[tokio::test]
async fn web_server_owned_status_stopping_retries_when_endpoint_appears_during_fallback_probe() {
    let handle = WebServerHandle::default();
    let lifecycle = handle.clone();
    let fallback_a = ("127.0.0.1".to_string(), 8821);
    let endpoint_b = ("127.0.0.1".to_string(), 8822);
    let endpoint_b_for_factory = endpoint_b.clone();
    let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
    let (publish_sender, publish_receiver) = oneshot::channel();
    let (published_sender, published_receiver) = oneshot::channel();
    let (finish_sender, finish_receiver) = oneshot::channel();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        move |generation, _admission, _generation_token| async move {
            assert!(factory_entered_sender.send(()).is_ok());
            publish_receiver
                .await
                .expect("release endpoint publication");
            assert!(
                !lifecycle.publish_effective_endpoint(
                    generation,
                    endpoint_b_for_factory.0,
                    endpoint_b_for_factory.1,
                ),
                "Stopping publication updates status but cannot continue start"
            );
            assert!(published_sender.send(()).is_ok());
            finish_receiver.await.expect("release factory terminal");
            Err(WEB_SERVER_START_CANCELLED.to_string())
        },
    );
    factory_entered_receiver.await.expect("factory entered");
    let stop = handle.begin_stop().expect("starting generation is owned");
    let before = handle.snapshot();
    assert_eq!(before.lifecycle, WebServerLifecycle::Stopping);
    assert_eq!(before.endpoint, None);

    let (probe_a_sender, probe_a_receiver) = oneshot::channel();
    let (probe_b_sender, probe_b_receiver) = oneshot::channel();
    let replies = Arc::new(Mutex::new(VecDeque::from([
        probe_a_receiver,
        probe_b_receiver,
    ])));
    let (calls_sender, mut calls_receiver) = mpsc::unbounded_channel();
    let status_handle = handle.clone();
    let replies_for_probe = Arc::clone(&replies);
    let fallback_for_status = fallback_a.clone();
    let status_task = tokio::spawn(async move {
        resolve_web_server_owned_status(&status_handle, fallback_for_status, move |addr| {
            calls_sender.send(addr).expect("record status probe");
            let reply = replies_for_probe
                .lock()
                .unwrap()
                .pop_front()
                .expect("configured probe response");
            async move { reply.await.expect("release status probe") }
        })
        .await
    });
    assert_eq!(
        calls_receiver.recv().await.expect("fallback probe call"),
        web_server_probe_addr(&fallback_a.0, fallback_a.1).unwrap()
    );
    assert!(publish_sender.send(()).is_ok());
    published_receiver.await.expect("endpoint published");
    let after_publish = handle.snapshot();
    assert_eq!(after_publish.generation, before.generation);
    assert_eq!(after_publish.lifecycle, WebServerLifecycle::Stopping);
    assert_eq!(after_publish.revision, before.revision + 1);
    assert_eq!(after_publish.endpoint, Some(endpoint_b.clone()));
    assert!(probe_a_sender.send(true).is_ok());
    assert_eq!(
        calls_receiver.recv().await.expect("effective probe call"),
        web_server_probe_addr(&endpoint_b.0, endpoint_b.1).unwrap()
    );
    assert!(probe_b_sender.send(false).is_ok());
    let status = status_task.await.expect("status task panicked");
    assert_eq!(status.bind, endpoint_b.0);
    assert_eq!(status.port, endpoint_b.1);
    assert!(!status.listening);
    assert_eq!(status.state, WebServerOwnershipState::Stopping);
    assert!(status.owned);
    assert!(!status.external_listening);
    assert!(!status.open_allowed);

    assert!(finish_sender.send(()).is_ok());
    assert_eq!(
        start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );
    stop.wait().await.expect("stopping generation drains");
}

async fn assert_wildcard_owned_phase_probe(
    handle: &WebServerHandle,
    expected_state: WebServerOwnershipState,
    probe_result: bool,
    port: u16,
) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_for_probe = Arc::clone(&calls);
    let status =
        resolve_web_server_owned_status(handle, ("0.0.0.0".to_string(), port), move |addr| {
            calls_for_probe.lock().unwrap().push(addr);
            async move { probe_result }
        })
        .await;
    assert_eq!(
        *calls.lock().unwrap(),
        vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))]
    );
    assert_eq!(status.state, expected_state);
    assert_eq!(status.listening, probe_result);
    assert!(status.owned);
    assert!(!status.external_listening);
    assert_eq!(
        status.open_allowed,
        expected_state == WebServerOwnershipState::OwnedRunning
    );
    assert_eq!(status.bind, "0.0.0.0");
    assert_eq!(status.port, port);
}

#[tokio::test]
async fn web_server_owned_status_wildcard_uses_probe_in_all_owned_phases() {
    let port = 8831;
    let handle = WebServerHandle::default();
    let lifecycle = handle.clone();
    let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
    let (factory_release_sender, factory_release_receiver) = oneshot::channel();
    let (admission_sender, admission_receiver) = oneshot::channel();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        move |generation, admission, generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(generation, "0.0.0.0".to_string(), port,));
            assert!(admission_sender.send(Arc::clone(&admission)).is_ok());
            assert!(factory_entered_sender.send(()).is_ok());
            factory_release_receiver.await.expect("release factory");
            Ok::<_, String>(Some(tauri::async_runtime::spawn(async move {
                generation_token.cancelled().await;
            })))
        },
    );
    factory_entered_receiver.await.expect("factory entered");
    let admission = admission_receiver.await.expect("admission exposed");
    for result in [true, false] {
        assert_wildcard_owned_phase_probe(&handle, WebServerOwnershipState::Starting, result, port)
            .await;
    }

    assert!(factory_release_sender.send(()).is_ok());
    assert_eq!(start.wait().await, Ok(true));
    for result in [true, false] {
        assert_wildcard_owned_phase_probe(
            &handle,
            WebServerOwnershipState::OwnedRunning,
            result,
            port,
        )
        .await;
    }

    let retained = admission
        .try_acquire()
        .expect("running generation admits retained work");
    let stop = handle.begin_stop().expect("running generation is owned");
    for result in [true, false] {
        assert_wildcard_owned_phase_probe(&handle, WebServerOwnershipState::Stopping, result, port)
            .await;
    }
    drop(retained);
    stop.wait().await.expect("wildcard generation drains");
}

fn profile_assignment_request(
    scope: super::ProfileAssignmentScope,
    confirmed_target_fingerprint: Option<&str>,
    typed_confirmation: Option<&str>,
) -> super::ApplyCodingAgentProfileSelectionRequest {
    super::ApplyCodingAgentProfileSelectionRequest {
        target_replica_path: "C:/ac/wg-1/__agent_dev-rust".to_string(),
        coding_agent_id: "dev-rust".to_string(),
        profile: "B".to_string(),
        scope,
        restart_sessions: false,
        confirmed_target_fingerprint: confirmed_target_fingerprint.map(|value| value.to_string()),
        typed_confirmation: typed_confirmation.map(|value| value.to_string()),
        assignment_mode: super::AssignmentMode::Ordinary,
        conflict_decision: None,
    }
}

/// #1941 - a synthetic enumeration lets the fingerprint unit tests exercise
/// the typed tuple without a filesystem fixture.
fn fingerprint_enumeration(paths: &[&str]) -> super::ProfileTargetEnumeration {
    super::ProfileTargetEnumeration {
        anchor_dir: PathBuf::from("c:/ac/wg-1/__agent_a"),
        anchor_matrix_dir: PathBuf::from("c:/ac/_agent_a"),
        candidates: paths
            .iter()
            .enumerate()
            .map(|(index, path)| super::ProfileCandidate {
                expected: None,
                target: super::ProfileAssignmentTarget {
                    workgroup_name: "wg-1".to_string(),
                    workgroup_path: "c:/ac/wg-1".to_string(),
                    replica_name: format!("agent_{index}"),
                    replica_path: (*path).to_string(),
                    identity_path: "../../_agent_a".to_string(),
                    origin_project: Some("ac".to_string()),
                    live_session_ids: Vec::new(),
                    saved_pair: None,
                    selection_state: super::SelectionState::Unlocked,
                    selection_error: None,
                },
                matrix_dir: PathBuf::from("c:/ac/_agent_a"),
            })
            .collect(),
        warnings: Vec::new(),
        scope_faults: Vec::new(),
        counts_complete: true,
    }
}

#[test]
fn profile_assignment_fingerprint_uses_typed_fields() {
    let enumeration = fingerprint_enumeration(&["c:/ac/wg-1/__agent_a"]);
    let other = fingerprint_enumeration(&["c:/ac/wg-1/__agent_b"]);
    let eligible = vec!["c:/ac/wg-1/__agent_a".to_string()];

    let base = super::assignment_fingerprint(
        &enumeration,
        &[],
        &super::ProfileAssignmentScope::Kind,
        "assign",
        "ab",
        "C",
        false,
        None,
        &eligible,
    )
    .expect("fingerprint");
    let agent_bc = super::assignment_fingerprint(
        &enumeration,
        &[],
        &super::ProfileAssignmentScope::Kind,
        "assign",
        "a",
        "BC",
        false,
        None,
        &eligible,
    )
    .expect("fingerprint");
    let restart = super::assignment_fingerprint(
        &enumeration,
        &[],
        &super::ProfileAssignmentScope::Kind,
        "assign",
        "ab",
        "C",
        true,
        None,
        &eligible,
    )
    .expect("fingerprint");
    let decision = super::assignment_fingerprint(
        &enumeration,
        &[],
        &super::ProfileAssignmentScope::Kind,
        "assign",
        "ab",
        "C",
        false,
        Some(super::ConflictDecision::ForceReviewed),
        &eligible,
    )
    .expect("fingerprint");
    let other_targets = super::assignment_fingerprint(
        &other,
        &[],
        &super::ProfileAssignmentScope::Kind,
        "assign",
        "ab",
        "C",
        false,
        None,
        &["c:/ac/wg-1/__agent_b".to_string()],
    )
    .expect("fingerprint");

    assert_eq!(base.len(), 64, "sha256 hex");
    assert_eq!(base, base.to_lowercase(), "lowercase hex");
    assert_ne!(base, agent_bc);
    assert_ne!(base, restart);
    assert_ne!(base, decision);
    assert_ne!(base, other_targets);
}

#[test]
fn profile_assignment_confirmation_hint_tracks_broad_scopes() {
    assert!(!super::profile_assignment_requires_explicit_confirmation(
        &super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary
    ));
    assert!(super::profile_assignment_requires_explicit_confirmation(
        &super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock
    ));
    assert!(super::profile_assignment_requires_explicit_confirmation(
        &super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary
    ));
    assert!(super::profile_assignment_requires_explicit_confirmation(
        &super::ProfileAssignmentScope::Kind,
        super::AssignmentMode::Ordinary
    ));
}

#[test]
fn kind_assignment_accepts_fingerprint_without_typed_confirmation() {
    assert_eq!(
        super::validate_profile_assignment_confirmation(
            &super::ProfileAssignmentScope::Kind,
            super::AssignmentMode::Ordinary,
            Some("fp-kind"),
            "fp-kind",
        ),
        Ok(())
    );
}

#[test]
fn kind_assignment_ignores_legacy_typed_confirmation() {
    let request = profile_assignment_request(
        super::ProfileAssignmentScope::Kind,
        Some("fp-kind"),
        Some("not the old exact phrase"),
    );

    assert_eq!(
        super::validate_profile_assignment_confirmation(
            &request.scope,
            request.assignment_mode,
            request.confirmed_target_fingerprint.as_deref(),
            "fp-kind",
        ),
        Ok(())
    );
}

#[test]
fn broad_assignment_rejects_missing_or_stale_fingerprint() {
    for scope in [
        super::ProfileAssignmentScope::Kind,
        super::ProfileAssignmentScope::Workgroup,
    ] {
        let missing = profile_assignment_request(scope.clone(), None, None);
        let err = super::validate_profile_assignment_confirmation(
            &missing.scope,
            missing.assignment_mode,
            missing.confirmed_target_fingerprint.as_deref(),
            "current-fp",
        )
        .unwrap_err();
        // #2051 - a missing fingerprint is a contract failure, not a change.
        assert!(err.contains("confirmationMissing"), "{err}");
        assert!(!err.contains("Target selection changed"), "{err}");

        let stale = profile_assignment_request(scope, Some("old-fp"), None);
        let err = super::validate_profile_assignment_confirmation(
            &stale.scope,
            stale.assignment_mode,
            stale.confirmed_target_fingerprint.as_deref(),
            "current-fp",
        )
        .unwrap_err();
        assert!(err.contains("Target selection changed"), "{err}");
    }
}

#[test]
fn replica_assign_and_lock_confirmation_shapes() {
    let validate = |mode: super::AssignmentMode, provided: Option<&str>| {
        super::validate_profile_assignment_confirmation(
            &super::ProfileAssignmentScope::Replica,
            mode,
            provided,
            "fp-replica",
        )
    };

    // (a) the preview's own fingerprint is accepted...
    assert_eq!(
        validate(super::AssignmentMode::AssignAndLock, Some("fp-replica")),
        Ok(())
    );
    // (b) ...while NO fingerprint at all is a contract failure, never stale.
    let err = validate(super::AssignmentMode::AssignAndLock, None).unwrap_err();
    assert!(err.contains("confirmationMissing"), "{err}");
    assert!(!err.contains("Target selection changed"), "{err}");
    // (c) a provided, non-matching value is stale.
    let err = validate(super::AssignmentMode::AssignAndLock, Some("old-fp")).unwrap_err();
    assert!(err.contains("stalePreview"), "{err}");
    // (d) the legacy replica ordinary path still accepts no fingerprint (D2).
    assert_eq!(validate(super::AssignmentMode::Ordinary, None), Ok(()));
    // (e) even ordinary is stale when a provided value does not match.
    let err = validate(super::AssignmentMode::Ordinary, Some("old-fp")).unwrap_err();
    assert!(err.contains("stalePreview"), "{err}");
}

#[test]
fn settings_draft_events_flag_agent_base_command_edit() {
    // #592/#597 - a bare agent base-command edit changes the effective-command
    // drift hash, so save_settings_draft must emit coding_agent_profiles_updated
    // even though the profile cells and envs are untouched. This is the bug the
    // user hit editing `claude` -> `claude-amp -c ...` in Settings -> Coding Agents.
    let before = settings_with_single_agent();
    let mut after = settings_with_single_agent();
    after.agents[0].command = "codex --resume".to_string();

    let events = super::settings_draft_update_events(&before, &after);
    assert!(
        events.profiles_changed,
        "an agent base-command edit must flag a profiles refresh"
    );
    assert!(events.env_agent_ids.is_empty(), "no env changed");
}

#[test]
fn id_hash_activation_events_ignore_only_cell_metadata() {
    let mut before = settings_with_single_agent();
    crate::config::settings::validate_and_repair_settings(&mut before).unwrap();
    before
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .id_hash_debug = Some(vec!["old".into()]);
    let mut metadata = before.clone();
    metadata.agents[0].id_hash_debug = Some(vec!["forged-base".into()]);
    metadata
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .id_hash_debug = Some(vec!["forged-pair".into()]);
    let debug_events = super::settings_draft_update_events(&before, &metadata);
    assert!(!debug_events.profiles_changed);
    assert!(debug_events.env_agent_ids.is_empty());
    metadata
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .id_hash = Some("client-metadata".into());
    let events = super::settings_draft_update_events(&before, &metadata);
    assert!(!events.profiles_changed);
    assert!(events.env_agent_ids.is_empty());
    assert_ne!(before.coding_agent_profiles, metadata.coding_agent_profiles);
    for change in [
        "command",
        "env",
        "enabled",
        "notes",
        "preflight",
        "timeout",
        "cell",
        "agent",
        "default",
        "label",
        "slots",
        "schema",
        "identity",
        "home",
    ] {
        let mut after = metadata.clone();
        let profiles = &mut after.coding_agent_profiles;
        let cell = profiles
            .profiles_by_agent
            .get_mut("agent-0")
            .unwrap()
            .get_mut("A")
            .unwrap();
        match change {
            "command" => cell.command.push_str("--real"),
            "env" => {
                cell.env.insert("K".into(), "V".into());
            }
            "enabled" => cell.enabled = !cell.enabled,
            "notes" => cell.notes.push_str("real"),
            "preflight" => cell.preflight = Some("real".into()),
            "timeout" => cell.preflight_timeout_seconds = Some(99),
            "cell" => {
                profiles
                    .profiles_by_agent
                    .get_mut("agent-0")
                    .unwrap()
                    .remove("A");
            }
            "agent" => {
                profiles
                    .profiles_by_agent
                    .insert("new".into(), BTreeMap::new());
            }
            "default" => {
                profiles
                    .default_profile_by_agent
                    .insert("agent-0".into(), "B".into());
            }
            "label" => {
                profiles.profile_labels_by_agent.insert(
                    "agent-0".into(),
                    BTreeMap::from([("A".into(), "real".into())]),
                );
            }
            "slots" => {
                profiles.profile_slots.clear();
            }
            "schema" => profiles.schema_version += 1,
            "identity" => {
                profiles
                    .identity_by_agent
                    .insert("agent-0".into(), "real".into());
            }
            "home" => {
                profiles
                    .codex_home_key_by_agent
                    .insert("agent-0".into(), "real".into());
            }
            _ => unreachable!(),
        }
        let events = super::settings_draft_update_events(&before, &after);
        assert!(events.profiles_changed, "{change}");
        assert!(events.env_agent_ids.is_empty(), "{change}");
    }
}

#[test]
fn settings_draft_events_quiet_for_non_drift_field_edit() {
    // A non-drift field (label) changing must NOT emit a profiles refresh, so the
    // fix does not over-fire on unrelated settings saves.
    let before = settings_with_single_agent();
    let mut after = settings_with_single_agent();
    after.agents[0].label = "Renamed".to_string();

    let events = super::settings_draft_update_events(&before, &after);
    assert!(!events.profiles_changed, "a label edit is not drift");
    assert!(events.env_agent_ids.is_empty());
}

#[test]
fn settings_draft_events_flag_agent_env_edit() {
    // Regression guard: the pre-existing env path still flags the agent id (the
    // base env is also a drift-hash input, surfaced via the env event).
    let mut before = settings_with_single_agent();
    crate::config::settings::validate_and_repair_settings(&mut before).unwrap();
    let mut after = before.clone();
    after.agents[0].envs = vec![CodingAgentEnv {
        key: "FOO".to_string(),
        value: "bar".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];

    after
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .id_hash = Some("derived-after-env-edit".into());
    after
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent-0")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .id_hash_debug = Some(vec!["forged-env-debug".into()]);
    let events = super::settings_draft_update_events(&before, &after);
    assert_eq!(events.env_agent_ids, vec!["agent-0".to_string()]);
    assert!(
        !events.profiles_changed,
        "env-only edit is not a profiles change"
    );
}

#[tokio::test]
async fn restart_failure_classification_depends_on_session_survival() {
    let manager = SessionManager::new();
    let session = manager
        .create_session(
            "codex".to_string(),
            Vec::new(),
            "C:/work/repo".to_string(),
            Some("codex".to_string()),
            Some("Codex".to_string()),
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let session_mgr = Arc::new(RwLock::new(manager));

    let (survived_code, survived_destroyed) =
        super::classify_restart_failure(&session_mgr, session.id).await;
    assert_eq!(survived_code, "restartFailed");
    assert!(!survived_destroyed);

    let (missing_code, missing_destroyed) =
        super::classify_restart_failure(&session_mgr, uuid::Uuid::new_v4()).await;
    assert_eq!(missing_code, "destroyedButNotRecreated");
    assert!(missing_destroyed);
}

#[tokio::test]
async fn invalid_env_settings_update_leaves_live_settings_unchanged() {
    let mut original = settings_with_single_agent();
    original.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "current".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    original.agents[0].isolated_home = true;
    let state = state_for(original.clone());

    let err = persist_coding_agent_env_settings_update(
        &state,
        "agent-0",
        vec![CodingAgentEnv {
            key: "AGENTSCOMMANDER_TOKEN".to_string(),
            value: "bad".to_string(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }],
        false,
    )
    .await
    .unwrap_err();

    assert!(err.contains("reserved by AgentsCommander"), "{err}");
    let live = state.read().await;
    assert_eq!(live.agents[0].envs, original.agents[0].envs);
    assert_eq!(
        live.agents[0].isolated_home,
        original.agents[0].isolated_home
    );
}

#[tokio::test]
async fn invalid_profile_settings_update_leaves_live_settings_unchanged() {
    let original = settings_with_single_agent();
    let mut invalid_profiles = original.coding_agent_profiles.clone();
    invalid_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: BTreeMap::from([("AGENTSCOMMANDER_ROOT".to_string(), "bad".to_string())]),
                notes: String::new(),
            },
        );
    let state = state_for(original.clone());

    let err = persist_coding_agent_profiles_update(&state, invalid_profiles)
        .await
        .unwrap_err();

    assert!(err.contains("reserved by AgentsCommander"), "{err}");
    let live = state.read().await;
    assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
}

#[tokio::test]
async fn invalid_settings_draft_transaction_leaves_generic_profiles_and_env_unchanged() {
    let mut original = settings_with_single_agent();
    original.sidebar_style = "noir-minimal".to_string();
    original.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "current".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    original.agents[0].isolated_home = true;
    original
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "B".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "codex --current-profile".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );
    let state = state_for(original.clone());

    let mut draft = original.clone();
    draft.sidebar_style = "command-center".to_string();
    draft.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "draft".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    draft.agents[0].isolated_home = false;
    draft
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "C".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "codex --draft-profile".to_string(),
                env: BTreeMap::from([("AGENTSCOMMANDER_TOKEN".to_string(), "bad".to_string())]),
                notes: String::new(),
            },
        );

    let err = persist_settings_draft_update_with_saver(
        &state,
        draft,
        |_| -> Result<AppSettings, String> { panic!("save must not run") },
    )
    .await
    .unwrap_err();

    assert!(err.contains("reserved by AgentsCommander"), "{err}");
    let live = state.read().await;
    assert_eq!(live.sidebar_style, original.sidebar_style);
    assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
    assert_eq!(live.agents[0].envs, original.agents[0].envs);
    assert_eq!(
        live.agents[0].isolated_home,
        original.agents[0].isolated_home
    );
}

#[tokio::test]
async fn settings_draft_transaction_save_failure_leaves_live_settings_unchanged() {
    let mut original = settings_with_single_agent();
    original.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "current".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    original.agents[0].isolated_home = true;
    let state = state_for(original.clone());

    let mut draft = original.clone();
    draft.sidebar_style = "command-center".to_string();
    draft.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "draft".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    draft.agents[0].isolated_home = false;

    let err = persist_settings_draft_update_with_saver(&state, draft, |_| {
        Err("simulated save failure".to_string())
    })
    .await
    .unwrap_err();

    assert_eq!(err, "simulated save failure");
    let live = state.read().await;
    assert_eq!(live.sidebar_style, original.sidebar_style);
    assert_eq!(live.coding_agent_profiles, original.coding_agent_profiles);
    assert_eq!(live.agents[0].envs, original.agents[0].envs);
    assert_eq!(
        live.agents[0].isolated_home,
        original.agents[0].isolated_home
    );
}

async fn assert_narrow_setter_save_failure_rolls_back(
    field_name: &'static str,
    mutate_candidate: impl FnOnce(&mut AppSettings) + Send,
    assert_candidate: impl FnOnce(&AppSettings) + Send,
) {
    let mut original = settings_with_single_agent();
    original.sounds_enabled = true;
    original.theme_light = false;
    let state = state_for(original.clone());
    let expected_err = format!("simulated save failure for {field_name}");

    let err = persist_narrow_settings_update_with_saver(
        &state,
        |candidate| {
            mutate_candidate(candidate);
            assert_candidate(candidate);
        },
        |_| Err(expected_err.clone()),
    )
    .await
    .unwrap_err();

    assert_eq!(err, expected_err);
    let live = state.read().await;
    assert_eq!(live.sounds_enabled, original.sounds_enabled, "{field_name}");
    assert_eq!(live.theme_light, original.theme_light, "{field_name}");
}

#[tokio::test]
async fn narrow_settings_setter_save_failure_leaves_live_settings_unchanged() {
    assert_narrow_setter_save_failure_rolls_back(
        "sounds_enabled",
        |candidate| candidate.sounds_enabled = false,
        |candidate| assert!(!candidate.sounds_enabled),
    )
    .await;
    assert_narrow_setter_save_failure_rolls_back(
        "theme_light",
        |candidate| candidate.theme_light = true,
        |candidate| assert!(candidate.theme_light),
    )
    .await;
}

// ── #2306 P2 - ordered move + whole-settings order protection ────────

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

fn order_ids(agents: &[AgentConfig]) -> Vec<String> {
    agents.iter().map(|agent| agent.id.clone()).collect()
}

fn order_positions(agents: &[AgentConfig]) -> Vec<Option<u32>> {
    agents.iter().map(|agent| agent.order).collect()
}

fn disk_ids_and_orders(path: &Path) -> (Vec<String>, Vec<u64>) {
    // #2716 (B3): `agents` lives in the agents file beside settings.json.
    let raw = std::fs::read_to_string(path.with_file_name("agents.30.instance.no-git.json"))
        .expect("read agents file");
    let disk: Value = serde_json::from_str(&raw).expect("parse agents file");
    let agents = disk["agents"].as_array().expect("agents array");
    (
        agents
            .iter()
            .map(|agent| agent["id"].as_str().unwrap().to_string())
            .collect(),
        agents
            .iter()
            .map(|agent| agent["order"].as_u64().unwrap())
            .collect(),
    )
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

#[tokio::test]
async fn agent_order_update_keeps_live_survivors_and_appends_incoming_only() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let live = settings_with_agents(&["a", "b", "d"]);
    let state = state_for(live.clone());
    // The incoming snapshot carries a forged/stale ordinal order and the
    // incoming membership: `d` is gone, `c` is new, `b` was edited.
    let mut incoming = AppSettings {
        agents: vec![
            agent_with_order("c", Some(9)),
            agent_with_order("a", Some(50)),
            agent_with_order("b", Some(0)),
        ],
        ..live.clone()
    };
    incoming.agents[2].label = "b-edited".to_string();

    let (written, _) =
        persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();

    assert_eq!(order_ids(&written.agents), ["a", "b", "c"]);
    assert_eq!(
        order_positions(&written.agents),
        [Some(0), Some(1), Some(2)]
    );
    assert_eq!(written.agents[1].label, "b-edited");
    let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
    assert_eq!(disk_ids, ["a", "b", "c"]);
    assert_eq!(disk_orders, [0, 1, 2]);
    let reloaded = crate::config::settings::load_settings_from_path(&path);
    assert_eq!(order_ids(&reloaded.agents), ["a", "b", "c"]);
    let live_now = state.read().await;
    assert_eq!(order_ids(&live_now.agents), ["a", "b", "c"]);
}

#[tokio::test]
async fn agent_order_stale_draft_cannot_undo_a_move_and_keeps_incoming_records() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    // Live state after moving `b` above `a`.
    let state = state_for(settings_with_agents(&["b", "a", "c"]));
    // Stale draft taken before the move.
    let mut stale = settings_with_agents(&["a", "b", "c"]);
    stale.agents[1].label = "b-edited-in-draft".to_string();

    let (written, _events) = persist_settings_draft_update_with_saver(&state, stale, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(candidate, &path)
    })
    .await
    .unwrap();

    assert_eq!(order_ids(&written.agents), ["b", "a", "c"]);
    assert_eq!(written.agents[0].label, "b-edited-in-draft");
    assert_eq!(
        order_positions(&written.agents),
        [Some(0), Some(1), Some(2)]
    );
    let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
    assert_eq!(disk_ids, ["b", "a", "c"]);
    assert_eq!(disk_orders, [0, 1, 2]);
}

#[tokio::test]
async fn agent_order_draft_add_appends_and_remove_compacts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let live = settings_with_agents(&["a", "b", "c"]);
    let state = state_for(live.clone());

    // Remove `b`: survivors keep the live order and ordinals compact.
    let mut removed = live.clone();
    removed.agents.retain(|agent| agent.id != "b");
    let (written, _) = persist_settings_draft_update_with_saver(&state, removed, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(candidate, &path)
    })
    .await
    .unwrap();
    assert_eq!(order_ids(&written.agents), ["a", "c"]);
    assert_eq!(order_positions(&written.agents), [Some(0), Some(1)]);

    // Add `d` at the end of the effective list, as the modal does.
    let mut added = written.clone();
    added.agents.push(agent_with_order("d", None));
    let (written, _) = persist_settings_draft_update_with_saver(&state, added, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(candidate, &path)
    })
    .await
    .unwrap();
    assert_eq!(order_ids(&written.agents), ["a", "c", "d"]);
    assert_eq!(
        order_positions(&written.agents),
        [Some(0), Some(1), Some(2)]
    );
    let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
    assert_eq!(disk_ids, ["a", "c", "d"]);
    assert_eq!(disk_orders, [0, 1, 2]);
}

#[tokio::test]
async fn agent_order_case_distinct_ids_stay_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let state = state_for(settings_with_agents(&["a"]));
    let mut incoming = settings_with_agents(&["a", "A"]);
    // Case-distinct stable IDs are legal; inputs must not introduce a
    // duplicate configuration under the case-folded physical delta gate.
    incoming.agents[1].command = "uppercase-id-tool".to_string();

    let (written, _) =
        persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();

    assert_eq!(order_ids(&written.agents), ["a", "A"]);
    assert_ne!(written.agents[0].id_hash, written.agents[1].id_hash);
}

#[tokio::test]
async fn agent_order_draft_save_failure_leaves_live_settings_unchanged() {
    let state = state_for(settings_with_agents(&["a", "b", "c"]));
    let stale = settings_with_agents(&["c", "b", "a"]);

    let err = persist_settings_draft_update_with_saver(&state, stale, |_| {
        Err("simulated order save failure".to_string())
    })
    .await
    .unwrap_err();

    assert_eq!(err, "simulated order save failure");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
    assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
}

#[tokio::test]
async fn move_coding_agent_with_saver_returns_written_order_and_persists_ordinals() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    let ids =
        move_coding_agent_inner_with_saver(&state, "b", "a", AgentMoveDirection::Up, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate, &path,
            )
        })
        .await
        .unwrap();

    assert_eq!(ids, ["b", "a", "c"]);
    let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
    assert_eq!(disk_ids, ["b", "a", "c"]);
    assert_eq!(disk_orders, [0, 1, 2]);
    let reloaded = crate::config::settings::load_settings_from_path(&path);
    assert_eq!(order_ids(&reloaded.agents), ["b", "a", "c"]);
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["b", "a", "c"]);
    assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
}

#[tokio::test]
async fn move_coding_agent_with_saver_rejection_never_calls_saver() {
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    // `c` sits below `b`, so it is not `b`'s up-neighbor in the latest order.
    let err = move_coding_agent_inner_with_saver(
        &state,
        "b",
        "c",
        AgentMoveDirection::Up,
        |_| -> Result<AppSettings, String> { panic!("saver must not run on a rejected move") },
    )
    .await
    .unwrap_err();

    assert!(err.contains("not adjacent"), "{err}");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
}

#[tokio::test]
async fn move_coding_agent_with_saver_write_failure_leaves_live_state_unchanged() {
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    let err = move_coding_agent_inner_with_saver(&state, "b", "a", AgentMoveDirection::Up, |_| {
        Err("simulated move save failure".to_string())
    })
    .await
    .unwrap_err();

    assert_eq!(err, "simulated move save failure");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
    assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
}

#[tokio::test]
async fn move_coding_agent_checks_adjacency_against_the_latest_order() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    move_coding_agent_inner_with_saver(&state, "c", "b", AgentMoveDirection::Up, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(candidate, &path)
    })
    .await
    .unwrap();

    // Latest order is a, c, b: the stale neighbor is rejected without a save.
    let err = move_coding_agent_inner_with_saver(
        &state,
        "c",
        "b",
        AgentMoveDirection::Up,
        |_| -> Result<AppSettings, String> {
            panic!("saver must not run on a stale-neighbor move")
        },
    )
    .await
    .unwrap_err();

    assert!(err.contains("not adjacent"), "{err}");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "c", "b"]);
}

#[tokio::test]
async fn move_coding_agent_overlay_owned_agents_rejects_before_saver_without_event() {
    use tauri::Listener;

    let temp = tempfile::tempdir().unwrap();
    let path = seed_overlay_agents_fixture(
        temp.path(),
        &json!([
            { "id": "ov-a", "label": "A", "command": "claude", "color": "#111111", "order": 1 },
            { "id": "ov-b", "label": "B", "command": "claude", "color": "#222222" }
        ]),
    );
    let settings = crate::config::settings::load_settings_from_path(&path);
    assert!(settings
        .local_overlay_state
        .owns_top_level(crate::config::settings::OVERLAY_KEY_AGENTS));
    let overlay_order = order_ids(&settings.agents);
    let state = state_for(settings);

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build overlay rejection app");
    let (tx, rx) = std::sync::mpsc::channel();
    app.listen_any("coding_agent_settings_updated", move |event| {
        let _ = tx.send(event.payload().to_string());
    });

    let err = move_coding_agent_command_with_saver(
        app.handle(),
        &state,
        "ov-a".to_string(),
        "ov-b".to_string(),
        AgentMoveDirection::Up,
        |_| -> Result<AppSettings, String> {
            panic!("saver must not run while the overlay owns `agents`")
        },
    )
    .await
    .unwrap_err();

    assert_eq!(err, AGENT_ORDER_OVERLAY_PINNED);
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), overlay_order);
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a rejected move must not emit an event"
    );
}

fn abc_ids_2542() -> Vec<String> {
    ["a", "b", "c"].iter().map(|id| id.to_string()).collect()
}

fn mock_app_with_settings_listener_2542() -> (
    tauri::App<tauri::test::MockRuntime>,
    std::sync::mpsc::Receiver<String>,
) {
    use tauri::Listener;

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build reorder rejection app");
    let (tx, rx) = std::sync::mpsc::channel();
    app.listen_any("coding_agent_settings_updated", move |event| {
        let _ = tx.send(event.payload().to_string());
    });
    (app, rx)
}

#[tokio::test]
async fn reorder_coding_agent_with_saver_persists_and_returns_written_order() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    let ids = reorder_coding_agent_inner_with_saver(&state, "a", &abc_ids_2542(), 2, |c| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(c, &path)
    })
    .await
    .unwrap();

    assert_eq!(ids, ["b", "c", "a"]);
    let (disk_ids, disk_orders) = disk_ids_and_orders(&path);
    assert_eq!(disk_ids, ["b", "c", "a"]);
    assert_eq!(disk_orders, [0, 1, 2]);
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["b", "c", "a"]);
}

#[tokio::test]
async fn reorder_coding_agent_with_saver_write_failure_leaves_live_state_unchanged() {
    let state = state_for(settings_with_agents(&["a", "b", "c"]));

    let err = reorder_coding_agent_inner_with_saver(&state, "a", &abc_ids_2542(), 2, |_| {
        Err("simulated reorder save failure".to_string())
    })
    .await
    .unwrap_err();

    assert_eq!(err, "simulated reorder save failure");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
    assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
}

#[tokio::test]
async fn reorder_coding_agent_overlay_owned_agents_rejects_before_saver_without_event() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed_overlay_agents_fixture(
        temp.path(),
        &json!([
            { "id": "ov-a", "label": "A", "command": "claude", "color": "#111111", "order": 1 },
            { "id": "ov-b", "label": "B", "command": "claude", "color": "#222222" }
        ]),
    );
    let settings = crate::config::settings::load_settings_from_path(&path);
    let overlay_order = order_ids(&settings.agents);
    let state = state_for(settings);
    let (app, rx) = mock_app_with_settings_listener_2542();

    let err = reorder_coding_agent_command_with_saver(
        app.handle(),
        &state,
        "ov-a".to_string(),
        overlay_order.clone(),
        0,
        |_| -> Result<AppSettings, String> {
            panic!("saver must not run while the overlay owns `agents`")
        },
    )
    .await
    .unwrap_err();

    assert_eq!(err, AGENT_ORDER_OVERLAY_PINNED);
    assert_eq!(order_ids(&state.read().await.agents), overlay_order);
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a rejected reorder must not emit an event"
    );
}

#[tokio::test]
async fn reorder_coding_agent_stale_expected_ids_rejects_before_saver_without_event() {
    let state = state_for(settings_with_agents(&["a", "b", "c"]));
    let (app, rx) = mock_app_with_settings_listener_2542();

    let err = reorder_coding_agent_command_with_saver(
        app.handle(),
        &state,
        "a".to_string(),
        vec!["b".to_string(), "a".to_string(), "c".to_string()],
        2,
        |_| -> Result<AppSettings, String> { panic!("saver must not run on a stale reorder") },
    )
    .await
    .unwrap_err();

    assert!(err.contains("changed since"), "{err}");
    let live = state.read().await;
    assert_eq!(order_ids(&live.agents), ["a", "b", "c"]);
    assert_eq!(order_positions(&live.agents), [Some(0), Some(1), Some(2)]);
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a rejected reorder must not emit an event"
    );
}

#[test]
fn reorder_coding_agent_is_registered_in_invoke_handler() {
    let lines: Vec<&str> = include_str!("../../lib.rs")
        .lines()
        .map(str::trim)
        .collect();
    let hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| **line == "commands::config::reorder_coding_agent,")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(hits.len(), 1, "registration must occur exactly once");
    assert!(hits[0] > 0);
    assert_eq!(lines[hits[0] - 1], "commands::config::move_coding_agent,");
}

#[test]
fn settings_snapshot_exposes_overlay_owns_agents_false_and_not_in_base_settings() {
    let settings = settings_with_agents(&["a", "b"]);

    let snapshot = settings_snapshot_from(&settings, None);
    assert!(!snapshot.overlay_owns_agents);
    let wire = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(wire["overlayOwnsAgents"], json!(false));
    // Read-only disclosure: the base settings object never carries the key.
    let base = serde_json::to_value(&settings).unwrap();
    assert!(base.get("overlayOwnsAgents").is_none());
}

#[test]
fn settings_snapshot_reports_overlay_owned_agents_with_overlay_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed_overlay_agents_fixture(
        temp.path(),
        &json!([
            { "id": "ov-a", "label": "A", "command": "claude", "color": "#111111", "order": 2 },
            { "id": "ov-b", "label": "B", "command": "claude", "color": "#222222" },
            { "id": "ov-c", "label": "C", "command": "claude", "color": "#333333", "order": -1 },
            { "id": "ov-d", "label": "D", "command": "claude", "color": "#444444", "order": "two" },
            { "id": "ov-e", "label": "E", "command": "claude", "color": "#555555", "order": 1 }
        ]),
    );
    let settings = crate::config::settings::load_settings_from_path(&path);
    // Capture AFTER the loader: the first load of this minimal fixture may
    // run its one-time repair write. Snapshot building must add no write.
    let base_before = std::fs::read(&path).unwrap();
    let local_path = temp.path().join("settings.50.personal.no-git.json");
    let local_before = std::fs::read(&local_path).unwrap();
    assert!(settings
        .local_overlay_state
        .owns_top_level(crate::config::settings::OVERLAY_KEY_AGENTS));

    let snapshot = settings_snapshot_from(&settings, None);
    assert!(snapshot.overlay_owns_agents);
    let wire = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(wire["overlayOwnsAgents"], json!(true));
    // P1 effective order: valid ordinals first (1 -> ov-e, 2 -> ov-a), then
    // absent/invalid records by their original overlay index.
    let wire_ids: Vec<&str> = wire["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["id"].as_str().unwrap())
        .collect();
    assert_eq!(wire_ids, ["ov-b", "ov-e", "ov-a", "ov-c", "ov-d"]);
    // Building the snapshot must not touch either file.
    assert_eq!(std::fs::read(&path).unwrap(), base_before);
    assert_eq!(std::fs::read(&local_path).unwrap(), local_before);
}

#[tokio::test]
async fn settings_update_does_not_clobber_in_memory_project_lists() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let disk_project = real_ac_project(temp.path(), "disk-project-a");
    write_settings_file(
        temp.path(),
        &AppSettings {
            project_paths: vec![disk_project.clone()],
            project_path: Some(disk_project.clone()),
            ..AppSettings::default()
        },
    );

    let mut current = settings_with_single_agent();
    current.project_paths = vec!["C:/stale/current".to_string()];
    current.project_path = Some("C:/stale/current".to_string());
    current.root_token = Some("current-root-token".to_string());

    let protected_state = state_for(current.clone());
    let mut stale = current.clone();
    stale.project_paths.clear();
    stale.project_path = None;
    stale.root_token = Some("frontend-root-token".to_string());

    let (saved, _) =
        persist_protected_settings_update_with_saver(&protected_state, stale, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

    assert_single_project(&saved, &disk_project);
    assert_eq!(saved.root_token.as_deref(), Some("current-root-token"));
    {
        let live = protected_state.read().await;
        assert_single_project(&live, &disk_project);
        assert_eq!(live.root_token.as_deref(), Some("current-root-token"));
    }

    let draft_state = state_for(current.clone());
    let mut draft = current;
    draft.project_paths.clear();
    draft.project_path = None;

    let (saved, _events) =
        persist_settings_draft_update_with_saver(&draft_state, draft, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

    assert_single_project(&saved, &disk_project);
    {
        let live = draft_state.read().await;
        assert_single_project(&live, &disk_project);
    }
}

#[tokio::test]
async fn update_settings_adopts_canonical_archive_without_resurrecting_absent_archive() {
    for canonical in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");
        let archived_project = real_ac_project(temp.path(), "archived-b");
        write_settings_file_without_keys(
            temp.path(),
            &AppSettings {
                project_paths: vec![live_project.clone()],
                project_path: Some(live_project.clone()),
                ..AppSettings::default()
            },
            &["archivedProjectPaths"],
        );
        let sidecar = temp.path().join("project-paths.json");
        if canonical {
            let mut managed: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
            for key in [
                "projectPath",
                "projectPathRelativeToInstance",
                "projectPaths",
                "projectPathsRelativeToInstance",
                "archivedProjectPaths",
                "archivedProjectPathsRelativeToInstance",
            ] {
                managed.as_object_mut().unwrap().remove(key);
            }
            std::fs::write(&settings_path, serde_json::to_vec(&managed).unwrap()).unwrap();
            std::fs::write(
                &sidecar,
                serde_json::to_vec(&serde_json::json!({
                    "projectPath":live_project, "projectPathRelativeToInstance":null,
                    "projectPaths":[live_project], "projectPathsRelativeToInstance":[null],
                    "archivedProjectPaths":[archived_project],
                    "archivedProjectPathsRelativeToInstance":[null]
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let canonical_bytes = canonical.then(|| std::fs::read(&sidecar).unwrap());
        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.clone()];
        current.project_path = Some(live_project.clone());
        current.archived_project_paths = vec![archived_project.clone()];
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["archivedProjectPaths"]);
        let (saved, _) =
            persist_protected_settings_update_with_saver(&state, payload, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();
        let disk = crate::config::settings::load_settings_from_path(&settings_path);
        let live = state.read().await;
        for adopted in [&saved, &*live, &disk] {
            assert_single_project(adopted, &live_project);
            if canonical {
                assert_eq!(
                    adopted.archived_project_paths,
                    vec![archived_project.clone()]
                );
            } else {
                assert!(adopted.archived_project_paths.is_empty());
            }
            assert_eq!(
                session_retention_project_paths(adopted)
                    .unwrap()
                    .contains(&archived_project),
                canonical
            );
        }
        let managed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        for key in [
            "projectPath",
            "projectPathRelativeToInstance",
            "projectPaths",
            "projectPathsRelativeToInstance",
            "archivedProjectPaths",
            "archivedProjectPathsRelativeToInstance",
        ] {
            assert!(
                managed.get(key).is_none(),
                "managed settings resurrected {key}"
            );
        }
        if let Some(bytes) = canonical_bytes {
            assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
        } else {
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            for key in &[
                "archivedProjectPaths",
                "archivedProjectPathsRelativeToInstance",
            ][..]
            {
                assert!(raw.get(*key).is_none(), "raw authority resurrected {key}");
            }
        }
    }
}

#[tokio::test]
async fn save_settings_draft_adopts_canonical_archive_without_resurrecting_absent_archive() {
    for canonical in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");
        let archived_project = real_ac_project(temp.path(), "archived-b");
        write_settings_file_without_keys(
            temp.path(),
            &AppSettings {
                project_paths: vec![live_project.clone()],
                project_path: Some(live_project.clone()),
                ..AppSettings::default()
            },
            &["archivedProjectPaths"],
        );
        let sidecar = temp.path().join("project-paths.json");
        if canonical {
            let mut managed: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
            for key in [
                "projectPath",
                "projectPathRelativeToInstance",
                "projectPaths",
                "projectPathsRelativeToInstance",
                "archivedProjectPaths",
                "archivedProjectPathsRelativeToInstance",
            ] {
                managed.as_object_mut().unwrap().remove(key);
            }
            std::fs::write(&settings_path, serde_json::to_vec(&managed).unwrap()).unwrap();
            std::fs::write(
                &sidecar,
                serde_json::to_vec(&serde_json::json!({
                    "projectPath":live_project, "projectPathRelativeToInstance":null,
                    "projectPaths":[live_project], "projectPathsRelativeToInstance":[null],
                    "archivedProjectPaths":[archived_project],
                    "archivedProjectPathsRelativeToInstance":[null]
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let canonical_bytes = canonical.then(|| std::fs::read(&sidecar).unwrap());
        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.clone()];
        current.project_path = Some(live_project.clone());
        current.archived_project_paths = vec![archived_project.clone()];
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["archivedProjectPaths"]);
        let (saved, _events) =
            persist_settings_draft_update_with_saver(&state, payload, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();
        let disk = crate::config::settings::load_settings_from_path(&settings_path);
        let live = state.read().await;
        for adopted in [&saved, &*live, &disk] {
            assert_single_project(adopted, &live_project);
            if canonical {
                assert_eq!(
                    adopted.archived_project_paths,
                    vec![archived_project.clone()]
                );
            } else {
                assert!(adopted.archived_project_paths.is_empty());
            }
            assert_eq!(
                session_retention_project_paths(adopted)
                    .unwrap()
                    .contains(&archived_project),
                canonical
            );
        }
        let managed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        for key in [
            "projectPath",
            "projectPathRelativeToInstance",
            "projectPaths",
            "projectPathsRelativeToInstance",
            "archivedProjectPaths",
            "archivedProjectPathsRelativeToInstance",
        ] {
            assert!(
                managed.get(key).is_none(),
                "managed settings resurrected {key}"
            );
        }
        if let Some(bytes) = canonical_bytes {
            assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
        } else {
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            for key in &[
                "archivedProjectPaths",
                "archivedProjectPathsRelativeToInstance",
            ][..]
            {
                assert!(raw.get(*key).is_none(), "raw authority resurrected {key}");
            }
        }
    }
}

#[tokio::test]
async fn update_settings_adopts_canonical_projects_without_resurrecting_absent_authority() {
    for canonical in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");
        let sidecar = temp.path().join("project-paths.json");
        if canonical {
            std::fs::write(
                &sidecar,
                serde_json::to_vec(&serde_json::json!({
                    "projectPath":live_project, "projectPathRelativeToInstance":null,
                    "projectPaths":[live_project], "projectPathsRelativeToInstance":[null],
                    "archivedProjectPaths":[], "archivedProjectPathsRelativeToInstance":[]
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let canonical_bytes = canonical.then(|| std::fs::read(&sidecar).unwrap());
        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.clone()];
        current.project_path = Some(live_project.clone());
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["projectPaths", "projectPath"]);
        let (saved, _) =
            persist_protected_settings_update_with_saver(&state, payload, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();
        let disk = crate::config::settings::load_settings_from_path(&settings_path);
        let live = state.read().await;
        for adopted in [&saved, &*live, &disk] {
            if canonical {
                assert_single_project(adopted, &live_project);
            } else {
                assert!(adopted.project_paths.is_empty());
                assert!(adopted.project_path.is_none());
            }
            assert_eq!(
                session_retention_project_paths(adopted)
                    .unwrap()
                    .contains(&live_project),
                canonical
            );
        }
        let managed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        for key in [
            "projectPath",
            "projectPathRelativeToInstance",
            "projectPaths",
            "projectPathsRelativeToInstance",
            "archivedProjectPaths",
            "archivedProjectPathsRelativeToInstance",
        ] {
            assert!(
                managed.get(key).is_none(),
                "managed settings resurrected {key}"
            );
        }
        if let Some(bytes) = canonical_bytes {
            assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
        } else {
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            for key in &[
                "projectPath",
                "projectPathRelativeToInstance",
                "projectPaths",
                "projectPathsRelativeToInstance",
            ][..]
            {
                assert!(raw.get(*key).is_none(), "raw authority resurrected {key}");
            }
        }
    }
}

#[tokio::test]
async fn save_settings_draft_adopts_canonical_projects_without_resurrecting_absent_authority() {
    for canonical in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        let live_project = real_ac_project(temp.path(), "live-a");
        let sidecar = temp.path().join("project-paths.json");
        if canonical {
            std::fs::write(
                &sidecar,
                serde_json::to_vec(&serde_json::json!({
                    "projectPath":live_project, "projectPathRelativeToInstance":null,
                    "projectPaths":[live_project], "projectPathsRelativeToInstance":[null],
                    "archivedProjectPaths":[], "archivedProjectPathsRelativeToInstance":[]
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let canonical_bytes = canonical.then(|| std::fs::read(&sidecar).unwrap());
        let mut current = settings_with_single_agent();
        current.project_paths = vec![live_project.clone()];
        current.project_path = Some(live_project.clone());
        let state = state_for(current.clone());
        let payload = settings_payload_without_keys(&current, &["projectPaths", "projectPath"]);
        let (saved, _events) =
            persist_settings_draft_update_with_saver(&state, payload, |candidate| {
                crate::config::settings::save_settings_to_path_preserving_project_paths(
                    candidate,
                    &settings_path,
                )
            })
            .await
            .unwrap();
        let disk = crate::config::settings::load_settings_from_path(&settings_path);
        let live = state.read().await;
        for adopted in [&saved, &*live, &disk] {
            if canonical {
                assert_single_project(adopted, &live_project);
            } else {
                assert!(adopted.project_paths.is_empty());
                assert!(adopted.project_path.is_none());
            }
            assert_eq!(
                session_retention_project_paths(adopted)
                    .unwrap()
                    .contains(&live_project),
                canonical
            );
        }
        let managed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        for key in [
            "projectPath",
            "projectPathRelativeToInstance",
            "projectPaths",
            "projectPathsRelativeToInstance",
            "archivedProjectPaths",
            "archivedProjectPathsRelativeToInstance",
        ] {
            assert!(
                managed.get(key).is_none(),
                "managed settings resurrected {key}"
            );
        }
        if let Some(bytes) = canonical_bytes {
            assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
        } else {
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            for key in &[
                "projectPath",
                "projectPathRelativeToInstance",
                "projectPaths",
                "projectPathsRelativeToInstance",
            ][..]
            {
                assert!(raw.get(*key).is_none(), "raw authority resurrected {key}");
            }
        }
    }
}

#[tokio::test]
async fn update_settings_still_takes_disk_archived_list_when_key_present() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let live_project = real_ac_project(temp.path(), "live-a");
    let disk_archived = real_ac_project(temp.path(), "archived-a");
    let live_archived = "C:/archived/b"; // distractor: overridden by disk
    let payload_archived = "C:/archived/c"; // distractor: overridden by disk
    write_settings_file(
        temp.path(),
        &AppSettings {
            project_paths: vec![live_project.clone()],
            project_path: Some(live_project.clone()),
            archived_project_paths: vec![disk_archived.clone()],
            ..AppSettings::default()
        },
    );

    let mut current = settings_with_single_agent();
    current.project_paths = vec![live_project.to_string()];
    current.project_path = Some(live_project.to_string());
    current.archived_project_paths = vec![live_archived.to_string()];
    let state = state_for(current.clone());
    let mut payload = settings_payload_without_keys(&current, &[]);
    payload.archived_project_paths = vec![payload_archived.to_string()];

    let (saved, _) = persist_protected_settings_update_with_saver(&state, payload, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(
            candidate,
            &settings_path,
        )
    })
    .await
    .unwrap();

    assert_eq!(
        saved.archived_project_paths,
        vec![disk_archived.to_string()]
    );
    {
        let live = state.read().await;
        assert_eq!(live.archived_project_paths, vec![disk_archived.to_string()]);
    }
    let disk: AppSettings =
        crate::config::settings::load_settings_from_path(std::path::Path::new(&settings_path));
    assert_eq!(disk.archived_project_paths, vec![disk_archived.to_string()]);
}

/// Build a pending (companion-population-eligible) SettingsState by decoding a
/// legacy `[project]` file (no companion) from `settings_path`.
fn pending_state_from_legacy(settings_path: &Path, project: &str) -> SettingsState {
    write_settings_file(
        settings_path.parent().unwrap(),
        &AppSettings {
            project_paths: vec![project.to_string()],
            project_path: Some(project.to_string()),
            ..AppSettings::default()
        },
    );
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(settings_path).unwrap()).unwrap();
    let decoded = crate::config::settings::decode_project_state(
        value.as_object().unwrap(),
        None,
        &crate::config::projects::FsCandidateResolver,
    );
    let startup = AppSettings {
        project_paths: decoded.active_selected(),
        project_path: decoded.selected_head.clone(),
        project_path_state: std::sync::Arc::new(decoded),
        ..AppSettings::default()
    };
    assert!(
        startup.project_path_state.active_reconcile_eligible,
        "legacy no-companion state should be pending"
    );
    state_for(startup)
}

#[tokio::test]
async fn auto_reconcile_redecodes_and_preserves_intervening_cli_write() {
    // Grinch Defect 2: the get_settings boundary must re-decode disk before
    // reconciling, so a CLI registration that landed after startup survives.
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let a = real_ac_project(temp.path(), "A");
    let b = real_ac_project(temp.path(), "B");
    let state = pending_state_from_legacy(&settings_path, &a);

    // Intervening CLI registration: disk now carries [A, B].
    write_settings_file(
        temp.path(),
        &AppSettings {
            project_paths: vec![a.clone(), b.clone()],
            project_path: Some(a.clone()),
            ..AppSettings::default()
        },
    );

    let _snap = super::settings_snapshot_helper(&state, Some(settings_path.clone())).await;

    let disk: AppSettings =
        crate::config::settings::load_settings_from_path(std::path::Path::new(&settings_path));
    assert_eq!(
        disk.project_paths,
        vec![a.clone(), b.clone()],
        "CLI-registered B must not be clobbered by the auto-reconcile"
    );
    let live = state.read().await;
    assert_eq!(live.project_paths, vec![a, b]);
}

#[tokio::test]
async fn auto_reconcile_reports_stage_read_on_corrupt_disk() {
    // Grinch Defect 2: a disk re-decode failure at the boundary reports
    // stage `read`, performs no write, and retains the prior state.
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let a = real_ac_project(temp.path(), "A");
    let state = pending_state_from_legacy(&settings_path, &a);

    // Corrupt the settings file after startup.
    std::fs::write(&settings_path, b"{ not valid json").unwrap();
    let snap = super::settings_snapshot_helper(&state, Some(settings_path.clone())).await;

    let err = snap
        .project_path_resolution
        .reconciliation_error
        .expect("a corrupt disk re-decode must surface a reconciliation error");
    assert!(matches!(err.stage, super::ReconciliationStage::Read));
    assert!(err.retryable);
    // No write: the corrupt bytes are untouched.
    assert_eq!(
        std::fs::read_to_string(&settings_path).unwrap(),
        "{ not valid json"
    );
}

#[tokio::test]
async fn unavailable_snapshot_blocks_purge_even_without_cached_repair() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let state = state_for(AppSettings::default());
    std::fs::write(&settings_path, b"invalid").unwrap();
    write_sessions_file(
        temp.path(),
        &[PersistedSession {
            name: "retained".into(),
            shell: "codex".into(),
            working_directory: temp.path().to_string_lossy().into_owned(),
            ..Default::default()
        }],
    );
    let session_path = temp.path().join("sessions.json");
    let original = std::fs::read(&session_path).unwrap();
    let snapshot = super::settings_snapshot_helper(&state, Some(settings_path.clone())).await;
    let error = snapshot
        .project_path_resolution
        .reconciliation_error
        .unwrap();
    assert!(matches!(error.stage, super::ReconciliationStage::Read));
    let saved = state.read().await.clone();
    assert!(saved.project_paths_persistence_error.is_some());
    assert!(
        purge_sessions_after_settings_update_in_dir(&saved, temp.path())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&session_path).unwrap(), original);
    assert_eq!(std::fs::read(&settings_path).unwrap(), b"invalid");
}

#[tokio::test]
async fn settings_update_returns_written_project_lists_for_session_purge() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("project-a");
    let kept_workdir = project.join(".ac").join("wg-1").join("__agent_dev");
    let outside = temp.path().join("project-b");
    let outside_workdir = outside.join(".ac").join("wg-1").join("__agent_dev");
    std::fs::create_dir_all(&kept_workdir).expect("create kept workdir");
    std::fs::create_dir_all(&outside_workdir).expect("create outside workdir");

    let project_path = canonical_display(&project);
    write_settings_file(
        temp.path(),
        &AppSettings {
            project_paths: vec![project_path.clone()],
            project_path: Some(project_path.clone()),
            ..AppSettings::default()
        },
    );
    write_sessions_file(
        temp.path(),
        &[
            PersistedSession {
                name: "kept".to_string(),
                shell: "codex".to_string(),
                working_directory: kept_workdir.to_string_lossy().to_string(),
                ..Default::default()
            },
            PersistedSession {
                name: "outside".to_string(),
                shell: "codex".to_string(),
                working_directory: outside_workdir.to_string_lossy().to_string(),
                ..Default::default()
            },
        ],
    );

    let mut current = settings_with_single_agent();
    current.project_paths.clear();
    current.project_path = None;
    let state = state_for(current.clone());
    let stale = current;

    let (saved, _) = persist_protected_settings_update_with_saver(&state, stale, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(
            candidate,
            &settings_path,
        )
    })
    .await
    .unwrap();

    purge_sessions_after_settings_update_in_dir(&saved, temp.path())
        .await
        .unwrap();

    let remaining = read_sessions_file(temp.path());
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "kept");
    assert_eq!(
        remaining[0].working_directory,
        kept_workdir.to_string_lossy()
    );
    assert_single_project(&saved, &project_path);
}

#[tokio::test]
async fn settings_update_missing_settings_file_preserves_project_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    assert!(!settings_path.exists());

    let project = temp.path().join("project-a");
    let kept_workdir = project.join(".ac").join("wg-1").join("__agent_dev");
    std::fs::create_dir_all(&kept_workdir).expect("create kept workdir");
    let project_path = canonical_display(&project);

    write_sessions_file(
        temp.path(),
        &[PersistedSession {
            name: "kept".to_string(),
            shell: "codex".to_string(),
            working_directory: kept_workdir.to_string_lossy().to_string(),
            ..Default::default()
        }],
    );

    let mut current = settings_with_single_agent();
    current.project_paths = vec![project_path.clone()];
    current.project_path = Some(project_path.clone());
    let state = state_for(current.clone());

    // Seed legacy persisted authority before Preserve, independent of renderer fields.
    std::fs::write(
        &settings_path,
        serde_json::json!({
            "projectPaths": [project_path.clone()],
            "projectPath": project_path.clone(),
            "defaultShell": "test-shell",
            "defaultShellArgs": [],
            "agents": [],
        })
        .to_string(),
    )
    .unwrap();
    let mut incoming = current.clone();
    incoming.sidebar_style = "deep-space".to_string();
    let (saved, _) = persist_protected_settings_update_with_saver(&state, incoming, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(
            candidate,
            &settings_path,
        )
    })
    .await
    .unwrap();

    assert_single_project(&saved, &project_path);
    {
        let live = state.read().await;
        assert_single_project(&live, &project_path);
    }
    assert!(settings_path.exists(), "writer must create settings.json");
    let sidecar = settings_path.with_file_name("project-paths.json");
    let sidecar_present = sidecar.try_exists().expect("inspect project authority");
    let authority = if sidecar_present {
        &sidecar
    } else {
        &settings_path
    };
    let disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(authority).unwrap()).unwrap();
    if sidecar_present {
        let legacy: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        for key in [
            "projectPath",
            "projectPaths",
            "archivedProjectPaths",
            "projectPathRelativeToInstance",
            "projectPathsRelativeToInstance",
            "archivedProjectPathsRelativeToInstance",
        ] {
            assert!(
                legacy.get(key).is_none(),
                "legacy project key remains: {key}"
            );
        }
    }
    assert_eq!(
        disk["projectPaths"],
        serde_json::json!([project_path.clone()])
    );
    assert_eq!(disk["projectPath"], serde_json::json!(project_path));

    purge_sessions_after_settings_update_in_dir(&saved, temp.path())
        .await
        .unwrap();

    let remaining = read_sessions_file(temp.path());
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "kept");
    assert_eq!(
        remaining[0].working_directory,
        kept_workdir.to_string_lossy()
    );
}

#[tokio::test]
async fn set_rail_collapse_persists_both_fields_and_publishes_to_live_state() {
    let mut original = settings_with_single_agent();
    original.rail_collapsed_projects = vec!["c:/stale".to_string()];
    original.rail_favorites_collapsed = false;
    let state = state_for(original);

    // Drives the REAL `set_rail_collapse_inner` mutation through its saver seam, so
    // dropping or swapping a field assignment inside that fn fails this test. The fake
    // saver keeps it off disk: the real `save_settings` resolves `config::config_dir()`,
    // a `OnceLock` no test redirects, which would overwrite the developer's settings.json.
    let captured: Mutex<Option<AppSettings>> = Mutex::new(None);
    set_rail_collapse_inner_with_saver(
        &state,
        vec!["c:/foo".to_string(), "d:/bar".to_string()],
        true,
        |candidate| {
            *captured.lock().expect("capture lock") = Some(candidate.clone());
            Ok(candidate.clone())
        },
    )
    .await
    .expect("persist rail collapse");

    let written = captured
        .lock()
        .expect("capture lock")
        .clone()
        .expect("saver ran");
    assert_eq!(
        written.rail_collapsed_projects,
        vec!["c:/foo".to_string(), "d:/bar".to_string()]
    );
    assert!(written.rail_favorites_collapsed);

    let live = state.read().await;
    assert_eq!(
        live.rail_collapsed_projects,
        vec!["c:/foo".to_string(), "d:/bar".to_string()]
    );
    assert!(live.rail_favorites_collapsed);
}

// (#965 F1) The two whole-object writers must not let a stale client payload
// revert the rail snapshot. Without these, `window-geometry.ts` alone (which
// fires a whole-object write on every window move/resize) would silently wipe
// a header click, and nothing re-persists it: the in-memory signal is never
// invalidated, so it does NOT self-heal.
#[tokio::test]
async fn update_settings_keeps_live_rail_collapse_when_payload_is_stale() {
    let mut current = settings_with_single_agent();
    current.rail_collapsed_projects = vec!["c:/foo".to_string()];
    current.rail_favorites_collapsed = true;
    let state = state_for(current);

    let mut stale = settings_with_single_agent();
    stale.rail_collapsed_projects = Vec::new();
    stale.rail_favorites_collapsed = false;

    let (saved, _) = persist_protected_settings_update_with_saver(&state, stale, |c| Ok(c.clone()))
        .await
        .expect("persist");

    assert_eq!(saved.rail_collapsed_projects, vec!["c:/foo".to_string()]);
    assert!(saved.rail_favorites_collapsed);
    let live = state.read().await;
    assert_eq!(live.rail_collapsed_projects, vec!["c:/foo".to_string()]);
    assert!(live.rail_favorites_collapsed);
}

#[tokio::test]
async fn save_settings_draft_keeps_live_rail_collapse_when_draft_is_stale() {
    let mut current = settings_with_single_agent();
    current.rail_collapsed_projects = vec!["c:/foo".to_string()];
    current.rail_favorites_collapsed = true;
    let state = state_for(current);

    let mut draft = settings_with_single_agent();
    draft.rail_collapsed_projects = Vec::new();
    draft.rail_favorites_collapsed = false;

    let (saved, _events) =
        persist_settings_draft_update_with_saver(&state, draft, |c| Ok(c.clone()))
            .await
            .expect("persist");

    assert_eq!(saved.rail_collapsed_projects, vec!["c:/foo".to_string()]);
    assert!(saved.rail_favorites_collapsed);
    let live = state.read().await;
    assert_eq!(live.rail_collapsed_projects, vec!["c:/foo".to_string()]);
    assert!(live.rail_favorites_collapsed);
}

#[tokio::test]
async fn protected_update_settings_transaction_preserves_current_coding_fields() {
    let mut current = settings_with_single_agent();
    current.agents[0].envs = vec![CodingAgentEnv {
        key: "OPENAI_API_BASE".to_string(),
        value: "current".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    }];
    current.agents[0].isolated_home = true;
    current
        .coding_agent_profiles
        .profiles_by_agent
        .entry("agent-0".to_string())
        .or_default()
        .insert(
            "B".to_string(),
            ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: "codex --current-profile".to_string(),
                env: BTreeMap::new(),
                notes: String::new(),
            },
        );
    let state = state_for(current.clone());

    let mut stale = current.clone();
    stale.sidebar_style = "command-center".to_string();
    stale.agents[0].envs.clear();
    stale.agents[0].isolated_home = false;
    stale.coding_agent_profiles.profiles_by_agent.clear();

    let (saved, _) = persist_protected_settings_update_with_saver(&state, stale, |candidate| {
        Ok(candidate.clone())
    })
    .await
    .unwrap();

    assert_eq!(saved.sidebar_style, "command-center");
    assert_eq!(saved.agents[0].envs, current.agents[0].envs);
    assert_eq!(
        saved.agents[0].isolated_home,
        current.agents[0].isolated_home
    );
    assert!(saved
        .coding_agent_profiles
        .profiles_by_agent
        .get("agent-0")
        .is_some_and(|cells| cells.contains_key("B")));
    let live = state.read().await;
    assert_eq!(live.sidebar_style, "command-center");
    assert_eq!(live.agents[0].envs, current.agents[0].envs);
    assert_eq!(
        live.agents[0].isolated_home,
        current.agents[0].isolated_home
    );
    assert!(live
        .coding_agent_profiles
        .profiles_by_agent
        .get("agent-0")
        .is_some_and(|cells| cells.contains_key("B")));
}

// ─────────────────────────────────────────────────────────────────────────
// #1737 D14 - the two payload-constructed `AppSettings` carry-overs, and D13's
// narrowing on the three other production writers of `codingAgentProfiles`.
// Plan section 11, P1 to P7.
// ─────────────────────────────────────────────────────────────────────────
mod local_overlay_1737;

// ── #1941 scoped selection-lock API ─────────────────────────────

struct SelectionApiFixture {
    _temp: tempfile::TempDir,
    project: PathBuf,
    ac_root: PathBuf,
}

fn selection_api_fixture() -> SelectionApiFixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let ac_root = project.join(".ac");
    std::fs::create_dir_all(&ac_root).expect("create .ac");
    SelectionApiFixture {
        _temp: temp,
        project,
        ac_root,
    }
}

fn selection_api_settings(fixture: &SelectionApiFixture) -> AppSettings {
    let mut settings = settings_with_single_agent();
    settings.project_paths = vec![fixture.project.to_string_lossy().to_string()];
    settings
}

/// Create a valid `_agent_<name>` Matrix plus a `room-*` replica whose
/// config.json stores the given tooling block and the correct identity.
fn selection_api_replica(
    fixture: &SelectionApiFixture,
    room: &str,
    name: &str,
    tooling: serde_json::Value,
) -> PathBuf {
    let matrix = fixture.ac_root.join(format!("_agent_{name}"));
    let replica = fixture.ac_root.join(room).join(format!("__agent_{name}"));
    selection_api_replica_at(fixture, &matrix, &replica, name, tooling);
    replica
}

fn selection_api_replica_at(
    _fixture: &SelectionApiFixture,
    matrix: &Path,
    replica: &Path,
    name: &str,
    tooling: serde_json::Value,
) {
    std::fs::create_dir_all(matrix).expect("create matrix");
    std::fs::create_dir_all(replica).expect("create replica");
    std::fs::write(matrix.join("Role.md"), "# Role\n").expect("write Role.md");
    let config = json!({
        "identity": format!("../../_agent_{name}"),
        "tooling": tooling,
    });
    std::fs::write(
        replica.join("config.json"),
        serde_json::to_vec(&config).expect("serialize config"),
    )
    .expect("write config");
}

fn locked_tooling(profile: &str, agent: &str) -> serde_json::Value {
    json!({
        "profile": profile,
        "currentCodingAgent": agent,
        "selectionLocked": true,
    })
}

fn unlocked_tooling(profile: &str, agent: &str) -> serde_json::Value {
    json!({
        "profile": profile,
        "currentCodingAgent": agent,
    })
}

fn config_bytes(replica: &Path) -> Vec<u8> {
    std::fs::read(replica.join("config.json")).expect("read config.json")
}

fn sidecar_path(replica: &Path) -> PathBuf {
    replica.join(".config.json.lock")
}

fn empty_session_manager() -> Arc<RwLock<SessionManager>> {
    Arc::new(RwLock::new(SessionManager::new()))
}

fn api_preview_request(
    path: &Path,
    scope: super::ProfileAssignmentScope,
    mode: super::AssignmentMode,
) -> super::PreviewCodingAgentProfileSelectionRequest {
    super::PreviewCodingAgentProfileSelectionRequest {
        target_replica_path: path.to_string_lossy().to_string(),
        coding_agent_id: "agent-0".to_string(),
        profile: "B".to_string(),
        scope,
        restart_sessions: false,
        assignment_mode: mode,
    }
}

fn api_apply_request(
    path: &Path,
    scope: super::ProfileAssignmentScope,
    mode: super::AssignmentMode,
    decision: Option<super::ConflictDecision>,
    fingerprint: Option<&str>,
) -> super::ApplyCodingAgentProfileSelectionRequest {
    super::ApplyCodingAgentProfileSelectionRequest {
        target_replica_path: path.to_string_lossy().to_string(),
        coding_agent_id: "agent-0".to_string(),
        profile: "B".to_string(),
        scope,
        restart_sessions: false,
        confirmed_target_fingerprint: fingerprint.map(str::to_string),
        typed_confirmation: None,
        assignment_mode: mode,
        conflict_decision: decision,
    }
}

fn api_removal_preview_request(
    path: &Path,
    scope: super::ProfileAssignmentScope,
) -> super::PreviewSelectionLockRemovalRequest {
    super::PreviewSelectionLockRemovalRequest {
        target_replica_path: path.to_string_lossy().to_string(),
        scope,
    }
}

fn api_removal_apply_request(
    path: &Path,
    scope: super::ProfileAssignmentScope,
    fingerprint: &str,
) -> super::ApplySelectionLockRemovalRequest {
    super::ApplySelectionLockRemovalRequest {
        target_replica_path: path.to_string_lossy().to_string(),
        scope,
        confirmed_target_fingerprint: fingerprint.to_string(),
    }
}

fn api_default_request(path: &Path) -> super::GetReplicaSelectionDefaultRequest {
    super::GetReplicaSelectionDefaultRequest {
        target_replica_path: path.to_string_lossy().to_string(),
    }
}

fn api_set_default_request(
    path: &Path,
    agent: &str,
    profile: &str,
    locked: bool,
    fingerprint: &str,
) -> super::SetReplicaSelectionDefaultRequest {
    super::SetReplicaSelectionDefaultRequest {
        target_replica_path: path.to_string_lossy().to_string(),
        coding_agent_id: agent.to_string(),
        requested_profile: profile.to_string(),
        selection_locked: locked,
        confirmed_default_fingerprint: fingerprint.to_string(),
    }
}

async fn api_preview(
    settings: &SettingsState,
    path: &Path,
    scope: super::ProfileAssignmentScope,
    mode: super::AssignmentMode,
) -> super::PreviewCodingAgentProfileSelectionResult {
    api_preview_result(settings, path, scope, mode)
        .await
        .expect("assignment preview")
}

async fn api_preview_result(
    settings: &SettingsState,
    path: &Path,
    scope: super::ProfileAssignmentScope,
    mode: super::AssignmentMode,
) -> Result<super::PreviewCodingAgentProfileSelectionResult, String> {
    super::preview_coding_agent_profile_selection_inner(
        &empty_session_manager(),
        settings,
        api_preview_request(path, scope, mode),
    )
    .await
}

/// Minimal PTY backend for the non-restarting apply paths: the manager is
/// required by the command signature but no spawn/kill is ever requested.
struct SelectionApiPtyBackend;

impl crate::pty::backend::PtyBackend for SelectionApiPtyBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        _spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        _id: uuid::Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn resize(
        &self,
        _id: uuid::Uuid,
        _cols: u16,
        _rows: u16,
    ) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn kill(&self, _id: uuid::Uuid) -> Result<(), crate::errors::AppError> {
        Ok(())
    }

    fn has_session(&self, _id: uuid::Uuid) -> bool {
        false
    }

    fn get_screen_snapshot(
        &self,
        _id: uuid::Uuid,
    ) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: uuid::Uuid) -> Option<(u16, u16)> {
        None
    }

    fn get_screen_rows(&self, _id: uuid::Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(
        &self,
        _session_id: uuid::Uuid,
        _request_id: String,
        _response_dir: PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, _id: uuid::Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

async fn api_apply(
    settings: &SettingsState,
    path: &Path,
    scope: super::ProfileAssignmentScope,
    mode: super::AssignmentMode,
    decision: Option<super::ConflictDecision>,
    fingerprint: Option<&str>,
) -> Result<super::ApplyCodingAgentProfileSelectionResult, String> {
    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build selection api test app");
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        Arc::new(SelectionApiPtyBackend),
    )));
    super::apply_coding_agent_profile_selection_inner(
        app.handle(),
        &empty_session_manager(),
        &pty_mgr,
        settings,
        api_apply_request(path, scope, mode, decision, fingerprint),
    )
    .await
    .map(|(result, _payload)| result)
}

async fn api_default(
    settings: &SettingsState,
    path: &Path,
) -> super::ReplicaSelectionDefaultResult {
    super::get_replica_selection_default_inner(settings, api_default_request(path))
        .await
        .expect("default read")
}

async fn api_removal_preview(
    settings: &SettingsState,
    path: &Path,
    scope: super::ProfileAssignmentScope,
) -> super::PreviewSelectionLockRemovalResult {
    super::preview_selection_lock_removal_inner(
        &empty_session_manager(),
        settings,
        api_removal_preview_request(path, scope),
    )
    .await
    .expect("removal preview")
}

async fn api_removal_apply(
    settings: &SettingsState,
    path: &Path,
    scope: super::ProfileAssignmentScope,
    fingerprint: &str,
) -> Result<super::ApplySelectionLockRemovalResult, String> {
    super::apply_selection_lock_removal_inner(
        settings,
        api_removal_apply_request(path, scope, fingerprint),
    )
    .await
    .map(|(result, _payload)| result)
}

#[tokio::test]
async fn issue_1937_selection_api_preview_replica_reports_saved_pair_and_state() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview = api_preview(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary,
    )
    .await;

    assert_eq!(preview.scope, super::ProfileAssignmentScope::Replica);
    assert!(preview.counts_complete);
    assert_eq!(preview.candidate_count, 1);
    assert_eq!(preview.protected_count, 1);
    assert_eq!(preview.invalid_count, 0);
    assert_eq!(preview.conflict_count, 0);
    assert!(preview.decisions.is_none());
    assert_eq!(
        preview.target_count, 1,
        "a deliberate replica target counts"
    );
    assert_eq!(preview.targets.len(), 1);
    let target = &preview.targets[0];
    assert_eq!(target.selection_state, super::SelectionState::Locked);
    assert_eq!(
        target.saved_pair,
        Some(super::SavedPair {
            coding_agent_id: Some("agent-0".to_string()),
            requested_profile: Some("B".to_string()),
        })
    );
    assert!(target.selection_error.is_none());
    assert_eq!(preview.target_fingerprint.len(), 64);
    assert_eq!(
        preview.target_fingerprint,
        preview.target_fingerprint.to_lowercase()
    );
}

#[tokio::test]
async fn issue_1937_selection_api_scopes_enumerate_canonical_matrices_only() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-2-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    // Homonymous Matrix in another configured project: same directory name,
    // different canonical origin Matrix, so Kind must exclude it.
    let other_project = fixture._temp.path().join("other-project");
    let other_fixture = SelectionApiFixture {
        _temp: tempfile::tempdir().expect("second tempdir"),
        project: other_project,
        ac_root: PathBuf::new(),
    };
    let other_ac = other_fixture.project.join(".ac");
    selection_api_replica_at(
        &other_fixture,
        &other_ac.join("_agent_dev-rust"),
        &other_ac.join("room-1-team").join("__agent_dev-rust"),
        "dev-rust",
        locked_tooling("C", "agent-0"),
    );
    let mut settings = selection_api_settings(&fixture);
    settings
        .project_paths
        .push(other_fixture.project.to_string_lossy().to_string());
    let settings = state_for(settings);

    let room = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    assert!(room.counts_complete);
    assert_eq!(room.candidate_count, 1, "one Workgroup means one room");

    let kind = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Kind,
        super::AssignmentMode::Ordinary,
    )
    .await;
    assert!(kind.counts_complete);
    assert_eq!(
        kind.candidate_count, 2,
        "the canonical Matrix, not the name"
    );
    assert_eq!(kind.protected_count, 1);
    assert_eq!(kind.invalid_count, 0);
    let paths: Vec<&str> = kind
        .targets
        .iter()
        .map(|target| target.replica_path.as_str())
        .collect();
    assert!(paths.iter().any(|path| path.contains("room-1-team")));
    assert!(paths.iter().any(|path| path.contains("room-2-team")));
    assert!(
        paths.iter().all(|path| path.contains("project")),
        "the homonymous other-project replica must stay excluded: {paths:?}"
    );
}

#[tokio::test]
async fn issue_1937_selection_api_preview_counts_offline_and_live_replicas() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let live = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let manager = SessionManager::new();
    let session = manager
        .create_session(
            "codex".to_string(),
            Vec::new(),
            live.to_string_lossy().to_string(),
            Some("agent-0".to_string()),
            Some("Codex".to_string()),
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create live session");
    let settings = state_for(selection_api_settings(&fixture));
    let preview = super::preview_coding_agent_profile_selection_inner(
        &Arc::new(RwLock::new(manager)),
        &settings,
        api_preview_request(
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        ),
    )
    .await
    .expect("preview");

    assert_eq!(preview.candidate_count, 2, "offline replicas stay listed");
    assert_eq!(preview.live_session_count, 1);
    let live_target = preview
        .targets
        .iter()
        .find(|target| target.replica_path.contains("b-rust"))
        .expect("live target");
    assert_eq!(live_target.live_session_ids, vec![session.id.to_string()]);
    let offline_target = preview
        .targets
        .iter()
        .find(|target| target.replica_path.contains("a-rust"))
        .expect("offline target");
    assert!(offline_target.live_session_ids.is_empty());
}

#[tokio::test]
async fn issue_1937_selection_api_all_protected_bulk_offers_both_decisions() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("B", "agent-0"),
    );
    // Equal pair as well: a valid lock is still a conflict.
    selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        locked_tooling("B", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "c-rust",
        locked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
    )
    .await;

    assert_eq!(preview.candidate_count, 3);
    assert_eq!(preview.protected_count, 3);
    assert_eq!(preview.conflict_count, 3, "valid protected candidates only");
    assert_eq!(preview.target_count, 0, "top-level uses unlockedOnly");
    let decisions = preview.decisions.as_ref().expect("both decisions");
    assert_eq!(decisions.unlocked_only.eligible_count, 0);
    assert_eq!(decisions.unlocked_only.skipped_locked_count, 3);
    assert_eq!(decisions.force_reviewed.eligible_count, 3);
    assert_eq!(decisions.force_reviewed.skipped_locked_count, 0);
    assert_ne!(
        decisions.unlocked_only.fingerprint,
        decisions.force_reviewed.fingerprint
    );
    assert_eq!(
        preview.target_fingerprint,
        decisions.unlocked_only.fingerprint
    );
}

#[tokio::test]
async fn issue_1937_selection_api_absent_assignment_mode_is_ordinary_legacy() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let request: super::PreviewCodingAgentProfileSelectionRequest = serde_json::from_value(json!({
        "targetReplicaPath": replica.to_string_lossy(),
        "codingAgentId": "agent-0",
        "profile": "B",
        "scope": "replica",
        "restartSessions": false,
    }))
    .expect("deserialize legacy preview request");
    assert_eq!(request.assignment_mode, super::AssignmentMode::Ordinary);

    let settings = state_for(selection_api_settings(&fixture));
    let preview = super::preview_coding_agent_profile_selection_inner(
        &empty_session_manager(),
        &settings,
        request,
    )
    .await
    .expect("legacy preview");
    assert!(!preview.requires_explicit_confirmation);
    let result = api_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary,
        None,
        None,
    )
    .await
    .expect("legacy replica apply without fingerprint");
    assert_eq!(result.updated_count, 1);
    assert_eq!(result.newly_protected_paths.len(), 0);
}

#[tokio::test]
async fn issue_1937_selection_api_apply_ordinary_bulk_skips_locked() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    let unlocked = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let locked_bytes = config_bytes(&anchor);
    let settings = state_for(selection_api_settings(&fixture));
    let preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    assert_eq!(
        preview.target_count, 1,
        "actionable count skips the locked one"
    );
    assert!(preview.decisions.is_none());

    let result = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect("ordinary bulk apply");

    assert_eq!(result.updated_count, 1);
    assert_eq!(result.newly_protected_paths.len(), 0);
    assert_eq!(result.skipped_locked_paths.len(), 1);
    assert!(result.skipped_locked_paths[0].contains("a-rust"));
    assert!(result.errors.is_empty());
    assert_eq!(result.locked_after_apply_count, Some(1));
    assert!(!result.force_applied);
    assert_eq!(
        config_bytes(&anchor),
        locked_bytes,
        "locked bytes untouched"
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&config_bytes(&unlocked)).expect("parse unlocked config");
    assert_eq!(saved["tooling"]["profile"], json!("B"));
    assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
    assert!(saved["tooling"].get("selectionLocked").is_none());
}

#[tokio::test]
async fn issue_1937_selection_api_replica_assign_and_lock_is_atomic() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview = api_preview(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
    )
    .await;
    assert!(preview.requires_explicit_confirmation);
    assert!(
        preview.decisions.is_none(),
        "replica needs no conflict dialog"
    );

    let result = api_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect("replica assign and lock");

    assert_eq!(result.updated_count, 1);
    assert_eq!(
        result.newly_protected_paths,
        vec![result.updated_replica_paths[0].clone()]
    );
    assert!(!result.force_applied);
    let saved: serde_json::Value =
        serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
    assert_eq!(saved["tooling"]["selectionLocked"], json!(true));
    assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
    assert_eq!(saved["tooling"]["profile"], json!("B"));
}

#[tokio::test]
async fn issue_2051_selection_api_replica_lock_changed_target_is_stale_without_writes() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));

    // 1. approve the replica + lock pair on this state.
    let preview = api_preview(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
    )
    .await;

    // 2. the stored pair changes after the preview.
    let mut value: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
    value["tooling"]["profile"] = json!("C");
    std::fs::write(
        replica.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite config");
    let changed_bytes = config_bytes(&replica);

    // 3. the stale approval is rejected before any write.
    let error = api_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect_err("a changed target must stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&replica), changed_bytes, "no write on stale");
    assert!(!sidecar_path(&replica).exists(), "no sidecar on stale");
    let saved: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
    assert_ne!(
        saved["tooling"]["selectionLocked"],
        json!(true),
        "no lock on stale"
    );

    // 4. control: a fresh preview over the changed target still applies, so
    // step 3 rejected because of the change, not because lock is broken.
    let fresh = api_preview(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
    )
    .await;
    let result = api_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
        None,
        Some(&fresh.target_fingerprint),
    )
    .await
    .expect("a fresh preview applies");
    assert_eq!(result.updated_count, 1);
    assert_eq!(
        result.newly_protected_paths,
        vec![result.updated_replica_paths[0].clone()]
    );
}

#[tokio::test]
async fn issue_2051_selection_api_replica_lock_missing_fingerprint_is_not_stale() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let before = config_bytes(&replica);

    let error = api_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
        None,
        None,
    )
    .await
    .expect_err("a lock apply without a fingerprint must be refused");

    // #2051 - nothing was sent is a client contract failure, never a
    // target change: the message must not blame a stale preview.
    assert!(error.contains("confirmationMissing"), "{error}");
    assert!(!error.contains("Target selection changed"), "{error}");
    assert_eq!(
        config_bytes(&replica),
        before,
        "no write on a missing fingerprint"
    );
    assert!(!sidecar_path(&replica).exists(), "no sidecar on rejection");
    let saved: serde_json::Value =
        serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
    assert_ne!(
        saved["tooling"]["selectionLocked"],
        json!(true),
        "no lock written"
    );
}

#[tokio::test]
async fn issue_2010_selection_api_replica_locked_same_pair_keeps_lock() {
    for mode in [
        super::AssignmentMode::Ordinary,
        super::AssignmentMode::AssignAndLock,
    ] {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            locked_tooling("B", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let before = config_bytes(&replica);
        let preview = api_preview(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            mode,
        )
        .await;

        let result = api_apply(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            mode,
            None,
            Some(&preview.target_fingerprint),
        )
        .await
        .expect("locked same-pair replica apply");

        assert!(result.errors.is_empty(), "{mode:?}: {:?}", result.errors);
        assert_eq!(result.updated_count, 0, "{mode:?}");
        assert!(result.updated_replica_paths.is_empty(), "{mode:?}");
        assert!(result.newly_protected_paths.is_empty(), "{mode:?}");
        assert!(result.skipped_locked_paths.is_empty(), "{mode:?}");
        assert_eq!(result.restarted_count, 0, "{mode:?}");
        assert!(!result.force_applied, "{mode:?}");
        assert_eq!(config_bytes(&replica), before, "{mode:?}");
        let saved: serde_json::Value =
            serde_json::from_slice(&config_bytes(&replica)).expect("parse config");
        assert_eq!(saved["tooling"]["selectionLocked"], json!(true), "{mode:?}");
    }
}

#[tokio::test]
async fn issue_2010_selection_api_replica_locked_different_pair_still_errors() {
    for mode in [
        super::AssignmentMode::Ordinary,
        super::AssignmentMode::AssignAndLock,
    ] {
        let fixture = selection_api_fixture();
        let replica = selection_api_replica(
            &fixture,
            "room-1-team",
            "dev-rust",
            locked_tooling("A", "agent-0"),
        );
        let settings = state_for(selection_api_settings(&fixture));
        let before = config_bytes(&replica);
        let preview = api_preview(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            mode,
        )
        .await;

        let result = api_apply(
            &settings,
            &replica,
            super::ProfileAssignmentScope::Replica,
            mode,
            None,
            Some(&preview.target_fingerprint),
        )
        .await
        .expect("apply reaches the writer");

        assert_eq!(result.errors.len(), 1, "{mode:?}: {:?}", result.errors);
        assert!(
            result.errors[0].message.contains("locked"),
            "{mode:?}: {:?}",
            result.errors
        );
        assert_eq!(result.updated_count, 0, "{mode:?}");
        assert_eq!(config_bytes(&replica), before, "{mode:?}");
    }
}

#[tokio::test]
async fn issue_1937_selection_api_rejects_decision_outside_bulk_assign_and_lock() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        locked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let bytes_before = config_bytes(&anchor);

    let ordinary = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        Some(super::ConflictDecision::ForceReviewed),
        Some("whatever"),
    )
    .await
    .expect_err("ordinary plus decision must be rejected");
    assert!(
        ordinary.contains("only valid with assignmentMode assignAndLock"),
        "{ordinary}"
    );

    let single = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::AssignAndLock,
        Some(super::ConflictDecision::ForceReviewed),
        Some("whatever"),
    )
    .await
    .expect_err("replica plus decision must be rejected");
    assert!(single.contains("not valid for replica scope"), "{single}");

    assert_eq!(
        config_bytes(&anchor),
        bytes_before,
        "no writes on rejection"
    );
    assert!(!sidecar_path(&anchor).exists(), "no sidecar on rejection");
}

#[tokio::test]
async fn issue_1937_selection_api_bulk_assign_and_lock_requires_decision() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    let second = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        locked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let bytes_before = config_bytes(&anchor);
    let second_bytes = config_bytes(&second);
    let preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
    )
    .await;

    let error = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect_err("conflicts need a decision");
    assert!(error.contains("conflictDecisionRequired"), "{error}");
    // A force decision without the current preview fingerprint is rejected.
    let error = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
        Some(super::ConflictDecision::ForceReviewed),
        None,
    )
    .await
    .expect_err("a force decision still needs the reviewed fingerprint");
    // #2051 - a missing fingerprint is reported as missing, never stale.
    assert!(error.contains("confirmationMissing"), "{error}");
    assert!(!error.contains("Target selection changed"), "{error}");
    assert_eq!(config_bytes(&anchor), bytes_before);
    assert_eq!(config_bytes(&second), second_bytes);
    assert!(!sidecar_path(&anchor).exists());
    assert!(!sidecar_path(&second).exists());
}

#[tokio::test]
async fn issue_1937_selection_api_decision_snapshots_are_per_decision() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        locked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
    )
    .await;
    let decisions = preview.decisions.as_ref().expect("decisions");

    // Force with the unlockedOnly hash is a stale approval: no force applied.
    let stale = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
        Some(super::ConflictDecision::ForceReviewed),
        Some(&decisions.unlocked_only.fingerprint),
    )
    .await
    .expect_err("force with the wrong decision hash");
    assert!(stale.contains("stalePreview"), "{stale}");

    // UnlockedOnly with its own hash skips every protected replica.
    let unlocked_only = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
        Some(super::ConflictDecision::UnlockedOnly),
        Some(&decisions.unlocked_only.fingerprint),
    )
    .await
    .expect("unlockedOnly apply");
    assert_eq!(unlocked_only.updated_count, 0);
    assert_eq!(unlocked_only.skipped_locked_paths.len(), 2);
    assert!(!unlocked_only.force_applied);
    assert_eq!(unlocked_only.locked_after_apply_count, Some(2));

    // ForceReviewed with its own hash overwrites the protected targets.
    let forced = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::AssignAndLock,
        Some(super::ConflictDecision::ForceReviewed),
        Some(&decisions.force_reviewed.fingerprint),
    )
    .await
    .expect("forceReviewed apply");
    assert_eq!(forced.updated_count, 2);
    assert!(forced.skipped_locked_paths.is_empty());
    assert!(
        forced.force_applied,
        "a protected target was actually overwritten"
    );
    assert_eq!(forced.locked_after_apply_count, Some(2));
    assert!(
        forced.newly_protected_paths.is_empty(),
        "already locked stays locked"
    );
}

#[tokio::test]
async fn issue_1937_selection_api_stale_approvals_are_rejected_without_writes() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));

    // (a) the stored pair changes after the preview.
    let pair_preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    let mut value: Value = serde_json::from_slice(&config_bytes(&anchor)).expect("parse config");
    value["tooling"]["profile"] = json!("C");
    std::fs::write(
        anchor.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite config");
    let changed_bytes = config_bytes(&anchor);
    let error = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&pair_preview.target_fingerprint),
    )
    .await
    .expect_err("pair change must stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&anchor), changed_bytes);

    // (b) the flag changes after the preview.
    let flag_preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    let mut value: Value = serde_json::from_slice(&config_bytes(&anchor)).expect("parse config");
    value["tooling"]["selectionLocked"] = json!(true);
    value["tooling"]["currentCodingAgent"] = json!("agent-0");
    std::fs::write(
        anchor.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite config");
    let changed_bytes = config_bytes(&anchor);
    let error = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&flag_preview.target_fingerprint),
    )
    .await
    .expect_err("flag change must stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&anchor), changed_bytes);

    // (c) membership grows after the preview.
    let membership_preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    let added = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let added_bytes = config_bytes(&added);
    let error = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&membership_preview.target_fingerprint),
    )
    .await
    .expect_err("membership change must stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&added), added_bytes);

    // (d) a relevant live session appears after the preview.
    let manager = SessionManager::new();
    let live = selection_api_replica(
        &fixture,
        "room-1-team",
        "c-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let session_mgr = Arc::new(RwLock::new(manager));
    let session_preview = super::preview_coding_agent_profile_selection_inner(
        &session_mgr,
        &settings,
        api_preview_request(
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
        ),
    )
    .await
    .expect("preview with sessions");
    {
        let manager = session_mgr.write().await;
        manager
            .create_session(
                "codex".to_string(),
                Vec::new(),
                live.to_string_lossy().to_string(),
                Some("agent-0".to_string()),
                Some("Codex".to_string()),
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create session");
    }
    let live_bytes = config_bytes(&live);
    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        Arc::new(SelectionApiPtyBackend),
    )));
    let error = super::apply_coding_agent_profile_selection_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        &settings,
        api_apply_request(
            &anchor,
            super::ProfileAssignmentScope::Workgroup,
            super::AssignmentMode::Ordinary,
            None,
            Some(&session_preview.target_fingerprint),
        ),
    )
    .await
    .expect_err("session change must stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&live), live_bytes);
}

#[tokio::test]
async fn issue_1937_selection_api_skipped_locked_never_restarts_live_session() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    let manager = SessionManager::new();
    let session = manager
        .create_session(
            "codex".to_string(),
            Vec::new(),
            anchor.to_string_lossy().to_string(),
            Some("agent-0".to_string()),
            Some("Codex".to_string()),
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create session");
    let session_mgr = Arc::new(RwLock::new(manager));
    let settings = state_for(selection_api_settings(&fixture));
    let bytes_before = config_bytes(&anchor);
    let mut preview_request = api_preview_request(
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    );
    preview_request.restart_sessions = true;
    let preview = super::preview_coding_agent_profile_selection_inner(
        &session_mgr,
        &settings,
        preview_request,
    )
    .await
    .expect("preview");
    assert_eq!(
        preview.live_session_count, 0,
        "the actionable count skips the protected replica's session"
    );
    assert_eq!(preview.target_count, 0, "the only replica is protected");
    assert_eq!(preview.targets.len(), 1);
    assert_eq!(
        preview.targets[0].live_session_ids,
        vec![session.id.to_string()],
        "the protected target still reports its live session"
    );

    let app = crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        Arc::new(SelectionApiPtyBackend),
    )));
    let mut request = api_apply_request(
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&preview.target_fingerprint),
    );
    request.restart_sessions = true;
    let result = super::apply_coding_agent_profile_selection_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        &settings,
        request,
    )
    .await
    .expect("skipping a locked replica is not an error")
    .0;

    assert_eq!(result.updated_count, 0);
    assert_eq!(
        result.restarted_count, 0,
        "no lifecycle invocation for a skip"
    );
    assert_eq!(result.skipped_locked_paths.len(), 1);
    assert!(result.errors.is_empty());
    assert_eq!(config_bytes(&anchor), bytes_before);
    assert!(
        session_mgr
            .read()
            .await
            .get_session(session.id)
            .await
            .is_some(),
        "the skipped replica's session must survive untouched"
    );
}

#[tokio::test]
async fn issue_1937_selection_api_later_target_race_yields_partial_results() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let later = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    let later_bytes = config_bytes(&later);

    // Hold the later target's sidecar lock to force a real per-file write
    // failure after the first target has already committed.
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(sidecar_path(&later))
        .expect("open sidecar");
    lock_file.lock().expect("hold later target lock");
    let result = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect("earlier writes stay committed");
    lock_file.unlock().expect("release sidecar");
    drop(lock_file);

    assert_eq!(result.updated_count, 1, "the first target still committed");
    assert!(result.updated_replica_paths[0].contains("a-rust"));
    assert_eq!(
        config_bytes(&later),
        later_bytes,
        "the failed target is untouched"
    );
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].code, "configLockTimeout");
    assert!(result.errors[0].replica_paths[0].contains("b-rust"));
    assert_eq!(result.locked_after_apply_count, Some(0));
}

#[tokio::test]
async fn issue_1937_selection_api_invalid_state_is_visible_and_never_written() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        json!({"selectionLocked": true}),
    );
    let unlocked = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let invalid_bytes = config_bytes(&anchor);
    let preview = api_preview(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
    )
    .await;
    assert_eq!(preview.invalid_count, 1);
    assert_eq!(preview.protected_count, 0);
    assert_eq!(
        preview.target_count, 1,
        "only the valid replica is actionable"
    );
    let invalid = preview
        .targets
        .iter()
        .find(|target| target.replica_path.contains("a-rust"))
        .expect("invalid target listed");
    assert_eq!(invalid.selection_state, super::SelectionState::Invalid);
    assert!(invalid.saved_pair.is_none());
    assert!(invalid.selection_error.is_some());

    let result = api_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        super::AssignmentMode::Ordinary,
        None,
        Some(&preview.target_fingerprint),
    )
    .await
    .expect("valid targets still apply");
    assert_eq!(result.updated_count, 1);
    assert_eq!(result.invalid_paths.len(), 1);
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].code, "invalidSelectionState");
    assert_eq!(config_bytes(&anchor), invalid_bytes);
    assert_eq!(
        result.locked_after_apply_count, None,
        "invalid state is unknowable"
    );
    let saved: Value = serde_json::from_slice(&config_bytes(&unlocked)).expect("parse");
    assert_eq!(saved["tooling"]["profile"], json!("B"));
}

#[tokio::test]
async fn issue_1937_selection_api_anchor_must_be_a_configured_replica() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let matrix = fixture.ac_root.join("_agent_dev-rust");
    // Origin Matrix is not a lockable replica target.
    let mut unconfigured = settings_with_single_agent();
    unconfigured.project_paths = Vec::new();
    let unconfigured = state_for(unconfigured);
    let error = api_preview_result(
        &unconfigured,
        &replica,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary,
    )
    .await
    .expect_err("an unconfigured project root must be rejected");
    assert!(
        error.contains("outside configured AC project roots"),
        "{error}"
    );

    let settings = state_for(selection_api_settings(&fixture));
    let error = api_preview_result(
        &settings,
        &matrix,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary,
    )
    .await
    .expect_err("the origin Matrix must be rejected");
    assert!(error.contains("not a Room replica"), "{error}");

    // A `__agent_*` directly under `.ac` has no room parent.
    let roomless = fixture.ac_root.join("__agent_dev-rust");
    std::fs::create_dir_all(&roomless).expect("create roomless replica dir");
    let error = api_preview_result(
        &settings,
        &roomless,
        super::ProfileAssignmentScope::Replica,
        super::AssignmentMode::Ordinary,
    )
    .await
    .expect_err("a roomless replica must be rejected");
    assert!(
        error.contains("room-*") || error.contains("Room"),
        "{error}"
    );
}

#[tokio::test]
async fn issue_1937_selection_api_removal_zero_protected_is_a_pure_no_op() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let second = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        unlocked_tooling("C", "agent-0"),
    );
    let matrix = fixture.ac_root.join("_agent_a-rust");
    let settings = state_for(selection_api_settings(&fixture));
    let anchor_bytes = config_bytes(&anchor);
    let second_bytes = config_bytes(&second);
    let mut dir_before: Vec<std::ffi::OsString> = std::fs::read_dir(&anchor)
        .expect("read anchor dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    dir_before.sort();

    let preview =
        api_removal_preview(&settings, &anchor, super::ProfileAssignmentScope::Workgroup).await;
    assert!(preview.counts_complete);
    assert_eq!(preview.candidate_count, 2);
    assert_eq!(preview.protected_count, 0);
    assert_eq!(preview.already_unlocked_count, 2);
    assert_eq!(preview.invalid_count, 0);
    assert_eq!(preview.target_fingerprint.len(), 64);

    let result = api_removal_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Workgroup,
        &preview.target_fingerprint,
    )
    .await
    .expect("zero-protected removal");
    assert_eq!(result.removed_count, 0);
    assert_eq!(result.already_unlocked_paths.len(), 2);
    assert!(result.failed_replica_paths.is_empty());
    assert_eq!(result.remaining_protected_count, Some(0));
    assert!(result.errors.is_empty());

    assert_eq!(config_bytes(&anchor), anchor_bytes);
    assert_eq!(config_bytes(&second), second_bytes);
    let mut dir_after: Vec<std::ffi::OsString> = std::fs::read_dir(&anchor)
        .expect("read anchor dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    dir_after.sort();
    assert_eq!(
        dir_after, dir_before,
        "zero-protected removal creates no sidecar or temp file"
    );
    assert!(!sidecar_path(&anchor).exists(), "no sidecar for a no-op");
    assert!(!sidecar_path(&second).exists(), "no sidecar for a no-op");
    assert!(
        !matrix.join("config.json").exists(),
        "removal never writes a Matrix default"
    );

    // The single-replica scope has the same zero-protected shape.
    let single =
        api_removal_preview(&settings, &anchor, super::ProfileAssignmentScope::Replica).await;
    assert_eq!(single.candidate_count, 1);
    assert_eq!(single.protected_count, 0);
    let single_result = api_removal_apply(
        &settings,
        &anchor,
        super::ProfileAssignmentScope::Replica,
        &single.target_fingerprint,
    )
    .await
    .expect("single zero-protected removal");
    assert_eq!(single_result.removed_count, 0);
    assert_eq!(config_bytes(&anchor), anchor_bytes);
}

#[tokio::test]
async fn issue_1937_selection_api_removal_preserves_pair_and_unknown_tooling() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        json!({
            "profile": "A",
            "currentCodingAgent": "agent-0",
            "selectionLocked": true,
            "lastCodingAgent": "legacy-agent",
            "profileContentHash": "deadbeef",
        }),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview =
        api_removal_preview(&settings, &replica, super::ProfileAssignmentScope::Replica).await;
    assert_eq!(preview.protected_count, 1);

    let result = api_removal_apply(
        &settings,
        &replica,
        super::ProfileAssignmentScope::Replica,
        &preview.target_fingerprint,
    )
    .await
    .expect("removal");
    assert_eq!(result.removed_count, 1);
    assert_eq!(result.remaining_protected_count, Some(0));
    assert!(result.errors.is_empty());
    let saved: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse");
    assert_eq!(saved["tooling"]["selectionLocked"], json!(false));
    assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
    assert_eq!(saved["tooling"]["profile"], json!("A"));
    let state: Value = serde_json::from_slice(
        &std::fs::read(replica.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME))
            .expect("read state file"),
    )
    .expect("parse state");
    assert_eq!(state["tooling"]["lastCodingAgent"], json!("legacy-agent"));
    assert_eq!(state["tooling"]["profileContentHash"], json!("deadbeef"));
    assert!(saved["tooling"].get("lastCodingAgent").is_none());
    assert!(saved["tooling"].get("profileContentHash").is_none());
}

#[tokio::test]
async fn issue_1937_selection_api_removal_mixed_skips_the_unlocked_target() {
    let fixture = selection_api_fixture();
    let first = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        locked_tooling("B", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "c-rust",
        locked_tooling("C", "agent-0"),
    );
    let unlocked_fourth = selection_api_replica(
        &fixture,
        "room-1-team",
        "d-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let fourth_bytes = config_bytes(&unlocked_fourth);
    let preview =
        api_removal_preview(&settings, &first, super::ProfileAssignmentScope::Workgroup).await;
    assert_eq!(preview.candidate_count, 4);
    assert_eq!(preview.protected_count, 3);
    assert_eq!(preview.already_unlocked_count, 1);

    let result = api_removal_apply(
        &settings,
        &first,
        super::ProfileAssignmentScope::Workgroup,
        &preview.target_fingerprint,
    )
    .await
    .expect("mixed removal");
    assert_eq!(result.removed_count, 3);
    assert_eq!(result.already_unlocked_paths.len(), 1);
    assert!(result.already_unlocked_paths[0].contains("d-rust"));
    assert_eq!(
        config_bytes(&unlocked_fourth),
        fourth_bytes,
        "no writer for the fourth"
    );
    assert!(
        !sidecar_path(&unlocked_fourth).exists(),
        "the unlocked fourth target must not create a write sidecar"
    );
    assert!(
        sidecar_path(&first).exists(),
        "the protected targets did write"
    );
    assert_eq!(result.remaining_protected_count, Some(0));
    assert!(result.errors.is_empty());
}

#[tokio::test]
async fn issue_1937_selection_api_removal_stale_scope_rejects_without_effects() {
    let fixture = selection_api_fixture();
    let first = selection_api_replica(
        &fixture,
        "room-1-team",
        "a-rust",
        locked_tooling("A", "agent-0"),
    );
    let second = selection_api_replica(
        &fixture,
        "room-1-team",
        "b-rust",
        locked_tooling("B", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let preview =
        api_removal_preview(&settings, &first, super::ProfileAssignmentScope::Workgroup).await;
    // Another window unlocks the first replica after the preview.
    let mut value: Value = serde_json::from_slice(&config_bytes(&first)).expect("parse");
    value["tooling"]["selectionLocked"] = json!(false);
    std::fs::write(
        first.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite");
    let first_bytes = config_bytes(&first);
    let second_bytes = config_bytes(&second);

    let error = api_removal_apply(
        &settings,
        &first,
        super::ProfileAssignmentScope::Workgroup,
        &preview.target_fingerprint,
    )
    .await
    .expect_err("scope-level stale");
    assert!(error.contains("stalePreview"), "{error}");
    assert_eq!(config_bytes(&first), first_bytes);
    assert_eq!(
        config_bytes(&second),
        second_bytes,
        "the second stays locked"
    );
}

const C3_CLEANUP_PREFIX: &str =
    "Config pair r/config.json and r/config.state.no-git.json cleanup failed: ";

/// #2816 (C3) E10 - the cleanup-wrapped TRACKED text keeps its wire code.
#[test]
fn issue_2816_selection_error_code_keeps_the_tracked_tooling_code() {
    assert_eq!(
        super::selection_error_code(&format!(
            "{C3_CLEANUP_PREFIX}'tooling' must be a JSON object in the tracked config file"
        )),
        "invalidSelectionState"
    );
}

/// #2816 (C3) E10 - the arm is not widened: the state-side text and an
/// unrelated write failure stay `configWriteFailed`.
#[test]
fn issue_2816_selection_error_code_does_not_reclassify_the_state_side() {
    assert_eq!(
        super::selection_error_code(&format!(
            "{C3_CLEANUP_PREFIX}'tooling' must be a JSON object in the state file"
        )),
        "configWriteFailed"
    );
    assert_eq!(
        super::selection_error_code("Failed to write temp config r/.config.json.1.tmp: denied"),
        "configWriteFailed"
    );
}

/// #2816 (C3) E10 - the write path: a tracked `tooling` that turns
/// malformed after the enumeration still classifies as before.
#[tokio::test]
async fn issue_2816_write_path_malformed_tracked_tooling_keeps_its_code() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let settings_snapshot = settings.read().await.clone();
    let enumeration = super::enumerate_profile_assignment_targets(
        &settings_snapshot,
        &replica,
        &super::ProfileAssignmentScope::Replica,
        &[],
    )
    .expect("enumerate");
    let target = &enumeration.candidates[0].target;
    let expected = crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
        configuration_ref: None,
        profile_override_source: None,
        identity: target.identity_path.clone(),
        pair: super::target_pair(target),
        locked: false,
    };
    // The tracked `tooling` turns malformed after the enumeration. No state
    // file exists, so the cleanup's tracked check is the one measured.
    let mut value: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse");
    value["tooling"] = json!(5);
    std::fs::write(
        replica.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite");
    let changed_bytes = config_bytes(&replica);
    assert!(!replica
        .join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
        .exists());

    let error = crate::config::coding_agent_profiles::write_replica_selection(
        &settings_snapshot,
        &replica,
        &crate::config::coding_agent_profiles::ReplicaSelectionPair {
            coding_agent_id: "agent-0".to_string(),
            requested_profile: "B".to_string(),
        },
        crate::config::coding_agent_profiles::SelectionWriteIntent::BulkOrdinary,
        &expected,
    )
    .expect_err("a malformed tracked tooling must fail");
    assert_eq!(
        super::selection_error_code(&error),
        "invalidSelectionState",
        "{error}"
    );
    assert_eq!(config_bytes(&replica), changed_bytes);
}

#[tokio::test]
async fn issue_1937_selection_api_per_file_cas_rejects_changed_target() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let settings_snapshot = settings.read().await.clone();
    let enumeration = super::enumerate_profile_assignment_targets(
        &settings_snapshot,
        &replica,
        &super::ProfileAssignmentScope::Replica,
        &[],
    )
    .expect("enumerate");
    let target = &enumeration.candidates[0].target;
    let expected = crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
        configuration_ref: None,
        profile_override_source: None,
        identity: target.identity_path.clone(),
        pair: super::target_pair(target),
        locked: false,
    };
    // The stored pair changes after the enumeration but before the write.
    let mut value: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse");
    value["tooling"]["profile"] = json!("C");
    std::fs::write(
        replica.join("config.json"),
        serde_json::to_vec(&value).expect("serialize"),
    )
    .expect("rewrite");
    let changed_bytes = config_bytes(&replica);

    let error = crate::config::coding_agent_profiles::write_replica_selection(
        &settings_snapshot,
        &replica,
        &crate::config::coding_agent_profiles::ReplicaSelectionPair {
            coding_agent_id: "agent-0".to_string(),
            requested_profile: "B".to_string(),
        },
        crate::config::coding_agent_profiles::SelectionWriteIntent::BulkOrdinary,
        &expected,
    )
    .expect_err("stale expectation must fail its CAS");
    assert!(error.starts_with("stale replica selection"), "{error}");
    assert_eq!(
        config_bytes(&replica),
        changed_bytes,
        "failed CAS writes nothing"
    );
}

#[tokio::test]
async fn issue_1937_selection_api_default_round_trip_and_atomic_publication() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let matrix = fixture.ac_root.join("_agent_dev-rust");
    std::fs::write(
        matrix.join("config.json"),
        serde_json::to_vec(&json!({
            "tooling": { "defaultProfile": "A", "unknownKey": 123 },
        }))
        .expect("serialize matrix config"),
    )
    .expect("seed matrix config");
    let settings = state_for(selection_api_settings(&fixture));
    let replica_bytes = config_bytes(&replica);

    let initial = api_default(&settings, &replica).await;
    assert!(initial.default.is_none());
    assert_eq!(initial.default_fingerprint.len(), 64);
    // The returned replica path is the canonical identity, not the raw
    // spelling: on Windows the temp path may carry an 8.3 short component
    // (`RUNNER~1`) while the validated path is the long canonical form.
    assert_eq!(initial.target_replica_path, canonical_display(&replica));

    let error = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(&replica, "agent-0", "B", true, "wrong"),
    )
    .await
    .expect_err("stale default fingerprint");
    assert!(error.contains("stalePreview"), "{error}");

    let (set, payload) = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(&replica, "agent-0", "B", true, &initial.default_fingerprint),
    )
    .await
    .expect("set default");
    assert_eq!(
        set.default,
        Some(super::SelectionDefault {
            coding_agent_id: "agent-0".to_string(),
            requested_profile: "B".to_string(),
            selection_locked: true,
        })
    );
    assert_eq!(payload["scope"], json!("default"));
    assert_eq!(payload["operation"], json!("default"));
    assert_eq!(payload["agentPath"], json!(set.matrix_path));
    assert_eq!(payload["affectedPaths"], json!([]));
    assert_eq!(payload["default"]["selectionLocked"], json!(true));

    let disk: Value =
        serde_json::from_slice(&std::fs::read(matrix.join("config.json")).expect("read"))
            .expect("parse matrix config");
    assert_eq!(
        disk["tooling"]["replicaSelectionDefault"],
        json!({
            "codingAgentId": "agent-0",
            "requestedProfile": "B",
            "selectionLocked": true,
        })
    );
    assert_eq!(disk["tooling"]["defaultProfile"], json!("A"));
    assert_eq!(disk["tooling"]["unknownKey"], json!(123));
    assert_eq!(
        config_bytes(&replica),
        replica_bytes,
        "replicas are untouched"
    );

    let reread = api_default(&settings, &replica).await;
    assert_eq!(reread.default, set.default);
    assert_eq!(reread.default_fingerprint, set.default_fingerprint);

    // A second window that still holds the pre-set fingerprint is stale.
    let error = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(
            &replica,
            "agent-0",
            "C",
            false,
            &initial.default_fingerprint,
        ),
    )
    .await
    .expect_err("the old window must not overwrite the new default");
    assert!(error.contains("stalePreview"), "{error}");
    let unchanged: Value =
        serde_json::from_slice(&std::fs::read(matrix.join("config.json")).expect("read"))
            .expect("parse");
    assert_eq!(
        unchanged["tooling"]["replicaSelectionDefault"]["requestedProfile"],
        json!("B")
    );
}

#[tokio::test]
async fn issue_1937_selection_api_default_rejects_malformed_and_invalid_input() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let matrix = fixture.ac_root.join("_agent_dev-rust");
    let settings = state_for(selection_api_settings(&fixture));

    // A malformed stored default is an error, never a silent absence.
    std::fs::write(
        matrix.join("config.json"),
        serde_json::to_vec(&json!({
            "tooling": { "replicaSelectionDefault": { "codingAgentId": "agent-0" } },
        }))
        .expect("serialize malformed config"),
    )
    .expect("write malformed config");
    let error =
        super::get_replica_selection_default_inner(&settings, api_default_request(&replica))
            .await
            .expect_err("malformed default must fail the read");
    let error = error.to_string();
    assert!(error.contains("replicaSelectionDefault"), "{error}");

    let error = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(&replica, "agent-0", "B", true, "whatever"),
    )
    .await
    .expect_err("malformed prior default must fail the CAS read");
    assert!(error.contains("replicaSelectionDefault"), "{error}");

    std::fs::remove_file(matrix.join("config.json")).expect("remove malformed config");
    let error = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(&replica, "agent-0", "BB", true, "whatever"),
    )
    .await
    .expect_err("the profile must be a single letter");
    assert!(error.contains("single letter A through Z"), "{error}");

    let error = super::set_replica_selection_default_inner(
        &settings,
        api_set_default_request(&replica, "missing-agent", "B", true, "whatever"),
    )
    .await
    .expect_err("the coding agent must be configured");
    assert!(error.contains("not configured"), "{error}");
}

#[tokio::test]
async fn issue_1937_selection_api_barrier_unlock_waits_for_paused_apply_publication() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let settings_for_holder = settings.clone();
    let replica_for_holder = replica.clone();
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    let (release_tx, release_rx) = oneshot::channel::<()>();

    let holder = tokio::spawn(crate::session::selection::run_owned_selection_operation(
        move || {
            let settings = settings_for_holder;
            let replica = replica_for_holder;
            async move {
                // Publication phase of an apply, then the held restart span.
                let snapshot = settings.read().await.clone();
                let enumeration = super::enumerate_profile_assignment_targets(
                    &snapshot,
                    &replica,
                    &super::ProfileAssignmentScope::Replica,
                    &[],
                )
                .expect("enumerate");
                let target = &enumeration.candidates[0].target;
                let expected = crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
                    configuration_ref: None,
                    profile_override_source: None,
                    identity: target.identity_path.clone(),
                    pair: super::target_pair(target),
                    locked: false,
                };
                crate::config::coding_agent_profiles::write_replica_selection(
                    &snapshot,
                    &replica,
                    &crate::config::coding_agent_profiles::ReplicaSelectionPair {
                        coding_agent_id: "agent-0".to_string(),
                        requested_profile: "B".to_string(),
                    },
                    SelectionWriteIntent::IndividualAssignLock,
                    &expected,
                )
                .expect("publish the lock");
                entered_tx.send(()).expect("signal entered");
                release_rx.await.expect("release the publication span");
                Ok::<(), String>(())
            }
        },
    ));
    entered_rx.await.expect("holder entered");

    let preview =
        api_removal_preview(&settings, &replica, super::ProfileAssignmentScope::Replica).await;
    assert_eq!(preview.protected_count, 1);
    let fingerprint = preview.target_fingerprint.clone();
    let settings_for_unlock = settings.clone();
    let replica_for_unlock = replica.clone();
    let unlock = tokio::spawn(crate::session::selection::run_owned_selection_operation(
        move || {
            let settings = settings_for_unlock;
            let replica = replica_for_unlock;
            let fingerprint = fingerprint.clone();
            async move {
                super::apply_selection_lock_removal_inner(
                    &settings,
                    api_removal_apply_request(
                        &replica,
                        super::ProfileAssignmentScope::Replica,
                        &fingerprint,
                    ),
                )
                .await
                .map(|(result, _payload)| result)
            }
        },
    ));
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        !unlock.is_finished(),
        "the unlock must not acknowledge while the earlier apply still owns the turn"
    );

    release_tx.send(()).expect("release holder");
    holder
        .await
        .expect("holder join")
        .expect("holder operation");
    let removed = unlock
        .await
        .expect("unlock join")
        .expect("unlock operation");
    assert_eq!(removed.removed_count, 1);
    assert_eq!(removed.remaining_protected_count, Some(0));
    let saved: Value = serde_json::from_slice(&config_bytes(&replica)).expect("parse");
    assert_eq!(saved["tooling"]["selectionLocked"], json!(false));
    assert_eq!(saved["tooling"]["currentCodingAgent"], json!("agent-0"));
}

#[tokio::test]
async fn issue_1937_selection_api_barrier_read_cannot_observe_half_settled_apply() {
    let fixture = selection_api_fixture();
    let replica = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        unlocked_tooling("A", "agent-0"),
    );
    let settings = state_for(selection_api_settings(&fixture));
    let settings_for_holder = settings.clone();
    let replica_for_holder = replica.clone();
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    let (release_tx, release_rx) = oneshot::channel::<()>();

    let holder = tokio::spawn(crate::session::selection::run_owned_selection_operation(
        move || {
            let settings = settings_for_holder;
            let replica = replica_for_holder;
            async move {
                let snapshot = settings.read().await.clone();
                let enumeration = super::enumerate_profile_assignment_targets(
                    &snapshot,
                    &replica,
                    &super::ProfileAssignmentScope::Replica,
                    &[],
                )
                .expect("enumerate");
                let target = &enumeration.candidates[0].target;
                let expected = crate::config::coding_agent_profiles::ReplicaSelectionExpectation {
                    configuration_ref: None,
                    profile_override_source: None,
                    identity: target.identity_path.clone(),
                    pair: super::target_pair(target),
                    locked: false,
                };
                crate::config::coding_agent_profiles::write_replica_selection(
                    &snapshot,
                    &replica,
                    &crate::config::coding_agent_profiles::ReplicaSelectionPair {
                        coding_agent_id: "agent-0".to_string(),
                        requested_profile: "B".to_string(),
                    },
                    SelectionWriteIntent::IndividualAssignLock,
                    &expected,
                )
                .expect("publish the lock");
                entered_tx.send(()).expect("signal entered");
                release_rx.await.expect("release the publication span");
                Ok::<(), String>(())
            }
        },
    ));
    entered_rx.await.expect("holder entered");
    let published_bytes = config_bytes(&replica);
    let mut dir_before: Vec<std::ffi::OsString> = std::fs::read_dir(&replica)
        .expect("read replica dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    dir_before.sort();

    let settings_for_read = settings.clone();
    let replica_for_read = replica.clone();
    let read = tokio::spawn(crate::session::selection::run_owned_selection_operation(
        move || {
            let settings = settings_for_read;
            let replica = replica_for_read;
            async move {
                super::preview_selection_lock_removal_inner(
                    &empty_session_manager(),
                    &settings,
                    api_removal_preview_request(&replica, super::ProfileAssignmentScope::Replica),
                )
                .await
                .map(|preview| preview.protected_count)
            }
        },
    ));
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        !read.is_finished(),
        "the read must not observe the state while the mutation still owns the turn"
    );

    // Dropping the read waiter leaves the earlier mutation's ownership intact.
    read.abort();
    let _ = read.await;
    let probe = tokio::time::timeout(
        Duration::from_millis(150),
        crate::session::selection::acquire_selection_operation_turn(),
    )
    .await;
    assert!(
        probe.is_err(),
        "the earlier mutation must still own the turn after the read waiter is dropped"
    );

    release_tx.send(()).expect("release holder");
    holder
        .await
        .expect("holder join")
        .expect("holder operation");
    assert_eq!(
        config_bytes(&replica),
        published_bytes,
        "the read wrote nothing"
    );
    let mut dir_after: Vec<std::ffi::OsString> = std::fs::read_dir(&replica)
        .expect("read replica dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    dir_after.sort();
    assert_eq!(dir_after, dir_before, "the read created no files");

    // A fresh read after release observes the settled publication.
    let preview =
        api_removal_preview(&settings, &replica, super::ProfileAssignmentScope::Replica).await;
    assert_eq!(preview.protected_count, 1);
    assert_eq!(
        preview.targets[0].saved_pair,
        Some(super::SavedPair {
            coding_agent_id: Some("agent-0".to_string()),
            requested_profile: Some("B".to_string()),
        })
    );
}

// ── #2348 - whole-settings writers inherit central placement preservation ──

fn placement_geometry(x: f64, y: f64) -> WindowGeometry {
    WindowGeometry {
        x,
        y,
        width: 800.0,
        height: 600.0,
    }
}

/// Seed a `settings.json` with the placement pair present, absent, or both,
/// so a Preserve save has (or lacks) disk truth to copy.
fn write_placement_settings_file(
    dir: &Path,
    geometry: Option<(f64, f64)>,
    state: Option<MainWindowDisplayState>,
) {
    let mut value = serde_json::to_value(settings_with_single_agent()).unwrap();
    let object = value.as_object_mut().unwrap();
    match geometry {
        Some((x, y)) => {
            object.insert(
                "mainGeometry".to_string(),
                json!({ "x": x, "y": y, "width": 800.0, "height": 600.0 }),
            );
        }
        None => {
            object.remove("mainGeometry");
        }
    }
    match state {
        Some(state) => {
            object.insert(
                "mainWindowDisplayState".to_string(),
                serde_json::to_value(state).unwrap(),
            );
        }
        None => {
            object.remove("mainWindowDisplayState");
        }
    }
    std::fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
}

fn assert_disk_placement(dir: &Path, expected_state: MainWindowDisplayState, expected_x: f64) {
    let raw = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    let disk: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        disk["mainWindowDisplayState"],
        serde_json::to_value(expected_state).unwrap()
    );
    assert_eq!(disk["mainGeometry"]["x"], json!(expected_x));
}

#[tokio::test]
async fn issue_2348_settings_update_cannot_regress_the_disk_placement_pair() {
    for (label, disk_state, stale_state) in [
        (
            "maximized disk, normal stale",
            MainWindowDisplayState::Maximized,
            MainWindowDisplayState::Normal,
        ),
        (
            "normal disk, maximized stale",
            MainWindowDisplayState::Normal,
            MainWindowDisplayState::Maximized,
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");
        write_placement_settings_file(temp.path(), Some((1.0, 2.0)), Some(disk_state));

        let mut current = settings_with_single_agent();
        current.main_geometry = Some(placement_geometry(1.0, 2.0));
        current.main_window_display_state = disk_state;
        let state = state_for(current.clone());

        let mut stale = current.clone();
        stale.main_geometry = Some(placement_geometry(999.0, 999.0));
        stale.main_window_display_state = stale_state;

        let (saved, _) = persist_protected_settings_update_with_saver(&state, stale, |candidate| {
            crate::config::settings::save_settings_to_path_preserving_project_paths(
                candidate,
                &settings_path,
            )
        })
        .await
        .unwrap();

        assert_eq!(saved.main_window_display_state, disk_state, "{label}");
        assert_eq!(
            saved.main_geometry.as_ref().map(|g| (g.x, g.y)),
            Some((1.0, 2.0)),
            "{label}"
        );
        assert_disk_placement(temp.path(), disk_state, 1.0);
    }
}

#[tokio::test]
async fn issue_2348_settings_draft_cannot_regress_the_disk_placement_pair() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    write_placement_settings_file(
        temp.path(),
        Some((1.0, 2.0)),
        Some(MainWindowDisplayState::Maximized),
    );

    let mut current = settings_with_single_agent();
    current.main_geometry = Some(placement_geometry(1.0, 2.0));
    current.main_window_display_state = MainWindowDisplayState::Maximized;
    let state = state_for(current.clone());

    let mut draft = current.clone();
    draft.main_geometry = Some(placement_geometry(999.0, 999.0));
    draft.main_window_display_state = MainWindowDisplayState::Normal;

    let (saved, _events) = persist_settings_draft_update_with_saver(&state, draft, |candidate| {
        crate::config::settings::save_settings_to_path_preserving_project_paths(
            candidate,
            &settings_path,
        )
    })
    .await
    .unwrap();

    assert_eq!(
        saved.main_window_display_state,
        MainWindowDisplayState::Maximized
    );
    assert_eq!(saved.main_geometry.as_ref().map(|g| g.x), Some(1.0));
    assert_disk_placement(temp.path(), MainWindowDisplayState::Maximized, 1.0);
}

#[tokio::test]
async fn issue_2348_cli_style_whole_settings_payload_cannot_regress_the_disk_placement_pair() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    write_placement_settings_file(
        temp.path(),
        Some((1.0, 2.0)),
        Some(MainWindowDisplayState::Maximized),
    );

    let mut stale = settings_with_single_agent();
    stale.main_geometry = Some(placement_geometry(999.0, 999.0));
    stale.main_window_display_state = MainWindowDisplayState::Normal;

    crate::config::settings::save_settings_with_project_paths_to_path(&stale, &settings_path)
        .unwrap();

    assert_disk_placement(temp.path(), MainWindowDisplayState::Maximized, 1.0);
}

#[tokio::test]
async fn issue_2348_absent_disk_placement_keys_keep_the_caller_values() {
    let temp = tempfile::tempdir().unwrap();
    let settings_path = temp.path().join("settings.json");
    write_placement_settings_file(temp.path(), None, None);

    let mut candidate = settings_with_single_agent();
    candidate.main_geometry = Some(placement_geometry(11.0, 22.0));
    candidate.main_window_display_state = MainWindowDisplayState::Normal;

    let written = crate::config::settings::save_settings_to_path_preserving_project_paths(
        &candidate,
        &settings_path,
    )
    .unwrap();

    assert_eq!(written.main_geometry.as_ref().map(|g| g.x), Some(11.0));
    assert_eq!(
        written.main_window_display_state,
        MainWindowDisplayState::Normal
    );
    let raw = std::fs::read_to_string(&settings_path).unwrap();
    let disk: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(disk["mainGeometry"]["x"], json!(11.0));
    assert_eq!(disk["mainWindowDisplayState"], json!("normal"));
}

// ── #2475 synthetic selection-lock bench ─────────────────────────
//
// Synthetic numbers from a temporary directory: they are NOT the numbers
// of the real app. Latency is only reported under AC_SELECTION_BENCH=1
// with --test-threads=1; without it the bench asserts fixture,
// collection, correlation and occupancy invariants only.

use crate::config::replica_identity::strict_read_probe;
use crate::session::selection::timing_probe::{
    bench_serialization_lock, BenchContext, Occupancy, OccupancyGuard, SelectionTimingSample,
    TimingSinkRegistration,
};

const BENCH_COMMANDS: [&str; 3] = [
    "preview_selection_lock_removal",
    "preview_coding_agent_profile_selection",
    "get_replica_selection_default",
];
/// One budget per batch: it covers the serialization lock, `between` and
/// the wait for the reads, all measured against one `deadline_at`.
const BENCH_BATCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);
/// Drain grace after the budget, as an absolute `grace_at = deadline_at +
/// grace`, never counted from when the expiry is observed.
const BENCH_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(30);
const BENCH_REPETITIONS: usize = 10;

/// R rooms per project path, M project paths, one replica per room. The R
/// replicas of project 1 share the anchor's Matrix; projects 2..M hold
/// `other-<j>` replicas with their own Matrix, excluded by the Kind
/// Matrix filter after their strict read.
struct BenchFixture {
    /// Taken (and kept on disk) by the drain-expiry path.
    temp: std::sync::Mutex<Option<tempfile::TempDir>>,
    root: PathBuf,
    settings: AppSettings,
    anchor: PathBuf,
    ac_roots: Vec<PathBuf>,
    configs: Vec<PathBuf>,
}

impl BenchFixture {
    fn root(&self) -> &Path {
        &self.root
    }
}

fn bench_fixture(r: usize, m: usize) -> BenchFixture {
    let fixture = selection_api_fixture();
    let mut projects = vec![fixture.project.clone()];
    let mut ac_roots = vec![fixture.ac_root.clone()];
    for index in 2..=m {
        let project = fixture._temp.path().join(format!("project-{index}"));
        let ac_root = project.join(".ac");
        std::fs::create_dir_all(&ac_root).expect("create .ac");
        projects.push(project);
        ac_roots.push(ac_root);
    }
    let mut configs = Vec::new();
    for (index, ac_root) in ac_roots.iter().enumerate() {
        let name = if index == 0 {
            "bench-agent".to_string()
        } else {
            format!("other-{}", index + 1)
        };
        let matrix = ac_root.join(format!("_agent_{name}"));
        for room in 1..=r {
            let replica = ac_root
                .join(format!("room-{room}-bench"))
                .join(format!("__agent_{name}"));
            selection_api_replica_at(
                &fixture,
                &matrix,
                &replica,
                &name,
                locked_tooling("B", "agent-0"),
            );
            configs.push(replica.join("config.json"));
        }
    }
    let anchor = ac_roots[0].join("room-1-bench").join("__agent_bench-agent");
    let mut settings = selection_api_settings(&fixture);
    settings.project_paths = projects
        .iter()
        .map(|project| project.to_string_lossy().to_string())
        .collect();
    let SelectionApiFixture { _temp: temp, .. } = fixture;
    BenchFixture {
        root: temp.path().to_path_buf(),
        temp: std::sync::Mutex::new(Some(temp)),
        settings,
        anchor,
        ac_roots,
        configs,
    }
}

type BenchRead =
    std::pin::Pin<Box<dyn std::future::Future<Output = (u64, bool, std::time::Instant)>>>;
type BenchWait =
    std::pin::Pin<Box<dyn std::future::Future<Output = Vec<(u64, bool, std::time::Instant)>>>>;

struct BenchRun {
    samples: Vec<SelectionTimingSample>,
    by_invocation: std::collections::BTreeMap<u64, (&'static str, super::ProfileAssignmentScope)>,
    completed: std::collections::BTreeMap<u64, std::time::Instant>,
    batch_wall_ms: f64,
    occupancy_max: usize,
    body_threads: std::collections::HashSet<std::thread::ThreadId>,
}

/// What the harness does once the drain grace also expired: it lost
/// control of reads it launched. `Abort` is terminal; `Record` hands the
/// retained state back to a test that owns the stuck read.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum DrainExpiryAction {
    #[default]
    Abort,
    Record,
}

/// Instants the batch records, so a test can check the budget arithmetic.
#[derive(Clone, Copy, Debug, Default)]
struct BenchMilestones {
    deadline_at: Option<std::time::Instant>,
    grace_at: Option<std::time::Instant>,
    acquired: Option<std::time::Instant>,
    expired_observed: Option<std::time::Instant>,
}

#[derive(Default)]
struct BenchOptions {
    /// Measurement mode: an uncorrelated timed sample fails the run.
    measurement: bool,
    /// Test-only: record one timed sample WITHOUT a run context.
    inject_uncorrelated: bool,
    /// Test-only: a witness entered while the serialization lock is held.
    witness: Option<Arc<Occupancy>>,
    /// `None` means `BENCH_BATCH_BUDGET`.
    budget: Option<std::time::Duration>,
    /// `None` means `BENCH_DRAIN_GRACE`.
    grace: Option<std::time::Duration>,
    drain_expiry: DrainExpiryAction,
    /// Test-only: notified when the budget expires.
    on_deadline: Option<Arc<tokio::sync::Notify>>,
    /// Test-only: where the batch records its instants.
    milestones: Option<Arc<std::sync::Mutex<BenchMilestones>>>,
}

impl BenchOptions {
    fn record(&self, update: impl FnOnce(&mut BenchMilestones)) {
        if let Some(milestones) = &self.milestones {
            update(&mut milestones.lock().expect("milestones"));
        }
    }
}

/// The drain grace expired and the terminal action is `Record`: this
/// owns everything the batch still has in flight. Nothing is released
/// until the owner drains `wait`.
struct DrainExpired {
    diagnostic: String,
    pending: Vec<(u64, &'static str)>,
    retained: PathBuf,
    wait: BenchWait,
    sinks: Vec<TimingSinkRegistration>,
    serial: tokio::sync::MutexGuard<'static, ()>,
    _app: tauri::App<tauri::test::MockRuntime>,
}

impl DrainExpired {
    async fn drain(&mut self) {
        self.wait.as_mut().await;
    }
}

enum BenchError {
    Failed(String),
    DrainExpired(Box<DrainExpired>),
}

impl BenchError {
    fn message(&self) -> String {
        match self {
            Self::Failed(message) => message.clone(),
            Self::DrainExpired(expired) => expired.diagnostic.clone(),
        }
    }
}

impl std::fmt::Debug for BenchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

fn bench_deadline_at(options: &BenchOptions) -> std::time::Instant {
    std::time::Instant::now() + options.budget.unwrap_or(BENCH_BATCH_BUDGET)
}

/// The only way to take the bench serialization lock: bounded by the
/// batch's `deadline_at`.
async fn acquire_bench_serialization(
    deadline_at: std::time::Instant,
) -> Result<tokio::sync::MutexGuard<'static, ()>, String> {
    tokio::time::timeout_at(
        tokio::time::Instant::from_std(deadline_at),
        bench_serialization_lock().lock(),
    )
    .await
    .map_err(|_| "bench serialization lock not acquired in time".to_string())
}

fn bench_measurement_mode() -> bool {
    std::env::var("AC_SELECTION_BENCH").as_deref() == Ok("1")
}

fn ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Percentile with linear interpolation. Never an average.
fn pct(values: &mut [f64], q: f64) -> f64 {
    assert!(!values.is_empty(), "percentile of nothing");
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    let rank = q * (values.len() - 1) as f64;
    let low = rank.floor() as usize;
    let high = rank.ceil() as usize;
    values[low] + (values[high] - values[low]) * (rank - low as f64)
}

async fn bench_batch(bench: &BenchFixture) -> BenchRun {
    bench_batch_with(
        bench,
        BenchOptions {
            measurement: bench_measurement_mode(),
            ..BenchOptions::default()
        },
    )
    .await
    .expect("bench batch")
}

async fn bench_batch_with(
    bench: &BenchFixture,
    options: BenchOptions,
) -> Result<BenchRun, BenchError> {
    let deadline_at = bench_deadline_at(&options);
    let serial = acquire_bench_serialization(deadline_at)
        .await
        .map_err(BenchError::Failed)?;
    options.record(|m| m.acquired = Some(std::time::Instant::now()));
    bench_batch_locked(serial, bench, &options, deadline_at, async {}).await
}

/// The batch of 13 reads, each through its real command and the timed
/// turn wrapper. Every read is SUBMITTED before `between` runs. The batch
/// owns `serial` and its sinks until every submitted read has finished.
///
/// Cooperative budget: no WAIT of the batch path goes past `deadline_at`
/// (the caller used the same instant for the lock) and no drain wait
/// goes past `grace_at`. Fixture setup, the submitting polls and any
/// future that does not yield are outside the timer.
async fn bench_batch_locked(
    serial: tokio::sync::MutexGuard<'static, ()>,
    bench: &BenchFixture,
    options: &BenchOptions,
    deadline_at: std::time::Instant,
    between: impl std::future::Future<Output = ()> + 'static,
) -> Result<BenchRun, BenchError> {
    use std::task::Poll;
    use tauri::Manager;
    let grace_at = deadline_at + options.grace.unwrap_or(BENCH_DRAIN_GRACE);
    options.record(|m| {
        m.deadline_at = Some(deadline_at);
        m.grace_at = Some(grace_at);
    });
    let anchor = bench.anchor.as_path();
    let _witness = options.witness.as_ref().map(OccupancyGuard::enter);
    let app = tauri::test::mock_app();
    app.manage(empty_session_manager());
    app.manage(state_for(bench.settings.clone()));
    let handle = app.handle().clone();
    let done = Arc::new(std::sync::Mutex::new(
        std::collections::BTreeSet::<u64>::new(),
    ));

    let (sample_tx, sample_rx) = std::sync::mpsc::channel::<SelectionTimingSample>();
    let sample_tx = Arc::new(std::sync::Mutex::new(sample_tx));
    let _sinks: Vec<TimingSinkRegistration> = BENCH_COMMANDS
        .iter()
        .map(|command| {
            let sample_tx = Arc::clone(&sample_tx);
            TimingSinkRegistration::new_sample(
                command,
                Arc::new(move |sample: &SelectionTimingSample, _line: &str| {
                    let _ = sample_tx.lock().expect("sample tx").send(sample.clone());
                }),
            )
        })
        .collect();
    if options.inject_uncorrelated {
        crate::session::selection::run_owned_selection_operation_timed(
            "get_replica_selection_default",
            || async { Ok::<(), String>(()) },
        )
        .await
        .map_err(BenchError::Failed)?;
    }

    let occupancy = Arc::new(Occupancy::default());
    let mut by_invocation = std::collections::BTreeMap::new();
    let mut reads: Vec<BenchRead> = Vec::new();
    let scopes = [
        super::ProfileAssignmentScope::Replica,
        super::ProfileAssignmentScope::Kind,
        super::ProfileAssignmentScope::Workgroup,
    ];
    for scope in &scopes {
        for _ in 0..2 {
            let context = BenchContext::new(&occupancy);
            let id = context.invocation;
            by_invocation.insert(id, ("preview_selection_lock_removal", scope.clone()));
            let request = api_removal_preview_request(anchor, scope.clone());
            let (handle, done) = (handle.clone(), Arc::clone(&done));
            reads.push(Box::pin(async move {
                let ok = crate::session::selection::BENCH_CONTEXT
                    .scope(
                        context,
                        super::preview_selection_lock_removal(
                            handle.state(),
                            handle.state(),
                            request,
                        ),
                    )
                    .await
                    .is_ok();
                done.lock().expect("done").insert(id);
                (id, ok, std::time::Instant::now())
            }));
        }
    }
    for scope in &scopes {
        for _ in 0..2 {
            let context = BenchContext::new(&occupancy);
            let id = context.invocation;
            by_invocation.insert(
                id,
                ("preview_coding_agent_profile_selection", scope.clone()),
            );
            let request =
                api_preview_request(anchor, scope.clone(), super::AssignmentMode::Ordinary);
            let (handle, done) = (handle.clone(), Arc::clone(&done));
            reads.push(Box::pin(async move {
                let ok = crate::session::selection::BENCH_CONTEXT
                    .scope(
                        context,
                        super::preview_coding_agent_profile_selection(
                            handle.state(),
                            handle.state(),
                            request,
                        ),
                    )
                    .await
                    .is_ok();
                done.lock().expect("done").insert(id);
                (id, ok, std::time::Instant::now())
            }));
        }
    }
    {
        let context = BenchContext::new(&occupancy);
        let id = context.invocation;
        by_invocation.insert(
            id,
            (
                "get_replica_selection_default",
                super::ProfileAssignmentScope::Replica,
            ),
        );
        let request = api_default_request(anchor);
        let (handle, done) = (handle.clone(), Arc::clone(&done));
        reads.push(Box::pin(async move {
            let ok = crate::session::selection::BENCH_CONTEXT
                .scope(
                    context,
                    super::get_replica_selection_default(handle.state(), request),
                )
                .await
                .is_ok();
            done.lock().expect("done").insert(id);
            (id, ok, std::time::Instant::now())
        }));
    }

    // One poll per read runs its submission: `submitted` is stamped and
    // the owned task is spawned before the next read is built.
    let mut finished = Vec::new();
    let mut pending = Vec::new();
    for mut read in reads {
        match futures::poll!(read.as_mut()) {
            Poll::Ready(done) => finished.push(done),
            Poll::Pending => pending.push(read),
        }
    }
    let submitted_reads = pending.len();
    let mut wait: BenchWait = Box::pin(async move {
        between.await;
        futures::future::join_all(pending).await
    });
    let joined =
        match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline_at), wait.as_mut())
            .await
        {
            Ok(joined) => joined,
            Err(_) => {
                options.record(|m| m.expired_observed = Some(std::time::Instant::now()));
                if let Some(notify) = &options.on_deadline {
                    notify.notify_one();
                }
                // The owned reads are detached, not cancelled, when their
                // waiters drop: keep the sinks and the serialization lock
                // until every one of them has finished.
                if tokio::time::timeout_at(tokio::time::Instant::from_std(grace_at), wait.as_mut())
                    .await
                    .is_ok()
                {
                    drop(serial);
                    return Err(BenchError::Failed(format!(
                        "bench batch timed out; {submitted_reads} submitted reads drained before \
                    release"
                    )));
                }
                // The harness lost control of reads it launched. In order:
                // 1. keep the fixture on disk, so no orphan reads a deleted path;
                let retained = bench
                    .temp
                    .lock()
                    .expect("fixture temp")
                    .take()
                    .map(tempfile::TempDir::keep)
                    .unwrap_or_else(|| bench.root.clone());
                // 2. keep the sinks registered and 3. keep `serial` held;
                // 4. name what is still pending and where the fixture is.
                let finished_ids = done.lock().expect("done").clone();
                let pending_reads: Vec<(u64, &'static str)> = by_invocation
                    .iter()
                    .filter(|(id, _)| !finished_ids.contains(id))
                    .map(|(id, (command, _))| (*id, *command))
                    .collect();
                let diagnostic = format!(
                "bench drain expired: pending invocations {pending_reads:?}; retained fixture {}",
                retained.display()
            );
                eprintln!("{diagnostic}");
                // 5. the terminal action.
                match options.drain_expiry {
                    DrainExpiryAction::Abort => std::process::abort(),
                    DrainExpiryAction::Record => {
                        return Err(BenchError::DrainExpired(Box::new(DrainExpired {
                            diagnostic,
                            pending: pending_reads,
                            retained,
                            wait,
                            sinks: _sinks,
                            serial,
                            _app: app,
                        })));
                    }
                }
            }
        };
    finished.extend(joined);

    let mut completed = std::collections::BTreeMap::new();
    for (id, ok, at) in finished {
        if !ok {
            return Err(BenchError::Failed(format!("bench read {id} failed")));
        }
        completed.insert(id, at);
    }

    // Every sample is recorded inside its owned task before that task
    // completes, so all of this batch's samples are already queued.
    let mut samples = Vec::new();
    let mut uncorrelated = 0usize;
    for sample in sample_rx.try_iter() {
        match sample.invocation {
            Some(id) if by_invocation.contains_key(&id) => samples.push(sample),
            Some(_) => {}
            None => uncorrelated += 1,
        }
    }
    if options.measurement && uncorrelated > 0 {
        return Err(BenchError::Failed(format!(
            "measurement run received {uncorrelated} uncorrelated timed sample(s)"
        )));
    }
    let received: std::collections::BTreeSet<u64> = samples
        .iter()
        .filter_map(|sample| sample.invocation)
        .collect();
    if samples.len() != by_invocation.len() || received.len() != by_invocation.len() {
        let missing: Vec<u64> = by_invocation
            .keys()
            .filter(|id| !received.contains(id))
            .copied()
            .collect();
        return Err(BenchError::Failed(format!(
            "received {} of {} samples; missing ids {missing:?}",
            samples.len(),
            by_invocation.len()
        )));
    }
    let first_submit = samples
        .iter()
        .map(|sample| sample.submitted)
        .min()
        .expect("samples");
    let last_done = *completed.values().max().expect("completions");
    Ok(BenchRun {
        batch_wall_ms: ms(last_done.saturating_duration_since(first_submit)),
        occupancy_max: occupancy.max(),
        body_threads: samples.iter().map(|sample| sample.body_thread).collect(),
        samples,
        by_invocation,
        completed,
    })
}

fn bench_key(run: &BenchRun, sample: &SelectionTimingSample) -> String {
    let (command, scope) = &run.by_invocation[&sample.invocation.expect("correlated")];
    format!("{command}/{scope:?}")
}

fn bench_sum_queue_wait_ms(run: &BenchRun) -> f64 {
    run.samples.iter().map(|sample| ms(sample.queue_wait)).sum()
}

/// One report block for a set of runs (the cold one, or the warm ones).
fn bench_report(label: &str, runs: &[&BenchRun]) {
    let mut walls: Vec<f64> = runs.iter().map(|run| run.batch_wall_ms).collect();
    let mut sums: Vec<f64> = runs
        .iter()
        .map(|run| bench_sum_queue_wait_ms(run))
        .collect();
    let mut work_by_key: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
    let mut work_by_command: std::collections::BTreeMap<&str, Vec<f64>> = Default::default();
    let mut wait_by_command: std::collections::BTreeMap<&str, Vec<f64>> = Default::default();
    let mut wait_all = Vec::new();
    for run in runs {
        for sample in &run.samples {
            work_by_key
                .entry(bench_key(run, sample))
                .or_default()
                .push(ms(sample.work));
            work_by_command
                .entry(sample.command)
                .or_default()
                .push(ms(sample.work));
            wait_by_command
                .entry(sample.command)
                .or_default()
                .push(ms(sample.queue_wait));
            wait_all.push(ms(sample.queue_wait));
        }
    }
    let (slowest_key, slowest_p90) = work_by_key
        .iter_mut()
        .map(|(key, values)| (key.clone(), pct(values, 0.9)))
        .fold((String::new(), f64::MIN), |best, next| {
            if next.1 > best.1 {
                next
            } else {
                best
            }
        });
    let occupancy = runs.iter().map(|run| run.occupancy_max).max().unwrap_or(0);
    let threads = runs
        .iter()
        .map(|run| run.body_threads.len())
        .max()
        .unwrap_or(0);
    println!(
        "[bench] {label} runs={} batch_wall_ms p50={:.1} p90={:.1} sum_queue_wait_ms p50={:.1} \
         slowest_work_p90_ms={:.1} ({slowest_key}) occupancy_max={occupancy} \
         diag_body_threads={threads}",
        runs.len(),
        pct(&mut walls.clone(), 0.5),
        pct(&mut walls, 0.9),
        pct(&mut sums, 0.5),
        slowest_p90,
    );
    println!(
        "[bench] {label} queue_wait_ms aggregate p50={:.1} p90={:.1}",
        pct(&mut wait_all.clone(), 0.5),
        pct(&mut wait_all, 0.9)
    );
    for (command, values) in wait_by_command.iter_mut() {
        let work = work_by_command.get_mut(command).expect("work");
        println!(
            "[bench] {label} {command} queue_wait_ms p50={:.1} p90={:.1} work_ms p50={:.1} \
            p90={:.1}",
            pct(&mut values.clone(), 0.5),
            pct(values, 0.9),
            pct(&mut work.clone(), 0.5),
            pct(work, 0.9)
        );
    }
    for (key, values) in work_by_key.iter_mut() {
        println!(
            "[bench] {label} {key} work_ms p50={:.1} p90={:.1}",
            pct(&mut values.clone(), 0.5),
            pct(values, 0.9)
        );
    }
}

fn assert_bench_correlation(run: &BenchRun) {
    assert_eq!(run.samples.len(), 13, "13 samples");
    let ids: std::collections::BTreeSet<u64> = run
        .samples
        .iter()
        .map(|sample| sample.invocation.expect("correlated sample"))
        .collect();
    assert_eq!(ids.len(), 13, "distinct invocation ids");
    for sample in &run.samples {
        let id = sample.invocation.expect("correlated sample");
        let (command, _scope) = run.by_invocation.get(&id).expect("known invocation");
        assert_eq!(*command, sample.command, "sample matches its invocation");
        assert!(sample.ok, "read {id} succeeded");
        let done = run.completed[&id];
        assert!(
            sample.submitted + sample.queue_wait + sample.work <= done,
            "invocation {id} submitted before it finished"
        );
    }
    let mut per_scope: std::collections::BTreeMap<String, usize> = Default::default();
    for (command, scope) in run.by_invocation.values() {
        *per_scope.entry(format!("{command}/{scope:?}")).or_default() += 1;
    }
    assert_eq!(per_scope.len(), 7, "declared mix: {per_scope:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_fixture_invariants() {
    for (r, m) in [(3usize, 2usize), (8, 1)] {
        let bench = bench_fixture(r, m);
        let on_disk = bench.configs.iter().filter(|path| path.is_file()).count();
        assert_eq!(on_disk, r * m, "config.json on disk for R={r} M={m}");
        let mut walked = super::CandidateDirs::new();
        for ac_root in &bench.ac_roots {
            super::collect_kind_replica_dirs(ac_root, &mut walked);
        }
        assert_eq!(walked.dirs.len(), r * m, "replicas walked by kind");
        let settings = state_for(bench.settings.clone());
        for (scope, expected) in [
            (super::ProfileAssignmentScope::Kind, r),
            (super::ProfileAssignmentScope::Workgroup, 1),
            (super::ProfileAssignmentScope::Replica, 1),
        ] {
            let removal = api_removal_preview(&settings, &bench.anchor, scope.clone()).await;
            assert_eq!(removal.targets.len(), expected, "{scope:?} R={r} M={m}");
            assert!(removal.counts_complete, "{scope:?} counts complete");
            let assignment = api_preview(
                &settings,
                &bench.anchor,
                scope.clone(),
                super::AssignmentMode::Ordinary,
            )
            .await;
            assert_eq!(assignment.targets.len(), expected, "{scope:?} R={r} M={m}");
            assert!(assignment.counts_complete, "{scope:?} counts complete");
        }
    }
}

/// Strict-reader calls in kind after p4's single read. p4 updated exactly
/// these two expected values: 16 -> 8 and 160 -> 120.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_strict_reads_before_the_single_read_change() {
    for (r, m, expected) in [(8usize, 1usize, 8usize), (40, 3, 120)] {
        let bench = bench_fixture(r, m);
        let settings = state_for(bench.settings.clone());
        strict_read_probe::register(bench.root());
        let removal = api_removal_preview(
            &settings,
            &bench.anchor,
            super::ProfileAssignmentScope::Kind,
        )
        .await;
        let calls = strict_read_probe::unregister(bench.root());
        assert_eq!(removal.targets.len(), r);
        assert_eq!(calls, expected, "strict-reader calls in kind, R={r} M={m}");
    }
}

#[test]
fn issue_2475_bench_counter_ignores_reads_outside_the_fixture() {
    let bench = bench_fixture(1, 1);
    let outside = selection_api_fixture();
    let outside_replica = selection_api_replica(
        &outside,
        "room-1-outside",
        "outside-agent",
        locked_tooling("B", "agent-0"),
    );
    strict_read_probe::register(bench.root());
    crate::config::replica_identity::read_wg_replica_config_read_only(&outside_replica)
        .expect("outside strict read");
    assert_eq!(
        strict_read_probe::count(bench.root()),
        0,
        "a strict read outside the fixture is not counted"
    );
    crate::config::replica_identity::read_wg_replica_config_read_only(&bench.anchor)
        .expect("inside strict read");
    assert_eq!(strict_read_probe::unregister(bench.root()), 1);
}

/// Strict-reader calls of one kind enumeration over `bench_fixture(r, m)`.
async fn kind_strict_reader_calls(r: usize, m: usize) -> usize {
    let bench = bench_fixture(r, m);
    let settings = state_for(bench.settings.clone());
    strict_read_probe::register(bench.root());
    let removal = api_removal_preview(
        &settings,
        &bench.anchor,
        super::ProfileAssignmentScope::Kind,
    )
    .await;
    let calls = strict_read_probe::unregister(bench.root());
    assert_eq!(removal.targets.len(), r, "kind targets, R={r} M={m}");
    calls
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_enumeration_reads_each_included_config_once() {
    assert_eq!(
        kind_strict_reader_calls(8, 1).await,
        8,
        "strict-reader calls in kind, R=8 M=1"
    );
}

/// The 8 Matrix-excluded replicas already cost one strict-reader call:
/// the Kind filter continues before the selection-state read. 16 -> 12,
/// never 24 -> 12.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_excluded_replicas_already_cost_one_read() {
    assert_eq!(
        kind_strict_reader_calls(4, 3).await,
        12,
        "strict-reader calls in kind, R=4 M=3"
    );
}

#[test]
fn issue_2475_enumeration_result_is_byte_identical_after_the_single_read() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "qa",
        unlocked_tooling("A", "agent-1"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "bad-tooling",
        json!({ "selectionLocked": "yes" }),
    );
    let broken = selection_api_replica(&fixture, "room-1-team", "broken", json!({}));
    std::fs::write(broken.join("config.json"), b"{not json").expect("break config");
    let unreadable = selection_api_replica(&fixture, "room-1-team", "unreadable", json!({}));
    std::fs::remove_file(unreadable.join("config.json")).expect("remove config");
    std::fs::create_dir(unreadable.join("config.json")).expect("config.json as a dir");

    let settings = selection_api_settings(&fixture);
    let enumeration = super::enumerate_profile_assignment_targets(
        &settings,
        &anchor,
        &super::ProfileAssignmentScope::Workgroup,
        &[],
    )
    .expect("enumerate");

    let canonical = |path: &Path| super::canonical_real_dir(path, "test").expect("canonical");
    let room = canonical(&fixture.ac_root.join("room-1-team"));
    let wire =
        |path: &Path| crate::path_utils::path_to_string_without_windows_verbatim_prefix(path);
    let target = |name: &str, pair: Value, state: &str, error: Value| {
        json!({
            "workgroupName": "room-1-team",
            "workgroupPath": wire(&room),
            "replicaName": name,
            "replicaPath": wire(&room.join(format!("__agent_{name}"))),
            "identityPath": format!("../../_agent_{name}"),
            "originProject": "project",
            "liveSessionIds": [],
            "savedPair": pair,
            "selectionState": state,
            "selectionError": error,
        })
    };
    // read_dir order is platform-defined, so the warnings compare sorted;
    // the candidates are sorted by the enumeration itself.
    let mut warnings = enumeration.warnings.clone();
    warnings.sort();
    let actual = json!({
        "candidates": enumeration
            .candidates
            .iter()
            .map(|candidate| serde_json::to_value(&candidate.target).expect("serialize"))
            .collect::<Vec<_>>(),
        "warnings": warnings,
        "countsComplete": enumeration.counts_complete,
    });
    let expected = json!({
        "candidates": [
            target(
                "bad-tooling",
                Value::Null,
                "invalid",
                json!("tooling.selectionLocked must be a boolean"),
            ),
            target(
                "dev-rust",
                json!({ "codingAgentId": "agent-0", "requestedProfile": "B" }),
                "locked",
                Value::Null,
            ),
            target(
                "qa",
                json!({ "codingAgentId": "agent-1", "requestedProfile": "A" }),
                "unlocked",
                Value::Null,
            ),
        ],
        "warnings": [
            format!("Skipping invalid replica '{}'", room.join("__agent_broken").display()),
            format!("Skipping invalid replica '{}'", room.join("__agent_unreadable").display()),
        ],
        "countsComplete": false,
    });
    assert_eq!(actual, expected);
}

/// #2557 - the #2475 byte-identity fixture: two invalid replicas
/// (`broken` is not JSON, `unreadable` has config.json as a directory).
fn issue_2557_broken_room() -> (
    SelectionApiFixture,
    super::ProfileTargetEnumeration,
    PathBuf,
) {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "qa",
        unlocked_tooling("A", "agent-1"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "bad-tooling",
        json!({ "selectionLocked": "yes" }),
    );
    let broken = selection_api_replica(&fixture, "room-1-team", "broken", json!({}));
    std::fs::write(broken.join("config.json"), b"{not json").expect("break config");
    let unreadable = selection_api_replica(&fixture, "room-1-team", "unreadable", json!({}));
    std::fs::remove_file(unreadable.join("config.json")).expect("remove config");
    std::fs::create_dir(unreadable.join("config.json")).expect("config.json as a dir");

    let settings = selection_api_settings(&fixture);
    let enumeration = super::enumerate_profile_assignment_targets(
        &settings,
        &anchor,
        &super::ProfileAssignmentScope::Workgroup,
        &[],
    )
    .expect("enumerate");
    let room =
        super::canonical_real_dir(&fixture.ac_root.join("room-1-team"), "test").expect("canonical");
    (fixture, enumeration, room)
}

#[test]
fn issue_2557_scope_faults_name_the_replica_and_the_reason() {
    let (_fixture, enumeration, room) = issue_2557_broken_room();
    let wire =
        |path: &Path| crate::path_utils::path_to_string_without_windows_verbatim_prefix(path);
    let faults = &enumeration.scope_faults;

    assert_eq!(faults.len(), 2, "{faults:?}");
    assert_eq!(
        faults
            .iter()
            .map(|fault| (fault.replica_name.as_str(), fault.code))
            .collect::<Vec<_>>(),
        vec![
            ("broken", super::ScopeFaultCode::ConfigNotJson),
            ("unreadable", super::ScopeFaultCode::ConfigUnreadable),
        ]
    );
    assert_eq!(
        serde_json::to_value(&faults[0]).expect("serialize"),
        json!({
            "code": "configNotJson",
            "replicaName": "broken",
            "replicaPath": wire(&room.join("__agent_broken")),
        })
    );
}

#[test]
fn issue_2557_warnings_stay_byte_identical() {
    let (_fixture, enumeration, _room) = issue_2557_broken_room();
    assert_eq!(enumeration.warnings.len(), 2);
    assert_eq!(
        enumeration.warnings.len(),
        enumeration.scope_faults.len(),
        "every skipped replica yields one warning and one fault"
    );
}

#[test]
fn issue_2557_unreadable_folder_is_named_too() {
    let fixture = selection_api_fixture();
    let missing = fixture.ac_root.join("no-existe");
    type Collector = fn(&Path, &mut super::CandidateDirs);
    let collectors: [(&str, Collector); 2] = [
        ("kind", super::collect_kind_replica_dirs),
        ("workgroup", super::collect_replica_dirs_in_workgroup),
    ];
    for (label, collect) in collectors {
        let mut dirs = super::CandidateDirs::new();
        collect(&missing, &mut dirs);
        assert!(!dirs.complete, "{label}");
        assert_eq!(dirs.faults.len(), 1, "{label}: {:?}", dirs.faults);
        assert_eq!(
            dirs.faults[0].code,
            super::ScopeFaultCode::FolderUnreadable,
            "{label}"
        );
        assert_eq!(dirs.faults[0].replica_name, "no-existe", "{label}");
    }
}

#[test]
fn issue_2557_clean_scope_has_zero_faults() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "qa",
        unlocked_tooling("A", "agent-1"),
    );
    let enumeration = super::enumerate_profile_assignment_targets(
        &selection_api_settings(&fixture),
        &anchor,
        &super::ProfileAssignmentScope::Workgroup,
        &[],
    )
    .expect("enumerate");
    assert!(enumeration.counts_complete);
    assert!(
        enumeration.scope_faults.is_empty(),
        "{:?}",
        enumeration.scope_faults
    );
}

#[test]
fn issue_2475_anchor_read_failure_keeps_its_error_text() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    // The lenient anchor validation accepts a duplicate key; the strict
    // reader rejects it, so the failure is the strict read's own.
    std::fs::write(
        anchor.join("config.json"),
        br#"{"identity":"../../_agent_dev-rust","tooling":{},"tooling":{}}"#,
    )
    .expect("break anchor");
    let settings = selection_api_settings(&fixture);
    let error = match super::enumerate_profile_assignment_targets(
        &settings,
        &anchor,
        &super::ProfileAssignmentScope::Replica,
        &[],
    ) {
        Ok(_) => panic!("a strictly unparseable anchor must fail the enumeration"),
        Err(error) => error,
    };
    assert!(error.contains("Target replica '"), "{error}");
}

#[test]
fn issue_2475_invalid_candidate_still_clears_counts_complete() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    let broken = selection_api_replica(&fixture, "room-1-team", "broken", json!({}));
    std::fs::write(broken.join("config.json"), b"{not json").expect("break config");
    let settings = selection_api_settings(&fixture);
    let enumeration = super::enumerate_profile_assignment_targets(
        &settings,
        &anchor,
        &super::ProfileAssignmentScope::Workgroup,
        &[],
    )
    .expect("enumerate");
    assert_eq!(enumeration.candidates.len(), 1);
    assert!(
        !enumeration.counts_complete,
        "an invalid replica clears counts_complete"
    );
}

/// The batch test. Without AC_SELECTION_BENCH it asserts one batch's
/// collection and correlation; with AC_SELECTION_BENCH=1 (run it with
/// --test-threads=1) it also runs 10 repetitions of both fixtures and
/// prints the baseline. Rep 1 is cold, reps 2..10 are warm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_samples_carry_invocation_scope_and_submit_instant() {
    let bench = bench_fixture(8, 1);
    let run = bench_batch(&bench).await;
    assert_bench_correlation(&run);
    assert!(run.occupancy_max >= 1, "bodies entered the occupancy");

    if !bench_measurement_mode() {
        return;
    }
    println!(
        "[bench] synthetic numbers from a temporary directory, not the real app; \
         mix 6 preview_selection_lock_removal + 6 preview_coding_agent_profile_selection \
         (2 per scope) + 1 get_replica_selection_default; available_parallelism={}",
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    for (r, m) in [(8usize, 1usize), (40, 3)] {
        let bench = bench_fixture(r, m);
        let mut runs = Vec::new();
        for _ in 0..BENCH_REPETITIONS {
            let run = bench_batch(&bench).await;
            assert_bench_correlation(&run);
            runs.push(run);
        }
        let cold = [&runs[0]];
        let warm: Vec<&BenchRun> = runs[1..].iter().collect();
        bench_report(&format!("R={r} M={m} cold"), &cold);
        bench_report(&format!("R={r} M={m} warm"), &warm);
    }
}

/// Writer control: every reader's wait covers the part of the writer's
/// hold it actually overlapped, `submitted` taken from the sample.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_reader_wait_covers_the_measured_overlap() {
    const TOLERANCE_MS: f64 = 0.05;
    let bench = bench_fixture(3, 1);
    let options = BenchOptions::default();
    // The test takes the shared lock under its own bound; the small
    // batch budget starts once it holds it.
    let serial =
        acquire_bench_serialization(std::time::Instant::now() + std::time::Duration::from_secs(60))
            .await
            .expect("serialization");
    let deadline_at = bench_deadline_at(&options);
    let writer = crate::session::selection::acquire_selection_operation_turn()
        .await
        .expect("writer turn");
    let acquired = std::time::Instant::now();
    let released = Arc::new(std::sync::Mutex::new(None::<std::time::Instant>));
    let release_slot = Arc::clone(&released);
    let run = bench_batch_locked(serial, &bench, &options, deadline_at, async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        *release_slot.lock().expect("release slot") = Some(std::time::Instant::now());
        drop(writer);
    })
    .await
    .expect("bench batch");
    let released = released
        .lock()
        .expect("release slot")
        .expect("writer released");
    assert_bench_correlation(&run);
    for sample in &run.samples {
        let start = sample.submitted.max(acquired);
        assert!(start < released, "reader submitted while the writer held");
        let overlap = ms(released - start);
        assert!(
            ms(sample.queue_wait) + TOLERANCE_MS >= overlap,
            "queue_wait {:.3} ms must cover the {overlap:.3} ms overlap",
            ms(sample.queue_wait)
        );
    }
}

#[test]
fn issue_2475_bench_percentiles_are_not_averages() {
    let values = vec![1.0, 2.0, 3.0, 4.0, 100.0];
    assert_eq!(pct(&mut values.clone(), 0.5), 3.0);
    assert!(pct(&mut values.clone(), 0.9) >= 60.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_rejects_an_uncorrelated_timed_sample() {
    let bench = bench_fixture(1, 1);
    let rejected = bench_batch_with(
        &bench,
        BenchOptions {
            measurement: true,
            inject_uncorrelated: true,
            ..BenchOptions::default()
        },
    )
    .await;
    let error = rejected.err().expect("measurement run must fail").message();
    assert!(error.contains("uncorrelated"), "{error}");
    // Outside measurement mode the same batch is accepted.
    let accepted = bench_batch_with(
        &bench,
        BenchOptions {
            inject_uncorrelated: true,
            ..BenchOptions::default()
        },
    )
    .await
    .expect("non-measurement run");
    assert_bench_correlation(&accepted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_fixture_leaves_config_bytes_unchanged() {
    let bench = bench_fixture(3, 2);
    let before: Vec<Vec<u8>> = bench
        .configs
        .iter()
        .map(|path| std::fs::read(path).expect("read config.json"))
        .collect();
    let run = bench_batch(&bench).await;
    assert_bench_correlation(&run);
    let after: Vec<Vec<u8>> = bench
        .configs
        .iter()
        .map(|path| std::fs::read(path).expect("read config.json"))
        .collect();
    assert_eq!(before.len(), 6);
    assert!(before == after, "the bench must not write any config.json");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_holds_the_serialization_lock() {
    let bench = bench_fixture(3, 1);
    let witness = Arc::new(Occupancy::default());
    let options = || BenchOptions {
        witness: Some(Arc::clone(&witness)),
        ..BenchOptions::default()
    };
    let (first, second) = tokio::join!(
        bench_batch_with(&bench, options()),
        bench_batch_with(&bench, options()),
    );
    assert_bench_correlation(&first.expect("first run"));
    assert_bench_correlation(&second.expect("second run"));
    assert_eq!(witness.max(), 1, "two bench runs never overlap");
    assert_eq!(witness.in_flight(), 0);
}

/// Correlated samples seen for the three bench commands, via sinks the
/// test itself owns.
fn bench_correlated_counter() -> (
    Arc<std::sync::atomic::AtomicUsize>,
    Vec<TimingSinkRegistration>,
) {
    let correlated = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sinks = BENCH_COMMANDS
        .iter()
        .map(|command| {
            let correlated = Arc::clone(&correlated);
            TimingSinkRegistration::new_sample(
                command,
                Arc::new(move |sample: &SelectionTimingSample, _line: &str| {
                    if sample.invocation.is_some() {
                        correlated.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                }),
            )
        })
        .collect();
    (correlated, sinks)
}

fn bench_registered_sinks() -> usize {
    BENCH_COMMANDS
        .iter()
        .map(|command| crate::session::selection::timing_probe::registered_sinks(command))
        .sum()
}

/// Step 7.6 test 1: the batch returns only after every submitted read
/// finished. Observed at the instant of the return, not after a join.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_deadline_drains_submitted_reads_before_release() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let bench = bench_fixture(3, 1);
    let expired = Arc::new(tokio::sync::Notify::new());
    let options = BenchOptions {
        budget: Some(std::time::Duration::from_millis(50)),
        grace: Some(std::time::Duration::from_secs(10)),
        drain_expiry: DrainExpiryAction::Record,
        on_deadline: Some(Arc::clone(&expired)),
        ..BenchOptions::default()
    };
    // The test takes the shared lock under its own bound; the small
    // batch budget starts once it holds it.
    let serial =
        acquire_bench_serialization(std::time::Instant::now() + std::time::Duration::from_secs(60))
            .await
            .expect("serialization");
    let deadline_at = bench_deadline_at(&options);
    let (correlated, _probe_sinks) = bench_correlated_counter();
    let writer = crate::session::selection::acquire_selection_operation_turn()
        .await
        .expect("writer turn");
    let released = Arc::new(AtomicBool::new(false));
    let wake_releaser = Arc::clone(&expired);
    let releaser = tokio::spawn({
        let released = Arc::clone(&released);
        let correlated = Arc::clone(&correlated);
        async move {
            // Bounded: the writer turn is process-wide and must come back.
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(60), expired.notified()).await;
            let before = correlated.load(Ordering::SeqCst);
            released.store(true, Ordering::SeqCst);
            drop(writer);
            before
        }
    });
    let (result, at_return) = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let result = bench_batch_locked(serial, &bench, &options, deadline_at, async {}).await;
        // Copied in the immediate continuation of the batch's return.
        let at_return = (
            released.load(Ordering::SeqCst),
            correlated.load(Ordering::SeqCst),
        );
        (result, at_return)
    })
    .await
    .expect("the test itself is bounded");
    // If the batch returned without reaching its deadline, the releaser
    // must still give the process-wide turn back now.
    wake_releaser.notify_one();
    let before_release = releaser.await.expect("releaser");
    let error = match result {
        Err(BenchError::Failed(error)) => error,
        Err(BenchError::DrainExpired(mut expired)) => {
            expired.drain().await;
            panic!("unexpected drain expiry: {}", expired.diagnostic);
        }
        Ok(_) => panic!("the batch must fail on its budget"),
    };
    assert!(error.contains("timed out"), "{error}");
    assert_eq!(
        before_release, 0,
        "no read ran while the writer held the turn"
    );
    assert!(
        at_return.0,
        "the batch returned only after the writer released"
    );
    assert_eq!(
        at_return.1, 13,
        "every submitted read finished before the batch returned"
    );
}

/// Step 7.6 test 2: while a batch drains, a bounded acquisition of the
/// serialization lock fails with its own error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_draining_batch_does_not_hand_over_serialization() {
    let bench = bench_fixture(3, 1);
    let expired = Arc::new(tokio::sync::Notify::new());
    let options = BenchOptions {
        budget: Some(std::time::Duration::from_millis(50)),
        grace: Some(std::time::Duration::from_secs(10)),
        drain_expiry: DrainExpiryAction::Record,
        on_deadline: Some(Arc::clone(&expired)),
        ..BenchOptions::default()
    };
    // The test takes the shared lock under its own bound; the small
    // batch budget starts once it holds it.
    let serial =
        acquire_bench_serialization(std::time::Instant::now() + std::time::Duration::from_secs(60))
            .await
            .expect("serialization");
    let deadline_at = bench_deadline_at(&options);
    let writer = crate::session::selection::acquire_selection_operation_turn()
        .await
        .expect("writer turn");
    let batch = bench_batch_locked(serial, &bench, &options, deadline_at, async {});
    tokio::pin!(batch);
    let reached_drain = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        tokio::select! {
            _ = &mut batch => false,
            _ = expired.notified() => true,
        }
    })
    .await
    .expect("the test itself is bounded");
    // The batch is now draining and must still own the lock.
    let second = acquire_bench_serialization(
        std::time::Instant::now() + std::time::Duration::from_millis(100),
    )
    .await;
    drop(writer);
    let result = tokio::time::timeout(std::time::Duration::from_secs(60), batch)
        .await
        .expect("the test itself is bounded");
    assert!(reached_drain, "the batch reached its drain");
    assert_eq!(
        second.err().as_deref(),
        Some("bench serialization lock not acquired in time"),
        "no batch takes the turn from a draining one"
    );
    assert!(result
        .err()
        .expect("budget failure")
        .message()
        .contains("timed out"));
}

/// Step 7.6 test 3: with `Record`, the terminal boundary keeps everything
/// (fixture on disk, sinks registered, lock held, diagnostic), and only
/// after the stuck read is released and drained does the test recover.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_drain_expiry_retains_then_recovers_the_shared_state() {
    let bench = bench_fixture(3, 1);
    let options = BenchOptions {
        budget: Some(std::time::Duration::from_millis(50)),
        grace: Some(std::time::Duration::from_millis(50)),
        drain_expiry: DrainExpiryAction::Record,
        ..BenchOptions::default()
    };
    // The test takes the shared lock under its own bound; the small
    // batch budget starts once it holds it.
    let serial =
        acquire_bench_serialization(std::time::Instant::now() + std::time::Duration::from_secs(60))
            .await
            .expect("serialization");
    let deadline_at = bench_deadline_at(&options);
    // The test owns the stuck read: this writer keeps it queued past grace_at.
    let writer = crate::session::selection::acquire_selection_operation_turn()
        .await
        .expect("writer turn");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        bench_batch_locked(serial, &bench, &options, deadline_at, async {}),
    )
    .await
    .expect("the test itself is bounded");

    // Observations first, at the terminal boundary.
    let mut expired = match result {
        Err(BenchError::DrainExpired(expired)) => Some(expired),
        _ => None,
    };
    let observed = expired.as_ref().map(|expired| {
        let pending_named = expired.pending.iter().all(|(id, command)| {
            expired.diagnostic.contains(&id.to_string()) && expired.diagnostic.contains(command)
        });
        (
            expired.retained.is_dir(),
            bench.temp.lock().expect("temp").is_none(),
            bench_registered_sinks(),
            expired.pending.len(),
            pending_named,
            expired
                .diagnostic
                .contains(&expired.retained.display().to_string()),
        )
    });
    let lock_held = acquire_bench_serialization(
        std::time::Instant::now() + std::time::Duration::from_millis(100),
    )
    .await
    .is_err();

    // Deterministic recovery, whatever the observations say: release the
    // read, join ALL submitted work, then sinks, lock, fixture.
    drop(writer);
    let mut sinks_after = None;
    if let Some(mut expired) = expired.take() {
        tokio::time::timeout(std::time::Duration::from_secs(60), expired.drain())
            .await
            .expect("the released read drains");
        let DrainExpired {
            sinks,
            serial,
            retained,
            _app,
            ..
        } = *expired;
        drop(sinks);
        sinks_after = Some(bench_registered_sinks());
        drop(serial);
        std::fs::remove_dir_all(&retained).expect("remove the retained fixture");
    }
    let later = bench_fixture(3, 1);
    let later_run = tokio::time::timeout(std::time::Duration::from_secs(60), bench_batch(&later))
        .await
        .expect("the test itself is bounded");

    let observed = observed.expect("the drain grace expired with Record");
    assert!(observed.0, "the fixture stays on disk");
    assert!(observed.1, "the fixture TempDir was taken and kept");
    assert_eq!(observed.2, 3, "the batch sinks stay registered");
    assert_eq!(observed.3, 13, "every read is still pending");
    assert!(observed.4, "the diagnostic names every pending invocation");
    assert!(observed.5, "the diagnostic names the retained fixture");
    assert!(lock_held, "the serialization lock was not released");
    assert_eq!(sinks_after, Some(0), "recovery unregistered the sinks");
    assert_bench_correlation(&later_run);
}

/// Step 7.6 test 4: the default value only, not its dispatch.
#[test]
fn issue_2475_bench_default_drain_expiry_action_is_abort() {
    assert_eq!(
        BenchOptions::default().drain_expiry,
        DrainExpiryAction::Abort
    );
}

const BENCH_ABORT_CHILD_MARKER: &str = "AC_BENCH_ABORT_CHILD";
const BENCH_ABORT_CHILD_TEST: &str =
    "commands::config::tests::issue_2475_bench_drain_expiry_aborts_in_a_child_process";

/// Step 7.6 test 6: the default action really aborts. The terminal path
/// runs in a child process (this same test, marked), so the suite
/// survives it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_drain_expiry_aborts_in_a_child_process() {
    if std::env::var_os(BENCH_ABORT_CHILD_MARKER).is_some() {
        let bench = bench_fixture(1, 1);
        let options = BenchOptions {
            budget: Some(std::time::Duration::from_millis(50)),
            grace: Some(std::time::Duration::from_millis(50)),
            ..BenchOptions::default()
        };
        let deadline_at = bench_deadline_at(&options);
        let serial = acquire_bench_serialization(deadline_at)
            .await
            .expect("serialization");
        let _writer = crate::session::selection::acquire_selection_operation_turn()
            .await
            .expect("writer turn");
        let _ = bench_batch_locked(serial, &bench, &options, deadline_at, async {}).await;
        panic!("the drain expiry returned instead of aborting");
    }

    let child = std::process::Command::new(std::env::current_exe().expect("test exe"))
        .args([
            "--exact",
            BENCH_ABORT_CHILD_TEST,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(BENCH_ABORT_CHILD_MARKER, "1")
        .env_remove("AC_SELECTION_BENCH")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the child");
    // Bounded wait: kill and reap on expiry; pipes are read on their own
    // threads so a full pipe never blocks the child.
    let (status, stdout, stderr) = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut child = child;
        let mut out = child.stdout.take().expect("stdout");
        let mut err = child.stderr.take().expect("stderr");
        let out = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = out.read_to_end(&mut bytes);
            bytes
        });
        let err = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = err.read_to_end(&mut bytes);
            bytes
        });
        let limit = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll the child") {
                break Some(status);
            }
            if std::time::Instant::now() >= limit {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let stdout = String::from_utf8_lossy(&out.join().expect("stdout reader")).to_string();
        let stderr = String::from_utf8_lossy(&err.join().expect("stderr reader")).to_string();
        (status, stdout, stderr)
    })
    .await
    .expect("join the waiter");
    let status = status.unwrap_or_else(|| {
        panic!("the child did not terminate in time\nstdout:\n{stdout}\nstderr:\n{stderr}")
    });

    // Clean the known retained fixture before asserting.
    let retained = stderr
        .lines()
        .find_map(|line| {
            line.split_once("retained fixture ")
                .map(|(_, path)| path.trim())
        })
        .map(PathBuf::from);
    if let Some(retained) = &retained {
        let _ = std::fs::remove_dir_all(retained);
    }

    assert!(
        stdout.contains("running 1 test") && stdout.contains(BENCH_ABORT_CHILD_TEST),
        "the child ran exactly the terminal test:\n{stdout}"
    );
    assert!(!status.success(), "an abort is never a success: {status:?}");
    #[cfg(windows)]
    assert_eq!(
        status.code(),
        Some(0xC000_0409_u32 as i32),
        "abort exit code, not a panic or another crash: {status:?}\n{stderr}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(6), "SIGABRT: {status:?}\n{stderr}");
    }
    assert!(
        stderr.contains("bench drain expired: pending invocations"),
        "diagnostic with the pending invocations:\n{stderr}"
    );
    assert!(
        retained.is_some(),
        "diagnostic names the retained fixture:\n{stderr}"
    );
}

/// Step 7.6 test 5: acquisition and work share ONE deadline. A third
/// party holds the serialization for 0.6 x budget; the late batch must
/// still expire against the original `deadline_at`. Only an attempt where
/// a FOREIGN test got the lock first is retried; the arithmetic is
/// asserted on the first clean attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_one_budget_is_shared_between_acquisition_and_work() {
    let budget = std::time::Duration::from_millis(400);
    let grace = std::time::Duration::from_secs(10);
    // A reset budget would expire ~0.6 x budget later than allowed; this
    // tolerance stays well below that.
    let tolerance = budget / 4;
    let hold = budget.mul_f64(0.6);
    let bench = bench_fixture(1, 1);
    for attempt in 1..=5 {
        let expired = Arc::new(tokio::sync::Notify::new());
        let milestones = Arc::new(std::sync::Mutex::new(BenchMilestones::default()));
        let options = BenchOptions {
            budget: Some(budget),
            grace: Some(grace),
            drain_expiry: DrainExpiryAction::Record,
            on_deadline: Some(Arc::clone(&expired)),
            milestones: Some(Arc::clone(&milestones)),
            ..BenchOptions::default()
        };
        let third_party = acquire_bench_serialization(
            std::time::Instant::now() + std::time::Duration::from_secs(60),
        )
        .await
        .expect("third party serialization");
        let writer = crate::session::selection::acquire_selection_operation_turn()
            .await
            .expect("writer turn");
        let wake_releaser = Arc::clone(&expired);
        let releaser = tokio::spawn(async move {
            // Bounded: the writer turn is process-wide and must come back.
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(60), expired.notified()).await;
            drop(writer);
        });
        let (result, third_released) =
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                tokio::join!(bench_batch_with(&bench, options), async move {
                    tokio::time::sleep(hold).await;
                    let released = std::time::Instant::now();
                    drop(third_party);
                    released
                })
            })
            .await
            .expect("the test itself is bounded");
        wake_releaser.notify_one();
        releaser.await.expect("releaser");
        let error = match result {
            Err(BenchError::Failed(error)) => error,
            Err(BenchError::DrainExpired(mut expired)) => {
                expired.drain().await;
                panic!("unexpected drain expiry: {}", expired.diagnostic);
            }
            Ok(_) => panic!("the batch must fail on its budget"),
        };
        if error == "bench serialization lock not acquired in time" {
            // A foreign test was queued ahead of this batch.
            println!("[bench] budget test attempt {attempt}: foreign lock holder, retrying");
            continue;
        }
        let m = *milestones.lock().expect("milestones");
        let deadline_at = m.deadline_at.expect("deadline_at");
        let acquired = m.acquired.expect("acquired");
        let observed = m.expired_observed.expect("expiry observed");
        assert!(error.contains("timed out"), "{error}");
        assert!(acquired >= third_released, "the batch acquired late");
        assert!(observed >= deadline_at, "never expires early");
        assert!(
            observed - acquired <= budget.mul_f64(0.4) + tolerance,
            "expired {:?} after a late acquisition; a shared deadline allows {:?}",
            observed - acquired,
            budget.mul_f64(0.4) + tolerance
        );
        assert_eq!(
            m.grace_at,
            Some(deadline_at + grace),
            "grace_at derives from deadline_at"
        );
        return;
    }
    panic!("five attempts in a row lost the lock to a foreign test");
}

/// #2133 (P4) - every test injects `<tempdir>/settings.json`; none resolves the config dir.
mod agent_help_2133;

/// p3 test 8: the p1 batch of 13 on both fixtures, through the read side.
/// Without AC_SELECTION_BENCH it asserts collection and correlation; the
/// latency gates are computed over 5 interleaved serial runs per side
/// (p3 measurement protocol), from the lines printed under the variable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_batch_of_thirteen_reads_drops_queue_wait() {
    for (r, m) in [(8usize, 1usize), (40, 3)] {
        let bench = bench_fixture(r, m);
        let run = bench_batch(&bench).await;
        assert_bench_correlation(&run);
        if bench_measurement_mode() {
            println!(
                "[bench-p3] R={r} M={m} batch_wall_ms={:.1} sum_queue_wait_ms={:.1} \
                occupancy_max={}",
                run.batch_wall_ms,
                bench_sum_queue_wait_ms(&run),
                run.occupancy_max
            );
        }
    }
}

/// p3 test 9: in the batch of 13, read bodies are inside the turn at the
/// same time. The occupancy object belongs to this run only, and is read
/// after all of its work has been awaited. With the old Mutex it is 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_concurrent_read_bodies_occupy_the_turn_simultaneously() {
    let bench = bench_fixture(8, 1);
    let run = bench_batch(&bench).await;
    assert_bench_correlation(&run);
    assert!(
        run.occupancy_max >= 2,
        "read bodies must overlap: occupancy.max() = {}",
        run.occupancy_max
    );
}

mod e2432_p02_source_api_tests;
mod issue_3083_tests;
mod p02_wire_schema_tests;
