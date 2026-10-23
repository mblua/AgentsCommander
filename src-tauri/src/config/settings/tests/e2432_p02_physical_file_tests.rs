use super::*;
use serde_json::json;
use std::time::Duration;

fn write(path: &Path, value: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn source() -> Value {
    json!({"agents":[{"id":"same","label":"Own","color":"#123456","baseCommand":"tool","extensio\
        n":{"keep":null}}],
            "codingAgentProfiles":{"profilesByAgent":{"same":{"B":{"additionalArguments":"--own","o\
                paque":{"keep":true}}}},"profileLabelsByAgent":{"same":{"B":"Own B"}}},
            "unrelated":{"preserve":[1,null]}})
}

fn fixture() -> (tempfile::TempDir, PathBuf, Arc<SettingsOwner>) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    write(
        &path,
        &serde_json::to_value(AppSettings::default()).unwrap(),
    );
    write(&agents_instance_path(&path), &source());
    let owner = SettingsOwner::open_active(&path, Duration::from_secs(1)).unwrap();
    (temp, path, owner)
}

fn selected(owner: &SettingsOwner, path: &Path) -> (SourceReadContext, SelectedAgentFileSnapshot) {
    let current = owner.blocking_current_snapshot().unwrap();
    let context = SourceReadContext::from_settings(&current, path).unwrap();
    let snapshot =
        read_agent_file_snapshot(&context, &context.source(SourceKind::RegisteredInstance))
            .unwrap();
    assert_eq!(snapshot.availability, SourceAvailability::Available);
    (context, snapshot)
}

fn edits(value: Value) -> AgentFileEdits {
    serde_json::from_value(value).unwrap()
}

#[test]
fn e2432_p02_registered_scoped_save_raw_alias_and_fresh_revision() {
    let (_temp, path, owner) = fixture();
    let (_, before) = selected(&owner, &path);
    let main = std::fs::read(&path).unwrap();
    let hash = configuration_identity("tool", &[]);
    let edit = edits(
        json!({"rows":[{"entryHint":"same","action":"upsert","patch":{"baseCommand":"tool \
            --new"}}],"profileSets":[]}),
    );
    let report = owner
        .with_operation_blocking("p02-save", |current, guard| {
            save_agent_file_locked(
                current,
                guard,
                &before.source,
                &before.revision,
                &before.context_revision,
                &edit,
            )
        })
        .unwrap();
    assert_eq!(report.outcome, OperationOutcome::Committed);
    let after = report.value.unwrap();
    assert_ne!(before.revision, after.revision);
    let raw: Value =
        serde_json::from_slice(&std::fs::read(agents_instance_path(&path)).unwrap()).unwrap();
    assert_eq!(raw["unrelated"], source()["unrelated"]);
    assert_eq!(
        raw["agents"][0]["extension"],
        source()["agents"][0]["extension"]
    );
    assert_eq!(
        raw["codingAgentProfiles"]["profilesByBaseHash"][&hash]["B"]["opaque"],
        json!({"keep":true})
    );
    let new_hash = configuration_identity("tool --new", &[]);
    assert_eq!(
        raw["codingAgentProfiles"]["profilesByBaseHash"][&new_hash],
        raw["codingAgentProfiles"]["profilesByBaseHash"][&hash]
    );
    assert!(raw["codingAgentProfiles"].get("profilesByAgent").is_none());
    assert_eq!(std::fs::read(&path).unwrap(), main);
    assert!(!source_protocol_active());
}

#[test]
fn e2432_p02_cas_forgery_and_overlay_reject_before_publish() {
    let (_temp, path, owner) = fixture();
    let (_, snapshot) = selected(&owner, &path);
    let bytes = std::fs::read(agents_instance_path(&path)).unwrap();
    for case in 0..4 {
        if case == 3 {
            let overlay = path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
            write(
                &overlay,
                &json!({"agents":[{"id":"overlay","label":"Other","color":"red","baseCommand":"othe\
                    r"}]}),
            );
        }
        let report = owner
            .with_operation_blocking("p02-reject", |current, guard| {
                let mut source = snapshot.source.clone();
                if case == 0 {
                    source.context_root.push_str("-forged");
                }
                let current = current.clone();
                let revision = if case == 1 {
                    SourceRevision::Absent {}
                } else {
                    snapshot.revision.clone()
                };
                let context = if case == 2 {
                    "stale"
                } else {
                    &snapshot.context_revision
                };
                let result = save_agent_file_locked(
                    &current,
                    guard,
                    &source,
                    &revision,
                    context,
                    &AgentFileEdits::default(),
                );
                assert!(result.is_err());
                assert_eq!(guard.effect(), OperationEffect::None);
                result
            })
            .unwrap();
        assert_eq!(report.outcome, OperationOutcome::Rejected);
        assert_eq!(std::fs::read(agents_instance_path(&path)).unwrap(), bytes);
    }
}

#[test]
fn e2432_p02_inventory_keeps_instance_catalogs_under_primary_project() {
    let (_temp, path, owner) = fixture();
    let project = path.parent().unwrap().join("project");
    std::fs::create_dir(&project).unwrap();
    let mut current = owner.blocking_current_snapshot().unwrap();
    current.project_paths = vec![project.to_string_lossy().into_owned()];
    let context = SourceReadContext::from_settings(&current, &path).unwrap();
    let mut instance = context.clone();
    instance.project = false;
    instance.catalog_root = path.parent().unwrap().canonicalize().unwrap();
    let catalog = json!({"schemaVersion":1,"agents":[{"key":"same","baseCommand":"tool","label":"Ca\
        talog","color":"blue"}]});
    for ctx in [&context, &instance] {
        for kind in [SourceKind::CatalogBase, SourceKind::CatalogPersonal] {
            write(&ctx.locator(&ctx.source(kind)).unwrap(), &catalog);
        }
    }
    write(
        &context
            .locator(&context.source(SourceKind::CatalogProject))
            .unwrap(),
        &catalog,
    );
    let listed = list_agent_files(&current, &path).unwrap();
    assert_eq!(listed.sources.len(), 6);
    assert_eq!(
        listed
            .sources
            .iter()
            .filter(|s| s.source.scope == SourceScope::Instance)
            .count(),
        3
    );
    assert_eq!(
        listed
            .sources
            .iter()
            .filter(|s| s.capabilities.editable_save)
            .count(),
        2
    );
    let absent = context.source(SourceKind::CatalogPersonal);
    std::fs::remove_file(context.locator(&absent).unwrap()).unwrap();
    assert_eq!(list_agent_files(&current, &path).unwrap().sources.len(), 5);
    current.project_paths = vec![path.parent().unwrap().to_string_lossy().into_owned()];
    let changed = SourceReadContext::from_settings(&current, &path).unwrap();
    assert!(read_agent_file_snapshot(&changed, &absent).is_err());
}

#[test]
fn e2432_p02_preferences_independent_missing_retained_and_first_use_separate() {
    let (_temp, path, owner) = fixture();
    let (context, snapshot) = selected(&owner, &path);
    let catalog_ref = context.source(SourceKind::CatalogPersonal);
    let catalog_path = context.locator(&catalog_ref).unwrap();
    write(&catalog_path, &json!({"schemaVersion":1,"agents":[]}));
    let current = owner.blocking_current_snapshot().unwrap();
    let first = list_agent_files(&current, &path).unwrap();
    assert!(first.last_selected.settings.is_none());
    assert_eq!(
        first.active_source_by_menu.settings,
        Some(snapshot.source.clone())
    );
    let agents = std::fs::read(agents_instance_path(&path)).unwrap();
    for (menu, source) in [
        (AgentFileSourceMenu::Settings, &snapshot.source),
        (AgentFileSourceMenu::Picker, &catalog_ref),
    ] {
        let report = owner
            .with_operation_blocking("p02-remember", |current, guard| {
                remember_agent_file_locked(current, guard, menu, source)
            })
            .unwrap();
        assert_eq!(report.outcome, OperationOutcome::Committed);
    }
    assert_eq!(std::fs::read(agents_instance_path(&path)).unwrap(), agents);
    std::fs::remove_file(&catalog_path).unwrap();
    let current = owner.blocking_current_snapshot().unwrap();
    let listed = list_agent_files(&current, &path).unwrap();
    assert_eq!(listed.last_selected.picker, Some(catalog_ref.clone()));
    assert!(listed.active_source_by_menu.picker.is_none());
    assert_eq!(listed.active_source_by_menu.settings, Some(snapshot.source));
    assert!(listed.menu_diagnostics.contains_key("picker"));
    let bytes = std::fs::read(&path).unwrap();
    let rejected = owner
        .with_operation_blocking("p02-absent-choice", |current, guard| {
            let result = remember_agent_file_locked(
                current,
                guard,
                AgentFileSourceMenu::Picker,
                &catalog_ref,
            );
            assert_eq!(guard.effect(), OperationEffect::None);
            result
        })
        .unwrap();
    assert_eq!(rejected.outcome, OperationOutcome::Rejected);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(read_agent_file_snapshot(&context, &catalog_ref)
        .unwrap()
        .tickets
        .is_empty());
}

#[test]
fn e2432_p02_nonregular_invalid_and_unreadable_are_diagnostic_options() {
    let (_temp, path, owner) = fixture();
    let (context, _) = selected(&owner, &path);
    let source = context.source(SourceKind::CatalogPersonal);
    let catalog = context.locator(&source).unwrap();
    std::fs::create_dir_all(&catalog).unwrap();
    let listed = list_agent_files(&owner.blocking_current_snapshot().unwrap(), &path).unwrap();
    let invalid = listed.sources.iter().find(|s| s.source == source).unwrap();
    assert_eq!(invalid.availability, SourceAvailability::Invalid);
    assert!(!invalid.capabilities.available_read);
    assert!(serde_json::to_value(invalid).unwrap().get("rows").is_none());
    std::fs::remove_dir(&catalog).unwrap();
    std::fs::write(&catalog, b"{broken").unwrap();
    assert_eq!(
        read_agent_file_snapshot(&context, &source)
            .unwrap()
            .availability,
        SourceAvailability::Invalid
    );
    std::fs::write(&catalog, br#"{"schemaVersion":9,"agents":[]}"#).unwrap();
    assert_eq!(
        read_agent_file_snapshot(&context, &source)
            .unwrap()
            .availability,
        SourceAvailability::Invalid
    );
    // Real sharing-denial (Windows), without links or permission-dependent root behavior.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let _held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&catalog)
            .unwrap();
        assert_eq!(
            read_agent_file_snapshot(&context, &source)
                .unwrap()
                .availability,
            SourceAvailability::Invalid
        );
    }
}

#[test]
fn e2432_p02_partial_remove_rows_stay_raw_and_profile_deletion_is_hash_scoped() {
    let (_temp, path, owner) = fixture();
    let (context, _) = selected(&owner, &path);
    let source = context.source(SourceKind::CatalogPersonal);
    let catalog = context.locator(&source).unwrap();
    let raw = json!({"schemaVersion":1,"agents":[
            {"key":"same","baseCommand":"tool","label":"Own","color":"blue","unknown":{"kee\
                p":null}},
            {"key":"partial","label":"Incomplete","unknown":42},
            {"key":"removed","remove":true,"opaque":[null]}],"extension":{"keep":true},
            "codingAgentProfiles":{"same":{"B":{"notes":"own","opaque":42}}},"profileLabels":{"sam\
                e":{"B":"Own"}}});
    write(&catalog, &raw);
    let snapshot = read_agent_file_snapshot(&context, &source).unwrap();
    assert_eq!(snapshot.availability, SourceAvailability::Available);
    assert_eq!(snapshot.rows.iter().filter(|row| row.launchable).count(), 1);
    assert!(snapshot.rows[1].base_hash.is_none());
    assert!(snapshot.rows[2].base_hash.is_none());
    let hash = configuration_identity("tool", &[]);
    let edit = edits(json!({"rows":[],"profileSets":[{"baseHash":hash,"cells":{},"labels":{}}]}));
    let after = prepare_agent_file_edits(&raw, false, &edit).unwrap();
    assert_eq!(after["agents"], raw["agents"]);
    assert_eq!(after["extension"], raw["extension"]);
    assert!(after["profilesByBaseHash"][&hash].get("B").is_none());
    assert!(after["profileLabelsByBaseHash"][&hash]
        .as_object()
        .unwrap()
        .is_empty());
    let bad = edits(
        json!({"rows":[{"entryHint":"same","action":"upsert","patch":{"id":"forged"}}],"profileSet\
            s":[]}),
    );
    assert!(prepare_agent_file_edits(&raw, false, &bad).is_err());
}
#[test]
fn e2432_p02_profile_edits_preserve_unknowns_and_reject_duplicate_or_orphan_targets() {
    let raw = source();
    let hash = configuration_identity("tool", &[]);
    let valid = edits(
        json!({"rows":[],"profileSets":[{"baseHash":hash,"cells":{"B":{"additionalArguments":"--edi\
            ted"}},"labels":{"B":"Edited"}}]}),
    );
    let after = prepare_agent_file_edits(&raw, true, &valid).unwrap();
    assert_eq!(
        after["codingAgentProfiles"]["profilesByBaseHash"][&hash]["B"]["opaque"],
        json!({"keep":true})
    );
    for bad in [
        json!({"rows":[],"profileSets":[{"baseHash":"unknown","cells":{},"labels":{}}]}),
        json!({"rows":[],"profileSets":[{"baseHash":hash,"cells":{},"labels":{}},{"baseHash":hash,"\
            cells":{},"labels":{}}]}),
        json!({"rows":[{"entryHint":"same","action":"delete","patch":{}},{"entryHint":"same","actio\
            n":"delete","patch":{}}],"profileSets":[]}),
        json!({"rows":[],"profileSets":[{"baseHash":hash,"cells":{"B":{"newUnknown":true}},"label\
            s":{}}]}),
    ] {
        assert!(prepare_agent_file_edits(&raw, true, &edits(bad)).is_err());
    }
    let deletion =
        edits(json!({"rows":[{"entryHint":"same","action":"delete","patch":{}}],"profileSets":[]}));
    let removed = prepare_agent_file_edits(&after, true, &deletion).unwrap();
    assert!(removed["agents"].as_array().unwrap().is_empty());
    assert_eq!(
        removed["codingAgentProfiles"]["profilesByBaseHash"],
        after["codingAgentProfiles"]["profilesByBaseHash"]
    );
}

#[test]
fn e2432_p02_registered_atomic_failure_retains_actual_revision_and_owner() {
    let (_temp, path, owner) = fixture();
    let (context, before) = selected(&owner, &path);
    let target = context.locator(&before.source).unwrap();
    let bytes = std::fs::read(agents_instance_path(&path)).unwrap();
    let edit = edits(
        json!({"rows":[{"entryHint":"same","action":"upsert","patch":{"label":"Edited"}}],"profileS\
            ets":[]}),
    );
    let report = owner
        .with_operation_blocking("p02-registered-atomic-failure", |current, guard| {
            PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
                *slot.borrow_mut() = Some((target.clone(), SettingsSaveStage::TempCreate))
            });
            let result = save_agent_file_locked(
                current,
                guard,
                &before.source,
                &before.revision,
                &before.context_revision,
                &edit,
            );
            let fault_fired = matches!(
                &result,
                Err(reason) if reason.contains("injected migration atomic failure")
            );
            PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| *slot.borrow_mut() = None);
            assert!(fault_fired, "exact publisher checkpoint must fire");
            assert_eq!(guard.effect(), OperationEffect::Possible);
            result
        })
        .unwrap();
    assert_eq!(report.outcome, OperationOutcome::PartialOrUnknown);
    assert_eq!(std::fs::read(agents_instance_path(&path)).unwrap(), bytes);
    let (_, actual) = selected(&owner, &path);
    assert_eq!(actual.revision, before.revision);
    assert_eq!(actual.rows[0].raw["label"], json!("Own"));
}
