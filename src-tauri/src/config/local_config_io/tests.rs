use super::*;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn transient_publish_errors_are_classified_for_retry() {
    for code in super::TRANSIENT_PUBLISH_OS_ERRORS {
        let err = std::io::Error::from_raw_os_error(code);
        assert!(
            is_transient_publish_error(&err),
            "os error {code} should be treated as transient"
        );
    }
    // A permanent code (ERROR_FILE_NOT_FOUND) must not be retried.
    let permanent = std::io::Error::from_raw_os_error(2);
    assert!(!is_transient_publish_error(&permanent));
}

#[test]
fn transient_publish_error_message_is_human_readable() {
    let err = std::io::Error::from_raw_os_error(1175);
    let msg = format_publish_error(
        Path::new("C:/x/config.json"),
        Path::new("C:/x/.config.json.1.tmp"),
        &err,
    );
    assert!(msg.contains("briefly locked"), "{msg}");
    assert!(msg.contains("try again"), "{msg}");
    assert!(msg.contains("1175"), "{msg}");
    assert!(!msg.contains("Failed to replace"), "{msg}");
}

#[test]
fn permanent_publish_error_message_keeps_low_level_detail() {
    let err = std::io::Error::from_raw_os_error(2);
    let msg = format_publish_error(Path::new("C:/x/config.json"), Path::new("C:/x/.tmp"), &err);
    assert!(msg.contains("Failed to replace"), "{msg}");
}

#[test]
fn update_config_json_object_rejects_invalid_existing_json() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    std::fs::write(&path, "{ invalid").unwrap();

    let err = update_config_json_object(&path, false, |_| Ok(())).unwrap_err();

    assert!(err.contains("Failed to parse"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ invalid");
}

#[test]
fn write_file_atomic_keeps_original_when_publish_fails() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    std::fs::write(&path, b"before").unwrap();

    let err = write_file_atomic_with_publish(&path, b"after", |_tmp, _path| {
        Err("forced publish failure".to_string())
    })
    .unwrap_err();

    assert!(err.contains("forced publish failure"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), b"before");
    let tmp_entries = std::fs::read_dir(temp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .collect::<Vec<_>>();
    assert!(tmp_entries.is_empty(), "temp files left behind");
}

#[test]
fn update_config_json_object_preserves_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    std::fs::write(&path, r#"{"identity":"../../_agent_a","tooling":{"x":1}}"#).unwrap();

    let value = update_config_json_object(&path, false, |obj| {
        obj.insert("context".to_string(), serde_json::json!(["Role.md"]));
        Ok(())
    })
    .unwrap();

    assert_eq!(value["identity"], "../../_agent_a");
    assert_eq!(value["tooling"]["x"], 1);
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(saved["context"][0], "Role.md");
}

#[test]
fn agent_replica_root_config_writes_go_through_shared_helper() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // #1938 - the roots are every crate area that can hold a replica or
    // local config writer. The scan reports matches; it never whitelists
    // them, so an unexpected writer fails here and is fixed (or escalated
    // with evidence), never silenced.
    let roots = [
        manifest.join("src/config"),
        manifest.join("src/commands"),
        manifest.join("src/cli"),
        manifest.join("src/phone"),
        manifest.join("src/web"),
    ];
    let mut offenders = Vec::new();
    for root in roots {
        scan_dir(&root, &mut offenders);
    }

    assert!(
        offenders.is_empty(),
        "local config writers must use update_config_json_object:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn direct_write_guard_detects_async_and_shorthand_writes() {
    assert!(line_mentions_direct_write(
        "tokio::fs::write(&path, body).await?;"
    ));
    assert!(line_mentions_direct_write("fs::write(&path, body)?;"));
    assert!(!line_mentions_direct_write("safe_fs::write(&path, body)?;"));
}

#[test]
fn direct_write_guard_detects_multiline_config_target_binding() {
    let source = r#"
pub fn bad(agent_dir: &Path) -> Result<(), String> {
let target = agent_dir.join("config.json");
fs::write(&target, "{}")?;
Ok(())
}
"#;
    let mut offenders = Vec::new();
    scan_source(
        Path::new("src/config/agent_config.rs"),
        source,
        &mut offenders,
    );

    assert_eq!(offenders.len(), 1);
    assert!(offenders[0].contains("fs::write"), "{offenders:?}");
}

fn scan_dir(root: &Path, offenders: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Test-only files (`foo/tests.rs`, `foo/tests/`, `foo/<name>_tests.rs`) hold
        // what `strip_test_modules` cuts off when the tests live inline.
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("");
        if stem == "tests" || stem.ends_with("_tests") {
            continue;
        }
        if path.is_dir() {
            scan_dir(&path, offenders);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        scan_file(&path, offenders);
    }
}

fn scan_file(path: &Path, offenders: &mut Vec<String>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let stripped = strip_test_modules(&content);
    scan_source(path, &stripped, offenders);
}

fn scan_source(path: &Path, content: &str, offenders: &mut Vec<String>) {
    let mut config_target_vars = HashSet::new();
    let mut depth = 0;
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if depth == 0 {
            config_target_vars.clear();
        }
        if let Some(var) = local_config_target_binding(trimmed) {
            config_target_vars.insert(var);
        }
        let mentions_local_target = line_mentions_local_config_target(trimmed)
            || line_mentions_tracked_config_target(trimmed, &config_target_vars);
        if !line_mentions_direct_write(trimmed) || !mentions_local_target {
            depth += brace_delta(line);
            if depth <= 0 {
                depth = 0;
                config_target_vars.clear();
            }
            continue;
        }
        if is_allowed_line(path, trimmed) {
            depth += brace_delta(line);
            if depth <= 0 {
                depth = 0;
                config_target_vars.clear();
            }
            continue;
        }
        offenders.push(format!("{}:{}: {}", path.display(), idx + 1, trimmed));
        depth += brace_delta(line);
        if depth <= 0 {
            depth = 0;
            config_target_vars.clear();
        }
    }
}

fn line_mentions_direct_write(line: &str) -> bool {
    line_contains_call_path(line, "std::fs::write")
        || line_contains_call_path(line, "tokio::fs::write")
        || line_contains_call_path(line, "fs::write")
        || line_contains_call_path(line, "File::create")
        || line_contains_call_path(line, "OpenOptions::new")
        || line.contains(".write_all")
}

fn line_mentions_local_config_target(line: &str) -> bool {
    line.contains("config.json")
        || line.contains("join(\"config.json\")")
        || line.contains("config_path")
}

fn local_config_target_binding(line: &str) -> Option<String> {
    if !line_mentions_local_config_target(line) {
        return None;
    }
    let rest = line.strip_prefix("let ")?;
    let rest = rest.strip_prefix("mut ").unwrap_or(rest);
    let name = rest.split('=').next()?.trim();
    let name = name.split(':').next().unwrap_or(name).trim();
    if !is_rust_identifier(name) {
        return None;
    }
    Some(name.to_string())
}

fn line_mentions_tracked_config_target(line: &str, vars: &HashSet<String>) -> bool {
    vars.iter().any(|var| line_mentions_identifier(line, var))
}

fn line_contains_call_path(line: &str, needle: &str) -> bool {
    let mut rest = line;
    while let Some(idx) = rest.find(needle) {
        let before = rest[..idx].chars().next_back();
        let after = &rest[idx + needle.len()..];
        let before_ok = before.is_none_or(|ch| !is_rust_identifier_char(ch));
        let after_ok = after.chars().find(|ch| !ch.is_ascii_whitespace()) == Some('(');
        if before_ok && after_ok {
            return true;
        }
        rest = &rest[idx + needle.len()..];
    }
    false
}

fn line_mentions_identifier(line: &str, ident: &str) -> bool {
    let mut rest = line;
    while let Some(idx) = rest.find(ident) {
        let before = rest[..idx].chars().next_back();
        let after = rest[idx + ident.len()..].chars().next();
        let before_ok = before.is_none_or(|ch| !is_rust_identifier_char(ch));
        let after_ok = after.is_none_or(|ch| !is_rust_identifier_char(ch));
        if before_ok && after_ok {
            return true;
        }
        rest = &rest[idx + ident.len()..];
    }
    false
}

fn is_rust_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic()) && chars.all(is_rust_identifier_char)
}

fn is_rust_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn is_allowed_line(path: &Path, line: &str) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.ends_with("src/config/local_config_io.rs")
        || (normalized.ends_with("src/commands/entity_creation.rs")
            && (line.contains("write_team_config")
                || line.contains("create_new_team_config_on_disk")
                || line.contains("team_dir.join(\"config.json\")")))
}

fn strip_test_modules(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("#[cfg(test)]") {
            out.push('\n');
            if let Some(next) = lines.next() {
                if next.trim_start().starts_with("mod tests") {
                    out.push_str(&blank_braced_block(next, &mut lines));
                    continue;
                }
                out.push_str(next);
                out.push('\n');
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn blank_braced_block<'a>(first_line: &str, lines: &mut impl Iterator<Item = &'a str>) -> String {
    let mut blanked = String::new();
    let mut depth = brace_delta(first_line);
    blanked.push('\n');
    while depth > 0 {
        let Some(line) = lines.next() else {
            break;
        };
        depth += brace_delta(line);
        blanked.push('\n');
    }
    blanked
}

fn brace_delta(line: &str) -> i32 {
    let opens = line.chars().filter(|ch| *ch == '{').count() as i32;
    let closes = line.chars().filter(|ch| *ch == '}').count() as i32;
    opens - closes
}

fn non_utf8_file_name() -> std::ffi::OsString {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        // An unpaired surrogate: a legal OsString, never a legal `&str`.
        std::ffi::OsString::from_wide(&[0xD800])
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0xff, 0xfe])
    }
    #[cfg(not(any(windows, unix)))]
    {
        std::ffi::OsString::from("config.json")
    }
}

/// #1209, instance-dir half: every temporary name this module can publish is
/// inside the pattern the instance `.gitignore` emits for atomic-write
/// temporaries.
///
/// The assertion calls the registry's own predicate instead of restating the
/// glob. A restatement does not follow the pattern when the pattern changes,
/// which would leave the tie this test is named for untied; the predicate
/// lives next to the pattern and a registry test pins their agreement.
///
/// The private allocator actually creates the stage; this fixture
/// verifies the returned name against the existing artifact policy.
#[test]
fn atomic_temp_names_stay_inside_the_ignored_glob() {
    use crate::config::instance_artifacts::matches_atomic_write_tmp_glob;
    let root = tempfile::tempdir().unwrap();
    let mut targets: Vec<PathBuf> = [
        "settings.json",
        "sessions",
        "a.b.c.json",
        crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME,
        crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME,
    ]
    .iter()
    .map(|name| root.path().join(name))
    .collect();
    targets.push(root.path().join(non_utf8_file_name()));
    for target in targets {
        let guard = super::PairTargetGuard::new(&|| Ok(()));
        let (temp, file) = super::acquire_fresh_config_stage(&target, &guard).unwrap();
        drop(file);
        let produced = temp.file_name().unwrap().to_str().unwrap();
        assert!(
            matches_atomic_write_tmp_glob(produced),
            "{produced:?} for {target:?}"
        );
        if target.file_name().unwrap()
            == crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME
            || target.file_name().unwrap()
                == crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME
        {
            let prefix = format!(".{}.", target.file_name().unwrap().to_str().unwrap());
            assert!(produced.starts_with(&prefix) && produced.ends_with(".tmp"));
        }
        assert!(temp.is_file());
    }
}

// -----------------------------------------------------------------------
// #1938 cross-process config write lock.
//
// The parent tests below re-execute this same lib test binary with
// `--exact config::local_config_io::tests::issue_1937_config_lock_child`
// and the child-only environment below. The child performs one action and
// exits; parents bound the child lifetime and assert the named test pass,
// not just the exit code. No app session, token, or real config directory
// is used: every fixture is a fresh tempdir.
// -----------------------------------------------------------------------

const CHILD_ACTION_ENV: &str = "AC_1937_CONFIG_LOCK_CHILD_ACTION";
const CHILD_DIR_ENV: &str = "AC_1937_CONFIG_LOCK_CHILD_DIR";
const CHILD_TIMEOUT_MS_ENV: &str = "AC_1937_CONFIG_LOCK_CHILD_TIMEOUT_MS";
const CHILD_KEY_ENV: &str = "AC_1937_CONFIG_LOCK_CHILD_KEY";
const CHILD_TEST_FQN: &str = "config::local_config_io::tests::issue_1937_config_lock_child";
const CHILD_READY_FILE: &str = "child-ready";
const CHILD_RELEASE_FILE: &str = "child-release";

fn lock_sidecar_path(config_path: &Path) -> PathBuf {
    let parent = config_path
        .parent()
        .expect("config path has a parent")
        .canonicalize()
        .expect("config parent canonicalizes");
    config_lock_path(&parent, config_path).expect("lock path")
}

fn wait_for_file(path: &Path, timeout: Duration, label: &str) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < timeout,
            "{label}: timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn spawn_config_lock_child(action: &str, dir: &Path, extra_env: &[(&str, &str)]) -> Child {
    let exe = std::env::current_exe().expect("current test exe");
    let mut command = Command::new(exe);
    command
        .args(["--exact", CHILD_TEST_FQN, "--nocapture", "--test-threads=1"])
        .env(CHILD_ACTION_ENV, action)
        .env(CHILD_DIR_ENV, dir)
        .env_remove(CHILD_TIMEOUT_MS_ENV)
        .env_remove(CHILD_KEY_ENV)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.spawn().expect("spawn config lock child")
}

fn wait_bounded(
    mut child: Child,
    timeout: Duration,
    label: &str,
) -> (std::process::ExitStatus, String, String) {
    let started = Instant::now();
    loop {
        match child.try_wait().expect("poll child") {
            Some(_) => break,
            None if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(20));
            }
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{label} exceeded its {timeout:?} lifetime bound and was killed");
            }
        }
    }
    let output = child.wait_with_output().expect("collect child output");
    (
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn assert_child_success(
    label: &str,
    status: std::process::ExitStatus,
    stdout: &str,
    stderr: &str,
    marker: &str,
) {
    assert!(
        status.success(),
        "{label} child failed: status={status:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(&format!("test {CHILD_TEST_FQN} ...")),
        "{label} child did not report {CHILD_TEST_FQN} running:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("test result: ok. 1 passed; 0 failed"),
        "{label} child did not report exactly one passing test:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(marker),
        "{label} child is missing marker {marker:?}:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// #1938 - the child half of the cross-process tests. Without the child-only
/// action environment this test is a no-op, so a normal suite run (and the
/// focused CI filter) passes it without touching any fixture.
#[test]
fn issue_1937_config_lock_child() {
    let Some(action) = std::env::var_os(CHILD_ACTION_ENV) else {
        return;
    };
    let action = action.to_string_lossy().into_owned();
    let dir = PathBuf::from(std::env::var_os(CHILD_DIR_ENV).expect("child dir env"));
    let config_path = dir.join("config.json");
    match action.as_str() {
        "hold" => {
            let _lock = acquire_config_file_write_lock(&config_path, CONFIG_LOCK_TIMEOUT)
                .expect("child must acquire the lock");
            std::fs::write(dir.join(CHILD_READY_FILE), b"ready")
                .expect("child must announce readiness");
            let deadline = Instant::now() + Duration::from_secs(60);
            while !dir.join(CHILD_RELEASE_FILE).exists() {
                assert!(
                    Instant::now() < deadline,
                    "child hold exceeded its 60s bound"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        "assert_timeout" => {
            let timeout_ms: u64 = std::env::var(CHILD_TIMEOUT_MS_ENV)
                .expect("child timeout env")
                .parse()
                .expect("child timeout ms");
            let error =
                acquire_config_file_write_lock(&config_path, Duration::from_millis(timeout_ms))
                    .expect_err("child must not acquire a held lock");
            assert!(error.contains("configLockTimeout"), "child saw: {error}");
            assert!(error.contains(".config.json.lock"), "child saw: {error}");
            println!("AC_1937_CONFIG_LOCK_CHILD_TIMEOUT_OK");
        }
        "update" => {
            let key = std::env::var(CHILD_KEY_ENV).expect("child key env");
            for index in 0..25_u32 {
                update_config_json_object(&config_path, true, |obj| {
                    obj.insert(key.clone(), serde_json::json!(index));
                    Ok(())
                })
                .expect("child update must succeed");
            }
            println!("AC_1937_CONFIG_LOCK_CHILD_UPDATE_OK key={key}");
        }
        other => panic!("unknown child action {other}"),
    }
    println!("AC_1937_CONFIG_LOCK_CHILD_DONE action={action}");
}

/// #1938 - the production deadline is 5 seconds and the injected short
/// deadline is honored: a bounded wait with a distinct diagnostic naming
/// the sidecar and the elapsed limit, then a clean reacquire.
#[test]
fn issue_1937_config_lock_timeout() {
    assert_eq!(CONFIG_LOCK_TIMEOUT, Duration::from_secs(5));
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.json");
    let held = acquire_config_file_write_lock(&path, CONFIG_LOCK_TIMEOUT).expect("hold lock");
    let started = Instant::now();
    let error = acquire_config_file_write_lock(&path, Duration::from_millis(150))
        .expect_err("a held lock must time out");
    let elapsed = started.elapsed();
    assert!(error.contains("configLockTimeout"), "{error}");
    assert!(error.contains(".config.json.lock"), "{error}");
    assert!(error.contains("150 ms"), "{error}");
    assert!(elapsed >= Duration::from_millis(150), "elapsed {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(2),
        "the wait must stay bounded, elapsed {elapsed:?}"
    );
    drop(held);
    let reacquired = acquire_config_file_write_lock(&path, Duration::from_millis(500))
        .expect("lock must be free after the holder drops");
    drop(reacquired);
    assert!(lock_sidecar_path(&path).is_file(), "sidecar must survive");
}

/// #1938 - exclusion across separate processes: a live holder in this
/// process makes a separate test process time out.
#[test]
fn issue_1937_config_lock_process_exclusion() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.json");
    std::fs::write(&path, b"{}\n").expect("seed config");
    let _held = acquire_config_file_write_lock(&path, CONFIG_LOCK_TIMEOUT).expect("hold lock");

    let child = spawn_config_lock_child(
        "assert_timeout",
        temp.path(),
        &[(CHILD_TIMEOUT_MS_ENV, "400")],
    );
    let (status, stdout, stderr) =
        wait_bounded(child, Duration::from_secs(60), "process_exclusion child");
    assert_child_success(
        "process_exclusion",
        status,
        &stdout,
        &stderr,
        "AC_1937_CONFIG_LOCK_CHILD_TIMEOUT_OK",
    );
    assert!(
        stdout.contains("AC_1937_CONFIG_LOCK_CHILD_DONE action=assert_timeout"),
        "{stdout}"
    );
}

/// #1938 - normal child exit releases the lock: while the child holds it the
/// parent times out, and after the child exits the parent reacquires.
#[test]
fn issue_1937_config_lock_process_release() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.json");
    std::fs::write(&path, b"{}\n").expect("seed config");

    let child = spawn_config_lock_child("hold", temp.path(), &[]);
    wait_for_file(
        &temp.path().join(CHILD_READY_FILE),
        Duration::from_secs(60),
        "child ready",
    );

    let error = acquire_config_file_write_lock(&path, Duration::from_millis(300))
        .expect_err("the child holds the lock");
    assert!(error.contains("configLockTimeout"), "{error}");

    std::fs::write(temp.path().join(CHILD_RELEASE_FILE), b"go").expect("release child");
    let (status, stdout, stderr) =
        wait_bounded(child, Duration::from_secs(60), "process_release child");
    assert_child_success(
        "process_release",
        status,
        &stdout,
        &stderr,
        "AC_1937_CONFIG_LOCK_CHILD_DONE action=hold",
    );

    let reacquired = acquire_config_file_write_lock(&path, Duration::from_secs(2))
        .expect("child exit must release the lock");
    drop(reacquired);
    assert!(lock_sidecar_path(&path).is_file(), "sidecar must survive");
}

/// #1938 - process death (kill, no unwind) also releases the OS lock in the
/// kernel, so no sidecar deletion or stale-lock cleanup is needed.
#[test]
fn issue_1937_config_lock_process_death_release() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.json");
    std::fs::write(&path, b"{}\n").expect("seed config");

    let mut child = spawn_config_lock_child("hold", temp.path(), &[]);
    wait_for_file(
        &temp.path().join(CHILD_READY_FILE),
        Duration::from_secs(60),
        "child ready",
    );
    let held_error = acquire_config_file_write_lock(&path, Duration::from_millis(200))
        .expect_err("the child holds the lock");
    assert!(held_error.contains("configLockTimeout"), "{held_error}");

    child.kill().expect("kill lock holder");
    let status = child.wait().expect("reap killed child");
    assert!(
        !status.success(),
        "killed child must not report success: {status:?}"
    );

    let reacquired = acquire_config_file_write_lock(&path, Duration::from_secs(2))
        .expect("process death must release the OS lock");
    drop(reacquired);
    assert!(
        lock_sidecar_path(&path).is_file(),
        "sidecar must survive process death"
    );
}

/// #1938 - separate handles in one process contend exactly like separate
/// processes, so the guarantee does not depend on one caller shape.
#[test]
fn issue_1937_config_lock_same_process_handles() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.json");
    let first = acquire_config_file_write_lock(&path, CONFIG_LOCK_TIMEOUT).expect("first handle");
    let error = acquire_config_file_write_lock(&path, Duration::from_millis(120))
        .expect_err("the second handle must contend");
    assert!(error.contains("configLockTimeout"), "{error}");
    drop(first);
    let second = acquire_config_file_write_lock(&path, Duration::from_millis(500))
        .expect("the first handle dropped");
    drop(second);
}

/// #1938 - canonicalization folds raw, dot-segment, symlink, and Windows
/// case / verbatim aliases of one directory onto the single physical
/// sidecar.
#[test]
fn issue_1937_config_lock_path_aliases_share_one_sidecar() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path();
    let path = dir.join("config.json");
    let held = acquire_config_file_write_lock(&path, CONFIG_LOCK_TIMEOUT).expect("hold lock");

    let nested = dir.join("nested");
    std::fs::create_dir_all(&nested).expect("nested dir");
    let mut aliases = vec![
        dir.join(".").join("config.json"),
        nested.join("..").join("config.json"),
    ];
    #[cfg(windows)]
    {
        // Windows paths are case-insensitive, so `.CONFIG.JSON.lock` and
        // `.config.json.lock` are the same physical file.
        aliases.push(dir.join("CONFIG.JSON"));
        aliases.push(PathBuf::from(format!(r"\\?\{}", dir.display())).join("config.json"));
    }
    #[cfg(unix)]
    {
        let link = dir.join("alias");
        std::os::unix::fs::symlink(dir, &link).expect("symlink alias");
        aliases.push(link.join("config.json"));
    }

    for alias in aliases {
        let error = acquire_config_file_write_lock(&alias, Duration::from_millis(120))
            .err()
            .unwrap_or_else(|| panic!("alias {alias:?} must share the held sidecar lock"));
        assert!(
            error.contains("configLockTimeout"),
            "alias {alias:?}: {error}"
        );
    }
    drop(held);
    assert!(lock_sidecar_path(&path).is_file());
}

/// #1938 - a malformed config is rejected under the lock and the original
/// bytes stay untouched.
#[test]
fn issue_1937_config_lock_malformed_json_preserved() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.json");
    std::fs::write(&path, b"{ invalid").expect("seed malformed config");

    let error =
        update_config_json_object(&path, false, |_| Ok(())).expect_err("malformed JSON must fail");
    assert!(error.contains("Failed to parse"), "{error}");
    assert_eq!(std::fs::read(&path).expect("read"), b"{ invalid");
    assert!(lock_sidecar_path(&path).is_file(), "sidecar must survive");
    let reacquired = acquire_config_file_write_lock(&path, Duration::from_millis(500))
        .expect("lock must be released after a parse failure");
    drop(reacquired);
}

/// #1938 - a failing mutation closure leaves the file untouched and releases
/// the lock.
#[test]
fn issue_1937_config_lock_closure_failure_preserved() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.json");
    let before = br#"{"keep":true}"#.to_vec();
    std::fs::write(&path, &before).expect("seed config");

    let error = update_config_json_object(&path, false, |obj| {
        obj.insert("scratch".to_string(), serde_json::json!(1));
        Err("forced closure failure".to_string())
    })
    .expect_err("the closure failure must surface");
    assert!(error.contains("forced closure failure"), "{error}");
    assert_eq!(std::fs::read(&path).expect("read"), before);
    assert!(lock_sidecar_path(&path).is_file(), "sidecar must survive");
    let reacquired = acquire_config_file_write_lock(&path, Duration::from_millis(500))
        .expect("lock must be released after a closure failure");
    drop(reacquired);
}

/// #1938 - an atomic publish failure preserves the prior bytes, removes the
/// temp file, releases the lock, and leaves the sidecar in place.
#[test]
fn issue_1937_config_lock_publish_failure_preserves_prior_bytes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.json");
    let before = br#"{"keep":true}"#.to_vec();
    std::fs::write(&path, &before).expect("seed config");

    let error = update_config_json_object_with_publish(
        &path,
        false,
        |obj| {
            obj.insert("scratch".to_string(), serde_json::json!(2));
            Ok(())
        },
        |_tmp, _target| Err("forced publish failure".to_string()),
    )
    .expect_err("the publish failure must surface");
    assert!(error.contains("forced publish failure"), "{error}");
    assert_eq!(std::fs::read(&path).expect("read"), before);
    let temp_files: Vec<PathBuf> = std::fs::read_dir(temp.path())
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|entry| entry.to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(
        temp_files.is_empty(),
        "temp files left behind: {temp_files:?}"
    );
    let lock_path = lock_sidecar_path(&path);
    assert!(
        lock_path.is_file(),
        "sidecar must survive a publish failure"
    );
    assert_eq!(
        std::fs::read(&lock_path).expect("read lock"),
        b"",
        "lock contents must never be written"
    );
    let reacquired = acquire_config_file_write_lock(&path, Duration::from_millis(500))
        .expect("lock must be released after a publish failure");
    drop(reacquired);
}

/// #1938 - the sidecar is never deleted, including error recovery, and it is
/// never truncated or written to.
#[test]
fn issue_1937_config_lock_sidecar_survives_every_outcome() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.json");
    std::fs::write(&path, b"{}\n").expect("seed config");

    update_config_json_object(&path, false, |obj| {
        obj.insert("ok".to_string(), serde_json::json!(true));
        Ok(())
    })
    .expect("successful update");
    let lock_path = lock_sidecar_path(&path);
    assert!(lock_path.is_file(), "sidecar must exist after success");
    assert_eq!(std::fs::read(&lock_path).expect("read"), b"");

    let _ = update_config_json_object(&path, false, |_| Err("boom".to_string()));
    assert!(
        lock_path.is_file(),
        "sidecar must exist after a closure failure"
    );

    std::fs::write(&path, b"{ nope").expect("seed malformed config");
    let _ = update_config_json_object(&path, false, |_| Ok(()));
    assert!(
        lock_path.is_file(),
        "sidecar must exist after a parse failure"
    );

    std::fs::write(&path, b"{}\n").expect("reseed config");
    let _ = update_config_json_object_with_publish(
        &path,
        false,
        |_| Ok(()),
        |_, _| Err("forced".to_string()),
    );
    assert!(
        lock_path.is_file(),
        "sidecar must exist after a publish failure"
    );
    assert_eq!(std::fs::read(&lock_path).expect("read"), b"");
}

/// #1938 - two live processes update disjoint keys; the cross-process lock
/// serializes their read-modify-publish cycles so both keys survive.
#[test]
fn issue_1937_config_lock_concurrent_disjoint_updates_both_survive() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.json");
    std::fs::write(&path, b"{}\n").expect("seed config");

    let alpha = spawn_config_lock_child("update", temp.path(), &[(CHILD_KEY_ENV, "alpha")]);
    let beta = spawn_config_lock_child("update", temp.path(), &[(CHILD_KEY_ENV, "beta")]);

    let (alpha_status, alpha_stdout, alpha_stderr) =
        wait_bounded(alpha, Duration::from_secs(120), "concurrent update alpha");
    let (beta_status, beta_stdout, beta_stderr) =
        wait_bounded(beta, Duration::from_secs(120), "concurrent update beta");

    assert_child_success(
        "concurrent alpha",
        alpha_status,
        &alpha_stdout,
        &alpha_stderr,
        "AC_1937_CONFIG_LOCK_CHILD_UPDATE_OK key=alpha",
    );
    assert_child_success(
        "concurrent beta",
        beta_status,
        &beta_stdout,
        &beta_stderr,
        "AC_1937_CONFIG_LOCK_CHILD_UPDATE_OK key=beta",
    );

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read saved config"))
            .expect("saved config is JSON");
    assert_eq!(saved["alpha"], serde_json::json!(24), "{saved}");
    assert_eq!(saved["beta"], serde_json::json!(24), "{saved}");
    assert!(lock_sidecar_path(&path).is_file(), "sidecar must survive");
}

#[cfg(windows)]
fn wide_to_string(wide: &[u16]) -> String {
    assert_eq!(wide.last(), Some(&0), "output must be NUL-terminated");
    String::from_utf16(&wide[..wide.len() - 1]).expect("valid UTF-16")
}

#[cfg(windows)]
#[test]
fn publish_path_wide_prefixes_a_plain_drive_path() {
    let out = super::publish_path_wide(Path::new(r"C:\a\config.json"));
    assert_eq!(wide_to_string(&out), r"\\?\C:\a\config.json");
}

#[cfg(windows)]
#[test]
fn publish_path_wide_normalizes_forward_slashes_and_unc() {
    let cases = [
        (r"C:/a/./config.json", r"\\?\C:\a\config.json"),
        (
            r"\\server\share\config.json",
            r"\\?\UNC\server\share\config.json",
        ),
        // Round-2 finding: the raw prefix text `//server/share` must not
        // be copied, or the server would be named `server/share`.
        (
            r"//server/share/config.json",
            r"\\?\UNC\server\share\config.json",
        ),
    ];
    for (input, expected) in cases {
        let out = super::publish_path_wide(Path::new(input));
        assert_eq!(wide_to_string(&out), expected, "input {input}");
    }
}

#[cfg(windows)]
#[test]
fn publish_path_wide_leaves_unconvertible_shapes_byte_identical() {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let mut nul_units: Vec<u16> = r"C:\a\conf".encode_utf16().collect();
    nul_units.push(0);
    nul_units.extend("ig.json".encode_utf16());
    let interior_nul = PathBuf::from(OsString::from_wide(&nul_units));

    let cases: Vec<(&str, PathBuf)> = vec![
        ("relative", PathBuf::from(r"sub\config.json")),
        ("drive-relative", PathBuf::from(r"C:config.json")),
        ("bare drive", PathBuf::from(r"C:")),
        ("parent dir", PathBuf::from(r"C:\a\..\config.json")),
        (
            "trailing-dot file name",
            PathBuf::from(r"C:\a\config.json."),
        ),
        (
            "trailing-space directory",
            PathBuf::from(r"C:\a \config.json"),
        ),
        ("already verbatim", PathBuf::from(r"\\?\C:\a\config.json")),
        ("device namespace", PathBuf::from(r"\\.\PIPE\x")),
        ("interior NUL", interior_nul),
    ];
    for (name, p) in cases {
        let expected: Vec<u16> = p
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>();
        assert_eq!(super::publish_path_wide(&p), expected, "case {name}");
    }
}

/// Builds a non-verbatim directory over 300 wide units holding an existing
/// `config.json` (old bytes) and a temp file (new bytes), and asserts the
/// fixture rules of plan #2378 section 5.1 before the caller acts.
#[cfg(windows)]
fn long_path_publish_fixture(root: &Path) -> (PathBuf, PathBuf) {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Prefix};

    let segment = "a".repeat(40);
    let mut dir = root.to_path_buf();
    while dir.as_os_str().encode_wide().count() <= 300 {
        dir.push(&segment);
    }
    std::fs::create_dir_all(&dir).expect("create long dir");
    let dest = dir.join("config.json");
    use std::io::Write;
    std::fs::write(&dest, b"old content").expect("write dest");
    let guard = super::PairTargetGuard::new(&|| Ok(()));
    let (tmp, mut file) = super::acquire_fresh_config_stage(&dest, &guard).unwrap();
    file.write_all(b"new content").unwrap();
    file.flush().unwrap();
    file.sync_all().unwrap();
    drop(file);

    for p in [&dest, &tmp] {
        match p.components().next() {
            Some(Component::Prefix(prefix)) => assert!(
                matches!(prefix.kind(), Prefix::Disk(_)),
                "fixture must be an ordinary Disk path, got {:?}",
                prefix.kind()
            ),
            other => panic!("fixture must start with a prefix, got {other:?}"),
        }
        let wide_len = p.as_os_str().encode_wide().count();
        assert!(
            wide_len > 260,
            "fixture length {wide_len} must exceed MAX_PATH"
        );
        assert!(p.exists(), "fixture file must exist: {}", p.display());
    }
    (dest, tmp)
}

#[cfg(windows)]
#[test]
fn publish_over_a_destination_longer_than_max_path_succeeds() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (dest, tmp) = long_path_publish_fixture(temp.path());
    assert!(dest.exists());
    assert!(tmp.exists());

    super::publish_temp_config(&tmp, &dest).expect("long-path publish");

    assert_eq!(std::fs::read(&dest).expect("read dest"), b"new content");
    assert!(!tmp.exists(), "temp file must be consumed");
}

/// Inverse control for the long-path publish test: raw non-verbatim
/// arguments still fail with ERROR_PATH_NOT_FOUND, so that test is not
/// vacuous. If this ever goes red, the process gained long-path awareness
/// (for example through a manifest); re-triage #2378 instead of relaxing
/// this test.
#[cfg(windows)]
#[test]
fn raw_non_verbatim_replacefilew_still_fails_over_max_path() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    let temp = tempfile::tempdir().expect("tempdir");
    let (dest, tmp) = long_path_publish_fixture(temp.path());
    assert!(dest.exists());
    assert!(tmp.exists());

    let dest_wide: Vec<u16> = dest
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let tmp_wide: Vec<u16> = tmp
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        ReplaceFileW(
            dest_wide.as_ptr(),
            tmp_wide.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    let err = std::io::Error::last_os_error();
    assert_eq!(ok, 0, "raw ReplaceFileW must fail over MAX_PATH");
    assert_eq!(err.raw_os_error(), Some(3), "{err}");
}

// -----------------------------------------------------------------------
// #2786 (C1) - the config pair primitive. The state path is a literal and
// the stage hook is test-local: this module must name no other module.
// -----------------------------------------------------------------------

const PAIR_STATE_NAME: &str = "config.state.no-git.json";
const PAIR_STATE_KEYS: [&str; 4] = [
    "lastCodingAgent",
    "codingAgents",
    "lastAgentMessageAt",
    "profileContentHash",
];

fn pair_fixture(
    decisions: Option<&str>,
    state: Option<&str>,
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let decisions_path = temp.path().join("config.json");
    let state_path = temp.path().join(PAIR_STATE_NAME);
    if let Some(body) = decisions {
        std::fs::write(&decisions_path, body).expect("seed decisions");
    }
    if let Some(body) = state {
        std::fs::write(&state_path, body).expect("seed state");
    }
    (temp, decisions_path, state_path)
}

fn no_stage(_stage: &str) {}

fn read_value(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read json")).expect("parse json")
}

fn split_marker() -> Value {
    json!({"v": 1, "keys": PAIR_STATE_KEYS})
}

#[test]
fn the_state_file_is_written_before_the_decisions_file() {
    let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), None);
    let debris = temp_config_path(&decisions);
    std::fs::create_dir(&debris).unwrap();
    let stages = std::cell::RefCell::new(Vec::new());
    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &|stage| stages.borrow_mut().push(stage.to_string()),
        |d, s| {
            d.insert("a".to_string(), json!(2));
            s.insert("tooling".to_string(), json!({"lastCodingAgent": "claude"}));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(read_value(&state)["tooling"]["lastCodingAgent"], "claude");
    assert_eq!(read_value(&decisions)["a"], 2);
    assert_eq!(
        stages.into_inner(),
        vec!["after_caller_state_publish", "after_caller_decisions_write"]
    );
    assert!(debris.is_dir());
}

#[test]
fn an_absent_state_file_is_an_empty_tooling() {
    let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), None);
    let saw_empty = std::cell::Cell::new(false);

    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |_d, s| {
            saw_empty.set(s.is_empty());
            Ok(())
        },
    )
    .expect("an absent state file is a valid state");

    assert!(saw_empty.get(), "the closure must see an empty state map");
    assert!(
        !state.exists(),
        "an untouched absent state file stays absent"
    );
}

#[test]
fn an_untouched_side_is_not_written() {
    // Decisions only: no state file appears.
    let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), None);
    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |d, _s| {
            d.insert("a".to_string(), json!(2));
            Ok(())
        },
    )
    .expect("decisions-only pair call");
    assert_eq!(read_value(&decisions)["a"], 2);
    assert!(
        !state.exists(),
        "a decisions-only call must not create a state file"
    );

    // State only: the decisions file stays byte-identical.
    let original = "{\"a\":1,  \"b\" : [1,2]}";
    let (_temp, decisions, state) = pair_fixture(Some(original), None);
    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |_d, s| {
            s.insert(
                "tooling".to_string(),
                json!({"lastAgentMessageAt": "2026-09-29T00:00:00Z"}),
            );
            Ok(())
        },
    )
    .expect("state-only pair call");
    assert_eq!(std::fs::read_to_string(&decisions).unwrap(), original);
    assert_eq!(
        read_value(&state)["tooling"]["lastAgentMessageAt"],
        "2026-09-29T00:00:00Z"
    );
}

#[test]
fn an_unparseable_state_file_blocks_the_pair() {
    let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), Some("{ not json"));
    let ran = std::cell::Cell::new(false);

    let err = update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |d, s| {
            ran.set(true);
            d.insert("a".to_string(), json!(2));
            s.insert("tooling".to_string(), json!({}));
            Ok(())
        },
    )
    .expect_err("an unparseable state file must block the pair");

    assert!(err.contains(&decisions.display().to_string()), "{err}");
    assert!(err.contains(&state.display().to_string()), "{err}");
    assert!(!ran.get(), "mutate must not run");
    assert_eq!(std::fs::read_to_string(&decisions).unwrap(), r#"{"a":1}"#);
    assert_eq!(std::fs::read_to_string(&state).unwrap(), "{ not json");
}

#[test]
fn the_pair_preserves_unknown_keys_on_both_sides() {
    let decisions_seed = json!({"unknownDecision": {"x": [1, 2]}, "tooling": {"telegramBot": "b"}});
    let state_seed = json!({"unknownState": "keep", "split": split_marker(), "tooling": {}});
    let (_temp, decisions, state) = pair_fixture(
        Some(&decisions_seed.to_string()),
        Some(&state_seed.to_string()),
    );

    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |d, s| {
            d.insert("context".to_string(), json!(["Role.md"]));
            s.insert("tooling".to_string(), json!({"lastCodingAgent": "claude"}));
            Ok(())
        },
    )
    .expect("pair call");

    let saved_decisions = read_value(&decisions);
    let saved_state = read_value(&state);
    assert_eq!(
        saved_decisions["unknownDecision"],
        decisions_seed["unknownDecision"]
    );
    assert_eq!(saved_decisions["tooling"], decisions_seed["tooling"]);
    assert_eq!(saved_state["unknownState"], state_seed["unknownState"]);
    assert_eq!(saved_state["split"], split_marker());
    assert_eq!(saved_decisions["context"][0], "Role.md");
    assert_eq!(saved_state["tooling"]["lastCodingAgent"], "claude");
}

#[test]
fn the_guard_is_not_released_between_the_two_sequences() {
    let (_temp, decisions, state) = pair_fixture(
        Some(r#"{"tooling":{"lastCodingAgent":"claude","telegramBot":"b"}}"#),
        None,
    );
    let (paused_tx, paused_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();

    let worker = {
        let decisions = decisions.clone();
        let state = state.clone();
        std::thread::spawn(move || {
            let on_stage = |stage: &str| {
                if stage == "after_cleanup_tracked_write" {
                    paused_tx.send(()).expect("announce pause");
                    release_rx
                        .recv_timeout(Duration::from_secs(60))
                        .expect("release the pause");
                }
            };
            let cleanup = |d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
                let moved = d
                    .get_mut("tooling")
                    .and_then(Value::as_object_mut)
                    .and_then(|tooling| tooling.remove("lastCodingAgent"))
                    .ok_or_else(|| "no key to move".to_string())?;
                s.insert("tooling".to_string(), json!({"lastCodingAgent": moved}));
                s.insert("split".to_string(), split_marker());
                Ok(())
            };
            update_config_pair(
                &decisions,
                &state,
                &PAIR_STATE_KEYS,
                Some(&cleanup),
                &on_stage,
                |_d, _s| Ok(()),
            )
        })
    };

    paused_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("the call must reach after_cleanup_tracked_write");
    let blocked = acquire_config_file_write_lock(&decisions, Duration::from_millis(300));
    let blocked_err = blocked
        .as_ref()
        .err()
        .cloned()
        .unwrap_or_else(|| "acquired".to_string());
    drop(blocked);
    release_tx.send(()).expect("release");
    worker.join().expect("worker").expect("pair call");

    assert!(
        blocked_err.contains("configLockTimeout"),
        "a second acquirer must fail while the pause holds, got: {blocked_err}"
    );
    drop(
        acquire_config_file_write_lock(&decisions, Duration::from_millis(500))
            .expect("the sidecar must be free once the call returns"),
    );
    let saved_state = read_value(&state);
    assert_eq!(saved_state["tooling"]["lastCodingAgent"], "claude");
    assert_eq!(saved_state["split"], split_marker());
    let saved_decisions = read_value(&decisions);
    assert!(saved_decisions["tooling"].get("lastCodingAgent").is_none());
    assert_eq!(saved_decisions["tooling"]["telegramBot"], "b");
}

#[test]
fn the_cleanup_sequence_is_ordered_and_aborts_whole() {
    let decisions_seed = r#"{"tooling":{"lastCodingAgent":"claude"}}"#;
    let state_seed = r#"{"tooling":{"lastAgentMessageAt":"x"}}"#;

    // Leg 1: a failing cleanup writes nothing and mutate never runs.
    let (_temp, decisions, state) = pair_fixture(Some(decisions_seed), Some(state_seed));
    let ran = std::cell::Cell::new(false);
    let failing = |_d: &mut Map<String, Value>, _s: &mut Map<String, Value>| {
        Err("policy refused".to_string())
    };
    let err = update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        Some(&failing),
        &no_stage,
        |d, s| {
            ran.set(true);
            d.insert("a".to_string(), json!(1));
            s.insert("a".to_string(), json!(1));
            Ok(())
        },
    )
    .expect_err("a failing cleanup aborts the call");
    assert!(!ran.get(), "mutate must never run after a failed cleanup");
    assert!(err.contains("policy refused"), "{err}");
    assert!(err.contains(&decisions.display().to_string()), "{err}");
    assert!(err.contains(&state.display().to_string()), "{err}");
    assert_eq!(std::fs::read_to_string(&decisions).unwrap(), decisions_seed);
    assert_eq!(std::fs::read_to_string(&state).unwrap(), state_seed);

    // Leg 2: a state-only cleanup, writing the marker, fires only its
    // state stage and leaves the decisions file byte-identical.
    let (_temp, decisions, state) = pair_fixture(Some(decisions_seed), None);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let on_stage = |stage: &str| tx.send(stage.to_string()).expect("record stage");
    let state_only = |_d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
        s.insert("split".to_string(), split_marker());
        Ok(())
    };
    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        Some(&state_only),
        &on_stage,
        |_d, _s| Ok(()),
    )
    .expect("state-only cleanup");
    assert_eq!(
        rx.recv_timeout(Duration::from_millis(200)).as_deref(),
        Ok("after_cleanup_state_publish")
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "after_cleanup_tracked_write must not fire for a state-only cleanup"
    );
    assert_eq!(std::fs::read_to_string(&decisions).unwrap(), decisions_seed);
    assert_eq!(read_value(&state)["split"], split_marker());

    // Leg 3: no cleanup behaves as a single-sequence call.
    let (_temp, decisions, state) = pair_fixture(Some(decisions_seed), None);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let on_stage = |stage: &str| tx.send(stage.to_string()).expect("record stage");
    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &on_stage,
        |d, _s| {
            d.insert("context".to_string(), json!([]));
            Ok(())
        },
    )
    .expect("single-sequence call");
    drop(tx);
    let stages: Vec<String> = rx.try_iter().collect();
    assert_eq!(stages, vec!["after_caller_decisions_write".to_string()]);
    assert!(
        !state.exists(),
        "a decisions-only mutate must not create a state file"
    );
}

fn prepared_fixture_plan() -> PreparedConfigPairPlan {
    let before = [
        PhysicalState::from_bytes(br#"{"d":0,"unknown":[1,2]}"#.to_vec()),
        PhysicalState::from_bytes(br#"{"s":0,"unknown":null}"#.to_vec()),
    ];
    let cleanup = |d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
        d.insert("d".into(), json!(1));
        s.insert("s".into(), json!(1));
        Ok(())
    };
    prepare_config_pair_plan(before, Some(&cleanup), |d, s| {
        d.insert("d".into(), json!(2));
        s.insert("s".into(), json!(2));
        Ok(())
    })
    .unwrap()
}

fn prepared_install(path: &Path, image: &PhysicalState) {
    match image {
        PhysicalState::Absent => {
            if path.exists() {
                std::fs::remove_file(path).unwrap();
            }
        }
        PhysicalState::Bytes { bytes, .. } => std::fs::write(path, bytes).unwrap(),
    }
}

#[test]
fn prepared_all_25_side_combinations_reachable_or_conflict() {
    let plan = prepared_fixture_plan();
    let d_steps = [0, 0, 1, 1, 2];
    let s_steps = [0, 1, 1, 2, 2];
    let mut reached = 0;
    let mut conflicts = 0;
    for (i, &d_step) in d_steps.iter().enumerate() {
        for (j, &s_step) in s_steps.iter().enumerate() {
            let (_temp, d, s) = pair_fixture(None, None);
            prepared_install(&d, &plan.stages[i][0]);
            prepared_install(&s, &plan.stages[j][1]);
            let before = [std::fs::read(&d).unwrap(), std::fs::read(&s).unwrap()];
            let reachable = s_step == d_step || s_step == d_step + 1;
            let result = execute_prepared_config_pair(&d, &s, &plan);
            if reachable {
                result.unwrap();
                reached += 1;
                assert_eq!(
                    [
                        read_config_pair_physical(&d).unwrap(),
                        read_config_pair_physical(&s).unwrap()
                    ],
                    plan.stages[4]
                );
                assert_eq!(read_value(&d)["unknown"], json!([1, 2]));
                assert_eq!(read_value(&s)["unknown"], Value::Null);
            } else {
                assert_eq!(result, Err(PreparedPairError::Conflict));
                conflicts += 1;
                assert_eq!(
                    [std::fs::read(&d).unwrap(), std::fs::read(&s).unwrap()],
                    before
                );
            }
        }
    }
    assert_eq!((reached, conflicts), (16, 9));
}

#[test]
fn prepared_each_publish_cut_original_plan_resume_idempotent() {
    for cut in 1..=4 {
        let plan = prepared_fixture_plan();
        let (_temp, d, s) = pair_fixture(None, None);
        prepared_install(&d, &plan.stages[0][0]);
        prepared_install(&s, &plan.stages[0][1]);
        let stages = std::cell::RefCell::new(Vec::new());
        let result = execute_prepared_config_pair_with_stage(&d, &s, &plan, &|stage| {
            stages.borrow_mut().push(stage);
            assert!(write_file_atomic(&d, b"{}")
                .unwrap_err()
                .contains("Nested config write"));
            // Both sidecars remain held across cleanup and caller images.
            for path in [&d, &s] {
                assert!(acquire_config_file_write_lock(path, Duration::ZERO)
                    .unwrap_err()
                    .contains("configLockTimeout"));
            }
            if stage == cut {
                Err(PreparedPairError::Io("lost progress".into()))
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err(PreparedPairError::Io("lost progress".into())));
        assert_eq!(*stages.borrow(), (1..=cut).collect::<Vec<_>>());
        assert_eq!(
            [
                read_config_pair_physical(&d).unwrap(),
                read_config_pair_physical(&s).unwrap()
            ],
            plan.stages[cut]
        );
        let restored: PreparedConfigPairPlan =
            serde_json::from_slice(&serde_json::to_vec(&plan).unwrap()).unwrap();
        execute_prepared_config_pair(&d, &s, &restored).unwrap();
        execute_prepared_config_pair_with_stage(&d, &s, &restored, &|_| {
            panic!("complete replay publishes nothing")
        })
        .unwrap();
        assert_eq!(
            [
                read_config_pair_physical(&d).unwrap(),
                read_config_pair_physical(&s).unwrap()
            ],
            plan.stages[4]
        );
    }
}

#[test]
fn prepared_initial_absence_collapsed_stages_and_empty_object_distinct() {
    let (_temp, d, s) = pair_fixture(None, None);
    let plan = prepare_config_pair_plan(
        [PhysicalState::Absent, PhysicalState::Absent],
        None,
        |_, _| Ok(()),
    )
    .unwrap();
    execute_prepared_config_pair_with_stage(&d, &s, &plan, &|_| panic!("no target writes"))
        .unwrap();
    assert!(!d.exists() && !s.exists());
    std::fs::write(&s, b"{}").unwrap();
    assert_eq!(
        execute_prepared_config_pair(&d, &s, &plan),
        Err(PreparedPairError::Conflict)
    );
    let create = prepare_config_pair_plan(
        [
            PhysicalState::Absent,
            PhysicalState::from_bytes(b"{}".to_vec()),
        ],
        None,
        |d, _| {
            d.insert("owned".into(), json!(true));
            Ok(())
        },
    )
    .unwrap();
    execute_prepared_config_pair(&d, &s, &create).unwrap();
    assert_eq!(std::fs::read(&s).unwrap(), b"{}");
    assert_eq!(read_value(&d)["owned"], true);
}

#[test]
fn prepared_external_bytes_disappearance_and_mid_stage_drift_no_clobber() {
    for external in [Some(b"{broken".as_slice()), Some(b"{}".as_slice()), None] {
        let plan = prepared_fixture_plan();
        let (_temp, d, s) = pair_fixture(None, None);
        prepared_install(&d, &plan.stages[0][0]);
        prepared_install(&s, &plan.stages[0][1]);
        if let Some(bytes) = external {
            std::fs::write(&s, bytes).unwrap();
        } else {
            std::fs::remove_file(&s).unwrap();
        }
        assert_eq!(
            execute_prepared_config_pair(&d, &s, &plan),
            Err(PreparedPairError::Conflict)
        );
        assert_eq!(read_config_pair_physical(&d).unwrap(), plan.stages[0][0]);
        assert_eq!(
            read_config_pair_physical(&s).unwrap(),
            external
                .map(|b| PhysicalState::from_bytes(b.to_vec()))
                .unwrap_or(PhysicalState::Absent)
        );
    }
    let plan = prepared_fixture_plan();
    let (_temp, d, s) = pair_fixture(None, None);
    prepared_install(&d, &plan.stages[0][0]);
    prepared_install(&s, &plan.stages[0][1]);
    let result = execute_prepared_config_pair_with_stage(&d, &s, &plan, &|stage| {
        assert_eq!(stage, 1);
        std::fs::write(&d, b"{\"external\":true}").unwrap();
        Ok(())
    });
    assert_eq!(result, Err(PreparedPairError::Conflict));
    assert_eq!(std::fs::read(&d).unwrap(), b"{\"external\":true}");
    assert_eq!(read_config_pair_physical(&s).unwrap(), plan.stages[1][1]);
}

#[test]
fn prepared_rejects_invalid_plan_and_nested_writer_before_target_effects() {
    let (_temp, d, s) = pair_fixture(None, None);
    let nested = prepare_config_pair_plan(
        [PhysicalState::Absent, PhysicalState::Absent],
        None,
        |_, _| write_file_atomic(&d, b"{}"),
    );
    assert!(nested
        .unwrap_err()
        .to_string()
        .contains("Nested config write"));
    assert!(!d.exists() && !s.exists());
    let rejected = prepare_config_pair_plan(
        [PhysicalState::Absent, PhysicalState::Absent],
        None,
        |d, _| {
            d.insert("owned".into(), json!(1));
            Err("rejected".into())
        },
    );
    assert!(matches!(rejected, Err(PreparedPairError::Preparation(_))));
    assert!(!d.exists());
    for malformed in [b"null".as_slice(), b"[]".as_slice(), b"{broken".as_slice()] {
        assert!(prepare_config_pair_plan(
            [
                PhysicalState::from_bytes(malformed.to_vec()),
                PhysicalState::Absent
            ],
            None,
            |_, _| Ok(())
        )
        .is_err());
    }
    let mut plan = prepared_fixture_plan();
    if let PhysicalState::Bytes { sha256, .. } = &mut plan.stages[0][0] {
        *sha256 = "forged".into();
    }
    assert!(matches!(
        execute_prepared_config_pair(&d, &s, &plan),
        Err(PreparedPairError::InvalidPlan(_))
    ));
    let mut plan = prepared_fixture_plan();
    plan.stages[1][0] = plan.stages[4][0].clone();
    assert!(matches!(
        execute_prepared_config_pair(&d, &s, &plan),
        Err(PreparedPairError::InvalidPlan(_))
    ));
    assert!(!d.exists() && !s.exists());
    assert!(!lock_sidecar_path(&d).exists());
}

#[test]
fn prepared_occupied_legacy_stage_does_not_block_and_releases_locks() {
    // Occupied legacy staging is debris, no longer an injected IO failure.
    let plan = prepared_fixture_plan();
    let (_temp, d, s) = pair_fixture(None, None);
    prepared_install(&d, &plan.stages[0][0]);
    prepared_install(&s, &plan.stages[0][1]);
    let debris = temp_config_path(&d);
    std::fs::create_dir(&debris).unwrap();
    execute_prepared_config_pair(&d, &s, &plan).unwrap();
    assert_eq!(
        [
            read_config_pair_physical(&d).unwrap(),
            read_config_pair_physical(&s).unwrap()
        ],
        plan.stages[4]
    );
    assert!(debris.is_dir());
    for path in [&d, &s] {
        drop(acquire_config_file_write_lock(path, Duration::ZERO).unwrap());
    }
}

#[test]
fn prepared_expected_absent_publication_preserves_other_creator() {
    let (_temp, d, _s) = pair_fixture(Some("{\"other\":1}"), None);
    let after = PhysicalState::from_bytes(b"{\"owned\":1}\n".to_vec());
    assert_eq!(
        publish_prepared_pair_image(&d, &PhysicalState::Absent, &after),
        Err(PreparedPairError::Conflict)
    );
    assert_eq!(std::fs::read(&d).unwrap(), b"{\"other\":1}");
    assert!(std::fs::read_dir(d.parent().unwrap())
        .unwrap()
        .all(|entry| entry
            .unwrap()
            .path()
            .extension()
            .is_none_or(|extension| extension != "tmp")));
}

#[test]
fn prepared_sidecar_timeout_and_process_death_release() {
    // Reuse the exact lock's bounded existing child-process coverage.
    issue_1937_config_lock_process_death_release();
    issue_1937_config_lock_same_process_handles();
}

const NESTED_CHILD_ACTION_ENV: &str = "AC_2786_NESTED_WRITER_CHILD_ACTION";
const NESTED_CHILD_DIR_ENV: &str = "AC_2786_NESTED_WRITER_CHILD_DIR";
const NESTED_CHILD_TEST_FQN: &str = "config::local_config_io::tests::a_nested_config_writer_child";

/// #2786 (C1) - the child half of E2c. A no-op without the child-only
/// environment, so a normal suite run passes it untouched.
#[test]
fn a_nested_config_writer_child() {
    let Some(action) = std::env::var_os(NESTED_CHILD_ACTION_ENV) else {
        return;
    };
    let action = action.to_string_lossy().into_owned();
    let dir = PathBuf::from(std::env::var_os(NESTED_CHILD_DIR_ENV).expect("child dir env"));
    let decisions = dir.join("config.json");
    let state = dir.join(PAIR_STATE_NAME);
    let other = dir.join("other.json");

    if action == "panic" {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            update_config_pair(
                &decisions,
                &state,
                &PAIR_STATE_KEYS,
                None,
                &no_stage,
                |_d, _s| panic!("forced panic inside mutate"),
            )
        }));
        assert!(
            outcome.is_err(),
            "the mutate panic must reach the test boundary"
        );
        update_config_json_object(&decisions, true, |obj| {
            obj.insert("sameThread".to_string(), json!(true));
            Ok(())
        })
        .expect("a normal write on the same thread must succeed after a panic");
        let other_thread = other.clone();
        std::thread::spawn(move || write_file_atomic(&other_thread, b"{}\n"))
            .join()
            .expect("writer thread")
            .expect("a normal write on another thread must succeed after a panic");
        println!("AC_2786_NESTED_WRITER_CHILD_OK action={action}");
        return;
    }

    let entry = match action.as_str() {
        "nest_pair" => "update_config_pair",
        "nest_json" => "update_config_json_object",
        "nest_atomic" => "write_file_atomic",
        other => panic!("unknown child action {other}"),
    };
    let err = update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |_d, _s| match entry {
            "update_config_pair" => update_config_pair(
                &other,
                &state,
                &PAIR_STATE_KEYS,
                None,
                &no_stage,
                |_d, _s| Ok(()),
            ),
            "update_config_json_object" => {
                update_config_json_object(&other, true, |_obj| Ok(())).map(|_| ())
            }
            _ => write_file_atomic(&other, b"{}\n"),
        },
    )
    .expect_err("a nested config writer must return an error");
    assert!(
        err.contains(&format!("Nested config write: {entry}")),
        "the error must name the nesting: {err}"
    );
    update_config_json_object(&decisions, true, |obj| {
        obj.insert("afterNesting".to_string(), json!(true));
        Ok(())
    })
    .expect("the thread must stay writable after the nested error");
    println!("AC_2786_NESTED_WRITER_CHILD_OK action={action}");
}

/// #2786 (C1) E2c - every leg runs in a child process with a lifetime bound,
/// because a hang cannot be asserted in-process and a poisoned `Mutex`
/// must not leak into the rest of this suite.
#[test]
fn a_nested_config_writer_is_an_error_not_a_hang() {
    // One child at a time: `wait_bounded` kills and reaps the child it
    // waits on, so a timeout never leaves another child running.
    for action in ["nest_pair", "nest_json", "nest_atomic", "panic"] {
        let temp = tempfile::tempdir().expect("tempdir");
        let child = Command::new(std::env::current_exe().expect("current test exe"))
            .args([
                "--exact",
                NESTED_CHILD_TEST_FQN,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(NESTED_CHILD_ACTION_ENV, action)
            .env(NESTED_CHILD_DIR_ENV, temp.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn nested writer child");
        let label = format!("nested writer child {action}");
        let (status, stdout, stderr) = wait_bounded(child, Duration::from_secs(30), &label);
        let report = format!(
            "status={status:?}
stdout:
{stdout}
stderr:
{stderr}"
        );
        assert!(status.success(), "{label} failed: {report}");
        assert!(
            stdout.contains("test result: ok. 1 passed; 0 failed"),
            "{label} did not run exactly one passing test: {report}"
        );
        assert!(
            stdout.contains(&format!("AC_2786_NESTED_WRITER_CHILD_OK action={action}")),
            "{label} is missing its marker: {report}"
        );
    }
}

mod pair_activity;
