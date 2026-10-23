mod issue_3047_schema;

use super::{
    config_seed_dest_os_token_stem, merge_protected_coding_agent_settings,
    repair_coding_agent_profiles_config, validate_agent_commands, validate_api_server_settings,
    validate_resource_settings, AgentConfig, AppSettings, CodingAgentEnv, CodingAgentEnvSource,
    MainSidebarSide, ProfileCellConfig, ProfileSlotConfig, ResourceWatchdogAction,
    TelegramNetworkPollErrorLogging, TelegramPollFailureLogLevel, TelegramPollRecoveryLogLevel,
};
use std::collections::BTreeMap;

mod agent_order_and_derived_home;
mod e2432_n1_and_round7;
mod e2432_round7_o2_fallback;
mod id_hash_activation;
mod issue_3047_pair;
mod legacy_profiles_and_agent_commands;
mod lock_debug_and_id_hash_schema;
mod migrations_defaults_and_menus;
mod p03_project_capture;
mod preflight_seed_and_hotkeys;
mod project_migration_process;
mod project_paths_and_watchers;
mod response_close_2850;
mod save_backups_and_diagnostics;
mod save_failures_and_terminal_gate;
mod serde_field_defaults;
mod sidecar_export_and_mark;

use e2432_n1_and_round7::assert_o2_stable_full_load;
use e2432_round7_o2_fallback::assert_selected_project_decode_fallback_case;
use id_hash_activation::{b3_all_loaders, hash_activation_assert};
use legacy_profiles_and_agent_commands::LEGACY_PROFILES_SETTINGS_FIXTURE;
use migrations_defaults_and_menus::agent_1757;
use p03_project_capture::n1_test_overlay;
use preflight_seed_and_hotkeys::settings_with_agents;
use project_migration_process::{migration_fixture, migration_transaction};
use project_paths_and_watchers::real_ac_project_path;
use save_backups_and_diagnostics::{
    assert_no_issue_1330_temp_files, capture_settings_save_diagnostics,
    parse_settings_save_diagnostic, CapturedRecordsSnapshot, SettingsSaveDiagnosticCaptureGuard,
};
#[cfg(windows)]
use save_backups_and_diagnostics::{
    expected_io_kind_from_literal_table, expected_rendered_diagnostic_path,
};

// ─────────────────────────────────────────────────────────────────────────
// #2450 - orphaned profiles are re-bound by a recorded agent identity.
// ─────────────────────────────────────────────────────────────────────────
mod profile_identity_2450;

// ─────────────────────────────────────────────────────────────────────────
// #1737 - the `.local` alter-ego override layer. Plan section 11, S1 to S34.
// ─────────────────────────────────────────────────────────────────────────
mod local_overlay_1737;

// ─────────────────────────────────────────────────────────────────────────
// #1905 - the blocking-menus store: shipped file, `.local` overlay, precedence.
// ─────────────────────────────────────────────────────────────────────────
mod blocking_menus_1905;

// ─────────────────────────────────────────────────────────────────────────
// #1925 - the downloaded layer: the published bytes, the whole-file validator,
// per-read cache validation, and precedence between the layers.
// ─────────────────────────────────────────────────────────────────────────
mod remote_blocking_menus_1925;

/// #2348 - main-window placement persistence: serde, the central present-only
/// `Preserve` table, the legacy-migration one-save regression, and overlay
/// base-preservation for the new key.
mod issue_2348_main_window_placement;

/// #2133 (P2) - the agent-help data model, parser and local overlay. Every test drives a
/// `tempfile::tempdir()`; none reads the test config dir.
mod agent_help_2133;

// #2716 (B3) - the agents file. Rows E4 to E20b of the phase plan.

fn b3_agent(id: &str, label: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "label": label,
        "command": "claude",
        "color": "#112233",
    })
}

fn b3_profiles(marker: &str) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": 2,
        "profileLabelsByAgent": { "alpha": { "A": marker } },
    })
}

/// The E4 fixture: a settings file still carrying both legacy groups.
fn b3_legacy_fixture(label: &str, marker: &str) -> serde_json::Value {
    serde_json::json!({
        "defaultShell": "test-shell",
        "defaultShellArgs": [],
        "rootToken": "base-token",
        "agents": [b3_agent("alpha", label)],
        "codingAgentProfiles": b3_profiles(marker),
    })
}

/// A settings file that no longer carries either group.
fn b3_migrated_fixture() -> serde_json::Value {
    serde_json::json!({
        "defaultShell": "test-shell",
        "defaultShellArgs": [],
        "rootToken": "base-token",
    })
}

fn b3_write(path: &std::path::Path, value: &serde_json::Value) {
    std::fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

fn b3_settings_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(super::SETTINGS_FILE_NAME)
}

fn b3_sidecar_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(super::AGENTS_INSTANCE_FILE_NAME)
}

fn b3_overlay_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(super::SETTINGS_LOCAL_OVERRIDE_FILE_NAME)
}

fn b3_object(path: &std::path::Path) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap() {
        serde_json::Value::Object(object) => object,
        other => panic!("{} is not an object: {other}", path.display()),
    }
}

fn b3_labels(settings: &super::AppSettings) -> Vec<String> {
    settings.agents.iter().map(|a| a.label.clone()).collect()
}

fn b3_marker(settings: &super::AppSettings) -> Option<String> {
    settings
        .coding_agent_profiles
        .profile_labels_by_agent
        .get("alpha")
        .and_then(|cells| cells.get("A"))
        .cloned()
}

/// An agents file in the exact bytes the save writes for `label` and `marker`,
/// so a steady-state save that changes nothing leaves it byte-identical.
fn b3_seed_sidecar(dir: &std::path::Path, label: &str, marker: &str) -> Vec<u8> {
    let source = tempfile::tempdir().unwrap();
    let source_path = b3_settings_path(source.path());
    b3_write(&source_path, &b3_legacy_fixture(label, marker));
    let settings = super::load_settings_for_cli_from_path(&source_path);
    let mut groups = serde_json::Map::new();
    groups.insert(
        "agents".to_string(),
        serde_json::to_value(&settings.agents).unwrap(),
    );
    groups.insert(
        "codingAgentProfiles".to_string(),
        serde_json::to_value(&settings.coding_agent_profiles).unwrap(),
    );
    let bytes = serde_json::to_vec_pretty(&serde_json::Value::Object(groups)).unwrap();
    std::fs::write(b3_sidecar_path(dir), &bytes).unwrap();
    bytes
}

fn hash_activation_seed(sidecar: bool) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = b3_settings_path(temp.path());
    let mut settings = super::AppSettings {
        root_token: Some("hash-test-token".into()),
        agents: vec![serde_json::from_value(b3_agent("alpha", "Incumbent")).unwrap()],
        ..super::AppSettings::default()
    };
    super::validate_and_repair_settings(&mut settings).unwrap();
    super::normalize_agent_order(&mut settings.agents);
    let mut value = serde_json::to_value(settings).unwrap();
    super::replace_project_projection(value.as_object_mut().unwrap(), &serde_json::Map::new());
    value["agents"][0]
        .as_object_mut()
        .unwrap()
        .remove("id_hash");
    let groups = if sidecar {
        let object = value.as_object_mut().unwrap();
        serde_json::json!({
            "agents": object.remove("agents").unwrap(),
            "codingAgentProfiles": object.remove("codingAgentProfiles").unwrap()
        })
    } else {
        // Avoid the independent startup export; exercise missing-group legacy fallback.
        serde_json::json!({})
    };
    b3_write(&path, &value);
    b3_write(&b3_sidecar_path(temp.path()), &groups);
    // The existing project migration publishes even an empty projection.
    // Seed its destination before measuring hash-only read/rejection effects.
    b3_write(&super::project_paths_path(&path), &serde_json::json!({}));
    (temp, path)
}

fn hash_activation_files(dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && path.extension().is_none_or(|ext| ext != "lock"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(path).unwrap(),
            )
        })
        .collect()
}

/// #2716 (B3) 6.1 (b), `blockingMenus` alone: every seeded entry survives in
/// order and field for field; an extra entry is one of the command's shipped
/// defaults; an array that is not all defaults matches the seed exactly.
pub(super) fn b3_assert_blocking_menus_legs(
    seeded: Option<&serde_json::Value>,
    held: Option<&serde_json::Value>,
    command: &str,
    label: &str,
) {
    let Some(seeded) = seeded else {
        assert!(
            held.is_none(),
            "{label}: an omitted blockingMenus was written"
        );
        return;
    };
    let seeded = seeded.as_array().expect("seeded blockingMenus array");
    let held = held
        .unwrap_or_else(|| panic!("{label}: blockingMenus dropped"))
        .as_array()
        .unwrap_or_else(|| panic!("{label}: blockingMenus is not an array"));
    let defaults: Vec<serde_json::Value> = super::default_blocking_menus_for_command(command)
        .iter()
        .map(|entry| serde_json::to_value(entry).unwrap())
        .collect();
    if !seeded.iter().all(|entry| defaults.contains(entry)) {
        assert_eq!(held, seeded, "{label}: a customized array changed");
        return;
    }
    let mut next = 0;
    for entry in held {
        if next < seeded.len() && *entry == seeded[next] {
            next += 1;
        } else {
            assert!(
                defaults.contains(entry),
                "{label}: {entry} is neither seeded nor a shipped default"
            );
        }
    }
    assert_eq!(
        next,
        seeded.len(),
        "{label}: a seeded entry was dropped or edited"
    );
}

/// #2716 (B3) 6.1 - the split form of a whole-file comparison. (a) Containment:
/// every seeded key but the two groups keeps its seeded value, and a key the
/// save adds equals a default `AppSettings`, save the seven environment keys the
/// S6 normalizer names. (b) The agents file holds every seeded field of both
/// groups; `blockingMenus` takes the three legs; `skip` names agent fields the
/// caller asserts separately.
fn b3_assert_split_matches_seed(path: &std::path::Path, seeded: &[u8], skip: &[&str]) {
    let seed: serde_json::Value = serde_json::from_slice(seeded).unwrap();
    let mut seed = seed.as_object().unwrap().clone();
    let physical_agents = seed.get("agents").cloned();
    let ambiguous_rows = super::physical_profile_rows(
        &serde_json::json!({"agent\
                s":physical_agents.clone().unwrap_or_else(|| serde_json::json!([]))}),
        true,
    )
    .is_err();
    let mut disk = b3_object(path);
    let sidecar = b3_object(&super::agents_instance_path(path));
    for key in super::AGENTS_INSTANCE_KEYS {
        assert!(
            !disk.contains_key(key),
            "{key} is still in the settings file"
        );
        disk.remove(key);
        let Some(seeded_group) = seed.remove(key) else {
            continue;
        };
        let held = sidecar
            .get(key)
            .unwrap_or_else(|| panic!("the agents file does not hold {key}"));
        match (&seeded_group, held) {
            (serde_json::Value::Array(seeded_rows), serde_json::Value::Array(held_rows)) => {
                b3_assert_seeded_rows(key, seeded_rows, held_rows, skip);
            }
            (serde_json::Value::Object(seeded_fields), serde_json::Value::Object(held)) => {
                if held.get("profileBindingVersion") == Some(&serde_json::json!(1)) {
                    b3_assert_canonical_seeded_profiles(
                        seeded_fields,
                        held,
                        physical_agents.as_ref(),
                    );
                    continue;
                }
                b3_assert_legacy_seeded_profiles(
                    key,
                    seeded_fields,
                    held,
                    physical_agents.as_ref(),
                    ambiguous_rows,
                );
            }
            other => panic!("{key}: unexpected shapes {other:?}"),
        }
    }
    let exempt = [
        super::FIELD_PROJECT_PATH,
        super::FIELD_PROJECT_PATH_REL,
        super::FIELD_PROJECT_PATHS,
        super::FIELD_PROJECT_PATHS_REL,
        super::FIELD_ARCHIVED,
        super::FIELD_ARCHIVED_REL,
        "rootToken",
    ];
    // r12k: the reference is the seed decoded by the loader's typed path and
    // serialized again, compared key by key, both directions.
    let typed: super::AppSettings = serde_json::from_slice(seeded).unwrap();
    let mut reference = match serde_json::to_value(&typed).unwrap() {
        serde_json::Value::Object(object) => object,
        other => panic!("AppSettings serialized to {other}"),
    };
    for key in super::AGENTS_INSTANCE_KEYS.iter().chain(exempt.iter()) {
        reference.remove(*key);
        disk.remove(*key);
    }
    let mut differing: Vec<&String> = reference
        .keys()
        .chain(disk.keys())
        .filter(|key| reference.get(*key) != disk.get(*key))
        .collect();
    differing.sort();
    differing.dedup();
    assert!(
        differing.is_empty(),
        "keys differ from the typed seed: {differing:?}"
    );
}

fn b3_assert_seeded_rows(
    key: &str,
    seeded_rows: &[serde_json::Value],
    held_rows: &[serde_json::Value],
    skip: &[&str],
) {
    assert_eq!(seeded_rows.len(), held_rows.len(), "{key} length");
    for (seeded_row, held_row) in seeded_rows.iter().zip(held_rows) {
        let command = seeded_row
            .get("baseCommand")
            .or_else(|| seeded_row.get("command"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let mut leaves = vec![command.to_string()];
        for env in seeded_row
            .get("envs")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            leaves.push(env["key"].as_str().unwrap().to_string());
            leaves.push(env["value"].as_str().unwrap().to_string());
        }
        assert_eq!(held_row["id_hash_debug"], serde_json::json!(leaves));
        if seeded_row.get("blockingMenus").is_none() {
            b3_assert_blocking_menus_legs(
                None,
                held_row.get("blockingMenus"),
                "",
                &format!("{key}.{}", seeded_row["id"]),
            );
        }
        for (field, value) in seeded_row.as_object().unwrap() {
            if skip.contains(&field.as_str()) {
                continue;
            }
            if field == "blockingMenus" {
                b3_assert_blocking_menus_legs(
                    Some(value),
                    held_row.get(field),
                    seeded_row["command"].as_str().unwrap_or_default(),
                    &format!("{key}.{}", seeded_row["id"]),
                );
                continue;
            }
            assert_eq!(held_row.get(field), Some(value), "{key}.{field}");
        }
    }
}

fn b3_assert_canonical_seeded_profiles(
    seeded_fields: &serde_json::Map<String, serde_json::Value>,
    held: &serde_json::Map<String, serde_json::Value>,
    physical_agents: Option<&serde_json::Value>,
) {
    // Compare storage directly against the seeded physical rows,
    // including unknown cells and sibling fields, never an ID view.
    let mut expected_cells = serde_json::Map::new();
    let mut expected_labels = serde_json::Map::new();
    let mut resolved = std::collections::HashSet::new();
    for row in physical_agents
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let agent: super::AgentConfig = serde_json::from_value(row.clone()).unwrap();
        assert!(
            resolved.insert(agent.id.clone()),
            "ambiguous rows cannot convert"
        );
        let hash = super::configuration_identity(&agent.command, &agent.envs);
        let mut cells = seeded_fields
            .get("profilesByAgent")
            .and_then(|map| map.get(&agent.id))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        for cell in cells.as_object_mut().unwrap().values_mut() {
            let typed: super::ProfileCellConfig = serde_json::from_value(cell.clone()).unwrap();
            let mut defaults = serde_json::to_value(typed).unwrap();
            defaults.as_object_mut().unwrap().remove("id_hash");
            defaults.as_object_mut().unwrap().remove("id_hash_debug");
            let raw = cell.as_object_mut().unwrap();
            raw.remove("id_hash");
            raw.remove("id_hash_debug");
            raw.extend(defaults.as_object().unwrap().clone());
            for key in ["preflight", "preflightTimeoutSeconds"] {
                if raw.get(key).is_some_and(serde_json::Value::is_null) {
                    raw.remove(key);
                }
            }
        }
        cells
            .as_object_mut()
            .unwrap()
            .entry("A".to_string())
            .or_insert_with(|| serde_json::to_value(super::empty_profile_cell()).unwrap());
        let labels = seeded_fields
            .get("profileLabelsByAgent")
            .and_then(|map| map.get(&agent.id))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(previous) = expected_cells.insert(hash.clone(), cells.clone()) {
            assert_eq!(previous, cells, "complete alias cells");
        }
        if let Some(previous) = expected_labels.insert(hash, labels.clone()) {
            assert_eq!(previous, labels, "complete alias labels");
        }
    }
    assert_eq!(
        held.get("profilesByBaseHash"),
        Some(&serde_json::Value::Object(expected_cells))
    );
    assert_eq!(
        held.get("profileLabelsByBaseHash"),
        Some(&serde_json::Value::Object(expected_labels))
    );
    let mut orphan_records = serde_json::Map::new();
    for (map_key, record_key) in [
        ("profilesByAgent", "cells"),
        ("profileLabelsByAgent", "labels"),
        ("identityByAgent", "identity"),
    ] {
        for (id, raw) in seeded_fields
            .get(map_key)
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flatten()
        {
            if !resolved.contains(id) {
                let record = orphan_records
                    .entry(id.clone())
                    .or_insert_with(|| serde_json::json!([{}]));
                record[0][record_key] = raw.clone();
            }
        }
        assert!(!held.contains_key(map_key));
    }
    if !orphan_records.is_empty() {
        assert_eq!(
            held.get("legacyProfileOrphans"),
            Some(&serde_json::Value::Object(orphan_records))
        );
    }
    for (field, value) in seeded_fields {
        if !["profilesByAgent", "profileLabelsByAgent", "identityByAgent"].contains(&field.as_str())
        {
            assert_eq!(held.get(field), Some(value), "raw sibling {field}");
        }
    }
}

fn b3_assert_legacy_seeded_profiles(
    key: &str,
    seeded_fields: &serde_json::Map<String, serde_json::Value>,
    held: &serde_json::Map<String, serde_json::Value>,
    physical_agents: Option<&serde_json::Value>,
    ambiguous_rows: bool,
) {
    for (field, value) in seeded_fields {
        let mut expected = value.clone();
        if field == "profilesByAgent" && !ambiguous_rows {
            // Compare the complete original cells, including old
            // command values, plus only the new physical leaf.
            for (id, cells) in expected.as_object_mut().unwrap().iter_mut() {
                // First raw ID wins before decoding, including an
                // unresolvable first row. Never use effective/held.
                let base = physical_agents
                    .and_then(serde_json::Value::as_array)
                    .and_then(|rows| {
                        rows.iter().find(|row| {
                            row.get("id").and_then(serde_json::Value::as_str) == Some(id.as_str())
                        })
                    })
                    .and_then(|row| serde_json::from_value::<super::AgentConfig>(row.clone()).ok());
                for cell in cells.as_object_mut().unwrap().values_mut() {
                    assert!(cell.get("additionalArguments").is_none());
                    cell["additionalArguments"] = cell["command"].clone();
                    let profile: super::ProfileCellConfig =
                        serde_json::from_value(cell.clone()).unwrap();
                    let hash = base.as_ref().map(|base| {
                        // Independent raw-field preimage, every base
                        // env row (disabled included), then cell env.
                        let parts = std::iter::once(base.command.as_str())
                            .chain(
                                base.envs
                                    .iter()
                                    .flat_map(|env| [env.key.as_str(), env.value.as_str()]),
                            )
                            .chain(std::iter::once(profile.command.as_str()))
                            .chain(
                                profile
                                    .env
                                    .iter()
                                    .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
                            );
                        let mut chars: Vec<char> = parts
                            .flat_map(str::chars)
                            .flat_map(char::to_lowercase)
                            .filter(|c| !c.is_whitespace())
                            .collect();
                        chars.sort_unstable();
                        serde_json::Value::String(format!(
                            "cid1:{:x}",
                            <sha2::Sha256 as sha2::Digest>::digest(
                                chars.iter().collect::<String>()
                            )
                        ))
                    });
                    // Replace only expected derived metadata. None
                    // asserts concrete absence; observed stays raw.
                    let object = cell.as_object_mut().unwrap();
                    object.remove("id_hash");
                    object.extend(hash.map(|hash| ("id_hash".to_string(), hash)));
                    object.remove("id_hash_debug");
                    if let Some(base) = &base {
                        let mut leaves = vec![base.command.clone()];
                        for env in &base.envs {
                            leaves.extend([env.key.clone(), env.value.clone()]);
                        }
                        leaves.push(profile.command.clone());
                        for (key, value) in &profile.env {
                            leaves.extend([key.clone(), value.clone()]);
                        }
                        object.insert("id_hash_debug".into(), serde_json::json!(leaves));
                    }
                }
            }
        }
        assert_eq!(held.get(field), Some(&expected), "{key}.{field}");
    }
}

mod independent_source_tests;

mod source_identity_tests;

mod source_inventory_projection_tests;

mod derived_agent_binding_projection_tests;

mod p02_configuration_schema_tests;

mod p02_physical_schema_tests;

mod p02_duplicate_row_tests;

mod e2992_p41_owner_tests;

mod e2432_profile_storage_tests;

mod e2432_p02_physical_file_tests;

mod e2432_p02_closed_owner_tests;
