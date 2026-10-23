use super::*;

fn migration_wait_marker(root: &std::path::Path, name: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !root.join(name).is_file() {
        assert!(
            std::time::Instant::now() < deadline,
            "barrier timed out: {name}; diagnostics {}",
            root.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

struct MigrationChild {
    child: std::process::Child,
    root: std::path::PathBuf,
}

impl MigrationChild {
    fn finish(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                std::fs::write(self.root.join("child-exit.txt"), status.to_string()).unwrap();
                assert!(
                    status.success(),
                    "child failed; diagnostics {}",
                    self.root.display()
                );
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child exit timed out; diagnostics {}",
                self.root.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for MigrationChild {
    fn drop(&mut self) {
        // Cleanup cannot replace a primary assertion/error with a second panic.
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            _ => {
                let killed = self.child.kill();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    let reaped = self.child.try_wait();
                    let done = matches!(&reaped, Ok(Some(_)));
                    if done || std::time::Instant::now() >= deadline {
                        let _ = std::fs::write(
                            self.root.join("child-cleanup.txt"),
                            format!(
                                "kill={killed:?}; reap={reaped:?}; deadline_expired={}",
                                !done
                            ),
                        );
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }
}

fn launch_migration_child(root: &std::path::Path, role: &str) -> MigrationChild {
    launch_migration_child_with_path(root, role, &root.join("settings.json"))
}

fn launch_migration_child_with_path(
    root: &std::path::Path,
    role: &str,
    path: &std::path::Path,
) -> MigrationChild {
    let log = std::fs::File::create(root.join("child.log")).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "config::settings::tests::project_migration_process::project_migration_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("AC_P02_CHILD_ROOT", root)
        .env("AC_P02_CHILD_ROLE", role)
        .env("AC_P03_DATA_PATH", path)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(log.try_clone().unwrap()))
        .stdout(std::process::Stdio::from(log))
        .spawn()
        .unwrap();
    MigrationChild {
        child,
        root: root.to_path_buf(),
    }
}

#[test]
fn project_migration_child() {
    let Some(root) = std::env::var_os("AC_P02_CHILD_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    assert!(root.is_absolute());
    assert_eq!(
        std::fs::read(root.join("isolated-fixture.marker")).unwrap(),
        b"P02 isolated process fixture"
    );
    let role = std::env::var("AC_P02_CHILD_ROLE").unwrap();
    if role.starts_with("race-") {
        project_archive_preserve_child(&root, &role);
        return;
    }
    assert!(matches!(
        role.as_str(),
        "hold" | "crash-backup-sync" | "crash-backup-published" | "crash-destination"
    ));
    std::fs::write(root.join("ready"), std::process::id().to_string()).unwrap();
    migration_wait_marker(&root, "begin");
    let transaction = migration_transaction(&root.join("settings.json"));
    std::fs::write(root.join("acquired"), b"lock owned").unwrap();
    super::super::ensure_project_paths_migrated_with(&transaction, &mut |stage, _| {
        let stop = match role.as_str() {
            "hold" | "crash-destination" => stage == "destination_published",
            "crash-backup-sync" => stage == "backup_sync",
            "crash-backup-published" => stage == "backup_published",
            _ => unreachable!(),
        };
        if stop {
            std::fs::write(root.join("publication"), stage.as_bytes()).unwrap();
            migration_wait_marker(&root, "attempt");
            migration_wait_marker(&root, "release-cleanup");
        }
        Ok(())
    })
    .unwrap();
    std::fs::write(
        root.join("cleanup"),
        b"source cleaned with lock still owned",
    )
    .unwrap();
    migration_wait_marker(&root, "release-lock");
    drop(transaction);
}

// Separate control roots share one data path and actual native settings lock.
fn project_archive_preserve_child(root: &std::path::Path, role: &str) {
    let path = std::path::PathBuf::from(std::env::var_os("AC_P03_DATA_PATH").unwrap());
    assert!(path.is_absolute());
    let data = path.parent().unwrap();
    let draft: super::super::AppSettings =
        serde_json::from_slice(&std::fs::read(data.join("stale-draft.json")).unwrap()).unwrap();
    let target = std::fs::read_to_string(data.join("target.txt")).unwrap();
    let holder = role.ends_with("-holder");
    let preserve = role.starts_with("race-preserve-");
    let unarchive = role.starts_with("race-unarchive-");
    assert!(preserve || unarchive || role.starts_with("race-archive-"));
    std::fs::write(root.join("ready"), b"ready").unwrap();
    migration_wait_marker(root, "begin");
    if preserve && holder {
        let control = root.to_path_buf();
        super::super::SETTINGS_PRESERVE_LOCK_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_| {
                std::fs::write(control.join("acquired"), b"Preserve lock owned").unwrap();
                migration_wait_marker(&control, "release");
            }));
        });
    }
    let operation = || -> Result<(), String> {
        if preserve {
            super::super::save_settings_to_path_preserving_project_paths_typed(&draft, &path)
                .map(|_| ())
                .map_err(|error| {
                    super::super::report_settings_save_error(
                        error,
                        super::super::SettingsSaveReportSurface::GeneralSettings,
                    )
                })
        } else {
            let mut transaction =
                super::super::begin_project_paths_transaction_at_path(&draft, &path)?;
            if holder {
                std::fs::write(root.join("acquired"), b"project token owned").unwrap();
                migration_wait_marker(root, "release");
            }
            let mut candidate = transaction.fresh_settings().clone();
            transaction.refresh_raw_projects(&mut candidate)?;
            if unarchive {
                assert!(super::super::projects::unarchive_project_path(
                    &mut candidate,
                    &target
                ));
            } else {
                assert!(super::super::projects::archive_project_path(
                    &mut candidate,
                    &target
                ));
            }
            super::super::resync_project_state_from_runtime(&mut candidate);
            transaction.commit_project_groups(&candidate, true, true)?;
            Ok(())
        }
    };
    if !holder {
        let source = std::fs::read(&path).unwrap();
        let sidecar = std::fs::read(super::super::project_paths_path(&path)).unwrap();
        let started = std::time::Instant::now();
        let refused = operation().expect_err("held native lock must refuse competing writer");
        assert!(started.elapsed() >= std::time::Duration::from_secs(2));
        assert_eq!(std::fs::read(&path).unwrap(), source);
        assert_eq!(
            std::fs::read(super::super::project_paths_path(&path)).unwrap(),
            sidecar
        );
        std::fs::write(root.join("refused"), refused).unwrap();
        migration_wait_marker(root, "retry");
    }
    let before = std::fs::read(super::super::project_paths_path(&path)).unwrap();
    operation().unwrap();
    if preserve {
        assert_eq!(
            std::fs::read(super::super::project_paths_path(&path)).unwrap(),
            before
        );
    }
    std::fs::write(root.join("done"), b"writer succeeded").unwrap();
}

#[test]
fn project_archive_unarchive_processes_race_typed_preserve_both_orders() {
    for unarchive in [false, true] {
        for preserve_first in [false, true] {
            project_archive_preserve_case(unarchive, preserve_first);
        }
    }
}

fn project_archive_preserve_case(unarchive: bool, preserve_first: bool) {
    let root = tempfile::tempdir().unwrap().keep();
    let path = root.join("settings.json");
    let a = root.join("A").to_string_lossy().replace('\\', "/");
    let b = root.join("B").to_string_lossy().replace('\\', "/");
    // Missing raw registrations exercise retained companion pairs.
    assert!(!root.join("A").exists() && !root.join("B").exists());
    let (active, archived, active_rel, archived_rel) = if unarchive {
        (
            serde_json::json!([b]),
            serde_json::json!([a]),
            serde_json::json!(["missing-companion-B"]),
            serde_json::json!(["missing-companion-A"]),
        )
    } else {
        (
            serde_json::json!([a, b]),
            serde_json::json!([]),
            serde_json::json!(["missing-companion-A", "missing-companion-B"]),
            serde_json::json!([]),
        )
    };
    let mut managed = serde_json::to_value(super::super::AppSettings {
        room_number_mask: "##".into(),
        root_token: Some("fixture".into()),
        ..super::super::AppSettings::default()
    })
    .unwrap();
    super::super::replace_project_projection(
        managed.as_object_mut().unwrap(),
        &serde_json::Map::new(),
    );
    std::fs::write(&path, serde_json::to_vec(&managed).unwrap()).unwrap();
    std::fs::write(
        super::super::project_paths_path(&path),
        serde_json::to_vec(&serde_json::json!({
            "projectPaths":active,"projectPathsRelativeToInstance":active_rel,
            "archivedProjectPaths":archived,
            "archivedProjectPathsRelativeToInstance":archived_rel,
            "projectPath": if unarchive { &b } else { &a },
            "projectPathRelativeToInstance":null,"unknownSidecar":{"sentinel":29}
        }))
        .unwrap(),
    )
    .unwrap();
    let draft = super::super::AppSettings {
        project_paths: vec![a.clone(), b.clone()],
        project_path: Some(a.clone()),
        archived_project_paths: vec![b.clone()],
        room_number_mask: "##".into(),
        root_token: Some("fixture".into()),
        ..super::super::AppSettings::default()
    };
    std::fs::write(
        root.join("stale-draft.json"),
        serde_json::to_vec(&draft).unwrap(),
    )
    .unwrap();
    std::fs::write(root.join("target.txt"), &a).unwrap();
    let holder_root = root.join("holder");
    let contender_root = root.join("contender");
    for control in [&holder_root, &contender_root] {
        std::fs::create_dir(control).unwrap();
        std::fs::write(
            control.join("isolated-fixture.marker"),
            b"P02 isolated process fixture",
        )
        .unwrap();
    }
    let mutation = if unarchive {
        "race-unarchive"
    } else {
        "race-archive"
    };
    let (holder_role, contender_role) = if preserve_first {
        (
            "race-preserve-holder".to_string(),
            format!("{mutation}-contender"),
        )
    } else {
        (
            format!("{mutation}-holder"),
            "race-preserve-contender".to_string(),
        )
    };
    let mut holder = launch_migration_child_with_path(&holder_root, &holder_role, &path);
    let mut contender = launch_migration_child_with_path(&contender_root, &contender_role, &path);
    migration_wait_marker(&holder_root, "ready");
    migration_wait_marker(&contender_root, "ready");
    std::fs::write(holder_root.join("begin"), b"begin").unwrap();
    migration_wait_marker(&holder_root, "acquired");
    let before_settings = std::fs::read(&path).unwrap();
    let before_sidecar = std::fs::read(super::super::project_paths_path(&path)).unwrap();
    std::fs::write(contender_root.join("begin"), b"begin").unwrap();
    migration_wait_marker(&contender_root, "refused");
    assert_eq!(std::fs::read(&path).unwrap(), before_settings);
    assert_eq!(
        std::fs::read(super::super::project_paths_path(&path)).unwrap(),
        before_sidecar
    );
    std::fs::write(holder_root.join("release"), b"release").unwrap();
    holder.finish();
    std::fs::write(contender_root.join("retry"), b"retry").unwrap();
    contender.finish();
    let raw = b3_object(&super::super::project_paths_path(&path));
    assert_eq!(
        raw["projectPaths"],
        if unarchive {
            serde_json::json!([b, a])
        } else {
            serde_json::json!([b])
        }
    );
    assert_eq!(
        raw["archivedProjectPaths"],
        if unarchive {
            serde_json::json!([])
        } else {
            serde_json::json!([a])
        }
    );
    assert_eq!(raw["projectPath"], serde_json::json!(b));
    assert_eq!(raw["projectPathRelativeToInstance"], "missing-companion-B");
    assert_eq!(
        raw["projectPathsRelativeToInstance"],
        if unarchive {
            serde_json::json!(["missing-companion-B", null])
        } else {
            serde_json::json!(["missing-companion-B"])
        }
    );
    assert_eq!(
        raw["archivedProjectPathsRelativeToInstance"],
        if unarchive {
            serde_json::json!([])
        } else {
            serde_json::json!([null])
        }
    );
    assert_eq!(raw["unknownSidecar"], serde_json::json!({"sentinel":29}));
    let managed = b3_object(&path);
    assert_eq!(managed["roomNumberMask"], "##");
    assert!(super::super::raw_project_projection(&managed).is_empty());
    eprintln!(
        "P03 unarchive={unarchive}/Preserve-first={preserve_first} diagnostics {}",
        root.display()
    );
}

// Dormant writer control: same P01 token, migration before the requested
// mutation. Production entry points remain deliberately legacy in P02.
fn migration_fixture_writer(
    path: &std::path::Path,
    variant: &str,
) -> Result<(), super::super::SettingsSaveError> {
    let lock = super::super::SettingsFileLock::acquire(path, std::time::Duration::from_secs(2))?;
    let transaction = super::super::ProjectPathsWriteTransaction {
        _lock: lock,
        path: path.to_path_buf(),
        fresh: super::super::AppSettings::default(),
        revalidated_after_failure: false,
    };
    super::super::ensure_project_paths_migrated_locked(&transaction)?;
    let target = if variant == "Reconcile" {
        super::super::project_paths_path(path)
    } else {
        path.to_path_buf()
    };
    let mut object = super::super::read_raw_project_source(&target)?
        .unwrap()
        .object;
    match variant {
        "Preserve" => {
            object.insert("requestedPreserve".into(), serde_json::json!(true));
        }
        "Reconcile" => {
            object.insert(
                "projectPaths".into(),
                serde_json::json!(["C:/requested-registration"]),
            );
        }
        "CAS" => {
            assert_eq!(object["terminalSnapshotsEnabled"], false);
            object.insert("terminalSnapshotsEnabled".into(), serde_json::json!(true));
        }
        _ => panic!("unknown fixture writer"),
    }
    super::super::write_value_atomic(&serde_json::Value::Object(object), &target)?;
    Ok(())
}

#[test]
fn project_migration_process_timeout_retry_preserve_reconcile_cas() {
    use sha2::{Digest, Sha256};
    for variant in ["Preserve", "Reconcile", "CAS"] {
        let (temp, path, source) = migration_fixture(
            &serde_json::json!({"projectPaths":[],"terminalSnapshotsEnabled":false,"unrelated":9}),
        );
        let root = temp.keep(); // Retain child diagnostics, including failure/crash evidence.
        std::fs::write(
            root.join("isolated-fixture.marker"),
            b"P02 isolated process fixture",
        )
        .unwrap();
        std::fs::write(
            root.join("source-sha256.txt"),
            format!("{:x}", Sha256::digest(&source)),
        )
        .unwrap();
        let mut child = launch_migration_child(&root, "hold");
        migration_wait_marker(&root, "ready");
        std::fs::write(root.join("begin"), b"begin").unwrap();
        migration_wait_marker(&root, "acquired");
        migration_wait_marker(&root, "publication");
        let destination = std::fs::read(super::super::project_paths_path(&path)).unwrap();
        std::fs::write(root.join("attempt"), variant).unwrap();
        let current = super::super::AppSettings::default();
        // Real production lock entry points must also reject the request.
        match variant {
            "Preserve" => assert!(super::super::save_settings_value(
                &current,
                &path,
                super::super::ProjectWriteMode::Preserve
            )
            .is_err()),
            "Reconcile" => {
                assert!(
                    super::super::begin_project_paths_transaction_at_path(&current, &path).is_err()
                )
            }
            "CAS" => assert!(
                super::super::compare_and_set_terminal_snapshots_enabled_at_path(
                    &current, &path, false, true
                )
                .is_err()
            ),
            _ => unreachable!(),
        }
        let start = std::time::Instant::now();
        let error = migration_fixture_writer(&path, variant).unwrap_err();
        assert!(matches!(
            error.cause,
            super::super::SettingsSaveCause::Semantic(
                super::super::SettingsSaveReason::LockTimedOut
            )
        ));
        assert!(start.elapsed() >= std::time::Duration::from_secs(2));
        std::fs::write(
            root.join("timeout-ms.txt"),
            start.elapsed().as_millis().to_string(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), source);
        assert_eq!(
            std::fs::read(super::super::project_paths_path(&path)).unwrap(),
            destination
        );
        assert!(!root.join("cleanup").exists());
        std::fs::write(root.join("release-cleanup"), b"release").unwrap();
        migration_wait_marker(&root, "cleanup");
        assert!(super::super::raw_project_projection(
            &super::super::read_raw_project_source(&path)
                .unwrap()
                .unwrap()
                .object
        )
        .is_empty());
        // Cleanup completion does not release ownership early.
        let early =
            super::super::SettingsFileLock::acquire(&path, std::time::Duration::from_millis(30));
        assert!(early.is_err());
        std::fs::write(root.join("release-lock"), b"release").unwrap();
        child.finish();
        migration_fixture_writer(&path, variant).unwrap();
        let settings = super::super::read_raw_project_source(&path)
            .unwrap()
            .unwrap()
            .object;
        assert_eq!(settings["unrelated"], 9);
        assert!(super::super::raw_project_projection(&settings).is_empty());
        match variant {
            "Preserve" => assert_eq!(settings["requestedPreserve"], true),
            "CAS" => assert_eq!(settings["terminalSnapshotsEnabled"], true),
            "Reconcile" => assert_eq!(
                super::super::read_raw_project_source(&super::super::project_paths_path(&path))
                    .unwrap()
                    .unwrap()
                    .object["projectPaths"],
                serde_json::json!(["C:/requested-registration"])
            ),
            _ => unreachable!(),
        }
        eprintln!("P02 {variant} process diagnostics {}", root.display());
    }
}

#[test]
fn project_migration_process_crash_reopen_between_publications() {
    for role in [
        "crash-backup-sync",
        "crash-backup-published",
        "crash-destination",
    ] {
        let (temp, path, bytes) =
            migration_fixture(&serde_json::json!({"projectPaths":["C:/gone"],"unknown":9}));
        let root = temp.keep();
        std::fs::write(
            root.join("isolated-fixture.marker"),
            b"P02 isolated process fixture",
        )
        .unwrap();
        let child = launch_migration_child(&root, role);
        migration_wait_marker(&root, "ready");
        std::fs::write(root.join("begin"), b"begin").unwrap();
        migration_wait_marker(&root, "publication");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let orphans: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| {
                let p = entry.unwrap().path();
                p.extension()
                    .is_some_and(|extension| extension == "tmp")
                    .then(|| (p.clone(), std::fs::read(p).unwrap()))
            })
            .collect();
        let backup =
            root.join(super::super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
        if role == "crash-backup-sync" {
            assert!(!backup.exists());
            assert_eq!(orphans.len(), 1);
        }
        // Drop kills and reaps the child while it holds the actual OS lock.
        drop(child);
        let cleanup = std::fs::read_to_string(root.join("child-cleanup.txt")).unwrap();
        assert!(cleanup.contains("deadline_expired=false"), "{cleanup}");
        super::super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), bytes);
        for (orphan, original) in orphans {
            assert_eq!(std::fs::read(orphan).unwrap(), original);
        }
        assert!(super::super::raw_project_projection(
            &super::super::read_raw_project_source(&path)
                .unwrap()
                .unwrap()
                .object
        )
        .is_empty());
        eprintln!("P02 {role} crash diagnostics {}", root.display());
    }
}

#[test]
fn project_migration_readback_and_cleanup_order_oracle() {
    for unsafe_destination in [false, true] {
        let (_temp, path, bytes) =
            migration_fixture(&serde_json::json!({"projectPaths":["C:/gone"],"unknown":9}));
        let destination = super::super::project_paths_path(&path);
        let transaction = migration_transaction(&path);
        let result =
            super::super::ensure_project_paths_migrated_with(&transaction, &mut |stage, target| {
                if stage == "destination_published" {
                    if unsafe_destination {
                        std::fs::remove_file(target).unwrap();
                        std::fs::create_dir(target).unwrap();
                    } else {
                        std::fs::write(target, b"{\"projectPaths\":[\"wrong\"]}").unwrap();
                    }
                }
                Ok(())
            });
        assert!(
            result.is_err(),
            "unsafe publication must never authorize cleanup"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "readback/order oracle: source loss"
        );
        if unsafe_destination {
            std::fs::remove_dir(&destination).unwrap();
        }
        std::fs::write(
            &destination,
            b"{\"projectPaths\":[\"C:/gone\"],\"unknownTarget\":17}",
        )
        .unwrap();
        super::super::ensure_project_paths_migrated_locked(&transaction).unwrap();
        assert_eq!(
            super::super::read_raw_project_source(&destination)
                .unwrap()
                .unwrap()
                .object["unknownTarget"],
            17
        );
        assert_eq!(
            super::super::read_raw_project_source(&path)
                .unwrap()
                .unwrap()
                .object["unknown"],
            9
        );
    }
}

pub(super) fn migration_transaction(
    path: &std::path::Path,
) -> super::super::ProjectPathsWriteTransaction {
    super::super::ProjectPathsWriteTransaction {
        _lock: super::super::SettingsFileLock::acquire(path, std::time::Duration::from_secs(2))
            .unwrap(),
        path: path.to_path_buf(),
        fresh: super::super::AppSettings::default(),
        revalidated_after_failure: false,
    }
}

pub(super) fn migration_fixture(
    raw: &serde_json::Value,
) -> (tempfile::TempDir, std::path::PathBuf, Vec<u8>) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let bytes = serde_json::to_vec_pretty(raw).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    (temp, path, bytes)
}

#[test]
fn project_migration_raw_matrix_and_noop_reopen() {
    let cases = [
        serde_json::json!({}),
        serde_json::json!({"projectPath":"C:/singular-only"}),
        serde_json::json!({"projectPaths":null,"projectPath":null,"archivedProjectPaths":[]}),
        serde_json::json!({"projectPaths":["C:/gone","D:/missing","//server/share/missing"],"projec\
            tPathsRelativeToInstance":["../gone",null,"../conflict"],"projectPath":"C:/gone","proje\
            ctPathRelativeToInstance":"../gone","archivedProjectPaths":["C:/archived-missing"],"arc\
            hivedProjectPathsRelativeToInstance":["../archived-missing"]}),
        serde_json::json!({"projectPaths":42,"projectPathsRelativeToInstance":{},"archivedProjectPa\
            ths":[null,7],"projectPath":true}),
    ];
    for mut raw in cases {
        raw["unrelatedUnknown"] = serde_json::json!({"nested":[null,42,"unchanged"]});
        let (temp, path, bytes) = migration_fixture(&raw);
        let expected = super::super::raw_project_projection(raw.as_object().unwrap());
        assert_eq!(
            super::super::select_project_source_at_path(&path).unwrap(),
            expected
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(
            !super::super::project_paths_path(&path).exists(),
            "read-only must not migrate"
        );
        let transaction = migration_transaction(&path);
        super::super::ensure_project_paths_migrated_locked(&transaction).unwrap();
        let destination =
            super::super::read_raw_project_source(&super::super::project_paths_path(&path))
                .unwrap()
                .unwrap();
        assert_eq!(destination.object, expected);
        let cleaned = super::super::read_raw_project_source(&path)
            .unwrap()
            .unwrap();
        assert!(super::super::raw_project_projection(&cleaned.object).is_empty());
        assert_eq!(cleaned.object["unrelatedUnknown"], raw["unrelatedUnknown"]);
        let state = super::super::decode_project_state(
            &destination.object,
            Some(temp.path()),
            &super::super::projects::FsCandidateResolver,
        );
        if raw["projectPaths"] == 42 {
            assert!(state.has_structural());
        }
        // Windows rejects reads through another handle while the lock is
        // held. Snapshot every file only between migration transactions.
        drop(transaction);
        let before: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| {
                let p = entry.unwrap().path();
                (
                    p.clone(),
                    std::fs::read(&p).unwrap(),
                    std::fs::metadata(&p).unwrap().modified().unwrap(),
                )
            })
            .collect();
        let transaction = migration_transaction(&path);
        super::super::ensure_project_paths_migrated_locked(&transaction).unwrap();
        drop(transaction);
        assert_eq!(
            std::fs::read_dir(temp.path()).unwrap().count(),
            before.len()
        );
        for (p, bytes, modified) in before {
            assert_eq!(std::fs::read(&p).unwrap(), bytes);
            assert_eq!(std::fs::metadata(p).unwrap().modified().unwrap(), modified);
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    assert!(super::super::select_project_source_at_path(&path)
        .unwrap()
        .is_empty());
    super::super::ensure_project_paths_migrated_locked(&migration_transaction(&path)).unwrap();
    assert_eq!(
        std::fs::read(super::super::project_paths_path(&path)).unwrap(),
        b"{}"
    );
    assert!(!path.exists());
}

#[test]
fn project_migration_rejects_invalid_sources_and_conflicts() {
    for bad in [
        b"[1]".as_slice(),
        b"{",
        b"{\"projectPaths\":[],\"projectPaths\":[1]}",
    ] {
        for bad_destination in [false, true] {
            let (_temp, path, bytes) = migration_fixture(&serde_json::json!({"projectPaths":[]}));
            let destination = super::super::project_paths_path(&path);
            let bad_path = if bad_destination { &destination } else { &path };
            std::fs::write(bad_path, bad).unwrap();
            let transaction = migration_transaction(&path);
            assert!(super::super::ensure_project_paths_migrated_locked(&transaction).is_err());
            assert_eq!(std::fs::read(bad_path).unwrap(), bad);
            if bad_destination {
                let error = super::super::select_project_source_locked(&transaction).unwrap_err();
                assert!(!error
                    .legacy_outward
                    .into_general_string()
                    .contains("settings.json exists but could not be parsed"));
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            } else {
                assert!(!destination.exists());
                // A present valid destination cannot hide an invalid source:
                // conflict/remnant evaluation requires both readable objects.
                std::fs::write(&destination, b"{\"projectPaths\":[]}").unwrap();
                assert!(super::super::ensure_project_paths_migrated_locked(&transaction).is_err());
                assert_eq!(std::fs::read(&path).unwrap(), bad);
                assert_eq!(
                    std::fs::read(&destination).unwrap(),
                    b"{\"projectPaths\":[]}"
                );
            }
        }
    }
    let (_temp, path, bytes) = migration_fixture(&serde_json::json!({"projectPath":null}));
    let destination = super::super::project_paths_path(&path);
    std::fs::write(&destination, b"{}").unwrap();
    let transaction = migration_transaction(&path);
    let error = super::super::ensure_project_paths_migrated_locked(&transaction).unwrap_err();
    assert!(super::super::report_settings_save_error(
        error,
        super::super::SettingsSaveReportSurface::GeneralSettings
    )
    .contains("project_paths_conflict"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(std::fs::read(&destination).unwrap(), b"{}");
    std::fs::remove_file(&destination).unwrap();
    std::fs::create_dir(&destination).unwrap();
    assert!(super::super::ensure_project_paths_migrated_locked(&transaction).is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn project_transaction_refreshes_after_barrier_controlled_second_writer() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = temp.path().join("A");
    let b = temp.path().join("B");
    for project in [&a, &b] {
        std::fs::create_dir_all(project.join(".ac")).unwrap();
    }
    let raw = serde_json::json!({"defaultShell": "test-shell", "defaultShellArgs": [], "agent\
        s": [], "projectPaths": [a], "projectPath": a});
    std::fs::write(&path, raw.to_string()).unwrap();
    let mut stale = super::super::AppSettings::default();
    super::super::refresh_and_decode_project_paths_from_path(&mut stale, &path).unwrap();
    // Also exercise a cached object that claims no pending repair.
    stale.project_path_state = std::sync::Arc::default();
    let lock =
        super::super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1)).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let worker_barrier = barrier.clone();
    let worker_path = path.clone();
    let worker = std::thread::spawn(move || {
        worker_barrier.wait();
        super::super::repair_fresh_project_paths_at_path(&mut stale, &worker_path).unwrap();
        stale
    });
    barrier.wait();
    // This writer owns the file lock before the repair can acquire it.
    let fresh = serde_json::json!({"defaultShell": "test-shell", "defaultShellArgs": [], "agent\
        s": [], "projectPaths": [a, b], "projectPath": a});
    super::super::write_value_atomic(&fresh, &path).unwrap();
    drop(lock);
    let repaired = worker.join().unwrap();
    assert_eq!(repaired.project_paths.len(), 2);
    let disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(super::super::project_paths_path(&path)).unwrap())
            .unwrap();
    assert_eq!(disk["projectPaths"].as_array().unwrap().len(), 2);
    assert!(repaired.project_paths_persistence_error.is_none());
}

#[test]
fn project_transaction_timeout_preserves_bytes_and_live_paths() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let bytes =
        b"{\"defaultShell\":\"test-shell\",\"defaultShellArgs\":[],\"agents\":[],\"projectP\
        aths\":[]}";
    std::fs::write(&path, bytes).unwrap();
    let _lock =
        super::super::SettingsFileLock::acquire(&path, std::time::Duration::from_secs(1)).unwrap();
    let mut current = super::super::AppSettings {
        project_paths: vec!["cached".into()],
        ..super::super::AppSettings::default()
    };
    let result = super::super::repair_fresh_project_paths_at_path(&mut current, &path);
    assert!(matches!(
        result,
        Err((super::super::ProjectPathsRepairStage::Read, _))
    ));
    assert_eq!(current.project_paths, vec!["cached"]);
    assert!(super::super::project_state_has_structural(&current));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn project_fresh_repair_retains_missing_raw_registration() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let active = temp.path().join("active");
    let missing = temp.path().join("missing").to_string_lossy().into_owned();
    std::fs::create_dir_all(active.join(".ac")).unwrap();
    let raw = serde_json::json!({"defaultShell": "test-shell", "defaultShellArgs": [], "agent\
        s": [], "projectPaths": [active, missing], "projectPath": active});
    std::fs::write(&path, raw.to_string()).unwrap();
    let mut current = super::super::AppSettings::default();
    super::super::repair_fresh_project_paths_at_path(&mut current, &path).unwrap();
    let disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(super::super::project_paths_path(&path)).unwrap())
            .unwrap();
    assert_eq!(disk["projectPaths"].as_array().unwrap().len(), 2);
    assert_eq!(disk["projectPaths"][1], missing);
    assert_eq!(current.project_paths.len(), 1);
}

#[test]
fn project_transaction_keeps_candidate_separate_on_precommit_failure() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        b"{\"defaultShell\":\"test-shell\",\"defaultShellArgs\":[],\"agents\":[]\
        ,\"projectPaths\":[]}",
    )
    .unwrap();
    let current = super::super::AppSettings::default();
    let mut transaction =
        super::super::begin_project_paths_transaction_at_path(&current, &path).unwrap();
    let mut candidate = transaction.fresh_settings().clone();
    candidate.project_paths = vec!["candidate".into()];
    super::super::resync_project_state_from_runtime(&mut candidate);
    // Simulate a noncooperating fault, not a second cooperating writer.
    std::fs::write(&path, b"invalid").unwrap();
    assert!(transaction
        .commit_project_groups(&candidate, true, true)
        .is_err());
    assert!(transaction.fresh_settings().project_paths.is_empty());
    assert!(current.project_paths.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), b"invalid");
}

#[test]
fn project_transaction_uncertain_commit_adopts_only_revalidated_disk() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    std::fs::write(
        &path,
        b"{\"defaultShell\":\"test-shell\",\"defaultShellArgs\":[],\"agents\":[]\
        ,\"projectPaths\":[]}",
    )
    .unwrap();
    let mut transaction = super::super::begin_project_paths_transaction_at_path(
        &super::super::AppSettings::default(),
        &path,
    )
    .unwrap();
    let error = || {
        super::super::SettingsSaveError::semantic(
            super::super::SettingsSaveStage::PostWriteRead,
            &path,
            None,
            super::super::SettingsSaveReason::PostWriteReadRejected,
            super::super::SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    };
    // Model a completed publication followed by uncertain readback.
    let published = serde_json::json!({"defaultShell": "test-shell", "defaultShellArgs": [], "agent\
        s": [], "projectPaths": [], "sidebarStyle": "published"});
    super::super::write_value_atomic(&published, &path).unwrap();
    assert!(transaction.handle_commit_result(Err(error())).is_err());
    assert_eq!(
        transaction.fresh_settings().sidebar_style,
        super::super::AppSettings::default().sidebar_style
    );
    assert!(transaction
        .fresh_settings()
        .project_paths_persistence_error
        .is_none());
    assert!(transaction.revalidated_after_failure);
    // Failure to revalidate must block rather than publish guessed state.
    std::fs::write(&path, b"invalid").unwrap();
    assert!(transaction.handle_commit_result(Err(error())).is_err());
    assert!(transaction
        .fresh_settings()
        .project_paths_persistence_error
        .is_some());
    assert_eq!(std::fs::read(&path).unwrap(), b"invalid");
}

#[test]
fn project_fresh_repair_clears_unavailable_only_after_valid_read() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut current = super::super::AppSettings::default();
    std::fs::write(&path, b"invalid").unwrap();
    assert!(super::super::repair_fresh_project_paths_at_path(&mut current, &path).is_err());
    assert!(current.project_paths_persistence_error.is_some());
    std::fs::write(
        &path,
        b"{\"defaultShell\":\"test-shell\",\"defaultShellArgs\":[],\"agents\":[],\"projectPath\
                s\":[],\"archivedProjectPaths\":[]}",
    )
    .unwrap();
    super::super::repair_fresh_project_paths_at_path(&mut current, &path).unwrap();
    assert!(current.project_paths_persistence_error.is_none());
    assert!(current.project_paths.is_empty());
    assert!(!super::super::project_state_has_structural(&current));
    let mut hidden = current.clone();
    hidden.project_paths_persistence_error = Some("unavailable".into());
    assert_eq!(
        serde_json::to_value(&hidden).unwrap(),
        serde_json::to_value(&current).unwrap()
    );
}
