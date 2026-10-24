use super::*;
use std::sync::Mutex;

// resolve_plan mutates/reads process env; serialize the env-touching tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn first_run() -> ContainerFirstRunState {
    ContainerFirstRunState {
        file: ".claude.json",
        onboarding_flag: "hasCompletedOnboarding",
        projects_key: "projects",
        project_flags: &["hasTrustDialogAccepted", "hasCompletedProjectOnboarding"],
    }
}

fn source(host_dir_env: Option<&'static str>) -> ContainerCredentialSource {
    ContainerCredentialSource {
        host_dir: ".ac-nonexistent-home-930",
        host_dir_env,
        file: ".credentials.json",
        container_dir: ".claude",
        first_run: Some(first_run()),
    }
}

/// A plan pointing at `<root>/.claude/.credentials.json`, like resolve_plan
/// builds for a container Claude session.
fn plan_for(root: &Path, first_run: Option<ContainerFirstRunState>) -> ContainerCredentialPlan {
    ContainerCredentialPlan {
        source: root.join("unused-host-source"),
        dest: root.join(".claude").join(".credentials.json"),
        first_run,
    }
}

fn with_env<R>(key: &str, value: Option<&str>, f: impl FnOnce() -> R) -> R {
    let prev = std::env::var_os(key);
    match value {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
    let out = f();
    match prev {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
    out
}

#[test]
fn resolve_plan_uses_absolute_env_override_and_builds_dest() {
    let _g = ENV_LOCK.lock().unwrap();
    let key = "AC_TEST_CRED_DIR_930_ABS";
    let host_dir = tempfile::tempdir().expect("host dir");
    std::fs::write(host_dir.path().join(".credentials.json"), b"tok").expect("write cred");
    let host_root = tempfile::tempdir().expect("host root");

    let plan = with_env(key, Some(&host_dir.path().to_string_lossy()), || {
        resolve_plan(&source(Some(key)), &host_root.path().to_string_lossy())
    })
    .expect("plan present");

    assert_eq!(plan.source, host_dir.path().join(".credentials.json"));
    assert_eq!(
        plan.dest,
        host_root.path().join(".claude").join(".credentials.json")
    );
    // #930 - the first-run descriptor must reach the plan: the container
    // backend sees only the plan, never the agent profile.
    assert_eq!(plan.first_run, Some(first_run()));
}

#[test]
fn resolve_plan_skips_when_source_file_absent() {
    let _g = ENV_LOCK.lock().unwrap();
    let key = "AC_TEST_CRED_DIR_930_MISSING";
    let host_dir = tempfile::tempdir().expect("host dir"); // empty, no cred file
    let host_root = tempfile::tempdir().expect("host root");

    let plan = with_env(key, Some(&host_dir.path().to_string_lossy()), || {
        resolve_plan(&source(Some(key)), &host_root.path().to_string_lossy())
    });
    assert!(plan.is_none(), "missing host file must be a clean skip");
}

#[test]
fn resolve_plan_ignores_relative_env_override_f6() {
    let _g = ENV_LOCK.lock().unwrap();
    let key = "AC_TEST_CRED_DIR_930_REL";
    let host_root = tempfile::tempdir().expect("host root");

    // A relative override must be ignored (F6), so the result is identical to
    // having no override at all (both fall back to ~/<host_dir>, which does
    // not contain our sentinel file).
    let with_rel = with_env(key, Some("relative/not-absolute-930"), || {
        resolve_plan(&source(Some(key)), &host_root.path().to_string_lossy())
    });
    let with_none = with_env(key, None, || {
        resolve_plan(&source(None), &host_root.path().to_string_lossy())
    });
    assert_eq!(with_rel, with_none, "relative override must be ignored");
}

#[test]
fn copy_in_then_remove_copied_is_idempotent() {
    let src_dir = tempfile::tempdir().expect("src");
    let src = src_dir.path().join(".credentials.json");
    std::fs::write(&src, b"secret-bytes").expect("write src");
    let dest_root = tempfile::tempdir().expect("dest");
    let dest = dest_root.path().join(".claude").join(".credentials.json");
    let plan = ContainerCredentialPlan {
        source: src,
        dest: dest.clone(),
        first_run: None,
    };

    assert_eq!(
        copy_in(&plan).expect("copy_in ok"),
        CopyOutcome::Copied,
        "a real copy must report Copied"
    );
    assert!(dest.is_file(), "dest created");
    assert_eq!(std::fs::read(&dest).unwrap(), b"secret-bytes");

    remove_copied(&dest);
    assert!(!dest.exists(), "dest removed");
    // Idempotent: a second delete of a now-missing file is a no-op.
    remove_copied(&dest);
    assert!(!dest.exists());
}

#[test]
fn ensure_first_run_state_merges_into_existing_config_without_touching_other_keys() {
    // #930 - the replica config dir already holds a seeded .claude.json.
    // Add the flags, preserve everything else (identity fields, other
    // projects), or we would clobber user config.
    let root = tempfile::tempdir().expect("root");
    let dir = root.path().join(".claude");
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join(".claude.json");
    std::fs::write(
        &path,
        include_str!("tests/fixtures/existing_config.json").as_bytes(),
    )
    .expect("seed config");

    ensure_first_run_state(&plan_for(root.path(), Some(first_run())), "/workspace");

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    assert_eq!(v["hasCompletedOnboarding"], serde_json::json!(true));
    assert_eq!(
        v["projects"]["/workspace"]["hasTrustDialogAccepted"],
        serde_json::json!(true)
    );
    assert_eq!(
        v["projects"]["/workspace"]["hasCompletedProjectOnboarding"],
        serde_json::json!(true)
    );
    // Untouched: pre-existing keys and unrelated project entries.
    assert_eq!(v["userID"], serde_json::json!("abc"));
    assert_eq!(
        v["oauthAccount"]["emailAddress"],
        serde_json::json!("x@y.z")
    );
    assert_eq!(
        v["projects"]["/other"]["hasTrustDialogAccepted"],
        serde_json::json!(false)
    );
}

#[test]
fn ensure_first_run_state_creates_config_when_absent() {
    // #930 - an empty config dir onboards too, so "no config seed" does not
    // save us: the file must be created.
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join(".claude").join(".claude.json");
    assert!(!path.exists());

    ensure_first_run_state(&plan_for(root.path(), Some(first_run())), "/workspace");

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    assert_eq!(v["hasCompletedOnboarding"], serde_json::json!(true));
    assert_eq!(
        v["projects"]["/workspace"]["hasTrustDialogAccepted"],
        serde_json::json!(true)
    );
    assert_eq!(
        v["projects"]["/workspace"]["hasCompletedProjectOnboarding"],
        serde_json::json!(true)
    );
}

#[test]
fn ensure_first_run_state_skips_unparseable_config() {
    // #930 - never clobber a config we cannot parse: warn and leave it byte
    // for byte. The user sees onboarding, which is exactly today's behavior.
    let root = tempfile::tempdir().expect("root");
    let dir = root.path().join(".claude");
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join(".claude.json");
    std::fs::write(&path, b"{not json").expect("seed config");

    ensure_first_run_state(&plan_for(root.path(), Some(first_run())), "/workspace");

    assert_eq!(
        std::fs::read(&path).expect("read"),
        b"{not json".to_vec(),
        "an unparseable config must be left untouched"
    );
}

#[test]
fn ensure_first_run_state_writes_nothing_without_a_descriptor() {
    // #930 - agents with no first-run descriptor (Codex, Antigravity) are
    // untouched: no file, not even the config dir, is created.
    let root = tempfile::tempdir().expect("root");

    ensure_first_run_state(&plan_for(root.path(), None), "/workspace");

    assert!(
        !root.path().join(".claude").exists(),
        "no descriptor must mean no writes at all"
    );
}

#[cfg(unix)]
fn try_symlink_file(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn try_symlink_file(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

#[test]
fn skipped_copy_reports_skipped_and_never_stamps_first_run_state() {
    // grinch Finding 1 - a prior container plants a symlink at the credential
    // leaf (the mount is RW: exactly the F2 threat model). F2 refuses to write
    // through it, so NO token exists. The caller must see SkippedReparse and
    // NOT stamp first-run state: suppressing the login wizard on a container
    // that has no credential would strand the agent unauthenticated, with no
    // way to notice. A skipped copy is not a copy.
    let root = tempfile::tempdir().expect("root");
    let src_dir = tempfile::tempdir().expect("src");
    let src = src_dir.path().join(".credentials.json");
    std::fs::write(&src, b"secret-bytes").expect("write src");

    let dir = root.path().join(".claude");
    std::fs::create_dir_all(&dir).expect("dir");
    let dest = dir.join(".credentials.json");
    let planted = src_dir.path().join("attacker-target");
    if !try_symlink_file(&planted, &dest) {
        // Creating a symlink needs a privilege on Windows; skip cleanly.
        eprintln!("skipping: no symlink privilege on this platform");
        return;
    }

    let plan = ContainerCredentialPlan {
        source: src,
        dest: dest.clone(),
        first_run: Some(first_run()),
    };

    // Mirror the backend call site (container_backend.rs): stamp ONLY on
    // Copied. If copy_in ever reports a skip as Copied again, this takes the
    // Copied arm, stamps, and the assertion below fails.
    match copy_in(&plan).expect("an F2 skip is not an error") {
        CopyOutcome::Copied => ensure_first_run_state(&plan, "/workspace"),
        CopyOutcome::SkippedReparse => {}
    }

    assert!(
        !planted.exists(),
        "F2: must never write the token through the planted link"
    );
    assert!(
        !dir.join(".claude.json").exists(),
        concat!(
            "a skipped copy must not stamp first-run state: with no token, ",
            "the login wizard is the correct UX"
        )
    );
}

#[test]
fn is_reparse_path_false_for_plain_file_and_missing() {
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("plain");
    std::fs::write(&file, b"x").expect("write");
    assert!(!is_reparse_path(&file), "plain file is not a reparse point");
    assert!(
        !is_reparse_path(&dir.path().join("does-not-exist")),
        "missing path is not a reparse point"
    );
}
