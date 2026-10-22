use super::*;

/// Test 11 (pure): `validate_session_creation_cwd` accepts a root-agent
/// path (even when missing) and an archived-root path (existence waived).
#[test]
fn validate_session_creation_cwd_accepts_root_agent_and_archived_roots() {
    let root = crate::config::root_agent::root_agent_dir().expect("root dir resolves");
    assert!(
        validate_session_creation_cwd(&root, &[], &[]).is_ok(),
        "root-agent path allowed even when missing"
    );
    let temp = tempfile::tempdir().unwrap();
    let archived_root = temp.path().to_string_lossy().to_string();
    // A cwd under an archived root that does NOT exist is still Ok.
    let missing_under_archived = temp.path().join("not-there").to_string_lossy().to_string();
    assert!(
        validate_session_creation_cwd(&missing_under_archived, &[], &[archived_root]).is_ok(),
        "archived-root path allowed with existence waived"
    );
}

/// Round-3 regression (CI): the archived-root gate exemption must be structural
/// and independent of on-disk state. When the archived root resolves through a
/// symlink/junction but the cwd below it does not (yet) exist, the OLD
/// canonicalizing check failed on Ubuntu (the canonical root no longer prefixed the
/// raw cwd), falsely refusing a legitimate archived-root spawn. This pins the fix:
/// the cwd is Ok regardless of the symlink resolution or of the missing child.
/// Skipped gracefully on hosts that forbid symlink creation (e.g. non-developer
/// Windows); runs on Linux CI where the original failure occurred.
#[test]
fn archived_root_exemption_is_structural_even_when_root_resolves_through_symlink() {
    let base = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let link = base.path().join("link");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(target.path(), &link);
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(target.path(), &link);
    if let Err(_error) = made {
        return;
    }
    let archived_root = link.to_string_lossy().to_string();
    let missing_child = link.join("not-there").to_string_lossy().to_string();
    assert!(
        validate_session_creation_cwd(&missing_child, &[], &[archived_root]).is_ok(),
        "archived-root exemption must hold when the root resolves through a symlink and the cwd \
         does not exist"
    );
}

/// Test 12 (pure): rejects unregistered and missing cwds; accepts an
/// existing registered root; home-dir with empty roots is rejected (S5).
#[test]
fn validate_session_creation_cwd_rejects_unregistered_and_missing() {
    let temp = tempfile::tempdir().unwrap();
    // Derive the root and the missing cwd from the CANONICAL path so the
    // prefix membership check is robust to Windows Temp-drive junctions
    // (a real root here canonicalizes to itself; the retained root is
    // re-canonicalized identically inside the gate).
    let root = std::fs::canonicalize(temp.path())
        .expect("canonical root")
        .to_string_lossy()
        .to_string();
    let retained = vec![root.clone()];

    // Outside all registered roots (a sibling of the root).
    let parent = std::path::Path::new(&root)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap();
    let outside = format!("{}/elsewhere", parent);
    let err = validate_session_creation_cwd(&outside, &retained, &[]).unwrap_err();
    assert!(err.starts_with("sessionCreateBlocked:"), "{err}");
    assert!(err.contains("outside all registered projects"), "{err}");

    // Registered (root) but the cwd does not exist on disk.
    let missing = format!("{}/gone/deeper", root);
    let err = validate_session_creation_cwd(&missing, &retained, &[]).unwrap_err();
    assert!(err.starts_with("sessionCreateBlocked:"), "{err}");
    assert!(err.contains("does not exist on disk"), "{err}");

    // Registered existing root -> Ok.
    assert!(validate_session_creation_cwd(&root, &retained, &[]).is_ok());

    // S5 predicate: a home-dir-like input with empty registered roots is
    // refused (as the production `create_session` default would be).
    let home = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    if !home.is_empty() {
        let err = validate_session_creation_cwd(&home, &[], &[]).unwrap_err();
        assert!(err.starts_with("sessionCreateBlocked:"), "{err}");
    }
}

/// #2716 (B3) E42 - the unreadable mark is keyed by directory: marking A
/// never suppresses B's save, and loading B's readable agents file never
/// clears A's key. The mark is set by the real loader, not a test setter.
#[test]
fn the_unreadable_mark_is_scoped_to_its_own_directory() {
    use crate::config::instance_artifacts::{AGENTS_INSTANCE_FILE_NAME, SETTINGS_FILE_NAME};
    let settings_json = r#"{ "defaultShell": "test-shell", "defaultShellArgs": [] }"#;
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    for dir in [a.path(), b.path()] {
        std::fs::write(dir.join(SETTINGS_FILE_NAME), settings_json).unwrap();
    }
    std::fs::write(a.path().join(AGENTS_INSTANCE_FILE_NAME), "{ not json").unwrap();
    std::fs::write(
        b.path().join(AGENTS_INSTANCE_FILE_NAME),
        r#"{ "agents": [] }"#,
    )
    .unwrap();
    let a_seed = PersistedSession {
        name: "a-seed".into(),
        working_directory: "C:/projects/a".into(),
        ..Default::default()
    };
    save_sessions_to_dir(a.path(), &[a_seed]).expect("seed A before the mark");
    let a_before = std::fs::read(a.path().join("sessions.json")).unwrap();

    let a_settings =
        crate::config::settings::load_settings_from_path(&a.path().join(SETTINGS_FILE_NAME));
    assert!(a_settings.agents_layer.unreadable_path().is_some());
    let b_settings =
        crate::config::settings::load_settings_from_path(&b.path().join(SETTINGS_FILE_NAME));
    assert!(b_settings.agents_layer.unreadable_path().is_none());

    let row = PersistedSession {
        name: "written".into(),
        working_directory: "C:/projects/b".into(),
        ..Default::default()
    };
    save_sessions_to_dir(b.path(), std::slice::from_ref(&row)).expect("save into B");
    let b_written = std::fs::read_to_string(b.path().join("sessions.json"))
        .expect("B's save was suppressed by A's mark");
    assert!(b_written.contains("written"));

    save_sessions_to_dir(a.path(), &[row]).expect("a suppressed save answers Ok");
    assert_eq!(
        std::fs::read(a.path().join("sessions.json")).unwrap(),
        a_before,
        "A's sessions.json was rewritten under the mark"
    );
    assert!(
        crate::config::settings::agents_layer_unreadable_for_dir(a.path()),
        "loading B's readable agents file cleared A's key"
    );
}

/// #2716 (B3) - marks `dir` through the real loader: a settings file plus an
/// unparseable agents file, loaded by `load_settings_from_path`.
fn b3_mark_dir_unreadable(dir: &std::path::Path) -> crate::config::settings::AppSettings {
    use crate::config::instance_artifacts::{AGENTS_INSTANCE_FILE_NAME, SETTINGS_FILE_NAME};
    std::fs::write(
        dir.join(SETTINGS_FILE_NAME),
        r#"{ "defaultShell": "test-shell", "defaultShellArgs": [] }"#,
    )
    .unwrap();
    std::fs::write(dir.join(AGENTS_INSTANCE_FILE_NAME), "{ not json").unwrap();
    let settings = crate::config::settings::load_settings_from_path(&dir.join(SETTINGS_FILE_NAME));
    assert!(settings.agents_layer.unreadable_path().is_some());
    settings
}

fn b3_working_agent_row(cwd: &std::path::Path) -> PersistedSession {
    PersistedSession {
        name: "working-agent".into(),
        working_directory: cwd.to_string_lossy().to_string(),
        agent_id: Some("claude".into()),
        id: Some("00000000-0000-0000-0000-000000000001".into()),
        status: Some(SessionStatus::Running),
        waiting_for_input: Some(false),
        ..Default::default()
    }
}

fn b3_sha(path: &std::path::Path) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(std::fs::read(path).unwrap()))
}

/// #2716 (B3) E34a: with the mark set by the real loader, two distinct
/// writers, a persist and the startup purge, leave `sessions.json` with its
/// exact bytes. The purge fixture really writes when the mark is clear, so a
/// gate relocated to the persist functions still fails this row.
#[tokio::test]
async fn an_unreadable_sidecar_suppresses_every_session_file_write() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let outside = temp.path().join("outside");
    let kept = project.join(".ac").join("wg-1").join("__agent_keep");
    let dropped = outside.join(".ac").join("wg-1").join("__agent_old");
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::create_dir_all(&dropped).unwrap();
    let agent_row = b3_working_agent_row(&kept);
    let plain_row = PersistedSession {
        name: "outside".into(),
        working_directory: dropped.to_string_lossy().to_string(),
        ..Default::default()
    };
    save_sessions_to_dir(temp.path(), &[agent_row, plain_row]).expect("seed sessions");
    let sessions_path = temp.path().join("sessions.json");
    let before = b3_sha(&sessions_path);
    let project_paths = vec![project.to_string_lossy().to_string()];

    b3_mark_dir_unreadable(temp.path());

    // Writer one: a persist of a live snapshot that no longer carries the
    // skipped agent session, only a plain terminal.
    let plain_terminal = PersistedSession {
        name: "plain-terminal".into(),
        working_directory: kept.to_string_lossy().to_string(),
        ..Default::default()
    };
    let prep = super::super::prepare_persist_snapshot(vec![plain_terminal], &project_paths)
        .await
        .unwrap();
    super::super::persist_prepared_locked(
        None,
        temp.path(),
        prep,
        super::super::PersistMode::NoPrune,
        &super::super::acquire_carrier_mutation_permit().await,
    )
    .await
    .expect("a suppressed persist answers Ok");
    assert_eq!(
        b3_sha(&sessions_path),
        before,
        "the persist rewrote sessions.json"
    );

    // Writer two: the startup purge, which drops the row outside the project.
    super::super::purge_sessions_outside_project_paths_in_dir_locked(
        temp.path(),
        &project_paths,
        None,
    )
    .await
    .expect("a suppressed purge answers Ok");
    assert_eq!(
        b3_sha(&sessions_path),
        before,
        "the purge rewrote sessions.json"
    );
}

/// #2716 (B3) E34b: start one runs under the mark, the sidecar is repaired,
/// and start two still finds the agent row eligible to wake, with its resume
/// prompt; a write in start two lands and the exact key is gone.
#[tokio::test]
async fn the_restore_path_preserves_wake_eligibility_across_a_repair() {
    use crate::config::instance_artifacts::{AGENTS_INSTANCE_FILE_NAME, SETTINGS_FILE_NAME};
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let kept = project.join(".ac").join("wg-1").join("__agent_keep");
    std::fs::create_dir_all(&kept).unwrap();
    save_sessions_to_dir(temp.path(), &[b3_working_agent_row(&kept)]).expect("seed");
    let project_paths = vec![project.to_string_lossy().to_string()];

    // Start one: the mark is set, the agent session is skipped, and the final
    // persist rebuilds from a live snapshot that does not carry it.
    b3_mark_dir_unreadable(temp.path());
    let prep = super::super::prepare_persist_snapshot(Vec::new(), &project_paths)
        .await
        .unwrap();
    super::super::persist_prepared_locked(
        None,
        temp.path(),
        prep,
        super::super::PersistMode::NoPrune,
        &super::super::acquire_carrier_mutation_permit().await,
    )
    .await
    .unwrap();

    // Repair, then start two.
    std::fs::write(
        temp.path().join(AGENTS_INSTANCE_FILE_NAME),
        r#"{ "agents": [] }"#,
    )
    .unwrap();
    let mut settings =
        crate::config::settings::load_settings_from_path(&temp.path().join(SETTINGS_FILE_NAME));
    assert!(settings.agents_layer.unreadable_path().is_none());
    settings.restart_resume_wake_working_agents = true;
    settings.restart_resume_agent_prompt = "resume".to_string();
    let rows = super::super::load_sessions_from_dir(temp.path());
    let row = rows
        .iter()
        .find(|row| row.name == "working-agent")
        .expect("start one deleted the agent row");
    let working =
        crate::session::session::persisted_is_working(row.status.as_ref(), row.waiting_for_input);
    assert!(working, "start one destroyed the wake inputs: {row:?}");
    assert!(crate::should_wake_working_agent_on_restore(
        settings.restart_resume_wake_working_agents,
        false,
        working
    ));
    assert_eq!(
        crate::commands::session::restart_resume_prompt_for(&settings, false, false, working),
        Some("resume")
    );

    // A write in start two lands, and the exact key the leaf reads is gone.
    assert!(!crate::config::settings::agents_layer_unreadable_for_dir(
        temp.path()
    ));
    let mut next = rows.clone();
    next.push(PersistedSession {
        name: "start-two".into(),
        working_directory: kept.to_string_lossy().to_string(),
        ..Default::default()
    });
    save_sessions_to_dir(temp.path(), &next).expect("start two write");
    let reread = super::super::load_sessions_from_dir(temp.path());
    assert!(
        reread.iter().any(|row| row.name == "start-two"),
        "start two's write was suppressed"
    );
}
