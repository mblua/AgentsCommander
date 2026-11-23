//! #4845 unit tests of the config-file core (tempdirs, no settings types).

use super::*;
use std::fs;
use std::path::Path;

fn dirs_in(base: &Path) -> ConfigDirs {
    let instance_dir = base.join("instance");
    let project_dir = base.join("project");
    ConfigDirs {
        registered_path: instance_dir.join("agents.30.instance.no-git.json"),
        instance_dir: instance_dir.clone(),
        project: Some(("pkey".to_string(), project_dir.clone())),
        layers: vec![
            (
                ConfigScope::Instance,
                CatalogLayer::CatalogPersonal,
                instance_dir.join("agents.50.personal.no-git.json"),
            ),
            (
                ConfigScope::Instance,
                CatalogLayer::CatalogBase,
                instance_dir.join("agents.10.default.json"),
            ),
            (
                ConfigScope::Project,
                CatalogLayer::CatalogProject,
                project_dir.join("agents.40.project.json"),
            ),
            (
                ConfigScope::Project,
                CatalogLayer::CatalogBase,
                project_dir.join("agents.10.default.json"),
            ),
        ],
    }
}

fn code_of(result: Result<std::path::PathBuf, ConfigFileError>) -> &'static str {
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
    let base = tempfile::tempdir().unwrap();
    let dirs = dirs_in(base.path());
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
        code_of(resolve_locator(&sel, &dirs, &ctx, &unknown)),
        "configFileUnknown"
    );
}

#[test]
fn resolve_rejects_traversal_in_other_and_named() {
    let base = tempfile::tempdir().unwrap();
    let dirs = dirs_in(base.path());
    let sel = CodingAgentConfigFiles::default();
    let ctx = ConfigContext::Instance;
    for file_name in ["..\\x.json", "a/b.json", "../x", "C:x.json", "..", "."] {
        let loc = ConfigFileLocator::Other {
            scope: ConfigScope::Instance,
            file_name: file_name.to_string(),
        };
        let code = code_of(resolve_locator(&sel, &dirs, &ctx, &loc));
        assert_eq!(code, "configLocatorInvalid", "{file_name}");
    }
    for name in ["../x", "a/b", "C:x", "Team"] {
        let loc = ConfigFileLocator::Named {
            scope: ConfigScope::Instance,
            name: name.to_string(),
        };
        let code = code_of(resolve_locator(&sel, &dirs, &ctx, &loc));
        assert_eq!(code, "configLocatorInvalid", "{name}");
    }
    let good = ConfigFileLocator::Named {
        scope: ConfigScope::Instance,
        name: "team".to_string(),
    };
    assert_eq!(
        resolve_locator(&sel, &dirs, &ctx, &good).unwrap(),
        dirs.instance_dir.join("ac.coding-agents-config.team.json")
    );
    let other = ConfigFileLocator::Other {
        scope: ConfigScope::Instance,
        file_name: "agents.30.instance.old.json".to_string(),
    };
    assert!(resolve_locator(&sel, &dirs, &ctx, &other).is_ok());
}

#[test]
fn resolve_rejects_project_scope_outside_its_project() {
    let base = tempfile::tempdir().unwrap();
    let dirs = dirs_in(base.path());
    let sel = CodingAgentConfigFiles::default();
    let loc = ConfigFileLocator::Named {
        scope: ConfigScope::Project,
        name: "team".to_string(),
    };
    for ctx in [
        ConfigContext::Instance,
        ConfigContext::Project("other".to_string()),
    ] {
        let code = code_of(resolve_locator(&sel, &dirs, &ctx, &loc));
        assert_eq!(code, "configFileWrongProject");
    }
    let own = ConfigContext::Project("pkey".to_string());
    assert!(resolve_locator(&sel, &dirs, &own, &loc).is_ok());
}

#[test]
fn resolve_rejects_unknown_layer_pair() {
    let base = tempfile::tempdir().unwrap();
    let dirs = dirs_in(base.path());
    let sel = CodingAgentConfigFiles::default();
    let loc = ConfigFileLocator::Layer {
        scope: ConfigScope::Instance,
        layer: CatalogLayer::CatalogProject,
    };
    let ctx = ConfigContext::Project("pkey".to_string());
    assert_eq!(
        code_of(resolve_locator(&sel, &dirs, &ctx, &loc)),
        "configLocatorInvalid"
    );
    let ok = ConfigFileLocator::Layer {
        scope: ConfigScope::Project,
        layer: CatalogLayer::CatalogProject,
    };
    assert_eq!(
        resolve_locator(&sel, &dirs, &ctx, &ok).unwrap(),
        dirs.layers[2].2
    );
}

#[test]
fn context_key_is_one_normalizer() {
    if cfg!(windows) {
        assert_eq!(context_key(r"\\?\C:\p\"), context_key(r"C:\p"));
        assert_eq!(context_key("C:/P/"), context_key(r"c:\p"));
    } else {
        assert_eq!(context_key("/p/"), context_key("/p"));
        assert_eq!(context_key("/p/"), "/p");
    }
}

#[test]
fn context_for_cwd_picks_longest_project_root() {
    let roots = vec!["/p".to_string(), "/p/inner/".to_string()];
    let ctx = context_for_cwd(&roots, "/p/inner/agent");
    assert_eq!(ctx, ConfigContext::Project(context_key("/p/inner")));
    assert_eq!(
        context_for_cwd(&roots, "/p/x"),
        ConfigContext::Project(context_key("/p"))
    );
    assert_eq!(
        context_for_cwd(&roots, "/p"),
        ConfigContext::Project(context_key("/p"))
    );
    assert_eq!(context_for_cwd(&roots, "/p2"), ConfigContext::Instance);
    assert_eq!(
        context_for_root(&roots, None).unwrap(),
        ConfigContext::Instance
    );
    assert_eq!(
        context_for_root(&roots, Some("/p/")).unwrap(),
        ConfigContext::Project(context_key("/p"))
    );
    let unknown = context_for_root(&roots, Some("/q")).expect_err("unknown root");
    assert_eq!(unknown.code, "configContextUnknown");
}

#[test]
fn inventory_lists_layers_named_other_and_hides_unrelated() {
    let base = tempfile::tempdir().unwrap();
    let dirs = dirs_in(base.path());
    let (_, project_dir) = dirs.project.clone().unwrap();
    fs::create_dir_all(&dirs.instance_dir).unwrap();
    fs::create_dir_all(&project_dir).unwrap();
    for name in [
        "agents.30.instance.no-git.json",
        "agents.50.personal.no-git.json",
        "agents.10.default.json",
        "ac.coding-agents-config.zeta.json",
        "ac.coding-agents-config.alpha.json",
        "ac.coding-agents-config.Bad Name.json",
        "agents.30.instance.old.json",
        "agents.local.FUERA.json",
        "agents.local.json",
        "notes.txt",
    ] {
        fs::write(dirs.instance_dir.join(name), "{}").unwrap();
    }
    fs::create_dir(dirs.instance_dir.join("ac.coding-agents-config.dir.json")).unwrap();
    fs::write(project_dir.join("ac.coding-agents-config.team.json"), "{}").unwrap();
    let mut sel = CodingAgentConfigFiles::default();
    sel.loaded.push("/abs/loaded.json".to_string());
    let ctx = ConfigContext::Project("pkey".to_string());
    let entries = list_entries(&sel, &dirs, &ctx);
    let listed: Vec<(EntryGroup, &str)> = entries
        .iter()
        .map(|entry| (entry.group, entry.file_name.as_str()))
        .collect();
    assert_eq!(
        listed,
        vec![
            (EntryGroup::Instance, "agents.30.instance.no-git.json"),
            (EntryGroup::Instance, "agents.50.personal.no-git.json"),
            (EntryGroup::Instance, "ac.coding-agents-config.alpha.json"),
            (EntryGroup::Instance, "ac.coding-agents-config.zeta.json"),
            (EntryGroup::Instance, "agents.10.default.json"),
            (EntryGroup::Project, "agents.40.project.json"),
            (EntryGroup::Project, "ac.coding-agents-config.team.json"),
            (EntryGroup::Project, "agents.10.default.json"),
            (EntryGroup::Loaded, "loaded.json"),
            (EntryGroup::Other, "ac.coding-agents-config.Bad Name.json"),
            (EntryGroup::Other, "agents.30.instance.old.json"),
        ]
    );
    let instance_only = list_entries(&sel, &dirs, &ConfigContext::Instance);
    assert!(instance_only
        .iter()
        .all(|entry| entry.group != EntryGroup::Project));
}

#[derive(serde::Deserialize)]
struct Holder {
    #[serde(default, deserialize_with = "lenient_selection")]
    value: Option<CodingAgentConfigFiles>,
}

fn read(json: serde_json::Value) -> Option<CodingAgentConfigFiles> {
    serde_json::from_value::<Holder>(json).unwrap().value
}

#[test]
fn bad_selection_value_reads_as_none() {
    for bad in [
        serde_json::json!({ "value": 7 }),
        serde_json::json!({ "value": "x" }),
        serde_json::json!({ "value": { "activeByContext": {} } }),
        serde_json::json!({ "value": { "version": 1, "loaded": "x" } }),
        serde_json::json!({ "value": null }),
        serde_json::json!({}),
    ] {
        assert_eq!(read(bad.clone()), None, "{bad}");
    }
}

#[test]
fn selection_drops_undecodable_entries() {
    let sel = read(serde_json::json!({ "value": { "version": 1,
        "activeByContext": {
            "instance": { "kind": "registeredInstance" },
            "/abs/project": { "kind": "named", "scope": "project", "name": "team" },
            "/abs/bad": { "kind": "nope" },
            "/abs/extra": { "kind": "loaded", "path": "/x", "more": 1 },
            "/abs/other": { "kind": "other", "scope": "instance", "fileName": "a.json" }
        },
        "loaded": ["/abs/path/team.json"] } }))
    .unwrap();
    let keys: Vec<&str> = sel.active_by_context.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["/abs/other", "/abs/project", "instance"]);
    assert_eq!(sel.loaded, vec!["/abs/path/team.json".to_string()]);
    let text = serde_json::to_value(&sel).unwrap();
    assert_eq!(text["activeByContext"]["/abs/other"]["fileName"], "a.json");
}

#[test]
fn absent_selection_is_registered_instance() {
    let ctx = ConfigContext::Project("/p".to_string());
    let registered = ConfigFileLocator::RegisteredInstance {};
    assert_eq!(active_locator(None, &ctx), registered);
    let named = ConfigFileLocator::Named {
        scope: ConfigScope::Project,
        name: "team".to_string(),
    };
    let sel = with_active(None, &ctx, named.clone());
    assert_eq!(active_locator(Some(&sel), &ctx), named);
    assert_eq!(
        active_locator(Some(&sel), &ConfigContext::Instance),
        registered
    );
    let sel = with_loaded(Some(&sel), "/a");
    let sel = with_loaded(Some(&sel), "/b");
    let sel = with_loaded(Some(&sel), "/a");
    assert_eq!(sel.loaded, vec!["/a".to_string(), "/b".to_string()]);
    assert_eq!(sel.version, 1);
}
