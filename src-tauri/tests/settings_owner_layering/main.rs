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

#[test]
fn registry_does_not_assign_settings_to_an_external_owner() {
    let registry: serde_json::Value =
        serde_json::from_str(&source("../scripts/containers.json")).unwrap();
    for entry in registry["containers"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        if name == "settings.json" || name == "settings.30.instance.json" {
            assert_eq!(entry["scope"], "src-tauri/src/config/settings/");
            assert!(entry["owner"]
                .as_str()
                .unwrap()
                .starts_with("src-tauri/src/config/settings/"));
            assert_eq!(entry["externalLiteralBaseline"], 0);
        }
    }
}
