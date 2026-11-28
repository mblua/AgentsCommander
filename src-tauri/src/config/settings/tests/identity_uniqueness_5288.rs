//! #5288: a save never adds a row to a shared configuration identity, while
//! pre-existing duplicates keep loading and saving unrelated changes.

use super::*;
use serde_json::json;

use super::super::{
    load_settings_for_cli_from_path, load_settings_for_cli_strict_from_path,
    save_settings_to_path_preserving_project_paths as save, AgentConfig, AppSettings,
    CodingAgentEnv,
};

fn d4(name: &str) -> String {
    format!(
        "DuplicateConfigurationIdentity: Coding Agent '{name}' has the same command and envs \
         (ignoring order, case and spaces), so both would share the same profiles. Change the \
         command or the envs."
    )
}

fn agent(id: &str, label: &str, command: &str) -> AgentConfig {
    let mut agent: AgentConfig = serde_json::from_value(b3_agent(id, label)).unwrap();
    agent.command = command.into();
    agent
}

fn envs(rows: &[(&str, &str)]) -> Vec<CodingAgentEnv> {
    let rows: Vec<_> = rows
        .iter()
        .map(|(k, v)| json!({"key":k,"value":v}))
        .collect();
    serde_json::from_value(json!(rows)).unwrap()
}

fn load(path: &std::path::Path) -> AppSettings {
    load_settings_for_cli_strict_from_path(Some(path)).unwrap()
}

fn refused(settings: &AppSettings, path: &std::path::Path, dir: &std::path::Path) -> String {
    let files = hash_activation_files(dir);
    let error = save(settings, path).unwrap_err().to_string();
    assert_eq!(hash_activation_files(dir), files);
    error
}

/// The file holding the rows of this layout: sidecar, or legacy main.
fn rows_path(dir: &std::path::Path, sidecar: bool) -> std::path::PathBuf {
    if sidecar {
        b3_sidecar_path(dir)
    } else {
        b3_settings_path(dir)
    }
}

/// Append rows cloned from row 0 straight to disk, bypassing save.
fn write_rows(dir: &std::path::Path, sidecar: bool, rows: &[(&str, &str, &str)]) {
    let file = rows_path(dir, sidecar);
    let mut raw = serde_json::Value::Object(b3_object(&file));
    for (id, label, command) in rows {
        let mut row = raw["agents"][0].clone();
        row["id"] = json!(id);
        row["label"] = json!(label);
        for key in ["command", "baseCommand"] {
            if row.get(key).is_some() {
                row[key] = json!(command);
            }
        }
        raw["agents"].as_array_mut().unwrap().push(row);
    }
    b3_write(&file, &raw);
}

fn ac1_add_refused(sidecar: bool) {
    let (temp, path) = hash_activation_seed(sidecar);
    let mut settings = load(&path);
    settings.agents.push(agent("beta", "Beta", "claude"));
    let error = refused(&settings, &path, temp.path());
    assert!(error.contains(&d4("Incumbent")), "{error}");
}

#[test]
fn ac1_add_refused_sidecar_layout() {
    ac1_add_refused(true);
}

#[test]
fn ac1_add_refused_legacy_main_layout() {
    ac1_add_refused(false);
}

#[test]
fn ac1_two_new_rows_same_identity_refused() {
    let (temp, path) = hash_activation_seed(true);
    let mut settings = load(&path);
    settings.agents.push(agent("beta", "Beta", "fresh"));
    settings.agents.push(agent("gamma", "Gamma", "fresh"));
    let error = refused(&settings, &path, temp.path());
    assert!(error.contains("DuplicateConfigurationIdentity"), "{error}");
}

fn distinct_pair() -> (tempfile::TempDir, std::path::PathBuf) {
    let (temp, path) = hash_activation_seed(true);
    let mut settings = load(&path);
    settings.agents.push(agent("beta", "Second", "codex"));
    save(&settings, &path).unwrap();
    (temp, path)
}

#[test]
fn ac2_edit_later_row_names_earlier() {
    let (temp, path) = distinct_pair();
    let mut settings = load(&path);
    settings.agents[1].command = "claude".into();
    let error = refused(&settings, &path, temp.path());
    assert!(error.contains(&d4("Incumbent")), "{error}");
}

#[test]
fn ac2_edit_earlier_row_names_later() {
    let (temp, path) = distinct_pair();
    let mut settings = load(&path);
    settings.agents[0].command = "codex".into();
    let error = refused(&settings, &path, temp.path());
    assert!(error.contains(&d4("Second")), "{error}");
    assert!(!error.contains("'Incumbent'"), "{error}");
}

#[test]
fn ac2_env_order_case_space_do_not_escape() {
    let (temp, path) = hash_activation_seed(true);
    let mut settings = load(&path);
    let mut incumbent = agent("beta", "WithEnvs", "claude");
    incumbent.envs = envs(&[("MODE", "fast"), ("HOME_DIR", "x")]);
    settings.agents.push(incumbent);
    settings.agents.push(agent("gamma", "Gamma", "codex"));
    save(&settings, &path).unwrap();
    let mut settings = load(&path);
    settings.agents[2].command = " CLAUDE ".into();
    settings.agents[2].envs = envs(&[("home_dir", " X"), ("Mo De", "FAST")]);
    let error = refused(&settings, &path, temp.path());
    assert!(error.contains(&d4("WithEnvs")), "{error}");
}

#[test]
fn ac2_swap_refused() {
    let (temp, path) = hash_activation_seed(true);
    write_rows(
        temp.path(),
        true,
        &[("beta", "Beta", "claude"), ("gamma", "Gamma", "codex")],
    );
    let mut settings = load(&path);
    settings.agents[1].command = "unique-beta".into();
    settings.agents[2].command = "claude".into();
    let error = refused(&settings, &path, temp.path());
    assert!(error.contains(&d4("Incumbent")), "{error}");
}

#[test]
fn ac3_same_command_different_envs_allowed() {
    let (_temp, path) = hash_activation_seed(true);
    let mut settings = load(&path);
    let mut other = agent("beta", "Beta", "claude");
    other.envs = envs(&[("RTK_DB_PATH", "x")]);
    settings.agents.push(other);
    save(&settings, &path).unwrap();
    let reloaded = load(&path);
    assert_eq!(reloaded.agents.len(), 2);
    assert_ne!(reloaded.agents[0].id_hash, reloaded.agents[1].id_hash);
}

fn existing_duplicates(sidecar: bool, remove_sidecar: bool) {
    let (temp, path) = hash_activation_seed(sidecar);
    write_rows(temp.path(), sidecar, &[("beta", "Beta", "claude")]);
    if remove_sidecar {
        std::fs::remove_file(b3_sidecar_path(temp.path())).unwrap();
    }
    let settings = load(&path);
    assert_eq!(settings.agents.len(), 2);
    assert_eq!(settings.agents[0].id_hash, settings.agents[1].id_hash);
    let mut settings = load_settings_for_cli_from_path(&path);
    assert_eq!(settings.agents.len(), 2);
    settings.log_level = Some("debug".into());
    save(&settings, &path).unwrap();
    let mut settings = load(&path);
    settings.gemini_api_key = "key-5288".into();
    save(&settings, &path).unwrap();
    let mut settings = load(&path);
    settings.agents[1].label = "Beta renamed".into();
    save(&settings, &path).unwrap();
    let reloaded = load(&path);
    let ids: Vec<_> = reloaded.agents.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["alpha", "beta"]);
    assert_eq!(reloaded.agents[1].label, "Beta renamed");
    assert_eq!(reloaded.log_level.as_deref(), Some("debug"));
}

#[test]
fn ac4_existing_duplicates_sidecar_layout() {
    existing_duplicates(true, false);
}

#[test]
fn ac4_existing_duplicates_legacy_main_layout() {
    existing_duplicates(false, false);
}

#[test]
fn ac4_existing_duplicates_legacy_main_no_sidecar() {
    existing_duplicates(false, true);
}

#[test]
fn ac4_existing_duplicate_fix_or_remove() {
    for sidecar in [true, false] {
        let (temp, path) = hash_activation_seed(sidecar);
        write_rows(temp.path(), sidecar, &[("beta", "Beta", "claude")]);
        let mut fixed = load(&path);
        fixed.agents[1].command = "unique-beta".into();
        save(&fixed, &path).unwrap();
        assert_eq!(load(&path).agents[1].command, "unique-beta");

        let (temp, path) = hash_activation_seed(sidecar);
        write_rows(temp.path(), sidecar, &[("beta", "Beta", "claude")]);
        let mut removed = load(&path);
        removed.agents.retain(|a| a.id != "beta");
        save(&removed, &path).unwrap();
        assert_eq!(load(&path).agents.len(), 1);
    }
}

#[test]
fn ac1_overlay_profile_labels_only_still_refused() {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let mut alpha = b3_agent("alpha", "Incumbent");
    alpha["command"] = json!("codex");
    let mut main = serde_json::to_value(AppSettings::default()).unwrap();
    main["agents"] = json!([alpha]);
    main["codingAgentProfiles"] = json!({
        "profileBindingVersion": 1,
        "profilesByAgent": {},
        "profileLabelsByAgent": {}
    });
    b3_write(&path, &main);
    b3_write(
        &b3_overlay_path(temp.path()),
        &json!({"codingAgentProfiles":{"profileLabelsByAgent":{}}}),
    );
    let mut settings = load_settings_for_cli_from_path(&path);
    settings.log_level = Some("debug".into());
    save(&settings, &path).unwrap();
    let mut settings = load_settings_for_cli_from_path(&path);
    settings.agents.push(agent("beta", "Beta", "codex"));
    let error = save(&settings, &path).unwrap_err().to_string();
    assert!(error.contains(&d4("Incumbent")), "{error}");
    assert_eq!(load_settings_for_cli_from_path(&path).agents.len(), 1);
}

#[test]
fn defaults_have_no_shared_identity() {
    let mut settings = AppSettings::default();
    super::super::validate_and_repair_settings(&mut settings).unwrap();
    let mut seen = std::collections::HashSet::new();
    for agent in &settings.agents {
        let hash = super::super::configuration_identity(&agent.command, &agent.envs);
        assert!(seen.insert(hash), "shared identity: {}", agent.id);
    }
}

#[test]
fn duplicate_configuration_identity_message_text() {
    let error = super::super::IdentityValidationError {
        code: super::super::IdentityValidationCode::DuplicateConfigurationIdentity,
        existing_name: "Incumbent".into(),
        entry_hint: "beta".into(),
        requested_profile: None,
    };
    assert_eq!(error.to_string(), d4("Incumbent"));
}
