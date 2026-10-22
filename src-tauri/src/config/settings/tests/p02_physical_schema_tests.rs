use super::super::*;
use serde_json::json;

#[test]
fn p02_three_loaders_do_not_rewrite_and_locked_save_retains_legacy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let main = json!({"defaultShell":"test-shell","defaultShellArgs":[],"rootToken":"base-token"});
    let groups = json!({"agents":[{"id":"own","label":"Own","color":"#123456","baseCommand":"tool --current","command":"tool --legacy","order":0}],"codingAgentProfiles":{"schemaVersion":2,"profileSlots":{"A":{"label":""}},"defaultProfileByAgent":{},"profilesByAgent":{"own":{"A":{"enabled":true,"additionalArguments":"--current","command":"--legacy","env":{},"notes":"keep"}}},"profileLabelsByAgent":{}}});
    std::fs::write(&path, serde_json::to_vec_pretty(&main).unwrap()).unwrap();
    let sidecar = agents_instance_path(&path);
    std::fs::write(&sidecar, serde_json::to_vec_pretty(&groups).unwrap()).unwrap();
    // BASE already materializes project-paths.json and a persistent lock on
    // first use. Seed those controls before measuring schema read effects.
    let project_paths = project_paths_path(&path);
    std::fs::write(&project_paths, b"{}").unwrap();
    drop(
        SettingsFileLock::acquire_contextual(
            &path,
            std::time::Duration::from_secs(2),
            "p02-read-baseline",
        )
        .unwrap(),
    );
    let main_before = std::fs::read(&path).unwrap();
    let agents_before = std::fs::read(&sidecar).unwrap();
    let names = || {
        let mut names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let names_before = names();
    let strict = load_settings_for_cli_strict_from_path(Some(&path)).unwrap();
    let cli = load_settings_for_cli_from_path(&path);
    assert_eq!(std::fs::read(&path).unwrap(), main_before);
    assert_eq!(std::fs::read(&sidecar).unwrap(), agents_before);
    assert_eq!(names(), names_before, "CLI reads create no files");
    let gui = load_settings_from_path(&path);
    let first_gui_main = std::fs::read(&path).unwrap();
    let first_gui_agents = std::fs::read(&sidecar).unwrap();
    let first_gui_names = names();
    for settings in [&strict, &cli, &gui] {
        assert_eq!(settings.agents[0].command, "tool --current");
        assert_eq!(
            settings.coding_agent_profiles.profiles_by_agent["own"]["A"].command,
            "--current"
        );
    }
    let canonical: Value = serde_json::from_slice(&first_gui_agents).unwrap();
    assert_eq!(canonical["codingAgentProfiles"]["profileBindingVersion"], 1);
    assert!(canonical["codingAgentProfiles"]
        .get("profilesByAgent")
        .is_none());
    load_settings_from_path(&path);
    assert_eq!(std::fs::read(&path).unwrap(), first_gui_main);
    assert_eq!(std::fs::read(&sidecar).unwrap(), first_gui_agents);
    assert_eq!(std::fs::read(&project_paths).unwrap(), b"{}");
    assert_eq!(names(), first_gui_names, "second GUI load is a fixed point");
    let mut edited = strict;
    edited.agents[0].command = "tool --edited".into();
    edited
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("own")
        .unwrap()
        .get_mut("A")
        .unwrap()
        .command = "--edited".into();
    let written = save_settings_to_path_preserving_project_paths(&edited, &path).unwrap();
    let raw: Value = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(raw["agents"][0]["baseCommand"], "tool --edited");
    assert_eq!(raw["agents"][0]["command"], "tool --legacy");
    assert_eq!(
        raw["codingAgentProfiles"]["profilesByBaseHash"]
            [&configuration_identity("tool --edited", &[])]["A"]["additionalArguments"],
        "--edited"
    );
    assert_eq!(
        raw["codingAgentProfiles"]["profilesByBaseHash"]
            [&configuration_identity("tool --edited", &[])]["A"]["command"],
        "--legacy"
    );
    let bytes = std::fs::read(&sidecar).unwrap();
    save_settings_to_path_preserving_project_paths(&written, &path).unwrap();
    assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
}
