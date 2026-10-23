use super::*;

#[test]
fn open_rejects_empty_path() {
    let mut s = AppSettings::default();
    assert!(matches!(
        register_existing_project(&mut s, ""),
        Err(ProjectError::EmptyPath(_))
    ));
}

#[test]
fn open_rejects_missing_path() {
    let fix = FixtureRoot::new("proj-open-missing");
    let p = fix.path().join("does-not-exist");
    let mut s = AppSettings::default();
    let r = register_existing_project(&mut s, p.to_str().unwrap());
    assert!(matches!(r, Err(ProjectError::PathMissing(_))));
    assert!(s.project_paths.is_empty());
}

#[test]
fn open_rejects_path_without_ac_root() {
    let fix = FixtureRoot::new("proj-open-no-workspace");
    let mut s = AppSettings::default();
    let r = register_existing_project(&mut s, fix.path().to_str().unwrap());
    assert!(matches!(r, Err(ProjectError::AcRootMissing(_))));
    assert!(s.project_paths.is_empty());
}

#[test]
fn open_registers_path_with_ac() {
    let fix = FixtureRoot::new("proj-open-ok");
    std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
    let mut s = AppSettings::default();
    let r = register_existing_project(&mut s, fix.path().to_str().unwrap()).unwrap();
    assert!(r.registered);
    assert!(!r.created);
    assert_eq!(s.project_paths.len(), 1);
    assert_eq!(s.project_path.as_deref(), Some(r.path.as_str()));
}

#[test]
fn open_rejects_non_ac_root() {
    let fix = FixtureRoot::new("proj-open-invalid-workspace");
    std::fs::create_dir_all(fix.path().join(".workspace")).unwrap();
    let mut s = AppSettings::default();
    let r = register_existing_project(&mut s, fix.path().to_str().unwrap());
    assert!(matches!(r, Err(ProjectError::AcRootMissing(_))));
    assert!(s.project_paths.is_empty());
}

#[test]
fn open_registers_path_when_ac_exists() {
    let fix = FixtureRoot::new("proj-open-both");
    std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
    let mut s = AppSettings::default();
    let r = register_existing_project(&mut s, fix.path().to_str().unwrap()).unwrap();
    assert!(r.registered);
    assert_eq!(existing_ac_root(fix.path()), Some(fix.path().join(".ac")));
}

#[test]
fn open_is_idempotent_on_repeat_call() {
    let fix = FixtureRoot::new("proj-open-idem");
    std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
    let mut s = AppSettings::default();
    let _ = register_existing_project(&mut s, fix.path().to_str().unwrap()).unwrap();
    let r2 = register_existing_project(&mut s, fix.path().to_str().unwrap()).unwrap();
    assert!(!r2.registered);
    assert_eq!(s.project_paths.len(), 1);
}

#[test]
fn open_dedup_is_case_insensitive_and_slash_agnostic() {
    let fix = FixtureRoot::new("proj-open-norm");
    std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
    let mut s = AppSettings::default();
    // Seed an entry with the exact path
    let original = fix.path().to_string_lossy().to_string();
    let _ = register_existing_project(&mut s, &original).unwrap();
    // Re-register with mixed slash + case
    let mangled = original.replace('\\', "/").to_uppercase();
    let r2 = register_existing_project(&mut s, &mangled).unwrap();
    assert!(!r2.registered, "case+slash variant should dedup");
    assert_eq!(s.project_paths.len(), 1);
    // Original retained, NOT replaced with the mangled form.
    assert_eq!(s.project_paths[0], original);
}

// ── register_new_project ─────────────────────────────────────────────

#[test]
fn new_creates_ac_when_missing() {
    let fix = FixtureRoot::new("proj-new-mkdir");
    let mut s = AppSettings::default();
    let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
    assert!(r.created);
    assert!(r.registered);
    assert!(fix.path().join(".ac").is_dir());
    assert!(fix.path().join(".ac").join(".gitignore").is_file());
    assert!(fix
        .path()
        .join(".ac")
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
    assert!(fix
        .path()
        .join(".ac")
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
    assert!(!fix.path().join(".ac").join("templates").exists());
}

#[test]
fn new_creates_parent_directory_when_missing() {
    // Covers the `create_dir_all(&abs)` branch in register_new_project
    // for a path whose project folder does NOT yet exist on disk. The
    // existing `new_creates_ac_when_missing` test passes `fix.path()`
    // which `FixtureRoot::new` already created, so the parent-mkdir
    // branch was previously unexercised.
    let fix = FixtureRoot::new("proj-new-parent");
    let nested = fix.path().join("nested-not-yet-created");
    let mut s = AppSettings::default();
    let r = register_new_project(&mut s, nested.to_str().unwrap()).unwrap();
    assert!(r.created, "should report created=true for fresh path");
    assert!(r.registered);
    assert!(nested.is_dir(), "project root should have been created");
    assert!(nested.join(".ac").is_dir());
    assert!(nested.join(".ac").join(".gitignore").is_file());
}

#[test]
fn new_skips_creation_when_ac_already_exists() {
    let fix = FixtureRoot::new("proj-new-existing");
    std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
    let mut s = AppSettings::default();
    let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
    assert!(!r.created);
    assert!(r.registered);
    // gitignore swept opportunistically even though .ac pre-existed
    assert!(fix.path().join(".ac").join(".gitignore").is_file());
}

#[test]
fn new_does_not_overwrite_existing_context_templates() {
    let fix = FixtureRoot::new("proj-new-template-existing");
    let ac_root = fix.path().join(".ac");
    std::fs::create_dir_all(&ac_root).unwrap();
    let agent_template = ac_root.join("Context.AgentsCommander.md");
    let coordinator_template = ac_root.join("Context.coordinator.md");
    std::fs::write(&agent_template, "CUSTOM_AGENT").unwrap();
    std::fs::write(&coordinator_template, "CUSTOM_COORDINATOR").unwrap();

    let mut s = AppSettings::default();
    let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();

    assert!(!r.created);
    assert_eq!(
        std::fs::read_to_string(agent_template).unwrap(),
        "CUSTOM_AGENT"
    );
    assert_eq!(
        std::fs::read_to_string(coordinator_template).unwrap(),
        "CUSTOM_COORDINATOR"
    );
}

#[test]
fn new_registration_repairs_a_failed_fresh_context_seed() {
    let fix = FixtureRoot::new("proj-new-template-retry");
    let mut s = AppSettings::default();

    let first_result = prepare_new_project_impl(
        fix.path().to_str().unwrap(),
        None,
        |ac_root, _on_publication| {
            std::fs::write(
                ac_root.join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME),
                crate::config::session_context::get_default_agent_template(),
            )
            .expect("write partial agent context");
            Err("injected seed failure".to_string())
        },
    );

    assert!(matches!(
        first_result,
        Err(ProjectError::ContextTemplatesCreateFailed(_, _))
    ));
    assert!(
        fix.path().join(".ac").exists(),
        "fresh partial .ac directory must remain truthful after seed failure"
    );
    assert!(s.project_paths.is_empty());

    let retry = register_new_project(&mut s, fix.path().to_str().unwrap())
        .expect("retry register new project");

    assert!(!retry.created);
    assert!(retry.registered);
    assert!(fix
        .path()
        .join(".ac")
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
    assert!(fix
        .path()
        .join(".ac")
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
}

#[test]
fn new_project_contender_can_complete_on_the_same_visible_root_before_creator_gate() {
    let fix = FixtureRoot::new("proj-new-contender");
    let path = fix.path().to_string_lossy().to_string();
    let creator_path = path.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let creator = std::thread::spawn(move || {
        let prepared = prepare_new_project_impl_with_hook(
            &creator_path,
            None,
            |ac_root, _on_publication| {
                crate::config::session_context::create_default_context_templates(ac_root)
            },
            move |_, created| {
                assert!(created, "creator must own the create_dir win");
                ready_tx.send(()).expect("signal visible root");
                release_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("release creator gate acquisition");
            },
        )
        .expect("creator preparation");
        let created = prepared.created;
        prepared.release();
        created
    });

    ready_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("creator published .ac");
    let mut contender_settings = AppSettings::default();
    let contender = register_new_project(&mut contender_settings, &path)
        .expect("contender completes shared-root setup");
    assert!(!contender.created);
    assert!(contender.registered);
    assert!(fix
        .path()
        .join(".ac")
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
    assert!(fix
        .path()
        .join(".ac")
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
        .is_file());

    release_tx.send(()).expect("release creator");
    assert!(creator.join().expect("join creator"));
    assert!(fix.path().join(".ac").is_dir());
}

#[test]
fn new_project_creator_failure_after_gate_preserves_contender_output_and_root() {
    // Plan acceptance item 14 (lines 654/765): after the contender acquires/
    // releases the same in-root gate and publishes into the unchanged identity,
    // a creator that fails AFTER acquiring its own gate must not delete the root
    // or the contender's output and must register nothing; a later registration
    // then repairs from the truthful partial state. This directly exercises the
    // plan-line-1214 no-deletion fix in the same-identity contender race
    // (creation intent is revalidation data, never recursive-deletion authority).
    let fix = FixtureRoot::new("proj-new-contender-creator-fails");
    let path = fix.path().to_string_lossy().to_string();
    let creator_path = path.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let creator = std::thread::spawn(move || {
        // The template closure runs post-acquisition (projects.rs create path),
        // so returning Err here is a deterministic "creator fails after its own
        // gate acquisition". It writes nothing, so the contender's published
        // templates must be left untouched. Reduce to a Send-safe result.
        match prepare_new_project_impl_with_hook(
            &creator_path,
            None,
            |_ac_root, _on_publication| Err("injected creator failure after gate".to_string()),
            move |_, created| {
                assert!(created, "creator must own the create_dir win");
                ready_tx.send(()).expect("signal visible root");
                release_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("release creator gate acquisition");
            },
        ) {
            Ok(prepared) => {
                prepared.release();
                Ok(())
            }
            Err(error) => Err(error),
        }
    });

    // Contender uses the same unchanged root while the creator is paused before
    // its own acquisition; it acquires/releases the in-root gate and publishes
    // both templates.
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("creator published .ac");
    let mut contender_settings = AppSettings::default();
    let contender = register_new_project(&mut contender_settings, &path)
        .expect("contender completes shared-root setup");
    assert!(!contender.created);
    assert!(contender.registered);
    let agent_template = fix
        .path()
        .join(".ac")
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    let coordinator_template = fix
        .path()
        .join(".ac")
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
    assert!(agent_template.is_file());
    assert!(coordinator_template.is_file());
    let agent_bytes = std::fs::read(&agent_template).expect("read contender agent template");
    let coordinator_bytes =
        std::fs::read(&coordinator_template).expect("read contender coordinator template");

    // Release the creator; it acquires the now-free gate and then fails.
    release_tx.send(()).expect("release creator");
    let creator_result = creator.join().expect("join creator");
    assert!(
        matches!(
            creator_result,
            Err(ProjectError::ContextTemplatesCreateFailed(_, _))
        ),
        "creator must fail after gate acquisition, got {creator_result:?}"
    );

    // Neither the root nor the contender's output was deleted by the failed creator,
    // and the creator registered no incomplete setup.
    assert!(
        fix.path().join(".ac").is_dir(),
        "root must survive creator failure"
    );
    assert!(
        agent_template.is_file(),
        "contender agent template must survive creator failure"
    );
    assert!(
        coordinator_template.is_file(),
        "contender coordinator template must survive creator failure"
    );
    assert_eq!(
        std::fs::read(&agent_template).expect("re-read agent template"),
        agent_bytes,
        "contender agent template bytes must be untouched"
    );
    assert_eq!(
        std::fs::read(&coordinator_template).expect("re-read coordinator template"),
        coordinator_bytes,
        "contender coordinator template bytes must be untouched"
    );

    // A later registration re-runs template ensure under the gate and completes
    // from the truthful partial state (the failed creator registered nothing).
    let mut repair_settings = AppSettings::default();
    let repair = register_new_project(&mut repair_settings, &path)
        .expect("later registration repairs from truthful partial state");
    assert!(!repair.created);
    assert!(repair.registered);
}

#[test]
fn new_project_rejects_a_replaced_root_before_gate_without_touching_replacement() {
    let fix = FixtureRoot::new("proj-new-replaced-root");
    let replacement_marker = fix.path().join(".ac").join("foreign.txt");

    let result = prepare_new_project_impl_with_hook(
        fix.path().to_str().unwrap(),
        None,
        |ac_root, _on_publication| {
            crate::config::session_context::create_default_context_templates(ac_root)
        },
        |ac_root, created| {
            assert!(created);
            let detached = ac_root.with_extension("ac-detached");
            std::fs::rename(ac_root, &detached).expect("detach created root");
            std::fs::create_dir(ac_root).expect("install replacement root");
            std::fs::write(ac_root.join("foreign.txt"), b"FOREIGN")
                .expect("write replacement marker");
        },
    );

    assert!(matches!(
        result,
        Err(ProjectError::ProjectSetupChanged(_, _))
    ));
    assert_eq!(std::fs::read(&replacement_marker).unwrap(), b"FOREIGN");
    assert!(!fix
        .path()
        .join(".ac")
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .exists());
    assert!(!fix.path().join(".ac").join(".seed-manifest.lock").exists());
}

#[test]
fn synchronous_new_project_refreshes_and_saves_disk_authoritative_settings() {
    let fix = FixtureRoot::new("proj-new-sync-settings");
    let settings_path = fix.path().join("settings.json");
    std::fs::create_dir_all(fix.path().join("disk-only/.ac")).unwrap();
    let disk_only = fix.path().join("disk-only").to_string_lossy().to_string();
    let mut disk_settings = AppSettings {
        project_paths: vec![disk_only.clone()],
        project_path: Some(disk_only.clone()),
        ..AppSettings::default()
    };
    crate::config::settings::save_settings_with_project_paths_to_path(
        &disk_settings,
        &settings_path,
    )
    .expect("seed disk settings");
    disk_settings.project_paths = vec!["stale-memory".to_string()];
    disk_settings.project_path = Some("stale-memory".to_string());
    let project = fix.path().join("new-project");

    let result = register_new_project_with_settings_path(
        &mut disk_settings,
        project.to_str().unwrap(),
        &settings_path,
        None,
    )
    .expect("gated synchronous registration");

    assert!(result.registered);
    let expected_disk_only = super::display_canonical(
        std::fs::canonicalize(&disk_only)
            .unwrap()
            .to_string_lossy()
            .as_ref(),
    );
    let expected_new_project = super::display_canonical(
        std::fs::canonicalize(&result.path)
            .unwrap()
            .to_string_lossy()
            .as_ref(),
    );
    assert_eq!(
        disk_settings.project_paths,
        vec![expected_disk_only, expected_new_project]
    );
    // #2716 (B3): the file no longer decodes as a whole `AppSettings` (its
    // `agents` moved to the agents file), so the written key is read raw.
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(settings_path.with_file_name("project-paths.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        saved["projectPaths"],
        serde_json::json!([disk_only, result.path])
    );
}

#[test]
fn new_backfills_templates_when_ac_already_exists() {
    let fix = FixtureRoot::new("proj-new-template-no-backfill");
    std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
    let mut s = AppSettings::default();

    let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();

    assert!(!r.created);
    assert!(fix
        .path()
        .join(".ac")
        .join("Context.AgentsCommander.md")
        .exists());
    assert!(fix
        .path()
        .join(".ac")
        .join("Context.coordinator.md")
        .exists());
    assert!(!fix.path().join(".ac").join("templates").exists());
}

#[test]
fn new_skips_creation_when_ac_already_exists_via_ac_root_lookup() {
    let fix = FixtureRoot::new("proj-new-existing-lookup");
    std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
    let mut s = AppSettings::default();
    let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
    assert!(!r.created);
    assert!(r.registered);
    assert!(fix.path().join(".ac").is_dir());
}

#[test]
fn new_is_idempotent_for_registration() {
    let fix = FixtureRoot::new("proj-new-idem");
    let mut s = AppSettings::default();
    let _ = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
    let r2 = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
    assert!(!r2.created);
    assert!(!r2.registered);
    assert_eq!(s.project_paths.len(), 1);
}

// Stage E (#1064) clone/no-backfill registration sentinel (plan section 10.3
// item 7, section 12 item 10, acceptance item 28/38): a cloned project
// arrives with its `.ac` already present; registration never recreates it,
// and re-discovery from a fresh settings snapshot neither backfills nor
// duplicates the registration.
#[test]
fn stage_e_registering_a_cloned_project_preserves_ac_and_does_not_duplicate() {
    let fix = FixtureRoot::new("proj-stage-e-clone");
    std::fs::create_dir_all(fix.path().join(".ac")).unwrap();
    let mut settings = AppSettings::default();
    let first = register_new_project(&mut settings, fix.path().to_str().unwrap()).unwrap();
    assert!(!first.created, "an existing .ac is never recreated");
    assert!(first.registered);
    assert!(
        fix.path().join(".ac").is_dir(),
        "the cloned .ac is preserved"
    );

    let mut fresh = AppSettings::default();
    let rediscovered = register_new_project(&mut fresh, fix.path().to_str().unwrap()).unwrap();
    assert!(!rediscovered.created);
    assert_eq!(
        fresh.project_paths.len(),
        1,
        "re-discovering a clone must not duplicate the registration"
    );
    assert!(fix.path().join(".ac").is_dir());
}

#[test]
fn new_rejects_when_path_is_a_regular_file() {
    let fix = FixtureRoot::new("proj-new-file");
    let f = fix.path().join("file.txt");
    std::fs::write(&f, b"x").unwrap();
    let mut s = AppSettings::default();
    let r = register_new_project(&mut s, f.to_str().unwrap());
    assert!(matches!(r, Err(ProjectError::NotADirectory(_))));
    assert!(s.project_paths.is_empty());
}

// ── upsert keeps legacy projectPath in lockstep ───────────────────────
