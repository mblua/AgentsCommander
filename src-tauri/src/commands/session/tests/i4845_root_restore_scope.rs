//! #4845 P13: the root agent, restore and running-session lookups read the agent's own
//! file. Tempdir only: instance cases use `launch_view_at` with tempdir `LaunchPaths`;
//! production entries see only project-scope named files under a tempdir project root.

use super::root_agent::make_info;
use super::{compute_profile_outdated, resolve_root_agent_command};
use crate::config::agent_command::build_configured_agent_spawn_for_cwd;
use crate::config::agent_config::set_last_coding_agent_with_source;
use crate::config::coding_agent_files::locator::context_key;
use crate::config::coding_agent_files::{
    with_active, ConfigContext, ConfigFileLocator, ConfigScope,
};
use crate::config::settings::agent::{launch_view, launch_view_at, LaunchPaths};
use crate::config::settings::{AgentConfig, AppSettings};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn agent(id: &str, command: &str) -> Value {
    json!({"id": id, "label": id, "command": command, "color": "#123"})
}

fn named(scope: ConfigScope, name: &str) -> ConfigFileLocator {
    ConfigFileLocator::Named {
        scope,
        name: name.into(),
    }
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn write_file(dir: &Path, name: &str, agents: Value) {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("ac.coding-agents-config.{name}.json"));
    std::fs::write(path, json!({ "agents": agents }).to_string()).unwrap();
}

fn launch(dir: &Path, id: &str, source: &ConfigFileLocator) {
    let source = serde_json::to_value(source).unwrap();
    set_last_coding_agent_with_source(&s(dir), id, id, None, None, Some(&source)).unwrap();
}

fn settings_30(command: &str) -> AppSettings {
    AppSettings {
        agents: vec![serde_json::from_value::<AgentConfig>(agent("claude", command)).unwrap()],
        ..AppSettings::default()
    }
}

struct Root {
    _temp: tempfile::TempDir,
    paths: LaunchPaths,
    root: PathBuf,
    settings: AppSettings,
}

/// Tempdir paths; instance named `solo` = [codex] is the Outside projects active file.
fn root_with_named_active() -> Root {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let paths = LaunchPaths {
        settings_path: temp.path().join("settings.json"),
        root_agent_dir: Some(root.clone()),
    };
    write_file(
        &temp.path().join("coding-agents"),
        "solo",
        json!([agent("codex", "tool")]),
    );
    let mut settings = settings_30("tool");
    let active = named(ConfigScope::Instance, "solo");
    settings.coding_agent_config_files = Some(with_active(None, &ConfigContext::Instance, active));
    Root {
        _temp: temp,
        paths,
        root,
        settings,
    }
}

fn root_agent_id(r: &Root, last: Option<&str>) -> Option<String> {
    let view = launch_view_at(&r.settings, &r.paths, &r.root);
    resolve_root_agent_command(&view.settings, None, last)
        .unwrap()
        .2
}

#[test]
fn root_agent_uses_last_used_from_30_when_outside_projects_active_is_named() {
    let r = root_with_named_active();
    launch(&r.root, "claude", &ConfigFileLocator::RegisteredInstance {});
    assert_eq!(root_agent_id(&r, Some("claude")).as_deref(), Some("claude"));
}

#[test]
fn root_agent_uses_outside_projects_active_file_when_never_started() {
    let r = root_with_named_active();
    assert_eq!(root_agent_id(&r, None).as_deref(), Some("codex"));
}

#[test]
fn root_agent_context_is_instance() {
    let mut r = root_with_named_active();
    // A project root that contains the config dir and the root agent dir, with its own
    // active file: the root agent still reads the Outside projects file.
    let project = r.paths.settings_path.parent().unwrap().to_path_buf();
    write_file(
        &project.join(".ac").join("coding-agents"),
        "team",
        json!([agent("other", "tool")]),
    );
    r.settings.project_paths = vec![s(&project)];
    let ctx = ConfigContext::Project(context_key(&s(&project)));
    let sel = r.settings.coding_agent_config_files.as_ref();
    let sel = with_active(sel, &ctx, named(ConfigScope::Project, "team"));
    r.settings.coding_agent_config_files = Some(sel);
    let view = launch_view_at(&r.settings, &r.paths, &r.root);
    assert_eq!(view.locator, named(ConfigScope::Instance, "solo"));
    assert_eq!(root_agent_id(&r, None).as_deref(), Some("codex"));
}

struct Proj {
    _temp: tempfile::TempDir,
    root: PathBuf,
    agent_dir: PathBuf,
    settings: AppSettings,
}

/// A tempdir project root with `team` = [claude "tool-team"]; the 30 file = [claude "tool"].
fn proj() -> Proj {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("proj");
    let agent_dir = root.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let dir = root.join(".ac").join("coding-agents");
    write_file(&dir, "team", json!([agent("claude", "tool-team")]));
    let mut settings = settings_30("tool");
    settings.project_paths = vec![s(&root)];
    Proj {
        _temp: temp,
        root,
        agent_dir,
        settings,
    }
}

impl Proj {
    fn activate_team(&mut self) {
        let ctx = ConfigContext::Project(context_key(&s(&self.root)));
        let team = named(ConfigScope::Project, "team");
        self.settings.coding_agent_config_files = Some(with_active(None, &ctx, team));
    }

    fn spawn(&self) -> crate::config::agent_command::AgentSpawnCommand {
        build_configured_agent_spawn_for_cwd(&self.settings, "claude", &s(&self.agent_dir), None)
            .unwrap()
            .unwrap()
    }
}

#[test]
fn restore_recipe_uses_recorded_source() {
    let p = proj();
    launch(&p.agent_dir, "claude", &named(ConfigScope::Project, "team"));
    assert_eq!(p.spawn().shell, "tool-team");
}

#[test]
fn profile_outdated_ignores_active_file_switch() {
    let mut p = proj();
    launch(
        &p.agent_dir,
        "claude",
        &ConfigFileLocator::RegisteredInstance {},
    );
    let loaded = p.spawn().profile_content_hash;
    let info = make_info(&s(&p.agent_dir), Some("claude"), None, Some(loaded));
    p.activate_team();
    let view = launch_view(&p.settings, &p.agent_dir);
    assert!(!compute_profile_outdated(&view.settings, &info));
}
