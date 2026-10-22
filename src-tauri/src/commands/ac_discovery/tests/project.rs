use super::*;

#[cfg(windows)]
#[test]
fn projected_path_string_converts_verbatim_unc() {
    assert_eq!(
        projected_path_string(Path::new(r"\\?\UNC\server\share\repo")),
        r"\\server\share\repo"
    );
}

#[tokio::test]
async fn check_project_path_accepts_ac() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(tmp.path().join(".ac")).expect("create .ac");

    assert!(check_project_path(tmp.path().to_string_lossy().to_string())
        .await
        .expect("check path"));
}

#[tokio::test]
async fn check_project_path_rejects_non_ac_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(tmp.path().join(".workspace")).expect("create non-AC workspace");

    assert!(
        !check_project_path(tmp.path().to_string_lossy().to_string())
            .await
            .expect("check path")
    );
}

#[tokio::test]
async fn create_ac_project_creates_ac() {
    let tmp = tempfile::tempdir().expect("tempdir");

    create_ac_project(tmp.path().to_string_lossy().to_string())
        .await
        .expect("create project");

    assert!(tmp.path().join(".ac").is_dir());
    assert!(tmp.path().join(".ac").join(".gitignore").is_file());
    assert!(tmp
        .path()
        .join(".ac")
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
    assert!(tmp
        .path()
        .join(".ac")
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
    assert!(!tmp.path().join(".ac").join("templates").exists());
}

// #1065 Stage F activation coverage: the direct fresh-root `create_ac_project`
// path records its two project context templates into the seed manifest under the
// gate held since acquisition. Removing the recording closure (the adapter) would
// leave no manifest and fail this test (plan acceptance items 22/38).
#[test]
fn create_ac_project_fresh_root_records_context_templates() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("Proj");
    let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
    create_ac_project_impl(
        &project.to_string_lossy(),
        Some(&token),
        |ac_root, on_publication| {
            crate::config::session_context::create_default_context_templates_with_publications(
                ac_root,
                on_publication,
            )
        },
    )
    .expect("create_ac_project fresh root");
    let manifest = std::fs::read_to_string(project.join(".ac").join("seed-manifest.toml"))
        .expect("fresh create_ac_project records the project context templates");
    assert!(
        manifest.contains("scope = \"context:agentscommander\""),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("scope = \"context:coordinator\""),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("kind = \"project_context_template\""),
        "manifest: {manifest}"
    );
}

// #1065 Stage F activation coverage: an explicit context-template overwrite
// records its commit-point publication into the seed manifest under the gate.
// Removing the `record_project_context_publication` call from
// `overwrite_context_template_recorded` (the adapter) would leave no manifest and
// fail this test (plan acceptance item 22).
#[test]
fn overwrite_context_template_records_under_the_gate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path();
    let ac_root = project.join(".ac");
    std::fs::create_dir(&ac_root).expect("create workspace");
    let filename = crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME;
    std::fs::write(ac_root.join(filename), "custom coordinator guidance")
        .expect("write custom coordinator");
    let update = crate::config::seeded_context_templates::scan_project_context_template_updates(
        project, &ac_root,
    )
    .expect("scan updates")
    .pop()
    .expect("pending overwrite update");

    let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
    overwrite_context_template_recorded(
        project,
        &ac_root,
        &update.filename,
        &update.current_file_sha256,
        &update.current_default_sha256,
        Some(&token),
    )
    .expect("overwrite context template");

    let manifest = std::fs::read_to_string(ac_root.join("seed-manifest.toml"))
        .expect("an explicit overwrite records a manifest row under the gate");
    assert!(
        manifest.contains("scope = \"context:coordinator\""),
        "manifest: {manifest}"
    );
}

#[tokio::test]
async fn create_ac_project_preserves_partial_root_for_registration_repair() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().to_string_lossy().to_string();

    let first_result = create_ac_project_impl(&path, None, |ac_root, _on_publication| {
        std::fs::write(
            ac_root.join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            crate::config::session_context::get_default_agent_template(),
        )
        .expect("write partial agent context");
        Err("injected seed failure".to_string())
    });

    assert!(first_result.is_err());
    assert!(
        tmp.path().join(".ac").exists(),
        "fresh partial .ac directory must remain after seed failure"
    );

    create_ac_project(path.clone())
        .await
        .expect("bare retry keeps the existing project");

    assert!(tmp
        .path()
        .join(".ac")
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
    assert!(!tmp
        .path()
        .join(".ac")
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
        .exists());

    let mut settings = crate::config::settings::AppSettings::default();
    crate::config::projects::register_new_project(&mut settings, &path)
        .expect("registration repairs the partial project");
    assert!(tmp
        .path()
        .join(".ac")
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
}

#[tokio::test]
async fn create_ac_project_does_not_backfill_existing_ac_contexts() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");

    create_ac_project(tmp.path().to_string_lossy().to_string())
        .await
        .expect("create project");

    assert!(!ac_root
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .exists());
    assert!(!ac_root
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
        .exists());
}

#[test]
fn create_ac_project_creation_intent_comes_only_from_the_create_dir_winner() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().to_string_lossy().to_string();
    let creator_path = path.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let creator = std::thread::spawn(move || {
        create_ac_project_impl_with_hook(
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
                    .expect("release creator");
            },
        )
    });

    ready_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("creator published .ac");
    create_ac_project_impl(&path, None, |_, _on_publication| {
        panic!("create_dir loser must not backfill context templates")
    })
    .expect("contending bare create");
    assert!(!tmp
        .path()
        .join(".ac")
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .exists());

    release_tx.send(()).expect("release creator");
    creator
        .join()
        .expect("join creator")
        .expect("creator completes");
    assert!(tmp
        .path()
        .join(".ac")
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
}

#[tokio::test]
async fn missing_sources_discard_stale_live_lists_and_sidecar_survives_absent_settings() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    assert!(!settings_path.exists(), "precondition: no settings.json");

    fn base(paths: &[String]) -> Vec<&str> {
        paths
            .iter()
            .map(|p| p.rsplit(['/', '\\']).next().unwrap())
            .collect()
    }

    let mk = |name: &str| {
        let path = temp.path().join(name);
        std::fs::create_dir_all(path.join(".ac")).expect("create project .ac");
        path.to_string_lossy().to_string()
    };
    let (a, b, c) = (mk("proj-a"), mk("proj-b"), mk("proj-c"));

    let settings = crate::config::settings::AppSettings {
        project_paths: vec![a.clone(), b.clone()],
        project_path: Some(a.clone()),
        ..Default::default()
    };
    let state: SettingsState = crate::config::settings::new_settings_state(settings);

    let disk = || -> Vec<String> {
        let value: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(settings_path.with_file_name("project-paths.json")).unwrap(),
        )
        .unwrap();
        value["projectPaths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect()
    };

    remove_project_inner_with_settings_path(&state, &a, Some(&settings_path))
        .await
        .expect("remove");
    let memory = state.read().await.project_paths.clone();
    assert_eq!(
        base(&memory),
        Vec::<&str>::new(),
        "Absent sources cannot resurrect stale B"
    );
    assert_eq!(
        base(&disk()),
        Vec::<&str>::new(),
        "Absent sources begin empty"
    );

    open_project_inner_with_settings_path(&state, &c, Some(&settings_path))
        .await
        .expect("open");
    let memory = state.read().await.project_paths.clone();
    assert_eq!(base(&memory), vec!["proj-c"]);
    assert_eq!(base(&disk()), vec!["proj-c"]);

    assert!(!settings_path.exists());
    let d_path = temp.path().join("proj-d");
    let d = d_path.to_string_lossy().to_string();
    new_project_inner_with_settings_path(&state, &d, Some(&settings_path))
        .await
        .expect("new project");
    assert!(d_path.join(".ac").is_dir());
    let memory = state.read().await.project_paths.clone();
    assert_eq!(base(&memory), vec!["proj-c", "proj-d"]);
    assert_eq!(
        base(&disk()),
        vec!["proj-c", "proj-d"],
        "Sidecar C survives absent managed settings"
    );
}

#[tokio::test]
async fn new_project_settings_save_failure_keeps_live_state_and_valid_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("project");
    let project_text = project.to_string_lossy().to_string();
    let state: SettingsState = crate::config::settings::new_settings_state(AppSettings::default());
    let hooks = NewProjectTransactionHooks {
        fail_settings_save: true,
        ..NewProjectTransactionHooks::default()
    };

    let error = new_project_inner_with_settings_path_and_hooks(
        &state,
        &project_text,
        Some(&settings_path),
        hooks,
    )
    .await
    .expect_err("injected save failure must surface");

    assert!(error.contains("injected new-project settings save failure"));
    assert!(state.read().await.project_paths.is_empty());
    assert!(!settings_path.exists());
    assert!(project
        .join(".ac")
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
    assert!(project
        .join(".ac")
        .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
        .is_file());
}

#[cfg(unix)]
#[tokio::test]
async fn new_project_replacement_after_save_rolls_back_registration_and_is_retryable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("project");
    let project_text = project.to_string_lossy().to_string();
    let state: SettingsState = crate::config::settings::new_settings_state(AppSettings::default());
    let hooks = NewProjectTransactionHooks {
        before_final_revalidation: Some(std::sync::Arc::new(|ac_root| {
            let detached = ac_root.with_extension("ac-detached-after-save");
            std::fs::rename(ac_root, &detached).expect("detach prepared root");
            std::fs::create_dir(ac_root).expect("install replacement root");
            std::fs::write(ac_root.join("foreign.txt"), b"FOREIGN")
                .expect("write replacement marker");
        })),
        ..NewProjectTransactionHooks::default()
    };

    let error = new_project_inner_with_settings_path_and_hooks(
        &state,
        &project_text,
        Some(&settings_path),
        hooks,
    )
    .await
    .expect_err("replacement must reject registration");

    assert!(error.contains("retry the operation"));
    assert!(state.read().await.project_paths.is_empty());
    let disk: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(settings_path.with_file_name("project-paths.json")).unwrap(),
    )
    .unwrap();
    assert!(disk["projectPaths"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read(project.join(".ac").join("foreign.txt")).unwrap(),
        b"FOREIGN"
    );
    assert!(!project
        .join(".ac")
        .join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME)
        .exists());
}

#[cfg(windows)]
#[tokio::test]
async fn new_project_lock_replacement_after_save_rolls_back_registration_and_is_retryable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let settings_path = temp.path().join("settings.json");
    let project = temp.path().join("project");
    let project_text = project.to_string_lossy().to_string();
    let state: SettingsState = crate::config::settings::new_settings_state(AppSettings::default());
    let hooks = NewProjectTransactionHooks {
        before_final_revalidation: Some(std::sync::Arc::new(|ac_root| {
            let lock = ac_root.join(crate::config::seed_manifest::SEED_MANIFEST_LOCK_FILENAME);
            let detached = ac_root.join(".seed-manifest.lock-detached-after-save");
            std::fs::rename(&lock, &detached).expect("detach held lock identity");
            std::fs::write(&lock, b"").expect("install replacement lock identity");
        })),
        ..NewProjectTransactionHooks::default()
    };

    let error = new_project_inner_with_settings_path_and_hooks(
        &state,
        &project_text,
        Some(&settings_path),
        hooks,
    )
    .await
    .expect_err("lock replacement must reject registration");

    assert!(error.contains("retry the operation"));
    assert!(state.read().await.project_paths.is_empty());
    let disk: AppSettings =
        crate::config::settings::load_settings_from_path(std::path::Path::new(&settings_path));
    assert!(disk.project_paths.is_empty());
    assert!(project
        .join(".ac")
        .join(".seed-manifest.lock-detached-after-save")
        .is_file());
}

#[tokio::test]
async fn mutate_save_failure_does_not_publish_unsaved_candidate() {
    // Grinch Defect 3: on an atomic-save failure the live state must remain
    // the refreshed pre-mutation state, never the unsaved mutated candidate.
    let temp = tempfile::tempdir().expect("tempdir");
    // A regular FILE where the settings directory is expected, so the writer's
    // create_dir_all (or the pre-mutation read) fails deterministically.
    let blocker = temp.path().join("blocker");
    std::fs::write(&blocker, b"x").expect("write blocker file");
    let settings_path = blocker.join("settings.json");
    let mk = |name: &str| {
        let p = temp.path().join(name);
        std::fs::create_dir_all(p.join(".ac")).expect("create project .ac");
        p.to_string_lossy().to_string()
    };
    let (x, y) = (mk("keep-x"), mk("new-y"));
    let settings = crate::config::settings::AppSettings {
        project_paths: vec![x.clone()],
        project_path: Some(x.clone()),
        ..Default::default()
    };
    let state: SettingsState = crate::config::settings::new_settings_state(settings);

    let err = open_project_inner_with_settings_path(&state, &y, Some(&settings_path))
        .await
        .expect_err("save into a file-blocked directory must fail");
    assert!(!err.is_empty());

    let live = state.read().await;
    let names: Vec<&str> = live
        .project_paths
        .iter()
        .map(|p| {
            std::path::Path::new(p)
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
        })
        .collect();
    assert_eq!(
        names,
        vec!["keep-x"],
        "the unsaved candidate ([keep-x, new-y]) must not leak into the live state"
    );
}

#[tokio::test]
async fn keep_custom_context_template_rejects_workspace_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    std::fs::create_dir(&ac_root).expect("create .ac");

    let err = keep_custom_context_template(
        ac_root.to_string_lossy().to_string(),
        crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME.to_string(),
        "file".to_string(),
        "default".to_string(),
    )
    .await
    .expect_err("workspace path must not be accepted as project path");

    assert!(err.contains("Project AC Root not found"));
}
