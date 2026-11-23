//! #4845 D8: whole-settings saves keep the coding-agent config-file selection.

use super::{
    persist_protected_settings_update_with_saver, persist_settings_draft_update_with_saver,
    state_for,
};
use crate::commands::config::agent_file_source::{
    get_scoped_coding_agents_inner, list_coding_agent_config_files_inner,
    load_coding_agent_config_file_inner, save_as_coding_agent_config_file_inner,
    set_active_coding_agent_config_file_inner,
};
use crate::config::coding_agent_files::{
    with_active, with_loaded, CatalogLayer, CodingAgentConfigFiles, ConfigContext,
    ConfigFileLocator, ConfigScope,
};
use crate::config::settings::{
    load_settings_from_path, save_settings_to_path_preserving_project_paths, AgentConfig,
    AppSettings, CodingAgentProfilesConfig, SettingsOwner, SettingsState, SourceAvailability,
    SourceKind,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn seed(dir: &Path) -> PathBuf {
    let path = dir.join("settings.json");
    let json = serde_json::to_vec_pretty(&AppSettings::default()).unwrap();
    std::fs::write(&path, json).unwrap();
    path
}

fn stored_selection(path: &Path) -> Option<CodingAgentConfigFiles> {
    let raw: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    raw.get("codingAgentConfigFiles")
        .cloned()
        .map(|value| serde_json::from_value(value).unwrap())
}

fn selections() -> [CodingAgentConfigFiles; 2] {
    let named = ConfigFileLocator::Named {
        scope: ConfigScope::Instance,
        name: "team".into(),
    };
    let loaded_path = "/somewhere/agents.json";
    let loaded = with_loaded(None, loaded_path);
    [
        with_active(None, &ConfigContext::Instance, named),
        with_active(
            Some(&loaded),
            &ConfigContext::Instance,
            ConfigFileLocator::Loaded {
                path: loaded_path.into(),
            },
        ),
    ]
}

async fn assert_kept(draft: bool, selection: CodingAgentConfigFiles) {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(temp.path());
    let old_snapshot = load_settings_from_path(&path);
    assert!(old_snapshot.coding_agent_config_files.is_none());
    let mut live = old_snapshot.clone();
    live.coding_agent_config_files = Some(selection.clone());
    let state = state_for(live);
    let save_path = path.clone();
    let save = move |candidate: &AppSettings| {
        save_settings_to_path_preserving_project_paths(candidate, &save_path)
    };
    if draft {
        persist_settings_draft_update_with_saver(&state, old_snapshot, save)
            .await
            .unwrap();
    } else {
        persist_protected_settings_update_with_saver(&state, old_snapshot, save)
            .await
            .unwrap();
    }
    assert_eq!(
        state.read().await.coding_agent_config_files.as_ref(),
        Some(&selection)
    );
    assert_eq!(stored_selection(&path), Some(selection));
}

#[tokio::test]
async fn whole_settings_save_keeps_selection() {
    for selection in selections() {
        assert_kept(false, selection.clone()).await;
        assert_kept(true, selection).await;
    }
}

// #4845 P02: config-file commands on both transports. Tempdir settings path only (D15).

struct Fixture {
    _temp: tempfile::TempDir,
    dir: PathBuf,
    path: PathBuf,
    owner: SettingsState,
    projects: Vec<String>,
}

fn fixture(project_count: usize) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("instance");
    std::fs::create_dir_all(&dir).unwrap();
    let projects: Vec<String> = (0..project_count)
        .map(|index| {
            let root = temp.path().join(format!("project{index}"));
            std::fs::create_dir_all(root.join(".ac")).unwrap();
            root.to_string_lossy().into_owned()
        })
        .collect();
    let path = crate::config::settings::settings_fixture_path(&dir);
    let settings = AppSettings {
        project_paths: projects.clone(),
        ..AppSettings::default()
    };
    std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let registered = json!({"agents":[
        {"id":"base","label":"Base","color":"red","baseCommand":"tool"}
    ]});
    std::fs::write(
        dir.join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
        serde_json::to_vec(&registered).unwrap(),
    )
    .unwrap();
    let owner = SettingsOwner::open_active(&path, Duration::from_secs(1)).unwrap();
    Fixture {
        _temp: temp,
        dir,
        path,
        owner,
        projects,
    }
}

fn agents(id: &str) -> Vec<AgentConfig> {
    serde_json::from_value(json!([
        {"id": id, "label": id, "color": "blue", "baseCommand": "tool"}
    ]))
    .unwrap()
}

fn raw(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn disk_selection(path: &Path) -> CodingAgentConfigFiles {
    serde_json::from_value(raw(path)["codingAgentConfigFiles"].clone()).unwrap()
}

fn named(scope: ConfigScope, name: &str) -> ConfigFileLocator {
    ConfigFileLocator::Named {
        scope,
        name: name.into(),
    }
}

async fn save_as(f: &Fixture, context: Option<String>, scope: ConfigScope, name: &str) {
    save_as_coding_agent_config_file_inner(
        &f.owner,
        &f.path,
        context,
        scope,
        name.into(),
        agents(name),
        CodingAgentProfilesConfig::default(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn save_as_sets_active() {
    let f = fixture(0);
    save_as(&f, None, ConfigScope::Instance, "team").await;
    let team = named(ConfigScope::Instance, "team");
    assert_eq!(disk_selection(&f.path).active_by_context["instance"], team);
    let list = list_coding_agent_config_files_inner(&f.owner, &f.path, None)
        .await
        .unwrap();
    assert_eq!(list.active, team);
    assert!(list.active_notice.is_none());
    let scoped = get_scoped_coding_agents_inner(&f.owner, &f.path, None, None)
        .await
        .unwrap();
    assert_eq!(scoped.agents[0].id, "team");
    assert!(scoped.notice.is_none());
}

#[tokio::test]
async fn set_active_rejects_other_project_file() {
    let f = fixture(2);
    let (first, second) = (f.projects[0].clone(), f.projects[1].clone());
    save_as(&f, Some(first), ConfigScope::Project, "mine").await;
    let before = std::fs::read(&f.path).unwrap();
    let error = set_active_coding_agent_config_file_inner(
        &f.owner,
        &f.path,
        Some(second),
        named(ConfigScope::Project, "mine"),
    )
    .await
    .unwrap_err();
    assert!(error.starts_with("configFileUnavailable:"), "{error}");
    assert_eq!(std::fs::read(&f.path).unwrap(), before);
}

#[tokio::test]
async fn load_invalid_file_changes_nothing() {
    let f = fixture(0);
    let bad = f.dir.join("bad.json");
    std::fs::write(&bad, b"{\"agents\": 3}").unwrap();
    let before = std::fs::read(&f.path).unwrap();
    let live = f
        .owner
        .current_snapshot()
        .await
        .unwrap()
        .coding_agent_config_files;
    let path = bad.to_string_lossy().into_owned();
    let error = load_coding_agent_config_file_inner(&f.owner, &f.path, None, path)
        .await
        .unwrap_err();
    assert!(error.starts_with("configFileInvalid:"), "{error}");
    assert_eq!(std::fs::read(&f.path).unwrap(), before);
    let after = f.owner.current_snapshot().await.unwrap();
    assert_eq!(after.coding_agent_config_files, live);
}

#[tokio::test]
async fn load_sets_active_loaded_file() {
    let f = fixture(0);
    let file = f.dir.join("external-agents.json");
    let body = json!({"agents":[{"id":"ext","label":"Ext","color":"red","baseCommand":"x"}]});
    std::fs::write(&file, serde_json::to_vec(&body).unwrap()).unwrap();
    let canonical = crate::filesystem::settings_sources::canonical(&file)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let path = file.to_string_lossy().into_owned();
    let list = load_coding_agent_config_file_inner(&f.owner, &f.path, None, path)
        .await
        .unwrap();
    let loaded = ConfigFileLocator::Loaded {
        path: canonical.clone(),
    };
    assert_eq!(list.active, loaded);
    let selection = disk_selection(&f.path);
    assert_eq!(selection.loaded, vec![canonical]);
    assert_eq!(selection.active_by_context["instance"], loaded);
    let live = f
        .owner
        .current_snapshot()
        .await
        .unwrap()
        .coding_agent_config_files;
    assert_eq!(live, Some(selection));
}

#[tokio::test]
async fn set_active_accepts_catalog_layer() {
    let f = fixture(0);
    let catalog = crate::config::coding_agents_catalog::independent_catalog_path(
        &f.dir,
        SourceKind::CatalogPersonal,
    )
    .unwrap();
    std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    let body = json!({"schemaVersion":1,
        "agents":[{"key":"cat","label":"Cat","color":"blue","baseCommand":"tool"}]});
    std::fs::write(&catalog, serde_json::to_vec(&body).unwrap()).unwrap();
    let layer = ConfigFileLocator::Layer {
        scope: ConfigScope::Instance,
        layer: CatalogLayer::CatalogPersonal,
    };
    let list = set_active_coding_agent_config_file_inner(&f.owner, &f.path, None, layer.clone())
        .await
        .unwrap();
    assert_eq!(list.active, layer);
    assert_eq!(disk_selection(&f.path).active_by_context["instance"], layer);
}

#[tokio::test]
async fn list_orders_groups_and_marks_read_only() {
    let f = fixture(1);
    let project = f.projects[0].clone();
    save_as(&f, None, ConfigScope::Instance, "solo").await;
    save_as(&f, Some(project.clone()), ConfigScope::Project, "shared").await;
    let list = list_coding_agent_config_files_inner(&f.owner, &f.path, Some(project))
        .await
        .unwrap();
    assert_eq!(list.contexts.len(), 2);
    assert_eq!(list.contexts[0].name, "Outside projects");
    assert_eq!(list.contexts[1].name, "project0");
    assert_eq!(
        list.entries[0].locator,
        ConfigFileLocator::RegisteredInstance {}
    );
    let groups: Vec<_> = list.entries.iter().map(|entry| entry.group).collect();
    let first_project = groups.iter().position(|g| *g == "project").unwrap();
    assert!(groups[..first_project].iter().all(|g| *g == "instance"));
    assert!(groups[first_project..].iter().all(|g| *g == "project"));
    let solo = named(ConfigScope::Instance, "solo");
    let shared = named(ConfigScope::Project, "shared");
    for entry in &list.entries {
        let low =
            entry.file_name.starts_with("agents.10.") || entry.file_name.starts_with("agents.40.");
        assert_eq!(entry.read_only, low, "{}", entry.file_name);
        assert_eq!(
            entry.activatable,
            entry.availability == SourceAvailability::Available
        );
    }
    assert!(list
        .entries
        .iter()
        .any(|e| e.locator == solo && e.agent_count == Some(1)));
    assert!(list
        .entries
        .iter()
        .any(|e| e.locator == shared && e.group == "project"));
    assert_eq!(list.active, shared);
}

#[tokio::test]
async fn set_active_publishes_selection() {
    let f = fixture(0);
    save_as(&f, None, ConfigScope::Instance, "one").await;
    save_as(&f, None, ConfigScope::Instance, "two").await;
    let before = raw(&f.path);
    let one = named(ConfigScope::Instance, "one");
    set_active_coding_agent_config_file_inner(&f.owner, &f.path, None, one.clone())
        .await
        .unwrap();
    let after = raw(&f.path);
    let selection = disk_selection(&f.path);
    assert_eq!(selection.active_by_context["instance"], one);
    let (before, after) = (before.as_object().unwrap(), after.as_object().unwrap());
    assert_eq!(before.len(), after.len());
    for (key, value) in before
        .iter()
        .filter(|(key, _)| *key != "codingAgentConfigFiles")
    {
        assert_eq!(after.get(key), Some(value), "{key}");
    }
    let live = f
        .owner
        .current_snapshot()
        .await
        .unwrap()
        .coding_agent_config_files;
    assert_eq!(live, Some(selection));
}

fn ws_state(owner: SettingsState) -> crate::web::commands::WsState {
    use crate::pty::manager::PtyManager;
    use std::sync::{Arc, Mutex};
    let app = crate::test_support::test_builder()
        .manage(crate::session::warnings::new_session_warning_state())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let handle = app.handle().clone();
    let broadcaster = crate::web::broadcast::WsBroadcaster::new();
    let sessions = Arc::new(tokio::sync::RwLock::new(
        crate::session::manager::SessionManager::new(),
    ));
    let idle = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let git = crate::pty::git_watcher::GitWatcher::new(Arc::clone(&sessions), handle.clone());
    let pty = Arc::new(Mutex::new(PtyManager::new(
        Arc::new(Mutex::new(std::collections::HashMap::new())),
        idle,
        git,
        Some(broadcaster.clone()),
        None,
    )));
    crate::web::commands::WsState::new(sessions, pty, owner, broadcaster, handle)
}

/// One test: a second mock app in parallel fails to own the GTK main context on Linux.
#[tokio::test]
async fn web_dispatch_config_file_commands() {
    let f = fixture(0);
    let state = ws_state(f.owner.clone());
    web_round_trips(&f, &state).await;
    web_rejects_load_and_bad_locators(&f, &state).await;
}

async fn web_round_trips(f: &Fixture, state: &crate::web::commands::WsState) {
    use crate::web::commands::config_file_command;
    let profiles = json!({});
    let created = config_file_command(
        state,
        &f.path,
        "save_as_coding_agent_config_file",
        &json!({"context": null, "scope": "instance", "name": "web",
            "agents": agents("web"), "codingAgentProfiles": profiles}),
    )
    .await
    .unwrap();
    let locator = created["locator"].clone();
    assert_eq!(
        locator,
        json!({"kind": "named", "scope": "instance", "name": "web"})
    );
    let read = config_file_command(
        state,
        &f.path,
        "read_coding_agent_config_file",
        &json!({"context": null, "locator": locator}),
    )
    .await
    .unwrap();
    assert_eq!(read["readOnly"], json!(false));
    let saved = config_file_command(
        state,
        &f.path,
        "save_coding_agent_config_file",
        &json!({"context": null, "locator": locator, "expectedRevision": read["revision"],
            "agents": agents("web2"), "codingAgentProfiles": profiles}),
    )
    .await
    .unwrap();
    assert_eq!(saved["agents"][0]["id"], json!("web2"));
    let registered = json!({"kind": "registeredInstance"});
    let list = config_file_command(
        state,
        &f.path,
        "set_active_coding_agent_config_file",
        &json!({"context": null, "locator": registered}),
    )
    .await
    .unwrap();
    assert_eq!(list["active"], registered);
    let list = config_file_command(state, &f.path, "list_coding_agent_config_files", &json!({}))
        .await
        .unwrap();
    assert_eq!(list["entries"][0]["group"], json!("instance"));
    let scoped = config_file_command(
        state,
        &f.path,
        "get_scoped_coding_agents",
        &json!({"context": null, "locator": locator}),
    )
    .await
    .unwrap();
    assert_eq!(
        scoped["fileName"],
        saved["path"].as_str().map(file_name).unwrap()
    );
}

fn file_name(path: &str) -> Value {
    json!(Path::new(path).file_name().unwrap().to_string_lossy())
}

async fn web_rejects_load_and_bad_locators(f: &Fixture, state: &crate::web::commands::WsState) {
    use crate::web::commands::config_file_command;
    let reply = crate::web::commands::dispatch(
        state,
        1,
        "load_coding_agent_config_file",
        &json!({"context": null, "path": "/x/agents.json"}),
    )
    .await;
    let error = reply["error"].as_str().unwrap();
    assert!(error.starts_with("Unknown command"), "{error}");
    let read = |locator: Value| {
        let path = f.path.clone();
        async move {
            config_file_command(
                state,
                &path,
                "read_coding_agent_config_file",
                &json!({"context": null, "locator": locator}),
            )
            .await
            .unwrap_err()
        }
    };
    let error = read(json!({"kind": "loaded", "path": "/x/agents.json"})).await;
    assert!(error.starts_with("configFileUnknown:"), "{error}");
    let error = read(json!({"kind": "other", "scope": "instance", "fileName": "../x.json"})).await;
    assert!(error.starts_with("configLocatorInvalid:"), "{error}");
}
