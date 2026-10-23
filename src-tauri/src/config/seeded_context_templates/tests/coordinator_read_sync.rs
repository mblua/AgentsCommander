use super::*;

#[test]
fn read_sync_updates_old_generated_coordinator_template() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
        OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND,
    )
    .expect("write old coordinator");

    let published_at = fixed_publication_time();
    let publications = sync_for_read_at(
        &ac_root,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        published_at,
    );
    assert_one_publication(
        &publications,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        published_at,
    );

    let content = std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
        .expect("read coordinator");
    assert_eq!(content, get_default_coordinator_template());
}

/// #1030 failing-first migration proof for the STATELESS population (E4 row 4):
/// the assert_ne is the only check that fails if the v4 rule edit is skipped
/// while the freeze lands, and the assert! is the only one that fails if the
/// const is never wired into `is_known_generated_coordinator_template`. The
/// sync half proves a pristine v3 body with no state file auto-upgrades.
#[test]
fn read_sync_updates_pristine_v3_coordinator_template() {
    assert_ne!(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE,
        get_default_coordinator_template(),
        "the v4 edit must actually change the template or the freeze is pointless"
    );
    assert!(
        is_known_generated_coordinator_template(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE
        ),
        "the recognizer must accept the frozen v3 bytes"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE,
    )
    .expect("write pristine v3 coordinator");

    let published_at = fixed_publication_time();
    let publications = sync_for_read_at(
        &ac_root,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        published_at,
    );
    assert_one_publication(
        &publications,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        published_at,
    );

    let content = std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
        .expect("read coordinator");
    assert_eq!(content, get_default_coordinator_template());
}

/// #1030: the SEEDED population (E4 row 3), which is the branch every existing
/// workspace with a trusted state entry is in, so this is the test that proves
/// the migration actually reaches them. A pristine v3 body whose
/// lastSeededSha256 equals the file hash auto-updates and persists the bump.
#[test]
fn read_sync_updates_seeded_v3_coordinator_and_bumps_version() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE,
    )
    .expect("write pristine v3 coordinator");
    std::fs::write(
        ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME),
        format!(
            concat!(
                r#"{{"schemaVersion":1,"templates":{{"coordinator":{{"#,
                r#""templateId":"coordinator","currentVersion":3,"#,
                r#""lastSeededSha256":"{}","lastObservedSha256":null,"#,
                r#""ignoredDefaultSha256":null,"ignoredObservedSha256":null}}}}}}"#
            ),
            hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE)
        ),
    )
    .expect("write seeded v3 state");

    // Pin the E4 row 3 preconditions before syncing. Every assertion after the sync
    // call also passes on row 4 (:831-836), which auto-updates to the same v4 body
    // when there is no entry at all, and `load_state` (:552-565) turns an unparseable
    // fixture into a trusted empty map rather than an error. So without this block a
    // fixture that never parses still leaves the test green while exercising row 4.
    let spec = project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
        .expect("coordinator spec by filename");
    let pre_sync = load_state(&ac_root, true).expect("load the hand-written v3 state");
    let entry = pre_sync
        .trusted_entry(spec)
        .expect("the v3 fixture must parse into a trusted coordinator entry (row 3)");
    let v3_sha256 = hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE);
    let v4_sha256 = hash_text(get_default_coordinator_template());
    assert_eq!(
        entry.current_version,
        Some(3),
        "row 3 requires the fixture to describe a seeded v3 coordinator"
    );
    assert_eq!(
        entry.last_seeded_sha256.as_deref(),
        Some(v3_sha256.as_str()),
        "row 3 requires lastSeededSha256 to match the pristine v3 body on disk"
    );
    assert_ne!(
        v3_sha256, v4_sha256,
        "row 3 requires the seeded hash to differ from the current v4 default"
    );
    assert!(
        is_known_generated_coordinator_template(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE
        ),
        "row 3 requires the pristine v3 body to be recognized as generated"
    );

    let published_at = fixed_publication_time();
    let publications = sync_for_read_at(
        &ac_root,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        published_at,
    );
    assert_one_publication(
        &publications,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        published_at,
    );

    let content = std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
        .expect("read coordinator");
    assert_eq!(
        content,
        get_default_coordinator_template(),
        "a seeded pristine v3 body must auto-upgrade to the v4 default"
    );

    let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
        .expect("read seeded state");
    let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
    assert_eq!(
        parsed["templates"]["coordinator"]["currentVersion"], 6,
        "coordinator current_version must be bumped to 6 by the #1614 room rename"
    );
    assert_eq!(
        parsed["templates"]["coordinator"]["lastSeededSha256"]
            .as_str()
            .expect("lastSeededSha256 is a string"),
        hash_text(get_default_coordinator_template()),
        "the seeded hash must record the new v4 default"
    );
}

#[test]
fn custom_coordinator_is_preserved_and_reported() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let custom = "custom coordinator guidance";
    std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
        .expect("write custom coordinator");

    let mut clock = || fixed_publication_time();
    let mut publications = Vec::new();
    let updates = scan_project_context_template_updates_with_clock(
        temp.path(),
        &ac_root,
        &mut clock,
        &mut |filename, publication| publications.push((filename, publication)),
    )
    .expect("scan updates");

    assert_eq!(updates.len(), 1);
    assert!(
        publications.is_empty(),
        "observing custom content must not produce publication evidence"
    );
    assert_eq!(updates[0].filename, COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    assert_eq!(
        std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read coordinator"),
        custom
    );
}

#[test]
fn global_unknown_without_state_is_not_reported() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        "legacy rendered global body with project paths",
    )
    .expect("write global");

    let updates =
        scan_project_context_template_updates(temp.path(), &ac_root).expect("scan updates");

    assert!(updates.is_empty());
}

#[test]
fn forged_manifest_does_not_auto_overwrite_custom_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let custom = "custom coordinator with forged seeded hash";
    std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
        .expect("write custom coordinator");
    let mut state = SeededContextTemplateState::default();
    state.templates.insert(
        "coordinator".to_string(),
        SeededContextTemplateEntry {
            template_id: "coordinator".to_string(),
            current_version: Some(1),
            last_seeded_sha256: Some(hash_text(custom)),
            last_observed_sha256: None,
            ignored_default_sha256: None,
            ignored_observed_sha256: None,
        },
    );
    persist_state(&ac_root, &state).expect("persist forged state");

    let updates =
        scan_project_context_template_updates(temp.path(), &ac_root).expect("scan updates");

    assert_eq!(updates.len(), 1);
    assert_eq!(
        std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read coordinator"),
        custom
    );
}

#[test]
fn dismiss_suppresses_same_file_and_default_hash() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let custom = "custom coordinator guidance";
    std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
        .expect("write custom coordinator");
    let update = scan_project_context_template_updates(temp.path(), &ac_root)
        .expect("scan updates")
        .pop()
        .expect("pending update");

    dismiss_context_template_update(
        &ac_root,
        &update.filename,
        &update.current_file_sha256,
        &update.current_default_sha256,
    )
    .expect("dismiss");

    let mut clock = || fixed_publication_time();
    let mut publications = Vec::new();
    let updates = scan_project_context_template_updates_with_clock(
        temp.path(),
        &ac_root,
        &mut clock,
        &mut |filename, publication| publications.push((filename, publication)),
    )
    .expect("scan again");
    assert!(updates.is_empty());
    assert!(
        publications.is_empty(),
        "a dismissed observation must not manufacture publication evidence"
    );
}

/// #1748 retargeted this pair at the coordinator: `global` is distribution-owned
/// and has no ignore, dismiss or pending-update lifecycle any more (new test 5
/// pins the rejection). The coordinator still has one, so the property survives.
#[test]
fn ignored_v3_pair_becomes_pending_against_v4() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let custom = "# Custom coordinator\n\nKEEP THIS CONTENT\n";
    let custom_hash = hash_text(custom);
    let v3_hash = hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE);
    let v4_hash = hash_text(get_default_coordinator_template());
    std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
        .expect("write custom coordinator");
    let mut state = SeededContextTemplateState::default();
    state.templates.insert(
        "coordinator".to_string(),
        SeededContextTemplateEntry {
            template_id: "coordinator".to_string(),
            current_version: Some(3),
            last_seeded_sha256: Some(v3_hash.clone()),
            last_observed_sha256: Some(custom_hash.clone()),
            ignored_default_sha256: Some(v3_hash.clone()),
            ignored_observed_sha256: Some(custom_hash.clone()),
        },
    );
    persist_state(&ac_root, &state).expect("persist ignored v3 pair");

    let mut clock = || fixed_publication_time();
    let mut publications = Vec::new();
    let updates = scan_project_context_template_updates_with_clock(
        temp.path(),
        &ac_root,
        &mut clock,
        &mut |filename, publication| publications.push((filename, publication)),
    )
    .expect("scan stale ignored pair");
    assert!(publications.is_empty());
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].current_file_sha256, custom_hash);
    assert_eq!(updates[0].current_default_sha256, v4_hash);
    assert_eq!(updates[0].current_default_version, 6);
    assert_eq!(
        std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read preserved custom coordinator"),
        custom
    );
    let after_observation = load_state(&ac_root, true).expect("load observed state");
    let entry = after_observation
        .trusted_entry(project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).unwrap())
        .expect("trusted global entry");
    assert_eq!(
        entry.ignored_observed_sha256.as_deref(),
        Some(custom_hash.as_str())
    );
    assert_eq!(
        entry.ignored_default_sha256.as_deref(),
        Some(v3_hash.as_str())
    );

    dismiss_context_template_update(
        &ac_root,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        &updates[0].current_file_sha256,
        &updates[0].current_default_sha256,
    )
    .expect("dismiss v4 update");
    let second = scan_project_context_template_updates(temp.path(), &ac_root)
        .expect("scan re-dismissed v4 pair");
    assert!(second.is_empty());
    assert_eq!(
        std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("re-read preserved custom coordinator"),
        custom
    );

    let spec = project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).unwrap();
    let mut loaded = load_state(&ac_root, true).expect("load re-dismissed state");
    let mut clock = || fixed_publication_time();
    let outcome = sync_one_template(
        Some(temp.path()),
        &ac_root,
        spec,
        &mut loaded,
        false,
        true,
        false,
        &mut clock,
    )
    .completion
    .expect("classify re-dismissed pair");
    assert!(matches!(
        outcome.target_outcome,
        TemplatePublication::Skipped(ContextTemplateSkipReason::IgnoredByUser)
    ));
}

#[test]
fn ignored_current_v6_pair_remains_suppressed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let custom = "# Current ignored custom coordinator\n";
    let custom_hash = hash_text(custom);
    let v4_hash = hash_text(get_default_coordinator_template());
    std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
        .expect("write custom coordinator");
    let mut state = SeededContextTemplateState::default();
    state.templates.insert(
        "coordinator".to_string(),
        SeededContextTemplateEntry {
            template_id: "coordinator".to_string(),
            current_version: Some(6),
            last_seeded_sha256: Some(v4_hash.clone()),
            last_observed_sha256: Some(custom_hash.clone()),
            ignored_default_sha256: Some(v4_hash),
            ignored_observed_sha256: Some(custom_hash),
        },
    );
    persist_state(&ac_root, &state).expect("persist ignored v4 pair");

    let mut clock = || fixed_publication_time();
    let mut publications = Vec::new();
    let updates = scan_project_context_template_updates_with_clock(
        temp.path(),
        &ac_root,
        &mut clock,
        &mut |filename, publication| publications.push((filename, publication)),
    )
    .expect("scan ignored v4 pair");
    assert!(updates.is_empty());
    assert!(publications.is_empty());
    assert_eq!(
        std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read ignored custom coordinator"),
        custom
    );

    let spec = project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).unwrap();
    let mut loaded = load_state(&ac_root, true).expect("load ignored v4 state");
    let outcome = sync_one_template(
        Some(temp.path()),
        &ac_root,
        spec,
        &mut loaded,
        false,
        true,
        false,
        &mut clock,
    )
    .completion
    .expect("classify ignored v4 pair");
    assert!(matches!(
        outcome.target_outcome,
        TemplatePublication::Skipped(ContextTemplateSkipReason::IgnoredByUser)
    ));
}

#[test]
fn explicit_keep_repairs_invalid_state_json() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let custom = "custom coordinator guidance";
    std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
        .expect("write custom coordinator");
    std::fs::write(
        ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME),
        "not json",
    )
    .expect("write invalid state");

    dismiss_context_template_update(
        &ac_root,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        &hash_text(custom),
        &hash_text(get_default_coordinator_template()),
    )
    .expect("dismiss with invalid state");

    let repaired = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
        .expect("read repaired state");
    let parsed: SeededContextTemplateState =
        serde_json::from_str(&repaired).expect("state is repaired JSON");
    assert_eq!(parsed.schema_version, STATE_SCHEMA_VERSION);
}

#[test]
fn overwrite_creates_backup_and_writes_default() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let custom = "custom coordinator guidance";
    std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
        .expect("write custom coordinator");
    let update = scan_project_context_template_updates(temp.path(), &ac_root)
        .expect("scan updates")
        .pop()
        .expect("pending update");

    let result = overwrite_context_template_with_default(
        &ac_root,
        &update.filename,
        &update.current_file_sha256,
        &update.current_default_sha256,
    )
    .expect("overwrite");

    assert_eq!(
        std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read coordinator"),
        get_default_coordinator_template()
    );
    assert_eq!(
        std::fs::read_to_string(result.backup_path).expect("read backup"),
        custom
    );
}

#[test]
fn overwrite_publication_survives_later_strict_state_save_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let path = ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    let custom = "custom coordinator guidance";
    std::fs::write(&path, custom).expect("write custom coordinator");
    let update = scan_project_context_template_updates(temp.path(), &ac_root)
        .expect("scan updates")
        .pop()
        .expect("pending update");
    let published_at = fixed_publication_time();
    let mut clock = || published_at;
    let handled_publication = std::cell::Cell::new(None);

    let execution = overwrite_context_template_with_default_with(
        &ac_root,
        &update.filename,
        &update.current_file_sha256,
        &update.current_default_sha256,
        &mut clock,
        &mut |filename, publication| {
            assert_eq!(filename, COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
            handled_publication.set(Some(publication));
        },
        |_, _| {
            assert_eq!(
                std::fs::read_to_string(&path).expect("read target before state save"),
                get_default_coordinator_template(),
                "the physical replacement must precede specialized state persistence"
            );
            assert_eq!(
                handled_publication.get(),
                Some(ContextPublication { published_at }),
                "the publication must be consumed before specialized state persistence"
            );
            Err("injected strict state persistence failure".to_string())
        },
    );

    assert_eq!(
        execution.published,
        Some(ContextPublication { published_at })
    );
    assert_eq!(
        execution
            .completion
            .expect_err("strict state save failure remains an outer error"),
        "injected strict state persistence failure"
    );
    assert_eq!(
        std::fs::read_to_string(path).expect("read surviving published target"),
        get_default_coordinator_template()
    );
}

#[test]
fn future_schema_is_not_rewritten_by_scan() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let state_path = ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
    std::fs::write(&state_path, r#"{"schemaVersion":999,"templates":{}}"#)
        .expect("write future state");

    let updates =
        scan_project_context_template_updates(temp.path(), &ac_root).expect("scan updates");

    assert!(updates.is_empty());
    assert_eq!(
        std::fs::read_to_string(state_path).expect("read state"),
        r#"{"schemaVersion":999,"templates":{}}"#
    );
}

#[test]
fn state_directory_blocks_explicit_commands() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    std::fs::create_dir(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
        .expect("create state dir");

    let err = dismiss_context_template_update(
        &ac_root,
        COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        "file",
        "default",
    )
    .expect_err("state dir must error");

    assert!(err.contains("state path"));
}

#[test]
fn dedupe_keeps_distinct_file_hashes() {
    let mut updates = vec![
        ContextTemplateUpdate {
            project_path: "project".to_string(),
            workspace_path: "workspace".to_string(),
            file_path: "workspace/Context.coordinator.md".to_string(),
            filename: COORDINATOR_CONTEXT_TEMPLATE_FILENAME.to_string(),
            label: "Orchestrator context".to_string(),
            current_file_sha256: "file-a".to_string(),
            current_default_sha256: "default".to_string(),
            current_default_version: 2,
        },
        ContextTemplateUpdate {
            project_path: "project".to_string(),
            workspace_path: "workspace".to_string(),
            file_path: "workspace/Context.coordinator.md".to_string(),
            filename: COORDINATOR_CONTEXT_TEMPLATE_FILENAME.to_string(),
            label: "Orchestrator context".to_string(),
            current_file_sha256: "file-a".to_string(),
            current_default_sha256: "default".to_string(),
            current_default_version: 2,
        },
        ContextTemplateUpdate {
            project_path: "project".to_string(),
            workspace_path: "workspace".to_string(),
            file_path: "workspace/Context.coordinator.md".to_string(),
            filename: COORDINATOR_CONTEXT_TEMPLATE_FILENAME.to_string(),
            label: "Orchestrator context".to_string(),
            current_file_sha256: "file-b".to_string(),
            current_default_sha256: "default".to_string(),
            current_default_version: 2,
        },
    ];

    dedupe_context_template_updates(&mut updates);

    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].current_file_sha256, "file-a");
    assert_eq!(updates[1].current_file_sha256, "file-b");
}

#[test]
fn root_custom_template_is_preserved() {
    let temp = tempfile::tempdir().expect("tempdir");
    let custom = "custom root context";
    std::fs::write(
        temp.path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME),
        custom,
    )
    .expect("write root custom");

    ensure_root_context_template(temp.path()).expect("ensure root context");

    assert_eq!(
        std::fs::read_to_string(
            temp.path()
                .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME)
        )
        .expect("read root context"),
        custom
    );
}
