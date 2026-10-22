use super::super::*;

fn env(key: &str, value: &str, enabled: bool) -> CodingAgentEnv {
    CodingAgentEnv {
        key: key.into(),
        value: value.into(),
        source: CodingAgentEnvSource::User,
        enabled,
    }
}

fn entry<'a>(key: &'a str, name: &'a str, command: &'a str) -> IdentitySnapshotEntry<'a> {
    IdentitySnapshotEntry {
        key,
        name,
        command,
        envs: &[],
        profiles: None,
    }
}

fn source_ref() -> SourceRef {
    SourceRef {
        family: SourceFamily::Agents,
        role: SourceRole::Catalog,
        scope: SourceScope::Project,
        kind: SourceKind::CatalogPersonal,
        context_root: "D:/fixture/project".into(),
    }
}

fn configuration_ref() -> ConfigurationRef {
    ConfigurationRef {
        version: 1,
        source: source_ref(),
        calculated_id: configuration_identity("tool", &[]),
        requested_profile: Some("B".into()),
        profile_calculated_id: Some(profile_identity(&empty_profile_cell())),
        legacy_entry_hint: Some("local-key".into()),
    }
}

#[test]
fn identity_unicode_scalar_hash_literal_and_disabled_env() {
    // Independent SHA256 fixture of the explicitly sorted UTF8 string
    // "--abioozßé\u{0307}", obtained with Node crypto, not this normalizer.
    assert_eq!(
        configuration_identity("Ba--", &[env(" Z\t", "İß É\u{00a0}OO", false)]),
        "cid1:7eccb85b0439780c9cb0fcd45fdfdf0e6ee28906a805ea0a1a1bfcd34d875199"
    );
    let mut agent: AgentConfig = serde_json::from_value(serde_json::json!({
        "id":"unicode", "label":"ignored", "color":"red", "baseCommand":"Ba--",
        "envs":[{"key":" Z\t","value":"İß É\u{00a0}OO","enabled":false}]
    }))
    .unwrap();
    refresh_agent_id_hashes(std::slice::from_mut(&mut agent));
    assert_eq!(
        agent.id_hash.as_deref(),
        Some("cid1:7eccb85b0439780c9cb0fcd45fdfdf0e6ee28906a805ea0a1a1bfcd34d875199")
    );
    assert_eq!(
        agent.id_hash_debug,
        Some(vec!["Ba--".into(), " Z\t".into(), "İß É\u{00a0}OO".into()])
    );
}

#[test]
fn identity_repeated_dash_and_o_are_not_sets() {
    assert_eq!(
        configuration_identity("oo -- O", &[]),
        "cid1:955d19cd39ddcfc4f57dc7d099bb56f703f0dbe00aa64806a6d398993eba4b7c"
    );
    assert_eq!(
        configuration_identity("oo --", &[]),
        "cid1:b82e31c7f845633e569797ff2ca05d61c74105b2b7580c0e3012ce4d7de4155f"
    );
    assert_ne!(
        configuration_identity("--oo", &[]),
        configuration_identity("-oo", &[])
    );
}

#[test]
fn identity_anagrams_env_order_and_disabled_metadata() {
    let rows = vec![env("X", "BA", false), env("Y", "CA", true)];
    let mut reversed = rows.clone();
    reversed.reverse();
    reversed[0].enabled = false;
    reversed[0].source = CodingAgentEnvSource::System;
    assert_eq!(
        configuration_identity("tool", &rows),
        configuration_identity("loot", &reversed)
    );
    assert_ne!(
        configuration_identity("tool", &rows),
        configuration_identity("tool", &rows[1..])
    );
    assert_eq!(
        configuration_identity("abc", &[]),
        configuration_identity("a", &[env("b", "c", false)])
    );
}

#[test]
fn identity_unicode_whitespace_without_nfc_or_zero_width_removal() {
    assert_eq!(
        configuration_identity("T\u{0085}\u{00a0}\u{2003}\u{2028}\u{3000} O\nOL", &[]),
        configuration_identity("tool", &[])
    );
    assert_ne!(
        configuration_identity("é", &[]),
        configuration_identity("e\u{0301}", &[])
    );
    assert_ne!(
        configuration_identity("tool\u{200b}", &[]),
        configuration_identity("tool", &[])
    );
    assert_ne!(
        configuration_identity("tool\u{feff}", &[]),
        configuration_identity("tool", &[])
    );
}

#[test]
fn identity_profile_empty_present_and_agent_local() {
    let empty = empty_profile_cell();
    assert_eq!(
        profile_identity(&empty),
        "pid1:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    let profiles = BTreeMap::from([("B".into(), empty.clone())]);
    let mut entries = [
        entry("first", "First", "one"),
        entry("second", "Second", "two"),
    ];
    entries[0].profiles = Some(&profiles);
    entries[1].profiles = Some(&profiles);
    assert!(
        validate_identity_snapshot(&entries).is_ok(),
        "same pid is allowed in different agents"
    );
    let mut changed = empty;
    changed.enabled = false;
    changed.notes = "decorative".into();
    changed.preflight = Some("not identity input".into());
    assert_eq!(profile_identity(&changed), profile_identity(&profiles["B"]));
    assert!(validate_identity_snapshot(&[entry("absent", "Absent", "one")]).is_ok());
}

#[test]
fn identity_profile_parameters_and_env_change_pid_not_cid() {
    let mut profile = empty_profile_cell();
    profile.command = "--MODEL A".into();
    profile.env.insert("API".into(), "value".into());
    let original_pid = profile_identity(&profile);
    let cid = configuration_identity("tool", &[env("BASE", "value", true)]);
    profile.command.push('o');
    assert_ne!(original_pid, profile_identity(&profile));
    assert_eq!(
        cid,
        configuration_identity("tool", &[env("BASE", "value", true)])
    );
    profile.command.pop();
    profile.env.insert("API".into(), "different".into());
    assert_ne!(original_pid, profile_identity(&profile));
}

#[test]
fn identity_duplicate_reports_incumbent_name_crossfile_and_update() {
    let first = entry("first", "Existing Coding Agent", "tool");
    let replacement = entry("second", "Different name", "loot");
    assert!(validate_identity_snapshot(&[entry("first", "Existing Coding Agent", "tool")]).is_ok());
    assert!(validate_identity_snapshot(&[entry("second", "Different name", "loot")]).is_ok());
    let error = validate_identity_snapshot(&[first, replacement]).unwrap_err();
    assert_eq!(
        error.code,
        IdentityValidationCode::DuplicateConfigurationIdentity
    );
    assert_eq!(error.existing_name, "Existing Coding Agent");
    assert_eq!(error.entry_hint, "second");
    // An update replaces only its own row; it cannot exclude another one.
    assert!(validate_identity_snapshot(&[
        entry("first", "Renamed", "tool"),
        entry("second", "Second", "other")
    ])
    .is_ok());
}

#[test]
fn identity_disabled_profile_duplicates_are_validated_locally() {
    let mut disabled = empty_profile_cell();
    disabled.enabled = false;
    let profiles = BTreeMap::from([("A".into(), empty_profile_cell()), ("B".into(), disabled)]);
    let mut candidate = entry("first", "First", "tool");
    candidate.profiles = Some(&profiles);
    let error = validate_identity_snapshot(&[candidate]).unwrap_err();
    assert_eq!(error.code, IdentityValidationCode::DuplicateProfileIdentity);
    assert_eq!(error.requested_profile.as_deref(), Some("B"));
}

#[test]
fn identity_injected_hash_collisions_do_not_merge_preimages() {
    let entries = [
        entry("first", "First", "abc"),
        entry("second", "Second", "xyz"),
    ];
    let error =
        validate_identity_snapshot_with_digest(&entries, |_, _| "forced".into()).unwrap_err();
    assert_eq!(error.code, IdentityValidationCode::IdentityHashCollision);
    let first_profiles = BTreeMap::from([("A".into(), empty_profile_cell())]);
    let mut second = empty_profile_cell();
    second.command = "different".into();
    let second_profiles = BTreeMap::from([("Z".into(), second)]);
    let mut entries = entries;
    entries[0].profiles = Some(&first_profiles);
    entries[1].profiles = Some(&second_profiles);
    let error = validate_identity_snapshot_with_digest(&entries, |prefix, preimage| {
        if prefix == "pid1:" {
            "forced".into()
        } else {
            identity_digest(prefix, preimage)
        }
    })
    .unwrap_err();
    assert_eq!(error.code, IdentityValidationCode::IdentityHashCollision);
    assert_eq!(error.requested_profile.as_deref(), Some("Z"));
}

#[test]
fn identity_name_suffix_first_free_unicode_comparison() {
    let occupied = vec!["Name".into(), "Name (1)".into(), "Name (3)".into()];
    assert_eq!(identity_persisted_name(" name ", &occupied), "name (2)");
    assert_eq!(identity_persisted_name("Other", &occupied), "Other");
    assert_eq!(
        identity_persisted_name("i\u{0307}", &["İ".into()]),
        "i\u{0307} (1)"
    );
    assert_eq!(occupied[0], "Name", "incumbent stays unchanged");
}

#[test]
fn identity_revision_absent_is_not_empty_bytes_and_rejects_mixed_union() {
    let absent = serde_json::to_value(SourceRevision::Absent {}).unwrap();
    let bytes = serde_json::to_value(SourceRevision::Bytes {
        sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
    })
    .unwrap();
    assert_eq!(absent, serde_json::json!({"kind":"absent"}));
    assert_ne!(absent, bytes);
    assert!(serde_json::from_value::<SourceRevision>(
        serde_json::json!({"kind":"absent","sha256":""})
    )
    .is_err());
    for digest in [
        "",
        "abc",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        assert!(serde_json::from_value::<SourceRevision>(
            serde_json::json!({"kind":"bytes","sha256":digest})
        )
        .is_err());
    }
}

#[test]
fn identity_serde_companion_carriers_ticket_and_snapshot() {
    let pair = serde_json::json!({"codingAgentId":"legacy", "requestedProfile":"B", "selectionLocked":true});
    let physical = serde_json::json!({"tracked":{"kind":"absent"},"state":{"kind":"bytes","sha256":"full-state-digest"}});
    let state = SourceSelectionState {
        legacy_pair: pair.clone(),
        configuration_ref: Some(configuration_ref()),
        physical_revision: physical.clone(),
    };
    let value = serde_json::to_value(&state).unwrap();
    assert_eq!(value["legacyPair"], pair);
    assert_eq!(value["physicalRevision"], physical);
    let decoded: SourceSelectionState<Value, Value> =
        serde_json::from_value(value.clone()).unwrap();
    assert_eq!(decoded, state);
    let expectation: SourceSelectionExpectation<Value, Value> =
        serde_json::from_value(value).unwrap();
    assert_eq!(expectation.configuration_ref, state.configuration_ref);
    let default = SourceSelectionDefault {
        legacy_pair: pair,
        configuration_ref: configuration_ref(),
        physical_revision: physical,
    };
    assert_eq!(
        serde_json::from_value::<SourceSelectionDefault<Value, Value>>(
            serde_json::to_value(&default).unwrap()
        )
        .unwrap(),
        default
    );
    let ticket = SourceSelectionTicket {
        configuration_ref: configuration_ref(),
        expected_source_revision: SourceRevision::Absent {},
        catalog_context_revision: "context-generation".into(),
    };
    assert_eq!(
        serde_json::from_value::<SourceSelectionTicket>(serde_json::to_value(&ticket).unwrap())
            .unwrap(),
        ticket
    );
    let snapshot = SourceSnapshot::<AgentConfig> {
        source: source_ref(),
        revision: SourceRevision::Absent {},
        availability: SourceAvailability::Absent,
        capabilities: SourceCapabilities {
            editable_create: true,
            ..Default::default()
        },
        agents: Vec::new(),
        profiles: BTreeMap::new(),
        diagnostics: Vec::new(),
        context_revision: "context-generation".into(),
    };
    let decoded: SourceSnapshot<AgentConfig> =
        serde_json::from_value(serde_json::to_value(snapshot).unwrap()).unwrap();
    assert!(!decoded.capabilities.available_read);
    assert!(decoded.capabilities.editable_create);
    let mut malicious = serde_json::to_value(source_ref()).unwrap();
    malicious["rawPath"] = Value::String("unauthorized".into());
    assert!(serde_json::from_value::<SourceRef>(malicious).is_err());
}
