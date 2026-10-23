use super::builtin_support::builtin_agent_support_ships_only_muse_disabled;
use super::embedded_default::{
    embedded_default_matches_current_presets_exactly,
    install_commands_2736_shipped_for_eight_builtins_and_absent_for_muse,
};
use super::install_commands_2736::{
    install_commands_2736_local_layer_patches_windows_and_inherits_default,
    install_commands_2736_managed_base_refreshes_from_the_previous_revision,
    install_commands_2736_one_bad_local_value_discards_the_entire_local_layer,
    install_commands_2736_pin_wire_keeps_presence_semantics,
};
use super::project_layers::{
    project_absence_preserves_direct_report_and_catalog,
    project_invalid_schema_preserves_base_and_independent_personal,
    project_ownership_support_and_unavailable_base_contract,
    project_rejection_has_no_donor_and_personal_rejection_keeps_project,
    project_then_personal_merges_fields_nulls_and_exact_order,
};
use super::*;

#[test]
fn scope_b_catalog_2800_exact_32_cells() {
    let expected: serde_json::Value = serde_json::from_str(SCOPE_B_EXPECTED_2800).unwrap();
    let actual: serde_json::Value = serde_json::from_str(EMBEDDED_DEFAULT_CATALOG_JSON).unwrap();
    assert_eq!(expected.as_object().unwrap().len(), 8);
    let mut cells = 0;
    for (key, commands) in expected.as_object().unwrap() {
        let row = actual["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == key.as_str())
            .unwrap();
        assert_eq!(row["installCommands"], *commands, "{key}");
        assert_eq!(commands.as_object().unwrap().len(), 4);
        for os in ["windows", "macos", "linux", "default"] {
            assert!(!commands[os].as_str().unwrap().is_empty());
            cells += 1;
        }
        let definition = embedded_default_catalog()
            .agents
            .into_iter()
            .find(|row| row.key == *key)
            .unwrap();
        let typed = definition.install_commands.unwrap();
        for os in ["windows", "macos", "linux", "default"] {
            assert_eq!(
                install_command_for_os(&typed, os),
                commands[os].as_str().unwrap()
            );
        }
    }
    assert_eq!(cells, 32);
    embedded_default_matches_current_presets_exactly();
    install_commands_2736_shipped_for_eight_builtins_and_absent_for_muse();
}

#[test]
fn scope_b_catalog_2800_non_install_fields_unchanged() {
    let expected: serde_json::Value = serde_json::from_str(SCOPE_B_NON_INSTALL_MAIN_2800).unwrap();
    let mut actual: serde_json::Value =
        serde_json::from_str(EMBEDDED_DEFAULT_CATALOG_JSON).unwrap();
    for row in actual["agents"].as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("installCommands");
    }
    assert_eq!(
        actual, expected,
        "pinned main fields/schema/order/ninth row"
    );
}

#[test]
fn scope_b_catalog_2800_unknown_os_and_missing_override_use_default() {
    let mut commands = InstallCommands {
        default: "default sentinel".to_string(),
        windows: Some("windows sentinel".to_string()),
        macos: Some("macos sentinel".to_string()),
        linux: Some("linux sentinel".to_string()),
    };
    for unknown in ["unknown", "freebsd", "android", ""] {
        assert_eq!(
            install_command_for_os(&commands, unknown),
            "default sentinel"
        );
    }
    commands.windows = None;
    commands.macos = None;
    commands.linux = None;
    for os in ["windows", "macos", "linux"] {
        assert_eq!(install_command_for_os(&commands, os), "default sentinel");
    }
}

#[test]
fn scope_b_catalog_2800_host_selector_matches_supported_cfg() {
    let commands = InstallCommands {
        default: "default sentinel".to_string(),
        windows: Some("windows sentinel".to_string()),
        macos: Some("macos sentinel".to_string()),
        linux: Some("linux sentinel".to_string()),
    };
    let expected = if cfg!(target_os = "windows") {
        "windows sentinel"
    } else if cfg!(target_os = "macos") {
        "macos sentinel"
    } else if cfg!(target_os = "linux") {
        "linux sentinel"
    } else {
        "default sentinel"
    };
    assert_eq!(resolve_install_command(&commands), expected);
}

#[test]
fn scope_b_catalog_2800_project_and_personal_composition_preserved() {
    // Reuse the exact existing main regressions; no copied implementation/harness.
    project_absence_preserves_direct_report_and_catalog();
    project_then_personal_merges_fields_nulls_and_exact_order();
    install_commands_2736_local_layer_patches_windows_and_inherits_default();
    install_commands_2736_one_bad_local_value_discards_the_entire_local_layer();
    install_commands_2736_managed_base_refreshes_from_the_previous_revision();
    install_commands_2736_pin_wire_keeps_presence_semantics();
}

#[test]
fn scope_b_catalog_2800_malformed_project_and_muse_policy_preserved() {
    project_invalid_schema_preserves_base_and_independent_personal();
    project_rejection_has_no_donor_and_personal_rejection_keeps_project();
    project_ownership_support_and_unavailable_base_contract();
    builtin_agent_support_ships_only_muse_disabled();
}
