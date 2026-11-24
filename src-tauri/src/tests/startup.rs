use super::*;

#[test]
fn startup_unavailable_project_state_never_enters_session_load_or_purge() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("sessions.json");
    let bytes = b"[{\"name\":\"retained\"}]";
    std::fs::write(&file, bytes).unwrap();
    let settings = crate::config::settings::AppSettings {
        project_paths_persistence_error: Some("unavailable".into()),
        ..Default::default()
    };
    let result = super::load_startup_sessions(&settings, |_| {
        std::fs::write(&file, b"[]").unwrap();
        Vec::new()
    });
    assert_eq!(result.unwrap_err(), "unavailable");
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    // Valid empty is a real source and still reaches the loader.
    let mut called = false;
    let result = super::load_startup_sessions(&Default::default(), |paths| {
        assert!(paths.is_empty());
        called = true;
        Vec::new()
    });
    assert!(called);
    assert!(result.unwrap().is_empty());
}

// (B1b) - the startup naming migration. Every test drives the inner
// functions against a tempdir; none calls `preflight_config_startup()`,
// which would read the developer's own configuration directory. File names
// are literals on purpose: an independent oracle, not the constants.

const INSTANCE_FAMILIES_FIXTURE: [(&str, &str); 8] = [
    ("settings.json", "settings.30.instance.no-git.json"),
    ("settings.local.json", "settings.50.personal.no-git.json"),
    (
        "settings.backup.1.json",
        "settings.30.instance.no-git.backup.1.json",
    ),
    (
        "settings.pre-384-v1.json",
        "settings.30.instance.no-git.pre-384-v1.json",
    ),
    (
        "settings-blocking-menus.json",
        "blocking-menus.10.default.no-git.json",
    ),
    (
        "settings-blocking-menus.local.json",
        "blocking-menus.50.personal.no-git.json",
    ),
    (
        "settings-blocking-menus.remote.json",
        "blocking-menus.20.remote.no-git.json",
    ),
    (
        "blocking-menus-remote-check.json",
        "blocking-menus.state.no-git.json",
    ),
];

fn seed_instance_families(dir: &Path) {
    for (from, _) in INSTANCE_FAMILIES_FIXTURE {
        std::fs::write(dir.join(from), format!("bytes of {from}")).unwrap();
    }
}

fn instance_families_record(dir: &Path) -> crate::config::naming_migration::ScopeRecord {
    crate::config::naming_migration::read_journal(Some(dir))
        .unwrap()
        .expect("journal exists")
        .scope("instance-families")
        .expect("scope recorded")
        .clone()
}

fn directory_snapshot(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| {
            (
                entry.file_name().into_string().unwrap(),
                std::fs::read(entry.path()).unwrap_or_default(),
            )
        })
        .collect()
}

/// The files the migration itself creates: the journal, its lock and the
/// two settings lock sidecars.
fn is_migration_artifact(name: &str) -> bool {
    matches!(
        name,
        "naming-migration.state.no-git.json"
            | ".naming-migration.state.no-git.json.lock"
            | "settings.30.instance.no-git.json.lock"
            | "settings.json.lock"
    )
}

fn data_files(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    directory_snapshot(dir)
        .into_iter()
        .filter(|(name, _)| !is_migration_artifact(name))
        .collect()
}

/// E7.
#[test]
fn instance_families_happy_path() {
    let summary_owner = crate::startup::StartupMigrationSummary::new();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);

    crate::preflight_config_startup_in(&summary_owner, dir).expect("migration succeeds");

    for (from, to) in INSTANCE_FAMILIES_FIXTURE {
        assert!(!dir.join(from).exists(), "{from} survived");
        assert_eq!(
            std::fs::read_to_string(dir.join(to)).unwrap(),
            format!("bytes of {from}"),
            "{to} must hold the exact bytes of {from}"
        );
    }
    let record = instance_families_record(dir);
    assert_eq!(
        record.status,
        crate::config::naming_migration::ScopeStatus::Complete
    );

    let ordered_steps: Vec<_> = record
        .steps
        .iter()
        .map(|step| (step.from.as_str(), step.to.as_str()))
        .collect();
    assert_eq!(ordered_steps, INSTANCE_FAMILIES_FIXTURE);
    assert!(record.steps.iter().all(|step| {
        step.state == crate::config::naming_migration::StepState::Renamed
            && step.set_aside.is_none()
    }));

    let before = directory_snapshot(dir);
    crate::preflight_config_startup_in(&summary_owner, dir).expect("second run is a no-op");
    assert_eq!(
        directory_snapshot(dir),
        before,
        "a second run changed bytes"
    );
}

/// The personal overlay is still read after the migration renamed it.
#[test]
fn the_overlay_is_read_under_its_new_name_after_migration() {
    let summary_owner = crate::startup::StartupMigrationSummary::new();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    std::fs::write(dir.join("settings.json"), r#"{"logLevel":"info"}"#).unwrap();
    std::fs::write(dir.join("settings.local.json"), r#"{"logLevel":"debug"}"#).unwrap();

    crate::preflight_config_startup_in(&summary_owner, dir).unwrap();

    let mut base = serde_json::json!({"logLevel": "info"});
    let overlay = crate::config::local_overlay::LocalSettingsOverlay::load_and_merge(
        &dir.join("settings.30.instance.no-git.json"),
        &mut base,
        &[],
        &[],
        &[],
    );
    assert!(!overlay.is_empty(), "the overlay was not found");
    assert_eq!(base["logLevel"], serde_json::json!("debug"));
}

/// E8.
#[test]
fn a_pre_existing_target_wins_and_the_old_file_is_set_aside() {
    let summary_owner = crate::startup::StartupMigrationSummary::new();
    for existing_ordinals in [0_u32, 1] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        seed_instance_families(dir);
        std::fs::write(dir.join("settings.30.instance.no-git.json"), "the new file").unwrap();
        for n in 1..=existing_ordinals {
            std::fs::write(
                dir.join(format!("settings.json.deprecated-{n}.no-git")),
                format!("older set-aside {n}"),
            )
            .unwrap();
        }
        let expected_name = format!("settings.json.deprecated-{}.no-git", existing_ordinals + 1);
        let before = data_files(dir);

        crate::preflight_config_startup_in(&summary_owner, dir).expect("both-present is not fatal");

        assert_eq!(
            std::fs::read_to_string(dir.join("settings.30.instance.no-git.json")).unwrap(),
            "the new file"
        );
        assert!(!dir.join("settings.json").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join(&expected_name)).unwrap(),
            "bytes of settings.json"
        );
        for n in 1..=existing_ordinals {
            assert_eq!(
                std::fs::read_to_string(dir.join(format!("settings.json.deprecated-{n}.no-git")))
                    .unwrap(),
                format!("older set-aside {n}"),
                "an older set-aside copy was touched"
            );
        }
        let record = instance_families_record(dir);
        assert_eq!(
            record.status,
            crate::config::naming_migration::ScopeStatus::Complete
        );
        let step = record
            .steps
            .iter()
            .find(|step| step.from == "settings.json")
            .expect("settings.json step");
        assert_eq!(
            step.state,
            crate::config::naming_migration::StepState::SetAside
        );
        assert_eq!(step.set_aside.as_deref(), Some(expected_name.as_str()));

        // Every byte stays on disk: the same number of data files, one of
        // them under its set-aside name, and every seeded content present.
        let after = data_files(dir);
        assert_eq!(after.len(), before.len(), "{after:?}");
        let mut before_bytes: Vec<&Vec<u8>> = before.values().collect();
        let mut after_bytes: Vec<&Vec<u8>> = after.values().collect();
        before_bytes.sort();
        after_bytes.sort();
        assert_eq!(after_bytes, before_bytes, "a byte was lost or duplicated");
    }
}

/// E9.
#[test]
fn a_stale_lock_or_temp_is_left_on_disk_and_noted() {
    let summary_owner = crate::startup::StartupMigrationSummary::new();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);
    std::fs::write(dir.join("settings.json.lock"), "old lock bytes").unwrap();
    std::fs::write(dir.join("settings.json.99.op.tmp"), "stale temp bytes").unwrap();

    crate::preflight_config_startup_in(&summary_owner, dir).expect("migration succeeds");

    assert_eq!(
        std::fs::read_to_string(dir.join("settings.json.lock")).unwrap(),
        "old lock bytes"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("settings.json.99.op.tmp")).unwrap(),
        "stale temp bytes"
    );
    let notes = instance_families_record(dir).notes;
    for name in ["settings.json.lock", "settings.json.99.op.tmp"] {
        assert!(
            notes.iter().any(|note| note.contains(name)),
            "{name} not noted: {notes:?}"
        );
    }
}

/// E12.
#[test]
fn only_the_three_fatal_outcomes_stop_startup() {
    let summary_owner = crate::startup::StartupMigrationSummary::new();

    // An unreadable journal.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);
    let journal = dir.join("naming-migration.state.no-git.json");
    std::fs::write(&journal, "not json").unwrap();
    let message = crate::preflight_config_startup_in(&summary_owner, dir)
        .expect_err("an unreadable journal is fatal")
        .to_string();
    assert!(
        message.contains(&journal.display().to_string()),
        "{message}"
    );
    assert!(message.contains("move an unreadable naming-migration journal aside"));
    assert!(dir.join("settings.json").exists(), "nothing renamed");

    // An `Io` refusal: the new lock sidecar cannot be opened.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);
    let lock = dir.join("settings.30.instance.no-git.json.lock");
    std::fs::create_dir(&lock).unwrap();
    let message = crate::preflight_config_startup_in(&summary_owner, dir)
        .expect_err("an Io refusal is fatal")
        .to_string();
    assert!(message.contains(&lock.display().to_string()), "{message}");
    assert!(message.contains("make the directory writable"), "{message}");

    // Both names present is resolved, never fatal.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);
    std::fs::write(dir.join("settings.30.instance.no-git.json"), "the new file").unwrap();
    crate::preflight_config_startup_in(&summary_owner, dir).expect("both-present is not fatal");
    assert_eq!(
        instance_families_record(dir).status,
        crate::config::naming_migration::ScopeStatus::Complete
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("settings.json.deprecated-1.no-git")).unwrap(),
        "bytes of settings.json"
    );
}

/// E13.
#[test]
fn a_contended_migration_waits_then_defers_to_the_winner() {
    let summary_owner = crate::startup::StartupMigrationSummary::new();
    use crate::config::naming_migration as nm;

    // The winner completes the scope inside the budget.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().to_path_buf();
    std::fs::write(dir.join("settings.json"), "bytes of settings.json").unwrap();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let winner = {
        let dir = dir.clone();
        std::thread::spawn(move || {
            let held = nm::lock_scope(
                &dir,
                "settings.30.instance.no-git.json.lock",
                Some("settings.json.lock"),
                nm::MIGRATION_LOCK_BUDGET,
            )
            .unwrap();
            held_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(500));
            for rename in &crate::INSTANCE_FAMILIES_RENAMES {
                nm::rename_step(&dir, rename, "instance-families", Some(&dir), &held);
            }
            nm::update_journal(Some(&dir), |journal| {
                journal.set_status("instance-families", nm::ScopeStatus::Complete);
            })
            .unwrap();
            drop(held);
        })
    };
    held_rx.recv().unwrap();
    crate::preflight_config_startup_in(&summary_owner, &dir)
        .expect("the winner completed the scope");
    winner.join().unwrap();
    let record = instance_families_record(&dir);
    assert_eq!(record.status, nm::ScopeStatus::Complete);
    let renamed: Vec<_> = record
        .steps
        .iter()
        .filter(|step| step.state == nm::StepState::Renamed)
        .map(|step| step.from.as_str())
        .collect();
    assert_eq!(renamed, ["settings.json"]);
    assert_eq!(
        std::fs::read_to_string(dir.join("settings.30.instance.no-git.json")).unwrap(),
        "bytes of settings.json"
    );

    // The holder outlives the budget and never completes.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().to_path_buf();
    std::fs::write(dir.join("settings.json"), "bytes of settings.json").unwrap();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder = {
        let dir = dir.clone();
        std::thread::spawn(move || {
            let held = nm::lock_scope(
                &dir,
                "settings.30.instance.no-git.json.lock",
                Some("settings.json.lock"),
                nm::MIGRATION_LOCK_BUDGET,
            )
            .unwrap();
            held_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(std::time::Duration::from_secs(30));
            drop(held);
        })
    };
    held_rx.recv().unwrap();
    let message = crate::preflight_config_startup_in(&summary_owner, &dir)
        .expect_err("an expired budget with no winner is fatal")
        .to_string();
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    assert!(message.contains("retry once it has finished"), "{message}");
    assert!(dir.join("settings.json").exists(), "nothing renamed");
}

/// E14. Both entries, the GUI `run()` and the CLI branch of `main.rs`, call
/// `log_naming_migration_summary` right after `init_logger`, and it logs
/// exactly what `take_naming_migration_summary` returns.
#[test]
fn the_post_logger_summary_names_what_was_renamed() {
    let summary_owner = crate::startup::StartupMigrationSummary::new();
    let _ = crate::take_naming_migration_summary(&summary_owner);
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);
    std::fs::write(dir.join("settings.30.instance.no-git.json"), "the new file").unwrap();

    crate::preflight_config_startup_in(&summary_owner, dir).unwrap();
    let summary =
        crate::take_naming_migration_summary(&summary_owner).expect("a migrating start logs");
    assert!(summary.contains("instance-families"), "{summary}");
    assert!(summary.contains("7 renamed"), "{summary}");
    assert!(
        summary.contains("settings.json.deprecated-1.no-git"),
        "the demoted file must be named: {summary}"
    );
    assert_eq!(
        crate::take_naming_migration_summary(&summary_owner),
        None,
        "taken once"
    );

    crate::preflight_config_startup_in(&summary_owner, dir).unwrap();
    assert_eq!(
        crate::take_naming_migration_summary(&summary_owner),
        None,
        "a no-op start logs nothing"
    );
}

/// E19.
#[test]
fn a_complete_scope_whose_old_name_is_back_is_re_run() {
    let summary_owner = crate::startup::StartupMigrationSummary::new();

    // The data rename was lost behind a durable `Complete`.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);
    crate::preflight_config_startup_in(&summary_owner, dir).unwrap();
    std::fs::rename(
        dir.join("settings.30.instance.no-git.json"),
        dir.join("settings.json"),
    )
    .unwrap();
    assert_eq!(
        instance_families_record(dir).status,
        crate::config::naming_migration::ScopeStatus::Complete
    );
    crate::preflight_config_startup_in(&summary_owner, dir).expect("re-run succeeds");
    assert!(
        !dir.join("settings.json").exists(),
        "the stale Complete was trusted"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("settings.30.instance.no-git.json")).unwrap(),
        "bytes of settings.json"
    );

    // Both names present behind a durable `Complete`.
    std::fs::write(dir.join("settings.json"), "a restored old file").unwrap();
    crate::preflight_config_startup_in(&summary_owner, dir).expect("both-present is not fatal");
    assert_eq!(
        std::fs::read_to_string(dir.join("settings.30.instance.no-git.json")).unwrap(),
        "bytes of settings.json"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("settings.json.deprecated-1.no-git")).unwrap(),
        "a restored old file"
    );
}

/// E21. An absent configuration directory is skipped and not created; a
/// path that exists but is not a readable directory is fatal.
#[test]
fn a_missing_config_dir_skips_and_a_bad_one_is_fatal() {
    let summary_owner = crate::startup::StartupMigrationSummary::new();
    let temp = tempfile::tempdir().unwrap();

    // Absent: skipped, recorded, and nothing is written anywhere.
    let _ = crate::take_naming_migration_summary(&summary_owner);
    let absent = temp.path().join("absent-config");
    crate::preflight_config_startup_in(&summary_owner, &absent)
        .expect("an absent directory is skipped");
    assert!(!absent.exists(), "the directory was created");
    assert_eq!(
        std::fs::read_dir(temp.path()).unwrap().count(),
        0,
        "a journal, lock or temporary was written"
    );
    let summary =
        crate::take_naming_migration_summary(&summary_owner).expect("the skip is recorded");
    assert!(summary.contains("skipped"), "{summary}");

    // A regular file where the directory should be.
    let file = temp.path().join("config-is-a-file");
    std::fs::write(&file, "not a directory").unwrap();
    let message = crate::preflight_config_startup_in(&summary_owner, &file)
        .expect_err("a regular file is fatal")
        .to_string();
    assert!(message.contains(&file.display().to_string()), "{message}");

    // A path below a regular file. Unix reports ENOTDIR, which is fatal.
    // Windows reports ERROR_PATH_NOT_FOUND (3), which std maps to
    // `NotFound`, so there it is the absent case: skipped, nothing created.
    let child = file.join("child");
    #[cfg(unix)]
    {
        let message = crate::preflight_config_startup_in(&summary_owner, &child)
            .expect_err("a path below a file is fatal")
            .to_string();
        assert!(message.contains(&child.display().to_string()), "{message}");
    }
    #[cfg(windows)]
    {
        crate::preflight_config_startup_in(&summary_owner, &child)
            .expect("Windows reports a path below a file as not found");
        assert!(std::fs::metadata(&file).unwrap().is_file());
        assert!(!child.exists());
    }
}

/// E20.
#[test]
fn no_config_dir_skips_the_migration_and_starts() {
    let summary_owner = crate::startup::StartupMigrationSummary::new();
    let _ = crate::take_naming_migration_summary(&summary_owner);
    let untouched = tempfile::tempdir().unwrap();

    crate::preflight_config_startup_for(&summary_owner, None)
        .expect("no configuration directory is not fatal");

    assert_eq!(
        std::fs::read_dir(untouched.path()).unwrap().count(),
        0,
        "the migration wrote somewhere"
    );
    let summary =
        crate::take_naming_migration_summary(&summary_owner).expect("the skip is recorded");
    assert!(summary.contains("skipped"), "{summary}");
}

#[test]
fn issue_1577_app_outbox_error_is_typed_and_exact() {
    let config_dir = std::path::PathBuf::from("config-root");
    let app_outbox_path = config_dir.join("instances/fixed/outbox");
    let error = StartupError {
        kind: StartupErrorKind::AppOutboxCreate {
            config_dir: config_dir.clone(),
            app_outbox_path: app_outbox_path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        },
    };
    assert_eq!(
        error.to_string(),
        format!(
            "AgentsCommander cannot start because it could not create app outbox directory \
                \"{}\" for configuration directory \"{}\": denied. Set \
                AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.",
            app_outbox_path.display(),
            config_dir.display()
        )
    );
    let StartupErrorKind::AppOutboxCreate { source, .. } = &error.kind else {
        panic!("expected typed app-outbox error");
    };
    assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn issue_1577_prepare_app_outbox_creates_the_shared_path() {
    let temp = tempfile::TempDir::new().unwrap();
    let instance_id = "00000000-0000-4000-8000-000000001577";
    let (path, outbox) = prepare_app_outbox(temp.path(), instance_id).unwrap();
    let expected = temp
        .path()
        .join("instances")
        .join(instance_id)
        .join("outbox");
    assert_eq!(path, expected);
    assert_eq!(outbox.path(), expected.to_string_lossy());
    assert!(expected.is_dir());
}

#[cfg(target_os = "linux")]
mod issue_1930_linux;

#[cfg(target_os = "linux")]
#[test]
fn issue_1930_linux_unmarked_adjacent_unwritable_refuses_to_start() {
    let summary_owner = crate::startup::StartupMigrationSummary::new();
    use issue_1930_linux::{
        assert_child_refuses_startup, prepare_unwritable_case_root, restore_case_root_mode,
        run_child, verify_child_output, CHILD_SENTINEL,
    };
    use std::os::unix::fs::PermissionsExt;

    if std::env::var_os(CHILD_SENTINEL).is_some() {
        assert_child_refuses_startup(&summary_owner);
        return;
    }

    let case_temp = tempfile::TempDir::new().unwrap();
    let home_temp = tempfile::TempDir::new().unwrap();
    let case_root = case_temp.path().to_path_buf();
    let home_root = home_temp.path().to_path_buf();
    let original_mode = std::fs::metadata(&case_root).unwrap().permissions().mode();
    let copied_executable = case_root.join("agentscommander_issue1577_linux_subprocess");
    let adjacent = case_root.join(".agentscommander_issue1577_linux_subprocess");

    let body = (|| -> Result<(), String> {
        prepare_unwritable_case_root(&case_root, &copied_executable, &adjacent, original_mode)?;
        let output = run_child(&copied_executable, &case_root, &home_root)?;
        verify_child_output(
            &output,
            &case_root,
            &home_root,
            &copied_executable,
            &adjacent,
        )
    })();

    let restore_result = restore_case_root_mode(&case_root, original_mode);
    let case_close = case_temp
        .close()
        .map_err(|error| format!("close case TempDir {} failed: {error}", case_root.display()));
    let home_close = home_temp
        .close()
        .map_err(|error| format!("close home TempDir {} failed: {error}", home_root.display()));

    let mut failures = Vec::new();
    if let Err(error) = body {
        failures.push(error);
    }
    if let Err(error) = restore_result {
        failures.push(error);
    }
    if let Err(error) = case_close {
        failures.push(error);
    }
    if let Err(error) = home_close {
        failures.push(error);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn startup_summary_clones_preserve_once_none_overwrite_and_poison() {
    let root = crate::AppOwners::new();
    let repeated_run = root.clone();
    crate::startup::record_naming_migration_summary(
        root.startup_summary.as_ref(),
        Some("old".into()),
    );
    crate::startup::record_naming_migration_summary(
        repeated_run.startup_summary.as_ref(),
        Some("new".into()),
    );
    assert_eq!(
        crate::take_naming_migration_summary(root.startup_summary.as_ref()).as_deref(),
        Some("new")
    );
    assert_eq!(
        crate::take_naming_migration_summary(repeated_run.startup_summary.as_ref()),
        None
    );
    crate::startup::record_naming_migration_summary(
        root.startup_summary.as_ref(),
        Some("discard".into()),
    );
    crate::startup::record_naming_migration_summary(repeated_run.startup_summary.as_ref(), None);
    assert_eq!(
        crate::take_naming_migration_summary(root.startup_summary.as_ref()),
        None
    );
    assert!(std::panic::catch_unwind(|| repeated_run.startup_summary.poison_for_test()).is_err());
    crate::startup::record_naming_migration_summary(
        root.startup_summary.as_ref(),
        Some("ignored".into()),
    );
    assert_eq!(
        crate::take_naming_migration_summary(repeated_run.startup_summary.as_ref()),
        None
    );
}

#[test]
fn startup_skips_temp_room_sessions() {
    let base = crate::config::config_dir().expect("config dir");
    let normal_dir = tempfile::tempdir().unwrap();
    let temp = crate::config::sessions_persistence::PersistedSession {
        name: "temp".into(),
        working_directory: base.join("temp-room-tr01").to_string_lossy().into_owned(),
        ..Default::default()
    };
    let normal = crate::config::sessions_persistence::PersistedSession {
        name: "normal".into(),
        working_directory: normal_dir.path().to_string_lossy().into_owned(),
        ..Default::default()
    };
    let result = super::load_startup_sessions(&Default::default(), |_| vec![temp, normal]);
    let names: Vec<_> = result.unwrap().into_iter().map(|s| s.name).collect();
    assert_eq!(names, vec!["normal".to_string()]);
}
