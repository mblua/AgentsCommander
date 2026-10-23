use super::super::*;
use serde_json::json;
use std::time::Duration;

struct Fixture {
    path: PathBuf,
    catalog: PathBuf,
    owner: Arc<SettingsOwner>,
    source: SourceRef,
    snapshot: SelectedAgentFileSnapshot,
    _temp: tempfile::TempDir,
}

fn write(path: &Path, value: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(SETTINGS_FILE_NAME);
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let settings = AppSettings {
        project_paths: vec![project.to_string_lossy().into_owned()],
        ..AppSettings::default()
    };
    write(&path, &serde_json::to_value(settings).unwrap());
    write(
        &agents_instance_path(&path),
        &json!({"agents":[{"id":"own","label":"Own","color":"red","baseCommand":"tool"}]}),
    );
    let catalog = crate::config::coding_agents_catalog::independent_catalog_path(
        &project.join(".ac"),
        SourceKind::CatalogPersonal,
    )
    .unwrap();
    write(
        &catalog,
        &json!({"schemaVersion":1,"agents":[{"key":"own","baseCommand":"catalog-tool","label":"Cata\
            log","color":"blue","unknown":[null]}],"extension":{"keep":true}}),
    );
    let owner = SettingsOwner::open_active(&path, Duration::from_secs(1)).unwrap();
    let list = match owner
        .read_agent_files_blocking(AgentFileReadRequest::List)
        .unwrap()
    {
        AgentFileReadReply::List(list) => list,
        _ => panic!("list"),
    };
    let source = list
        .sources
        .into_iter()
        .find(|s| {
            s.source.scope == SourceScope::Project && s.source.kind == SourceKind::CatalogPersonal
        })
        .unwrap()
        .source;
    let snapshot = match owner
        .read_agent_files_blocking(AgentFileReadRequest::Read(source.clone()))
        .unwrap()
    {
        AgentFileReadReply::Read(snapshot) => snapshot,
        _ => panic!("snapshot"),
    };
    Fixture {
        _temp: temp,
        path,
        catalog,
        owner,
        source,
        snapshot,
    }
}

struct MalformedPreferencesCase {
    _temp: tempfile::TempDir,
    path: PathBuf,
    project: PathBuf,
    settings: AppSettings,
    main: Value,
    invalid: Value,
    registered: PathBuf,
    registered_before: Vec<u8>,
}

impl MalformedPreferencesCase {
    fn new(invalid: Value) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(SETTINGS_FILE_NAME);
        let project = temp.path().join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::create_dir(project.join(".ac")).unwrap();
        let settings = AppSettings {
            default_shell: "p02-custom-shell".into(),
            default_shell_args: vec!["--p02".into()],
            root_token: Some("p02-preserved-token".into()),
            project_paths: vec![project.to_string_lossy().into_owned()],
            ..AppSettings::default()
        };
        let mut main = serde_json::to_value(&settings).unwrap();
        main.as_object_mut().unwrap().remove("agents");
        main.as_object_mut().unwrap().remove("codingAgentProfiles");
        main["agentFileSourceByMenu"] = invalid.clone();
        write(&path, &main);
        let packet = json!({
            "agents":[{"id":"own","label":"Own","color":"red","baseCommand":"tool"}],
            "codingAgentProfiles":{
                "schemaVersion":2,
                "profileSlots":{"A":{"label":""}},
                "profilesByAgent":{"own":{"A":{}}}
            }
        });
        let packet = canonical_profile_source(&packet, &packet, true).unwrap();
        let registered = agents_instance_path(&path);
        write(&registered, &packet);
        let registered_before = std::fs::read(&registered).unwrap();
        Self {
            _temp: temp,
            path,
            project,
            settings,
            main,
            invalid,
            registered,
            registered_before,
        }
    }

    fn assert_ordinary_loads(&self) -> (AppSettings, Vec<u8>) {
        let mut ordinary_value = serde_json::to_value(&self.settings).unwrap();
        ordinary_value["agentFileSourceByMenu"] = self.invalid.clone();
        let decoded: AppSettings = serde_json::from_value(ordinary_value).unwrap();
        assert_eq!(decoded.default_shell, self.settings.default_shell);
        assert!(decoded.agent_file_source_by_menu.is_none());
        assert!(
            serde_json::from_value::<AgentFileSourcePreferences>(self.invalid.clone()).is_err()
        );
        let (ordinary, _, _) = read_settings_for_startup(&self.path);
        assert_eq!(ordinary.default_shell, self.settings.default_shell);
        assert_eq!(ordinary.agents[0].id, "own");
        assert!(ordinary.agent_file_source_by_menu.is_none());
        let migrated: Value = serde_json::from_slice(&std::fs::read(&self.path).unwrap()).unwrap();
        assert_eq!(migrated["agentFileSourceByMenu"], self.invalid);
        assert_eq!(migrated["defaultShell"], json!(self.settings.default_shell));
        assert!(migrated.get("projectPaths").is_none());
        let project_disk: Value =
            serde_json::from_slice(&std::fs::read(project_paths_path(&self.path)).unwrap())
                .unwrap();
        assert_eq!(project_disk["projectPaths"], self.main["projectPaths"]);
        let loaded = load_settings_from_path(&self.path);
        assert_eq!(loaded.default_shell, self.settings.default_shell);
        assert_eq!(loaded.default_shell_args, self.settings.default_shell_args);
        assert_eq!(loaded.root_token, self.settings.root_token);
        assert_eq!(loaded.agents[0].command, "tool");
        assert!(loaded.agent_file_source_by_menu.is_none());
        assert!(loaded.project_paths.iter().any(|p| {
            Path::new(p).canonicalize().unwrap() == self.project.canonicalize().unwrap()
        }));
        let original = std::fs::read(&self.path).unwrap();
        let stabilized: Value = serde_json::from_slice(&original).unwrap();
        assert_eq!(stabilized["agentFileSourceByMenu"], self.invalid);
        assert_eq!(
            stabilized["defaultShell"],
            json!(self.settings.default_shell)
        );
        assert_eq!(
            std::fs::read(&self.registered).unwrap(),
            self.registered_before
        );
        (loaded, original)
    }

    fn assert_strict_owner_refusals(&self, original: &[u8]) -> SourceRef {
        let owner = SettingsOwner::open_active(&self.path, Duration::from_secs(1)).unwrap();
        let adopted = owner.blocking_current_snapshot().unwrap();
        assert_eq!(adopted.default_shell, self.settings.default_shell);
        assert_eq!(adopted.agents[0].id, "own");
        assert!(adopted.agent_file_source_by_menu.is_none());
        let context = SourceReadContext::from_settings(&adopted, &self.path).unwrap();
        let source = context.source(SourceKind::RegisteredInstance);
        assert!(matches!(
            owner.read_agent_files_blocking(AgentFileReadRequest::List),
            Err(reason) if reason == "invalidSourcePreferences"
        ));
        assert!(matches!(
            owner.read_agent_files_blocking(AgentFileReadRequest::Read(source.clone())),
            Err(reason) if reason == "invalidSourcePreferences"
        ));
        let rejected = owner
            .with_operation_blocking("p02-malformed-preferences", |current, guard| {
                let result = remember_agent_file_locked(
                    current,
                    guard,
                    AgentFileSourceMenu::Settings,
                    &source,
                );
                assert_eq!(guard.effect(), OperationEffect::None);
                result
            })
            .unwrap();
        assert_eq!(rejected.outcome, OperationOutcome::Rejected);
        assert_eq!(rejected.error.as_deref(), Some("invalidSourcePreferences"));
        assert_eq!(std::fs::read(&self.path).unwrap(), original);
        owner.drain_blocking(Duration::from_secs(1));
        source
    }

    fn assert_save_and_roundtrip(&self, loaded: &AppSettings, source: SourceRef) {
        let saved = save_settings_to_path_preserving_project_paths(loaded, &self.path).unwrap();
        assert_eq!(saved.default_shell, self.settings.default_shell);
        assert!(saved.agent_file_source_by_menu.is_none());
        let disk: Value = serde_json::from_slice(&std::fs::read(&self.path).unwrap()).unwrap();
        assert_eq!(disk["agentFileSourceByMenu"], self.invalid);
        assert_eq!(disk["defaultShell"], json!(self.settings.default_shell));
        let valid = AgentFileSourcePreferences {
            settings: Some(source.clone()),
            picker: Some(source),
        };
        let mut valid_main = serde_json::to_value(&self.settings).unwrap();
        valid_main["agentFileSourceByMenu"] = serde_json::to_value(&valid).unwrap();
        let roundtrip: AppSettings = serde_json::from_value(valid_main).unwrap();
        assert_eq!(
            serde_json::to_value(&roundtrip.agent_file_source_by_menu).unwrap(),
            json!(valid)
        );
        let encoded = serde_json::to_value(&roundtrip).unwrap();
        assert_eq!(encoded["agentFileSourceByMenu"], json!(valid));
    }
}

#[cfg(windows)]
fn assert_simulated_lease_failure_preserved(
    owner: &SettingsOwner,
    epoch: u64,
    ram: &Value,
    counters: [u64; 4],
    bytes: Vec<(PathBuf, Option<Vec<u8>>)>,
) {
    assert_eq!(owner.availability(), SettingsAvailability::Invalid);
    assert_eq!(owner.identity().epoch, epoch);
    assert_eq!(
        serde_json::to_value(owner.settings.blocking_read().clone()).unwrap(),
        *ram
    );
    assert_eq!(
        [
            owner
                .counters
                .transaction_lock_attempts
                .load(Ordering::Relaxed),
            owner.counters.captures.load(Ordering::Relaxed),
            owner.counters.source_reads.load(Ordering::Relaxed),
            owner.counters.operations.load(Ordering::Relaxed),
        ],
        counters
    );
    for (path, before) in bytes {
        match before {
            Some(before) => assert_eq!(std::fs::read(path).unwrap(), before),
            None => assert!(!path.exists()),
        }
    }
    assert_eq!(owner.control().operations, 0);
    assert_eq!(owner.control().permits, 0);
}

fn save(f: &Fixture) -> AgentFileSaveReply {
    f.owner
        .save_catalog_source_blocking(
            f.source.clone(),
            f.snapshot.revision.clone(),
            f.snapshot.context_revision.clone(),
            AgentFileEdits::default(),
        )
        .unwrap()
}

#[test]
fn e2432_p02_closed_catalog_isolates_registered_failure_and_preserves_owner() {
    for nonregular in [false, true] {
        let mut f = fixture();
        let registered = agents_instance_path(&f.path);
        if nonregular {
            std::fs::remove_file(&registered).unwrap();
            std::fs::create_dir(&registered).unwrap();
        } else {
            std::fs::write(&registered, b"{malformed").unwrap();
            f.owner.drain_blocking(Duration::from_secs(1));
            f.owner = SettingsOwner::open_active(&f.path, Duration::from_secs(1)).unwrap();
        }
        let adopted = serde_json::to_value(f.owner.blocking_current_snapshot().unwrap()).unwrap();
        let epoch = f.owner.control().epoch;
        let main_before = std::fs::read(&f.path).unwrap();
        let listed = match f
            .owner
            .read_agent_files_blocking(AgentFileReadRequest::List)
            .unwrap()
        {
            AgentFileReadReply::List(listed) => listed,
            _ => panic!("list"),
        };
        let registered_source = listed
            .sources
            .iter()
            .find(|s| s.source.kind == SourceKind::RegisteredInstance)
            .unwrap();
        assert_eq!(registered_source.availability, SourceAvailability::Invalid);
        let actual = match f
            .owner
            .read_agent_files_blocking(AgentFileReadRequest::Read(registered_source.source.clone()))
            .unwrap()
        {
            AgentFileReadReply::Read(snapshot) => snapshot,
            _ => panic!("snapshot"),
        };
        assert!(actual.rows.is_empty() && actual.tickets.is_empty());
        let committed = save(&f);
        assert_eq!(committed.outcome, AgentFileSaveOutcome::Committed);
        let disk = std::fs::read(&f.catalog).unwrap();
        assert_eq!(
            committed.snapshot.unwrap().revision,
            source_bytes_revision(&disk)
        );
        assert_eq!(
            serde_json::to_value(committed.effective_settings.unwrap()).unwrap(),
            adopted
        );
        assert!(committed
            .diagnostics
            .iter()
            .any(|d| d.code == "effectiveSettingsUnchanged"));
        assert_eq!(f.owner.control().epoch, epoch);
        assert_eq!(
            serde_json::to_value(f.owner.blocking_current_snapshot().unwrap()).unwrap(),
            adopted
        );
        let mut called = false;
        let rejected =
            f.owner
                .with_operation_blocking("p02-preference-participants", |current, guard| {
                    called = true;
                    remember_agent_file_locked(
                        current,
                        guard,
                        AgentFileSourceMenu::Picker,
                        &f.source,
                    )
                });
        assert!(rejected.is_err());
        assert!(!called);
        assert_eq!(std::fs::read(&f.path).unwrap(), main_before);
        if nonregular {
            assert!(registered.is_dir());
        } else {
            assert_eq!(std::fs::read(registered).unwrap(), b"{malformed");
        }
    }
}

#[test]
fn e2432_p02_authority_duplicates_and_syntax_refuse_public_owner_paths() {
    for sidecar in [false, true] {
        let f = fixture();
        let target = if sidecar {
            project_paths_path(&f.path)
        } else {
            f.path.clone()
        };
        let original = std::fs::read(&f.catalog).unwrap();
        let main = std::fs::read(&f.path).unwrap();
        if sidecar {
            std::fs::write(
                &target,
                br#"{"projectPaths":[],"unrelated":1,"unrelated":2}"#,
            )
            .unwrap();
        } else {
            let mut bytes = main.clone();
            bytes.pop();
            bytes
                .extend_from_slice(br#", "agentFileSourceByMenu":{}, "agentFileSourceByMenu":{}}"#);
            std::fs::write(&target, &bytes).unwrap();
        }
        let bad = std::fs::read(&target).unwrap();
        assert!(f
            .owner
            .read_agent_files_blocking(AgentFileReadRequest::List)
            .is_err());
        let rejected = save(&f);
        assert_eq!(rejected.outcome, AgentFileSaveOutcome::Rejected);
        assert_eq!(std::fs::read(&f.catalog).unwrap(), original);
        assert_eq!(std::fs::read(&target).unwrap(), bad);
        std::fs::write(&target, b"{").unwrap();
        assert!(f
            .owner
            .read_agent_files_blocking(AgentFileReadRequest::Read(f.source.clone()))
            .is_err());
        assert_eq!(save(&f).outcome, AgentFileSaveOutcome::Rejected);
    }
}

#[test]
fn e2432_p02_captured_validation_has_no_participant_reread() {
    use std::cell::Cell;
    use std::rc::Rc;
    let f = fixture();
    let registered = agents_instance_path(&f.path);
    let overlay = f.path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
    write(
        &overlay,
        &json!({"codingAgentProfiles":{"profilesByAgent":{}}}),
    );
    let context =
        SourceReadContext::from_settings(&f.owner.blocking_current_snapshot().unwrap(), &f.path)
            .unwrap();
    let registered_ref = context.source(SourceKind::RegisteredInstance);
    let captured = Rc::new(Cell::new(false));
    let flag = Rc::clone(&captured);
    let registered_path = registered.clone();
    let overlay_path = overlay.clone();
    AGENT_FILE_CHECKPOINT.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |stage, path| {
            if stage == "diagnostic_captured" {
                flag.set(true);
                for participant in [&registered_path, &overlay_path] {
                    std::fs::remove_file(participant).unwrap();
                    std::fs::create_dir(participant).unwrap();
                }
            } else if stage == "observe"
                && flag.get()
                && (path == registered_path.as_path() || path == overlay_path.as_path())
            {
                panic!("unexpected participant read");
            }
        }))
    });
    let reply = f
        .owner
        .read_agent_files_blocking(AgentFileReadRequest::Read(registered_ref));
    AGENT_FILE_CHECKPOINT.with(|hook| *hook.borrow_mut() = None);
    let snapshot = match reply.unwrap() {
        AgentFileReadReply::Read(snapshot) => snapshot,
        _ => panic!("snapshot"),
    };
    assert!(captured.get());
    assert_eq!(snapshot.availability, SourceAvailability::Available);
    assert_eq!(snapshot.rows[0].raw["baseCommand"], json!("tool"));
    assert!(registered.is_dir() && overlay.is_dir());
    let inputs = vec![
        (f.path.clone(), Ok(Some(std::fs::read(&f.path).unwrap()))),
        (registered, Ok(Some(b"{".to_vec()))),
        (
            overlay,
            Ok(Some(br#"{"agents":[],"unknown":1,"unknown":2}"#.to_vec())),
        ),
    ];
    let validation = classify_registered_profile_inputs(&f.path, inputs);
    assert!(validation
        .problems
        .iter()
        .any(|p| p.kind == RegisteredProfileProblemKind::JsonSyntax));
    assert!(validation
        .problems
        .iter()
        .any(|p| p.kind == RegisteredProfileProblemKind::DuplicateMember));
}

#[test]
fn e2432_p02_catalog_prepublication_authority_and_source_races_reject() {
    for change_main in [false, true] {
        let f = fixture();
        let path = if change_main {
            f.path.clone()
        } else {
            f.catalog.clone()
        };
        let mut raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        raw["externalChange"] = json!(true);
        let injected = serde_json::to_vec(&raw).unwrap();
        let bytes = injected.clone();
        let old_catalog = std::fs::read(&f.catalog).unwrap();
        AGENT_FILE_CHECKPOINT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |stage, _| {
                if stage == "catalog_pre_recheck" {
                    std::fs::write(&path, &bytes).unwrap();
                }
            }))
        });
        let rejected = save(&f);
        AGENT_FILE_CHECKPOINT.with(|hook| *hook.borrow_mut() = None);
        assert_eq!(rejected.outcome, AgentFileSaveOutcome::Rejected);
        if change_main {
            assert_eq!(std::fs::read(&f.catalog).unwrap(), old_catalog);
        } else {
            assert_eq!(std::fs::read(&f.catalog).unwrap(), injected);
        }
    }
}

#[test]
fn e2432_p02_catalog_postpossible_panic_and_reread_failure_are_uncertain() {
    for panic_publish in [false, true] {
        let f = fixture();
        let epoch = f.owner.control().epoch;
        let adopted = serde_json::to_value(f.owner.blocking_current_snapshot().unwrap()).unwrap();
        let target = f.catalog.clone();
        let original_revision = f.snapshot.revision.clone();
        AGENT_FILE_CHECKPOINT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |stage, _| {
                if panic_publish && stage == "catalog_publish" {
                    panic!("publisher interruption");
                }
                if !panic_publish && stage == "catalog_before_reread" {
                    std::fs::remove_file(&target).unwrap();
                    std::fs::create_dir(&target).unwrap();
                }
            }))
        });
        let uncertain = save(&f);
        AGENT_FILE_CHECKPOINT.with(|hook| *hook.borrow_mut() = None);
        assert_eq!(uncertain.outcome, AgentFileSaveOutcome::PartialOrUnknown);
        assert_eq!(f.owner.control().epoch, epoch);
        assert_eq!(f.owner.control().availability, SettingsAvailability::Ready);
        assert_eq!(
            serde_json::to_value(f.owner.blocking_current_snapshot().unwrap()).unwrap(),
            adopted
        );
        if panic_publish {
            assert_eq!(uncertain.snapshot.unwrap().revision, original_revision);
        } else {
            assert_eq!(
                uncertain.snapshot.unwrap().availability,
                SourceAvailability::Invalid
            );
        }
    }
}

#[test]
fn e2432_p02_preferences_invalid_catalog_fresh_other_menu_and_missing_ref() {
    let f = fixture();
    let sidecar_before = std::fs::read(agents_instance_path(&f.path)).unwrap();
    let context =
        SourceReadContext::from_settings(&f.owner.blocking_current_snapshot().unwrap(), &f.path)
            .unwrap();
    let registered = context.source(SourceKind::RegisteredInstance);
    let mut raw: Value = serde_json::from_slice(&std::fs::read(&f.path).unwrap()).unwrap();
    raw["agentFileSourceByMenu"] = json!({"picker":registered});
    write(&f.path, &raw);
    // Existing invalid catalog is an option, not a preference-write participant.
    std::fs::write(&f.catalog, b"{").unwrap();
    let report = f
        .owner
        .with_operation_blocking("p02-preference-invalid-catalog", |current, guard| {
            remember_agent_file_locked(current, guard, AgentFileSourceMenu::Settings, &f.source)
        })
        .unwrap();
    assert_eq!(report.outcome, OperationOutcome::Committed);
    let saved: Value = serde_json::from_slice(&std::fs::read(&f.path).unwrap()).unwrap();
    assert_eq!(saved["agentFileSourceByMenu"]["picker"], json!(registered));
    assert_eq!(saved["agentFileSourceByMenu"]["settings"], json!(f.source));
    assert_eq!(
        std::fs::read(agents_instance_path(&f.path)).unwrap(),
        sidecar_before
    );
    let list = match f
        .owner
        .read_agent_files_blocking(AgentFileReadRequest::List)
        .unwrap()
    {
        AgentFileReadReply::List(list) => list,
        _ => panic!("list"),
    };
    assert_eq!(list.active_source_by_menu.settings, Some(f.source.clone()));
    assert!(list.menu_diagnostics.contains_key("settings"));
    let invalid = match f
        .owner
        .read_agent_files_blocking(AgentFileReadRequest::Read(f.source.clone()))
        .unwrap()
    {
        AgentFileReadReply::Read(s) => s,
        _ => panic!("read"),
    };
    assert_eq!(invalid.availability, SourceAvailability::Invalid);
    assert!(invalid.rows.is_empty() && invalid.tickets.is_empty());
    std::fs::remove_file(&f.catalog).unwrap();
    let before = std::fs::read(&f.path).unwrap();
    let missing = match f
        .owner
        .read_agent_files_blocking(AgentFileReadRequest::List)
        .unwrap()
    {
        AgentFileReadReply::List(list) => list,
        _ => panic!("list"),
    };
    assert_eq!(missing.last_selected.settings, Some(f.source));
    assert!(missing.active_source_by_menu.settings.is_none());
    assert_eq!(missing.active_source_by_menu.picker, Some(registered));
    assert_eq!(std::fs::read(&f.path).unwrap(), before);
}

#[test]
fn e2432_p02_preference_failed_adoption_never_becomes_successful_owner_reply() {
    let f = fixture();
    let registered = agents_instance_path(&f.path);
    AGENT_FILE_CHECKPOINT.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |stage, _| {
            if stage == "preference_published" {
                std::fs::write(&registered, b"{").unwrap();
            }
        }))
    });
    let report = f
        .owner
        .with_operation_blocking("p02-preference-recapture", |current, guard| {
            remember_agent_file_locked(current, guard, AgentFileSourceMenu::Settings, &f.source)
        })
        .unwrap();
    AGENT_FILE_CHECKPOINT.with(|hook| *hook.borrow_mut() = None);
    assert_eq!(report.outcome, OperationOutcome::PartialOrUnknown);
    assert!(report.value.is_some());
    assert_eq!(report.availability, SettingsAvailability::Invalid);
    assert!(f.owner.blocking_current_snapshot().is_err());
    let saved: Value = serde_json::from_slice(&std::fs::read(&f.path).unwrap()).unwrap();
    assert_eq!(saved["agentFileSourceByMenu"]["settings"], json!(f.source));
    assert!(f
        .owner
        .read_agent_files_blocking(AgentFileReadRequest::List)
        .is_err());
    assert!(f
        .owner
        .save_catalog_source_blocking(
            f.source,
            f.snapshot.revision,
            f.snapshot.context_revision,
            AgentFileEdits::default()
        )
        .is_err());
}

#[test]
fn e2432_p02_catalog_waiting_lock_rechecks_captured_authority() {
    let f = fixture();
    let held = crate::config::coding_agents_catalog::hold_selected_catalog_test_lock(&f.catalog);
    let (authorized_tx, authorized_rx) = std::sync::mpsc::channel();
    let owner = Arc::clone(&f.owner);
    let source = f.source.clone();
    let revision = f.snapshot.revision.clone();
    let context = f.snapshot.context_revision.clone();
    let worker = std::thread::spawn(move || {
        AGENT_FILE_CHECKPOINT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |stage, _| {
                if stage == "catalog_authorized" {
                    authorized_tx.send(()).unwrap();
                }
            }))
        });
        let reply = owner.save_catalog_source_blocking(
            source,
            revision,
            context,
            AgentFileEdits::default(),
        );
        AGENT_FILE_CHECKPOINT.with(|hook| *hook.borrow_mut() = None);
        reply.unwrap()
    });
    authorized_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut main: Value = serde_json::from_slice(&std::fs::read(&f.path).unwrap()).unwrap();
    main["externalWhileCatalogLocked"] = json!(true);
    write(&f.path, &main);
    let before = std::fs::read(&f.catalog).unwrap();
    drop(held);
    let rejected = worker.join().unwrap();
    assert_eq!(rejected.outcome, AgentFileSaveOutcome::Rejected);
    assert_eq!(std::fs::read(&f.catalog).unwrap(), before);
}

#[test]
fn e2432_p02_drain_retains_admitted_diagnostic_until_worker_releases() {
    let f = fixture();
    let (captured_tx, captured_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let owner = Arc::clone(&f.owner);
    let worker = std::thread::spawn(move || {
        AGENT_FILE_CHECKPOINT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |stage, _| {
                if stage == "diagnostic_captured" {
                    captured_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                }
            }))
        });
        let result = owner.read_agent_files_blocking(AgentFileReadRequest::List);
        AGENT_FILE_CHECKPOINT.with(|hook| *hook.borrow_mut() = None);
        result
    });
    captured_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let owner = Arc::clone(&f.owner);
    let drain = std::thread::spawn(move || owner.drain_blocking(Duration::from_secs(2)));
    let start = std::time::Instant::now();
    while f.owner.control().availability != SettingsAvailability::Draining
        && start.elapsed() < Duration::from_secs(2)
    {
        std::thread::yield_now();
    }
    assert_eq!(
        f.owner.control().availability,
        SettingsAvailability::Draining
    );
    assert_eq!(f.owner.control().operations, 1);
    assert!(f
        .owner
        .read_agent_files_blocking(AgentFileReadRequest::List)
        .is_err());
    assert!(!drain.is_finished());
    release_tx.send(()).unwrap();
    assert!(worker.join().unwrap().is_err());
    drain.join().unwrap();
    assert_eq!(f.owner.control().availability, SettingsAvailability::Closed);
    assert_eq!(f.owner.control().operations, 0);
}

#[cfg(unix)]
#[test]
fn e2432_p02_owner_identity_loss_invalidates_catalog_authority() {
    let temp = tempfile::tempdir().unwrap();
    let instance = temp.path().join("instance");
    std::fs::create_dir(&instance).unwrap();
    let path = instance.join(SETTINGS_FILE_NAME);
    write(
        &path,
        &serde_json::to_value(AppSettings::default()).unwrap(),
    );
    let owner = SettingsOwner::open_active(&path, Duration::from_secs(1)).unwrap();
    let context =
        SourceReadContext::from_settings(&owner.blocking_current_snapshot().unwrap(), &path)
            .unwrap();
    let mut source = context.source(SourceKind::CatalogPersonal);
    source.scope = SourceScope::Project;
    std::fs::rename(&instance, temp.path().join("old-instance")).unwrap();
    std::fs::create_dir(&instance).unwrap();
    write(
        &path,
        &serde_json::to_value(AppSettings::default()).unwrap(),
    );
    let result = owner.save_catalog_source_blocking(
        source,
        SourceRevision::Absent {},
        context.revision,
        AgentFileEdits::default(),
    );
    assert!(result.is_err());
    assert_eq!(owner.control().availability, SettingsAvailability::Invalid);
}

#[cfg(windows)]
#[test]
fn e2432_p02_simulated_windows_lease_failure_invalidates_catalog_authority() {
    let f = fixture();
    let owner = Arc::clone(&f.owner);
    let source = f.source.clone();
    let expected = f.snapshot.revision.clone();
    let context = f.snapshot.context_revision.clone();
    let paths = [
        f.path.clone(),
        agents_instance_path(&f.path),
        project_paths_path(&f.path),
        f.path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
        f.catalog.clone(),
    ];
    let bytes: Vec<_> = paths
        .into_iter()
        .map(|path| {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("fixture participant capture failed: {error}"),
            };
            (path, bytes)
        })
        .collect();
    std::thread::spawn(move || {
        let epoch = owner.identity().epoch;
        let ram = serde_json::to_value(owner.settings.blocking_read().clone()).unwrap();
        let counters = [
            owner
                .counters
                .transaction_lock_attempts
                .load(Ordering::Relaxed),
            owner.counters.captures.load(Ordering::Relaxed),
            owner.counters.source_reads.load(Ordering::Relaxed),
            owner.counters.operations.load(Ordering::Relaxed),
        ];
        let path = {
            let lease = owner.lease_guard();
            lease.as_ref().unwrap().settings_path.clone()
        };
        // Simulates the verifier error, never Windows physical substitution.
        let fault = SettingsOwnerLeaseVerifyFaultGuard::install(path);
        let hits = std::rc::Rc::clone(&fault.hits);
        let result = owner.save_catalog_source_blocking(
            source.clone(),
            expected.clone(),
            context.clone(),
            AgentFileEdits::default(),
        );
        assert!(matches!(result, Err(reason) if reason == "ownerIntegrityLost"));
        assert_eq!(
            hits.get(),
            1,
            "first operation_guard verify must consume fault"
        );
        assert_simulated_lease_failure_preserved(&owner, epoch, &ram, counters, bytes);
        assert!(owner.blocking_current_snapshot().is_err());
        assert!(owner
            .read_agent_files_blocking(AgentFileReadRequest::List)
            .is_err());
        assert!(owner
            .save_catalog_source_blocking(source, expected, context, AgentFileEdits::default(),)
            .is_err());
        // One-shot consumption leaves the real retained directory verifier intact.
        {
            let lease = owner.lease_guard();
            assert!(lease.as_ref().unwrap().verify().is_ok());
        }
        assert_eq!(hits.get(), 1);
        drop(fault);
        SETTINGS_OWNER_LEASE_VERIFY_FAULT.with(|slot| assert!(slot.borrow().is_none()));
        assert_eq!(hits.get(), 1, "witness survives guard cleanup");
        let healthy = fixture();
        let committed = save(&healthy);
        assert_eq!(committed.outcome, AgentFileSaveOutcome::Committed);
        assert_eq!(healthy.owner.availability(), SettingsAvailability::Ready);
        assert_eq!(owner.availability(), SettingsAvailability::Invalid);
        assert_eq!(owner.control().operations, 0);
        assert_eq!(owner.control().permits, 0);
    })
    .join()
    .unwrap();
}

#[test]
fn e2432_p02_new_catalog_row_requires_complete_owned_fields() {
    let f = fixture();
    let before = std::fs::read(&f.catalog).unwrap();
    let incomplete: AgentFileEdits =
        serde_json::from_value(json!({"rows":[{"entryHint":"added","ac\
        tion":"upsert","patch":{"label":"Incomplete"}}],"profileSets":[]}))
        .unwrap();
    let rejected = f
        .owner
        .save_catalog_source_blocking(
            f.source.clone(),
            f.snapshot.revision.clone(),
            f.snapshot.context_revision.clone(),
            incomplete,
        )
        .unwrap();
    assert_eq!(rejected.outcome, AgentFileSaveOutcome::Rejected);
    assert_eq!(std::fs::read(&f.catalog).unwrap(), before);
    let complete: AgentFileEdits =
        serde_json::from_value(json!({"rows":[{"entryHint":"added","acti\
        on":"upsert","patch":{"label":"Added","description":"Own","color":"blue","baseCommand":"cat\
        alog-tool"}}],"profileSets":[]}))
        .unwrap();
    let committed = f
        .owner
        .save_catalog_source_blocking(
            f.source,
            f.snapshot.revision,
            f.snapshot.context_revision,
            complete,
        )
        .unwrap();
    assert_eq!(committed.outcome, AgentFileSaveOutcome::Committed);
    let snapshot = committed.snapshot.unwrap();
    assert_eq!(snapshot.rows.len(), 2);
    assert_eq!(snapshot.rows[1].entry_hint, "added");
    assert_eq!(
        snapshot.rows[1].base_hash,
        Some(configuration_identity("catalog-tool", &[]))
    );
    let stored: Value = serde_json::from_slice(&std::fs::read(&f.catalog).unwrap()).unwrap();
    assert_eq!(stored["agents"][0]["unknown"], json!([null]));
    assert_eq!(stored["extension"], json!({"keep":true}));
}

#[test]
fn e2432_p02_selected_catalog_raw_gates_reject_before_publish() {
    let invalids = vec![
        br#"{"schemaVersion":1,"agents":[],"unknown":1,"unknown":2}"#.to_vec(),
        br#"{"schemaVersion":1,"agents":[],"legacyProfileOrphans":null}"#.to_vec(),
        include_bytes!("fixtures/e2432_two_tool_agents.json").to_vec(),
        br#"{"schemaVersion":1,"agents":[],"managed":{}}"#.to_vec(),
    ];
    for bytes in invalids {
        let f = fixture();
        std::fs::write(&f.catalog, &bytes).unwrap();
        let epoch = f.owner.control().epoch;
        let rejected = f
            .owner
            .save_catalog_source_blocking(
                f.source,
                source_bytes_revision(&bytes),
                f.snapshot.context_revision,
                AgentFileEdits::default(),
            )
            .unwrap();
        assert_eq!(rejected.outcome, AgentFileSaveOutcome::Rejected);
        assert_eq!(std::fs::read(&f.catalog).unwrap(), bytes);
        assert_eq!(f.owner.control().epoch, epoch);
        assert_eq!(f.owner.control().availability, SettingsAvailability::Ready);
    }
    let f = fixture();
    let before = std::fs::read(&f.catalog).unwrap();
    let mut forged = f.source.clone();
    forged.context_root.push_str("-forged");
    assert_eq!(
        f.owner
            .save_catalog_source_blocking(
                forged,
                f.snapshot.revision.clone(),
                f.snapshot.context_revision.clone(),
                AgentFileEdits::default()
            )
            .unwrap()
            .outcome,
        AgentFileSaveOutcome::Rejected
    );
    assert_eq!(
        f.owner
            .save_catalog_source_blocking(
                f.source.clone(),
                SourceRevision::Bytes {
                    sha256: "0".repeat(64)
                },
                f.snapshot.context_revision.clone(),
                AgentFileEdits::default()
            )
            .unwrap()
            .outcome,
        AgentFileSaveOutcome::Rejected
    );
    assert_eq!(std::fs::read(&f.catalog).unwrap(), before);
    std::fs::remove_file(&f.catalog).unwrap();
    assert_eq!(save(&f).outcome, AgentFileSaveOutcome::Rejected);
    assert!(!f.catalog.exists());
}
#[test]
fn e2432_p02_malformed_optional_preferences_preserve_ordinary_settings() {
    for invalid in [
        json!({"picker":{"bogus":1}}),
        json!("wrong-type"),
        json!({"unknownMenu":{}}),
        json!({"settings":null,"picker":false}),
    ] {
        let case = MalformedPreferencesCase::new(invalid);
        let (loaded, original) = case.assert_ordinary_loads();
        let source = case.assert_strict_owner_refusals(&original);
        case.assert_save_and_roundtrip(&loaded, source);
    }
}
