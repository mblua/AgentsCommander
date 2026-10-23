use super::super::*;
use super::local_overlay_1737::{base_fixture, disk_object, seed};
use crate::config::coding_agent_mutations::{apply_coding_agent_op, CodingAgentOp};
use serde_json::json;
use std::path::Path;

fn agent_value(id: &str, label: &str, command: &str) -> Value {
    json!({
        "id": id,
        "label": label,
        "command": command,
        "color": "#000000",
        "blockingMenus": [],
    })
}

fn agent(id: &str, label: &str, command: &str) -> AgentConfig {
    serde_json::from_value(agent_value(id, label, command)).unwrap()
}

fn identity_of(id: &str, label: &str, command: &str) -> String {
    agent_profile_identity(&agent(id, label, command))
}

fn cell(command: &str) -> ProfileCellConfig {
    ProfileCellConfig {
        id_hash: None,
        id_hash_debug: None,
        preflight: None,
        preflight_timeout_seconds: None,
        command: command.to_string(),
        ..empty_profile_cell()
    }
}

fn cell_value(command: &str) -> Value {
    serde_json::to_value(cell(command)).unwrap()
}

/// Settings with agent `n` (a gap) and orphan `o` whose recorded identity is
/// `n`'s and whose `A` holds `--special`.
fn gap_and_matching_orphan() -> AppSettings {
    let mut settings = AppSettings {
        agents: vec![agent("n", "Claude", "claude")],
        ..AppSettings::default()
    };
    let profiles = &mut settings.coding_agent_profiles;
    profiles.profiles_by_agent.insert(
        "o".to_string(),
        BTreeMap::from([("A".to_string(), cell("--special"))]),
    );
    profiles
        .identity_by_agent
        .insert("o".to_string(), identity_of("n", "Claude", "claude"));
    settings
}

fn a_command(settings: &AppSettings, id: &str) -> Option<String> {
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .get(id)
        .and_then(|cells| cells.get("A"))
        .map(|cell| cell.command.clone())
}

fn load_save(path: &Path) -> AppSettings {
    let settings = load_settings_from_path(path);
    save_settings_to_path_preserving_project_paths(&settings, path).unwrap();
    load_settings_from_path(path)
}

// 1
#[test]
fn identity_is_stable_across_a_reminted_id() {
    assert_eq!(
        identity_of("agent_1_aaaaaa", "Claude", "claude --x"),
        identity_of("agent_2_bbbbbb", "Claude", "claude --x")
    );
    let identity = identity_of("agent_1_aaaaaa", "Claude", "claude --x");
    assert_eq!(identity.len(), 16);
    assert!(identity.bytes().all(|b| b.is_ascii_hexdigit()));
}

// 2
#[test]
fn identity_changes_with_command_env_or_label() {
    let base = agent("a", "Claude", "claude");
    let identity = agent_profile_identity(&base);

    let command = agent("a", "Claude", "claude --other");
    assert_ne!(agent_profile_identity(&command), identity);

    let mut env = base.clone();
    env.envs.push(CodingAgentEnv {
        key: "K".to_string(),
        value: "V".to_string(),
        source: CodingAgentEnvSource::User,
        enabled: true,
    });
    assert_ne!(agent_profile_identity(&env), identity);
    // A disabled env row is not an input.
    env.envs[0].enabled = false;
    assert_eq!(agent_profile_identity(&env), identity);

    let label = agent("a", "Other", "claude");
    assert_ne!(agent_profile_identity(&label), identity);
    // The label is case-folded.
    let folded = agent("a", "CLAUDE", "claude");
    assert_eq!(agent_profile_identity(&folded), identity);
}

// 3
#[test]
fn identity_is_not_the_cell_digest() {
    let base = agent("a", "Claude", "claude");
    let before = agent_profile_identity(&base);
    let digest_before = crate::config::agent_command::cell_identity(&base, &cell("--one"));
    let digest_after = crate::config::agent_command::cell_identity(&base, &cell("--two"));
    assert_ne!(
        digest_before, digest_after,
        "the cell digest follows the cell"
    );
    assert_eq!(
        agent_profile_identity(&base),
        before,
        "the agent identity does not"
    );
    assert_ne!(before, digest_before);
}

// 4
#[test]
fn absent_identity_by_agent_changes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["agents"] = json!([agent_value("n", "Claude", "claude")]);
    let path = seed(temp.path(), Some(&base), None);
    load_save(&path);
    let old = disk_object(&crate::config::settings::agents_instance_path(&path));
    assert_eq!(old["codingAgentProfiles"]["profileBindingVersion"], 1);
    assert!(old["codingAgentProfiles"].get("identityByAgent").is_none());
    let settings = load_settings_from_path(&path);
    assert_eq!(a_command(&settings, "n").as_deref(), Some(""));
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    let written = disk_object(&crate::config::settings::agents_instance_path(&path));
    assert!(written["codingAgentProfiles"]
        .get("identityByAgent")
        .is_none());
    assert_eq!(written, old, "BASE storage never regenerates ID identities");
}

// 5
#[test]
fn remove_then_add_through_the_real_ops_keeps_the_cells() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(temp.path(), Some(&base_fixture()), None);
    let mut settings = load_settings_from_path(&path);

    let add = |id: &str| CodingAgentOp::Add {
        agent: agent(id, "Claude", "claude"),
    };
    apply_coding_agent_op(&mut settings, &add("agent_1_aaaaaa")).unwrap();
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("agent_1_aaaaaa")
        .unwrap()
        .insert("A".to_string(), cell("--special"));
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "agent_1_aaaaaa".to_string(),
            BTreeMap::from([("A".to_string(), "Mine".to_string())]),
        );
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let mut settings = load_settings_from_path(&path);
    apply_coding_agent_op(
        &mut settings,
        &CodingAgentOp::Remove {
            id: "agent_1_aaaaaa".to_string(),
        },
    )
    .unwrap();
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let object = disk_object(&crate::config::settings::agents_instance_path(&path));
    assert_eq!(object["agents"], json!([]));
    assert_eq!(
        object["codingAgentProfiles"]["profilesByBaseHash"][&configuration_identity("claude", &[])]
            ["A"],
        cell_value("--special")
    );
    assert!(object["codingAgentProfiles"]
        .get("identityByAgent")
        .is_none());

    let mut settings = load_settings_from_path(&path);
    apply_coding_agent_op(&mut settings, &add("agent_2_bbbbbb")).unwrap();
    let settings = save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    assert_eq!(
        a_command(&settings, "agent_2_bbbbbb").as_deref(),
        Some("--special"),
        "the recreated agent holds the original cells, not a repaired empty A"
    );
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();

    let object = disk_object(&crate::config::settings::agents_instance_path(&path));
    let profiles = &object["codingAgentProfiles"];
    assert_eq!(
        profiles["profilesByBaseHash"][&configuration_identity("claude", &[])]["A"],
        cell_value("--special")
    );
    assert_eq!(
        profiles["profileLabelsByBaseHash"][&configuration_identity("claude", &[])],
        json!({"A": "Mine"})
    );
    assert!(profiles["profilesByAgent"].get("agent_1_aaaaaa").is_none());
    assert!(profiles["identityByAgent"].get("agent_1_aaaaaa").is_none());
    assert!(profiles.get("identityByAgent").is_none());
}

// 6
#[test]
fn adoption_runs_before_repair() {
    let mut settings = gap_and_matching_orphan();
    validate_and_repair_settings(&mut settings).unwrap();
    assert_eq!(a_command(&settings, "n").as_deref(), Some("--special"));
    assert!(!settings
        .coding_agent_profiles
        .profiles_by_agent
        .contains_key("o"));
    assert!(!settings
        .coding_agent_profiles
        .identity_by_agent
        .contains_key("o"));

    // An orphan without an `A` cell is what exposes the order: repair's
    // empty `A` also counts as a gap, so adoption after repair would still
    // fire, but it would replace that `A` and leave the agent without one.
    let mut settings = gap_and_matching_orphan();
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "o".to_string(),
        BTreeMap::from([("B".to_string(), cell("--b"))]),
    );
    validate_and_repair_settings(&mut settings).unwrap();
    let cells = &settings.coding_agent_profiles.profiles_by_agent["n"];
    assert_eq!(cells["B"].command, "--b");
    let mut expected_a = empty_profile_cell();
    expected_a.id_hash = Some(configuration_identity("claude", &[]));
    expected_a.id_hash_debug = Some(vec!["claude".into(), "".into()]);
    assert_eq!(cells.get("A"), Some(&expected_a));
    assert_eq!(
        cells["B"].id_hash,
        Some(configuration_identity("claude--b", &[]))
    );
}

// 7
#[test]
fn a_repair_default_entry_counts_as_a_gap() {
    let mut settings = gap_and_matching_orphan();
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "n".to_string(),
        BTreeMap::from([("A".to_string(), empty_profile_cell())]),
    );
    validate_and_repair_settings(&mut settings).unwrap();
    assert_eq!(a_command(&settings, "n").as_deref(), Some("--special"));
}

#[test]
fn issue_3047_schema_orphan_adopts_gap_with_metadata() {
    for hash in [None, Some("forged".to_string())] {
        let mut settings = gap_and_matching_orphan();
        let mut gap = empty_profile_cell();
        gap.id_hash = hash;
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .insert("n".into(), BTreeMap::from([("A".into(), gap)]));
        assert!(adopt_orphaned_profiles(&mut settings));
        assert_eq!(a_command(&settings, "n").as_deref(), Some("--special"));
        assert!(!settings
            .coding_agent_profiles
            .profiles_by_agent
            .contains_key("o"));
    }
}

// 8
#[test]
fn an_agent_with_a_real_cell_is_never_adopted_into() {
    let mut settings = gap_and_matching_orphan();
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "n".to_string(),
        BTreeMap::from([("A".to_string(), cell("--mine"))]),
    );
    let mut labelled = gap_and_matching_orphan();
    labelled
        .coding_agent_profiles
        .profile_labels_by_agent
        .insert(
            "n".to_string(),
            BTreeMap::from([("A".to_string(), "Mine".to_string())]),
        );
    for mut s in [settings, labelled] {
        let before = s.coding_agent_profiles.clone();
        assert!(!adopt_orphaned_profiles(&mut s));
        assert_eq!(s.coding_agent_profiles, before);
    }
}

// 9
#[test]
fn orphans_survive_a_save_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["agents"] = json!([agent_value("n", "Claude", "claude")]);
    base["codingAgentProfiles"] = json!({
        "profilesByAgent": {"o": {"B": cell_value("--orphan")}},
        "profileLabelsByAgent": {"o": {"B": "Old"}},
        "identityByAgent": {"o": identity_of("x", "Gone", "gone")},
    });
    let path = seed(temp.path(), Some(&base), None);
    load_save(&path);
    let profiles =
        &disk_object(&crate::config::settings::agents_instance_path(&path))["codingAgentProfiles"];
    assert_eq!(profiles["profileBindingVersion"], 1);
    assert_eq!(
        profiles["legacyProfileOrphans"]["o"],
        json!([{
            "cells": base["codingAgentProfiles"]["profilesByAgent"]["o"],
            "labels": base["codingAgentProfiles"]["profileLabelsByAgent"]["o"],
            "identity": base["codingAgentProfiles"]["identityByAgent"]["o"]
        }])
    );
    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
        assert!(profiles.get(key).is_none());
    }
}

// 10
#[test]
fn two_agents_with_one_identity_adopt_once_and_warn() {
    let mut settings = gap_and_matching_orphan();
    settings.agents.push(agent("m", "claude", "claude"));
    // A second orphan with the same identity must not reach the loser.
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "p".to_string(),
        BTreeMap::from([("A".to_string(), cell("--second"))]),
    );
    settings
        .coding_agent_profiles
        .identity_by_agent
        .insert("p".to_string(), identity_of("n", "Claude", "claude"));
    validate_and_repair_settings(&mut settings).unwrap();
    assert_eq!(a_command(&settings, "n").as_deref(), Some("--special"));
    assert_eq!(
        a_command(&settings, "m").as_deref(),
        Some(""),
        "the loser keeps its own empty state"
    );
    assert!(settings
        .coding_agent_profiles
        .profiles_by_agent
        .contains_key("p"));
}

// 11
#[test]
fn identity_rows_record_the_base_agent_under_an_overlay() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["agents"] = json!([agent_value("a", "Agent", "claude")]);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"agents": [agent_value("a", "Agent", "codex")]})),
    );
    let settings = load_settings_from_path(&path);
    assert_eq!(settings.agents[0].command, "codex");
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    let sidecar = disk_object(&crate::config::settings::agents_instance_path(&path));
    let profiles = &sidecar["codingAgentProfiles"];
    let physical = super::super::configuration_identity("claude", &[]);
    let effective = super::super::configuration_identity("codex", &[]);
    assert_eq!(profiles["profileBindingVersion"], 1);
    assert_eq!(
        profiles["profilesByBaseHash"][&physical],
        json!({
            "A":{"enabled":true,"additionalArguments":"","env":{},"notes":""}
        })
    );
    assert_eq!(profiles["profileLabelsByBaseHash"][&physical], json!({}));
    assert!(profiles["profilesByBaseHash"].get(&effective).is_none());
    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
        assert!(profiles.get(key).is_none());
    }
    assert!(sidecar.get("agents").is_none());
    assert_eq!(disk_object(&path)["agents"], base["agents"]);
    assert_eq!(load_settings_from_path(&path).agents[0].command, "codex");
}

// 12
#[test]
fn an_overlay_introduced_agent_id_does_not_persist_its_identity_row() {
    assert!(OVERLAY_DERIVED_ID_CLOSURES.iter().any(|closure| {
        closure.source_key == "agents"
            && closure.id_field == "id"
            && closure.derived_prefix == ["codingAgentProfiles", "identityByAgent"]
    }));
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["agents"] = json!([agent_value("codex", "codex", "codex")]);
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"agents": [
            agent_value("codex", "codex", "codex"),
            agent_value("scratch", "scratch", "scratch"),
        ]})),
    );
    let mut settings = load_settings_from_path(&path);
    // Even a row the caller put in memory stays out of the base file.
    settings
        .coding_agent_profiles
        .identity_by_agent
        .insert("scratch".to_string(), identity_of("x", "s", "s"));
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    let rows = &disk_object(&path)["codingAgentProfiles"]["identityByAgent"];
    assert!(rows.get("scratch").is_none(), "{rows}");
    assert!(rows.get("codex").is_some());
}

/// Base orphan `o` with cells, labels and an identity row equal to `n`'s.
fn orphan_profiles_value(identity: &str) -> Value {
    json!({
        "profilesByAgent": {"o": {"A": cell_value("--special")}},
        "profileLabelsByAgent": {"o": {"A": "Special"}},
        "identityByAgent": {"o": identity},
    })
}

// 13
#[test]
fn an_overlay_only_agent_never_consumes_a_base_orphan() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    let orphan = orphan_profiles_value(&identity_of("n", "Claude", "claude"));
    base["codingAgentProfiles"] = orphan.clone();
    let path = seed(
        temp.path(),
        Some(&base),
        Some(&json!({"agents": [agent_value("n", "Claude", "claude")]})),
    );
    let mut settings = load_settings_from_path(&path);
    assert!(!settings.local_overlay_state.is_empty());
    validate_and_repair_settings(&mut settings).unwrap();
    assert_eq!(
        a_command(&settings, "n").as_deref(),
        Some(""),
        "n never held them"
    );
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    std::fs::remove_file(temp.path().join("settings.50.personal.no-git.json")).unwrap();

    let profiles = &disk_object(&path)["codingAgentProfiles"];
    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
        assert_eq!(profiles[key]["o"], orphan[key]["o"], "{key}");
        assert!(profiles[key].get("n").is_none(), "{key}");
    }
}

/// Base `n` with repair's empty `A`, orphan `o` matching it.
fn profile_only_overlay_base() -> Value {
    let mut base = base_fixture();
    base["agents"] = json!([agent_value("n", "Claude", "claude")]);
    let mut profiles = orphan_profiles_value(&identity_of("n", "Claude", "claude"));
    profiles["profilesByAgent"]["n"] = json!({"A": cell_value("")});
    base["codingAgentProfiles"] = profiles;
    base
}

// 14
#[test]
fn a_profile_only_overlay_never_loses_a_cell() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(
        temp.path(),
        Some(&profile_only_overlay_base()),
        Some(&json!({"codingAgentProfiles": {"profilesByAgent": {"n": {"A": {"command": ""}}}}})),
    );
    let mut settings = load_settings_from_path(&path);
    assert!(!settings.local_overlay_state.is_empty());
    validate_and_repair_settings(&mut settings).unwrap();
    assert!(a_command(&settings, "o").is_none());
    assert_eq!(a_command(&settings, "n").as_deref(), Some(""));
    save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
    std::fs::remove_file(temp.path().join("settings.50.personal.no-git.json")).unwrap();

    let profiles = &disk_object(&path)["codingAgentProfiles"];
    assert_eq!(
        profiles["profilesByAgent"]["o"]["A"],
        cell_value("--special")
    );
    assert_eq!(
        profiles["profileLabelsByAgent"]["o"],
        json!({"A":"Special"})
    );
    assert_eq!(
        profiles["identityByAgent"]["o"],
        json!(identity_of("n", "Claude", "claude"))
    );
    let reloaded = load_settings_from_path(&path);
    assert_eq!(a_command(&reloaded, "n").as_deref(), Some(""));
    assert!(a_command(&reloaded, "o").is_none());
    let canonical = disk_object(&crate::config::settings::agents_instance_path(&path));
    assert_eq!(
        canonical["codingAgentProfiles"]["legacyProfileOrphans"]["o"],
        json!([{
            "cells":{"A":cell_value("--special")}, "labels":{"A":"Special"},
            "identity":identity_of("n", "Claude", "claude")
        }])
    );
}

// 15
#[test]
fn adoption_resumes_once_the_overlay_is_gone() {
    let temp = tempfile::tempdir().unwrap();
    let path = seed(temp.path(), Some(&profile_only_overlay_base()), None);
    let settings = load_settings_from_path(&path);
    assert_eq!(a_command(&settings, "n").as_deref(), Some(""));
    assert!(a_command(&settings, "o").is_none());
    let object = disk_object(&crate::config::settings::agents_instance_path(&path));
    let profiles = &object["codingAgentProfiles"];
    assert_eq!(profiles["profileBindingVersion"], 1);
    assert_eq!(
        profiles["profilesByBaseHash"][&configuration_identity("claude", &[])]["A"],
        cell_value("")
    );
    assert_eq!(
        profiles["legacyProfileOrphans"]["o"],
        json!([{
            "cells":{"A":cell_value("--special")},
            "labels":{"A":"Special"},
            "identity":identity_of("n", "Claude", "claude")
        }])
    );
    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
        assert!(profiles.get(key).is_none());
    }
    let before = std::fs::read(crate::config::settings::agents_instance_path(&path)).unwrap();
    let reloaded = load_settings_from_path(&path);
    assert_eq!(a_command(&reloaded, "n").as_deref(), Some(""));
    assert_eq!(
        std::fs::read(crate::config::settings::agents_instance_path(&path)).unwrap(),
        before
    );
}

// 16
#[test]
fn save_then_load_is_a_fixed_point() {
    let temp = tempfile::tempdir().unwrap();
    let mut base = base_fixture();
    base["agents"] = json!([agent_value("n", "Claude", "claude")]);
    base["codingAgentProfiles"] = orphan_profiles_value(&identity_of("x", "Gone", "gone"));
    let path = seed(temp.path(), Some(&base), None);
    load_save(&path);
    let first = std::fs::read(&path).unwrap();
    load_save(&path);
    assert_eq!(std::fs::read(&path).unwrap(), first);
}

// 17
#[test]
#[allow(non_snake_case)]
fn repair_still_gives_every_agent_an_A_cell() {
    let mut settings = gap_and_matching_orphan();
    settings.coding_agent_profiles.profiles_by_agent.insert(
        "o".to_string(),
        BTreeMap::from([("B".to_string(), cell("--b"))]),
    );
    settings.agents.push(agent("k", "Other", "codex"));
    validate_and_repair_settings(&mut settings).unwrap();
    let cells = &settings.coding_agent_profiles.profiles_by_agent;
    assert_eq!(cells["n"]["B"].command, "--b");
    let mut expected_a = empty_profile_cell();
    expected_a.id_hash = Some(configuration_identity("claude", &[]));
    expected_a.id_hash_debug = Some(vec!["claude".into(), "".into()]);
    assert_eq!(cells["n"]["A"], expected_a);
    expected_a.id_hash = Some(configuration_identity("codex", &[]));
    expected_a.id_hash_debug = Some(vec!["codex".into(), "".into()]);
    assert_eq!(cells["k"]["A"], expected_a);
    assert_eq!(
        cells["n"]["B"].id_hash,
        Some(configuration_identity("claude--b", &[]))
    );
}
