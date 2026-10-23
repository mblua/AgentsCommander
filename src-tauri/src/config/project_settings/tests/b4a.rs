use super::*;

// #2717 (B4a) - rows E6 to E13 of the phase plan. Every name below is a
// literal (4.1): no fixture composes a project-settings name from a constant.

const B4A_LEGACY_BYTES: &str = "{\n  \"groups\": [{\"id\": \"g1\", \"name\": \"Legacy\", \
    \"regex\": \"^legacy-\"}],\n  \"showAll\": true,\n  \"showUngrouped\": false,\n  \"userKey\": \
    7\n}\n";
const B4A_WINNER_BYTES: &str = "{\"groups\": [{\"id\": \"g2\", \"name\": \"Winner\", \"regex\": \
    \"^win-\"}], \"showAll\": true, \"showUngrouped\": true}";
const B4A_OLD_LOCK: &str = ".project-settings.json.lock";
const B4A_NEW_LOCK: &str = ".settings.50.personal.no-git.json.lock";
const B4A_SET_ASIDE: &str = "project-settings.json.deprecated-1.no-git";

struct B4aFixture {
    _tmp: tempfile::TempDir,
    project: PathBuf,
    cfg: PathBuf,
}

fn b4a_fixture() -> B4aFixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(project.join(".ac")).expect("create .ac");
    std::fs::create_dir_all(&cfg).expect("create cfg");
    B4aFixture {
        _tmp: tmp,
        project,
        cfg,
    }
}

fn b4a_ac_root(project: &Path) -> PathBuf {
    crate::config::ac_root::existing_ac_root(project).expect("the fixture has a .ac")
}

fn b4a_key(project: &Path) -> String {
    format!("project-settings:{}", b4a_ac_root(project).display())
}

fn b4a_record(cfg: &Path, project: &Path) -> Option<naming_migration::ScopeRecord> {
    naming_migration::read_journal(Some(cfg))
        .expect("readable journal")
        .and_then(|journal| journal.scope(&b4a_key(project)).cloned())
}

fn b4a_names(config: &WorkgroupGroupsConfig) -> Vec<String> {
    config.groups.iter().map(|g| g.name.clone()).collect()
}

fn b4a_entries(dir: &Path) -> std::collections::BTreeSet<String> {
    std::fs::read_dir(dir)
        .expect("list dir")
        .map(|e| e.expect("entry").file_name().into_string().expect("utf-8"))
        .collect()
}

fn b4a_saved() -> WorkgroupGroupsConfig {
    config_with_groups(vec![group("g3", "Saved", "^saved-")])
}

#[test]
fn every_entry_point_migrates_before_it_resolves_the_path() {
    for entry in ["load", "save"] {
        let f = b4a_fixture();
        std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
        std::fs::write(f.project.join(".ac").join(B4A_OLD_LOCK), b"").unwrap();

        if entry == "load" {
            let loaded = load_workgroup_groups_in(&f.project, Some(&f.cfg))
                .expect("load migrates and reads");
            assert_eq!(b4a_names(&loaded), ["Legacy"], "{entry}");
            assert_eq!(
                std::fs::read(settings_path(&f.project)).expect("new name"),
                B4A_LEGACY_BYTES.as_bytes(),
                "{entry}: the bytes did not survive under the new name"
            );
        } else {
            save_workgroup_groups_in(&f.project, b4a_saved(), Some(&f.cfg))
                .expect("save migrates and writes");
            let disk: Value = serde_json::from_str(
                &std::fs::read_to_string(settings_path(&f.project)).expect("new name"),
            )
            .unwrap();
            assert_eq!(disk["groups"][0]["name"], "Saved", "{entry}");
            assert_eq!(
                disk["userKey"], 7,
                "{entry}: the save wrote a fresh file instead of the migrated one"
            );
        }
        assert!(!legacy_settings_path(&f.project).exists(), "{entry}");
        assert!(
            f.project.join(".ac").join(B4A_OLD_LOCK).exists(),
            "{entry}: the old lock sidecar must stay"
        );
        let record = b4a_record(&f.cfg, &f.project).expect("scope recorded");
        assert_eq!(
            record.status,
            naming_migration::ScopeStatus::Complete,
            "{entry}"
        );
        assert!(
            record.notes.iter().any(|n| n.contains(B4A_OLD_LOCK)),
            "{entry}: the old lock is not noted: {:?}",
            record.notes
        );
    }
}

#[test]
fn a_pre_existing_target_wins_and_the_old_file_is_set_aside() {
    let f = b4a_fixture();
    let ac = f.project.join(".ac");
    std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
    std::fs::write(settings_path(&f.project), B4A_WINNER_BYTES).unwrap();
    std::fs::write(ac.join(B4A_OLD_LOCK), b"").unwrap();
    std::fs::write(ac.join(B4A_NEW_LOCK), b"").unwrap();
    std::fs::write(ac.join(".gitignore"), b"# user rules\n").unwrap();
    let before = b4a_entries(&ac);

    let loaded = load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("both present is Ok");

    assert_eq!(b4a_names(&loaded), ["Winner"]);
    assert_eq!(
        std::fs::read(settings_path(&f.project)).unwrap(),
        B4A_WINNER_BYTES.as_bytes()
    );
    assert!(!legacy_settings_path(&f.project).exists());
    assert_eq!(
        std::fs::read(ac.join(B4A_SET_ASIDE)).expect("set aside"),
        B4A_LEGACY_BYTES.as_bytes()
    );
    let mut expected = before.clone();
    expected.remove("project-settings.json");
    expected.insert(B4A_SET_ASIDE.to_string());
    assert_eq!(
        b4a_entries(&ac),
        expected,
        "the .ac directory changed otherwise"
    );
    let record = b4a_record(&f.cfg, &f.project).expect("scope recorded");
    assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
    assert!(record
        .steps
        .iter()
        .any(|s| s.state == naming_migration::StepState::SetAside));

    let saved = save_workgroup_groups_in(&f.project, b4a_saved(), Some(&f.cfg))
        .expect("the save after a set-aside is Ok");
    assert_eq!(b4a_names(&saved), ["Saved"]);
    assert!(
        !legacy_settings_path(&f.project).exists(),
        "a third file was created"
    );
    assert_eq!(b4a_entries(&ac), expected, "the save created another file");

    // The set-aside file is ignored in the user's repository.
    crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&ac, &[])
        .expect("write .ac/.gitignore");
    assert!(b4a_git(&f.project, &["init", "--quiet"]).status.success());
    let set_aside = format!(".ac/{B4A_SET_ASIDE}");
    assert!(
        b4a_git(
            &f.project,
            &["check-ignore", "--no-index", "-q", "--", &set_aside]
        )
        .status
        .success(),
        "the set-aside file is not ignored"
    );
}

#[test]
fn a_refused_lock_is_an_error_and_creates_nothing() {
    let f = b4a_fixture();
    let ac_root = b4a_ac_root(&f.project);
    std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
    let held = naming_migration::lock_scope(
        &ac_root,
        B4A_OLD_LOCK,
        None,
        naming_migration::MIGRATION_LOCK_BUDGET,
    )
    .expect("hold the old sidecar");

    let loaded = load_workgroup_groups_in(&f.project, Some(&f.cfg));
    let saved = save_workgroup_groups_in(&f.project, b4a_saved(), Some(&f.cfg));

    for (entry, error) in [("load", loaded.map(|_| ())), ("save", saved.map(|_| ()))] {
        let error = error.expect_err(entry);
        assert!(
            error.contains(&ac_root.display().to_string()),
            "{entry}: the error does not name the .ac directory: {error}"
        );
    }
    assert!(
        !settings_path(&f.project).exists(),
        "a refused migration created the destination"
    );
    assert_eq!(
        std::fs::read(legacy_settings_path(&f.project)).unwrap(),
        B4A_LEGACY_BYTES.as_bytes()
    );

    drop(held);
    let loaded = load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("lock free");
    assert_eq!(b4a_names(&loaded), ["Legacy"]);
    assert!(!legacy_settings_path(&f.project).exists());
}

#[test]
fn an_io_failure_after_the_rename_is_an_error_not_the_old_name() {
    let f = b4a_fixture();
    let ac_root = b4a_ac_root(&f.project);
    std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
    let journal_file = f
        .cfg
        .join(crate::config::instance_artifacts::NAMING_MIGRATION_STATE_NAME);
    let armed = naming_migration::pause::arm("after_rename", &ac_root);
    let (project, cfg) = (f.project.clone(), f.cfg.clone());
    let worker = std::thread::spawn(move || load_workgroup_groups_in(&project, Some(&cfg)));
    armed
        .reached
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the migration reached the rename");
    // Renamed on disk, and the `Renamed` commit is about to fail.
    std::fs::remove_file(&journal_file).expect("journal written by `Renaming`");
    std::fs::create_dir(&journal_file).expect("block the journal");
    armed.release.send(()).unwrap();

    let loaded = worker.join().unwrap();
    assert!(
        loaded.is_err(),
        "an Io after the rename must be an error, got {loaded:?}"
    );
    assert_eq!(
        std::fs::read(settings_path(&f.project)).unwrap(),
        B4A_LEGACY_BYTES.as_bytes()
    );
    assert!(
        project_settings_path_in(&f.project, Some(&f.cfg)).is_err(),
        "the path must be an error while the disk state is unknown"
    );
    assert!(
        save_workgroup_groups_in(&f.project, b4a_saved(), Some(&f.cfg)).is_err(),
        "the save must refuse"
    );
    assert!(
        !legacy_settings_path(&f.project).exists(),
        "the create-if-missing writer rebuilt the legacy file"
    );

    std::fs::remove_dir(&journal_file).unwrap();
    let loaded = load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("next call");
    assert_eq!(b4a_names(&loaded), ["Legacy"]);
    assert_eq!(
        b4a_record(&f.cfg, &f.project).expect("recorded").status,
        naming_migration::ScopeStatus::Complete
    );
}

#[test]
fn an_unreachable_project_is_skipped_and_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let missing = tmp.path().join("missing");
    let a_file = tmp.path().join("a-file");
    std::fs::write(&a_file, b"not a directory").unwrap();
    let no_ac = tmp.path().join("no-ac");
    std::fs::create_dir_all(&no_ac).unwrap();
    // `.ac` present but unwritable for the scope: a Windows directory's
    // read-only attribute does not stop a create, so the new lock sidecar's
    // name is taken by a directory and the scope cannot open it.
    let blocked = tmp.path().join("blocked");
    std::fs::create_dir_all(blocked.join(".ac").join(B4A_NEW_LOCK)).unwrap();
    std::fs::write(legacy_settings_path(&blocked), B4A_LEGACY_BYTES).unwrap();

    for project in [&missing, &a_file, &no_ac, &blocked] {
        assert!(
            load_workgroup_groups_in(project, Some(&cfg)).is_err(),
            "{} must be refused",
            project.display()
        );
    }
    let status = b4a_record(&cfg, &blocked).map(|r| r.status);
    assert_ne!(status, Some(naming_migration::ScopeStatus::Complete));

    std::fs::remove_file(&a_file).unwrap();
    std::fs::remove_dir(blocked.join(".ac").join(B4A_NEW_LOCK)).unwrap();
    for project in [&missing, &a_file, &no_ac] {
        std::fs::create_dir_all(project.join(".ac")).unwrap();
        std::fs::write(legacy_settings_path(project), B4A_LEGACY_BYTES).unwrap();
    }
    for project in [&missing, &a_file, &no_ac, &blocked] {
        let loaded = load_workgroup_groups_in(project, Some(&cfg))
            .unwrap_or_else(|e| panic!("{} retried: {e}", project.display()));
        assert_eq!(b4a_names(&loaded), ["Legacy"]);
        assert!(!legacy_settings_path(project).exists());
        assert_eq!(
            b4a_record(&cfg, project).expect("recorded").status,
            naming_migration::ScopeStatus::Complete
        );
    }
}

#[test]
fn no_journal_dir_still_migrates_and_never_creates_a_second_file() {
    let f = b4a_fixture();
    let ac = f.project.join(".ac");
    std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();

    let saved = save_workgroup_groups_in(&f.project, b4a_saved(), None).expect("Ok");

    assert_eq!(b4a_names(&saved), ["Saved"]);
    let disk: Value =
        serde_json::from_str(&std::fs::read_to_string(settings_path(&f.project)).unwrap()).unwrap();
    assert_eq!(disk["groups"][0]["name"], "Saved");
    assert_eq!(disk["userKey"], 7);
    assert!(!legacy_settings_path(&f.project).exists());
    assert!(
        b4a_entries(&f.cfg).is_empty(),
        "a journal-less run wrote into a configuration directory"
    );
    assert!(
        b4a_entries(&ac)
            .iter()
            .all(|n| !n.starts_with("naming-migration") && !n.ends_with(".tmp")),
        "a journal or temporary landed in the project: {:?}",
        b4a_entries(&ac)
    );

    let g = b4a_fixture();
    std::fs::write(settings_path(&g.project), B4A_WINNER_BYTES).unwrap();
    save_workgroup_groups_in(&g.project, b4a_saved(), None).expect("Ok(new)");
    assert!(
        !legacy_settings_path(&g.project).exists(),
        "a second file was created"
    );
}

fn b4a_stale_complete(cfg: &Path, project: &Path) {
    let ac_root = b4a_ac_root(project);
    let key = b4a_key(project);
    naming_migration::update_journal(Some(cfg), |j| {
        j.record_step(
            &key,
            naming_migration::StepRecord::new(
                &ac_root,
                "project-settings.json",
                "settings.50.personal.no-git.json",
                naming_migration::StepState::Renamed,
            ),
        );
        j.set_status(&key, naming_migration::ScopeStatus::Complete);
    })
    .expect("seed the journal");
}

#[test]
fn a_complete_scope_whose_old_name_is_back_is_re_run() {
    let f = b4a_fixture();
    std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
    b4a_stale_complete(&f.cfg, &f.project);

    let loaded = load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("re-run");

    assert_eq!(
        b4a_names(&loaded),
        ["Legacy"],
        "the stale Complete was trusted"
    );
    assert_eq!(
        std::fs::read(settings_path(&f.project)).unwrap(),
        B4A_LEGACY_BYTES.as_bytes()
    );
    assert!(!legacy_settings_path(&f.project).exists());

    let g = b4a_fixture();
    std::fs::write(legacy_settings_path(&g.project), B4A_LEGACY_BYTES).unwrap();
    std::fs::write(settings_path(&g.project), B4A_WINNER_BYTES).unwrap();
    b4a_stale_complete(&g.cfg, &g.project);
    let loaded = load_workgroup_groups_in(&g.project, Some(&g.cfg)).expect("Ok(new)");
    assert_eq!(b4a_names(&loaded), ["Winner"]);
    assert_eq!(
        std::fs::read(settings_path(&g.project)).unwrap(),
        B4A_WINNER_BYTES.as_bytes()
    );
    assert_eq!(
        std::fs::read(g.project.join(".ac").join(B4A_SET_ASIDE)).unwrap(),
        B4A_LEGACY_BYTES.as_bytes()
    );
}

// -- cross-process rows, E7d and E13 --------------------------------------

const B4A_CHILD_FQN: &str = "config::project_settings::tests::b4a::b4a_child_process_entry";
const B4A_CHILD_ROLE: &str = "AC_2717_CHILD_ROLE";
const B4A_CHILD_PROJECT: &str = "AC_2717_CHILD_PROJECT";
const B4A_CHILD_CFG: &str = "AC_2717_CHILD_CFG";
const B4A_CHILD_RENDEZVOUS: &str = "AC_2717_CHILD_RENDEZVOUS";
const B4A_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

fn b4a_spawn(
    role: &str,
    project: &Path,
    cfg: &Path,
    extra: &[(&str, &Path)],
) -> std::process::Child {
    let mut command =
        std::process::Command::new(std::env::current_exe().expect("current test exe"));
    command
        .args(["--exact", B4A_CHILD_FQN, "--nocapture", "--test-threads=1"])
        .env(B4A_CHILD_ROLE, role)
        .env(B4A_CHILD_PROJECT, project)
        .env(B4A_CHILD_CFG, cfg)
        .env_remove(naming_migration::pause::PAUSE_DIR_ENV)
        .env_remove(naming_migration::pause::PAUSE_STAGE_ENV)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (key, value) in extra {
        command.env(key, value);
    }
    command.spawn().expect("spawn child")
}

fn b4a_wait_file(path: &Path) {
    let started = std::time::Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < B4A_WAIT,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn b4a_wait_child(child: std::process::Child) {
    let output = child.wait_with_output().expect("child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "child failed or ran nothing: {}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Not a test of its own: the body a spawned child runs, inert otherwise.
#[test]
fn b4a_child_process_entry() {
    let Some(role) = std::env::var_os(B4A_CHILD_ROLE) else {
        return;
    };
    let project = PathBuf::from(std::env::var_os(B4A_CHILD_PROJECT).expect("project"));
    let cfg = PathBuf::from(std::env::var_os(B4A_CHILD_CFG).expect("cfg"));
    match role.to_str() {
        Some("migrate") => {
            load_workgroup_groups_in(&project, Some(&cfg)).expect("child migrates");
        }
        Some("timeout_then_save") => {
            let rendezvous =
                PathBuf::from(std::env::var_os(B4A_CHILD_RENDEZVOUS).expect("rendezvous"));
            let first = save_workgroup_groups_in(&project, b4a_saved(), Some(&cfg));
            assert!(first.is_err(), "the first save must time out: {first:?}");
            std::fs::write(rendezvous.join("timed-out"), b"").unwrap();
            b4a_wait_file(&rendezvous.join("go"));
            let _ = save_workgroup_groups_in(&project, b4a_saved(), Some(&cfg));
        }
        other => panic!("unknown child role {other:?}"),
    }
}

#[test]
fn a_timed_out_caller_never_writes_the_old_name_after_the_winner_finishes() {
    let f = b4a_fixture();
    std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
    let rv_a = f.cfg.join("rv-a");
    let rv_b = f.cfg.join("rv-b");
    std::fs::create_dir_all(&rv_a).unwrap();
    std::fs::create_dir_all(&rv_b).unwrap();
    let stage = std::path::PathBuf::from("after_rename");

    let winner = b4a_spawn(
        "migrate",
        &f.project,
        &f.cfg,
        &[
            (naming_migration::pause::PAUSE_DIR_ENV, &rv_a),
            (naming_migration::pause::PAUSE_STAGE_ENV, &stage),
        ],
    );
    b4a_wait_file(&rv_a.join(naming_migration::pause::READY_FILE));
    let loser = b4a_spawn(
        "timeout_then_save",
        &f.project,
        &f.cfg,
        &[(B4A_CHILD_RENDEZVOUS, &rv_b)],
    );
    b4a_wait_file(&rv_b.join("timed-out"));
    std::fs::write(rv_a.join(naming_migration::pause::RELEASE_FILE), b"go").unwrap();
    b4a_wait_child(winner);
    std::fs::write(rv_b.join("go"), b"").unwrap();
    b4a_wait_child(loser);

    assert!(
        !legacy_settings_path(&f.project).exists(),
        "the old name was written after the handover"
    );
    let disk: Value =
        serde_json::from_str(&std::fs::read_to_string(settings_path(&f.project)).unwrap()).unwrap();
    let name = disk["groups"][0]["name"].as_str().unwrap_or_default();
    assert!(
        name == "Saved" || name == "Legacy",
        "unexpected content: {disk}"
    );
}

#[test]
fn two_project_scopes_keep_both_record_sets() {
    let f = b4a_fixture();
    let other = f.cfg.parent().unwrap().join("other");
    std::fs::create_dir_all(other.join(".ac")).unwrap();
    std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
    std::fs::write(legacy_settings_path(&other), B4A_WINNER_BYTES).unwrap();
    let rv = f.cfg.join("rv");
    std::fs::create_dir_all(&rv).unwrap();
    let stage = std::path::PathBuf::from("after_rename");

    let first = b4a_spawn(
        "migrate",
        &f.project,
        &f.cfg,
        &[
            (naming_migration::pause::PAUSE_DIR_ENV, &rv),
            (naming_migration::pause::PAUSE_STAGE_ENV, &stage),
        ],
    );
    b4a_wait_file(&rv.join(naming_migration::pause::READY_FILE));
    // While the first holds its own data locks, the second commits.
    b4a_wait_child(b4a_spawn("migrate", &other, &f.cfg, &[]));
    std::fs::write(rv.join(naming_migration::pause::RELEASE_FILE), b"go").unwrap();
    b4a_wait_child(first);

    for (project, bytes) in [(&f.project, B4A_LEGACY_BYTES), (&other, B4A_WINNER_BYTES)] {
        let record = b4a_record(&f.cfg, project).expect("both scopes recorded");
        assert_eq!(record.status, naming_migration::ScopeStatus::Complete);
        assert!(
            record
                .steps
                .iter()
                .any(|s| s.state == naming_migration::StepState::Renamed),
            "{}: {:?}",
            project.display(),
            record.steps
        );
        assert_eq!(
            std::fs::read(settings_path(project)).unwrap(),
            bytes.as_bytes()
        );
    }
}

/// E9 (r12): key independence and commit non-interference only. The foreign
/// key's shape is copied from `catalog_scope_key` in
/// `config/coding_agents_catalog.rs`; F1's own deferral stays B2's evidence.
#[test]
fn a_foreign_scope_record_survives_this_scope_commit() {
    let f = b4a_fixture();
    std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
    let catalog = f.project.join(".ac").join("coding-agents");
    std::fs::create_dir_all(&catalog).unwrap();
    let catalog = std::fs::canonicalize(&catalog).unwrap();
    let foreign_key = format!("catalog:{}", catalog.display());
    naming_migration::update_journal(Some(&f.cfg), |j| {
        j.note(
            &foreign_key,
            "deferred: an interrupted #1968 migration is blocked",
        );
        j.set_status(&foreign_key, naming_migration::ScopeStatus::Deferred);
    })
    .expect("seed the foreign record");
    let foreign_before = naming_migration::read_journal(Some(&f.cfg))
        .unwrap()
        .unwrap()
        .scope(&foreign_key)
        .cloned()
        .expect("foreign record");

    load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("this scope runs");

    let own_key = b4a_key(&f.project);
    assert_ne!(own_key, foreign_key, "the two scope keys collide");
    let journal = naming_migration::read_journal(Some(&f.cfg))
        .expect("the journal still parses through the engine's reader")
        .expect("journal present");
    let foreign_after = journal
        .scope(&foreign_key)
        .cloned()
        .expect("foreign record kept");
    assert_eq!(
        serde_json::to_value(&foreign_after).unwrap(),
        serde_json::to_value(&foreign_before).unwrap(),
        "this scope's commit changed the foreign record"
    );
    assert_eq!(
        foreign_after.status,
        naming_migration::ScopeStatus::Deferred
    );
    assert_eq!(
        journal.scope(&own_key).expect("own record").status,
        naming_migration::ScopeStatus::Complete
    );
}

/// The one `git` site of these tests (E7, E17).
fn b4a_git(cwd: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git must execute")
}

/// #2717 (B4a) 4.4 1b: each row pattern is composed from the registry, and
/// is pinned against the constants, never against a typed-out name.
#[test]
fn the_ignore_rows_are_composed_from_the_registry() {
    use crate::config::instance_artifacts::{PROJECT_SETTINGS_TARGET_NAME, SET_ASIDE_GLOB};
    let [settings, lock, set_aside] = naming_migration::project_settings_ignore_rows();
    assert_eq!(settings.0, format!("/{PROJECT_SETTINGS_TARGET_NAME}"));
    assert_eq!(lock.0, format!("/.{PROJECT_SETTINGS_TARGET_NAME}.lock"));
    assert_eq!(set_aside.0, format!("/{SET_ASIDE_GLOB}"));
}

const B4A_ROWS: [(&str, &str); 3] = [
    (
        "/settings.50.personal.no-git.json",
        "# AgentsCommander: exclude generated project-local settings.",
    ),
    (
        "/.settings.50.personal.no-git.json.lock",
        "# AgentsCommander: exclude the project settings write-lock sidecar.",
    ),
    (
        "/*.deprecated-*.no-git",
        "# AgentsCommander: exclude project files the naming migration set aside.",
    ),
];

fn b4a_gitignore(project: &Path) -> String {
    std::fs::read_to_string(project.join(".ac").join(".gitignore")).expect("read .gitignore")
}

fn b4a_assert_rows_once(content: &str, label: &str) {
    let lines: Vec<&str> = content.lines().collect();
    for (pattern, comment) in B4A_ROWS {
        let at: Vec<usize> = (0..lines.len()).filter(|&i| lines[i] == pattern).collect();
        assert_eq!(at.len(), 1, "{label}: {pattern} is not there exactly once");
        assert!(
            at[0] > 0 && lines[at[0] - 1] == comment,
            "{label}: {pattern} lacks its comment line directly above"
        );
    }
}

#[test]
fn the_scope_ignores_the_new_names_before_it_renames_anything() {
    // The pre-upgrade shape, a project that is never registered.
    let f = b4a_fixture();
    let ac = f.project.join(".ac");
    std::fs::write(
        ac.join(".gitignore"),
        "# AgentsCommander: exclude generated project-local settings.\n/project-settings.json\n",
    )
    .unwrap();
    std::fs::write(legacy_settings_path(&f.project), B4A_LEGACY_BYTES).unwrap();
    load_workgroup_groups_in(&f.project, Some(&f.cfg)).expect("load");
    save_workgroup_groups_in(&f.project, b4a_saved(), Some(&f.cfg)).expect("save");
    let content = b4a_gitignore(&f.project);
    b4a_assert_rows_once(&content, "base");
    assert!(
        content.lines().any(|l| l == "/project-settings.json"),
        "the sweep is append-only; the old row stays until the writer retires it"
    );
    assert!(b4a_git(&f.project, &["init", "--quiet"]).status.success());
    std::fs::write(ac.join(B4A_SET_ASIDE), b"").unwrap();
    for (relative, rule) in [
        (".ac/settings.50.personal.no-git.json", B4A_ROWS[0].0),
        (".ac/.settings.50.personal.no-git.json.lock", B4A_ROWS[1].0),
        (
            ".ac/project-settings.json.deprecated-1.no-git",
            B4A_ROWS[2].0,
        ),
    ] {
        let output = b4a_git(
            &f.project,
            &["check-ignore", "-v", "--no-index", "--", relative],
        );
        assert!(output.status.success(), "{relative} is not ignored");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let (source, _) = stdout.trim_end().split_once('\t').expect("verbose output");
        assert!(
            source.ends_with(&format!(":{rule}")),
            "{relative}: {source}"
        );
    }

    // A failed sweep refuses and renames nothing.
    let g = b4a_fixture();
    std::fs::create_dir(g.project.join(".ac").join(".gitignore")).unwrap();
    std::fs::write(legacy_settings_path(&g.project), B4A_LEGACY_BYTES).unwrap();
    assert!(load_workgroup_groups_in(&g.project, Some(&g.cfg)).is_err());
    assert_eq!(
        std::fs::read(legacy_settings_path(&g.project)).unwrap(),
        B4A_LEGACY_BYTES.as_bytes(),
        "a failed sweep must leave the data at the old name"
    );
    assert!(!settings_path(&g.project).exists());

    // The decoy: a leading-space line is not the rule.
    let h = b4a_fixture();
    std::fs::write(
        h.project.join(".ac").join(".gitignore"),
        " /settings.50.personal.no-git.json\n",
    )
    .unwrap();
    std::fs::write(legacy_settings_path(&h.project), B4A_LEGACY_BYTES).unwrap();
    load_workgroup_groups_in(&h.project, Some(&h.cfg)).expect("load");
    b4a_assert_rows_once(&b4a_gitignore(&h.project), "decoy");

    // Both orders of the sweep and the registration writer: every AC row
    // once, and a user line seeded before either run survives.
    for sweep_first in [true, false] {
        let k = b4a_fixture();
        let ac = k.project.join(".ac");
        std::fs::write(ac.join(".gitignore"), "user-line-2717\n").unwrap();
        std::fs::write(legacy_settings_path(&k.project), B4A_LEGACY_BYTES).unwrap();
        let writer = || {
            crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&ac, &[])
                .expect("writer")
        };
        if sweep_first {
            load_workgroup_groups_in(&k.project, Some(&k.cfg)).expect("sweep");
            writer();
        } else {
            writer();
            load_workgroup_groups_in(&k.project, Some(&k.cfg)).expect("sweep");
        }
        let content = b4a_gitignore(&k.project);
        let label = if sweep_first {
            "sweep, writer"
        } else {
            "writer, sweep"
        };
        b4a_assert_rows_once(&content, label);
        assert!(content.lines().any(|l| l == "user-line-2717"), "{label}");
        let rules: Vec<&str> = content
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .collect();
        let unique: std::collections::BTreeSet<&str> = rules.iter().copied().collect();
        assert_eq!(unique.len(), rules.len(), "{label}: a row appears twice");
    }

    // Interleaved: the registration writer and a user edit land between the
    // sweep's read and its append. Nothing is lost; a 4.4 row may repeat
    // once; no other rule repeats.
    let expected_rules: Vec<String> = {
        let fresh = b4a_fixture();
        let fresh_ac = fresh.project.join(".ac");
        crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&fresh_ac, &[])
            .expect("writer");
        b4a_gitignore(&fresh.project)
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect()
    };
    let m = b4a_fixture();
    let ac = m.project.join(".ac");
    let ac_root = b4a_ac_root(&m.project);
    std::fs::write(ac.join(".gitignore"), "user-line-2717\n").unwrap();
    std::fs::write(legacy_settings_path(&m.project), B4A_LEGACY_BYTES).unwrap();
    let armed = naming_migration::pause::arm("before_gitignore_append", &ac_root);
    let (project, cfg) = (m.project.clone(), m.cfg.clone());
    let sweep = std::thread::spawn(move || load_workgroup_groups_in(&project, Some(&cfg)));
    armed
        .reached
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the sweep reached its append");
    crate::commands::ac_discovery::ensure_ac_root_gitignore_with_names(&ac, &[])
        .expect("writer in the window");
    {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(ac.join(".gitignore"))
            .unwrap();
        file.write_all(b"user-line-late\n").unwrap();
    }
    armed.release.send(()).unwrap();
    sweep.join().unwrap().expect("the sweep completes");
    let content = b4a_gitignore(&m.project);
    let count = |rule: &str| content.lines().filter(|l| *l == rule).count();
    for user in ["user-line-2717", "user-line-late"] {
        assert_eq!(count(user), 1, "interleaved: the user line {user} was lost");
    }
    for rule in &expected_rules {
        let n = count(rule);
        let is_4_4 = B4A_ROWS.iter().any(|(pattern, _)| pattern == rule);
        assert!(n >= 1, "interleaved: the writer's rule {rule} was lost");
        assert!(
            n <= if is_4_4 { 2 } else { 1 },
            "interleaved: {rule} appears {n} times"
        );
    }
}
