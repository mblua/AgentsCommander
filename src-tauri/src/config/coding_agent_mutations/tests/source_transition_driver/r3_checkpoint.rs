use super::*;

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
    let uuids = fixture.controls.uuid_count();
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
            (
                &|| {
                    reloads.set(reloads.get() + 1);
                    Ok(())
                },
                &fixture.controls
            ),
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
    assert_eq!(fixture.controls.uuid_count(), uuids);
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
    let operation = new_source_transition_id(&fixture.controls);
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
    fixture
        .controls
        .configure_durable_cut(Some(("prepared", 1)));
    let result = drive_identity_transition(
        &identity,
        &lease,
        &fixture.context,
        &operation,
        &authority,
        &rediscover,
        (&|| Ok(()), &fixture.controls),
    );
    assert!(
        matches!(&result, Err(SourceTransitionError::Coordination(
            SourceCoordinationError::Io(reason)
        )) if reason == "durable cut: prepared"),
        "r3 readiness result: {result:?}"
    );
    assert!(fixture.controls.durable_cut_state().0.is_none());
    fixture.controls.configure_durable_cut(None);
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
            },
            &fixture.controls,
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
    let uuids = fixture.controls.uuid_count();
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
    assert_eq!(fixture.controls.uuid_count(), uuids);
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
        let uuids = fixture.controls.uuid_count();
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
        assert_eq!(fixture.controls.uuid_count(), uuids);
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
