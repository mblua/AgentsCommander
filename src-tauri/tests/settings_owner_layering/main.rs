//! #3843 source guards for the production settings facade.
//! These inspect explicit declarations; gate 04 owns runtime token proof.
//! Test-only seams and serde transport projections remain permitted.

use std::path::Path;
use syn::{Item, UseTree, Visibility};

const RAW_EXPORTS: &[&str] = &[
    "settings_path",
    "begin_project_paths_transaction_at_path",
    "repair_fresh_project_paths_at_path",
    "ProjectPathsWriteTransaction",
    "load_local_agent_help_file",
    "load_remote_agent_help_file",
    "read_remote_agent_help_check_stamp",
    "read_remote_blocking_menus_check_stamp",
    "write_remote_agent_help_cache",
    "write_remote_agent_help_check_stamp",
    "write_remote_blocking_menus_cache",
    "write_remote_blocking_menus_check_stamp",
];

fn source(relative: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)).unwrap()
}

fn test_only(attributes: &[syn::Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && attribute
                .parse_args::<syn::Path>()
                .is_ok_and(|path| path.is_ident("test"))
    })
}

fn exported_names(tree: &UseTree, names: &mut Vec<String>) {
    match tree {
        UseTree::Path(path) => exported_names(&path.tree, names),
        UseTree::Name(name) => names.push(name.ident.to_string()),
        UseTree::Rename(rename) => {
            names.push(rename.ident.to_string());
            names.push(rename.rename.to_string());
        }
        UseTree::Group(group) => {
            for item in &group.items {
                exported_names(item, names);
            }
        }
        UseTree::Glob(_) => panic!("settings facade must enumerate exports"),
    }
}

#[test]
fn production_facade_hides_raw_persistence_and_project_module() {
    let file = syn::parse_file(&source("src/config/settings.rs")).unwrap();
    let mut private_project_module = false;
    for item in file.items {
        match item {
            Item::Use(item)
                if !test_only(&item.attrs) && !matches!(item.vis, Visibility::Inherited) =>
            {
                let mut names = Vec::new();
                exported_names(&item.tree, &mut names);
                for name in names {
                    assert!(!RAW_EXPORTS.contains(&name.as_str()), "raw export: {name}");
                }
            }
            Item::Mod(item) if item.ident == "project_path" => {
                assert!(matches!(item.vis, Visibility::Inherited));
                private_project_module = true;
            }
            _ => {}
        }
    }
    assert!(private_project_module);
}

#[test]
fn projects_uses_the_typed_root_facade() {
    let text = source("src/config/projects.rs");
    assert!(!text.contains("settings::project_path"));
    assert!(text.contains("super::settings::{"));
    // IPC serialization is still a supported consumer contract.
    assert!(text.contains("serde::Serialize"));
}

#[test]
fn owner_stays_in_staged_compatibility() {
    let text = source("src/config/settings/owner.rs");
    assert!(text.contains("SettingsOwnerStage::StagedCompatibility,"));
    assert!(text.contains("\"lock forwarding is StagedCompatibility only\""));
    assert!(text.contains("StagedCompatibility forwarding:"));
}

const SETTINGS_NAME: &str = "settings.30.instance.no-git.json";

fn registry_is_permitted(registry: &serde_json::Value) -> bool {
    let Some(entries) = registry["containers"].as_array() else {
        return false;
    };
    if entries.is_empty() {
        return false;
    }
    let settings: Vec<_> = entries
        .iter()
        .filter(|entry| entry["name"] == SETTINGS_NAME)
        .collect();
    // P10 has no settings declaration; P11 may add exactly this owner declaration.
    settings.len() <= 1
        && entries.iter().all(|entry| {
            entry["externalLiteralBaseline"] == 0
                && entry["name"] != "settings.json"
                && entry["name"] != "settings.30.instance.json"
        })
        && settings.iter().all(|entry| {
            entry["scope"] == "src-tauri/src/"
                && entry["owner"] == "src-tauri/src/config/settings.rs"
        })
}

#[test]
fn registry_does_not_assign_settings_to_an_external_owner() {
    let registry: serde_json::Value =
        serde_json::from_str(&source("../scripts/containers.json")).unwrap();
    assert!(registry_is_permitted(&registry));
    // Exercise the optional P11 declaration even while it is absent in P10.
    let mut future = registry.clone();
    future["containers"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["name"] != SETTINGS_NAME);
    future["containers"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "name": SETTINGS_NAME,
            "scope": "src-tauri/src/",
            "owner": "src-tauri/src/config/settings.rs",
            "externalLiteralBaseline": 0
        }));
    assert!(registry_is_permitted(&future));
    let entries = future["containers"].as_array_mut().unwrap();
    let entry = entries.last_mut().unwrap();
    entry["owner"] = serde_json::json!("src-tauri/src/commands/x.rs");
    assert!(!registry_is_permitted(&future));
    let entry = future["containers"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap();
    entry["owner"] = serde_json::json!("src-tauri/src/config/settings.rs");
    entry["externalLiteralBaseline"] = serde_json::json!(5);
    assert!(!registry_is_permitted(&future));
}
