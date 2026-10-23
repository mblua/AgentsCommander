use crate::config::coding_agent_mutations::*;
use crate::config::settings::*;

fn r3_ready_fixture() -> (DriverFixture, String) {
    let fixture = DriverFixture::new();
    let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
    std::fs::remove_file(&target).unwrap();
    let snapshot = read_source_snapshot(&fixture.context, &fixture.source).unwrap();
    assert_eq!(snapshot.snapshot.revision, SourceRevision::Absent {});
    assert!(!snapshot.snapshot.capabilities.editable_save);
    assert!(!snapshot.snapshot.capabilities.editable_create);
    // Current admission stays read-only. Historical recovery below does not
    // grant absent creation to the coordinator or any application caller.
    let publications = source_transition_publication_counts();
    let effects = source_transition_effect_counts().1;
    let native = crate::config::local_config_io::source_checkpoint_native_calls();
    let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
    let journal_before = read_own_source_journal(&fixture.context).unwrap();
    let reloads = std::cell::Cell::new(0);
    assert!(matches!(
        commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "absent-admission",
            &snapshot.snapshot.revision,
            &DriverFixture::candidate(),
            &|| {
                reloads.set(reloads.get() + 1);
                Ok(())
            }
        ),
        Err(SourceTransitionError::Preparation(
            SourcePreparationError::ReadOnly
        ))
    ));
    assert_eq!(
        read_own_source_journal(&fixture.context).unwrap(),
        journal_before
    );
    assert!(journal_before.1.intents.is_empty());
    assert_eq!(source_transition_publication_counts(), publications);
    assert_eq!(source_transition_effect_counts().1, effects);
    assert_eq!(
        crate::config::local_config_io::source_checkpoint_native_calls(),
        native
    );
    assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
    assert_eq!(reloads.get(), 0);
    assert!(!target.exists());

    // A historical absent-before candidate, not current admission. No
    // publication proof, stage or existing operation is supplied/rewritten.
    let candidate = DriverFixture::candidate();
    let prepared: PreparedSourceCandidate = serde_json::from_value(serde_json::json!({
        "source": fixture.source,
        "expectedRevision": snapshot.snapshot.revision,
        "contextRevision": snapshot.snapshot.context_revision,
        "beforeBytes": null,
        "serializedAfter": serde_json::to_vec(&candidate).unwrap(),
        "persistedNames": [],
        "originBinding": null
    }))
    .unwrap();
    let identity =
        IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
    let lease = SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
    let origin_inputs = OriginInputs::capture(&fixture.context).unwrap();
    let origin = LegacyCompatibilityOrigin::classify(&origin_inputs);
    let mut capture = capture_source_inventory(
        &fixture.settings,
        &snapshot,
        &origin,
        lease.physical_source_key(),
    )
    .unwrap();
    assert!(capture.original_targets().is_empty());
    capture.classify(Vec::new()).unwrap();
    let operation = new_source_transition_id();
    let authority = SourceInventoryProof::for_candidate(
        SourceInventoryInputs {
            settings: &fixture.settings,
            context: &fixture.context,
            identity: &identity,
            lease: &lease,
        },
        &snapshot,
        &prepared,
        &origin_inputs,
        capture,
        &operation,
    )
    .unwrap();
    let rediscover = || -> Result<(), SourceTransitionError> {
        let (witness, targets) = source_discovery_evidence(&fixture.settings)?;
        authority.verify_complete_discovery(&witness, &targets)?;
        Ok(())
    };
    rediscover().unwrap();
    authority
        .verify_original_classification(&|_| Ok(false))
        .unwrap();
    let payload =
        source_transition_digest(&(&fixture.source, &snapshot.snapshot.revision, &candidate))
            .unwrap();
    let intent = authority
        .stored_intent(&prepared, &operation, "r3-original", &payload)
        .unwrap();
    let revision = read_own_source_journal(&fixture.context).unwrap().0;
    update_own_source_journal_with_validation(
        &identity,
        &lease,
        &fixture.context,
        &revision,
        |journal| {
            journal.intents.insert(operation.clone(), intent);
            journal
                .inventories
                .insert(operation.clone(), authority.inventory().clone());
            journal
                .progress
                .insert(operation.clone(), SourceLocalProgress::default());
            Ok(())
        },
        &|| authority.validate(&identity, &lease, &fixture.context),
    )
    .unwrap();
    configure_source_durable_cut(Some(("prepared", 1)));
    let result = drive_identity_transition(
        &identity,
        &lease,
        &fixture.context,
        &operation,
        &authority,
        &rediscover,
        &|| Ok(()),
    );
    assert!(
        matches!(&result, Err(SourceTransitionError::Coordination(
            SourceCoordinationError::Io(reason)
        )) if reason == "durable cut: prepared"),
        "r3 readiness result: {result:?}"
    );
    SOURCE_DURABLE_CUT.with(|cut| assert!(cut.borrow().0.is_none()));
    configure_source_durable_cut(None);
    let journal = read_own_source_journal(&fixture.context).unwrap().1;
    assert!(journal.intents[&operation].before_bytes.is_none());
    assert!(journal.progress[&operation].source_publication.is_none());
    assert_eq!(
        get_identity_transition(&fixture.context, &fixture.source, &operation)
            .unwrap()
            .state,
        IdentityTransitionReadState::Prepared
    );
    drop(lease);
    drop(identity);
    r3_assert_pending(&fixture, &operation);
    (fixture, operation)
}

fn r3_prepared_fixture() -> (DriverFixture, String, std::path::PathBuf) {
    let (fixture, operation) = r3_ready_fixture();
    let native = crate::config::local_config_io::source_checkpoint_native_calls();
    {
        let identity =
            IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
        let lease =
            SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
        let intent = read_own_source_intent(&fixture.context, &operation)
            .unwrap()
            .1
            .unwrap();
        let cut = || {
            if read_own_source_journal(&fixture.context)
                .unwrap()
                .1
                .progress[&operation]
                .source_publication
                .is_some()
            {
                Err(IdentityReferenceError::Conflict)
            } else {
                Ok(())
            }
        };
        assert!(
            publish_source_intent_with_validation(&lease, &fixture.context, &intent, &cut).is_err()
        );
        assert_eq!(lease.source_revision().unwrap(), SourceRevision::Absent {});
    }
    assert_eq!(
        crate::config::local_config_io::source_checkpoint_native_calls(),
        native
    );
    let journal = read_own_source_journal(&fixture.context).unwrap().1;
    let proof = journal.progress[&operation]
        .source_publication
        .as_ref()
        .unwrap();
    let value = serde_json::to_value(proof).unwrap();
    let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
    let stage = target
        .parent()
        .unwrap()
        .join(value["stageLeaf"].as_str().unwrap());
    assert_eq!(
        std::fs::read(&stage).unwrap(),
        journal.intents[&operation].serialized_after
    );
    (fixture, operation, stage)
}

fn r3_assert_pending(fixture: &DriverFixture, operation: &str) {
    let journal = read_own_source_journal(&fixture.context).unwrap().1;
    let progress = &journal.progress[operation];
    assert!(progress.packets_prepared && !progress.source_published);
    assert!(!progress.refs_verified && !progress.reloaded && progress.receipt.is_none());
}

#[test]
fn single_editor_absent_foreign_identical_refusal() {
    // Calibrate the last premove validation on this same synchronous path.
    let (control, control_op, _) = r3_prepared_fixture();
    let premove = std::cell::Cell::new(0);
    let calls = std::cell::Cell::new(0);
    {
        let identity =
            IdentityWriteLease::acquire(&control.context, std::time::Duration::ZERO).unwrap();
        let lease =
            SourceWriteContext::acquire(&control.context, &control.source, &identity).unwrap();
        let intent = read_own_source_intent(&control.context, &control_op)
            .unwrap()
            .1
            .unwrap();
        let observe = || {
            calls.set(calls.get() + 1);
            if lease.source_revision().unwrap() == (SourceRevision::Absent {}) {
                premove.set(calls.get());
            }
            Ok(())
        };
        publish_source_intent_with_validation(&lease, &control.context, &intent, &observe).unwrap();
    }
    assert!(premove.get() > 0);
    let (fixture, operation, stage) = r3_prepared_fixture();
    let parent = crate::path_identity::retain_directory(stage.parent().unwrap()).unwrap();
    let own_object = parent.verify_regular_file(&stage).unwrap();
    let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
    let original = read_own_source_journal(&fixture.context).unwrap().1;
    let after = original.intents[&operation].serialized_after.clone();
    let native = crate::config::local_config_io::source_checkpoint_native_calls();
    {
        let identity =
            IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
        let lease =
            SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
        let calls = std::cell::Cell::new(0);
        let interfere = || {
            calls.set(calls.get() + 1);
            if calls.get() == premove.get() {
                std::fs::write(&target, &after).unwrap();
            }
            Ok(())
        };
        assert!(publish_source_intent_with_validation(
            &lease,
            &fixture.context,
            &original.intents[&operation],
            &interfere
        )
        .is_err());
        assert_eq!(
            crate::config::local_config_io::source_checkpoint_native_calls(),
            native + 1
        );
        let foreign = parent.verify_regular_file(&target).unwrap();
        assert!(!crate::path_identity::same_object(&own_object, &foreign));
        assert!(publish_source_intent_with_validation(
            &lease,
            &fixture.context,
            &original.intents[&operation],
            &|| Ok(())
        )
        .is_err());
        assert_eq!(parent.verify_regular_file(&target).unwrap(), foreign);
    }
    for _ in 0..2 {
        let reloads = std::cell::Cell::new(0);
        assert!(resume_identity_transition_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            &operation,
            &fixture.expectation(),
            &|| {
                reloads.set(reloads.get() + 1);
                Ok(())
            }
        )
        .is_err());
        assert_eq!(reloads.get(), 0);
        r3_assert_pending(&fixture, &operation);
    }
    assert_eq!(
        crate::config::local_config_io::source_checkpoint_native_calls(),
        native + 1
    );
    assert_eq!(
        read_own_source_journal(&fixture.context).unwrap().1,
        original
    );
    assert_eq!(std::fs::read(&target).unwrap(), after);
    assert_eq!(parent.verify_regular_file(&stage).unwrap(), own_object);
    assert_eq!(std::fs::read(&stage).unwrap(), after);
}

#[test]
fn single_editor_absent_ownmove_restart() {
    let (fixture, operation, stage) = r3_prepared_fixture();
    let parent = crate::path_identity::retain_directory(stage.parent().unwrap()).unwrap();
    let staged = parent.verify_regular_file(&stage).unwrap();
    let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
    let original = read_own_source_journal(&fixture.context).unwrap().1;
    let native = crate::config::local_config_io::source_checkpoint_native_calls();
    {
        let identity =
            IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
        let lease =
            SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
        let cut = || {
            if target.exists() {
                Err(IdentityReferenceError::Conflict)
            } else {
                Ok(())
            }
        };
        assert!(publish_source_intent_with_validation(
            &lease,
            &fixture.context,
            &original.intents[&operation],
            &cut
        )
        .is_err());
    }
    assert!(!stage.exists());
    assert!(crate::path_identity::same_object(
        &staged,
        &parent.verify_regular_file(&target).unwrap()
    ));
    r3_assert_pending(&fixture, &operation);
    fixture.resume_at(&fixture.context, &operation).unwrap();
    let complete = read_own_source_journal(&fixture.context).unwrap().1;
    assert!(complete.progress[&operation].receipt.is_some());
    assert_eq!(complete.intents[&operation], original.intents[&operation]);
    assert_eq!(
        complete.progress[&operation].source_publication,
        original.progress[&operation].source_publication
    );
    assert_eq!(
        crate::config::local_config_io::source_checkpoint_native_calls(),
        native + 1
    );
}

#[test]
fn single_editor_r3_checkpoint_same_stage() {
    let (fixture, operation, stage) = r3_prepared_fixture();
    let parent = crate::path_identity::retain_directory(stage.parent().unwrap()).unwrap();
    let staged = parent.verify_regular_file(&stage).unwrap();
    let original = read_own_source_journal(&fixture.context).unwrap().1;
    let bytes = &original.intents[&operation].serialized_after;
    let native = crate::config::local_config_io::source_checkpoint_native_calls();
    std::fs::write(&stage, b"{\"modified\":true}").unwrap();
    assert!(fixture.resume_at(&fixture.context, &operation).is_err());
    r3_assert_pending(&fixture, &operation);
    assert_eq!(
        crate::config::local_config_io::source_checkpoint_native_calls(),
        native
    );
    std::fs::write(&stage, bytes).unwrap();
    assert!(crate::path_identity::same_object(
        &staged,
        &parent.verify_regular_file(&stage).unwrap()
    ));
    let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
    fixture.resume_at(&fixture.context, &operation).unwrap();
    let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
    assert!(!stage.exists());
    assert!(crate::path_identity::same_object(
        &staged,
        &parent.verify_regular_file(&target).unwrap()
    ));
    let complete = read_own_source_journal(&fixture.context).unwrap().1;
    assert!(complete.progress[&operation].receipt.is_some());
    assert_eq!(
        complete.progress[&operation].source_publication,
        original.progress[&operation].source_publication
    );
    assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
    assert_eq!(
        crate::config::local_config_io::source_checkpoint_native_calls(),
        native + 1
    );
}

#[test]
fn single_editor_r3_stage_missing_replaced_modified() {
    for mode in ["missing", "replaced", "modified"] {
        let (fixture, operation, stage) = r3_prepared_fixture();
        let journal = read_own_source_journal(&fixture.context).unwrap().1;
        let parent = crate::path_identity::retain_directory(stage.parent().unwrap()).unwrap();
        let own = parent.verify_regular_file(&stage).unwrap();
        if mode == "modified" {
            std::fs::write(&stage, b"{\"external\":true}").unwrap();
        } else {
            std::fs::rename(&stage, stage.with_extension("external-diagnostic")).unwrap();
            if mode == "replaced" {
                std::fs::write(&stage, &journal.intents[&operation].serialized_after).unwrap();
                assert!(!crate::path_identity::same_object(
                    &own,
                    &parent.verify_regular_file(&stage).unwrap()
                ));
            }
        }
        let before = std::fs::read(&stage).ok();
        let native = crate::config::local_config_io::source_checkpoint_native_calls();
        let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
        for _ in 0..2 {
            assert!(fixture.resume_at(&fixture.context, &operation).is_err());
        }
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            journal
        );
        assert_eq!(std::fs::read(&stage).ok(), before);
        assert_eq!(
            crate::config::local_config_io::source_checkpoint_native_calls(),
            native
        );
        assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
        r3_assert_pending(&fixture, &operation);
    }
}

#[test]
fn single_editor_r3_cas_error_classification() {
    // Real Err-landed checkpoint is created by the existing validation cut.
    let (after_fixture, after_operation, after_stage) = r3_prepared_fixture();
    r3_assert_pending(&after_fixture, &after_operation);
    assert!(after_stage.exists());
    for mode in ["before", "other", "sticky"] {
        let (fixture, operation) = r3_ready_fixture();
        let before = read_own_source_journal(&fixture.context).unwrap();
        let native = crate::config::local_config_io::source_checkpoint_native_calls();
        let reached = std::cell::Cell::new(false);
        let private_path = fixture
            ._root
            .path()
            .join("C")
            .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
        {
            let identity =
                IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
            let lease =
                SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
            let cut = || {
                let metadata_stage = std::fs::read_dir(private_path.parent().unwrap())
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .any(|p| {
                        p.extension().is_some_and(|e| e == "tmp")
                            && std::fs::read(&p).ok().is_some_and(|b| {
                                serde_json::from_slice::<serde_json::Value>(&b)
                                    .ok()
                                    .is_some_and(|v| {
                                        v["progress"][&operation]["sourcePublication"].is_object()
                                    })
                            })
                    });
                if metadata_stage && !reached.replace(true) {
                    if mode == "other" {
                        std::fs::write(&private_path, b"{broken").unwrap();
                    }
                    return Err(IdentityReferenceError::Conflict);
                }
                if mode == "sticky" && reached.get() {
                    panic!("validation re-entered after sticky refusal");
                }
                Ok(())
            };
            assert!(publish_source_intent_with_validation(
                &lease,
                &fixture.context,
                &before.1.intents[&operation],
                &cut
            )
            .is_err());
            assert!(reached.get());
        }
        assert_eq!(
            crate::config::local_config_io::source_checkpoint_native_calls(),
            native
        );
        if mode == "other" {
            assert_eq!(std::fs::read(&private_path).unwrap(), b"{broken");
        } else {
            assert_eq!(read_own_source_journal(&fixture.context).unwrap(), before);
            r3_assert_pending(&fixture, &operation);
        }
    }
}

#[test]
fn single_editor_r3_checkpointed_stage_never_unlinked() {
    let (fixture, operation, stage) = r3_prepared_fixture();
    let original = read_own_source_journal(&fixture.context).unwrap().1;
    let parent = crate::path_identity::retain_directory(stage.parent().unwrap()).unwrap();
    let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
    let owned = parent.verify_regular_file(&stage).unwrap();
    let proof = original.progress[&operation]
        .source_publication
        .as_ref()
        .unwrap();
    let refusal = std::cell::Cell::new(false);
    let validate = || {
        if refusal.get() {
            Err(IdentityReferenceError::Conflict)
        } else {
            Ok(())
        }
    };
    let native = crate::config::local_config_io::source_checkpoint_native_calls();
    {
        let mut capability = crate::config::local_config_io::reopen_source_publication(
            &parent,
            &target,
            &original.intents[&operation].serialized_after,
            proof,
            &validate,
        )
        .unwrap();
        refusal.set(true);
        assert!(capability.commit(&|| Ok(())).is_err());
        refusal.set(false);
        assert!(capability.commit(&|| Ok(())).is_err());
    }
    assert_eq!(parent.verify_regular_file(&stage).unwrap(), owned);
    assert_eq!(
        std::fs::read(&stage).unwrap(),
        original.intents[&operation].serialized_after
    );
    assert!(!target.exists());
    assert_eq!(
        crate::config::local_config_io::source_checkpoint_native_calls(),
        native
    );
    assert_eq!(
        read_own_source_journal(&fixture.context).unwrap().1,
        original
    );
}

#[test]
fn single_editor_r3_stale_progress_preserves_proof() {
    let (fixture, operation, _) = r3_prepared_fixture();
    let identity =
        IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
    let lease = SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
    let (stale, original) = read_own_source_journal(&fixture.context).unwrap();
    let private_path = fixture
        ._root
        .path()
        .join("C")
        .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
    let mut bytes = std::fs::read(&private_path).unwrap();
    bytes.push(10);
    std::fs::write(&private_path, bytes).unwrap();
    let fresh = read_own_source_journal(&fixture.context).unwrap().0;
    for field in ["packets", "aggregate", "refs", "reload", "receipt"] {
        assert_eq!(
            update_own_source_journal_with_validation(
                &identity,
                &lease,
                &fixture.context,
                &stale,
                |journal| {
                    let progress = journal.progress.get_mut(&operation).unwrap();
                    match field {
                        "packets" => progress.packets.clear(),
                        "aggregate" => progress.packets_prepared = false,
                        "refs" => progress.refs_verified = false,
                        "reload" => progress.reloaded = true,
                        _ => progress.receipt = None,
                    }
                    Ok(())
                },
                &|| Ok(())
            ),
            Err(SourceCoordinationError::Stale)
        );
    }
    assert_eq!(
        update_own_source_journal_with_validation(
            &identity,
            &lease,
            &fixture.context,
            &fresh,
            |journal| {
                journal
                    .progress
                    .get_mut(&operation)
                    .unwrap()
                    .source_publication = None;
                Ok(())
            },
            &|| Ok(())
        ),
        Err(SourceCoordinationError::Invalid)
    );
    let current = read_own_source_journal(&fixture.context).unwrap().1;
    assert_eq!(
        current.progress[&operation].source_publication,
        original.progress[&operation].source_publication
    );
    assert_eq!(current.intents, original.intents);

    // The private journal spans sources; this lease's ledger does not.
    // An unchanged proof belonging to another source remains valid.
    let registered_path = fixture
        ._root
        .path()
        .join("C")
        .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME);
    std::fs::write(&registered_path, b"{\"agents\":[]}").unwrap();
    let other = list_active_sources(&fixture.context)
        .into_iter()
        .find(|source| source.kind == SourceKind::RegisteredInstance)
        .unwrap();
    assert_ne!(other, fixture.source);
    let other_lease = SourceWriteContext::acquire(&fixture.context, &other, &identity).unwrap();
    assert_eq!(
        update_own_source_journal_with_validation(
            &identity,
            &other_lease,
            &fixture.context,
            &fresh,
            |_| Ok(()),
            &|| Ok(())
        ),
        Ok(fresh)
    );
    assert_eq!(
        read_own_source_journal(&fixture.context).unwrap().1,
        current
    );
}

#[test]
fn single_editor_r3_legacy_none_before_after() {
    for foreign_after in [false, true] {
        let (fixture, operation) = r3_ready_fixture();
        let journal = read_own_source_journal(&fixture.context).unwrap().1;
        assert!(journal.progress[&operation].source_publication.is_none());
        let legacy = serde_json::to_value(&journal.progress[&operation]).unwrap();
        assert!(legacy.get("sourcePublication").is_none());
        let native = crate::config::local_config_io::source_checkpoint_native_calls();
        if foreign_after {
            let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
            std::fs::write(&target, &journal.intents[&operation].serialized_after).unwrap();
            assert!(fixture.resume_at(&fixture.context, &operation).is_err());
            assert_eq!(
                read_own_source_journal(&fixture.context).unwrap().1,
                journal
            );
            assert_eq!(
                crate::config::local_config_io::source_checkpoint_native_calls(),
                native
            );
            r3_assert_pending(&fixture, &operation);
        } else {
            fixture.resume_at(&fixture.context, &operation).unwrap();
            let complete = read_own_source_journal(&fixture.context).unwrap().1;
            assert!(complete.progress[&operation].source_publication.is_some());
            assert!(complete.progress[&operation].receipt.is_some());
            assert_eq!(
                crate::config::local_config_io::source_checkpoint_native_calls(),
                native + 1
            );
        }
    }
}

struct DriverFixture {
    _root: tempfile::TempDir,
    settings: AppSettings,
    context: SourceReadContext,
    source: SourceRef,
    project: std::path::PathBuf,
    initial_revision: SourceRevision,
}

impl DriverFixture {
    fn new() -> Self {
        let root = crate::config::settings::source_transition_test_root();
        let project = root.path().join("project");
        let config = root.path().join("C");
        std::fs::create_dir_all(project.join(".ac/coding-agents")).unwrap();
        std::fs::create_dir(&config).unwrap();
        let settings = AppSettings {
            project_paths: vec![project.to_string_lossy().into_owned()],
            ..AppSettings::default()
        };
        let context =
            SourceReadContext::from_settings(&settings, &config.join("settings.json")).unwrap();
        let source = crate::config::settings::SourceRef {
            family: crate::config::settings::SourceFamily::Agents,
            role: crate::config::settings::SourceRole::Catalog,
            scope: crate::config::settings::SourceScope::Project,
            kind: crate::config::settings::SourceKind::CatalogPersonal,
            context_root: std::fs::canonicalize(&project)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        };
        let seed = serde_json::json!({"schemaVersion":2,"agents":[{
            "key":"fixture-seed","label":"Fixture seed","description":"Owned","color":"#123456",
            "command":"fixture-before-command","envs":[]}]});
        assert_ne!(seed, Self::candidate());
        let target = source_transition_test_locator(&context, &source).unwrap();
        std::fs::write(&target, serde_json::to_vec(&seed).unwrap()).unwrap();
        let snapshot = read_source_snapshot(&context, &source).unwrap();
        assert_eq!(
            snapshot.snapshot.availability,
            SourceAvailability::Available
        );
        assert!(snapshot.snapshot.capabilities.editable_save);
        let initial_revision = snapshot.snapshot.revision;
        Self {
            _root: root,
            settings,
            context,
            source,
            project,
            initial_revision,
        }
    }
    fn candidate() -> serde_json::Value {
        serde_json::json!({"schemaVersion": 2, "agents": []})
    }
    fn start(&self, request: &str) -> IdentityTransitionRead {
        let revision = read_source_snapshot(&self.context, &self.source)
            .unwrap()
            .snapshot
            .revision;
        commit_source_candidate_internal(
            &self.settings,
            &self.context,
            &self.source,
            request,
            &revision,
            &Self::candidate(),
            &|| Ok(()),
        )
        .unwrap()
    }
    fn expectation(&self) -> SourceResumeExpectation {
        let identity =
            IdentityWriteLease::acquire(&self.context, std::time::Duration::ZERO).unwrap();
        let lease = SourceWriteContext::acquire(&self.context, &self.source, &identity).unwrap();
        SourceResumeExpectation {
            source: lease.source_revision().unwrap(),
            journal: read_own_source_journal(&self.context).unwrap().0,
        }
    }
    fn operation(&self) -> String {
        read_own_source_journal(&self.context)
            .unwrap()
            .1
            .intents
            .keys()
            .next()
            .unwrap()
            .clone()
    }
}

#[test]
fn single_editor_complete_without_participants() {
    let fixture = DriverFixture::new();
    let reloads = std::cell::Cell::new(0);
    let result = commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "request-empty",
        &fixture.initial_revision,
        &DriverFixture::candidate(),
        &|| {
            reloads.set(reloads.get() + 1);
            Ok(())
        },
    )
    .unwrap();
    let operation = fixture.operation();
    let journal = read_own_source_journal(&fixture.context).unwrap().1;
    assert!(journal.inventories[&operation].targets.is_empty());
    let progress = &journal.progress[&operation];
    assert!(progress.packets.is_empty());
    assert!(progress.refs_verified && progress.reloaded && progress.receipt.is_some());
    assert_eq!(reloads.get(), 1);
    assert_ne!(result.state, IdentityTransitionReadState::NotFound);
    // Real completed Resume must recognize after without calling the publisher.
    let publishers = source_transition_effect_counts().1;
    let expected = fixture.expectation();
    resume_identity_transition_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        &operation,
        &expected,
        &|| Ok(()),
    )
    .unwrap();
    assert_eq!(source_transition_effect_counts().1, publishers);
}

#[test]
fn single_editor_driver_request_lookup_before_uuid_and_new_click_has_new_identity() {
    let fixture = DriverFixture::new();
    fixture.start("request-1");
    let operation = fixture.operation();
    let count = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
    commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "request-1",
        &fixture.initial_revision,
        &DriverFixture::candidate(),
        &|| Ok(()),
    )
    .unwrap();
    assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), count);
    assert!(matches!(
        commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "request-1",
            &fixture.initial_revision,
            &serde_json::Value::Null,
            &|| Ok(())
        ),
        Err(SourceTransitionError::Coordination(
            SourceCoordinationError::Invalid
        ))
    ));
    fixture.start("request-2");
    let journal = read_own_source_journal(&fixture.context).unwrap().1;
    assert_eq!(journal.intents.len(), 2);
    assert!(journal.intents.contains_key(&operation));
    assert_eq!(
        SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get),
        count + 1
    );
}

#[test]
fn single_editor_driver_complete_resume_rejects_source_z_and_root_replacement() {
    let fixture = DriverFixture::new();
    fixture.start("request-z");
    let operation = fixture.operation();
    let source_path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
    std::fs::write(&source_path, b"{\"unrelated\":\"Z\"}").unwrap();
    let expected = fixture.expectation();
    let count = source_transition_effect_counts().1;
    assert!(resume_identity_transition_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        &operation,
        &expected,
        &|| Ok(())
    )
    .is_err());
    assert_eq!(
        std::fs::read(&source_path).unwrap(),
        b"{\"unrelated\":\"Z\"}"
    );
    assert_eq!(source_transition_effect_counts().1, count);
    let original = read_own_source_journal(&fixture.context).unwrap().1;
    assert!(original.progress[&operation].receipt.is_some());
}
#[test]
fn single_editor_driver_actual_guard_cuts_preserve_original_and_resume_without_new_uuid() {
    configure_source_factory_guard_cut(None);
    let baseline = DriverFixture::new();
    baseline.start("baseline");
    let cuts = source_factory_guard_calls();
    assert!(cuts > 0);
    for cut in 1..=cuts {
        let fixture = DriverFixture::new();
        configure_source_factory_guard_cut(Some(cut));
        let result = commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "request-cut",
            &fixture.initial_revision,
            &DriverFixture::candidate(),
            &|| Ok(()),
        );
        assert!(
            !source_factory_guard_cut_armed(),
            "actual guard cut {cut} was not reached"
        );
        assert!(result.is_err(), "guard refusal was swallowed at cut {cut}");
        configure_source_factory_guard_cut(None);
        let (_, original) = read_own_source_journal(&fixture.context).unwrap();
        if let Some((operation, intent)) = original.intents.iter().next() {
            let operation = operation.clone();
            let intent = intent.clone();
            let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
            let expected = fixture.expectation();
            resume_identity_transition_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                &operation,
                &expected,
                &|| Ok(()),
            )
            .unwrap();
            let recovered = read_own_source_journal(&fixture.context).unwrap().1;
            assert_eq!(recovered.intents[&operation], intent);
            assert_eq!(
                recovered.inventories[&operation],
                original.inventories[&operation]
            );
            assert!(recovered.progress[&operation].receipt.is_some());
            assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
            let publisher_calls = source_transition_effect_counts().1;
            let expected = fixture.expectation();
            resume_identity_transition_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                &operation,
                &expected,
                &|| Ok(()),
            )
            .unwrap();
            assert_eq!(
                source_transition_effect_counts().1,
                publisher_calls,
                "completed after recognition republished at cut {cut}"
            );
        } else {
            assert!(result.is_err());
            // An admission may exist, but no operation/source may be invented.
            assert_eq!(
                read_source_snapshot(&fixture.context, &fixture.source)
                    .unwrap()
                    .snapshot
                    .revision,
                fixture.initial_revision
            );
            assert!(original.inventories.is_empty() && original.progress.is_empty());
        }
    }
}

#[test]
fn single_editor_driver_rename_allowed_but_replaced_discovery_object_rejected() {
    let fixture = DriverFixture::new();
    fixture.start("request-root");
    let operation = fixture.operation();
    let new_source = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
    let canonical_project = std::fs::canonicalize(&fixture.project).unwrap();
    let canonical_source = std::fs::canonicalize(&new_source).unwrap();
    let relative_source = canonical_source
        .strip_prefix(&canonical_project)
        .unwrap()
        .to_path_buf();
    let old = fixture.project.with_file_name("original-project");
    std::fs::rename(&fixture.project, &old).unwrap();
    std::fs::create_dir_all(fixture.project.join(".ac/coding-agents")).unwrap();
    let old_source = old.join(relative_source);
    std::fs::copy(&old_source, &new_source).unwrap();
    let expected = fixture.expectation();
    let before = source_transition_effect_counts().1;
    assert!(resume_identity_transition_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        &operation,
        &expected,
        &|| Ok(())
    )
    .is_err());
    assert_eq!(source_transition_effect_counts().1, before);
    std::fs::remove_file(&new_source).unwrap();
}

impl DriverFixture {
    fn with_reference_targets() -> (
        Self,
        serde_json::Value,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let fixture = Self::new();
        let before = serde_json::json!({"schemaVersion":2,"agents":[{
            "key":"custom-k","label":"Custom K","description":"Owned","color":"#123456",
            "command":"original-command","envs":[]}],
            "codingAgentProfiles":{"custom-k":{"A":{"command":"--profile", "env":{}}}}});
        let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        std::fs::write(path, serde_json::to_vec(&before).unwrap()).unwrap();
        let snapshot = read_source_snapshot(&fixture.context, &fixture.source).unwrap();
        let selection = ConfigurationRef {
            version: 1,
            source: fixture.source.clone(),
            calculated_id: configuration_identity("original-command", &[]),
            requested_profile: Some("A".into()),
            profile_calculated_id: Some(profile_identity(
                &snapshot.snapshot.profiles["custom-k"]["A"],
            )),
            legacy_entry_hint: Some("custom-k".into()),
        };
        let target = fixture.project.join(".ac/_agent_N");
        let disjoint = fixture.project.join(".ac/_agent_U");
        std::fs::create_dir(&target).unwrap();
        std::fs::create_dir(&disjoint).unwrap();
        std::fs::write(
            target.join("config.json"),
            serde_json::to_vec(&serde_json::json!({"unknown":null,
            "tooling":{"configurationRef":selection.clone(), "unknown":{"keep":[null,false]}}}))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
            serde_json::to_vec(
                &serde_json::json!({"unknown":[null],"tooling":{"sourceReferences":[{
                "configurationRef":selection.clone(), "unknown":true}]}}),
            )
            .unwrap(),
        )
        .unwrap();
        let mut foreign = selection;
        foreign.source = crate::config::settings::SourceRef {
            family: crate::config::settings::SourceFamily::Agents,
            role: crate::config::settings::SourceRole::Catalog,
            scope: crate::config::settings::SourceScope::Project,
            kind: crate::config::settings::SourceKind::CatalogBase,
            context_root: std::fs::canonicalize(&fixture.project)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        };
        std::fs::write(
            disjoint.join("config.json"),
            serde_json::to_vec(&serde_json::json!({"unknown":true,
            "tooling":{"configurationRef":foreign}}))
            .unwrap(),
        )
        .unwrap();
        let mut candidate = before;
        candidate["agents"][0]["command"] = serde_json::json!("updated-command");
        (fixture, candidate, target, disjoint)
    }
}

#[test]
fn single_editor_driver_actual_packet_changes_only_affected_target_then_releases_original() {
    let (fixture, candidate, target, disjoint) = DriverFixture::with_reference_targets();
    let untouched = std::fs::read(disjoint.join("config.json")).unwrap();
    let revision = read_source_snapshot(&fixture.context, &fixture.source)
        .unwrap()
        .snapshot
        .revision;
    let reloads = std::cell::Cell::new(0);
    commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "request-packet",
        &revision,
        &candidate,
        &|| {
            reloads.set(reloads.get() + 1);
            Ok(())
        },
    )
    .unwrap();
    let operation = fixture.operation();
    let journal = read_own_source_journal(&fixture.context).unwrap().1;
    assert_eq!(journal.inventories[&operation].targets.len(), 1);
    assert_eq!(journal.progress[&operation].packets.len(), 1);
    assert_eq!(journal.progress[&operation].packets.len(), 1);
    assert!(journal.progress[&operation].receipt.is_some());
    assert_eq!(reloads.get(), 1);
    assert_eq!(
        std::fs::read(disjoint.join("config.json")).unwrap(),
        untouched
    );
    assert!(!disjoint
        .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
        .exists());
    let decisions: serde_json::Value =
        serde_json::from_slice(&std::fs::read(target.join("config.json")).unwrap()).unwrap();
    let state: serde_json::Value = serde_json::from_slice(
        &std::fs::read(target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME))
            .unwrap(),
    )
    .unwrap();
    let calculated = configuration_identity("updated-command", &[]);
    assert_eq!(
        decisions
            .pointer("/tooling/configurationRef/calculatedId")
            .and_then(serde_json::Value::as_str),
        Some(calculated.as_str())
    );
    assert_eq!(
        state
            .pointer("/tooling/sourceReferences/0/configurationRef/calculatedId")
            .and_then(serde_json::Value::as_str),
        Some(calculated.as_str())
    );
    assert_eq!(decisions["unknown"], serde_json::Value::Null);
    assert_eq!(state["unknown"], serde_json::json!([null]));
    assert!(!target
        .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
        .exists());
    let publishers = source_transition_effect_counts().1;
    let expected = fixture.expectation();
    resume_identity_transition_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        &operation,
        &expected,
        &|| Ok(()),
    )
    .unwrap();
    assert_eq!(source_transition_effect_counts().1, publishers);
}

#[test]
fn single_editor_driver_nonempty_actual_pair_guard_cuts_resume_original_packets() {
    configure_source_factory_guard_cut(None);
    let (baseline, candidate, _, _) = DriverFixture::with_reference_targets();
    let revision = read_source_snapshot(&baseline.context, &baseline.source)
        .unwrap()
        .snapshot
        .revision;
    commit_source_candidate_internal(
        &baseline.settings,
        &baseline.context,
        &baseline.source,
        "baseline-packet",
        &revision,
        &candidate,
        &|| Ok(()),
    )
    .unwrap();
    let cuts = source_factory_guard_calls();
    assert!(cuts > 0);
    for cut in 1..=cuts {
        let (fixture, candidate, target, disjoint) = DriverFixture::with_reference_targets();
        let source_before = read_source_snapshot(&fixture.context, &fixture.source)
            .unwrap()
            .snapshot
            .revision;
        let unrelated = std::fs::read(disjoint.join("config.json")).unwrap();
        configure_source_factory_guard_cut(Some(cut));
        let failed = commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "request-packet-cut",
            &source_before,
            &candidate,
            &|| Ok(()),
        );
        assert!(
            !source_factory_guard_cut_armed(),
            "nonempty guard cut {cut} was not reached"
        );
        assert!(
            failed.is_err(),
            "nonempty guard refusal was swallowed at cut {cut}"
        );
        configure_source_factory_guard_cut(None);
        let original = read_own_source_journal(&fixture.context)
            .unwrap_or_else(|error| {
                let journal_path = fixture
                    ._root
                    .path()
                    .join("C")
                    .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
                let raw =
                    crate::path_identity::read_bounded_regular(&journal_path, 16 * 1024 * 1024)
                        .map(|_| ());
                panic!("nonempty guard cut {cut}: journal read {error:?}; bounded read {raw:?}");
            })
            .1;
        if let Some((operation, intent)) = original.intents.iter().next() {
            let operation = operation.clone();
            let intent = intent.clone();
            let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
            let expected = fixture.expectation();
            resume_identity_transition_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                &operation,
                &expected,
                &|| Ok(()),
            )
            .unwrap();
            let recovered = read_own_source_journal(&fixture.context).unwrap().1;
            assert_eq!(recovered.intents[&operation], intent);
            assert_eq!(
                recovered.inventories[&operation],
                original.inventories[&operation]
            );
            assert!(recovered.progress[&operation].receipt.is_some());
            if let Some(packet) = original.progress[&operation].packets.first() {
                assert_eq!(&recovered.progress[&operation].packets[0], packet);
            }
            assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
            assert!(!target
                .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
                .exists());
            let calls = source_transition_effect_counts().1;
            let expected = fixture.expectation();
            resume_identity_transition_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                &operation,
                &expected,
                &|| Ok(()),
            )
            .unwrap();
            assert_eq!(source_transition_effect_counts().1, calls);
        }
        assert_eq!(
            std::fs::read(disjoint.join("config.json")).unwrap(),
            unrelated
        );
    }
}

#[test]
fn single_editor_driver_failed_reload_retains_pending_and_activity_max_through_release() {
    let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
    let revision = read_source_snapshot(&fixture.context, &fixture.source)
        .unwrap()
        .snapshot
        .revision;
    let state_path = target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME);
    let activity = "2026-10-11T06:00:00Z";
    let failure = commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "request-reload",
        &revision,
        &candidate,
        &|| {
            // Real physical interleaving, outside PAIR and the validation callback.
            let mut state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
            state["tooling"]["lastAgentMessageAt"] = serde_json::json!(activity);
            std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
            Err(SourceCoordinationError::Io("reload interrupted".into()))
        },
    );
    assert!(matches!(
        failure,
        Err(SourceTransitionError::Interrupted { .. })
    ));
    let operation = fixture.operation();
    let before = read_own_source_journal(&fixture.context).unwrap().1;
    assert!(before.progress[&operation].refs_verified);
    assert!(!before.progress[&operation].reloaded);
    assert!(before.progress[&operation].receipt.is_none());
    assert!(target
        .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
        .exists());
    let expected = fixture.expectation();
    resume_identity_transition_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        &operation,
        &expected,
        &|| Ok(()),
    )
    .unwrap();
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(
        state
            .pointer("/tooling/lastAgentMessageAt")
            .and_then(serde_json::Value::as_str),
        Some(activity)
    );
    assert!(read_own_source_journal(&fixture.context)
        .unwrap()
        .1
        .progress[&operation]
        .receipt
        .is_some());
    assert!(!target
        .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
        .exists());
}

#[test]
fn single_editor_original_factory_real_matrix_default_group_and_history_no_default() {
    for matrix in ["N", "K"] {
        let mut fixture = DriverFixture::new();
        let config = fixture._root.path().join("C");
        let target = fixture.project.join(".ac").join(format!("_agent_{matrix}"));
        std::fs::create_dir(&target).unwrap();
        let mut registered = serde_json::json!({"agents":[
            {"id":"K","label":"Own","color":"#123456","command":"own-base"},
            {"id":"J","label":"Other","color":"#123456","command":"other-base"}],
            "codingAgentProfiles":{"schemaVersion":2,"profilesByAgent":{"K":{
                "A":{"command":"--alpha","env":{}},"B":{"command":"--beta","env":{}}},
                "J":{"A":{"command":"--other","env":{}}}},"defaultProfileByAgent":{},
                "identityByAgent":{"K":"legacy-k","J":"legacy-j"}}});
        registered["codingAgentProfiles"]["defaultProfileByAgent"][matrix] = serde_json::json!("B");
        let mut settings_value = serde_json::to_value(&fixture.settings).unwrap();
        settings_value["codingAgentProfiles"]["defaultProfileByAgent"][matrix] =
            serde_json::json!("B");
        fixture.settings = serde_json::from_value(settings_value.clone()).unwrap();
        std::fs::write(
            config.join("settings.json"),
            serde_json::to_vec(&settings_value).unwrap(),
        )
        .unwrap();
        std::fs::write(
            config.join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
            serde_json::to_vec(&registered).unwrap(),
        )
        .unwrap();
        std::fs::write(
            config.join(crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
            serde_json::to_vec(
                &serde_json::json!({"codingAgentProfiles":{"defaultProfileByAgent":{(matrix):"B"},
                "profilesByAgent":{"K":{"B":{"command":"overlay-beta","env":{}}}}}}),
            )
            .unwrap(),
        )
        .unwrap();
        fixture.context =
            SourceReadContext::from_settings(&fixture.settings, &config.join("settings.json"))
                .unwrap();
        fixture.source = list_active_sources(&fixture.context)
            .into_iter()
            .find(|source| source.kind == SourceKind::RegisteredInstance)
            .unwrap();
        let snapshot = read_source_snapshot(&fixture.context, &fixture.source).unwrap();
        let inputs = OriginInputs::capture(&fixture.context).unwrap();
        let origin = LegacyCompatibilityOrigin::classify(&inputs);
        let identity =
            IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
        let lease =
            SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
        let selection = ConfigurationRef {
            version: 1,
            source: fixture.source.clone(),
            calculated_id: configuration_identity("own-base", &[]),
            requested_profile: Some("B".into()),
            profile_calculated_id: Some(profile_identity(&snapshot.snapshot.profiles["K"]["B"])),
            legacy_entry_hint: Some("K".into()),
        };
        let mut candidate = registered.clone();
        candidate["codingAgentProfiles"]["profilesByAgent"]["K"]["B"]["command"] =
            serde_json::json!("new-beta");
        assert!(prepare_source_candidate(
            &snapshot,
            &snapshot.snapshot.revision,
            &candidate,
            Some(&origin)
        )
        .is_err());
        std::fs::write(target.join("config.json"), serde_json::to_vec(&serde_json::json!({"tooling":{
            "replicaSelectionDefault":{"codingAgentId":"K","profile":"B","configurationRef":selection.clone()}}})).unwrap()).unwrap();
        let capture = capture_source_inventory(
            &fixture.settings,
            &snapshot,
            &origin,
            lease.physical_source_key(),
        )
        .unwrap();
        prepare_source_candidate_with_original_links(
            &snapshot,
            &snapshot.snapshot.revision,
            &candidate,
            Some(&origin),
            Some(capture.original_links()),
        )
        .unwrap();
        let mut disjoint_j = registered.clone();
        disjoint_j["agents"][1]["command"] = serde_json::json!("other-new-base");
        prepare_source_candidate_with_original_links(
            &snapshot,
            &snapshot.snapshot.revision,
            &disjoint_j,
            Some(&origin),
            Some(capture.original_links()),
        )
        .unwrap();
        let mut direct_default = registered.clone();
        direct_default["codingAgentProfiles"]["defaultProfileByAgent"][matrix] =
            serde_json::json!("A");
        prepare_source_candidate_with_original_links(
            &snapshot,
            &snapshot.snapshot.revision,
            &direct_default,
            Some(&origin),
            Some(capture.original_links()),
        )
        .unwrap();

        let overlay_path =
            config.join(crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
        let overlay_before = std::fs::read(&overlay_path).unwrap();
        let mut drift: serde_json::Value = serde_json::from_slice(&overlay_before).unwrap();
        drift["logLevel"] = serde_json::json!("debug");
        std::fs::write(&overlay_path, serde_json::to_vec(&drift).unwrap()).unwrap();
        let changed_inputs = OriginInputs::capture(&fixture.context).unwrap();
        let changed_origin = LegacyCompatibilityOrigin::classify(&changed_inputs);
        assert!(prepare_source_candidate_with_original_links(
            &snapshot,
            &snapshot.snapshot.revision,
            &candidate,
            Some(&changed_origin),
            Some(capture.original_links())
        )
        .is_err());
        std::fs::write(&overlay_path, &overlay_before).unwrap();
        let mut missing_letter = selection.clone();
        missing_letter.requested_profile = None;
        missing_letter.profile_calculated_id = None;
        std::fs::write(
            target.join("config.json"),
            serde_json::to_vec(&serde_json::json!({"tooling":{
            "replicaSelectionDefault":{"codingAgentId":"K","configurationRef":missing_letter}}}))
            .unwrap(),
        )
        .unwrap();
        let missing = capture_source_inventory(
            &fixture.settings,
            &snapshot,
            &origin,
            lease.physical_source_key(),
        )
        .unwrap();
        assert!(prepare_source_candidate_with_original_links(
            &snapshot,
            &snapshot.snapshot.revision,
            &candidate,
            Some(&origin),
            Some(missing.original_links())
        )
        .is_err());
        let mut both: serde_json::Value = serde_json::from_slice(&overlay_before).unwrap();
        both["codingAgentProfiles"]["profilesByAgent"]["K"]["A"] =
            serde_json::json!({"command":"overlay-alpha","env":{}});
        std::fs::write(&overlay_path, serde_json::to_vec(&both).unwrap()).unwrap();
        let both_inputs = OriginInputs::capture(&fixture.context).unwrap();
        let both_origin = LegacyCompatibilityOrigin::classify(&both_inputs);
        let mut alpha = selection.clone();
        alpha.requested_profile = Some("A".into());
        alpha.profile_calculated_id = Some(profile_identity(&snapshot.snapshot.profiles["K"]["A"]));
        std::fs::write(target.join("config.json"), serde_json::to_vec(&serde_json::json!({"tooling":{
            "replicaSelectionDefault":{"codingAgentId":"K","profile":"A","configurationRef":alpha}}})).unwrap()).unwrap();
        let alpha_capture = capture_source_inventory(
            &fixture.settings,
            &snapshot,
            &both_origin,
            lease.physical_source_key(),
        )
        .unwrap();
        assert!(
            prepare_source_candidate_with_original_links(
                &snapshot,
                &snapshot.snapshot.revision,
                &candidate,
                Some(&both_origin),
                Some(alpha_capture.original_links())
            )
            .is_err(),
            "A proof covered changed non-disjoint B"
        );
        std::fs::write(&overlay_path, &overlay_before).unwrap();
        std::fs::write(target.join("config.json"), b"{}").unwrap();
        std::fs::write(target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
            serde_json::to_vec(&serde_json::json!({"tooling":{"sourceReferences":[{"configurationRef":selection}]}})).unwrap()).unwrap();
        let history = capture_source_inventory(
            &fixture.settings,
            &snapshot,
            &origin,
            lease.physical_source_key(),
        )
        .unwrap();
        assert!(prepare_source_candidate_with_original_links(
            &snapshot,
            &snapshot.snapshot.revision,
            &candidate,
            Some(&origin),
            Some(history.original_links())
        )
        .is_err());
        std::fs::write(
            target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
            b"{}",
        )
        .unwrap();
        let replica = fixture
            .project
            .join(".ac/wg-7-proof")
            .join(format!("__agent_{matrix}"));
        std::fs::create_dir_all(&replica).unwrap();
        let replica_selection = ConfigurationRef {
            version: 1,
            source: fixture.source.clone(),
            calculated_id: configuration_identity("own-base", &[]),
            requested_profile: Some("B".into()),
            profile_calculated_id: Some(profile_identity(&snapshot.snapshot.profiles["K"]["B"])),
            legacy_entry_hint: Some("K".into()),
        };
        std::fs::write(
            replica.join("config.json"),
            serde_json::to_vec(&serde_json::json!({
            "identity":format!("../../_agent_{matrix}"),"tooling":{"replicaSelectionDefault":{
                "codingAgentId":"K","profile":"B","configurationRef":replica_selection}}}))
            .unwrap(),
        )
        .unwrap();
        let replica_capture = capture_source_inventory(
            &fixture.settings,
            &snapshot,
            &origin,
            lease.physical_source_key(),
        )
        .unwrap();
        assert!(
            prepare_source_candidate_with_original_links(
                &snapshot,
                &snapshot.snapshot.revision,
                &candidate,
                Some(&origin),
                Some(replica_capture.original_links())
            )
            .is_err(),
            "replica minted Matrix Default proof"
        );
    }
}

impl DriverFixture {
    fn resume_at(
        &self,
        context: &SourceReadContext,
        operation: &str,
    ) -> Result<IdentityTransitionRead, SourceTransitionError> {
        let expected = {
            let identity = IdentityWriteLease::acquire(context, std::time::Duration::ZERO).unwrap();
            let lease = SourceWriteContext::acquire(context, &self.source, &identity).unwrap();
            SourceResumeExpectation {
                source: lease.source_revision().unwrap(),
                journal: read_own_source_journal(context).unwrap().0,
            }
        };
        resume_identity_transition_internal(
            &self.settings,
            context,
            &self.source,
            operation,
            &expected,
            &|| Ok(()),
        )
    }
}

#[test]
fn single_editor_crash_resume_original() {
    let cuts = [
        ("beforeIntent", 1),
        ("intentInventory", 1),
        ("beforeFence", 1),
        ("packet", 1),
        ("packet", 2),
        ("allPackets", 1),
        ("prepared", 1),
        ("sourcePublished", 1),
        ("ref", 1),
        ("sourceAfter", 1),
        ("refsVerified", 1),
        ("reloaded", 1),
        ("receipt", 1),
        ("release", 1),
    ];
    let mut reached = 0;
    for cut in cuts {
        assert_source_durable_cut(cut);
        reached += 1;
    }
    assert_eq!(reached, cuts.len(), "not all durable cuts completed");
}

fn assert_source_durable_cut(cut: (&'static str, usize)) {
    let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
    let second = target.parent().unwrap().join("_agent_second");
    std::fs::create_dir(&second).unwrap();
    for name in [
        "config.json",
        crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME,
    ] {
        std::fs::copy(target.join(name), second.join(name)).unwrap();
    }
    let revision = read_source_snapshot(&fixture.context, &fixture.source)
        .unwrap()
        .snapshot
        .revision;
    configure_source_durable_cut(Some(cut));
    let result = commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "durable-boundary",
        &revision,
        &candidate,
        &|| Ok(()),
    );
    assert!(result.is_err(), "unreached cut: {cut:?}");
    SOURCE_DURABLE_CUT
        .with(|state| assert!(state.borrow().0.is_none(), "cut was not reached: {cut:?}"));
    configure_source_durable_cut(None);
    let original = read_own_source_journal(&fixture.context).unwrap().1;
    if cut.0 == "beforeIntent" {
        assert!(original.intents.is_empty());
        assert!(!matches!(
            result,
            Err(SourceTransitionError::Interrupted { .. })
        ));
        assert_eq!(
            read_source_snapshot(&fixture.context, &fixture.source)
                .unwrap()
                .snapshot
                .revision,
            revision
        );
        let fresh_revision = read_source_snapshot(&fixture.context, &fixture.source)
            .unwrap()
            .snapshot
            .revision;
        commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "new-after-before-intent",
            &fresh_revision,
            &candidate,
            &|| Ok(()),
        )
        .unwrap();
        return;
    }
    let operation = fixture.operation();
    assert!(
        matches!(result, Err(SourceTransitionError::Interrupted { operation_id, .. }) if operation_id == operation)
    );
    assert_eq!(original.inventories[&operation].targets.len(), 2);
    let observation =
        get_identity_transition(&fixture.context, &fixture.source, &operation).unwrap();
    assert_ne!(observation.state, IdentityTransitionReadState::NotFound);
    assert_original_durable_packets(&[target.as_path(), second.as_path()], &operation);
    if matches!(
        cut.0,
        "intentInventory" | "beforeFence" | "packet" | "allPackets" | "prepared"
    ) {
        assert_eq!(
            read_source_snapshot(&fixture.context, &fixture.source)
                .unwrap()
                .snapshot
                .revision,
            revision
        );
    }
    let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
    fixture.resume_at(&fixture.context, &operation).unwrap();
    let recovered = read_own_source_journal(&fixture.context).unwrap().1;
    assert_eq!(recovered.intents[&operation], original.intents[&operation]);
    assert_eq!(
        recovered.inventories[&operation],
        original.inventories[&operation]
    );
    assert_eq!(recovered.progress[&operation].packets.len(), 2);
    assert!(recovered.progress[&operation].receipt.is_some());
    assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
    for root in [&target, &second] {
        assert!(!root
            .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
            .exists());
    }
}

fn assert_original_durable_packets(roots: &[&std::path::Path], operation: &str) {
    for root in roots {
        let reservation =
            root.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME);
        if reservation.exists() {
            let packet: serde_json::Value =
                serde_json::from_slice(&std::fs::read(reservation).unwrap()).unwrap();
            assert_eq!(packet["planImages"]["stages"].as_array().unwrap().len(), 5);
            assert_eq!(packet["protectedImages"].as_array().unwrap().len(), 5);
            assert!(packet.get("timestampFloor").is_some());
            assert_eq!(packet["operationId"].as_str(), Some(operation));
        }
    }
}

#[test]
fn single_editor_driver_metadata_only_keeps_refs_bytes_and_runs_full_barrier() {
    let (fixture, _, target, _) = DriverFixture::with_reference_targets();
    let source_path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
    let mut candidate: serde_json::Value =
        serde_json::from_slice(&std::fs::read(source_path).unwrap()).unwrap();
    candidate["agents"][0]["name"] = serde_json::json!("metadata renamed");
    let decisions = std::fs::read(target.join("config.json")).unwrap();
    let state =
        std::fs::read(target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME))
            .unwrap();
    let before = read_source_snapshot(&fixture.context, &fixture.source)
        .unwrap()
        .snapshot
        .revision;
    let reloads = std::cell::Cell::new(0);
    commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "metadata-only",
        &before,
        &candidate,
        &|| {
            reloads.set(reloads.get() + 1);
            Ok(())
        },
    )
    .unwrap();
    let journal = read_own_source_journal(&fixture.context).unwrap().1;
    let operation = fixture.operation();
    assert!(journal.inventories[&operation].targets.is_empty());
    assert!(journal.progress[&operation].packets_prepared);
    assert!(journal.progress[&operation].packets.is_empty());
    assert!(journal.progress[&operation].receipt.is_some());
    assert_eq!(reloads.get(), 1);
    assert_eq!(
        std::fs::read(target.join("config.json")).unwrap(),
        decisions
    );
    assert_eq!(
        std::fs::read(target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME))
            .unwrap(),
        state
    );
}

#[test]
fn single_editor_all_authority_context_source_target_cas_failures() {
    for drift in [
        "protected",
        "packet",
        "history-index",
        "source-Z",
        "settings-binding",
        "overlay-binding",
    ] {
        let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
        let bound_settings_path = fixture
            ._root
            .path()
            .join("C")
            .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
        let settings_before = if drift == "settings-binding" {
            let before = read_identity_reference_physical(&bound_settings_path).unwrap();
            assert_eq!(
                before,
                IdentityReferencePhysicalState::Absent,
                "bound settings must be physically absent before capture"
            );
            Some(before)
        } else {
            None
        };
        let revision = read_source_snapshot(&fixture.context, &fixture.source)
            .unwrap()
            .snapshot
            .revision;
        configure_source_durable_cut(Some(("packet", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "authority-drift",
            &revision,
            &candidate,
            &|| Ok(())
        )
        .is_err());
        configure_source_durable_cut(None);
        let operation = fixture.operation();
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let path = match drift {
            "protected" => target.join("config.json"),
            "packet" => {
                target.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
            }
            "history-index" => {
                target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
            }
            "source-Z" => {
                source_transition_test_locator(&fixture.context, &fixture.source).unwrap()
            }
            "settings-binding" => bound_settings_path,
            _ => fixture
                ._root
                .path()
                .join("C")
                .join(crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
        };
        let changed = match drift {
            "packet" => b"{corrupt packet".to_vec(),
            "history-index" => {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                value["tooling"]["sourceReferences"]
                    .as_array_mut()
                    .unwrap()
                    .insert(0, serde_json::json!({"unknown":"new slot"}));
                serde_json::to_vec(&value).unwrap()
            }
            "protected" => {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                value["unknown"] = serde_json::json!("external protected drift");
                serde_json::to_vec(&value).unwrap()
            }
            "source-Z" => {
                serde_json::to_vec(&serde_json::json!({"schemaVersion":2,"agents":[]})).unwrap()
            }
            _ => br#"{"logLevel":"debug"}"#.to_vec(),
        };
        if let Some(before) = &settings_before {
            assert_eq!(
                read_identity_reference_physical(&path).unwrap(),
                *before,
                "bound settings changed before external drift"
            );
            assert_ne!(
                IdentityReferencePhysicalState::from_bytes(changed.clone()),
                *before,
                "settings drift must change physical state"
            );
        }
        std::fs::write(&path, &changed).unwrap();
        if settings_before.is_some() {
            assert_eq!(
                read_identity_reference_physical(&path).unwrap(),
                IdentityReferencePhysicalState::from_bytes(changed.clone()),
                "bound settings drift was not persisted exactly"
            );
        }
        let publishers = source_transition_effect_counts().1;
        assert!(
            fixture.resume_at(&fixture.context, &operation).is_err(),
            "drift accepted: {drift}"
        );
        assert_eq!(source_transition_effect_counts().1, publishers);
        assert_eq!(std::fs::read(path).unwrap(), changed);
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            original
        );
    }
}

#[test]
fn single_editor_resume_each_stale_physical_cas_rejects_before_effect() {
    for stale in ["source", "journal"] {
        let fixture = DriverFixture::new();
        configure_source_durable_cut(Some(("intentInventory", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "prefence-CAS",
            &fixture.initial_revision,
            &DriverFixture::candidate(),
            &|| Ok(())
        )
        .is_err());
        configure_source_durable_cut(None);
        let operation = fixture.operation();
        let journal = read_own_source_journal(&fixture.context).unwrap().1;
        let uuid_count = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
        assert!(matches!(
            commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "different-prefence-request",
                &fixture.initial_revision,
                &DriverFixture::candidate(),
                &|| Ok(())
            ),
            Err(SourceTransitionError::Interrupted { .. })
        ));
        assert_eq!(
            SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get),
            uuid_count
        );
        let mut expected = fixture.expectation();
        let wrong = SourceRevision::Bytes {
            sha256: "0".repeat(64),
        };
        match stale {
            "source" => expected.source = wrong,
            "journal" => expected.journal = wrong,
            _ => expected.journal = wrong,
        }
        let publishers = source_transition_effect_counts().1;
        assert!(matches!(
            resume_identity_transition_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                &operation,
                &expected,
                &|| Ok(())
            ),
            Err(SourceTransitionError::Coordination(
                SourceCoordinationError::Stale
            ))
        ));
        assert_eq!(source_transition_effect_counts().1, publishers);
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            journal
        );
    }
}

#[test]
fn single_editor_registered_source_after_restores_original_groups_two_targets() {
    for matrix in ["N", "K"] {
        let mut fixture = DriverFixture::new();
        let config = fixture._root.path().join("C");
        let target = fixture.project.join(".ac").join(format!("_agent_{matrix}"));
        std::fs::create_dir(&target).unwrap();
        let mut registered = serde_json::json!({"agents":[
            {"id":"K","label":"Own","color":"#123456","command":"own-base"},
            {"id":"J","label":"Other","color":"#123456","command":"other-base"}],
            "codingAgentProfiles":{"schemaVersion":2,"profilesByAgent":{"K":{
                "A":{"command":"--alpha","env":{}},"B":{"command":"--beta","env":{}}},
                "J":{"A":{"command":"--other","env":{}}}},"defaultProfileByAgent":{},
                "identityByAgent":{"K":"legacy-k","J":"legacy-j"}}});
        registered["codingAgentProfiles"]["defaultProfileByAgent"][matrix] = serde_json::json!("B");
        let mut settings_value = serde_json::to_value(&fixture.settings).unwrap();
        settings_value["codingAgentProfiles"]["defaultProfileByAgent"][matrix] =
            serde_json::json!("B");
        fixture.settings = serde_json::from_value(settings_value.clone()).unwrap();
        std::fs::write(
            config.join("settings.json"),
            serde_json::to_vec(&settings_value).unwrap(),
        )
        .unwrap();
        std::fs::write(
            config.join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
            serde_json::to_vec(&registered).unwrap(),
        )
        .unwrap();
        std::fs::write(
            config.join(crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
            serde_json::to_vec(
                &serde_json::json!({"codingAgentProfiles":{"defaultProfileByAgent":{(matrix):"B"},
                "profilesByAgent":{"K":{"B":{"command":"overlay-beta","env":{}}}}}}),
            )
            .unwrap(),
        )
        .unwrap();
        fixture.context =
            SourceReadContext::from_settings(&fixture.settings, &config.join("settings.json"))
                .unwrap();
        fixture.source = list_active_sources(&fixture.context)
            .into_iter()
            .find(|source| source.kind == SourceKind::RegisteredInstance)
            .unwrap();
        let snapshot = read_source_snapshot(&fixture.context, &fixture.source).unwrap();
        let identity =
            IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
        let lease =
            SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
        let selection = ConfigurationRef {
            version: 1,
            source: fixture.source.clone(),
            calculated_id: configuration_identity("own-base", &[]),
            requested_profile: Some("B".into()),
            profile_calculated_id: Some(profile_identity(&snapshot.snapshot.profiles["K"]["B"])),
            legacy_entry_hint: Some("K".into()),
        };
        let mut candidate = registered;
        candidate["codingAgentProfiles"]["profilesByAgent"]["K"]["B"]["command"] =
            serde_json::json!("new-beta");
        std::fs::write(target.join("config.json"), serde_json::to_vec(&serde_json::json!({"tooling":{
            "replicaSelectionDefault":{"codingAgentId":"K","profile":"B","configurationRef":selection.clone()}}})).unwrap()).unwrap();

        let second = fixture.project.join(".ac/_agent_second");
        std::fs::create_dir(&second).unwrap();
        std::fs::write(
            second.join("config.json"),
            serde_json::to_vec(&serde_json::json!({"tooling":{
            "codingAgentId":"K","configurationRef":selection},"unknown":{"keep":true}}))
            .unwrap(),
        )
        .unwrap();
        drop(lease);
        drop(identity);
        configure_source_durable_cut(Some(("sourceAfter", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "registered-source-after",
            &snapshot.snapshot.revision,
            &candidate,
            &|| Ok(())
        )
        .is_err());
        configure_source_durable_cut(None);
        let operation = fixture.operation();
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        assert_eq!(original.inventories[&operation].targets.len(), 2);
        assert_ne!(
            read_source_snapshot(&fixture.context, &fixture.source)
                .unwrap()
                .snapshot
                .revision,
            snapshot.snapshot.revision
        );
        let publishers = source_transition_effect_counts().1;
        let source_publications = source_transition_publication_counts()[2];
        fixture.resume_at(&fixture.context, &operation).unwrap();
        assert_eq!(
            source_transition_publication_counts()[2],
            source_publications
        );
        let recovered = read_own_source_journal(&fixture.context).unwrap().1;
        assert_eq!(recovered.intents[&operation], original.intents[&operation]);
        assert_eq!(
            recovered.inventories[&operation],
            original.inventories[&operation]
        );
        assert!(recovered.progress[&operation].receipt.is_some());
        for root in [&target, &second] {
            assert!(!root
                .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
                .exists());
        }
        let calls = source_transition_effect_counts().1;
        assert!(calls > publishers); // Progress/ack/receipt have real metadata publications.
        fixture.resume_at(&fixture.context, &operation).unwrap();
        assert_eq!(source_transition_effect_counts().1, calls);
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(second.join("config.json")).unwrap()).unwrap();
        assert_eq!(value["unknown"]["keep"], serde_json::json!(true));
    }
}

#[test]
fn single_editor_missing_corrupt_original_evidence_never_reconstructs_candidate() {
    for corrupt in [false, true] {
        let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
        let before = read_source_snapshot(&fixture.context, &fixture.source)
            .unwrap()
            .snapshot
            .revision;
        configure_source_durable_cut(Some(("intentInventory", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "missing-original",
            &before,
            &candidate,
            &|| Ok(())
        )
        .is_err());
        configure_source_durable_cut(None);
        let operation = fixture.operation();
        let path = fixture
            ._root
            .path()
            .join("C")
            .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
        if corrupt {
            std::fs::write(&path, b"{corrupt original").unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        let identity =
            IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
        let lease =
            SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
        let local = if corrupt {
            SourceRevision::Bytes {
                sha256: "0".repeat(64),
            }
        } else {
            SourceRevision::Absent {}
        };
        let expected = SourceResumeExpectation {
            source: lease.source_revision().unwrap(),
            journal: local,
        };
        drop(lease);
        drop(identity);
        let publications = source_transition_effect_counts().1;
        assert!(resume_identity_transition_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            &operation,
            &expected,
            &|| Ok(())
        )
        .is_err());
        assert_eq!(source_transition_effect_counts().1, publications);
        assert_eq!(
            read_source_snapshot(&fixture.context, &fixture.source)
                .unwrap()
                .snapshot
                .revision,
            before
        );
        assert!(!target
            .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
            .exists());
        if corrupt {
            assert_eq!(std::fs::read(path).unwrap(), b"{corrupt original");
        } else {
            assert!(!path.exists());
        }
    }
}

#[test]
fn single_editor_thread_oracles_require_worker_owned_observation() {
    let parent = source_transition_effect_counts();
    let worker = std::thread::spawn(|| {
        let fixture = DriverFixture::new();
        let before = source_transition_effect_counts();
        let owning_thread = std::thread::current().id();
        commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "worker-local",
            &fixture.initial_revision,
            &DriverFixture::candidate(),
            &|| {
                assert_eq!(std::thread::current().id(), owning_thread);
                Ok(())
            },
        )
        .unwrap();
        let after = source_transition_effect_counts();
        (before, after)
    })
    .join()
    .unwrap();
    assert_eq!(source_transition_effect_counts(), parent);
    assert!(worker.1 .0[0] > worker.0 .0[0]);
    assert_eq!(worker.1 .0[1], worker.0 .0[1], "no source lock acquisition");
    assert!(worker.1 .1 > worker.0 .1);
}

#[test]
fn single_editor_original_slot_parser_rejects_omitted_extra_and_changed_saved_slots() {
    let (fixture, candidate, _, _) = DriverFixture::with_reference_targets();
    let revision = fixture.expectation().source;
    configure_source_durable_cut(Some(("intentInventory", 1)));
    assert!(commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "slot-completeness",
        &revision,
        &candidate,
        &|| Ok(())
    )
    .is_err());
    configure_source_durable_cut(None);
    let operation = fixture.operation();
    let inventory = read_own_source_journal(&fixture.context)
        .unwrap()
        .1
        .inventories[&operation]
        .clone();
    inventory
        .verify_original_slots(&source_original_inventory_slots)
        .unwrap();
    for corruption in ["omitted", "extra", "changed"] {
        let mut value = serde_json::to_value(&inventory).unwrap();
        let target = value["originalEvidence"]["discovered"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|evidence| !evidence["slots"].as_array().unwrap().is_empty())
            .unwrap();
        let slots = target["slots"].as_array_mut().unwrap();
        match corruption {
            "omitted" => {
                slots.remove(0);
            }
            "extra" => {
                slots.push(slots[0].clone());
            }
            "changed" => {
                slots[0][1]["calculatedId"] = serde_json::json!("changed-id");
            }
            _ => unreachable!(),
        }
        let corrupt: SourceLocalInventory = serde_json::from_value(value).unwrap();
        assert!(corrupt
            .verify_original_slots(&source_original_inventory_slots)
            .is_err());
    }
}

#[test]
fn single_editor_production_save_and_resume_effect_guard_reject_actual_external_drift() {
    for uncertain_target in [false, true] {
        let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
        let path = target.join("config.json");
        let before = std::fs::read(&path).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&before).unwrap();
        value["unknown"] = serde_json::json!("actual external fence-time drift");
        let changed = if uncertain_target {
            b"not JSON".to_vec()
        } else {
            serde_json::to_vec(&value).unwrap()
        };
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let worker_barrier = barrier.clone();
        let worker = std::thread::spawn(move || {
            let revision = fixture.expectation().source;
            SOURCE_FENCE_EFFECT_BARRIER.with(|slot| *slot.borrow_mut() = Some(worker_barrier));
            let result = commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "real-fence-drift",
                &revision,
                &candidate,
                &|| Ok(()),
            );
            let error = result.unwrap_err().to_string();
            if uncertain_target {
                assert!(error.contains("sourceIdentityUncertain"), "{error}");
            }
            (fixture, candidate)
        });
        barrier.wait();
        std::fs::write(&path, &changed).unwrap();
        barrier.wait();
        let (fixture, _) = worker.join().unwrap();
        let operation = fixture.operation();
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let assert_no_fence = |fixture: &DriverFixture| {
            let identity =
                IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
            let lease =
                SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
            assert_eq!(
                lease.source_revision().unwrap(),
                original.intents[&operation].expected_revision
            );
            assert!(
                !read_own_source_journal(&fixture.context)
                    .unwrap()
                    .1
                    .progress[&operation]
                    .packets_prepared
            );
        };
        assert_no_fence(&fixture);
        assert_eq!(std::fs::read(&path).unwrap(), changed);
        std::fs::write(&path, &before).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let worker_barrier = barrier.clone();
        let resume_operation = operation.clone();
        let worker = std::thread::spawn(move || {
            SOURCE_FENCE_EFFECT_BARRIER.with(|slot| *slot.borrow_mut() = Some(worker_barrier));
            let error = fixture
                .resume_at(&fixture.context, &resume_operation)
                .unwrap_err()
                .to_string();
            if uncertain_target {
                assert!(error.contains("sourceIdentityUncertain"), "{error}");
            }
            fixture
        });
        barrier.wait();
        std::fs::write(&path, &changed).unwrap();
        barrier.wait();
        let fixture = worker.join().unwrap();
        assert_no_fence(&fixture);
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            original
        );
        assert_eq!(std::fs::read(&path).unwrap(), changed);
        std::fs::write(&path, before).unwrap();
        fixture.resume_at(&fixture.context, &operation).unwrap();
        assert!(read_own_source_journal(&fixture.context)
            .unwrap()
            .1
            .progress[&operation]
            .receipt
            .is_some());
    }
}

#[test]
fn single_editor_production_save_resume_before_fence_drift_preserves_original() {
    let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
    let revision = fixture.expectation().source;
    configure_source_durable_cut(Some(("beforeFence", 1)));
    assert!(commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "production-before-fence",
        &revision,
        &candidate,
        &|| Ok(())
    )
    .is_err());
    SOURCE_DURABLE_CUT.with(|state| assert!(state.borrow().0.is_none()));
    configure_source_durable_cut(None);
    let operation = fixture.operation();
    let original = read_own_source_journal(&fixture.context).unwrap().1;
    let path = target.join("config.json");
    let before = std::fs::read(&path).unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&before).unwrap();
    changed["unknown"] = serde_json::json!("drift after actual Save initial fence check");
    let bytes = serde_json::to_vec(&changed).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let publishers = source_transition_effect_counts().1;
    assert!(fixture.resume_at(&fixture.context, &operation).is_err());
    assert_eq!(source_transition_effect_counts().1, publishers);
    assert_eq!(
        read_own_source_journal(&fixture.context).unwrap().1,
        original
    );
    std::fs::write(&path, &before).unwrap();
    // Actual Resume reaches its own production beforeFence boundary.
    configure_source_durable_cut(Some(("beforeFence", 1)));
    assert!(fixture.resume_at(&fixture.context, &operation).is_err());
    SOURCE_DURABLE_CUT.with(|state| assert!(state.borrow().0.is_none()));
    configure_source_durable_cut(None);
    assert_eq!(
        read_own_source_journal(&fixture.context).unwrap().1,
        original
    );
    fixture.resume_at(&fixture.context, &operation).unwrap();
    assert!(read_own_source_journal(&fixture.context)
        .unwrap()
        .1
        .progress[&operation]
        .receipt
        .is_some());
}

#[test]
fn single_editor_backend_primary_context_drift_preserves_original_no_next_effect() {
    let mut fixture = DriverFixture::new();
    configure_source_durable_cut(Some(("intentInventory", 1)));
    assert!(commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "primary-context",
        &fixture.initial_revision,
        &DriverFixture::candidate(),
        &|| Ok(())
    )
    .is_err());
    configure_source_durable_cut(None);
    let operation = fixture.operation();
    let original = read_own_source_journal(&fixture.context).unwrap().1;
    let expected = fixture.expectation();
    let other = fixture._root.path().join("other-project");
    std::fs::create_dir_all(other.join(".ac/coding-agents")).unwrap();
    fixture
        .settings
        .project_paths
        .insert(0, other.to_string_lossy().into_owned());
    let changed = SourceReadContext::from_settings(
        &fixture.settings,
        &fixture._root.path().join("C/settings.json"),
    )
    .unwrap();
    let publishers = source_transition_effect_counts().1;
    assert!(resume_identity_transition_internal(
        &fixture.settings,
        &changed,
        &fixture.source,
        &operation,
        &expected,
        &|| Ok(())
    )
    .is_err());
    assert_eq!(source_transition_effect_counts().1, publishers);
    assert_eq!(
        read_own_source_journal(&fixture.context).unwrap().1,
        original
    );
}

fn existing_prepared(cut: &'static str) -> (DriverFixture, String, std::path::PathBuf) {
    let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
    let before = fixture.expectation().source;
    configure_source_durable_cut(Some((cut, 1)));
    let result = commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "existing-original",
        &before,
        &candidate,
        &|| Ok(()),
    );
    assert!(
        matches!(result, Err(SourceTransitionError::Interrupted { .. })),
        "{result:?}"
    );
    SOURCE_DURABLE_CUT.with(|state| assert!(state.borrow().0.is_none()));
    configure_source_durable_cut(None);
    let operation = fixture.operation();
    assert!(
        read_own_source_journal(&fixture.context).unwrap().1.intents[&operation]
            .before_bytes
            .is_some()
    );
    (fixture, operation, target)
}

#[test]
fn single_editor_existing_after_equivalent_resume() {
    for distinct_object in [false, true] {
        let (fixture, operation, target) = existing_prepared("sourceAfter");
        let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let after = original.intents[&operation].serialized_after.clone();
        let parent = crate::path_identity::retain_directory(path.parent().unwrap()).unwrap();
        let published = parent.verify_regular_file(&path).unwrap();
        if distinct_object {
            std::fs::rename(&path, path.with_extension("original-diagnostic")).unwrap();
            std::fs::write(&path, &after).unwrap();
            assert!(!crate::path_identity::same_object(
                &published,
                &parent.verify_regular_file(&path).unwrap()
            ));
        }
        let before = source_transition_publication_counts()[2];
        let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
        let fresh = SourceReadContext::from_settings(
            &fixture.settings,
            &fixture._root.path().join("C/settings.json"),
        )
        .unwrap();
        assert_eq!(
            get_identity_transition(&fresh, &fixture.source, &operation)
                .unwrap()
                .state,
            IdentityTransitionReadState::Prepared
        );
        assert_eq!(
            read_own_source_journal(&fresh).unwrap().1,
            original,
            "status never consolidates"
        );
        fixture.resume_at(&fresh, &operation).unwrap();
        let complete = read_own_source_journal(&fresh).unwrap().1;
        assert_eq!(complete.intents[&operation], original.intents[&operation]);
        assert_eq!(
            complete.inventories[&operation],
            original.inventories[&operation]
        );
        assert_eq!(
            complete.progress[&operation].packets,
            original.progress[&operation].packets
        );
        let progress = &complete.progress[&operation];
        assert!(
            progress.source_published
                && progress.refs_verified
                && progress.reloaded
                && progress.receipt.is_some()
        );
        assert!(
            progress.source_publication.is_none(),
            "existing equivalence has no ownership witness"
        );
        assert_eq!(
            source_transition_publication_counts()[2],
            before,
            "zero second source publication"
        );
        assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
        assert_eq!(std::fs::read(path).unwrap(), after);
        assert!(!target
            .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
            .exists());
    }
}

#[test]
fn single_editor_existing_after_divergent_refusal() {
    for drift in ["bytes", "context", "parent", "irregular"] {
        let (fixture, operation, target) = existing_prepared("sourceAfter");
        let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        let before = read_own_source_journal(&fixture.context).unwrap().1;
        let decisions = std::fs::read(target.join("config.json")).unwrap();
        let mut context = fixture.context.clone();
        match drift {
            "bytes" => std::fs::write(&path, b"{\"external\":\"Z\"}").unwrap(),
            "context" => {
                let other = fixture._root.path().join("changed-project");
                std::fs::create_dir_all(other.join(".ac/coding-agents")).unwrap();
                let mut settings = fixture.settings.clone();
                settings
                    .project_paths
                    .insert(0, other.to_string_lossy().into_owned());
                context = SourceReadContext::from_settings(
                    &settings,
                    &fixture._root.path().join("C/settings.json"),
                )
                .unwrap();
            }
            "parent" => {
                let parent = path.parent().unwrap();
                std::fs::rename(parent, parent.with_extension("original-parent-diagnostic"))
                    .unwrap();
                std::fs::create_dir(parent).unwrap();
                std::fs::write(&path, &before.intents[&operation].serialized_after).unwrap();
            }
            _ => {
                std::fs::remove_file(&path).unwrap();
                std::fs::create_dir(&path).unwrap();
            }
        }
        let source = std::fs::read(&path)
            .map(|bytes| source_transition_digest_bytes(&bytes))
            .unwrap_or_else(|_| {
                source_transition_digest_bytes(&before.intents[&operation].serialized_after)
            });
        let expected = SourceResumeExpectation {
            source,
            journal: read_own_source_journal(&context).unwrap().0,
        };
        let reloads = std::cell::Cell::new(0);
        let effects = source_transition_publication_counts();
        assert!(resume_identity_transition_internal(
            &fixture.settings,
            &context,
            &fixture.source,
            &operation,
            &expected,
            &|| {
                reloads.set(reloads.get() + 1);
                Ok(())
            }
        )
        .is_err());
        assert_eq!(source_transition_publication_counts(), effects);
        assert_eq!(reloads.get(), 0);
        assert_eq!(read_own_source_journal(&fixture.context).unwrap().1, before);
        assert_eq!(
            std::fs::read(target.join("config.json")).unwrap(),
            decisions
        );
    }
}

#[test]
fn single_editor_source_changed_refusal() {
    for cut in [
        "intentInventory",
        "prepared",
        "sourcePublished",
        "ref",
        "reloaded",
        "receipt",
    ] {
        let (fixture, operation, target) = existing_prepared(cut);
        let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let before_target = std::fs::read(target.join("config.json")).unwrap();
        let bytes = if original.progress[&operation].source_published {
            original.intents[&operation].before_bytes.clone().unwrap()
        } else {
            b"{\"external\":\"Z\"}".to_vec()
        };
        std::fs::write(&path, bytes).unwrap();
        let effects = source_transition_publication_counts();
        assert!(fixture.resume_at(&fixture.context, &operation).is_err());
        assert_eq!(
            source_transition_publication_counts(),
            effects,
            "never republish after source_published"
        );
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            original
        );
        assert_eq!(
            std::fs::read(target.join("config.json")).unwrap(),
            before_target
        );
    }
}

#[test]
fn single_editor_status_compatibility() {
    let fixture = DriverFixture::new();
    let private = fixture
        ._root
        .path()
        .join("C")
        .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
    for bytes in [
        b"{broken".as_slice(),
        b"{}",
        b"{\"schemaVersion\":1,\"intents\":{},\"inventories\":{},\"progress\":{}}",
        b"{\"schemaVersion\":3,\"intents\":{},\"inventories\":{},\"progress\":{}}",
    ] {
        std::fs::write(&private, bytes).unwrap();
        let effects = source_transition_effect_counts();
        assert!(get_identity_transition(&fixture.context, &fixture.source, "unknown").is_err());
        assert_eq!(std::fs::read(&private).unwrap(), bytes);
        assert_eq!(source_transition_effect_counts(), effects);
    }
    std::fs::remove_file(&private).unwrap();
    let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
    let legacy = path
        .parent()
        .unwrap()
        .join(crate::config::instance_artifacts::SOURCE_CATALOG_PERSONAL_LEDGER_NAME);
    std::fs::write(&legacy, b"legacy-relevant-diagnostic").unwrap();
    let effects = source_transition_effect_counts();
    assert!(get_identity_transition(&fixture.context, &fixture.source, "unknown").is_err());
    assert_eq!(
        std::fs::read(&legacy).unwrap(),
        b"legacy-relevant-diagnostic"
    );
    assert_eq!(source_transition_effect_counts(), effects);
    std::fs::remove_file(&legacy).unwrap();
    let absent = get_identity_transition(&fixture.context, &fixture.source, "unknown").unwrap();
    assert_eq!(absent.state, IdentityTransitionReadState::NotFound);
    assert_eq!(absent.status.journal_revision, SourceRevision::Absent {});
    let other_source = crate::config::settings::SourceRef {
        family: crate::config::settings::SourceFamily::Agents,
        role: crate::config::settings::SourceRole::Catalog,
        scope: crate::config::settings::SourceScope::Project,
        kind: crate::config::settings::SourceKind::CatalogProject,
        context_root: std::fs::canonicalize(&fixture.project)
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    };
    let unrelated_path = source_transition_test_locator(&fixture.context, &other_source).unwrap();
    // If both kinds share a parent, the source-specific legacy leaf still isolates diagnosis.
    let unrelated = unrelated_path
        .parent()
        .unwrap()
        .join(crate::config::instance_artifacts::SOURCE_CATALOG_PROJECT_LEDGER_NAME);
    std::fs::write(&unrelated, b"unrelated-legacy").unwrap();
    assert_eq!(
        get_identity_transition(&fixture.context, &fixture.source, "unknown")
            .unwrap()
            .state,
        IdentityTransitionReadState::NotFound
    );
    assert_eq!(std::fs::read(unrelated).unwrap(), b"unrelated-legacy");
}

#[test]
fn single_editor_original_local_journal_immutable_and_missing_inventory_refusal() {
    let (fixture, operation, target) = existing_prepared("sourcePublished");
    let identity =
        IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
    let lease = SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
    let (revision, original) = read_own_source_journal(&fixture.context).unwrap();
    for field in [
        "intent",
        "inventory",
        "packet",
        "prepared",
        "published",
        "receipt",
    ] {
        let result = update_own_source_journal_with_validation(
            &identity,
            &lease,
            &fixture.context,
            &revision,
            |journal| {
                match field {
                    "intent" => {
                        journal
                            .intents
                            .get_mut(&operation)
                            .unwrap()
                            .serialized_after
                            .push(b' ');
                    }
                    "inventory" => {
                        journal.inventories.remove(&operation);
                    }
                    "packet" => {
                        journal
                            .progress
                            .get_mut(&operation)
                            .unwrap()
                            .packets
                            .clear();
                    }
                    "prepared" => {
                        journal
                            .progress
                            .get_mut(&operation)
                            .unwrap()
                            .packets_prepared = false
                    }
                    "published" => {
                        journal
                            .progress
                            .get_mut(&operation)
                            .unwrap()
                            .source_published = false
                    }
                    _ => {
                        journal.progress.get_mut(&operation).unwrap().receipt =
                            Some(SourceReceipt {
                                operation_id: operation.clone(),
                                source_revision: revision.clone(),
                            })
                    }
                }
                Ok(())
            },
            &|| Ok(()),
        );
        assert!(result.is_err(), "mutation accepted: {field}");
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            original
        );
    }
    drop(lease);
    drop(identity);
    let private = fixture
        ._root
        .path()
        .join("C")
        .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
    let mut damaged = serde_json::to_value(&original).unwrap();
    damaged["inventories"]
        .as_object_mut()
        .unwrap()
        .remove(&operation);
    let bytes = serde_json::to_vec(&damaged).unwrap();
    std::fs::write(&private, &bytes).unwrap();
    let expected = SourceResumeExpectation {
        source: source_transition_digest_bytes(&original.intents[&operation].serialized_after),
        journal: source_transition_digest_bytes(&bytes),
    };
    let decision = std::fs::read(target.join("config.json")).unwrap();
    assert!(resume_identity_transition_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        &operation,
        &expected,
        &|| Ok(())
    )
    .is_err());
    assert_eq!(std::fs::read(private).unwrap(), bytes);
    assert_eq!(std::fs::read(target.join("config.json")).unwrap(), decision);
}
#[test]
fn single_editor_original_inventory_topology_and_local_fence() {
    let (fixture, operation, target) = existing_prepared("prepared");
    let original = read_own_source_journal(&fixture.context).unwrap().1;
    let packet_path =
        target.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME);
    let packet = std::fs::read(&packet_path).unwrap();
    let late = fixture.project.join(".ac/_agent_late");
    std::fs::create_dir(&late).unwrap();
    std::fs::write(late.join("config.json"), b"{}").unwrap();
    let effects = source_transition_publication_counts();
    assert!(fixture.resume_at(&fixture.context, &operation).is_err());
    assert_eq!(
        read_own_source_journal(&fixture.context).unwrap().1,
        original
    );
    assert_eq!(std::fs::read(&packet_path).unwrap(), packet);
    assert_eq!(source_transition_publication_counts(), effects);
    let result = commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "fresh-request-blocked",
        &fixture.expectation().source,
        &DriverFixture::candidate(),
        &|| Ok(()),
    );
    assert!(
        matches!(result, Err(SourceTransitionError::Interrupted { operation_id, .. }) if operation_id == operation)
    );
    assert_eq!(
        read_own_source_journal(&fixture.context).unwrap().1,
        original
    );
    std::fs::remove_file(late.join("config.json")).unwrap();
    std::fs::remove_dir(&late).unwrap();
    let complete = fixture.resume_at(&fixture.context, &operation).unwrap();
    assert_eq!(complete.state, IdentityTransitionReadState::Complete);
    let journal = read_own_source_journal(&fixture.context).unwrap().1;
    assert_eq!(journal.intents[&operation], original.intents[&operation]);
    assert_eq!(
        journal.inventories[&operation],
        original.inventories[&operation]
    );
    assert_eq!(
        journal.progress[&operation].packets,
        original.progress[&operation].packets
    );
}

#[test]
fn single_editor_source_context_physical_key_and_uncertainty() {
    let fixture = DriverFixture::new();
    let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
    let identity =
        IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
    let counts = source_transition_effect_counts().0;
    let lease = SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
    let key = lease.physical_source_key().to_owned();
    assert_eq!(source_transition_effect_counts().0[1], counts[1]);
    std::fs::write(&path, b"{}").unwrap();
    lease.revalidate(&fixture.context).unwrap();
    let replacement = path.with_extension("replacement");
    std::fs::write(&replacement, b"{\"replacement\":true}").unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    let fresh = SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
    assert_eq!(fresh.physical_source_key(), key);
    let alias_settings = fixture._root.path().join("C/./settings.json");
    let alias = SourceReadContext::from_settings(&fixture.settings, &alias_settings).unwrap();
    assert_eq!(
        SourceWriteContext::acquire(&alias, &fixture.source, &identity)
            .unwrap()
            .physical_source_key(),
        key
    );
    let foreign = fixture._root.path().join("foreign-C");
    std::fs::create_dir(&foreign).unwrap();
    let foreign_context =
        SourceReadContext::from_settings(&fixture.settings, &foreign.join("settings.json"))
            .unwrap();
    assert!(matches!(
        SourceWriteContext::acquire(&foreign_context, &fixture.source, &identity),
        Err(SourceCoordinationError::Unauthorized)
    ));
    let hardlink = path.with_extension("hardlink");
    std::fs::hard_link(&path, &hardlink).unwrap();
    assert!(SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"replacement\":true}");
}

#[test]
fn single_editor_pending_physical_source_isolation() {
    let fixture = DriverFixture::new();
    let registered = serde_json::json!({"agents":[
        {"id":"K","label":"Own","color":"#123456","command":"own-base"},
        {"id":"J","label":"Other","color":"#123456","command":"other-base"}],
        "codingAgentProfiles":{"schemaVersion":2,"profilesByAgent":{"K":{
            "A":{"command":"--alpha","env":{}},"B":{"command":"--beta","env":{}}},
            "J":{"A":{"command":"--other","env":{}}}},"defaultProfileByAgent":{},
            "identityByAgent":{"K":"legacy-k","J":"legacy-j"}}});
    let registered_path = fixture
        ._root
        .path()
        .join("C")
        .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME);
    std::fs::write(&registered_path, serde_json::to_vec(&registered).unwrap()).unwrap();
    let other = list_active_sources(&fixture.context)
        .into_iter()
        .find(|source| source.kind == SourceKind::RegisteredInstance)
        .unwrap();
    assert_ne!(other, fixture.source);
    let snapshot = read_source_snapshot(&fixture.context, &other).unwrap();
    assert!(snapshot.snapshot.capabilities.editable_save);
    let revision = snapshot.snapshot.revision;
    let mut candidate_b: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&registered_path).unwrap()).unwrap();
    candidate_b["agents"][0]["command"] = serde_json::json!("updated-own-base");
    configure_source_durable_cut(Some(("intentInventory", 1)));
    assert!(commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "personal-prefence",
        &fixture.initial_revision,
        &DriverFixture::candidate(),
        &|| Ok(())
    )
    .is_err());
    configure_source_durable_cut(None);
    let original = read_own_source_journal(&fixture.context).unwrap().1;
    let original_a_bytes: Vec<_> = original
        .intents
        .iter()
        .map(|(id, intent)| {
            (
                id.clone(),
                serde_json::to_vec(intent).unwrap(),
                serde_json::to_vec(&original.inventories[id]).unwrap(),
            )
        })
        .collect();
    commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &other,
        "base-independent",
        &revision,
        &candidate_b,
        &|| Ok(()),
    )
    .unwrap();
    let after = read_own_source_journal(&fixture.context).unwrap().1;
    for (id, intent) in &original.intents {
        assert_eq!(after.intents.get(id), Some(intent));
        assert_eq!(after.inventories.get(id), original.inventories.get(id));
    }
    for (id, intent_bytes, inventory_bytes) in original_a_bytes {
        assert_eq!(
            serde_json::to_vec(&after.intents[&id]).unwrap(),
            intent_bytes
        );
        assert_eq!(
            serde_json::to_vec(&after.inventories[&id]).unwrap(),
            inventory_bytes
        );
    }
    assert_eq!(after.intents.len(), 2);
    assert!(after
        .intents
        .values()
        .any(|intent| intent.source == fixture.source));
    assert!(after.intents.values().any(|intent| intent.source == other));
}
