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
        "{label} child did not report {CHILD_TEST_FQN} running:\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("test result: ok. 1 passed; 0 failed"),
        "{label} child did not report exactly one passing test:\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
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

mod pair;
mod pair_activity;
#[cfg(windows)]
mod publish_path;
