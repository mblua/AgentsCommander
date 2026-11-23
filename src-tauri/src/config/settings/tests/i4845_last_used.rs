//! #4845 P09: recorded launch sources and the last-used setup of an agent dir.
//! Every test uses tempdir paths; nothing touches the real config dir.

use crate::config::agent_config::{
    read_agent_local_config_json, recorded_launch, set_last_coding_agent,
    set_last_coding_agent_with_source,
};
use crate::config::coding_agent_files::last_used::{
    matches_source, recorded_sources, FileAgentKey, RecordedSource,
};
use crate::config::coding_agent_files::locator::context_key;
use crate::config::coding_agent_files::{
    with_active, ConfigContext, ConfigFileLocator, ConfigScope,
};
use crate::config::settings::agent::{
    last_used_at, launch_context, LastUsed, LastUsedGone, LaunchPaths,
};
use crate::config::settings::{AgentConfig, AppSettings};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn named(name: &str) -> ConfigFileLocator {
    ConfigFileLocator::Named {
        scope: ConfigScope::Instance,
        name: name.into(),
    }
}

fn agent(id: &str, label: &str) -> AgentConfig {
    serde_json::from_value(json!({"id": id, "label": label, "command": "tool", "color": "#123"}))
        .unwrap()
}

fn key(id: &str, letters: &[&str]) -> FileAgentKey {
    FileAgentKey {
        id: id.into(),
        letters: letters.iter().map(ToString::to_string).collect(),
    }
}

fn rec(id: &str, letter: Option<&str>) -> RecordedSource {
    RecordedSource {
        agent_id: id.into(),
        letter: letter.map(ToString::to_string),
        locator: Ok(ConfigFileLocator::RegisteredInstance {}),
    }
}

// ---- pure ----

#[test]
fn choice_needs_source_agent_and_source_file() {
    let config = json!({"tooling": {"currentCodingAgent": "claude"}});
    let launch = recorded_launch(&config);
    let sources = recorded_sources(
        launch
            .choice_agent
            .as_deref()
            .zip(launch.choice_file.as_ref()),
        None,
        None,
    );
    assert!(sources.choice.is_none());
    let file = serde_json::to_value(named("team")).unwrap();
    let sources = recorded_sources(Some(("claude", &file)), None, None);
    let choice = sources.choice.unwrap();
    assert_eq!(choice.agent_id, "claude");
    assert_eq!(choice.locator, Ok(named("team")));
}

#[test]
fn legacy_launch_without_source_is_registered_instance() {
    let sources = recorded_sources(None, Some(("claude", None)), Some("A"));
    let last = sources.last.unwrap();
    assert_eq!(last.locator, Ok(ConfigFileLocator::RegisteredInstance {}));
    assert_eq!(last.letter.as_deref(), Some("A"));
}

#[test]
fn bad_stored_locator_is_err() {
    let bad = json!({"kind": "nope"});
    let sources = recorded_sources(Some(("a", &bad)), Some(("a", Some(&bad))), None);
    assert_eq!(sources.choice.unwrap().locator, Err(()));
    assert_eq!(sources.last.unwrap().locator, Err(()));
}

#[test]
fn matches_same_id_even_when_command_changed() {
    assert!(matches_source(
        &rec("claude", Some("A")),
        &[key("claude", &["A"])]
    ));
    assert!(!matches_source(&rec("claude", None), &[key("codex", &[])]));
}

#[test]
fn two_rows_with_same_command_match_only_by_id() {
    let keys = [key("claude", &[]), key("claude-opus", &[])];
    assert!(matches_source(&rec("claude-opus", None), &keys));
    assert!(!matches_source(&rec("claude-sonnet", None), &keys));
    assert!(!matches_source(&rec("claude-opus", None), &keys[..1]));
}

#[test]
fn missing_letter_does_not_match() {
    assert!(!matches_source(
        &rec("claude", Some("B")),
        &[key("claude", &["A"])]
    ));
}

#[test]
fn agent_without_cells_matches_any_letter() {
    assert!(matches_source(
        &rec("claude", Some("Z")),
        &[key("claude", &[])]
    ));
}

// ---- glue ----

struct Fixture {
    _temp: tempfile::TempDir,
    paths: LaunchPaths,
    agent_dir: PathBuf,
    settings: AppSettings,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let agent_dir = temp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let paths = LaunchPaths {
        settings_path: temp.path().join("settings.json"),
        root_agent_dir: Some(temp.path().join("root")),
    };
    let settings = AppSettings {
        agents: vec![agent("claude", "Claude")],
        ..AppSettings::default()
    };
    Fixture {
        _temp: temp,
        paths,
        agent_dir,
        settings,
    }
}

impl Fixture {
    fn named_file(&self, name: &str, agents: Value) {
        let path = self
            .paths
            .settings_path
            .with_file_name("coding-agents")
            .join(format!("ac.coding-agents-config.{name}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, json!({ "agents": agents }).to_string()).unwrap();
    }

    fn launch(&self, id: &str, source: Option<&ConfigFileLocator>) {
        let source = source.map(|loc| serde_json::to_value(loc).unwrap());
        let dir = self.agent_dir.to_string_lossy();
        set_last_coding_agent_with_source(&dir, id, id, None, None, source.as_ref()).unwrap();
    }

    fn choose(&self, id: &str, file: &ConfigFileLocator) {
        let file = serde_json::to_value(file).unwrap();
        let config = json!({"tooling": {"currentSourceAgent": id, "currentSourceFile": file}});
        std::fs::write(self.agent_dir.join("config.json"), config.to_string()).unwrap();
    }

    fn activate(&mut self, loc: ConfigFileLocator) {
        let sel = with_active(None, &ConfigContext::Instance, loc);
        self.settings.coding_agent_config_files = Some(sel);
    }

    fn last_used(&self) -> LastUsed {
        last_used_at(&self.settings, &self.paths, &self.agent_dir).1
    }
}

fn file_agent(id: &str, label: &str) -> Value {
    json!({"id": id, "label": label, "command": "tool", "color": "#123"})
}

fn available(state: LastUsed) -> (ConfigFileLocator, String, String, bool) {
    match state {
        LastUsed::Available {
            locator,
            agent_id,
            label,
            in_active_file,
            ..
        } => (locator, agent_id, label, in_active_file),
        _ => panic!("expected Available"),
    }
}

fn gone(state: LastUsed) -> LastUsedGone {
    match state {
        LastUsed::Unavailable { reason, .. } => reason,
        _ => panic!("expected Unavailable"),
    }
}

#[test]
fn never_started_is_none() {
    let f = fixture();
    assert!(matches!(f.last_used(), LastUsed::None));
}

#[test]
fn available_in_other_file_reports_not_in_active() {
    let mut f = fixture();
    f.named_file("team", json!([file_agent("codex", "Codex")]));
    f.activate(named("team"));
    f.launch("claude", Some(&ConfigFileLocator::RegisteredInstance {}));
    let (loc, id, label, in_active) = available(f.last_used());
    assert_eq!(loc, ConfigFileLocator::RegisteredInstance {});
    assert_eq!((id.as_str(), label.as_str()), ("claude", "Claude"));
    assert!(!in_active);
}

#[test]
fn available_in_active_file_reports_in_active() {
    let f = fixture();
    f.launch("claude", Some(&ConfigFileLocator::RegisteredInstance {}));
    let (_, _, _, in_active) = available(f.last_used());
    assert!(in_active);
}

#[test]
fn same_id_in_active_copy_is_not_in_active() {
    let mut f = fixture();
    f.named_file("copy", json!([file_agent("claude", "Claude")]));
    f.activate(named("copy"));
    f.launch("claude", Some(&ConfigFileLocator::RegisteredInstance {}));
    let (loc, _, _, in_active) = available(f.last_used());
    assert_eq!(loc, ConfigFileLocator::RegisteredInstance {});
    assert!(!in_active);
}

#[test]
fn removed_row_is_unavailable_removed() {
    let f = fixture();
    f.launch("gone", Some(&ConfigFileLocator::RegisteredInstance {}));
    assert!(matches!(gone(f.last_used()), LastUsedGone::Removed));
}

#[test]
fn edited_row_is_still_available() {
    let mut f = fixture();
    f.launch("claude", Some(&ConfigFileLocator::RegisteredInstance {}));
    f.settings.agents[0].command = "other --flag".into();
    f.settings.agents[0].envs =
        serde_json::from_value(json!([{"key": "K", "value": "v"}])).unwrap();
    let (_, id, _, _) = available(f.last_used());
    assert_eq!(id, "claude");
}

#[test]
fn deleted_file_is_unavailable_file_gone() {
    let f = fixture();
    f.launch("claude", Some(&named("team")));
    assert!(matches!(gone(f.last_used()), LastUsedGone::FileGone));
}

#[test]
fn choice_wins_over_last_launch() {
    let f = fixture();
    f.named_file("team", json!([file_agent("same", "Own")]));
    f.choose("same", &named("team"));
    f.launch("claude", Some(&ConfigFileLocator::RegisteredInstance {}));
    let (loc, id, _, _) = available(f.last_used());
    assert_eq!((loc, id.as_str()), (named("team"), "same"));
}

#[test]
fn unavailable_choice_falls_to_available_last_launch() {
    let f = fixture();
    f.choose("same", &named("missing"));
    f.launch("claude", Some(&ConfigFileLocator::RegisteredInstance {}));
    let (loc, id, _, _) = available(f.last_used());
    assert_eq!(
        (loc, id.as_str()),
        (ConfigFileLocator::RegisteredInstance {}, "claude")
    );
}

#[test]
fn recorded_launch_reads_state_keys_only_in_owner() {
    let f = fixture();
    f.launch("claude", Some(&named("team")));
    let config = read_agent_local_config_json(&f.agent_dir).unwrap().unwrap();
    let launch = recorded_launch(&config);
    assert_eq!(launch.last_agent.as_deref(), Some("claude"));
    assert_eq!(
        launch.last_source,
        Some(serde_json::to_value(named("team")).unwrap())
    );
    assert!(launch.choice_agent.is_none() && launch.choice_file.is_none());
}

#[test]
fn choice_with_same_id_as_30_is_found_in_its_own_file() {
    let f = fixture();
    f.named_file("team", json!([file_agent("claude", "Own")]));
    f.choose("claude", &named("team"));
    let (loc, _, label, in_active) = available(f.last_used());
    assert_eq!((loc, label.as_str()), (named("team"), "Own"));
    assert!(!in_active);
}

#[test]
fn matrix_default_only_is_none() {
    let f = fixture();
    let config = json!({"tooling": {"currentCodingAgent": "claude"}});
    std::fs::write(f.agent_dir.join("config.json"), config.to_string()).unwrap();
    assert!(matches!(f.last_used(), LastUsed::None));
}

fn dir_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn root_agent_dir_is_instance_even_inside_a_project() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let settings = AppSettings {
        project_paths: vec![dir_str(temp.path())],
        ..AppSettings::default()
    };
    let ctx = launch_context(&settings, Some(&root), &root);
    assert_eq!(ctx, ConfigContext::Instance);
    let other = temp.path().join("other");
    let ctx = launch_context(&settings, Some(&root), &other);
    assert_eq!(
        ctx,
        ConfigContext::Project(context_key(&dir_str(temp.path())))
    );
}

#[test]
fn root_dir_none_uses_longest_root() {
    let temp = tempfile::tempdir().unwrap();
    let inner = temp.path().join("a");
    let settings = AppSettings {
        project_paths: vec![dir_str(temp.path()), dir_str(&inner)],
        ..AppSettings::default()
    };
    let ctx = launch_context(&settings, None, &inner.join("b"));
    assert_eq!(ctx, ConfigContext::Project(context_key(&dir_str(&inner))));
}

#[test]
fn old_writer_removes_source_file() {
    let f = fixture();
    f.launch("claude", Some(&named("team")));
    let dir = dir_str(&f.agent_dir);
    set_last_coding_agent(&dir, "claude", "claude", None, None).unwrap();
    let config = read_agent_local_config_json(&f.agent_dir).unwrap().unwrap();
    assert_eq!(recorded_launch(&config).last_source, None);
}
