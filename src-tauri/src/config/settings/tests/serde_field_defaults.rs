use super::*;

#[test]
fn team_idle_beep_enabled_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(s.team_idle_beep_enabled);
    s.team_idle_beep_enabled = false;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"teamIdleBeepEnabled\":false"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(!back.team_idle_beep_enabled);
}

#[test]
fn team_idle_beep_enabled_defaults_true_when_missing_from_json() {
    // Old settings.json without the new field must deserialize to true (default_true).
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
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(s.team_idle_beep_enabled);
}

#[test]
fn telegram_network_poll_error_logging_round_trips_through_serde() {
    let s = AppSettings {
        telegram_network_poll_error_logging: TelegramNetworkPollErrorLogging {
            first_failure_level: TelegramPollFailureLogLevel::Debug,
            transient_repeat_level: TelegramPollFailureLogLevel::Warn,
            sustained_level: TelegramPollFailureLogLevel::Error,
            sustained_after_seconds: 12,
            sustained_repeat_seconds: 34,
            recovery_level: TelegramPollRecoveryLogLevel::Warn,
        },
        ..AppSettings::default()
    };

    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"telegramNetworkPollErrorLogging\""));
    assert!(json.contains("\"sustainedAfterSeconds\":12"));

    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        back.telegram_network_poll_error_logging
            .sustained_after_seconds,
        12
    );
    assert_eq!(
        back.telegram_network_poll_error_logging.recovery_level,
        TelegramPollRecoveryLogLevel::Warn
    );
}

#[test]
fn telegram_network_poll_error_logging_defaults_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": []
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert_eq!(
        s.telegram_network_poll_error_logging,
        TelegramNetworkPollErrorLogging::default()
    );
}

#[test]
fn telegram_network_poll_error_logging_field_defaults_are_backfilled() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "telegramNetworkPollErrorLogging": {
                "sustainedAfterSeconds": 5
            }
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize partial policy");
    assert_eq!(
        s.telegram_network_poll_error_logging
            .sustained_after_seconds,
        5
    );
    assert_eq!(
        s.telegram_network_poll_error_logging.first_failure_level,
        TelegramPollFailureLogLevel::Warn
    );
    assert_eq!(
        s.telegram_network_poll_error_logging.transient_repeat_level,
        TelegramPollFailureLogLevel::Debug
    );
    assert_eq!(
        s.telegram_network_poll_error_logging.recovery_level,
        TelegramPollRecoveryLogLevel::Info
    );
}

/// Test 8 (#2265): a settings file written before this change carries no
/// `jev*` key and must load with every documented default.
#[test]
fn jev_fields_default_when_missing_from_json() {
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": []
        }"#;

    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert_eq!(s.jev_api_key, "");
    assert_eq!(s.jev_model, "jev-1.13.0");
    assert_eq!(s.jev_endpoint, "https://api.typesafe.ai/v1/systemone");
    assert_eq!(s.jev_timeout_secs, 20);
    assert_eq!(s.jev_threshold, 0.70);
    assert_eq!(s.jev_margin, 0.15);
}

/// Test 9 (#2265): the key default is empty (not a placeholder) and the
/// model default is the pinned literal, so a later floating tag is a
/// deliberate, visible change.
#[test]
fn jev_api_key_defaults_empty_and_model_is_pinned() {
    let s = AppSettings::default();
    assert_eq!(s.jev_api_key, "");
    assert_eq!(s.jev_model, "jev-1.13.0");
    assert_ne!(s.jev_model, "jev-latest");
}

#[test]
fn sounds_enabled_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(s.sounds_enabled);
    s.sounds_enabled = false;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"soundsEnabled\":false"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(!back.sounds_enabled);
}

#[test]
fn sounds_enabled_defaults_true_when_missing_from_json() {
    // Old settings.json with only team_idle_beep_enabled (and no soundsEnabled
    // field) must deserialize with sounds_enabled = true so existing users
    // remain audible until they explicitly mute.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "teamIdleBeepEnabled": false,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(s.sounds_enabled);
    // Existing per-feature toggle is honored as-is.
    assert!(!s.team_idle_beep_enabled);
}

#[test]
fn rail_collapse_fields_round_trip_through_serde() {
    let mut s = AppSettings::default();
    assert!(s.rail_collapsed_projects.is_empty());
    assert!(!s.rail_favorites_collapsed);

    s.rail_collapsed_projects = vec!["c:/foo/bar".to_string(), "d:/baz".to_string()];
    s.rail_favorites_collapsed = true;

    let json = serde_json::to_string(&s).expect("serialize");
    // (#965) camelCase guard. A broken `rename_all` would otherwise surface only as a
    // silently dead frontend, since the TS side reads `railCollapsedProjects` /
    // `railFavoritesCollapsed`.
    assert!(
        json.contains("\"railCollapsedProjects\":[\"c:/foo/bar\",\"d:/baz\"]"),
        "{json}"
    );
    assert!(json.contains("\"railFavoritesCollapsed\":true"), "{json}");

    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.rail_collapsed_projects, s.rail_collapsed_projects);
    assert!(back.rail_favorites_collapsed);
}

#[test]
fn rail_collapse_fields_default_when_missing_from_json() {
    // A legacy settings.json carries neither key. Build one by round-tripping a
    // default and deleting them, so every other field stays present and the test
    // isolates exactly the `#[serde(default)]` behavior.
    let mut value = serde_json::to_value(AppSettings::default()).expect("to_value");
    let obj = value.as_object_mut().expect("settings object");
    obj.remove("railCollapsedProjects");
    obj.remove("railFavoritesCollapsed");

    let s: AppSettings = serde_json::from_value(value).expect("deserialize legacy");

    assert!(s.rail_collapsed_projects.is_empty());
    assert!(!s.rail_favorites_collapsed);
}

#[test]
fn theme_light_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(!s.theme_light);
    s.theme_light = true;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"themeLight\":true"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(back.theme_light);
}

#[test]
fn theme_light_defaults_false_when_missing_from_json() {
    // Settings without themeLight now open dark by default.
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
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(!s.theme_light);
}

#[test]
fn theme_light_explicit_false_survives_round_trip() {
    // Once a user explicitly disables light mode, the value must survive
    // serialize/deserialize without being altered by the serde default.
    let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "themeLight": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize");
    assert!(!s.theme_light);
    let out = serde_json::to_string(&s).expect("serialize");
    let back: AppSettings = serde_json::from_str(&out).expect("re-deserialize");
    assert!(!back.theme_light);
}

#[test]
fn spec_board_enabled_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(!s.spec_board_enabled);
    let default_json = serde_json::to_string(&s).expect("serialize default");
    assert!(default_json.contains("\"specBoardEnabled\":false"));

    s.spec_board_enabled = true;

    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"specBoardEnabled\":true"));

    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(back.spec_board_enabled);
}

#[test]
fn resource_monitor_settings_round_trip_through_serde() {
    let mut s = AppSettings::default();
    assert!(s.resource_monitor_enabled);
    assert_eq!(s.max_concurrent_agent_processes, 32);
    assert_eq!(s.resource_watchdog_action, ResourceWatchdogAction::Warn);
    assert_eq!(s.agent_group_warn_private_bytes, 8 * 1024 * 1024 * 1024);
    assert_eq!(s.agent_group_kill_private_bytes, 12 * 1024 * 1024 * 1024);
    assert_eq!(s.agent_process_kill_private_bytes, 12 * 1024 * 1024 * 1024);
    assert!(s.resource_keep_last_snapshot);
    assert!(s.resource_backoff_polling);

    s.resource_monitor_enabled = false;
    s.max_concurrent_agent_processes = 5;
    s.resource_watchdog_action = ResourceWatchdogAction::KillGroup;
    s.agent_group_warn_private_bytes = 128;
    s.agent_group_kill_private_bytes = 256;
    s.agent_process_kill_private_bytes = 512;
    s.resource_keep_last_snapshot = false;
    s.resource_backoff_polling = false;

    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"resourceMonitorEnabled\":false"));
    assert!(json.contains("\"resourceWatchdogAction\":\"killGroup\""));

    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(!back.resource_monitor_enabled);
    assert_eq!(back.max_concurrent_agent_processes, 5);
    assert_eq!(
        back.resource_watchdog_action,
        ResourceWatchdogAction::KillGroup
    );
    assert_eq!(back.agent_group_warn_private_bytes, 128);
    assert_eq!(back.agent_group_kill_private_bytes, 256);
    assert_eq!(back.agent_process_kill_private_bytes, 512);
    assert!(!back.resource_keep_last_snapshot);
    assert!(!back.resource_backoff_polling);
}

#[test]
fn coordinator_clock_settings_default_when_keys_absent() {
    // #552: an old settings.json (no coordinator-* keys) must deserialize
    // cleanly to the documented defaults, no migration.
    // Serialize a default, strip ONLY these coordinator keys, deserialize back.
    let mut value =
        serde_json::to_value(AppSettings::default()).expect("serialize default to value");
    let obj = value
        .as_object_mut()
        .expect("settings serializes to an object");
    obj.remove("coordinatorIdleBadgeYellowMinutes");
    obj.remove("coordinatorIdleBadgeRedMinutes");
    obj.remove("coordinatorAutoCloseEnabled");
    obj.remove("coordinatorAutoCloseMinutes");
    obj.remove("coordinatorAutoCloseSkipTelegramAssigned");

    let back: AppSettings = serde_json::from_value(value).expect("deserialize without keys");
    assert!(back.coordinator_auto_close_enabled);
    assert_eq!(back.coordinator_auto_close_minutes, 60);
    assert!(!back.coordinator_auto_close_skip_telegram_assigned);
    assert_eq!(back.coordinator_idle_badge_yellow_minutes, 30);
    assert_eq!(back.coordinator_idle_badge_red_minutes, 60);
}

#[test]
fn coordinator_auto_close_skip_telegram_assigned_round_trips() {
    let s = AppSettings {
        coordinator_auto_close_skip_telegram_assigned: true,
        ..AppSettings::default()
    };

    let json = serde_json::to_value(&s).expect("serialize settings");
    assert_eq!(
        json.get("coordinatorAutoCloseSkipTelegramAssigned"),
        Some(&serde_json::Value::Bool(true))
    );

    let back: AppSettings = serde_json::from_value(json).expect("deserialize settings");
    assert!(back.coordinator_auto_close_skip_telegram_assigned);
}

#[test]
fn resolve_auto_self_clear_precedence_table() {
    // #640 §5 resolution table. Master-first kill switch, then per-agent
    // override, then the class-aware default computed at the call site.
    let mut s = AppSettings::default();

    // master ON, no override, class default ON (coordinator/Root) -> ON.
    assert!(super::super::resolve_auto_self_clear(&s, "tech-lead", true));
    // master ON, no override, class default OFF (specialist) -> OFF.
    assert!(!super::super::resolve_auto_self_clear(
        &s, "dev-rust", false
    ));

    // explicit per-agent opt-in overrides a specialist's OFF class default.
    s.auto_self_clear_by_agent
        .insert("dev-rust".to_string(), true);
    assert!(super::super::resolve_auto_self_clear(&s, "dev-rust", false));
    // explicit per-agent opt-out overrides a coordinator's ON class default.
    s.auto_self_clear_by_agent
        .insert("tech-lead".to_string(), false);
    assert!(!super::super::resolve_auto_self_clear(
        &s,
        "tech-lead",
        true
    ));

    // master OFF is an absolute kill switch: off for everyone, and it wins
    // even over a per-agent opt-in.
    s.auto_self_clear_enabled = false;
    assert!(!super::super::resolve_auto_self_clear(
        &s,
        "tech-lead",
        true
    ));
    assert!(!super::super::resolve_auto_self_clear(&s, "dev-rust", true));
}

#[test]
fn resolve_auto_self_clear_empty_name_uses_class_default() {
    // #640 L1: the Root derives to an empty name (no `_agent_`/`__agent_`
    // prefix); with no "" key it falls through to the class default. Same
    // path covers any name-derivation failure.
    let s = AppSettings::default();
    assert!(super::super::resolve_auto_self_clear(&s, "", true));
    assert!(!super::super::resolve_auto_self_clear(&s, "", false));
}

#[test]
fn auto_self_clear_defaults_when_keys_absent() {
    // #640: an old settings.json (no auto-self-clear keys) must deserialize
    // to the documented defaults: master ON + empty per-agent map.
    let mut value =
        serde_json::to_value(AppSettings::default()).expect("serialize default to value");
    let obj = value
        .as_object_mut()
        .expect("settings serializes to an object");
    obj.remove("autoSelfClearEnabled");
    obj.remove("autoSelfClearByAgent");

    let back: AppSettings = serde_json::from_value(value).expect("deserialize without keys");
    assert!(back.auto_self_clear_enabled);
    assert!(back.auto_self_clear_by_agent.is_empty());
}

#[test]
fn auto_self_clear_round_trips_through_serde() {
    let mut s = AppSettings {
        auto_self_clear_enabled: false,
        ..AppSettings::default()
    };
    s.auto_self_clear_by_agent
        .insert("dev-rust".to_string(), true);
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"autoSelfClearEnabled\":false"));
    assert!(json.contains("\"autoSelfClearByAgent\":{\"dev-rust\":true}"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(!back.auto_self_clear_enabled);
    assert_eq!(back.auto_self_clear_by_agent.get("dev-rust"), Some(&true));
}

#[test]
fn agent_auto_update_by_command_default_empty_and_round_trip_camel_case() {
    // #1327: the per-command override map defaults to empty on a fresh
    // settings object AND survives the serde round trip with camelCase keys.
    let defaults = AppSettings::default();
    assert!(defaults.agent_auto_update_by_command.is_empty());

    let mut s = AppSettings::default();
    s.agent_auto_update_by_command
        .insert("claude".to_string(), true);
    s.agent_auto_update_by_command
        .insert("codex".to_string(), false);
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"agentAutoUpdateByCommand\":{\"claude\":true,\"codex\":false}"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.agent_auto_update_by_command.get("claude"), Some(&true));
    assert_eq!(back.agent_auto_update_by_command.get("codex"), Some(&false));
}

#[test]
fn agent_auto_update_by_command_absent_key_deserializes_empty() {
    // An old settings.json without the #1327 key deserializes to an empty
    // map (serde default), never an error.
    let mut value =
        serde_json::to_value(AppSettings::default()).expect("serialize default to value");
    let obj = value.as_object_mut().expect("settings object");
    obj.remove("agentAutoUpdateByCommand");
    let back: AppSettings = serde_json::from_value(value).expect("deserialize without keys");
    assert!(back.agent_auto_update_by_command.is_empty());
}

#[test]
fn validate_resource_settings_rejects_invalid_limits() {
    let s = AppSettings {
        max_concurrent_agent_processes: 0,
        ..AppSettings::default()
    };
    assert!(validate_resource_settings(&s)
        .unwrap_err()
        .contains("maxConcurrentAgentProcesses"));

    let s = AppSettings {
        agent_group_warn_private_bytes: 10,
        agent_group_kill_private_bytes: 9,
        ..AppSettings::default()
    };
    assert!(validate_resource_settings(&s)
        .unwrap_err()
        .contains("agentGroupWarnPrivateBytes"));

    let s = AppSettings {
        agent_group_warn_private_bytes: 1,
        agent_group_kill_private_bytes: 0,
        ..AppSettings::default()
    };
    assert!(validate_resource_settings(&s)
        .unwrap_err()
        .contains("agentGroupKillPrivateBytes"));

    let s = AppSettings {
        agent_group_warn_private_bytes: 1,
        agent_group_kill_private_bytes: 10,
        agent_process_kill_private_bytes: 0,
        ..AppSettings::default()
    };
    assert!(validate_resource_settings(&s)
        .unwrap_err()
        .contains("agentProcessKillPrivateBytes"));
}

#[test]
fn validate_resource_settings_accepts_max_concurrent_above_legacy_cap() {
    // #565: the old 1..=16 upper ceiling is gone. Any value >= 1 is accepted;
    // the user is responsible for sizing it to their machine.
    for value in [17u32, 32, 64, 256, 10_000] {
        let s = AppSettings {
            max_concurrent_agent_processes: value,
            ..AppSettings::default()
        };
        assert!(
            validate_resource_settings(&s).is_ok(),
            "expected max_concurrent_agent_processes={value} to validate"
        );
    }
}

#[test]
fn validate_api_server_settings_rejects_invalid_bind_and_port() {
    let mut empty_bind = AppSettings {
        api_server_bind: "   ".to_string(),
        ..AppSettings::default()
    };
    assert!(validate_api_server_settings(&mut empty_bind)
        .unwrap_err()
        .contains("apiServerBind"));

    let mut zero_port = AppSettings {
        api_server_port: 0,
        ..AppSettings::default()
    };
    assert!(validate_api_server_settings(&mut zero_port)
        .unwrap_err()
        .contains("apiServerPort"));

    let mut hostname = AppSettings {
        api_server_bind: "localhost".to_string(),
        ..AppSettings::default()
    };
    assert!(validate_api_server_settings(&mut hostname)
        .unwrap_err()
        .contains("apiServerBind"));
}

#[test]
fn validate_api_server_settings_accepts_ip_binds_and_trims() {
    for bind in ["127.0.0.1", "0.0.0.0", "::1", "::"] {
        let mut settings = AppSettings {
            api_server_bind: format!(" {bind} "),
            ..AppSettings::default()
        };
        validate_api_server_settings(&mut settings).unwrap();
        assert_eq!(settings.api_server_bind, bind);
    }
}

#[test]
fn coord_sort_by_activity_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(!s.coord_sort_by_activity);
    s.coord_sort_by_activity = true;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"coordSortByActivity\":true"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(back.coord_sort_by_activity);
}

#[test]
fn activity_log_enabled_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(!s.activity_log_enabled);
    s.activity_log_enabled = true;
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"activityLogEnabled\":true"));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert!(back.activity_log_enabled);
}

#[test]
fn log_level_round_trips_through_serde() {
    let mut s = AppSettings::default();
    assert!(s.log_level.is_none());
    s.log_level = Some("info,agentscommander_lib::config::teams=debug".to_string());
    let json = serde_json::to_string(&s).expect("serialize");
    assert!(json.contains("\"logLevel\":\"info,agentscommander_lib::config::teams=debug\""));
    let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        back.log_level,
        Some("info,agentscommander_lib::config::teams=debug".to_string())
    );
}

#[test]
fn log_level_defaults_to_none_when_missing_from_json() {
    // Old settings.json without the new field must deserialize to None.
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
            "mainZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "mainGeometry": null,
            "mainSidebarWidth": 280.0,
            "mainSidebarSide": "right",
            "mainAlwaysOnTop": false,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false,
            "coordSortByActivity": false
        }"#;
    let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
    assert!(s.log_level.is_none());
}

#[test]
fn read_log_level_only_returns_value_when_present() {
    let dir = std::env::temp_dir().join(format!("rlol-present-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(
        &path,
        r#"{"logLevel":"info,agentscommander_lib::config::teams=debug","other":"x"}"#,
    )
    .unwrap();
    assert_eq!(
        super::super::read_log_level_from_path(&path),
        Some("info,agentscommander_lib::config::teams=debug".to_string())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_log_level_only_returns_none_when_log_level_missing() {
    let dir = std::env::temp_dir().join(format!("rlol-missing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{"other":"value"}"#).unwrap();
    assert_eq!(super::super::read_log_level_from_path(&path), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_log_level_only_returns_none_when_settings_missing() {
    let path = std::env::temp_dir().join(format!("rlol-no-such-file-{}.json", std::process::id()));
    // Intentionally do not create the file.
    assert_eq!(super::super::read_log_level_from_path(&path), None);
}

#[test]
fn read_log_level_only_returns_none_when_json_malformed() {
    let dir = std::env::temp_dir().join(format!("rlol-malformed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, "{ invalid json no closing brace").unwrap();
    assert_eq!(super::super::read_log_level_from_path(&path), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_log_level_only_returns_some_empty_string_when_log_level_is_empty() {
    // Asserts read_log_level_only returns Some("") (not None) when logLevel is the
    // empty string — the helper preserves the user's intent (the field is set, just
    // empty). Downstream filter machinery handles the rest, with semantics distinct
    // from the malformed-string case: empty-string → parse_filters("") produces 0
    // directives → env_filter's hidden {None, Error} default applies → Error-only logs
    // flow globally; malformed-string → 1 non-matching directive → all
    // agentscommander* logs suppressed. The helper is symmetric on both inputs
    // (returns Some(value)); the observable difference is at env_filter::Builder::build.
    let dir = std::env::temp_dir().join(format!("rlol-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{"logLevel":"","other":"value"}"#).unwrap();
    assert_eq!(
        super::super::read_log_level_from_path(&path),
        Some(String::new())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_activity_log_enabled_from_path_returns_true_when_true() {
    let dir = std::env::temp_dir().join(format!("rale-present-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{"activityLogEnabled":true,"other":"x"}"#).unwrap();
    assert!(super::super::read_activity_log_enabled_from_path(&path));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_activity_log_enabled_from_path_defaults_false_when_key_missing() {
    let dir = std::env::temp_dir().join(format!("rale-missing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{"other":"value"}"#).unwrap();
    assert!(!super::super::read_activity_log_enabled_from_path(&path));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_activity_log_enabled_from_path_defaults_false_when_null() {
    let dir = std::env::temp_dir().join(format!("rale-null-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{"activityLogEnabled":null}"#).unwrap();
    assert!(!super::super::read_activity_log_enabled_from_path(&path));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_activity_log_enabled_from_path_defaults_false_when_json_malformed() {
    let dir = std::env::temp_dir().join(format!("rale-malformed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, "{ invalid json no closing brace").unwrap();
    assert!(!super::super::read_activity_log_enabled_from_path(&path));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_activity_log_enabled_only_defaults_false_when_settings_missing() {
    let path = std::env::temp_dir().join(format!("rale-no-such-file-{}.json", std::process::id()));
    assert!(!super::super::read_activity_log_enabled_from_path(&path));
}
