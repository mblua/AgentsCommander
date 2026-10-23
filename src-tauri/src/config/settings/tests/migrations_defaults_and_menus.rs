use super::*;

// ── Issue #248 — legacy startOnlyCoordinators → restoreCoordinatorWakeState ──
//
// The minimal JSON below carries the three fields without serde defaults
// (`defaultShell`, `defaultShellArgs`, `agents`) plus whichever issue-#248
// field is under test. All other AppSettings fields use their serde defaults.

#[test]
fn issue_248_migration_legacy_true_translates_to_new_true() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "startOnlyCoordinators": true
        }"#;
    let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
    // Pre-migration: legacy field is parsed, new field is at its default.
    assert_eq!(s.legacy_start_only_coordinators, Some(true));
    assert!(!s.restore_coordinator_wake_state);

    super::super::apply_issue_248_migration(&mut s);

    assert!(s.restore_coordinator_wake_state);
    assert!(s.legacy_start_only_coordinators.is_none());

    // Round-trip — the legacy field must NOT reappear on next save.
    let out = serde_json::to_string(&s).expect("serialize");
    assert!(!out.contains("startOnlyCoordinators"));
    assert!(out.contains("\"restoreCoordinatorWakeState\":true"));
}

#[test]
fn issue_248_migration_legacy_false_translates_to_new_false() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "startOnlyCoordinators": false
        }"#;
    let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
    super::super::apply_issue_248_migration(&mut s);
    assert!(!s.restore_coordinator_wake_state);
    assert!(s.legacy_start_only_coordinators.is_none());
}

#[test]
fn issue_248_no_legacy_field_keeps_new_field_value() {
    // Fresh install or post-migrated settings.json — no legacy field.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "restoreCoordinatorWakeState": true
        }"#;
    let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
    super::super::apply_issue_248_migration(&mut s);
    assert!(s.restore_coordinator_wake_state); // untouched
    assert!(s.legacy_start_only_coordinators.is_none());
}

#[test]
fn issue_248_migration_conflict_keeps_new_value_and_drops_legacy() {
    // Grinch Z4 — both keys present, conflicting values. The user (or a
    // third-party tool) wrote restoreCoordinatorWakeState=true AFTER an
    // older startOnlyCoordinators=false was already on disk. The new value
    // wins; the legacy key is silently dropped from the next save. The
    // helper emits a `warn!` log line for triage — not asserted here (log
    // capture is not wired in the existing test suite), just exercised.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "startOnlyCoordinators": false,
            "restoreCoordinatorWakeState": true
        }"#;
    let mut s: AppSettings = serde_json::from_str(json).expect("deserialize");
    assert_eq!(s.legacy_start_only_coordinators, Some(false));
    assert!(s.restore_coordinator_wake_state);

    super::super::apply_issue_248_migration(&mut s);

    assert!(s.restore_coordinator_wake_state); // preserved
    assert!(s.legacy_start_only_coordinators.is_none()); // dropped
}

// ── Issue #1793 — restart-resume settings contract ─────────────────────

#[test]
fn restart_resume_defaults_are_the_decided_strings() {
    let s = super::super::AppSettings::default();
    assert_eq!(
        s.restart_resume_orchestrator_prompt,
        "AgentsCommander was restarted. Continue with the work that was in flight."
    );
    assert_eq!(s.restart_resume_agent_prompt, ".");
    // #2190: on a fresh install the struct default is now on.
    assert!(s.restart_resume_wake_working_agents);
}

#[test]
fn restart_resume_keys_absent_from_disk_load_as_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    // No restartResume* key of any kind. `sidebarAlwaysOnTop: true` is a
    // tripwire: it is not the default, so if this fixture failed to parse
    // and the loader fell back to defaults, the assertions below would be
    // comparing defaults with defaults and would prove nothing.
    std::fs::write(
        &path,
        r#"{
                "defaultShell": "bash",
                "defaultShellArgs": [],
                "agents": [],
                "sidebarAlwaysOnTop": true
            }"#,
    )
    .unwrap();

    let s = super::super::load_settings_from_path(&path);
    assert!(
        s.sidebar_always_on_top,
        "the fixture must have parsed; a fallback to defaults proves nothing"
    );
    assert_eq!(
        s.restart_resume_orchestrator_prompt,
        "AgentsCommander was restarted. Continue with the work that was in flight."
    );
    assert_eq!(s.restart_resume_agent_prompt, ".");
    // #2190 changed the struct default to true, but the field carries a bare
    // `#[serde(default)]`, so an existing settings.json without the key keeps
    // loading as false — existing user configs are deliberately untouched.
    assert!(!s.restart_resume_wake_working_agents);
}

#[test]
fn restart_resume_empty_prompt_survives_a_round_trip() {
    // Load-bearing: `#[serde(default = "...")]` fires only on ABSENCE, so an
    // explicitly empty prompt must not be re-defaulted on the way back in.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");

    let s = super::super::AppSettings {
        restart_resume_orchestrator_prompt: String::new(),
        restart_resume_agent_prompt: String::new(),
        ..super::super::AppSettings::default()
    };
    super::super::save_settings_to_path_preserving_project_paths(&s, &path).unwrap();

    let loaded = super::super::load_settings_from_path(&path);
    assert_eq!(loaded.restart_resume_orchestrator_prompt, "");
    assert_eq!(loaded.restart_resume_agent_prompt, "");
}

#[test]
fn restart_resume_serde_keys_are_camel_case() {
    // A default round trip cannot detect a wrong or renamed serde key, so the
    // raw key text is asserted here. Do not drop or merge these assertions.
    let json = serde_json::to_string(&super::super::AppSettings::default()).unwrap();
    assert!(
        json.contains("\"restartResumeOrchestratorPrompt\""),
        "the orchestrator prompt must serialize under its decided camelCase key"
    );
    assert!(
        json.contains("\"restartResumeAgentPrompt\""),
        "the agent prompt must serialize under its decided camelCase key"
    );
    assert!(
        json.contains("\"restartResumeWakeWorkingAgents\""),
        "the wake toggle must serialize under its decided camelCase key"
    );
}

#[test]
fn coord_sort_by_activity_defaults_when_missing_from_json() {
    // Old settings.json without the new field must deserialize to false.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(!s.coord_sort_by_activity);
}

#[test]
fn activity_log_enabled_defaults_false_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(!s.activity_log_enabled);
}

#[test]
fn spec_board_enabled_defaults_false_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(!s.spec_board_enabled);
}

#[test]
fn coding_agent_install_enabled_default_impl_is_false() {
    assert!(!AppSettings::default().coding_agent_install_enabled);
}

#[test]
fn coding_agent_install_enabled_defaults_false_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;
    let settings: AppSettings = serde_json::from_str(json).expect("deserialize legacy settings");
    assert!(!settings.coding_agent_install_enabled);
}

#[test]
fn coding_agent_install_enabled_explicit_false_round_trips() {
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value["codingAgentInstallEnabled"] = serde_json::Value::Bool(false);
    let settings: AppSettings = serde_json::from_value(value).unwrap();
    assert!(!settings.coding_agent_install_enabled);
    assert_eq!(
        serde_json::to_value(settings).unwrap()["codingAgentInstallEnabled"],
        serde_json::Value::Bool(false)
    );
}

#[test]
fn coding_agent_install_enabled_true_round_trips_under_camel_case_key() {
    let settings = AppSettings {
        coding_agent_install_enabled: true,
        ..AppSettings::default()
    };
    let value = serde_json::to_value(settings).unwrap();
    assert_eq!(
        value["codingAgentInstallEnabled"],
        serde_json::Value::Bool(true)
    );
    assert!(value.get("coding_agent_install_enabled").is_none());
    let back: AppSettings = serde_json::from_value(value).unwrap();
    assert!(back.coding_agent_install_enabled);
    assert_eq!(
        serde_json::to_value(back).unwrap()["codingAgentInstallEnabled"],
        serde_json::Value::Bool(true)
    );
}

#[test]
fn container_options_enabled_default_impl_is_false() {
    assert!(!AppSettings::default().container_options_enabled);
}

#[test]
fn container_options_enabled_defaults_false_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;
    let settings: AppSettings = serde_json::from_str(json).expect("deserialize legacy settings");
    assert!(!settings.container_options_enabled);
}

#[test]
fn container_options_enabled_explicit_false_round_trips() {
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value["containerOptionsEnabled"] = serde_json::Value::Bool(false);
    let settings: AppSettings = serde_json::from_value(value).unwrap();
    assert!(!settings.container_options_enabled);
    assert_eq!(
        serde_json::to_value(settings).unwrap()["containerOptionsEnabled"],
        serde_json::Value::Bool(false)
    );
}

#[test]
fn container_options_enabled_true_round_trips_under_camel_case_key() {
    let settings = AppSettings {
        container_options_enabled: true,
        ..AppSettings::default()
    };
    let value = serde_json::to_value(settings).unwrap();
    assert_eq!(
        value["containerOptionsEnabled"],
        serde_json::Value::Bool(true)
    );
    assert!(value.get("container_options_enabled").is_none());
    let back: AppSettings = serde_json::from_value(value).unwrap();
    assert!(back.container_options_enabled);
    assert_eq!(
        serde_json::to_value(back).unwrap()["containerOptionsEnabled"],
        serde_json::Value::Bool(true)
    );
}

#[test]
fn co_managed_enabled_defaults_false_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(!s.co_managed_enabled);
}

#[test]
fn co_managed_enabled_default_impl_is_false() {
    assert!(!AppSettings::default().co_managed_enabled);
}

#[test]
fn co_managed_enabled_round_trips_under_its_camel_case_key() {
    let settings = AppSettings {
        co_managed_enabled: true,
        ..AppSettings::default()
    };
    let value = serde_json::to_value(&settings).unwrap();
    assert_eq!(value["coManagedEnabled"], serde_json::Value::Bool(true));
    let back: AppSettings = serde_json::from_value(value).unwrap();
    assert_eq!(
        serde_json::to_value(&back).unwrap()["coManagedEnabled"],
        serde_json::Value::Bool(true)
    );
}

// ---- #548: per-(agent, letter) profile label overrides ----

#[test]
fn profile_labels_override_survives_serde_round_trip() {
    let mut settings = AppSettings::default();
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "codex".to_string(),
            BTreeMap::from([("B".to_string(), "turbo".to_string())]),
        );

    let json = serde_json::to_string(&settings).unwrap();
    let round_tripped: AppSettings = serde_json::from_str(&json).unwrap();

    assert_eq!(
        round_tripped.coding_agent_profiles.profile_labels_by_agent["codex"]["B"],
        "turbo"
    );
}

#[test]
fn profile_labels_default_to_empty_and_always_serialize() {
    // A pre-#548 v2 codingAgentProfiles object with NO profileLabelsByAgent key
    // must load as an empty map (proves #[serde(default)]).
    let json = r##"{
            "schemaVersion": 2,
            "profileSlots": { "A": { "label": "" } },
            "defaultProfileByAgent": {},
            "profilesByAgent": {}
        }"##;

    let profiles: super::super::CodingAgentProfilesConfig = serde_json::from_str(json).unwrap();
    assert!(profiles.profile_labels_by_agent.is_empty());

    // Re-serializing always emits the key (no skip_serializing_if), parity with
    // profilesByAgent, so the on-disk key is stable.
    let out = serde_json::to_string(&profiles).unwrap();
    assert!(out.contains("profileLabelsByAgent"));
}

#[test]
fn repair_prunes_invalid_letter_label_overrides_and_keeps_valid() {
    let mut settings = settings_with_agents(&[("Codex", "codex")]); // id = agent-0

    // Pre-seed the A cell so the config is otherwise repair-clean; the only
    // change repair makes is dropping the invalid-letter overrides.
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "agent-0".to_string(),
        BTreeMap::from([("A".to_string(), super::super::empty_profile_cell())]),
    );
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "agent-0".to_string(),
            BTreeMap::from([
                ("B".to_string(), "turbo".to_string()),
                ("1".to_string(), "bad-digit".to_string()),
                ("ab".to_string(), "bad-two-char".to_string()),
                ("b".to_string(), "bad-lowercase".to_string()),
            ]),
        );

    let changed =
        repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    assert!(changed);
    let labels = &settings.coding_agent_profiles.profile_labels_by_agent["agent-0"];
    assert_eq!(labels.get("B").map(String::as_str), Some("turbo"));
    assert!(!labels.contains_key("1"));
    assert!(!labels.contains_key("ab"));
    assert!(!labels.contains_key("b"));
}

#[test]
fn repair_keeps_orphan_agent_id_label_override() {
    // An override under an agent id NOT present in settings.agents must survive
    // repair: no agent-id prune (plan 3.7); it is inert dead storage the
    // resolver never reads.
    let mut settings = settings_with_agents(&[("Codex", "codex")]); // id = agent-0
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "ghost-agent".to_string(),
            BTreeMap::from([("B".to_string(), "turbo".to_string())]),
        );

    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    assert_eq!(
        settings.coding_agent_profiles.profile_labels_by_agent["ghost-agent"]["B"],
        "turbo"
    );
}

#[test]
fn repair_with_valid_label_map_does_not_flip_changed() {
    let mut settings = settings_with_agents(&[("Codex", "codex")]); // id = agent-0

    // Make the config otherwise repair-clean: agent-0 already holds its A cell.
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "agent-0".to_string(),
        BTreeMap::from([("A".to_string(), super::super::empty_profile_cell())]),
    );
    // A label map holding only valid letters must not trigger a disk rewrite.
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "agent-0".to_string(),
            BTreeMap::from([("B".to_string(), "turbo".to_string())]),
        );

    let changed =
        repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    assert!(!changed);
    assert_eq!(
        settings.coding_agent_profiles.profile_labels_by_agent["agent-0"]["B"],
        "turbo"
    );
}

#[test]
fn resource_monitor_settings_default_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(s.resource_monitor_enabled);
    assert_eq!(s.max_concurrent_agent_processes, 32);
    assert_eq!(s.resource_watchdog_action, ResourceWatchdogAction::Warn);
    assert!(validate_resource_settings(&s).is_ok());
}

// ---- #1032: the per-agent context regex ---------------------------------------

/// Criterion 6. Every settings file that exists today predates this field, so the one
/// thing this field must never do is make an existing config fail to load.
#[test]
fn settings_without_context_regex_deserialize_unchanged() {
    let json = r##"{ "id": "claude", "label": "Claude", "command": "claude",
                         "color": "#10b981" }"##;
    let agent: super::super::AgentConfig = serde_json::from_str(json).expect("deserializes");
    assert_eq!(agent.label, "Claude");
    assert_eq!(agent.context_regex, None, "absent must mean off, not empty");
}

/// The field is an agent-config field, where an absent key is the correct way to say
/// "off" - unlike the IPC payload, where `percent` must serialize as an explicit null.
#[test]
fn context_regex_round_trips_as_camel_case() {
    let json = r##"{ "id": "claude", "label": "Claude", "command": "claude",
                         "color": "#10b981",
                         "contextRegex": "^ {2}Context [\u2591\u2588]+ (\\d{1,3})%" }"##;
    let agent: super::super::AgentConfig = serde_json::from_str(json).expect("deserializes");
    assert_eq!(
        agent.context_regex.as_deref(),
        Some("^ {2}Context [\u{2591}\u{2588}]+ (\\d{1,3})%")
    );

    let back = serde_json::to_string(&agent).expect("serializes");
    assert!(
        back.contains("\"contextRegex\""),
        "the frontend contract is camelCase: {back}"
    );

    let cleared = super::super::AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        context_regex: None,
        ..agent
    };
    let back = serde_json::to_string(&cleared).expect("serializes");
    assert!(
        !back.contains("contextRegex"),
        "None must omit the key here, so an untouched config stays untouched: {back}"
    );
}

// ---- #1646 / #1647: blocking menus and menu guard -----------------------------

#[test]
fn shipped_defaults_serve_pi_and_codex_only() {
    // The shipped patterns, not any per-agent materialization: the #1905 export
    // drops pristine arrays and the store serves these defaults for `None`.

    // Pi
    let pi_menus = super::super::default_blocking_menus_for_command("pi");
    assert_eq!(pi_menus.len(), 1);
    let pi_cfg = pi_menus[0].valid().unwrap();
    assert_eq!(pi_cfg.pattern, r"^\s*Trust project folder\?");
    assert_eq!(
        pi_cfg.notification,
        "pi is waiting for you to answer the folder-trust menu in this terminal"
    );
    assert!(pi_cfg.enabled);
    assert_eq!(
        pi_cfg.captured_against.as_deref(),
        Some("pi 0.52 / Windows")
    );

    // Codex
    let codex_menus = super::super::default_blocking_menus_for_command("codex");
    assert_eq!(codex_menus.len(), 2);
    assert_eq!(
        codex_menus[1],
        super::super::BlockingMenuEntry::Valid(super::super::codex_hooks_review_menu())
    );
    let codex_cfg = codex_menus[0].valid().unwrap();
    assert_eq!(
        codex_cfg.pattern,
        r"^\s*Do you trust the contents of this directory\?"
    );
    assert_eq!(
        codex_cfg.notification,
        "codex is waiting for you to answer the folder-trust menu in this terminal"
    );
    assert!(codex_cfg.enabled);
    assert_eq!(
        codex_cfg.captured_against.as_deref(),
        Some("codex 0.x / Linux")
    );

    // #1757 - the hooks-review entry, appended AFTER folder-trust (D13).
    let codex_hooks_cfg = codex_menus[1].valid().unwrap();
    assert_eq!(
        codex_hooks_cfg.pattern,
        super::super::CODEX_HOOKS_REVIEW_PATTERN
    );
    assert_eq!(
        codex_hooks_cfg.notification,
        "codex is waiting for you to answer the hooks-review menu in this terminal"
    );
    assert!(codex_hooks_cfg.enabled);
    assert_eq!(
        codex_hooks_cfg.captured_against.as_deref(),
        Some("codex 0.153.2 / Windows")
    );

    // Claude -> empty array: no shipped set, nothing materialized anymore.
    let claude_menus = super::super::default_blocking_menus_for_command("claude");
    assert!(claude_menus.is_empty());
}

#[test]
fn test_blocking_menus_tolerant_parsing() {
    let json = r##"{
            "id": "pi",
            "label": "Pi",
            "command": "pi",
            "color": "#10b981",
            "blockingMenus": [
                {
                    "pattern": "^\\s*Trust",
                    "notification": "prompt msg",
                    "enabled": true
                },
                12345,
                {
                    "invalidFieldOnly": true
                }
            ]
        }"##;

    let agent: super::super::AgentConfig =
        serde_json::from_str(json).expect("deserializes with invalid entries");
    let menus = agent
        .blocking_menus
        .as_ref()
        .expect("blocking_menus present");
    assert_eq!(menus.len(), 3);
    assert!(menus[0].valid().is_some());
    assert!(menus[1].valid().is_none());
    assert!(menus[2].valid().is_none());

    let serialized = serde_json::to_string(&agent).expect("serializes back");
    assert!(serialized.contains("12345"));
    assert!(serialized.contains("invalidFieldOnly"));

    let round_trip: super::super::AgentConfig =
        serde_json::from_str(&serialized).expect("round trip deserializes");
    assert_eq!(round_trip.blocking_menus, agent.blocking_menus);
}

#[test]
fn test_blocking_menus_explicit_empty_array() {
    let json = r##"{
            "id": "pi",
            "label": "Pi",
            "command": "pi",
            "color": "#10b981",
            "blockingMenus": []
        }"##;

    let agent: super::super::AgentConfig =
        serde_json::from_str(json).expect("deserializes explicit empty array");
    assert_eq!(agent.blocking_menus, Some(vec![]));

    let serialized = serde_json::to_string(&agent).expect("serializes");
    assert!(
        serialized.contains(r#""blockingMenus":[]"#),
        "an explicit empty array round-trips: {serialized}"
    );
}

#[test]
fn test_menu_guard_master_switch() {
    let def = AppSettings::default();
    assert!(def.menu_guard_enabled);

    let json = r#"{
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserializes");
    assert!(s.menu_guard_enabled, "defaults to true when missing");

    let json_disabled = r#"{
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "menuGuardEnabled": false
        }"#;
    let s_disabled: AppSettings =
        serde_json::from_str(json_disabled).expect("deserializes disabled");
    assert!(!s_disabled.menu_guard_enabled);

    let serialized = serde_json::to_string(&s_disabled).expect("serializes");
    assert!(serialized.contains("\"menuGuardEnabled\":false"));
}

// ---- #1757: the Codex hooks-review blocking menu -----------------------

/// One agent carrying exactly the `blocking_menus` state under test.
pub(super) fn agent_1757(
    id: &str,
    command: &str,
    blocking_menus: Option<Vec<super::super::BlockingMenuEntry>>,
) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: id.to_string(),
        command: command.to_string(),
        color: "#000000".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus,
        backend: Default::default(),
    }
}

fn settings_1757(agents: Vec<AgentConfig>) -> AppSettings {
    AppSettings {
        agents,
        ..AppSettings::default()
    }
}

/// The pre-#1757 materialized Codex array's only entry.
fn folder_trust_entry_1757() -> super::super::BlockingMenuEntry {
    super::super::BlockingMenuEntry::Valid(super::super::BlockingMenuConfig {
        pattern: r"^\s*Do you trust the contents of this directory\?".to_string(),
        notification: "codex is waiting for you to answer the folder-trust menu in this terminal"
            .to_string(),
        enabled: true,
        captured_against: Some("codex 0.x / Linux".to_string()),
    })
}

fn hooks_review_entry_1757(enabled: bool) -> super::super::BlockingMenuEntry {
    super::super::BlockingMenuEntry::Valid(super::super::BlockingMenuConfig {
        pattern: super::super::CODEX_HOOKS_REVIEW_PATTERN.to_string(),
        notification: "codex is waiting for you to answer the hooks-review menu in this terminal"
            .to_string(),
        enabled,
        captured_against: Some("codex 0.153.2 / Windows".to_string()),
    })
}

fn assert_folder_trust_entry_1757(entry: &super::super::BlockingMenuEntry) {
    let cfg = entry.valid().expect("folder-trust entry is valid");
    assert_eq!(
        cfg.pattern,
        r"^\s*Do you trust the contents of this directory\?"
    );
    assert_eq!(
        cfg.notification,
        "codex is waiting for you to answer the folder-trust menu in this terminal"
    );
    assert!(cfg.enabled);
    assert_eq!(cfg.captured_against.as_deref(), Some("codex 0.x / Linux"));
}

fn assert_hooks_review_entry_1757(entry: &super::super::BlockingMenuEntry) {
    let cfg = entry.valid().expect("hooks-review entry is valid");
    assert_eq!(cfg.pattern, super::super::CODEX_HOOKS_REVIEW_PATTERN);
    assert_eq!(
        cfg.notification,
        "codex is waiting for you to answer the hooks-review menu in this terminal"
    );
    assert!(cfg.enabled);
    assert_eq!(
        cfg.captured_against.as_deref(),
        Some("codex 0.153.2 / Windows")
    );
}

// T4
#[test]
fn issue_1757_appends_to_an_already_materialized_codex_array() {
    let mut settings = settings_1757(vec![agent_1757(
        "codex",
        "codex",
        Some(vec![folder_trust_entry_1757()]),
    )]);

    assert!(
        super::super::apply_issue_1757_migration(&mut settings),
        "an already-materialized codex array must be extended"
    );

    let menus = settings.agents[0]
        .blocking_menus
        .as_ref()
        .expect("blocking_menus present");
    assert_eq!(menus.len(), 2);
    assert_folder_trust_entry_1757(&menus[0]);
    assert_hooks_review_entry_1757(&menus[1]);
}

// T5
#[test]
fn issue_1757_is_idempotent() {
    let mut settings = settings_1757(vec![agent_1757(
        "codex",
        "codex",
        Some(vec![folder_trust_entry_1757()]),
    )]);

    assert!(super::super::apply_issue_1757_migration(&mut settings));
    assert!(
        !super::super::apply_issue_1757_migration(&mut settings),
        "the second call must be a no-op: the pattern is already present"
    );
    assert_eq!(
        settings.agents[0]
            .blocking_menus
            .as_ref()
            .expect("blocking_menus present")
            .len(),
        2
    );
}

// T6
#[test]
fn issue_1757_never_activates_an_explicitly_empty_array() {
    let mut settings = settings_1757(vec![agent_1757("codex", "codex", Some(vec![]))]);

    assert!(
        !super::super::apply_issue_1757_migration(&mut settings),
        "Some(vec![]) is the documented explicitly-disabled state (D8)"
    );
    assert_eq!(settings.agents[0].blocking_menus, Some(vec![]));
}

// T7
#[test]
fn issue_1757_respects_a_disabled_hooks_entry() {
    let disabled = hooks_review_entry_1757(false);
    let mut settings = settings_1757(vec![agent_1757(
        "codex",
        "codex",
        Some(vec![disabled.clone()]),
    )]);

    assert!(
        !super::super::apply_issue_1757_migration(&mut settings),
        "the presence test ignores the enabled flag, so enabled: false is a durable off switch \
                (D9)"
    );
    assert_eq!(settings.agents[0].blocking_menus, Some(vec![disabled]));
}

// T8
#[test]
fn issue_1757_ignores_non_codex_agents() {
    let pi_menus = vec![super::super::BlockingMenuEntry::Valid(
        super::super::BlockingMenuConfig {
            pattern: r"^\s*Trust project folder\?".to_string(),
            notification: "pi is waiting for you to answer the folder-trust menu in this terminal"
                .to_string(),
            enabled: true,
            captured_against: Some("pi 0.52 / Windows".to_string()),
        },
    )];
    let claude_menus = vec![super::super::BlockingMenuEntry::Valid(
        super::super::BlockingMenuConfig {
            pattern: "^Something else entirely".to_string(),
            notification: "claude is waiting".to_string(),
            enabled: true,
            captured_against: None,
        },
    )];
    let mut settings = settings_1757(vec![
        agent_1757("pi", "pi", Some(pi_menus.clone())),
        agent_1757("claude", "claude", Some(claude_menus.clone())),
    ]);

    assert!(
        !super::super::apply_issue_1757_migration(&mut settings),
        "the stem gate must never target an agent the default would not"
    );
    assert_eq!(settings.agents[0].blocking_menus, Some(pi_menus));
    assert_eq!(settings.agents[1].blocking_menus, Some(claude_menus));
}

// T9 - the only new test that observes the real load chain. Every other
// needs_save source is deliberately inert on this fixture, so the export's
// own flag is the only thing that can drive the write.
#[test]
fn a_pre_1757_pristine_codex_array_is_dropped_by_the_export() {
    const FIXTURE: &str = include_str!("fixtures/pristine_codex_array_settings.json");

    // Tripwire: a single-backslash escape here is not legal JSON, the load would
    // fall back to default_settings_with_overlay with zero agents, and this test
    // would silently stop testing anything.
    serde_json::from_str::<serde_json::Value>(FIXTURE).expect("the fixture must be valid JSON");

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(&path, FIXTURE).unwrap();

    let settings = super::super::load_settings_from_path(&path);
    assert_eq!(
        settings.agents.len(),
        1,
        "the fixture must parse; zero agents means the loader fell back to defaults"
    );

    // The in-memory 1757 step ran (fixture had only folder-trust) and the array
    // then matched the shipped codex set, so the export dropped it as pristine:
    // the key is gone from settings.json and no `.local` file was created.
    let raw = std::fs::read_to_string(&path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(
        value["agents"][0].get("blockingMenus").is_none(),
        "the pristine array must be stripped from settings.json"
    );
    assert!(
        !super::super::blocking_menus_local_path(&path).exists(),
        "pristine arrays must not create the .local file"
    );

    // The stripped agent is served by the shipped file from now on.
    let store = super::super::BlockingMenusStore::load_from_settings_path(&path);
    assert_eq!(
        store.resolve_for(&settings.agents[0]),
        super::super::default_blocking_menus_for_command("codex"),
        "the shipped codex set (two entries) must serve the agent"
    );
    assert!(
        super::super::blocking_menus_shipped_path(&path).exists(),
        "the store must have materialized the shipped file"
    );
}
