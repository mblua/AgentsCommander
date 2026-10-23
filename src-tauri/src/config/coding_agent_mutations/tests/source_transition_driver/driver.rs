use super::*;

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
        let config_json = serde_json::json!({"tooling":{"replicaSelectionDefault":{
            "codingAgentId":"K","profile":"B","configurationRef":selection.clone()
        }}});
        std::fs::write(
            target.join("config.json"),
            serde_json::to_vec(&config_json).unwrap(),
        )
        .unwrap();
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
        let config_json = serde_json::json!({"tooling":{"replicaSelectionDefault":{
            "codingAgentId":"K","profile":"A","configurationRef":alpha
        }}});
        std::fs::write(
            target.join("config.json"),
            serde_json::to_vec(&config_json).unwrap(),
        )
        .unwrap();
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
        let state_json =
            serde_json::json!({"tooling":{"sourceReferences":[{"configurationRef":selection}]}});
        std::fs::write(
            target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
            serde_json::to_vec(&state_json).unwrap(),
        )
        .unwrap();
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
