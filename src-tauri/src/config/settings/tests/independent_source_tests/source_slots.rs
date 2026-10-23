use super::*;

#[test]
fn e2432_source_personal_absent_has_no_creation_authority() {
    let fixture = Fixture::new();
    let source = fixture.context.source(SourceKind::CatalogPersonal);
    let snapshot = read_source_snapshot(&fixture.context, &source).unwrap();
    assert_eq!(snapshot.snapshot.availability, SourceAvailability::Absent);
    assert_eq!(snapshot.snapshot.revision, SourceRevision::Absent {});
    assert!(!snapshot.snapshot.capabilities.editable_create);
    assert!(snapshot.candidate_template.is_none());
    assert!(matches!(
        prepare_source_candidate(
            &snapshot,
            &SourceRevision::Absent {},
            &json!({"schemaVersion":2,"agents":[]}),
            None
        ),
        Err(SourcePreparationError::ReadOnly)
    ));
    assert!(!fixture.context.locator(&source).unwrap().exists());
}

#[test]
fn source_p_q_separation_fallback_c_and_stale_context() {
    let fixture = Fixture::new();
    assert_eq!(list_active_sources(&fixture.context).len(), 1);
    let p = fixture.root.path().join("P");
    let q = fixture.root.path().join("Q");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::create_dir_all(&q).unwrap();
    let mut settings = AppSettings {
        project_paths: vec![
            p.to_string_lossy().into_owned(),
            q.to_string_lossy().into_owned(),
        ],
        ..AppSettings::default()
    };
    let context =
        SourceReadContext::from_settings(&settings, &fixture.context.settings_path).unwrap();
    assert_eq!(list_active_sources(&context).len(), 1);
    let source = context.source(SourceKind::CatalogPersonal);
    assert!(context
        .locator(&source)
        .unwrap()
        .starts_with(std::fs::canonicalize(&p).unwrap()));
    settings.project_paths.swap(0, 1);
    let newer =
        SourceReadContext::from_settings(&settings, &fixture.context.settings_path).unwrap();
    assert!(matches!(
        read_source_snapshot(&newer, &source),
        Err(SourcePreparationError::ContextUnavailable)
    ));
}

#[test]
fn source_invalid_bytes_are_not_absent_and_schema1_patch_not_composed() {
    let fixture = Fixture::new();
    for kind in [SourceKind::CatalogBase, SourceKind::CatalogPersonal] {
        let source = fixture.context.source(kind);
        let path = fixture.context.locator(&source).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"broken").unwrap();
        let snapshot = read_source_snapshot(&fixture.context, &source).unwrap();
        assert_eq!(snapshot.snapshot.availability, SourceAvailability::Invalid);
        assert_eq!(snapshot.snapshot.revision, source_bytes_revision(b"broken"));
        assert!(snapshot.candidate_template.is_none());
        std::fs::write(
            &path,
            include_bytes!("../fixtures/independent_source_own_agent.json"),
        )
        .unwrap();
        let snapshot = read_source_snapshot(&fixture.context, &source).unwrap();
        if kind == SourceKind::CatalogBase {
            assert!(snapshot.snapshot.capabilities.available_read);
            assert!(!snapshot.snapshot.capabilities.editable_save);
            assert!(!snapshot.snapshot.capabilities.publish_managed);
            assert!(snapshot.snapshot.profiles.contains_key("own"));
            assert_eq!(
                prepare_source_candidate(
                    &snapshot,
                    &snapshot.snapshot.revision,
                    &json!({"schemaVersion":2,"agents":[]}),
                    None,
                )
                .unwrap_err(),
                SourcePreparationError::ReadOnly
            );
        } else {
            assert_eq!(
                snapshot.snapshot.availability,
                SourceAvailability::Available
            );
            assert_eq!(snapshot.snapshot.source.scope, SourceScope::Instance);
            assert!(!snapshot.snapshot.capabilities.editable_save);
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(
                prepare_source_candidate(
                    &snapshot,
                    &snapshot.snapshot.revision,
                    &json!({"schemaVersion":1,"agents":[]}),
                    None,
                )
                .unwrap_err(),
                SourcePreparationError::ReadOnly
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
    // Catalog default slots roundtrip through the actual source snapshot,
    // without changing unknown-field or source-admission authority.
    let project = fixture.root.path().join("slots-project");
    std::fs::create_dir(&project).unwrap();
    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    let context =
        SourceReadContext::from_settings(&settings, &fixture.context.settings_path).unwrap();
    let source = context.source(SourceKind::CatalogPersonal);
    assert_eq!(source.scope, SourceScope::Project);
    let path = context.locator(&source).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for schema in [1, 2] {
        for before_slots in [false, true] {
            for candidate_slots in [false, true] {
                assert_empty_slots_candidate(
                    &context,
                    &source,
                    &path,
                    schema,
                    before_slots,
                    candidate_slots,
                );
            }
        }
        for slots in [
            json!({"A":{"label":"keep"}}),
            json!({"A":{"label":"keep","opaque":{"nested":[null,7]}}}),
        ] {
            assert_nonempty_slots_omission(&context, &source, &path, schema, slots);
        }
        assert_invalid_slot_shapes(&context, &source, &path, schema);
    }
}

fn assert_empty_slots_candidate(
    context: &SourceReadContext,
    source: &SourceRef,
    path: &std::path::Path,
    schema: u32,
    before_slots: bool,
    candidate_slots: bool,
) {
    let mut before = json!({"schemaVersion":schema,"agents":[]});
    let mut candidate = before.clone();
    if before_slots {
        before["profileSlots"] = json!({});
    }
    if candidate_slots {
        candidate["profileSlots"] = json!({});
    }
    std::fs::write(path, serde_json::to_vec(&before).unwrap()).unwrap();
    let bytes = std::fs::read(path).unwrap();
    let snapshot = read_source_snapshot(context, source).unwrap();
    assert_eq!(
        snapshot.snapshot.availability,
        SourceAvailability::Available
    );
    assert!(snapshot.snapshot.capabilities.editable_save);
    let prepared =
        prepare_source_candidate(&snapshot, &snapshot.snapshot.revision, &candidate, None).unwrap();
    let output: Value = serde_json::from_slice(&prepared.serialized_after).unwrap();
    assert_eq!(output["profileSlots"], json!({}));
    assert_eq!(output["schemaVersion"], schema);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert_eq!(
        prepare_source_candidate(&snapshot, &SourceRevision::Absent {}, &candidate, None,)
            .unwrap_err(),
        SourcePreparationError::StaleRevision
    );
}

fn assert_nonempty_slots_omission(
    context: &SourceReadContext,
    source: &SourceRef,
    path: &std::path::Path,
    schema: u32,
    slots: Value,
) {
    let before = json!({"schemaVersion":schema,"agents":[],"profileSlots":slots});
    std::fs::write(path, serde_json::to_vec(&before).unwrap()).unwrap();
    let bytes = std::fs::read(path).unwrap();
    let snapshot = read_source_snapshot(context, source).unwrap();
    assert_eq!(
        snapshot.snapshot.availability,
        SourceAvailability::Available
    );
    assert_eq!(
        prepare_source_candidate(
            &snapshot,
            &snapshot.snapshot.revision,
            &json!({"schemaVersion":schema,"agents":[]}),
            None,
        )
        .unwrap_err(),
        SourcePreparationError::InvalidCandidate
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

fn assert_invalid_slot_shapes(
    context: &SourceReadContext,
    source: &SourceRef,
    path: &std::path::Path,
    schema: u32,
) {
    let before = json!({"schemaVersion":schema,"agents":[],"profileSlots":{}});
    std::fs::write(path, serde_json::to_vec(&before).unwrap()).unwrap();
    let bytes = std::fs::read(path).unwrap();
    let snapshot = read_source_snapshot(context, source).unwrap();
    for bad in [Value::Null, json!(0), json!("bad"), json!([])] {
        let mut candidate = before.clone();
        candidate["profileSlots"] = bad;
        assert_eq!(
            prepare_source_candidate(&snapshot, &snapshot.snapshot.revision, &candidate, None,)
                .unwrap_err(),
            SourcePreparationError::InvalidCandidate
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    let before = json!({"schemaVersion":schema,"agents":[],"foreign":{"keep":[null,7]}});
    std::fs::write(path, serde_json::to_vec(&before).unwrap()).unwrap();
    let bytes = std::fs::read(path).unwrap();
    let snapshot = read_source_snapshot(context, source).unwrap();
    let refused = prepare_source_candidate(
        &snapshot,
        &snapshot.snapshot.revision,
        &json!({"schemaVersion":schema,"agents":[]}),
        None,
    )
    .unwrap_err();
    match snapshot.snapshot.availability {
        SourceAvailability::Available => {
            assert_eq!(refused, SourcePreparationError::InvalidCandidate)
        }
        SourceAvailability::Invalid => {
            assert!(!snapshot.snapshot.capabilities.editable_save);
            assert_eq!(refused, SourcePreparationError::ReadOnly);
        }
        other => panic!("unexpected catalog availability: {other:?}"),
    }
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    for bad in [Value::Null, json!(0), json!([])] {
        let before = json!({"schemaVersion":schema,"agents":[],"profileSlots":bad});
        std::fs::write(path, serde_json::to_vec(&before).unwrap()).unwrap();
        let bytes = std::fs::read(path).unwrap();
        let invalid = read_source_snapshot(context, source).unwrap();
        assert_eq!(invalid.snapshot.availability, SourceAvailability::Invalid);
        assert!(!invalid.snapshot.capabilities.editable_save);
        assert_eq!(
            prepare_source_candidate(
                &invalid,
                &invalid.snapshot.revision,
                &json!({"schemaVersion":schema,"agents":[]}),
                None,
            )
            .unwrap_err(),
            SourcePreparationError::ReadOnly
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn source_raw30_not_effective50_unknown_fields_and_duplicate_new_group() {
    let fixture = Fixture::new();
    fixture.overlay(
        &json!({"agents":[{"id":"K","label":"Personal","color":"red","command":"personal-only"}]}),
    );
    let snapshot = fixture.snapshot();
    assert_eq!(snapshot.snapshot.agents[0]["command"], "own-base");
    let fixture = Fixture::new();
    let (_, origin) = fixture.origin();
    let snapshot = fixture.snapshot();
    let mut candidate = fixture.registered.clone();
    candidate["agents"][1]["command"] = candidate["agents"][0]["command"].clone();
    assert!(prepare_source_candidate(
        &snapshot,
        &snapshot.snapshot.revision,
        &candidate,
        Some(&origin)
    )
    .is_err());
    candidate = fixture.registered.clone();
    candidate.as_object_mut().unwrap().remove("foreign");
    assert!(prepare_source_candidate(
        &snapshot,
        &snapshot.snapshot.revision,
        &candidate,
        Some(&origin)
    )
    .is_err());
    let mut registered = fixture.registered.clone();
    registered["profileSlots"] = json!({});
    let path = fixture.root.path().join(AGENTS_INSTANCE_FILE_NAME);
    std::fs::write(&path, serde_json::to_vec(&registered).unwrap()).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let snapshot = fixture.snapshot();
    let (_, origin) = fixture.origin();
    registered.as_object_mut().unwrap().remove("profileSlots");
    assert_eq!(
        prepare_source_candidate(
            &snapshot,
            &snapshot.snapshot.revision,
            &registered,
            Some(&origin),
        )
        .unwrap_err(),
        SourcePreparationError::InvalidCandidate
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn source_project40_is_owner_only_and_general_rejects_protected_keys() {
    let fixture = Fixture::new();
    let settings = AppSettings {
        project_paths: vec![fixture.root.path().to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    let context =
        SourceReadContext::from_settings(&settings, &fixture.context.settings_path).unwrap();
    let source = context.source(SourceKind::CatalogProject);
    let path = context.locator(&source).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let candidate = json!({"schemaVersion":2,"agents":[]});
    std::fs::write(&path, serde_json::to_vec(&candidate).unwrap()).unwrap();
    let snapshot = read_source_snapshot(&context, &source).unwrap();
    assert!(snapshot.snapshot.capabilities.available_read);
    assert!(!snapshot.snapshot.capabilities.repair_by_owner);
    assert!(!snapshot.snapshot.capabilities.editable_save);
    assert!(matches!(
        prepare_source_candidate(&snapshot, &snapshot.snapshot.revision, &candidate, None),
        Err(SourcePreparationError::ReadOnly)
    ));
    let inputs = fixture.origin().0;
    for key in [
        "agents",
        "codingAgentProfiles",
        "rootToken",
        "mainGeometry",
        "projectPaths",
        "unknownClientKey",
    ] {
        assert!(matches!(
            prepare_settings_without_source_write(&inputs, &json!({key:null})),
            Err(SourcePreparationError::ProtectedSettingsField)
        ));
    }
}

#[test]
fn source_general_absent_prepares_without_materializing_protected_groups() {
    let fixture = Fixture::new();
    std::fs::remove_file(fixture.root.path().join(SETTINGS_FILE_NAME)).unwrap();
    let inputs = fixture.origin().0;
    let plan = prepare_settings_without_source_write(&inputs, &Fixture::general_candidate("debug"))
        .unwrap();
    let value: Value = serde_json::from_slice(&plan.serialized_after).unwrap();
    for key in SOURCE_GENERAL_PROTECTED {
        assert!(value.get(*key).is_none(), "{key}");
    }
    assert!(!fixture.root.path().join(SETTINGS_FILE_NAME).exists());
    assert_eq!(plan.redecoded_settings.agents[0].command, "own-base");
}
