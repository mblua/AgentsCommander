use super::*;

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
    fixture.controls.configure_durable_cut(Some(cut));
    let result = commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "durable-boundary",
        &revision,
        &candidate,
        (&|| Ok(()), &fixture.controls),
    );
    assert!(result.is_err(), "unreached cut: {cut:?}");
    assert!(
        fixture.controls.durable_cut_state().0.is_none(),
        "cut was not reached: {cut:?}"
    );
    fixture.controls.configure_durable_cut(None);
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
            (&|| Ok(()), &fixture.controls),
        )
        .unwrap();
        return;
    }
    let operation = fixture.operation();
    assert!(matches!(
        result,
        Err(SourceTransitionError::Interrupted { operation_id, .. }) if operation_id == operation
    ));
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
    let uuids = fixture.controls.uuid_count();
    fixture.resume_at(&fixture.context, &operation).unwrap();
    let recovered = read_own_source_journal(&fixture.context).unwrap().1;
    assert_eq!(recovered.intents[&operation], original.intents[&operation]);
    assert_eq!(
        recovered.inventories[&operation],
        original.inventories[&operation]
    );
    assert_eq!(recovered.progress[&operation].packets.len(), 2);
    assert!(recovered.progress[&operation].receipt.is_some());
    assert_eq!(fixture.controls.uuid_count(), uuids);
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
        (
            &|| {
                reloads.set(reloads.get() + 1);
                Ok(())
            },
            &fixture.controls,
        ),
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
        fixture.controls.configure_durable_cut(Some(("packet", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "authority-drift",
            &revision,
            &candidate,
            (&|| Ok(()), &fixture.controls),
        )
        .is_err());
        fixture.controls.configure_durable_cut(None);
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
        fixture
            .controls
            .configure_durable_cut(Some(("intentInventory", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "prefence-CAS",
            &fixture.initial_revision,
            &DriverFixture::candidate(),
            (&|| Ok(()), &fixture.controls),
        )
        .is_err());
        fixture.controls.configure_durable_cut(None);
        let operation = fixture.operation();
        let journal = read_own_source_journal(&fixture.context).unwrap().1;
        let uuid_count = fixture.controls.uuid_count();
        assert!(matches!(
            commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "different-prefence-request",
                &fixture.initial_revision,
                &DriverFixture::candidate(),
                (&|| Ok(()), &fixture.controls),
            ),
            Err(SourceTransitionError::Interrupted { .. })
        ));
        assert_eq!(fixture.controls.uuid_count(), uuid_count);
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
                &|| Ok(()),
                &fixture.controls,
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
        let config_json = serde_json::json!({"tooling":{"replicaSelectionDefault":{
            "codingAgentId":"K","profile":"B","configurationRef":selection.clone()
        }}});
        std::fs::write(
            target.join("config.json"),
            serde_json::to_vec(&config_json).unwrap(),
        )
        .unwrap();

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
        fixture
            .controls
            .configure_durable_cut(Some(("sourceAfter", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "registered-source-after",
            &snapshot.snapshot.revision,
            &candidate,
            (&|| Ok(()), &fixture.controls),
        )
        .is_err());
        fixture.controls.configure_durable_cut(None);
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
        fixture
            .controls
            .configure_durable_cut(Some(("intentInventory", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "missing-original",
            &before,
            &candidate,
            (&|| Ok(()), &fixture.controls),
        )
        .is_err());
        fixture.controls.configure_durable_cut(None);
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
            &|| Ok(()),
            &fixture.controls,
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
            (
                &|| {
                    assert_eq!(std::thread::current().id(), owning_thread);
                    Ok(())
                },
                &fixture.controls,
            ),
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
    fixture
        .controls
        .configure_durable_cut(Some(("intentInventory", 1)));
    assert!(commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "slot-completeness",
        &revision,
        &candidate,
        (&|| Ok(()), &fixture.controls),
    )
    .is_err());
    fixture.controls.configure_durable_cut(None);
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
            let controls = IdentityTransitionTestControls::default();
            controls.set_fence_effect_barrier(worker_barrier);
            let result = commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "real-fence-drift",
                &revision,
                &candidate,
                (&|| Ok(()), &controls),
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
            let controls = IdentityTransitionTestControls::default();
            controls.set_fence_effect_barrier(worker_barrier);
            let error = fixture
                .resume_at_with_controls(&fixture.context, &resume_operation, &controls)
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
    fixture
        .controls
        .configure_durable_cut(Some(("beforeFence", 1)));
    assert!(commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "production-before-fence",
        &revision,
        &candidate,
        (&|| Ok(()), &fixture.controls),
    )
    .is_err());
    assert!(fixture.controls.durable_cut_state().0.is_none());
    fixture.controls.configure_durable_cut(None);
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
    fixture
        .controls
        .configure_durable_cut(Some(("beforeFence", 1)));
    assert!(fixture.resume_at(&fixture.context, &operation).is_err());
    assert!(fixture.controls.durable_cut_state().0.is_none());
    fixture.controls.configure_durable_cut(None);
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
    fixture
        .controls
        .configure_durable_cut(Some(("intentInventory", 1)));
    assert!(commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "primary-context",
        &fixture.initial_revision,
        &DriverFixture::candidate(),
        (&|| Ok(()), &fixture.controls),
    )
    .is_err());
    fixture.controls.configure_durable_cut(None);
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
        &|| Ok(()),
        &fixture.controls,
    )
    .is_err());
    assert_eq!(source_transition_effect_counts().1, publishers);
    assert_eq!(
        read_own_source_journal(&fixture.context).unwrap().1,
        original
    );
}
