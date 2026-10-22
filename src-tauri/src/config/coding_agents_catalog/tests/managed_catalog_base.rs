use super::*;

#[test]
fn managed_catalog_fresh_init_seeds_verified_base_and_exact_stub() {
    let dir = seed_dir();
    assert!(ensure_seeded(dir.path(), None).is_some());
    let base = base_json(dir.path());
    let expected_revision = managed_content_sha256(&supported_shipped_definitions());
    assert_eq!(base["managed"]["owner"], "agentscommander");
    assert_eq!(base["managed"]["version"], 1);
    assert_eq!(base["managed"]["revision"], expected_revision);
    assert_eq!(base["managed"]["contentSha256"], expected_revision);
    assert_eq!(base["agents"].as_array().unwrap().len(), 8);
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        LOCAL_STUB_BYTES,
        "the local stub is exactly the documented bytes plus LF"
    );

    let base_bytes = std::fs::read(manifest_path(dir.path())).unwrap();
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.catalog.len(), 8);
    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "a verified base at the shipped revision is never rewritten"
    );
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        base_bytes
    );
    let residue: Vec<std::ffi::OsString> = std::fs::read_dir(catalog_dir(dir.path()))
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
        .map(|entry| entry.file_name())
        .collect();
    assert!(residue.is_empty(), "no publication temporaries survive");
}

#[test]
fn managed_catalog_refresh_reaches_unpinned_values_and_keeps_pinned_local() {
    let dir = seed_dir();
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["label"] = serde_json::json!("OLD LABEL");
    write_managed_base(dir.path(), &[claude], "stale-revision", true);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"claude","label":"MINE"}]}"##,
    );
    let local_before = std::fs::read(local_catalog_path(dir.path())).unwrap();

    assert!(
        ensure_seeded(dir.path(), None).is_some(),
        "a base at a stale revision refreshes"
    );
    let base = base_json(dir.path());
    assert_eq!(
        base["managed"]["revision"],
        managed_content_sha256(&supported_shipped_definitions())
    );
    assert_eq!(
        base["agents"].as_array().unwrap().len(),
        8,
        "refresh publishes the whole shipped set"
    );
    assert_eq!(
        std::fs::read(local_catalog_path(dir.path())).unwrap(),
        local_before,
        "refresh never touches the local layer or the composed view"
    );

    let loaded = load_catalog(dir.path()).unwrap();
    let claude = loaded.iter().find(|d| d.key == "claude").unwrap();
    assert_eq!(claude.label, "MINE", "the pinned local value wins");
    assert_eq!(
        claude.description, "Coding Agent by Anthropic",
        "an unpinned value reaches the refreshed shipped default"
    );
    let report = load_catalog_report(dir.path());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn managed_catalog_support_gate_change_refreshes_the_base_both_ways() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let full_bytes = std::fs::read(manifest_path(dir.path())).unwrap();
    assert_eq!(base_json(dir.path())["agents"].as_array().unwrap().len(), 8);

    with_builtin_agent_support_for_test(TABLE_ALL_ENABLED, || {
        assert!(
            ensure_seeded(dir.path(), None).is_some(),
            "a support-gate change is part of the managed revision"
        );
        let base = base_json(dir.path());
        assert_eq!(base["agents"].as_array().unwrap().len(), 9);
        assert_eq!(
            base["managed"]["revision"],
            managed_content_sha256(&supported_shipped_definitions())
        );
        assert_eq!(load_catalog(dir.path()).unwrap().len(), 9);
    });

    // Back under the shipped table the revision changes again and the base
    // byte-for-byte returns to its original state.
    assert!(ensure_seeded(dir.path(), None).is_some());
    assert_eq!(
        std::fs::read(manifest_path(dir.path())).unwrap(),
        full_bytes
    );
    assert_eq!(load_catalog(dir.path()).unwrap().len(), 8);
}

#[test]
fn managed_catalog_edited_base_is_readable_and_never_refreshed() {
    let dir = seed_dir();
    let mut claude = shipped_def_json(&["claude"]).remove(0);
    claude["label"] = serde_json::json!("HAND EDITED");
    let bytes = write_managed_base(dir.path(), &[claude], "stale-revision", false);

    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog[0].label, "HAND EDITED");
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.code == "managedBaseEdited"),
        "{:?}",
        report.warnings
    );
    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "an edited managed base is never auto-refreshed"
    );
    assert_eq!(std::fs::read(manifest_path(dir.path())).unwrap(), bytes);
}

#[test]
fn managed_catalog_unknown_managed_fields_block_refresh_without_deletion() {
    let dir = seed_dir();
    let agents = shipped_def_json(&["claude"]);
    let definitions: Vec<CodingAgentDefinition> = agents
        .iter()
        .cloned()
        .map(|value| serde_json::from_value(value).unwrap())
        .collect();
    let content = managed_content_sha256(&definitions);
    let root = serde_json::json!({
        "schemaVersion": 1,
        "agents": definitions,
        "futureRootField": {"keep": "me"},
        "managed": {
            "owner": "agentscommander",
            "version": 1,
            "revision": "stale-revision",
            "contentSha256": content,
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
    bytes.push(b'\n');
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();

    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog.len(), 1);
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationPending"));
    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "unknown data blocks refresh rather than being deleted"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn managed_catalog_foreign_managed_marker_blocks_ownership() {
    let dir = seed_dir();
    let root = serde_json::json!({
        "schemaVersion": 1,
        "agents": shipped_def_json(&["claude"]),
        "managed": {
            "owner": "somebody-else",
            "version": 1,
            "revision": "x",
            "contentSha256": "y",
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&root).unwrap();
    bytes.push(b'\n');
    let path = manifest_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &bytes).unwrap();

    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog.len(), 1, "a foreign base stays readable");
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "migrationConflict"));
    assert!(
        ensure_seeded(dir.path(), None).is_none(),
        "a foreign marker neither refreshes nor migrates"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn managed_catalog_existing_local_entries_are_never_clobbered() {
    test09_directory_local_preserved();
    test09_regular_local_and_read_denied_preserved();
}

#[test]
fn managed_catalog_local_composes_every_field_and_explicit_false_empty() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[
            {"key":"claude","label":"My Claude","description":"D","color":"#010203","command":"claude","instructionsFilename":null,"envs":[{"key":"ZZ","value":"1"},{"key":"AA","value":"2","source":"system","enabled":false}],"isolatedHome":true,"configSeed":{"enabled":false},"removable":false,"updateCommands":[],"autoUpdate":true}
        ]}"##,
    );
    let loaded = load_catalog(dir.path()).unwrap();
    let claude = loaded.iter().find(|d| d.key == "claude").unwrap();
    assert_eq!(claude.label, "My Claude");
    assert_eq!(claude.description, "D");
    assert_eq!(claude.color, "#010203");
    assert_eq!(claude.instructions_filename, None, "explicit null clears");
    assert_eq!(claude.envs.len(), 2, "envs replace whole arrays in order");
    assert_eq!(claude.envs[0].key, "ZZ");
    assert_eq!(claude.envs[1].key, "AA");
    assert!(!claude.envs[1].enabled, "explicit false stays explicit");
    assert_eq!(claude.envs[1].source, CodingAgentEnvSource::System);
    assert!(claude.isolated_home);
    let seed = claude.config_seed.as_ref().expect("merged configSeed");
    assert!(!seed.enabled);
    assert_eq!(seed.dest, ".claude", "nested configSeed merges by presence");
    assert!(!claude.removable);
    assert!(claude.update_commands.is_empty(), "explicit [] stays empty");
    assert!(claude.auto_update);
    assert_eq!(loaded.len(), 8, "no other row changed");
}

#[test]
fn managed_catalog_local_new_row_order_and_tombstone() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[
            {"key":"zeta","label":"Zeta","description":"d","color":"#111","command":"zeta","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false},
            {"key":"grok","remove":true}
        ],"order":["zeta","claude"]}"##,
    );
    let loaded = load_catalog(dir.path()).unwrap();
    let keys = keys_of(&loaded);
    assert_eq!(keys[0], "zeta", "listed survivors come first in order");
    assert_eq!(keys[1], "claude");
    assert!(!keys.contains(&"grok"), "the tombstone removes the key");
    assert_eq!(keys.len(), 8, "8 shipped - 1 removed + 1 added");
}

#[test]
fn managed_catalog_local_invalid_layer_falls_back_as_a_whole() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    let bad = r##"{"schemaVersion":1,"agents":[
        {"key":"claude","label":"Would Apply"},
        {"key":"broken","label":"Broken"}
    ]}"##;
    write_local(dir.path(), bad);
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert_eq!(report.catalog.len(), 8);
    assert_eq!(
        report.catalog[0].label, "Claude Code",
        "the valid row must not be partially applied"
    );
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "localInvalid"));
    assert_eq!(
        read_text(&local_catalog_path(dir.path())),
        bad,
        "the offending file is never rewritten"
    );
}

#[test]
fn managed_catalog_local_schema_rejections_are_whole_layer() {
    let cases = [
        (
            "duplicate member",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","key":"pi"}]}"##,
        ),
        (
            "unknown field",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","mystery":1}]}"##,
        ),
        (
            "unsupported schema",
            r##"{"schemaVersion":999,"agents":[]}"##,
        ),
        (
            "remove false",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","remove":false}]}"##,
        ),
        (
            "tombstone extra field",
            r##"{"schemaVersion":1,"agents":[{"key":"claude","remove":true,"label":"x"}]}"##,
        ),
        (
            "duplicate local key",
            r##"{"schemaVersion":1,"agents":[{"key":"claude"},{"key":"claude"}]}"##,
        ),
        (
            "duplicate order key",
            r##"{"schemaVersion":1,"agents":[],"order":["claude","claude"]}"##,
        ),
        (
            "control character in update command",
            "{\"schemaVersion\":1,\"agents\":[{\"key\":\"claude\",\"updateCommands\":[\"a\\u0007b\"]}]}",
        ),
    ];
    for (label, contents) in cases {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        write_local(dir.path(), contents);
        let report = load_catalog_report(dir.path());
        assert!(report.unavailable.is_none(), "{label}");
        assert_eq!(report.catalog.len(), 8, "{label}: base rows intact");
        assert_eq!(report.catalog[0].label, "Claude Code", "{label}");
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.code == "localInvalid"),
            "{label}: {:?}",
            report.warnings
        );
        assert_eq!(
            read_text(&local_catalog_path(dir.path())),
            contents,
            "{label}"
        );
    }
}

#[test]
fn managed_catalog_local_removability_is_evaluated_against_the_base() {
    let dir = seed_dir();
    let mut grok = shipped_def_json(&["grok"]).remove(0);
    grok["removable"] = serde_json::json!(false);
    write_managed_base(dir.path(), &[grok], "stale", true);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"grok","remove":true}]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == "localInvalid"));
    assert!(report.catalog.iter().any(|d| d.key == "grok"));
}

#[test]
fn managed_catalog_local_support_gate_attempts_stay_suppressed() {
    with_builtin_agent_support_for_test(TABLE_MUSE_OFF, || {
        let dir = seed_dir();
        ensure_seeded(dir.path(), None);
        write_local(
            dir.path(),
            r##"{"schemaVersion":1,"agents":[{"key":"muse","label":"Muse","description":"d","color":"#0668E1","command":"muse","envs":[],"isolatedHome":false,"removable":true,"updateCommands":[],"autoUpdate":false}]}"##,
        );
        let report = load_catalog_report(dir.path());
        assert!(!report.catalog.iter().any(|d| d.key == "muse"));
        assert!(report.warnings.iter().any(|warning| {
            warning.code == "invalidDefinition" && warning.reason.contains("not supported")
        }));
    });
}

#[test]
fn managed_catalog_local_unknown_tombstone_and_order_keys_are_ignored() {
    let dir = seed_dir();
    ensure_seeded(dir.path(), None);
    write_local(
        dir.path(),
        r##"{"schemaVersion":1,"agents":[{"key":"future-key","remove":true}],"order":["ghost","claude"]}"##,
    );
    let report = load_catalog_report(dir.path());
    assert!(report.unavailable.is_none());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.catalog.len(), 8);
    assert_eq!(
        report.catalog[0].key, "claude",
        "unknown order keys are ignored"
    );
    assert!(
        read_text(&local_catalog_path(dir.path())).contains("future-key"),
        "an unknown tombstone is retained for a future shipped key"
    );
}
