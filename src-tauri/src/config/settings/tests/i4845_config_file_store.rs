//! #4845 P17: config-file read, save, create, register and set-active glue.
//! Every test uses tempdir paths; nothing touches the real config dir.

use crate::config::coding_agent_files::{
    with_active, CatalogLayer, CodingAgentConfigFiles, ConfigContext, ConfigFileLocator,
    ConfigScope,
};
use crate::config::settings::agent::file::{
    config_dirs, create_config_file, list_config_files, read_config_file, register_loaded,
    save_config_file, set_active_config_file,
};
use crate::config::settings::{AgentConfig, AppSettings, SourceAvailability, SourceRevision};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const FIXTURE: &str = include_str!("fixtures/i4845_named_config.json");

fn named(name: &str) -> ConfigFileLocator {
    ConfigFileLocator::Named {
        scope: ConfigScope::Instance,
        name: name.into(),
    }
}

fn named_path(settings_path: &Path, name: &str) -> PathBuf {
    settings_path
        .with_file_name("coding-agents")
        .join(format!("ac.coding-agents-config.{name}.json"))
}

fn setup() -> (tempfile::TempDir, PathBuf, AppSettings) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    (temp, path, AppSettings::default())
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn disk(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn agent(id: &str) -> AgentConfig {
    serde_json::from_value(json!({"id": id, "label": id, "command": "tool", "color": "#123456"}))
        .unwrap()
}

fn select(settings: &mut AppSettings, loc: ConfigFileLocator) {
    settings.coding_agent_config_files = Some(with_active(None, &ConfigContext::Instance, loc));
}

#[test]
fn read_named_fixture_round_trips_profiles_by_base_hash() {
    let (_temp, path, settings) = setup();
    let file = named_path(&path, "team");
    write(&file, FIXTURE.as_bytes());
    let ctx = ConfigContext::Instance;
    let before = read_config_file(&settings, &path, &ctx, &named("team")).unwrap();
    assert_eq!(before.availability, SourceAvailability::Available);
    assert_eq!(before.agents.len(), 1);
    let saved = save_config_file(
        &settings,
        &path,
        &ctx,
        &named("team"),
        &before.revision,
        before.agents.clone(),
        before.coding_agent_profiles.clone(),
    )
    .unwrap();
    assert_eq!(saved.availability, SourceAvailability::Available);
    assert_eq!(
        serde_json::to_value(&saved.agents).unwrap(),
        serde_json::to_value(&before.agents).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&saved.coding_agent_profiles).unwrap(),
        serde_json::to_value(&before.coding_agent_profiles).unwrap()
    );
    let raw = disk(&file);
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(raw["unknownTop"], fixture["unknownTop"]);
    assert_eq!(
        raw["codingAgentProfiles"]["profileSlots"]["A"]["note"],
        json!("x")
    );
    assert_eq!(
        raw["codingAgentProfiles"]["profilesByBaseHash"],
        fixture["codingAgentProfiles"]["profilesByBaseHash"]
    );
    let profiles = raw["codingAgentProfiles"].as_object().unwrap();
    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
        assert!(!profiles.contains_key(key), "ID-view key {key} on disk");
    }
}

#[test]
fn registered_instance_is_available_when_30_file_is_missing() {
    let (_temp, path, mut settings) = setup();
    settings.agents = vec![agent("live")];
    let loc = ConfigFileLocator::RegisteredInstance {};
    let snapshot = read_config_file(&settings, &path, &ConfigContext::Instance, &loc).unwrap();
    assert_eq!(snapshot.availability, SourceAvailability::Available);
    assert_eq!(snapshot.revision, SourceRevision::Absent {});
    assert_eq!(snapshot.agents[0].id, "live");
}

#[test]
fn save_rejects_stale_revision() {
    let (_temp, path, settings) = setup();
    let file = named_path(&path, "team");
    write(&file, FIXTURE.as_bytes());
    let error = save_config_file(
        &settings,
        &path,
        &ConfigContext::Instance,
        &named("team"),
        &SourceRevision::Absent {},
        vec![agent("x")],
        Default::default(),
    )
    .unwrap_err();
    assert_eq!(error.code, "staleConfigFile");
    assert_eq!(std::fs::read(&file).unwrap(), FIXTURE.as_bytes());
}

fn save_code(settings: &AppSettings, path: &Path, loc: ConfigFileLocator) -> &'static str {
    save_config_file(
        settings,
        path,
        &ConfigContext::Instance,
        &loc,
        &SourceRevision::Absent {},
        Vec::new(),
        Default::default(),
    )
    .unwrap_err()
    .code
}

#[test]
fn save_rejects_layer_and_other() {
    let (_temp, path, settings) = setup();
    let layer = ConfigFileLocator::Layer {
        scope: ConfigScope::Instance,
        layer: CatalogLayer::CatalogPersonal,
    };
    let other = ConfigFileLocator::Other {
        scope: ConfigScope::Instance,
        file_name: "agents.old.instance.json".into(),
    };
    assert_eq!(save_code(&settings, &path, layer), "configFileReadOnly");
    assert_eq!(save_code(&settings, &path, other), "configFileReadOnly");
}

#[test]
fn save_rejects_registered() {
    let (_temp, path, settings) = setup();
    let loc = ConfigFileLocator::RegisteredInstance {};
    assert_eq!(save_code(&settings, &path, loc), "configFileUseSettings");
    assert!(!path
        .with_file_name("agents.30.instance.no-git.json")
        .exists());
}

fn create(
    settings: &AppSettings,
    path: &Path,
    ctx: &ConfigContext,
    scope: ConfigScope,
    name: &str,
) -> Result<crate::config::settings::agent::file::ConfigFileSnapshot, &'static str> {
    create_config_file(
        settings,
        path,
        ctx,
        scope,
        name,
        vec![agent("a")],
        Default::default(),
    )
    .map_err(|error| error.code)
}

#[test]
fn create_rejects_duplicate_case_insensitive() {
    let (_temp, path, settings) = setup();
    let upper = path
        .with_file_name("coding-agents")
        .join("AC.Coding-Agents-Config.Team.json");
    write(&upper, b"{}");
    let ctx = ConfigContext::Instance;
    let error = create(&settings, &path, &ctx, ConfigScope::Instance, "Team").unwrap_err();
    assert_eq!(error, "configNameExists");
    assert_eq!(std::fs::read(&upper).unwrap(), b"{}");
    assert!(!named_path(&path, "team").exists());
}

#[test]
fn create_project_scope_needs_project_context() {
    let (_temp, path, settings) = setup();
    let ctx = ConfigContext::Instance;
    let error = create(&settings, &path, &ctx, ConfigScope::Project, "team").unwrap_err();
    assert_eq!(error, "configFileWrongProject");
}

#[test]
fn create_makes_missing_dir() {
    let (temp, path, settings) = setup();
    let project = temp.path().join("project");
    let ctx = ConfigContext::Project(project.to_string_lossy().into_owned());
    let dir = project.join(".ac").join("coding-agents");
    assert!(!dir.exists());
    let snapshot = create(&settings, &path, &ctx, ConfigScope::Project, "team").unwrap();
    assert_eq!(snapshot.availability, SourceAvailability::Available);
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(snapshot.path, dir.join("ac.coding-agents-config.team.json"));
    let raw = disk(&snapshot.path);
    assert!(raw["codingAgentProfiles"].get("profilesByAgent").is_none());
    let again = create(&settings, &path, &ctx, ConfigScope::Project, "team").unwrap_err();
    assert_eq!(again, "configNameExists");
}

#[test]
fn register_rejects_non_registered_shape_and_changes_nothing() {
    let (temp, _path, settings) = setup();
    let catalog = temp.path().join("catalog.json");
    write(&catalog, br#"{"schemaVersion":1,"rows":[]}"#);
    for bad in [
        catalog.clone(),
        temp.path().to_path_buf(),
        temp.path().join("missing.json"),
    ] {
        let error = register_loaded(&settings, &bad.to_string_lossy()).unwrap_err();
        assert_eq!(error.code, "configFileInvalid");
    }
    assert_eq!(settings.coding_agent_config_files, None);
    assert_eq!(
        std::fs::read(&catalog).unwrap(),
        br#"{"schemaVersion":1,"rows":[]}"#
    );
    let good = temp.path().join("good.json");
    write(&good, FIXTURE.as_bytes());
    let sel = register_loaded(&settings, &good.to_string_lossy()).unwrap();
    let canonical = std::fs::canonicalize(&good).unwrap();
    assert_eq!(sel.loaded, vec![canonical.to_string_lossy().into_owned()]);
    let settings = AppSettings {
        coding_agent_config_files: Some(sel.clone()),
        ..settings
    };
    assert_eq!(
        register_loaded(&settings, &good.to_string_lossy()).unwrap(),
        sel
    );
}

#[test]
fn set_active_accepts_layer_and_rejects_absent_and_wrong_project() {
    let (_temp, path, settings) = setup();
    let ctx = ConfigContext::Instance;
    let layer = ConfigFileLocator::Layer {
        scope: ConfigScope::Instance,
        layer: CatalogLayer::CatalogPersonal,
    };
    let dirs = config_dirs(&settings, &path, &ctx);
    let (_, _, layer_path) = dirs
        .layers
        .iter()
        .find(|(_, l, _)| *l == CatalogLayer::CatalogPersonal)
        .unwrap();
    write(layer_path, br#"{"schemaVersion":1,"agents":[]}"#);
    let sel = set_active_config_file(&settings, &path, &ctx, &layer).unwrap();
    assert_eq!(sel.active_by_context.get("instance"), Some(&layer));
    let absent = set_active_config_file(&settings, &path, &ctx, &named("gone")).unwrap_err();
    assert_eq!(absent.code, "configFileUnavailable");
    let project = ConfigFileLocator::Named {
        scope: ConfigScope::Project,
        name: "team".into(),
    };
    let wrong = set_active_config_file(&settings, &path, &ctx, &project).unwrap_err();
    assert_eq!(wrong.code, "configFileWrongProject");
}

#[test]
fn catalog_layer_projects_rows_to_launchable_agents() {
    let (_temp, path, settings) = setup();
    let ctx = ConfigContext::Instance;
    let dirs = config_dirs(&settings, &path, &ctx);
    let (_, _, layer_path) = dirs
        .layers
        .iter()
        .find(|(_, l, _)| *l == CatalogLayer::CatalogPersonal)
        .unwrap();
    let rows = json!({"schemaVersion":1,"agents":[
        {"key":"k1","baseCommand":"tool --x","label":"K","color":"blue"}]});
    write(layer_path, &serde_json::to_vec(&rows).unwrap());
    let loc = ConfigFileLocator::Layer {
        scope: ConfigScope::Instance,
        layer: CatalogLayer::CatalogPersonal,
    };
    let snapshot = read_config_file(&settings, &path, &ctx, &loc).unwrap();
    assert_eq!(snapshot.availability, SourceAvailability::Available);
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(snapshot.agents[0].id, "k1");
    assert_eq!(snapshot.agents[0].command, "tool --x");
    assert_eq!(snapshot.agents[0].context_regex, None);
}

#[test]
fn unregistered_loaded_locator_is_never_read() {
    let (temp, path, settings) = setup();
    let outside = temp.path().join("outside.json");
    write(&outside, FIXTURE.as_bytes());
    let loc = ConfigFileLocator::Loaded {
        path: outside.to_string_lossy().into_owned(),
    };
    let error = read_config_file(&settings, &path, &ConfigContext::Instance, &loc).unwrap_err();
    assert_eq!(error.code, "configFileUnknown");
}

#[test]
fn listing_marks_missing_active_with_notice() {
    let (_temp, path, mut settings) = setup();
    select(&mut settings, named("gone"));
    let listing = list_config_files(&settings, &path, &ConfigContext::Instance);
    assert_eq!(listing.active, named("gone"));
    assert_eq!(
        listing.active_notice.as_deref(),
        Some("activeConfigFileUnavailable")
    );
    let registered = &listing.entries[0];
    assert_eq!(registered.1, SourceAvailability::Available);
    let default = AppSettings::default();
    let listing = list_config_files(&default, &path, &ConfigContext::Instance);
    assert_eq!(listing.active_notice, None);
}

#[test]
fn save_rejects_missing_file_and_creates_nothing() {
    let (_temp, path, mut settings) = setup();
    let file = named_path(&path, "team");
    write(&file, FIXTURE.as_bytes());
    select(&mut settings, named("team"));
    let ctx = ConfigContext::Instance;
    let snapshot = read_config_file(&settings, &path, &ctx, &named("team")).unwrap();
    std::fs::remove_file(&file).unwrap();
    let error = save_config_file(
        &settings,
        &path,
        &ctx,
        &named("team"),
        &snapshot.revision,
        snapshot.agents,
        snapshot.coding_agent_profiles,
    )
    .unwrap_err();
    assert_eq!(error.code, "configFileUnavailable");
    assert!(!file.exists());
    let _: Option<CodingAgentConfigFiles> = settings.coding_agent_config_files;
}
