use super::*;

#[test]
fn embedded_default_parses_with_nine_agents_in_order() {
    let catalog = embedded_default_catalog();
    assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
    let keys: Vec<&str> = catalog.agents.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "claude",
            "codex",
            "hermes",
            "cursor",
            "pi",
            "opencode",
            "antigravity",
            "grok",
            "muse"
        ]
    );
    // Muse stays last, now immediately preceded by Grok.
    let last = catalog.agents.last().unwrap();
    assert_eq!(last.key, "muse");
    assert_eq!(last.command, "muse");
}

#[test]
pub(crate) fn embedded_default_matches_current_presets_exactly() {
    // Drift guard for every current preset field.
    let catalog = embedded_default_catalog();
    assert_eq!(catalog.agents.len(), EXPECTED_PRESETS.len());
    for (def, (key, label, desc, color, command, filename, seed_dest)) in
        catalog.agents.iter().zip(EXPECTED_PRESETS)
    {
        assert_eq!(def.key, key);
        assert_eq!(def.label, label);
        assert_eq!(def.description, desc);
        assert_eq!(def.color, color);
        assert_eq!(def.command, command);
        assert_eq!(def.instructions_filename.as_deref(), filename);
        // #769 P2: Claude/Codex/OpenCode ship an active configSeed; the other
        // six ship none (no master, no re-seed button).
        match seed_dest {
            Some(dest) => {
                let cs = def
                    .config_seed
                    .as_ref()
                    .unwrap_or_else(|| panic!("{key} must ship configSeed"));
                assert!(cs.enabled, "{key} configSeed must be enabled");
                assert_eq!(cs.dest, dest, "{key} configSeed dest");
            }
            None => assert!(
                def.config_seed.is_none(),
                "{key} must ship configSeed UNSET"
            ),
        }
        assert!(def.removable, "{key} must be removable");
        assert!(def.envs.is_empty());
        assert!(!def.isolated_home);
        // #2124 - every shipped row carries the conservative burst filter.
        let burst = def
            .idle_burst
            .as_ref()
            .unwrap_or_else(|| panic!("{key} must ship idleBurst"));
        assert_eq!(burst.max_bytes, Some(1024), "{key} idleBurst maxBytes");
        assert_eq!(burst.max_secs, Some(3.0), "{key} idleBurst maxSecs");
        assert_eq!(
            burst.prior_silence_secs,
            Some(60.0),
            "{key} idleBurst priorSilenceSecs"
        );
        assert_shipped_install_commands_2736(key, def);
    }
    let raw: serde_json::Value = serde_json::from_str(EMBEDDED_DEFAULT_CATALOG_JSON).unwrap();
    assert_eq!(raw["schemaVersion"], 1);
    assert_eq!(
        raw["agents"].as_array().unwrap().last().unwrap(),
        &serde_json::json!({
            "key": "muse",
            "label": "Muse Code",
            "description": "Meta terminal coding agent (beta; macOS/Linux host only)",
            "color": "#0668E1",
            "baseCommand": "muse",
            "envs": [],
            "isolatedHome": false,
            "removable": true,
            "updateCommands": [],
            "autoUpdate": false,
            "idleBurst": {
                "maxBytes": 1024,
                "maxSecs": 3.0,
                "priorSilenceSecs": 60.0
            }
        })
    );
    assert_eq!(
        catalog
            .agents
            .iter()
            .filter(|def| def.config_seed.is_some())
            .count(),
        3
    );
    assert_eq!(
        catalog
            .agents
            .iter()
            .filter(|def| def.config_seed.is_none())
            .count(),
        6
    );
}

#[test]
fn every_embedded_entry_validates() {
    for def in embedded_default_catalog().agents {
        validate_definition(&def).unwrap_or_else(|e| panic!("{}: {e}", def.key));
    }
}

/// #2124 T9 - serde, strict local layer and legacy migration for `idleBurst`.
#[test]
fn idle_burst_serde_and_local_layer() {
    // A definition without `idleBurst` deserializes `None` and does not
    // re-emit the key.
    let mut plain = shipped_def_json(&["claude"]).remove(0);
    plain.as_object_mut().unwrap().remove("idleBurst");
    let definition: CodingAgentDefinition = serde_json::from_value(plain).unwrap();
    assert!(definition.idle_burst.is_none());
    assert!(!serde_json::to_string(&definition)
        .unwrap()
        .contains("idleBurst"));

    // The embedded default ships the filter on all nine rows.
    let embedded = embedded_default_catalog();
    assert_eq!(embedded.agents.len(), 9);
    for row in &embedded.agents {
        let burst = row
            .idle_burst
            .as_ref()
            .unwrap_or_else(|| panic!("{} must ship idleBurst", row.key));
        assert_eq!(burst.max_bytes, Some(1024), "{}", row.key);
        assert_eq!(burst.max_secs, Some(3.0), "{}", row.key);
        assert_eq!(burst.prior_silence_secs, Some(60.0), "{}", row.key);
    }

    // Compose a managed base (1024/3/60) with a local object patch: the
    // stated subfield replaces, the absent ones inherit.
    let dir = seed_dir();
    let revision = managed_content_sha256(&supported_shipped_definitions());
    write_managed_base(dir.path(), &shipped_def_json(&["claude"]), &revision, true);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxBytes":2048}}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none(), "{:?}", report.unavailable);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let claude = report
        .catalog
        .iter()
        .find(|def| def.key == "claude")
        .unwrap();
    let burst = claude.idle_burst.as_ref().expect("composed idleBurst");
    assert_eq!(burst.max_bytes, Some(2048));
    assert_eq!(burst.max_secs, Some(3.0), "absent maxSecs inherits");
    assert_eq!(burst.prior_silence_secs, Some(60.0));

    // Local `null` clears the whole object (filter off).
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":null}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.catalog[0].idle_burst.is_none());

    // Unknown subfields and invalid value shapes disable the whole local
    // layer; the base entry stays intact.
    for local in [
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"foo":1}}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxBytes":-1}}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxSecs":"3"}}]}"##,
        r##"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxSecs":-1}}]}"##,
    ] {
        write_local(dir.path(), local);
        let report = load_catalog_report(dir.path());
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.code == "localInvalid"),
            "{local}: {:?}",
            report.warnings
        );
        let claude = report
            .catalog
            .iter()
            .find(|def| def.key == "claude")
            .unwrap();
        assert_eq!(
            claude.idle_burst.as_ref().unwrap().max_bytes,
            Some(1024),
            "the base entry stays intact after a rejected local layer"
        );
    }

    // A legacy row that states `idleBurst` explicitly pins it in the
    // extracted local layer, presence only.
    let mut legacy_claude = shipped_def_json(&["claude"]).remove(0);
    legacy_claude["idleBurst"] = serde_json::json!({"maxSecs": 2.0});
    let legacy = serde_json::json!({"schemaVersion": 1, "agents": [legacy_claude]}).to_string();
    let extracted = extract_legacy_local(legacy.as_bytes(), &supported_shipped_definitions())
        .expect("legacy extraction with explicit idleBurst");
    let local: serde_json::Value = serde_json::from_slice(&extracted).unwrap();
    assert_eq!(
        local["agents"][0]["idleBurst"],
        serde_json::json!({"maxSecs": 2.0})
    );
}

#[test]
pub(crate) fn install_commands_2736_shipped_for_eight_builtins_and_absent_for_muse() {
    let catalog = embedded_default_catalog();
    assert_eq!(catalog.agents.len(), 9);
    for def in &catalog.agents {
        assert_shipped_install_commands_2736(&def.key, def);
        assert_eq!(
            def.install_commands.as_ref().map(|ic| ic.default.as_str()),
            expected_install_2736(&def.key)
        );
    }
    assert_eq!(
        catalog
            .agents
            .iter()
            .filter(|def| def.install_commands.is_some())
            .count(),
        8
    );
}
