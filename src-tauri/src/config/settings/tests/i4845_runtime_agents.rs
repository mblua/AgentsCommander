//! #4845 P14: running sessions read the agent values of the file they launched from.
//! A local `LaunchedRegistry` only: the process-wide one is never touched here.

use crate::config::coding_agent_files::launch_chain::LaunchPick;
use crate::config::coding_agent_files::launched::LaunchedRegistry;
use crate::config::coding_agent_files::{
    with_active, ConfigContext, ConfigFileLocator, ConfigScope,
};
use crate::config::settings::agent::{
    launch_view_at, record_launch_runtime_in, runtime_agents_with, LaunchPaths, LaunchView,
};
use crate::config::settings::{AgentConfig, AppSettings, QuotaSourceConfig, QuotaSourceEntry};
use serde_json::json;
use std::borrow::Cow;

const R: &str = r"  Context (\d+)%";

fn agent(id: &str, command: &str, regex: Option<&str>) -> AgentConfig {
    let mut value = json!({"id": id, "label": id, "command": command, "color": "#123"});
    if let Some(regex) = regex {
        value["contextRegex"] = json!(regex);
    }
    serde_json::from_value(value).unwrap()
}

fn team() -> ConfigFileLocator {
    ConfigFileLocator::Named {
        scope: ConfigScope::Instance,
        name: "team".into(),
    }
}

fn view(agents: Vec<AgentConfig>, locator: ConfigFileLocator) -> LaunchView<'static> {
    LaunchView {
        settings: Cow::Owned(AppSettings {
            agents,
            ..AppSettings::default()
        }),
        locator,
        pick: LaunchPick::Active,
    }
}

/// The 30 file: X runs `/opt/x` without a contextRegex.
fn settings_30() -> AppSettings {
    AppSettings {
        agents: vec![agent("x", "/opt/x", None)],
        ..AppSettings::default()
    }
}

/// A registry after X launched from `team` (X runs `claude` with R) and Y (only in team).
fn launched_from_team() -> LaunchedRegistry<AgentConfig> {
    let reg = LaunchedRegistry::new();
    let named = view(
        vec![agent("x", "claude", Some(R)), agent("y", "tool-y", None)],
        team(),
    );
    record_launch_runtime_in(&reg, &named, "x");
    record_launch_runtime_in(&reg, &named, "y");
    reg
}

fn find<'a>(agents: &'a [AgentConfig], id: &str) -> Option<&'a AgentConfig> {
    agents.iter().find(|a| a.id == id)
}

#[test]
fn empty_registry_borrows_settings_agents() {
    let settings = settings_30();
    let reg = LaunchedRegistry::new();
    let agents = runtime_agents_with(&settings, &reg);
    assert!(matches!(agents, Cow::Borrowed(a) if std::ptr::eq(a, settings.agents.as_slice())));
}

#[test]
fn non_30_launch_overlays_context_regex_and_menus() {
    let settings = settings_30();
    let reg = launched_from_team();
    let agents = runtime_agents_with(&settings, &reg);
    let x = find(&agents, "x").unwrap();
    assert_eq!(x.context_regex.as_deref(), Some(R));
    assert_eq!(x.command, "claude");
    assert_eq!(agents.iter().filter(|a| a.id == "x").count(), 1);
}

#[test]
fn agent_only_in_named_file_is_appended() {
    let settings = settings_30();
    let reg = launched_from_team();
    let ids: Vec<_> = runtime_agents_with(&settings, &reg)
        .iter()
        .map(|a| a.id.clone())
        .collect();
    assert_eq!(ids, ["x", "y"]);
}

#[test]
fn launch_from_30_removes_overlay() {
    let settings = settings_30();
    let reg = launched_from_team();
    let from_30 = view(
        settings.agents.clone(),
        ConfigFileLocator::RegisteredInstance {},
    );
    record_launch_runtime_in(&reg, &from_30, "x");
    let agents = runtime_agents_with(&settings, &reg);
    assert_eq!(find(&agents, "x").unwrap().command, "/opt/x");
    assert_eq!(reg.snapshot().len(), 1);
}

#[test]
fn temp_room_view_is_recorded() {
    let temp = tempfile::tempdir().unwrap();
    let room = temp.path().join("temp-room-tr01");
    std::fs::create_dir_all(&room).unwrap();
    let room_text = room.to_string_lossy().into_owned();
    assert!(crate::config::temp_rooms::is_temp_room_path_in(
        &room_text,
        temp.path()
    ));
    let dir = temp.path().join("coding-agents");
    std::fs::create_dir_all(&dir).unwrap();
    let file = json!({"agents": [{"id": "x", "label": "x", "command": "claude",
        "color": "#123", "contextRegex": R}]});
    std::fs::write(
        dir.join("ac.coding-agents-config.team.json"),
        file.to_string(),
    )
    .unwrap();
    let mut settings = settings_30();
    settings.coding_agent_config_files = Some(with_active(None, &ConfigContext::Instance, team()));
    let paths = LaunchPaths {
        settings_path: temp.path().join("settings.json"),
        root_agent_dir: Some(temp.path().join("root")),
    };
    let view = launch_view_at(&settings, &paths, &room);
    assert_eq!(view.locator, team());
    let reg = LaunchedRegistry::new();
    record_launch_runtime_in(&reg, &view, "x");
    let snapshot = reg.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].1.context_regex.as_deref(), Some(R));
}

#[test]
fn real_builders_read_the_runtime_list() {
    let mut settings = settings_30();
    let reg = launched_from_team();
    let agents = runtime_agents_with(&settings, &reg);
    let patterns = crate::pty_wiring::context_patterns(&agents);
    assert_eq!(patterns.get("x").map(String::as_str), Some(R));

    let entry = QuotaSourceEntry::Valid(QuotaSourceConfig::ScreenRegex {
        pattern: r"Weekly (\d{1,3})% used".into(),
        enabled: true,
    });
    settings.quota_sources.insert("x".into(), entry);
    let agents = runtime_agents_with(&settings, &reg);
    let quota = crate::pty_wiring::QuotaSources::sources_map_for(&settings, &agents);
    assert_eq!(
        quota.get("x").map(|s| s.account_key.as_str()),
        Some("claude")
    );

    let watchers = crate::pty_wiring::watcher_agents(&settings, &agents);
    let x = watchers.iter().find(|w| w.id == "x").unwrap();
    assert_eq!(x.command, "claude");
}
