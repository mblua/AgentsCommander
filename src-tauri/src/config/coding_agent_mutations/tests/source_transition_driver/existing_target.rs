use super::*;

fn existing_prepared(cut: &'static str) -> (DriverFixture, String, std::path::PathBuf) {
    let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
    let before = fixture.expectation().source;
    fixture.controls.configure_durable_cut(Some((cut, 1)));
    let result = commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "existing-original",
        &before,
        &candidate,
        (&|| Ok(()), &fixture.controls),
    );
    assert!(
        matches!(result, Err(SourceTransitionError::Interrupted { .. })),
        "{result:?}"
    );
    assert!(fixture.controls.durable_cut_state().0.is_none());
    fixture.controls.configure_durable_cut(None);
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
        let uuids = fixture.controls.uuid_count();
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
        assert_eq!(fixture.controls.uuid_count(), uuids);
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
            },
            &fixture.controls,
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
        &|| Ok(()),
        &fixture.controls,
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
        (&|| Ok(()), &fixture.controls),
    );
    assert!(matches!(
        result,
        Err(SourceTransitionError::Interrupted { operation_id, .. }) if operation_id == operation
    ));
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
    fixture
        .controls
        .configure_durable_cut(Some(("intentInventory", 1)));
    assert!(commit_source_candidate_internal(
        &fixture.settings,
        &fixture.context,
        &fixture.source,
        "personal-prefence",
        &fixture.initial_revision,
        &DriverFixture::candidate(),
        (&|| Ok(()), &fixture.controls),
    )
    .is_err());
    fixture.controls.configure_durable_cut(None);
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
        (&|| Ok(()), &fixture.controls),
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
