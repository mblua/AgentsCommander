use super::*;

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
            "commands::config::tests::room_idle_confirmation::room_idle_confirmation_routes_child",
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
