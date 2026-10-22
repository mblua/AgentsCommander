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

// #2714 (B1b) - the startup naming migration. Every test drives the inner
// functions against a tempdir; none calls `preflight_config_startup()`,
// which would read the developer's own configuration directory. File names
// are literals on purpose: an independent oracle, not the constants.

/// Serializes the tests that record or take the process-wide summary.
static NAMING_MIGRATION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn naming_migration_guard() -> std::sync::MutexGuard<'static, ()> {
    NAMING_MIGRATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    let _guard = naming_migration_guard();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);

    crate::preflight_config_startup_in(dir).expect("migration succeeds");

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

    let before = directory_snapshot(dir);
    crate::preflight_config_startup_in(dir).expect("second run is a no-op");
    assert_eq!(
        directory_snapshot(dir),
        before,
        "a second run changed bytes"
    );
}

/// The personal overlay is still read after the migration renamed it.
#[test]
fn the_overlay_is_read_under_its_new_name_after_migration() {
    let _guard = naming_migration_guard();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    std::fs::write(dir.join("settings.json"), r#"{"logLevel":"info"}"#).unwrap();
    std::fs::write(dir.join("settings.local.json"), r#"{"logLevel":"debug"}"#).unwrap();

    crate::preflight_config_startup_in(dir).unwrap();

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
    let _guard = naming_migration_guard();
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

        crate::preflight_config_startup_in(dir).expect("both-present is not fatal");

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
    let _guard = naming_migration_guard();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);
    std::fs::write(dir.join("settings.json.lock"), "old lock bytes").unwrap();
    std::fs::write(dir.join("settings.json.99.op.tmp"), "stale temp bytes").unwrap();

    crate::preflight_config_startup_in(dir).expect("migration succeeds");

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
    let _guard = naming_migration_guard();

    // An unreadable journal.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);
    let journal = dir.join("naming-migration.state.no-git.json");
    std::fs::write(&journal, "not json").unwrap();
    let message = crate::preflight_config_startup_in(dir)
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
    let message = crate::preflight_config_startup_in(dir)
        .expect_err("an Io refusal is fatal")
        .to_string();
    assert!(message.contains(&lock.display().to_string()), "{message}");
    assert!(message.contains("make the directory writable"), "{message}");

    // Both names present is resolved, never fatal.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);
    std::fs::write(dir.join("settings.30.instance.no-git.json"), "the new file").unwrap();
    crate::preflight_config_startup_in(dir).expect("both-present is not fatal");
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
    use crate::config::naming_migration as nm;
    let _guard = naming_migration_guard();

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
                journal.set_status("instance-families", nm::ScopeStatus::Complete)
            })
            .unwrap();
            drop(held);
        })
    };
    held_rx.recv().unwrap();
    crate::preflight_config_startup_in(&dir).expect("the winner completed the scope");
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
    let message = crate::preflight_config_startup_in(&dir)
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
    let _guard = naming_migration_guard();
    let _ = crate::take_naming_migration_summary();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);
    std::fs::write(dir.join("settings.30.instance.no-git.json"), "the new file").unwrap();

    crate::preflight_config_startup_in(dir).unwrap();
    let summary = crate::take_naming_migration_summary().expect("a migrating start logs");
    assert!(summary.contains("instance-families"), "{summary}");
    assert!(summary.contains("7 renamed"), "{summary}");
    assert!(
        summary.contains("settings.json.deprecated-1.no-git"),
        "the demoted file must be named: {summary}"
    );
    assert_eq!(crate::take_naming_migration_summary(), None, "taken once");

    crate::preflight_config_startup_in(dir).unwrap();
    assert_eq!(
        crate::take_naming_migration_summary(),
        None,
        "a no-op start logs nothing"
    );
}

/// E19.
#[test]
fn a_complete_scope_whose_old_name_is_back_is_re_run() {
    let _guard = naming_migration_guard();

    // The data rename was lost behind a durable `Complete`.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    seed_instance_families(dir);
    crate::preflight_config_startup_in(dir).unwrap();
    std::fs::rename(
        dir.join("settings.30.instance.no-git.json"),
        dir.join("settings.json"),
    )
    .unwrap();
    assert_eq!(
        instance_families_record(dir).status,
        crate::config::naming_migration::ScopeStatus::Complete
    );
    crate::preflight_config_startup_in(dir).expect("re-run succeeds");
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
    crate::preflight_config_startup_in(dir).expect("both-present is not fatal");
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
    let _guard = naming_migration_guard();
    let temp = tempfile::tempdir().unwrap();

    // Absent: skipped, recorded, and nothing is written anywhere.
    let _ = crate::take_naming_migration_summary();
    let absent = temp.path().join("absent-config");
    crate::preflight_config_startup_in(&absent).expect("an absent directory is skipped");
    assert!(!absent.exists(), "the directory was created");
    assert_eq!(
        std::fs::read_dir(temp.path()).unwrap().count(),
        0,
        "a journal, lock or temporary was written"
    );
    let summary = crate::take_naming_migration_summary().expect("the skip is recorded");
    assert!(summary.contains("skipped"), "{summary}");

    // A regular file where the directory should be.
    let file = temp.path().join("config-is-a-file");
    std::fs::write(&file, "not a directory").unwrap();
    let message = crate::preflight_config_startup_in(&file)
        .expect_err("a regular file is fatal")
        .to_string();
    assert!(message.contains(&file.display().to_string()), "{message}");

    // A path below a regular file. Unix reports ENOTDIR, which is fatal.
    // Windows reports ERROR_PATH_NOT_FOUND (3), which std maps to
    // `NotFound`, so there it is the absent case: skipped, nothing created.
    let child = file.join("child");
    #[cfg(unix)]
    {
        let message = crate::preflight_config_startup_in(&child)
            .expect_err("a path below a file is fatal")
            .to_string();
        assert!(message.contains(&child.display().to_string()), "{message}");
    }
    #[cfg(windows)]
    {
        crate::preflight_config_startup_in(&child)
            .expect("Windows reports a path below a file as not found");
        assert!(std::fs::metadata(&file).unwrap().is_file());
        assert!(!child.exists());
    }
}

/// E20.
#[test]
fn no_config_dir_skips_the_migration_and_starts() {
    let _guard = naming_migration_guard();
    let _ = crate::take_naming_migration_summary();
    let untouched = tempfile::tempdir().unwrap();

    crate::preflight_config_startup_for(None).expect("no configuration directory is not fatal");

    assert_eq!(
        std::fs::read_dir(untouched.path()).unwrap().count(),
        0,
        "the migration wrote somewhere"
    );
    let summary = crate::take_naming_migration_summary().expect("the skip is recorded");
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
            "AgentsCommander cannot start because it could not create app outbox directory \"{}\" for configuration directory \"{}\": denied. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.",
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
#[test]
fn issue_1930_linux_unmarked_adjacent_unwritable_refuses_to_start() {
    use std::fs::OpenOptions;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    const CHILD_SENTINEL: &str = "AGENTSCOMMANDER_ISSUE_1577_CHILD";
    const TEST_NAME: &str = "tests::issue_1930_linux_unmarked_adjacent_unwritable_refuses_to_start";

    if std::env::var_os(CHILD_SENTINEL).is_some() {
        let executable = std::env::current_exe().unwrap();
        let case_root = executable.parent().unwrap();
        let adjacent = case_root.join(".agentscommander_issue1577_linux_subprocess");

        assert_eq!(crate::config::config_dir(), Some(adjacent.clone()));
        let Some(crate::config::ConfigStartupError::AdjacentDirectoryUnwritable {
            config_dir,
            reason,
        }) = crate::config::config_startup_error()
        else {
            panic!("an unwritable unmarked adjacent directory must refuse startup");
        };
        assert_eq!(config_dir, adjacent);
        let reason_prefix = format!(
            "write probe could not create configuration directory \"{}\" after 1 attempt(s): ",
            adjacent.display()
        );
        assert!(
            reason.starts_with(&reason_prefix),
            "unexpected reason: {reason}"
        );
        let refusal = crate::preflight_config_startup().expect_err("startup must be refused");
        assert_eq!(
            refusal.to_string(),
            format!(
                "AgentsCommander cannot start because it cannot write its configuration directory \"{}\" next to the executable: {}{} Move the executable to a writable folder, or set AGENTSCOMMANDER_CONFIG_DIR to a writable directory, and restart.",
                adjacent.display(),
                reason,
                if reason.ends_with('.') { "" } else { "." }
            )
        );
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
        let source_executable =
            std::env::current_exe().map_err(|error| format!("current_exe failed: {error}"))?;
        std::fs::copy(&source_executable, &copied_executable).map_err(|error| {
            format!(
                "copy {} -> {} failed: {error}",
                source_executable.display(),
                copied_executable.display()
            )
        })?;
        let mut executable_permissions = std::fs::metadata(&copied_executable)
            .map_err(|error| format!("copied executable metadata failed: {error}"))?
            .permissions();
        executable_permissions.set_mode(executable_permissions.mode() | 0o111);
        std::fs::set_permissions(&copied_executable, executable_permissions)
            .map_err(|error| format!("set executable mode failed: {error}"))?;

        if adjacent.exists() {
            return Err(format!("fixture not fresh adjacent={}", adjacent.display()));
        }

        let mut read_only_permissions = std::fs::metadata(&case_root)
            .map_err(|error| format!("case-root metadata failed: {error}"))?
            .permissions();
        read_only_permissions.set_mode(0o555);
        std::fs::set_permissions(&case_root, read_only_permissions)
            .map_err(|error| format!("chmod 0555 {} failed: {error}", case_root.display()))?;

        let fixture_probe = case_root.join("fixture-write-preflight.tmp");
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&fixture_probe)
        {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
            Err(error) => {
                return Err(format!(
                    "0555 preflight returned {:?} instead of PermissionDenied for {}: {error}",
                    error.kind(),
                    fixture_probe.display()
                ));
            }
            Ok(file) => {
                drop(file);
                let mut restored = std::fs::metadata(&case_root)
                    .map_err(|error| format!("restore metadata failed: {error}"))?
                    .permissions();
                restored.set_mode(original_mode);
                std::fs::set_permissions(&case_root, restored)
                    .map_err(|error| format!("restore after false preflight failed: {error}"))?;
                std::fs::remove_file(&fixture_probe).map_err(|error| {
                    format!("remove false-positive fixture probe failed: {error}")
                })?;
                return Err(format!(
                    "mode 0555 did not block create_new in {}; refusing false pass",
                    case_root.display()
                ));
            }
        }

        let mut child = Command::new(&copied_executable)
            .arg(TEST_NAME)
            .arg("--exact")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env("HOME", &home_root)
            .env_remove("AGENTSCOMMANDER_CONFIG_DIR")
            .env_remove("AGENTSCOMMANDER_TEST_CONFIG_DIR")
            .env(CHILD_SENTINEL, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "spawn copied child {} failed: {error}",
                    copied_executable.display()
                )
            })?;

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut status_poll_error = None;
        let terminal_status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(None) => break None,
                Err(error) => {
                    status_poll_error = Some(error);
                    break None;
                }
            }
        };

        if terminal_status.is_none() {
            let kill_error = child.kill().err();
            let reap_deadline = Instant::now() + Duration::from_secs(5);
            let reaped = loop {
                match child.try_wait() {
                    Ok(Some(_)) => break true,
                    Ok(None) if Instant::now() < reap_deadline => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Ok(None) | Err(_) => break false,
                }
            };
            if !reaped {
                return Err(format!(
                    "child did not reap within 5 seconds; kill_error={kill_error:?} poll_error={status_poll_error:?} case_root={} home_root={}",
                    case_root.display(),
                    home_root.display()
                ));
            }
            let output = child
                .wait_with_output()
                .map_err(|error| format!("timed-out child final reap failed: {error}"))?;
            return Err(format!(
                "child did not reach a clean terminal status; kill_error={kill_error:?} poll_error={status_poll_error:?} status={:?} stdout={:?} stderr={:?} case_root={} home_root={}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
                case_root.display(),
                home_root.display()
            ));
        }

        let output = child
            .wait_with_output()
            .map_err(|error| format!("child output collection failed: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            return Err(format!(
                "child failed status={:?} stdout={stdout:?} stderr={stderr:?} case_root={} home_root={}",
                output.status.code(),
                case_root.display(),
                home_root.display()
            ));
        }
        for text in [&*stdout, &*stderr] {
            if text.contains("panicked at") || text.contains("stack backtrace:") {
                return Err(format!(
                    "child emitted panic/backtrace text stdout={stdout:?} stderr={stderr:?}"
                ));
            }
        }

        let home_entries: Vec<_> = std::fs::read_dir(&home_root)
            .map_err(|error| format!("read home failed: {error}"))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<_, _>>()
            .map_err(|error| format!("read home entry failed: {error}"))?;
        if !home_entries.is_empty() {
            return Err(format!(
                "a suffixed build wrote into HOME: {home_entries:?}"
            ));
        }
        if adjacent.exists() {
            return Err(format!(
                "adjacent state appeared adjacent={}",
                adjacent.display()
            ));
        }
        let case_entries: Vec<_> = std::fs::read_dir(&case_root)
            .map_err(|error| format!("read case root failed: {error}"))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<_, _>>()
            .map_err(|error| format!("read case-root entry failed: {error}"))?;
        if case_entries
            != [copied_executable
                .file_name()
                .expect("copied executable file name")
                .to_os_string()]
        {
            return Err(format!("unexpected case-root entries: {case_entries:?}"));
        }
        Ok(())
    })();

    let restore_result = (|| -> Result<(), String> {
        let mut permissions = std::fs::metadata(&case_root)
            .map_err(|error| format!("mode-restore metadata failed: {error}"))?
            .permissions();
        permissions.set_mode(original_mode);
        std::fs::set_permissions(&case_root, permissions).map_err(|error| {
            format!(
                "restore exact mode on {} failed: {error}",
                case_root.display()
            )
        })
    })();
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
