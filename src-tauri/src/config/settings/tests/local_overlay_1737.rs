use super::super::*;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A base `settings.json` carrying the three fields that have no
/// `#[serde(default)]`, plus a root token so a load does not autogenerate
/// one unless the test wants it to.
pub(super) fn base_fixture() -> Value {
    json!({
        "defaultShell": "test-shell",
        "defaultShellArgs": [],
        "agents": [],
        "rootToken": "base-token",
    })
}

pub(super) fn seed(dir: &Path, base: Option<&Value>, local: Option<&Value>) -> PathBuf {
    let path = dir.join("settings.json");
    if let Some(base) = base {
        std::fs::write(&path, serde_json::to_string_pretty(base).unwrap()).unwrap();
    }
    if let Some(local) = local {
        std::fs::write(
            dir.join("settings.50.personal.no-git.json"),
            serde_json::to_string_pretty(local).unwrap(),
        )
        .unwrap();
    }
    path
}

pub(super) fn disk_object(path: &Path) -> Map<String, Value> {
    let raw = std::fs::read_to_string(path).unwrap();
    match serde_json::from_str::<Value>(&raw).unwrap() {
        Value::Object(object) => object,
        other => panic!("settings.json is not an object: {other}"),
    }
}

/// #1757 - the folder-trust entry exactly as a materialized codex
/// `blockingMenus` array carries it. NOT built from `agent_json`, whose
/// hardcoded empty array would make T10 pass with or without the guard.
pub(super) fn folder_trust_entry() -> Value {
    json!({
        "pattern": r"^\s*Do you trust the contents of this directory\?",
        "notification": "codex is waiting for you to answer the folder-trust menu in this terminal",
        "enabled": true,
        "capturedAgainst": "codex 0.x / Linux",
    })
}

pub(super) fn codex_agent_with(blocking_menus: Value) -> Value {
    json!({
        "id": "codex",
        "label": "Codex",
        "command": "codex",
        "color": "#000000",
        "blockingMenus": blocking_menus,
    })
}

/// The watcher base used by most of the JSON tests: a nested object with a
/// sibling the overlay must not touch.
fn watcher_base() -> Value {
    let mut base = base_fixture();
    base["watchers"] = json!({
        "a": { "mode": "state", "pattern": "^ready", "enabled": true, "dedupeWindowMs": 500 }
    });
    base
}

fn dedupe_window(settings: &AppSettings) -> u64 {
    settings.watchers["a"]
        .valid()
        .expect("valid watcher")
        .dedupe_window_ms
}

// S1
#[test]
fn parse_settings_json_applies_the_overlay_and_records_the_base_values() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(
        temp.path(),
        Some(&watcher_base()),
        Some(&json!({"watchers": {"a": {"dedupeWindowMs": 50}}})),
    );
    let contents = std::fs::read_to_string(&path).unwrap();
    let (settings, _) = parse_settings_json(&contents, "test", Some(&path)).unwrap();

    assert_eq!(dedupe_window(&settings), 50);
    assert!(
        settings.watchers["a"].valid().unwrap().enabled,
        "sibling survives"
    );
    assert_eq!(
        settings.local_overlay_state.owned_paths(),
        &[vec![
            "watchers".to_string(),
            "a".to_string(),
            "dedupeWindowMs".to_string()
        ]]
    );
}

// S2 and S3
#[test]
fn a_preserve_save_restores_the_base_leaf_and_keeps_the_overlay_in_memory() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(
        temp.path(),
        Some(&watcher_base()),
        Some(&json!({"watchers": {"a": {"dedupeWindowMs": 50}}})),
    );
    let mut settings = load_settings_from_path(&path);
    assert_eq!(dedupe_window(&settings), 50);

    settings.gemini_api_key = "unrelated-change".to_string();
    let written = save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    // S2: disk holds the BASE value, and the unrelated change is persisted.
    let object = disk_object(&path);
    assert_eq!(object["watchers"]["a"]["dedupeWindowMs"], json!(500));
    assert_eq!(object["geminiApiKey"], json!("unrelated-change"));
    // S3: memory still holds the OVERLAY value.
    assert_eq!(dedupe_window(&written), 50);
    assert!(!written.local_overlay_state.is_empty());
}

// S4
#[test]
fn deleting_the_local_file_restores_the_pre_override_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(
        temp.path(),
        Some(&watcher_base()),
        Some(&json!({"watchers": {"a": {"dedupeWindowMs": 50}}})),
    );
    let mut settings = load_settings_from_path(&path);
    settings.gemini_api_key = "unrelated-change".to_string();
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    std::fs::remove_file(temp.path().join("settings.50.personal.no-git.json")).unwrap();
    let reloaded = load_settings_from_path(&path);

    let control_temp = tempfile::tempdir().unwrap();
    let control_path = seed(control_temp.path(), Some(&watcher_base()), None);
    let mut control = load_settings_from_path(&control_path);
    control.gemini_api_key = "unrelated-change".to_string();
    save_settings_to_path_preserving_project_paths(&control, &control_path).unwrap();
    let control = load_settings_from_path(&control_path);

    assert_eq!(dedupe_window(&reloaded), dedupe_window(&control));
    assert_eq!(dedupe_window(&reloaded), 500);
    assert_eq!(reloaded.gemini_api_key, control.gemini_api_key);
    assert!(reloaded.local_overlay_state.is_empty());
}

// S5
#[test]
fn the_empty_overlay_is_a_typed_no_op_over_an_arbitrary_object() {
    let object = json!({
        "logLevel": "info",
        "agents": [{"id": "a"}, {"id": "b"}],
        "watchers": {"a": {"intervalMs": 500, "nested": {"deep": true}}},
        "count": 3,
        "flag": false,
    });
    let Value::Object(object) = object else {
        unreachable!()
    };

    let overlay = LocalSettingsOverlay::default();
    assert!(overlay.is_empty());
    assert!(overlay.owned_paths().is_empty());
    assert!(overlay.diagnostics("x").is_empty());

    let mut restored = object.clone();
    overlay.restore_base(&mut restored);
    assert_eq!(restored, object);

    let mut reapplied = object.clone();
    overlay.reapply_from(&object, &mut reapplied);
    assert_eq!(reapplied, object);
}

/// S6's fixture, captured on the pinned base `ac845616` through the
/// delivery-gate-8 materialise-and-revert procedure.
const S6_FIXTURE_JSON: &str = r##"{
  "defaultShell": "gate8-shell",
  "defaultShellArgs": ["-NoLogo", "-NoProfile"],
  "agents": [
    {
      "id": "codex",
      "label": "Codex",
      "command": "codex",
      "color": "#112233"
    },
    {
      "id": "claude",
      "label": "Claude",
      "command": "claude",
      "color": "#445566",
      "blockingMenus": []
    }
  ],
  "webServerPort": 8123,
  "apiServerPort": 8124,
  "rootToken": "gate8-fixed-token",
  "logLevel": "info",
  "activityLogEnabled": true,
  "mainZoom": 1.25,
  "mainGeometry": { "x": 1.0, "y": 2.0, "width": 300.0, "height": 400.0 },
  "mainWindowDisplayState": "normal",
  "watchers": {
    "w1": {
      "mode": "state",
      "pattern": "^ready",
      "enabled": true
    }
  },
  "watchersGeometry": { "x": 10.0, "y": 20.0, "width": 300.0, "height": 400.0 }
}"##;

/// AC-7's control, captured by running `s6_normalized_non_project_settings`
/// on the pinned base `ac845616` BEFORE the first edit of this change
/// (delivery gate 8). Two capture runs produced byte-identical files.
/// #2317 (phase 2) deliberately reverses the shipped absent-key gate
/// rule, so an absent-key fixture now pins `terminalSnapshotsEnabled`
/// true instead of the false the pinned base captured. Explicit-false
/// coverage lives in the dedicated gate tests, not in this control.
/// The six `FIELD_*` project keys and `rootToken` are removed because their
/// values depend on `production_instance_base()` and the filesystem; every
/// remaining key is pinned. The fixture pins `defaultShell`,
/// `defaultShellArgs` and `agents` (no `#[serde(default)]`) and the two
/// profile-aware ports, so the platform this runs on is not load-bearing.
/// #1905: re-captured after phase 3 - the loader no longer materializes
/// `blockingMenus` and the export strips every array (claude's explicit `[]`
/// is pristine and is dropped), so the control carries no `blockingMenus`.
/// #2236: re-captured after adding `sidebarCompactHotkey` (phase 4, #2281),
/// which is always serialized, using `print_s6_normalized_non_project_settings`
/// with two byte-identical JSON captures.
/// #2306 P1: re-captured - the loader finalizes every registered agent to
/// an explicit contiguous `order`, so the control carries codex at 0 and
/// claude at 1.
const EXPECTED_NON_PROJECT_SETTINGS_JSON: &str = r##"{
  "activityLogEnabled": true,
  "agentAutoUpdateByCommand": {},
  "agentGroupKillPrivateBytes": 12884901888,
  "agentGroupWarnPrivateBytes": 8589934592,
  "agentProcessKillPrivateBytes": 12884901888,
  "agentTemplatesPath": null,
  "agents": [
    {
      "color": "#112233",
      "command": "codex",
      "envs": [],
      "id": "codex",
      "isolatedHome": false,
      "label": "Codex",
      "order": 0
    },
    {
      "color": "#445566",
      "command": "claude",
      "envs": [],
      "id": "claude",
      "isolatedHome": false,
      "label": "Claude",
      "order": 1
    }
  ],
  "alwaysShowSelectedWorkgroup": true,
  "apiServerBind": "127.0.0.1",
  "apiServerEnabled": false,
  "apiServerPort": 8124,
  "autoGenerateTaskTitle": true,
  "autoSelfClearByAgent": {},
  "autoSelfClearEnabled": true,
  "branchStalenessEnabled": true,
  "branchStalenessIntervalSecs": 260,
  "branchStalenessNotifyOrchestrator": true,
  "ciActivityEnabled": true,
  "ciActivityNotifyOrchestrator": true,
  "ciSweepMinIntervalSecs": 30,
  "coManagedEnabled": false,
  "codingAgentProfiles": {
    "defaultProfileByAgent": {},
    "identityByAgent": {
      "claude": "57bf1d23533f9c40",
      "codex": "460361e30eb7c7a5"
    },
    "profileLabelsByAgent": {},
    "profileSlots": {
      "A": {
        "label": ""
      }
    },
    "profilesByAgent": {
      "claude": {
        "A": {
          "command": "",
          "enabled": true,
          "env": {},
          "notes": ""
        }
      },
      "codex": {
        "A": {
          "command": "",
          "enabled": true,
          "env": {},
          "notes": ""
        }
      }
    },
    "schemaVersion": 2
  },
  "containerCredentialsFromHost": true,
  "coordSortByActivity": false,
  "coordinatorAutoCloseEnabled": true,
  "coordinatorAutoCloseMinutes": 60,
  "coordinatorAutoCloseSkipTelegramAssigned": false,
  "coordinatorCascadeCloseEnabled": true,
  "coordinatorIdleBadgeRedMinutes": 60,
  "coordinatorIdleBadgeYellowMinutes": 30,
  "darkfactoryZoom": 1.0,
  "defaultShell": "gate8-shell",
  "defaultShellArgs": [
    "-NoLogo",
    "-NoProfile"
  ],
  "geminiApiKey": "",
  "geminiModel": "gemini-2.5-flash",
  "gitSweepConcurrency": 1,
  "gitSweepMinIntervalSecs": 10,
  "guideZoom": 1.0,
  "jevApiKey": "",
  "jevEndpoint": "https://api.typesafe.ai/v1/systemone",
  "jevMargin": 0.15000000596046448,
  "jevModel": "jev-1.13.0",
  "jevThreshold": 0.699999988079071,
  "jevTimeoutSecs": 20,
  "logLevel": "info",
  "mainAlwaysOnTop": false,
  "mainGeometry": {
    "height": 400.0,
    "width": 300.0,
    "x": 1.0,
    "y": 2.0
  },
  "mainResourceMonitorAttached": false,
  "mainSidebarSide": "right",
  "mainSidebarWidth": 240.0,
  "mainWindowDisplayState": "normal",
  "mainZoom": 1.25,
  "maxConcurrentAgentProcesses": 32,
  "menuGuardEnabled": true,
  "npmUpdateNotificationsEnabled": true,
  "onboardingDismissed": false,
  "railCollapsedProjects": [],
  "railFavoritesCollapsed": false,
  "raiseTerminalOnClick": true,
  "remoteAgentHelpEnabled": true,
  "remoteBlockingMenusEnabled": true,
  "resourceBackoffPolling": true,
  "resourceKeepLastSnapshot": true,
  "resourceMonitorEnabled": true,
  "resourceWatchdogAction": "warn",
  "restartResumeAgentPrompt": ".",
  "restartResumeOrchestratorPrompt": "AgentsCommander was restarted. Continue with the work that was in flight.",
  "restartResumeWakeWorkingAgents": false,
  "restoreCoordinatorWakeState": false,
  "roomNumberMask": "#",
  "screenshotCaptureHotkey": "Ctrl+Q",
  "selectedRowRailColor": "#FFFFFF",
  "selectedRowRailWidth": "9px",
  "sidebarAlwaysOnTop": false,
  "sidebarCompactHotkey": "Ctrl+Shift+E",
  "sidebarStyle": "noir-minimal",
  "sidebarZoom": 1.0,
  "soundsEnabled": true,
  "specBoardEnabled": false,
  "teamIdleBeepEnabled": true,
  "telegramBots": [],
  "telegramNetworkPollErrorLogging": {
    "firstFailureLevel": "warn",
    "recoveryLevel": "info",
    "sustainedAfterSeconds": 60,
    "sustainedLevel": "error",
    "sustainedRepeatSeconds": 60,
    "transientRepeatLevel": "debug"
  },
  "terminalSnapshotsEnabled": true,
  "terminalZoom": 1.0,
  "themeLight": false,
  "typingHoldSeconds": 30,
  "voiceAutoExecute": true,
  "voiceAutoExecuteDelay": 15,
  "voiceToTextEnabled": false,
  "watchers": {
    "w1": {
      "dedupe": "row",
      "dedupeWindowMs": 2000,
      "enabled": true,
      "mode": "state",
      "pattern": "^ready"
    }
  },
  "watchersGeometry": {
    "height": 400.0,
    "width": 300.0,
    "x": 10.0,
    "y": 20.0
  },
  "webServerBind": "127.0.0.1",
  "webServerEnabled": false,
  "webServerPort": 8123
}"##;

fn s6_normalized_non_project_settings() -> String {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, S6_FIXTURE_JSON).unwrap();
    let loaded = load_settings_from_path(&path);
    save_settings_to_path_preserving_project_paths(&loaded, &path).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let mut value: Value = serde_json::from_str(&raw).unwrap();
    let object = value.as_object_mut().unwrap();
    // #2716 (B3) 6.1: the two groups live in the agents file; a missing or
    // unparseable one panics, so the control never goes vacuous on them.
    let sidecar_raw = std::fs::read_to_string(super::super::agents_instance_path(&path))
        .expect("S6: the agents file is missing");
    let sidecar: Map<String, Value> =
        serde_json::from_str(&sidecar_raw).expect("S6: the agents file does not parse");
    for key in super::super::AGENTS_INSTANCE_KEYS {
        let group = sidecar
            .get(key)
            .unwrap_or_else(|| panic!("S6: the agents file does not hold {key}"));
        object.insert(key.to_string(), group.clone());
    }
    for key in [
        FIELD_PROJECT_PATH,
        FIELD_PROJECT_PATH_REL,
        FIELD_PROJECT_PATHS,
        FIELD_PROJECT_PATHS_REL,
        FIELD_ARCHIVED,
        FIELD_ARCHIVED_REL,
        "rootToken",
    ] {
        object.remove(key);
    }
    let sorted: BTreeMap<String, Value> =
        object.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    serde_json::to_string_pretty(&sorted).unwrap()
}

/// #2236: prints the S6 control so the fixture can be re-captured without a
/// scratch edit. Ignored by default; it asserts nothing.
#[test]
#[ignore = "capture helper: run with --ignored to re-capture the fixture"]
fn print_s6_normalized_non_project_settings() {
    println!("{}", s6_normalized_non_project_settings());
}

// S6
#[test]
fn a_no_overlay_save_writes_the_control_captured_on_the_pinned_base() {
    let mut expected: Value = serde_json::from_str(EXPECTED_NON_PROJECT_SETTINGS_JSON).unwrap();
    let object = expected.as_object_mut().unwrap();
    assert!(!object.contains_key("codingAgentInstallEnabled"));
    // #2800 extends the schema; the historical captured control stays intact.
    object.insert("codingAgentInstallEnabled".to_string(), Value::Bool(false));
    assert!(!object.contains_key("containerOptionsEnabled"));
    // #3062 adds a default-false field without changing the historical control.
    object.insert("containerOptionsEnabled".to_string(), Value::Bool(false));
    assert!(!object.contains_key("responseCloseEnabled"));
    assert!(!object.contains_key("responseCloseIdleSeconds"));
    object.insert("responseCloseEnabled".to_string(), Value::Bool(true));
    object.insert("responseCloseIdleSeconds".to_string(), Value::from(30));
    assert!(!object.contains_key("roomIdleConfirmationSeconds"));
    // #3123 adds a default-15 field; preserve the historical captured control.
    object.insert("roomIdleConfirmationSeconds".to_string(), Value::from(15));
    // H02 adds backend-derived metadata; never rewrite the frozen control.
    for raw in object.get_mut("agents").unwrap().as_array_mut().unwrap() {
        assert!(raw.get("id_hash").is_none());
        let agent: AgentConfig = serde_json::from_value(raw.clone()).unwrap();
        raw.as_object_mut().unwrap().insert(
            "id_hash".to_string(),
            Value::String(configuration_identity(&agent.command, &agent.envs)),
        );
        let mut leaves = vec![agent.command.clone()];
        for env in &agent.envs {
            leaves.extend([env.key.clone(), env.value.clone()]);
        }
        raw["id_hash_debug"] = json!(leaves);
        // The physical legacy agent text survives the canonical write.
        raw["baseCommand"] = raw["command"].clone();
    }
    // S6 never seeded profile cells. Their generated defaults are fresh
    // canonical rows and must not acquire invented historical fields.
    // Independent ASCII preimages: empty cell/env means the base text.
    let pair_hashes = BTreeMap::from([
        (
            "claude",
            "cid1:15a3fc2b7f142361454b99d62429610d12f5f7aedb12b8500a2bfa32d379837d",
        ),
        (
            "codex",
            "cid1:2cf586be28b9573af75bfbdadfe0bd8760e3382b547cfe2c527eea368920a8e5",
        ),
    ]);
    let profiles = object
        .get_mut("codingAgentProfiles")
        .unwrap()
        .as_object_mut()
        .unwrap();
    let cells = profiles.remove("profilesByAgent").unwrap();
    let labels = profiles.remove("profileLabelsByAgent").unwrap();
    profiles.remove("identityByAgent");
    let mut by_hash = Map::new();
    let mut labels_by_hash = Map::new();
    for (id, cells) in cells.as_object().unwrap() {
        let mut cells = cells.clone();
        for cell in cells.as_object_mut().unwrap().values_mut() {
            let command = cell.as_object_mut().unwrap().remove("command").unwrap();
            cell["additionalArguments"] = command;
            assert!(cell.get("id_hash").is_none());
        }
        by_hash.insert(pair_hashes[id.as_str()].to_string(), cells);
        labels_by_hash.insert(
            pair_hashes[id.as_str()].to_string(),
            labels.get(id).cloned().unwrap_or_else(|| json!({})),
        );
    }
    profiles.insert("profileBindingVersion".into(), json!(1));
    profiles.insert("profilesByBaseHash".into(), Value::Object(by_hash));
    profiles.insert(
        "profileLabelsByBaseHash".into(),
        Value::Object(labels_by_hash),
    );
    let sorted: BTreeMap<String, Value> =
        object.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let expected = serde_json::to_string_pretty(&sorted).unwrap();
    assert_eq!(s6_normalized_non_project_settings(), expected);
}

// S7
#[test]
fn an_overlay_key_absent_from_the_base_never_reaches_settings_json() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(
        temp.path(),
        Some(&base_fixture()),
        Some(&json!({"watchersGeometry": {"x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0}})),
    );
    let settings = load_settings_from_path(&path);
    assert!(settings.watchers_geometry.is_some());
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert!(
        disk_object(&path).get("watchersGeometry").is_none(),
        "the base file must never gain a key it did not have"
    );
}

// S8
#[test]
fn the_startup_root_token_write_copies_no_overlay_value() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base.as_object_mut().unwrap().remove("rootToken");
    base["logLevel"] = json!("info");
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"logLevel": "trace"})),
    );

    let settings = load_settings_from_path(&path);

    assert_eq!(settings.log_level.as_deref(), Some("trace"));
    assert!(!settings.local_overlay_state.is_empty());
    let object = disk_object(&path);
    assert_eq!(object["logLevel"], json!("info"));
    assert!(object.get("rootToken").is_some(), "the token was generated");
}

// S9
#[test]
fn the_disk_ineligible_table_is_pinned_by_equality() {
    assert_eq!(
        OVERLAY_INELIGIBLE_DISK_KEYS,
        &[
            "agentFileSourceByMenu",
            "archivedProjectPaths",
            "archivedProjectPathsRelativeToInstance",
            "projectPath",
            "projectPathRelativeToInstance",
            "projectPaths",
            "projectPathsRelativeToInstance",
            "rootToken",
            "terminalSnapshotsEnabled",
        ]
    );
    assert_eq!(OVERLAY_INELIGIBLE_DISK_KEYS[0], "agentFileSourceByMenu");
    assert_eq!(OVERLAY_INELIGIBLE_DISK_KEYS[1], FIELD_ARCHIVED);
    assert_eq!(OVERLAY_INELIGIBLE_DISK_KEYS[2], FIELD_ARCHIVED_REL);
    assert_eq!(OVERLAY_INELIGIBLE_DISK_KEYS[3], FIELD_PROJECT_PATH);
    assert_eq!(OVERLAY_INELIGIBLE_DISK_KEYS[4], FIELD_PROJECT_PATH_REL);
    assert_eq!(OVERLAY_INELIGIBLE_DISK_KEYS[5], FIELD_PROJECT_PATHS);
    assert_eq!(OVERLAY_INELIGIBLE_DISK_KEYS[6], FIELD_PROJECT_PATHS_REL);
    assert_eq!(OVERLAY_INELIGIBLE_DISK_KEYS[7], "rootToken");
    assert_eq!(
        OVERLAY_INELIGIBLE_DISK_KEYS[8],
        FIELD_TERMINAL_SNAPSHOTS_ENABLED
    );
    // `rootToken` is the serialized name of the field it protects.
    let with_token = AppSettings {
        root_token: Some("t".to_string()),
        agent_file_source_by_menu: Some(AgentFileSourcePreferences::default()),
        ..AppSettings::default()
    };
    let Value::Object(object) = serde_json::to_value(&with_token).unwrap() else {
        unreachable!()
    };
    assert!(object.contains_key("rootToken"));
    assert!(object.contains_key("agentFileSourceByMenu"));
}

// S10
#[test]
fn the_legacy_ineligible_table_is_pinned_to_serialized_field_names() {
    assert_eq!(
        OVERLAY_INELIGIBLE_LEGACY_KEYS,
        &[
            "agentFileSourceByMenu",
            "sidebarAlwaysOnTop",
            "sidebarZoom",
            "startOnlyCoordinators",
            "terminalGeometry",
        ]
    );
    let settings = AppSettings {
        legacy_start_only_coordinators: Some(true),
        agent_file_source_by_menu: Some(AgentFileSourcePreferences::default()),
        terminal_geometry: Some(WindowGeometry {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }),
        ..AppSettings::default()
    };
    let Value::Object(object) = serde_json::to_value(&settings).unwrap() else {
        unreachable!()
    };
    for key in OVERLAY_INELIGIBLE_LEGACY_KEYS {
        assert!(
            object.contains_key(*key),
            "{key} is not a serialized AppSettings field"
        );
    }
}

// S11
#[test]
fn the_derived_id_closure_table_is_pinned_to_serialized_names() {
    assert_eq!(OVERLAY_DERIVED_ID_CLOSURES.len(), 2);
    let closure = &OVERLAY_DERIVED_ID_CLOSURES[0];
    assert_eq!(closure.source_key, "agents");
    assert_eq!(closure.id_field, "id");
    assert_eq!(
        closure.derived_prefix,
        &["codingAgentProfiles", "profilesByAgent"]
    );

    let mut settings = AppSettings {
        agents: vec![serde_json::from_value::<AgentConfig>(agent_json("probe")).unwrap()],
        ..AppSettings::default()
    };
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .insert("probe".to_string(), BTreeMap::new());
    let Value::Object(object) = serde_json::to_value(&settings).unwrap() else {
        unreachable!()
    };
    assert!(object["agents"][0]
        .as_object()
        .unwrap()
        .contains_key(closure.id_field));
    assert!(object[closure.source_key].is_array());
    assert!(object["codingAgentProfiles"]
        .as_object()
        .unwrap()
        .contains_key("profilesByAgent"));
}

// S12
#[test]
fn an_overlay_naming_root_token_is_inert_in_memory_and_on_disk() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(
        temp.path(),
        Some(&base_fixture()),
        Some(&json!({"rootToken": "override-token"})),
    );
    let settings = load_settings_from_path(&path);
    assert_eq!(settings.root_token.as_deref(), Some("base-token"));
    assert!(settings.local_overlay_state.is_empty());
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(disk_object(&path)["rootToken"], json!("base-token"));
}

fn agent_json(id: &str) -> Value {
    json!({
        "id": id,
        "label": id,
        "command": id,
        "color": "#000000",
        "blockingMenus": [],
    })
}

// S13
#[test]
fn a_repair_driven_by_an_overlay_agent_stays_out_of_the_base_file() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["agents"] = json!([agent_json("codex")]);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"agents": [agent_json("codex"), agent_json("scratch-agent")]})),
    );

    let settings = load_settings_from_path(&path);
    assert!(settings
        .coding_agent_profiles
        .profiles_by_agent
        .contains_key("scratch-agent"));
    let object = disk_object(&path);
    assert!(
        object["codingAgentProfiles"]["profilesByAgent"]
            .get("scratch-agent")
            .is_none(),
        "the overlay-introduced agent's cells must not reach the base file"
    );

    std::fs::remove_file(temp.path().join("settings.50.personal.no-git.json")).unwrap();
    let reloaded = load_settings_from_path(&path);
    assert!(!reloaded
        .coding_agent_profiles
        .profiles_by_agent
        .contains_key("scratch-agent"));
    assert_eq!(reloaded.agents.len(), 1);
}

// S14
#[test]
fn the_save_time_repair_also_stays_out_of_the_base_file() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["agents"] = json!([agent_json("codex")]);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"agents": [agent_json("codex"), agent_json("scratch-agent")]})),
    );
    let mut settings = load_settings_from_path(&path);
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .remove("scratch-agent");
    validate_and_repair_settings(&mut settings).unwrap();
    assert!(settings
        .coding_agent_profiles
        .profiles_by_agent
        .contains_key("scratch-agent"));

    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert!(disk_object(&path)["codingAgentProfiles"]["profilesByAgent"]
        .get("scratch-agent")
        .is_none());
}

// S15
#[test]
fn the_cas_early_return_carries_both_the_overlay_state_and_its_values() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = watcher_base();
    base["terminalSnapshotsEnabled"] = json!(false);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"watchers": {"a": {"dedupeWindowMs": 50}}})),
    );
    let current = load_settings_from_path(&path);
    assert_eq!(dedupe_window(&current), 50);

    let candidate =
        compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, false, false).unwrap();
    assert!(!candidate.local_overlay_state.is_empty());
    assert_eq!(dedupe_window(&candidate), 50);
}

// S16
#[test]
fn the_cas_writing_branch_keeps_the_overlay_and_writes_base_leaves() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = watcher_base();
    base["terminalSnapshotsEnabled"] = json!(false);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"watchers": {"a": {"dedupeWindowMs": 50}}})),
    );
    let current = load_settings_from_path(&path);

    let written =
        compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, false, true).unwrap();
    assert!(written.terminal_snapshots_enabled);
    assert!(!written.local_overlay_state.is_empty());
    assert_eq!(dedupe_window(&written), 50);
    let object = disk_object(&path);
    assert_eq!(object["watchers"]["a"]["dedupeWindowMs"], json!(500));
    assert_eq!(object["terminalSnapshotsEnabled"], json!(true));
}

// S17
#[test]
fn reconcile_with_the_file_present_is_an_idempotent_restore() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(
        temp.path(),
        Some(&watcher_base()),
        Some(&json!({"watchers": {"a": {"dedupeWindowMs": 50}}})),
    );
    let mut settings = load_settings_from_path(&path);
    settings.project_paths = vec![temp.path().to_string_lossy().to_string()];

    save_settings_with_project_paths_to_path(&settings, &path).unwrap();
    assert_eq!(
        disk_object(&path)["watchers"]["a"]["dedupeWindowMs"],
        json!(500)
    );

    let written = reconcile_project_state_to_path(&settings, &path, true, true).unwrap();
    assert_eq!(dedupe_window(&written), 50);
    assert!(!written.local_overlay_state.is_empty());
}

// S18
#[test]
fn reconcile_with_the_file_absent_writes_no_overlay_value() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(
        temp.path(),
        Some(&watcher_base()),
        Some(&json!({"watchers": {"a": {"dedupeWindowMs": 50}}})),
    );
    let settings = load_settings_from_path(&path);
    std::fs::remove_file(&path).unwrap();

    save_settings_with_project_paths_to_path(&settings, &path).unwrap();
    assert_eq!(
                disk_object(&path)["watchers"]["a"]["dedupeWindowMs"],
                json!(500),
                "the absent-file reconcile arm seeds `out` from the live settings, so restore_base is load-bearing"
            );
}

// S19
#[test]
fn an_absent_base_still_applies_the_overlay_over_the_serialized_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(temp.path(), None, Some(&json!({"logLevel": "trace"})));
    assert!(!path.exists());

    let settings = load_settings_from_path(&path);
    assert_eq!(settings.log_level.as_deref(), Some("trace"));
    assert!(!settings.local_overlay_state.is_empty());
    let object = disk_object(&path);
    assert_eq!(
        object.get("logLevel"),
        Some(&Value::Null),
        "the created file holds the DEFAULT value, not the override"
    );

    std::fs::remove_file(temp.path().join("settings.50.personal.no-git.json")).unwrap();
    let reloaded = load_settings_from_path(&path);
    assert_eq!(reloaded.log_level, None);
}

// S20
#[test]
fn an_unparseable_base_still_applies_the_overlay_over_the_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, "{ not json").unwrap();
    std::fs::write(
        temp.path().join("settings.50.personal.no-git.json"),
        r#"{"logLevel": "trace"}"#,
    )
    .unwrap();

    let settings = load_settings_from_path(&path);
    assert_eq!(settings.log_level.as_deref(), Some("trace"));
    assert!(!settings.local_overlay_state.is_empty());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{ not json",
        "the preserve writer's disk gate refuses to overwrite a present-but-invalid file"
    );
}

// S21
#[test]
fn the_routed_readers_serve_the_merged_value() {
    let temp = tempfile::tempdir().unwrap();

    // Neither file sets the key.
    let path = seed(temp.path(), Some(&base_fixture()), None);
    assert_eq!(read_log_level_from_path(&path), None);
    assert!(!read_activity_log_enabled_from_path(&path));

    // The base sets it; there is no overlay.
    let mut base = base_fixture();
    base["logLevel"] = json!("info");
    base["activityLogEnabled"] = json!(true);
    let path = seed(temp.path(), Some(&base), None);
    assert_eq!(read_log_level_from_path(&path), Some("info".to_string()));
    assert!(read_activity_log_enabled_from_path(&path));

    // The overlay wins.
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"logLevel": "trace", "activityLogEnabled": false})),
    );
    assert_eq!(read_log_level_from_path(&path), Some("trace".to_string()));
    assert!(!read_activity_log_enabled_from_path(&path));
}

// S22
#[test]
fn a_routed_reader_serves_the_overlay_over_an_unparseable_base() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, "{ not json").unwrap();
    std::fs::write(
        temp.path().join("settings.50.personal.no-git.json"),
        r#"{"logLevel": "trace", "activityLogEnabled": true}"#,
    )
    .unwrap();
    assert_eq!(read_log_level_from_path(&path), Some("trace".to_string()));
    assert!(read_activity_log_enabled_from_path(&path));
}

// S23
#[test]
fn a_wrong_typed_overlay_value_falls_back_to_the_base_and_not_to_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["mainZoom"] = json!(1.75);
    base["geminiApiKey"] = json!("base-key");
    let path = seed(temp.path(), Some(&base), Some(&json!({"mainZoom": "big"})));

    let contents = std::fs::read_to_string(&path).unwrap();
    let (settings, _) = parse_settings_json(&contents, "test", Some(&path)).unwrap();

    assert_eq!(settings.main_zoom, 1.75, "the base value, not the default");
    assert_eq!(settings.gemini_api_key, "base-key");
    assert!(settings.local_overlay_state.is_empty());
    assert!(matches!(
        settings.local_overlay_state.rejection(),
        Some(crate::config::local_overlay::OverlayRejection::MergedValueUndecodable(_))
    ));
}

// S25
#[test]
fn the_five_json_side_diagnostic_records_are_asserted_through_the_typed_state() {
    use crate::config::local_overlay::{OverlayDiagnostic, OverlayDiagnosticLevel};
    let temp = tempfile::tempdir().unwrap();

    // 1. Rejection.
    let path = seed(temp.path(), Some(&base_fixture()), None);
    std::fs::write(
        temp.path().join("settings.50.personal.no-git.json"),
        "{ not json",
    )
    .unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    let (settings, _) = parse_settings_json(&contents, "test", Some(&path)).unwrap();
    let records = settings.local_overlay_state.diagnostics("local");
    assert!(records
        .iter()
        .any(|r| matches!(r, OverlayDiagnostic::Rejected { .. })));

    // 2 and 3. Both ineligible rules, plus 5, applied.
    let path = seed(
        temp.path(),
        Some(&base_fixture()),
        Some(&json!({
            "rootToken": "x",
            "sidebarZoom": 2.0,
            "logLevel": "trace",
        })),
    );
    let contents = std::fs::read_to_string(&path).unwrap();
    let (settings, _) = parse_settings_json(&contents, "test", Some(&path)).unwrap();
    let records = settings.local_overlay_state.diagnostics("local");
    let rules: Vec<&str> = records
        .iter()
        .filter_map(|r| match r {
            OverlayDiagnostic::IneligibleKeyDropped { rule, .. } => Some(*rule),
            _ => None,
        })
        .collect();
    assert_eq!(
        rules,
        vec![
            crate::config::local_overlay::RULE_DISK_AUTHORITATIVE,
            crate::config::local_overlay::RULE_LEGACY_MIGRATION_SOURCE,
        ]
    );
    let applied: Vec<&OverlayDiagnostic> = records
        .iter()
        .filter(|r| matches!(r, OverlayDiagnostic::Applied { .. }))
        .collect();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].level(), OverlayDiagnosticLevel::Info);
    for record in &records {
        if !matches!(record, OverlayDiagnostic::Applied { .. }) {
            assert_eq!(record.level(), OverlayDiagnosticLevel::Error);
        }
    }

    // 4. Undecodable merge.
    let mut base = base_fixture();
    base["mainZoom"] = json!(1.75);
    let path = seed(temp.path(), Some(&base), Some(&json!({"mainZoom": "big"})));
    let contents = std::fs::read_to_string(&path).unwrap();
    let (settings, _) = parse_settings_json(&contents, "test", Some(&path)).unwrap();
    let records = settings.local_overlay_state.diagnostics("local");
    assert!(records.iter().any(|r| matches!(
        r,
        OverlayDiagnostic::Rejected {
            rejection: crate::config::local_overlay::OverlayRejection::MergedValueUndecodable(_),
            ..
        }
    )));
}

/// The set of top-level keys added, removed, or changed between two objects.
fn mutated_keys(before: &Map<String, Value>, after: &Map<String, Value>) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for (key, value) in before {
        match after.get(key) {
            Some(other) if other == value => {}
            _ => {
                keys.insert(key.clone());
            }
        }
    }
    for key in after.keys() {
        if !before.contains_key(key) {
            keys.insert(key.clone());
        }
    }
    keys
}

fn object_of(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object,
        other => panic!("not an object: {other}"),
    }
}

// S26
#[test]
fn the_premigration_unsafe_set_is_derived_from_the_project_decode() {
    // Every project row is written by the decode: the two active keys are
    // rebuilt from the selected pairs (an unresolvable registration selects
    // nothing), the archived key is rebuilt from the archived pairs (a row
    // that carries no usable absolute string contributes nothing), and the
    // three companions are removed outright. An instance base is passed
    // explicitly, so the test never depends on `production_instance_base()`.
    let temp = tempfile::tempdir().unwrap();
    let instance_base = temp.path();

    let mut value = json!({
        "logLevel": "info",
        "activityLogEnabled": true,
        "agents": [{"id": "codex", "label": "Codex", "command": "codex", "color": "#000"}],
        "projectPath": "no-such-project-directory",
        "projectPathRelativeToInstance": "a",
        "projectPaths": ["no-such-project-directory"],
        "projectPathsRelativeToInstance": ["a"],
        "archivedProjectPaths": [null],
        "archivedProjectPathsRelativeToInstance": ["b"],
    });
    let before = object_of(value.clone());

    let _ = apply_project_decode_to_value(
        &mut value,
        Some(instance_base),
        &projects::FsCandidateResolver,
    );
    let after = object_of(value);

    let mutated = mutated_keys(&before, &after);
    let expected: BTreeSet<String> = OVERLAY_PREMIGRATION_UNSAFE_KEYS
        .iter()
        .map(|key| (*key).to_string())
        .collect();
    // Stated limit: this observes one fixture, not all inputs. It is the
    // tripwire that fires if the decode starts writing a key the D16
    // shortcut does not list; it is not a proof.
    assert_eq!(mutated, expected);
}

// S27
#[test]
fn the_load_time_chain_mutates_exactly_the_pinned_key_set() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");

    let mut fixture = object_of(serde_json::to_value(AppSettings::default()).unwrap());
    fixture.remove("rootToken");
    fixture.insert(
        FIELD_TERMINAL_SNAPSHOTS_ENABLED.to_string(),
        Value::Bool(false),
    );
    fixture.insert(
        "terminalGeometry".to_string(),
        json!({"x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0}),
    );
    fixture.insert("sidebarZoom".to_string(), json!(1.5));
    fixture.insert("sidebarAlwaysOnTop".to_string(), json!(true));
    fixture.insert("startOnlyCoordinators".to_string(), json!(true));
    // #2190: pinned explicitly so the legacy carrier still flips the
    // destination (false -> true) whatever the struct default is.
    fixture.insert("restoreCoordinatorWakeState".to_string(), json!(false));
    fixture.insert(
        "agents".to_string(),
        json!([{"id": "codex", "label": "Codex", "command": "codex", "color": "#000000"}]),
    );
    fixture.insert(
        "codingAgentProfiles".to_string(),
        json!({"schemaVersion": 2, "profileSlots": {}, "profilesByAgent": {}}),
    );
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&Value::Object(fixture.clone())).unwrap(),
    )
    .unwrap();

    let loaded = load_settings_from_path(&path);
    let after = object_of(serde_json::to_value(&loaded).unwrap());

    let mut mutated = mutated_keys(&fixture, &after);
    for key in [
        FIELD_PROJECT_PATH,
        FIELD_PROJECT_PATH_REL,
        FIELD_PROJECT_PATHS,
        FIELD_PROJECT_PATHS_REL,
        FIELD_ARCHIVED,
        FIELD_ARCHIVED_REL,
    ] {
        mutated.remove(key);
    }

    let expected: BTreeSet<String> = [
        "agents",
        "codingAgentProfiles",
        "mainAlwaysOnTop",
        "mainGeometry",
        "mainZoom",
        "restoreCoordinatorWakeState",
        "rootToken",
        "startOnlyCoordinators",
    ]
    .iter()
    .map(|key| (*key).to_string())
    .collect();
    assert_eq!(mutated, expected);
}

// S28
#[test]
fn the_save_time_repair_chain_mutates_exactly_the_pinned_key_set() {
    let mut settings = AppSettings {
        agents: vec![serde_json::from_value::<AgentConfig>(agent_json("codex")).unwrap()],
        api_server_bind: "  127.0.0.1  ".to_string(),
        ..AppSettings::default()
    };
    // Dirty `agents`: a container image with surrounding whitespace is what
    // `normalize_agent_backend_configs` rewrites.
    settings.agents[0].backend.kind = SessionBackendKind::ContainerTransport;
    settings.agents[0].backend.image = Some("  example/image:tag  ".to_string());
    settings.coding_agent_profiles.schema_version = 1;
    let before = object_of(serde_json::to_value(&settings).unwrap());

    validate_and_repair_settings(&mut settings).unwrap();
    let after = object_of(serde_json::to_value(&settings).unwrap());

    let expected: BTreeSet<String> = ["agents", "codingAgentProfiles", "apiServerBind"]
        .iter()
        .map(|key| (*key).to_string())
        .collect();
    assert_eq!(mutated_keys(&before, &after), expected);
}

// S29
#[test]
fn the_migration_destination_table_is_pinned_to_serialized_field_names() {
    assert_eq!(
        OVERLAY_MIGRATION_DESTINATION_KEYS,
        &[
            "agents",
            "mainAlwaysOnTop",
            "mainGeometry",
            "mainZoom",
            "restoreCoordinatorWakeState",
        ]
    );
    assert_eq!(OVERLAY_MIGRATION_DESTINATION_KEYS[0], OVERLAY_KEY_AGENTS);
    assert_eq!(
        OVERLAY_MIGRATION_DESTINATION_KEYS[1],
        OVERLAY_KEY_MAIN_ALWAYS_ON_TOP
    );
    assert_eq!(
        OVERLAY_MIGRATION_DESTINATION_KEYS[2],
        OVERLAY_KEY_MAIN_GEOMETRY
    );
    assert_eq!(OVERLAY_MIGRATION_DESTINATION_KEYS[3], OVERLAY_KEY_MAIN_ZOOM);
    assert_eq!(
        OVERLAY_MIGRATION_DESTINATION_KEYS[4],
        OVERLAY_KEY_RESTORE_COORDINATOR_WAKE_STATE
    );

    let settings = AppSettings {
        main_geometry: Some(WindowGeometry {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }),
        restore_coordinator_wake_state: true,
        ..AppSettings::default()
    };
    let object = object_of(serde_json::to_value(&settings).unwrap());
    for key in OVERLAY_MIGRATION_DESTINATION_KEYS {
        assert!(
            object.contains_key(*key),
            "{key} is not a serialized AppSettings field"
        );
    }
}

// #1757 (D11/D12) - an overlay that owns `agents` suppresses the migration.
#[test]
fn an_overlay_owned_agents_array_suppresses_the_1757_migration() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["agents"] = json!([codex_agent_with(json!([folder_trust_entry()]))]);
    let path = seed(
        temp.path(),
        Some(&base),
        // The overlay's array MUST be the non-empty one-entry shape: that is
        // precisely what the migration would act on if the guard were absent.
        Some(&json!({"agents": [codex_agent_with(json!([folder_trust_entry()]))]})),
    );

    let settings = load_settings_from_path(&path);
    assert!(
        settings
            .local_overlay_state
            .owns_top_level(OVERLAY_KEY_AGENTS),
        "the fixture stopped producing an overlay-owned agents array"
    );
    // #1905: the overlay-owned array is not exported (one info! per agent), so
    // no blocking-menus `.local` file appears and layer 0 keeps serving the
    // overlay's one-entry array rather than the shipped two.
    assert!(!blocking_menus_local_path(&path).exists());
    assert_eq!(
        BlockingMenusStore::load_from_settings_path(&path)
            .resolve_for(&settings.agents[0])
            .len(),
        1
    );

    fn in_memory(settings: &AppSettings) -> usize {
        settings
            .agents
            .iter()
            .find(|a| a.id == "codex")
            .expect("codex agent")
            .blocking_menus
            .as_ref()
            .expect("blocking_menus present")
            .len()
    }
    fn on_disk(path: &Path) -> usize {
        let object = disk_object(path);
        object["agents"][0]["blockingMenus"]
            .as_array()
            .expect("blockingMenus array")
            .len()
    }

    // Deleting the owns_top_level(OVERLAY_KEY_AGENTS) early return makes this
    // in-memory number 2. The on-disk number is 1 either way, because
    // restore_base writes the captured base array back on every save.
    assert_eq!(
        in_memory(&settings),
        1,
        "the migration must not touch an overlay-owned agents array in memory"
    );
    assert_eq!(on_disk(&path), 1, "the base file must stay clean");

    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    assert_eq!(in_memory(&settings), 1);
    assert_eq!(on_disk(&path), 1, "restore_base must keep the base array");
}

// S30
#[test]
fn the_default_arms_render_their_diagnostics_before_returning_defaults() {
    use crate::config::local_overlay::OverlayDiagnostic;

    let cases: [(&str, Option<&str>); 4] = [
        ("invalid-json", Some("{ not json")),
        ("top-level-array", Some("[1, 2]")),
        ("unreadable", None), // a directory at the overlay path
        (
            "all-ineligible",
            Some(r#"{"rootToken": "x", "projectPaths": []}"#),
        ),
    ];

    for (label, bytes) in cases {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let local = temp.path().join("settings.50.personal.no-git.json");
        match bytes {
            Some(bytes) => std::fs::write(&local, bytes).unwrap(),
            None => std::fs::create_dir(&local).unwrap(),
        }

        let settings = default_settings_with_overlay(&path, "test");
        let control = AppSettings::default();
        assert_eq!(
            serde_json::to_value(&settings).unwrap(),
            serde_json::to_value(&control).unwrap(),
            "{label}: the value returned must equal AppSettings::default()"
        );

        // The diagnostics the arm renders, observed on the overlay it loaded.
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        let overlay = LocalSettingsOverlay::load_and_merge_paired(
            &path,
            &mut value,
            OVERLAY_INELIGIBLE_DISK_KEYS,
            OVERLAY_INELIGIBLE_LEGACY_KEYS,
            OVERLAY_DERIVED_ID_CLOSURES,
            CONFIGURATION_COMMAND_PAIRS,
        );
        let records = overlay.diagnostics("test");
        assert!(!records.is_empty(), "{label}: expected a diagnostic");
        if label == "all-ineligible" {
            assert!(records
                .iter()
                .all(|r| matches!(r, OverlayDiagnostic::IneligibleKeyDropped { .. })));
        } else {
            assert!(records
                .iter()
                .any(|r| matches!(r, OverlayDiagnostic::Rejected { .. })));
        }

        // The same arm through the real loader, with an absent base.
        let loaded = load_settings_from_path(&path);
        assert!(
            loaded.local_overlay_state.is_empty(),
            "{label}: no overlay may be owned"
        );
    }

    // The control: no local file at all renders nothing.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    let overlay = LocalSettingsOverlay::load_and_merge_paired(
        &path,
        &mut value,
        OVERLAY_INELIGIBLE_DISK_KEYS,
        OVERLAY_INELIGIBLE_LEGACY_KEYS,
        OVERLAY_DERIVED_ID_CLOSURES,
        CONFIGURATION_COMMAND_PAIRS,
    );
    assert!(overlay.diagnostics("test").is_empty());
}

// S31
#[test]
fn owning_a_migration_destination_suppresses_the_migration_in_memory() {
    let geometry = json!({"x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0});

    // R2: sidebarZoom 1.5 -> mainZoom, overridden to the default value 1.0.
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["sidebarZoom"] = json!(1.5);
    let path = seed(temp.path(), Some(&base), Some(&json!({"mainZoom": 1.0})));
    assert_eq!(load_settings_from_path(&path).main_zoom, 1.0);
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    assert_eq!(load_settings_from_path(&control).main_zoom, 1.5);

    // R3: sidebarAlwaysOnTop true -> mainAlwaysOnTop, overridden to false.
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["sidebarAlwaysOnTop"] = json!(true);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"mainAlwaysOnTop": false})),
    );
    assert!(!load_settings_from_path(&path).main_always_on_top);
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    assert!(load_settings_from_path(&control).main_always_on_top);

    // R1: terminalGeometry -> mainGeometry, overridden to an explicit null.
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["terminalGeometry"] = geometry.clone();
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"mainGeometry": null})),
    );
    assert!(load_settings_from_path(&path).main_geometry.is_none());
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    assert!(load_settings_from_path(&control).main_geometry.is_some());

    // R4: startOnlyCoordinators true -> restoreCoordinatorWakeState, false.
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["startOnlyCoordinators"] = json!(true);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"restoreCoordinatorWakeState": false})),
    );
    assert!(!load_settings_from_path(&path).restore_coordinator_wake_state);
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    assert!(load_settings_from_path(&control).restore_coordinator_wake_state);
}

// S32
#[test]
fn the_248_legacy_carrier_survives_while_the_overlay_owns_the_destination() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["startOnlyCoordinators"] = json!(true);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"restoreCoordinatorWakeState": false})),
    );

    let settings = load_settings_from_path(&path);
    assert!(!settings.restore_coordinator_wake_state);
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(
        disk_object(&path).get("startOnlyCoordinators"),
        Some(&json!(true)),
        "the legacy carrier is deferred, not dropped"
    );

    std::fs::remove_file(temp.path().join("settings.50.personal.no-git.json")).unwrap();
    let reloaded = load_settings_from_path(&path);
    assert!(reloaded.restore_coordinator_wake_state);
    assert!(
        disk_object(&path).get("startOnlyCoordinators").is_none(),
        "removing the overlay lets the migration finish"
    );

    // The control: with no overlay the key is removed on the first save.
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    let settings = load_settings_from_path(&control);
    assert!(settings.restore_coordinator_wake_state);
    assert!(disk_object(&control).get("startOnlyCoordinators").is_none());
}

// S33
#[test]
fn a_skip_serializing_owned_key_keeps_its_override_across_every_save() {
    let geometry = json!({"x": 100.0, "y": 100.0, "width": 800.0, "height": 600.0});
    let temp = tempfile::tempdir().unwrap();
    let mut base = watcher_base();
    base["watchersGeometry"] = geometry.clone();
    base["watchers"]["a"]["commands"] = json!(["claude"]);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"watchersGeometry": null, "watchers": {"a": {"commands": null}}})),
    );

    let mut settings = load_settings_from_path(&path);
    assert!(
        settings.watchers_geometry.is_none(),
        "the operator's intent"
    );
    assert!(settings.watchers["a"].valid().unwrap().commands.is_none());

    settings.gemini_api_key = "unrelated-change".to_string();
    let written = save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let object = disk_object(&path);
    assert_eq!(object["watchersGeometry"], geometry);
    assert_eq!(object["watchers"]["a"]["commands"], json!(["claude"]));
    assert_eq!(object["geminiApiKey"], json!("unrelated-change"));
    assert!(
        written.watchers_geometry.is_none(),
        "the value the caller adopts must still be the override"
    );
    assert!(written.watchers["a"].valid().unwrap().commands.is_none());

    // A second save of the returned value must not resurrect the base value.
    let again = save_settings_to_path_preserving_project_paths(&written, &path).unwrap();
    assert!(again.watchers_geometry.is_none());
    assert_eq!(disk_object(&path)["watchersGeometry"], geometry);

    // The control: with no local file the same load-and-save keeps the base.
    let control_temp = tempfile::tempdir().unwrap();
    let control = seed(control_temp.path(), Some(&base), None);
    let mut settings = load_settings_from_path(&control);
    settings.gemini_api_key = "unrelated-change".to_string();
    let written = save_settings_to_path_preserving_project_paths(&settings, &control).unwrap();
    assert!(written.watchers_geometry.is_some());
}

// S34
#[test]
fn the_cas_early_return_honours_the_removal_rule_too() {
    let geometry = json!({"x": 100.0, "y": 100.0, "width": 800.0, "height": 600.0});
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["watchersGeometry"] = geometry;
    base["terminalSnapshotsEnabled"] = json!(false);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"watchersGeometry": null})),
    );

    let current = load_settings_from_path(&path);
    assert!(current.watchers_geometry.is_none());

    let candidate =
        compare_and_set_terminal_snapshots_enabled_at_path(&current, &path, false, false).unwrap();
    assert!(
        candidate.watchers_geometry.is_none(),
        "the disk-decoded candidate must not hand back the base geometry"
    );
}
