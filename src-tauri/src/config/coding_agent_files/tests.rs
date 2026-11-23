use super::inventory::list_entries;
use super::locator::{context_for_cwd, context_key, normalize_config_name, resolve_locator};
use super::*;
use serde_json::json;
use std::path::Path;

fn dirs_in(root: &Path, project: Option<&str>) -> ConfigDirs {
    let instance_dir = root.join("instance");
    let project_dir = root.join("project");
    ConfigDirs {
        registered_path: root.join("agents.30.instance.json"),
        instance_dir: instance_dir.clone(),
        project: project.map(|key| (key.to_string(), project_dir.clone())),
        layers: vec![
            (
                ConfigScope::Instance,
                CatalogLayer::CatalogBase,
                instance_dir.join("agents.50.base.json"),
            ),
            (
                ConfigScope::Instance,
                CatalogLayer::CatalogPersonal,
                instance_dir.join("agents.10.personal.json"),
            ),
            (
                ConfigScope::Project,
                CatalogLayer::CatalogProject,
                project_dir.join("agents.40.project.json"),
            ),
        ],
    }
}

fn code(result: Result<std::path::PathBuf, ConfigFileError>) -> &'static str {
    result.expect_err("must be rejected").code
}

#[test]
fn name_rule_accepts_and_rejects() {
    assert_eq!(normalize_config_name("Mariano").unwrap(), "mariano");
    assert_eq!(normalize_config_name(" team-1 ").unwrap(), "team-1");
    let long = "a".repeat(49);
    for bad in ["a b", "../x", "", "-x", long.as_str()] {
        let error = normalize_config_name(bad).expect_err(bad);
        assert_eq!(error.code, "configNameInvalid");
    }
    assert!(normalize_config_name(&"a".repeat(48)).is_ok());
}

#[test]
fn resolve_rejects_unregistered_loaded_path() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path(), None);
    let mut sel = CodingAgentConfigFiles::default();
    sel.loaded.push("/abs/known.json".to_string());
    let known = ConfigFileLocator::Loaded {
        path: "/abs/known.json".to_string(),
    };
    let ctx = ConfigContext::Instance;
    assert_eq!(
        resolve_locator(&sel, &dirs, &ctx, &known).unwrap(),
        Path::new("/abs/known.json")
    );
    let unknown = ConfigFileLocator::Loaded {
        path: "/abs/other.json".to_string(),
    };
    assert_eq!(
        code(resolve_locator(&sel, &dirs, &ctx, &unknown)),
        "configFileUnknown"
    );
}

#[test]
fn resolve_rejects_traversal_in_other_and_named() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path(), None);
    let sel = CodingAgentConfigFiles::default();
    let ctx = ConfigContext::Instance;
    for file_name in ["..\\x.json", "a/b.json", "../x", "C:x.json", ".", ".."] {
        let loc = ConfigFileLocator::Other {
            scope: ConfigScope::Instance,
            file_name: file_name.to_string(),
        };
        assert_eq!(
            code(resolve_locator(&sel, &dirs, &ctx, &loc)),
            "configLocatorInvalid"
        );
    }
    for name in ["../x", "a/b", "Team", "C:x"] {
        let loc = ConfigFileLocator::Named {
            scope: ConfigScope::Instance,
            name: name.to_string(),
        };
        assert_eq!(
            code(resolve_locator(&sel, &dirs, &ctx, &loc)),
            "configLocatorInvalid"
        );
    }
    let good = ConfigFileLocator::Named {
        scope: ConfigScope::Instance,
        name: "team".to_string(),
    };
    let path = resolve_locator(&sel, &dirs, &ctx, &good).unwrap();
    assert_eq!(
        path,
        dirs.instance_dir.join("ac.coding-agents-config.team.json")
    );
    let other = ConfigFileLocator::Other {
        scope: ConfigScope::Instance,
        file_name: "ac.coding-agents-config.Bad Name.json".to_string(),
    };
    assert!(resolve_locator(&sel, &dirs, &ctx, &other).is_ok());
}

#[test]
fn resolve_rejects_project_scope_outside_its_project() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path(), Some("/p"));
    let sel = CodingAgentConfigFiles::default();
    let loc = ConfigFileLocator::Named {
        scope: ConfigScope::Project,
        name: "team".to_string(),
    };
    let wrong = ConfigContext::Project("/q".to_string());
    for ctx in [ConfigContext::Instance, wrong] {
        assert_eq!(
            code(resolve_locator(&sel, &dirs, &ctx, &loc)),
            "configFileWrongProject"
        );
    }
    let ctx = ConfigContext::Project("/p".to_string());
    assert!(resolve_locator(&sel, &dirs, &ctx, &loc).is_ok());
}

#[test]
fn resolve_rejects_unknown_layer_pair() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path(), None);
    let sel = CodingAgentConfigFiles::default();
    let loc = ConfigFileLocator::Layer {
        scope: ConfigScope::Instance,
        layer: CatalogLayer::CatalogProject,
    };
    let ctx = ConfigContext::Instance;
    assert_eq!(
        code(resolve_locator(&sel, &dirs, &ctx, &loc)),
        "configLocatorInvalid"
    );
    let base = ConfigFileLocator::Layer {
        scope: ConfigScope::Instance,
        layer: CatalogLayer::CatalogBase,
    };
    assert_eq!(
        resolve_locator(&sel, &dirs, &ctx, &base).unwrap(),
        dirs.layers[0].2
    );
}

#[test]
fn context_key_is_one_normalizer() {
    #[cfg(windows)]
    assert_eq!(context_key(r"\\?\C:\p\"), context_key(r"C:\p"));
    #[cfg(not(windows))]
    {
        assert_eq!(context_key("/p/"), context_key("/p"));
        assert_eq!(context_key("/p"), "/p");
    }
}

#[test]
fn context_for_cwd_picks_longest_project_root() {
    let projects = vec!["/p".to_string(), "/p/inner/".to_string()];
    let key = |raw: &str| ConfigContext::Project(context_key(raw));
    assert_eq!(context_for_cwd(&projects, "/p/inner/x"), key("/p/inner"));
    assert_eq!(context_for_cwd(&projects, "/p/x"), key("/p"));
    assert_eq!(context_for_cwd(&projects, "/p"), key("/p"));
    assert_eq!(context_for_cwd(&projects, "/p2"), ConfigContext::Instance);
}

#[test]
fn inventory_lists_layers_named_other_and_hides_unrelated() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = dirs_in(tmp.path(), Some("/p"));
    std::fs::create_dir_all(&dirs.instance_dir).unwrap();
    let files = [
        "agents.50.base.json",
        "ac.coding-agents-config.zeta.json",
        "ac.coding-agents-config.alpha.json",
        "ac.coding-agents-config.Bad Name.json",
        "agents.local.FUERA.json",
        "agents.20.instance.extra.json",
        "notes.txt",
    ];
    for name in files {
        std::fs::write(dirs.instance_dir.join(name), "{}").unwrap();
    }
    let mut sel = CodingAgentConfigFiles::default();
    sel.loaded.push("/abs/team.json".to_string());
    let ctx = ConfigContext::Project("/p".to_string());
    let listed: Vec<(EntryGroup, String)> = list_entries(&sel, &dirs, &ctx)
        .into_iter()
        .map(|entry| (entry.group, entry.file_name))
        .collect();
    let want = |group, name: &str| (group, name.to_string());
    assert_eq!(
        listed,
        vec![
            want(EntryGroup::Instance, "agents.30.instance.json"),
            want(EntryGroup::Instance, "agents.50.base.json"),
            want(EntryGroup::Instance, "ac.coding-agents-config.alpha.json"),
            want(EntryGroup::Instance, "ac.coding-agents-config.zeta.json"),
            want(EntryGroup::Instance, "agents.10.personal.json"),
            want(EntryGroup::Project, "agents.40.project.json"),
            want(EntryGroup::Loaded, "team.json"),
            want(EntryGroup::Other, "ac.coding-agents-config.Bad Name.json"),
            want(EntryGroup::Other, "agents.20.instance.extra.json"),
        ]
    );
}

#[derive(serde::Deserialize)]
struct Holder {
    #[serde(default, deserialize_with = "lenient_selection")]
    value: Option<CodingAgentConfigFiles>,
}

fn read(value: serde_json::Value) -> Option<CodingAgentConfigFiles> {
    serde_json::from_value::<Holder>(json!({ "value": value }))
        .unwrap()
        .value
}

#[test]
fn bad_selection_value_reads_as_none() {
    for bad in [
        json!(5),
        json!("x"),
        json!({ "version": 1 }),
        json!({ "loaded": 3 }),
    ] {
        assert_eq!(read(bad), None);
    }
    let settings: crate::config::settings::AppSettings = serde_json::from_value(json!({
        "defaultShell": "sh", "defaultShellArgs": [], "agents": [],
        "codingAgentConfigFiles": [1, 2]
    }))
    .unwrap();
    assert_eq!(settings.coding_agent_config_files, None);
}

#[test]
fn selection_drops_undecodable_entries() {
    let value = json!({ "version": 1,
        "activeByContext": { "instance": { "kind": "registeredInstance" },
            "/abs/project": { "kind": "named", "scope": "project", "name": "team" },
            "/abs/bad": { "kind": "nope" },
            "/abs/extra": { "kind": "loaded", "path": "/x", "more": 1 } },
        "loaded": ["/abs/path/team.json"] });
    let sel = read(value).expect("good value");
    let keys: Vec<&str> = sel.active_by_context.keys().map(String::as_str).collect();
    assert_eq!(keys, ["/abs/project", "instance"]);
    assert_eq!(
        sel.active_by_context["/abs/project"],
        ConfigFileLocator::Named {
            scope: ConfigScope::Project,
            name: "team".to_string()
        }
    );
    assert_eq!(sel.loaded, ["/abs/path/team.json"]);
    let round = serde_json::to_value(&sel).unwrap();
    assert_eq!(
        round["activeByContext"]["instance"],
        json!({ "kind": "registeredInstance" })
    );
}

#[test]
fn absent_selection_is_registered_instance() {
    let ctx = ConfigContext::Project("/p".to_string());
    assert_eq!(
        active_locator(None, &ctx),
        ConfigFileLocator::RegisteredInstance {}
    );
    let sel = with_active(
        None,
        &ConfigContext::Instance,
        ConfigFileLocator::Loaded {
            path: "/x".to_string(),
        },
    );
    assert_eq!(
        active_locator(Some(&sel), &ctx),
        ConfigFileLocator::RegisteredInstance {}
    );
    assert_eq!(
        sel.active_by_context.keys().next().map(String::as_str),
        Some(INSTANCE_CONTEXT_KEY)
    );
    let twice = with_loaded(Some(&with_loaded(None, "/a")), "/a");
    assert_eq!(with_loaded(Some(&twice), "/b").loaded, ["/a", "/b"]);
    assert_eq!(read(serde_json::Value::Null), None);
}
