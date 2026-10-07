//! Plan #137 follow-up: pin runtime emission of `[task]` audit log lines
//! from the CLI path.
//!
//! Pre-fix bug: `main.rs` jumped straight into `cli::handle_cli` without
//! initializing any `log` backend, so every `log::info!("[task] ...")` call
//! in `task_set_title` / `task_append_body` was silently dropped. Plan #137
//! §3a HIGH-1 risk acceptance was conditional on those lines being grep-able
//! at `<config_dir>/app.log`.
//!
//! This test spawns the actual binary as a subprocess, exercises the happy
//! path of `task-set-title`, and asserts that a `[task] set-title:` line
//! lands in the file sink. Each invocation gets a freshly-copied binary in a
//! per-test tmp dir so `config_dir()` (which keys off `current_exe()`)
//! resolves to an isolated `<tmp>/.<stem>/` and cannot collide with sibling
//! tests, the dev build, or the user's installed standalone.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

/// Excludes one test's open write descriptor on a freshly copied binary from
/// overlapping another test's fork/exec.
///
/// These tests run in parallel and each copies the binary into its own temp dir
/// before exec'ing it. `Command::spawn` forks, and the child inherits the write
/// descriptor another thread still holds on *its* copy; exec'ing a binary that
/// any process holds open for writing fails with `ETXTBSY`. Covering both the
/// copy and the spawn closes that window. The lock is released before output is
/// collected, so the binary runs themselves still overlap.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

fn spawn_lock() -> MutexGuard<'static, ()> {
    // A test that panics elsewhere must not disable the guard for the rest.
    SPAWN_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn command_for_binary(bin: &Path) -> Command {
    let mut command = Command::new(bin);
    command.env_remove("AC_MACHINE_OUTPUT");
    let stem = bin.file_stem().expect("bin stem").to_string_lossy();
    if !stem.contains('_') {
        command.env("AGENTSCOMMANDER_CONFIG_DIR", config_dir_for_bin(bin));
    }
    command
}

struct Tmp(PathBuf);

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Tmp {
    fn new(prefix: &str) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::process::id().hash(&mut h);
        std::thread::current().id().hash(&mut h);
        let path = std::env::temp_dir().join(format!(
            "ac-{}-{}-{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            h.finish()
        ));
        std::fs::create_dir_all(&path).expect("create tmp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

/// Copy the bin under test into `tmp` so its `config_dir()` lands in
/// `<tmp>/.<stem>/`, isolated from every other consumer of the dev tree.
fn copy_binary_into(tmp: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander"));
    let file_name = src.file_name().expect("binary has a file name");
    let dst = tmp.join(file_name);
    {
        let _guard = spawn_lock();
        std::fs::copy(src, &dst).expect("copy binary under test into tmp dir");
    }
    dst
}

fn config_dir_for_bin(bin: &Path) -> PathBuf {
    let stem = bin
        .file_stem()
        .expect("bin has stem")
        .to_string_lossy()
        .to_string();
    bin.parent().expect("bin parent").join(format!(".{}", stem))
}

fn seed_master_token(config_dir: &Path, token: &str) {
    std::fs::create_dir_all(config_dir).expect("create config dir");
    std::fs::write(config_dir.join("master-token.txt"), token).expect("write master-token.txt");
}

fn backup_paths(task_path: &Path) -> Vec<PathBuf> {
    let task_dir = task_path.parent().expect("TASK.md parent");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(task_dir)
        .expect("read task dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("TASK.") && name.ends_with(".bak.md"))
        })
        .collect();
    paths.sort();
    paths
}

/// Build a workgroup fixture so `crate::phone::messaging::workgroup_root`
/// resolves under `--root`.
fn make_wg_fixture(tmp: &Path) -> PathBuf {
    let agent_root = tmp
        .join("proj")
        .join(".ac")
        .join("wg-1-test")
        .join("__agent_alice");
    std::fs::create_dir_all(&agent_root).expect("create agent root");
    agent_root
}

#[test]
fn task_set_title_audit_line_reaches_file_sink() {
    let tmp = Tmp::new("task-logger");
    let bin = copy_binary_into(tmp.path());
    let cfg_dir = config_dir_for_bin(&bin);

    // Pre-seed master-token so `validate_cli_token` returns is_root=true and
    // the coordinator gate is bypassed without needing a teams fixture.
    let master = "test-master-token-cli-logger".to_string();
    seed_master_token(&cfg_dir, &master);

    let agent_root = make_wg_fixture(tmp.path());
    let task_path = agent_root
        .parent()
        .expect("agent root has wg parent")
        .join("TASK.md");
    std::fs::write(&task_path, "# Old title\n\nOriginal body\n").expect("seed TASK.md");
    let outside_sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&outside_sentinel, "keep").expect("write outside sentinel");

    let mut command = command_for_binary(&bin);
    command
        .args([
            "task-set-title",
            "--token",
            &master,
            "--root",
            &agent_root.to_string_lossy(),
            "--title",
            "cli-logger smoke title",
        ])
        // Pin RUST_LOG so this assertion does not depend on the parent
        // (`cargo test`) shell's env. Without this, running
        // `RUST_LOG=warn cargo test --tests` filters out the `info!`
        // audit line and the `[task] set-title:` check below
        // false-fails; production behavior is unchanged.
        .env("RUST_LOG", "agentscommander=info")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn binary")
    };
    let out = child.wait_with_output().expect("collect output");

    assert!(
        out.status.success(),
        "task-set-title exited non-zero ({:?})\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let log_path = cfg_dir.join("app.log");
    let log_contents = std::fs::read_to_string(&log_path).unwrap_or_default();

    // Load-bearing assertion: HIGH-1 mitigation requires the audit line to
    // land in a persistent grep-able sink, NOT just stderr (PTY scroll loses
    // it). If this regresses, the inherited risk acceptance is built on a
    // non-functional foundation again.
    assert!(
        log_contents.contains("[task] set-title:"),
        "app.log at {} did not contain a [task] set-title line.\nstderr was:\n{}\nfile contents:\n{}",
        log_path.display(),
        String::from_utf8_lossy(&out.stderr),
        log_contents,
    );

    // Cross-check: the TASK.md write actually happened, so the log line
    // we observed is from the live happy path (not a zombie line cached on
    // disk from a prior test run. We copied to a fresh tmp dir, but be
    // defensive about future test refactors).
    let task_contents = std::fs::read_to_string(&task_path).expect("read TASK.md");
    assert!(
        task_contents.contains("title: 'cli-logger smoke title'"),
        "unexpected TASK.md contents:\n{}",
        task_contents
    );
    assert!(task_contents.contains("# Old title\n\nOriginal body"));
    assert_eq!(
        std::fs::read_to_string(&outside_sentinel).expect("read outside sentinel"),
        "keep"
    );
    assert_eq!(
        backup_paths(&task_path).len(),
        1,
        "expected one TASK.md backup"
    );
}

#[test]
fn task_append_body_audit_line_reaches_file_sink_and_preserves_title() {
    let tmp = Tmp::new("task-append-logger");
    let bin = copy_binary_into(tmp.path());
    let cfg_dir = config_dir_for_bin(&bin);
    let master = "test-master-token-cli-append-logger".to_string();
    seed_master_token(&cfg_dir, &master);

    let agent_root = make_wg_fixture(tmp.path());
    let task_path = agent_root
        .parent()
        .expect("agent root has wg parent")
        .join("TASK.md");
    std::fs::write(&task_path, "# Existing title\n\nOriginal body\n").expect("seed TASK.md");
    let outside_sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&outside_sentinel, "keep").expect("write outside sentinel");

    let mut command = command_for_binary(&bin);
    command
        .args([
            "task-append-body",
            "--token",
            &master,
            "--root",
            &agent_root.to_string_lossy(),
            "--text",
            "Appended body from subprocess",
        ])
        .env("RUST_LOG", "agentscommander=info")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn binary")
    };
    let out = child.wait_with_output().expect("collect output");

    assert!(
        out.status.success(),
        "task-append-body exited non-zero ({:?})\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let task_contents = std::fs::read_to_string(&task_path).expect("read TASK.md");
    assert!(
        task_contents.starts_with("# Existing title\n\nOriginal body"),
        "unexpected TASK.md prefix:\n{}",
        task_contents
    );
    assert!(
        task_contents.contains("Appended body from subprocess"),
        "TASK.md did not contain appended body:\n{}",
        task_contents
    );
    assert_eq!(
        std::fs::read_to_string(&outside_sentinel).expect("read outside sentinel"),
        "keep"
    );
    assert_eq!(
        backup_paths(&task_path).len(),
        1,
        "expected one TASK.md backup"
    );

    let log_path = cfg_dir.join("app.log");
    let log_contents = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log_contents.contains("[task] append-body:"),
        "app.log at {} did not contain a [task] append-body line.\nstderr was:\n{}\nfile contents:\n{}",
        log_path.display(),
        String::from_utf8_lossy(&out.stderr),
        log_contents,
    );
}

#[test]
fn task_snapshot_write_replay_emit_real_audit_without_secrets() {
    let tmp = Tmp::new("task-status-audit");
    let bin = copy_binary_into(tmp.path());
    let cfg = config_dir_for_bin(&bin);
    let token = "master-P2-audit-sensitive-token";
    seed_master_token(&cfg, token);
    let root = make_wg_fixture(tmp.path());
    let room = std::fs::canonicalize(root.parent().unwrap()).unwrap();
    let log_path = cfg.join("app.log");
    assert!(
        !log_path.exists(),
        "audit log must be newly created by real processes"
    );
    let id = uuid::Uuid::new_v4().to_string();
    let text = "P2-audit-sensitive-body 🦀\nRemaining/FUP/continuation";
    let root_arg = root.to_string_lossy();
    let mut first_receipt = None;
    for (index, args) in [
        vec!["task-get", "--token", token, "--root", &root_arg],
        vec![
            "task-status-set",
            "--token",
            token,
            "--root",
            &root_arg,
            "--expected-revision",
            "legacy:0",
            "--request-id",
            &id,
            "--text",
            text,
        ],
        vec![
            "task-status-set",
            "--token",
            token,
            "--root",
            &root_arg,
            "--expected-revision",
            "legacy:0",
            "--request-id",
            &id,
            "--text",
            text,
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let mut command = command_for_binary(&bin);
        command
            .args(args)
            .env_remove("AC_MACHINE_OUTPUT")
            .env("RUST_LOG", "agentscommander=info")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = {
            let _guard = spawn_lock();
            command.spawn().unwrap()
        };
        let pid = child.id();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(out.stderr, b"");
        assert!(out.stdout.ends_with(b"\n"));
        assert_eq!(out.stdout.iter().filter(|b| **b == b'\n').count(), 1);
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let log = std::fs::read_to_string(&log_path).unwrap();
        let verb = if index == 0 { "get" } else { "status-set" };
        let line = log
            .lines()
            .find(|l| l.contains(&format!("[task] {verb}:")) && l.contains(&format!("pid={pid}")))
            .expect("actual INFO receipt for child");
        assert!(line.contains("[INFO]"));
        assert!(line.contains("sender=proj:wg-1-test/alice"));
        assert!(line.contains(&format!("wg={}", room.display())));
        assert!(line.contains(&format!("revision={}", value["revision"].as_str().unwrap())));
        assert!(!log.contains(token) && !log.contains("P2-audit-sensitive-body"));
        if index > 0 {
            assert!(line.contains(&format!("requestId={id}")));
            assert!(line.contains(&format!("replayed={}", index == 2)));
            assert_eq!(value["status"], text);
            if index == 1 {
                first_receipt = Some(value.clone());
            } else {
                assert_eq!(
                    value["revision"],
                    first_receipt.as_ref().unwrap()["revision"]
                );
            }
            let bytes = std::fs::read(room.join("TASK-status.jsonl")).unwrap();
            assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
        }
    }
}

fn run_task_cli(bin: &Path, args: &[&str]) -> std::process::Output {
    let mut command = command_for_binary(bin);
    command
        .args(args)
        .env("RUST_LOG", "agentscommander=info")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn task CLI")
    };
    child.wait_with_output().expect("collect task CLI output")
}

fn set_body_cli(bin: &Path, token: &str, root: &Path, text: &str) -> std::process::Output {
    // Fail in the harness before an accidentally uncontained absolute root
    // can resolve a real room ancestor through the inherited CLI resolver.
    assert!(root.starts_with(std::env::temp_dir()));
    assert_eq!(root.parent().unwrap().file_name().unwrap(), "wg-1-test");
    run_task_cli(
        bin,
        &[
            "task-set-body",
            "--token",
            token,
            "--root",
            &root.to_string_lossy(),
            "--text",
            text,
        ],
    )
}

fn task_snapshot_cli(bin: &Path, token: &str, root: &Path) -> serde_json::Value {
    let out = run_task_cli(
        bin,
        &[
            "task-get",
            "--token",
            token,
            "--root",
            &root.to_string_lossy(),
        ],
    );
    assert!(
        out.status.success(),
        "task-get: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("task-get JSON")
}

#[test]
fn task_set_body_replace_clear_preserves_title_status_and_secret_free_audit() {
    let tmp = Tmp::new("task-set-body-status");
    let bin = copy_binary_into(tmp.path());
    let cfg = config_dir_for_bin(&bin);
    let token = "master-set-body-sensitive-token";
    seed_master_token(&cfg, token);
    let root = make_wg_fixture(tmp.path());
    let room = root.parent().unwrap();
    let task = room.join("TASK.md");
    let original = "\u{feff}---\r\ntitle: 'USER: Real'\r\nextra: kept\r\n---\r\nold body\r\n";
    std::fs::write(&task, original).unwrap();
    let sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&sentinel, "keep").unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let status_out = run_task_cli(
        &bin,
        &[
            "task-status-set",
            "--token",
            token,
            "--root",
            &root.to_string_lossy(),
            "--expected-revision",
            "legacy:0",
            "--request-id",
            &request,
            "--text",
            "Tickets/FUP/continuation",
        ],
    );
    assert!(
        status_out.status.success(),
        "{}",
        String::from_utf8_lossy(&status_out.stderr)
    );
    let status_bytes = std::fs::read(room.join("TASK-status.jsonl")).unwrap();
    let before = task_snapshot_cli(&bin, token, &root);
    let text = "set-body-sensitive-content 🦀\n \tline\r\nend\n\n";
    let out = set_body_cli(&bin, token, &root, text);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("body replaced; backup:"));
    assert_eq!(
        std::fs::read_to_string(&task).unwrap(),
        format!("\u{feff}---\r\ntitle: 'USER: Real'\r\nextra: kept\r\n---\r\n{text}")
    );
    let first_backup = backup_paths(&task);
    assert_eq!(first_backup.len(), 1);
    assert_eq!(
        std::fs::read(&first_backup[0]).unwrap(),
        original.as_bytes()
    );
    let after = task_snapshot_cli(&bin, token, &root);
    assert_eq!(after["description"], text);
    for key in ["taskTitle", "revision", "status", "statusRecord"] {
        assert_eq!(after[key], before[key]);
    }
    let replaced_bytes = std::fs::read(&task).unwrap();
    let out = set_body_cli(&bin, token, &root, text);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("unchanged"));
    assert_eq!(backup_paths(&task), first_backup);
    assert_eq!(std::fs::read(&task).unwrap(), replaced_bytes);
    let out = set_body_cli(&bin, token, &root, "");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("body cleared; backup:"));
    let backups = backup_paths(&task);
    assert_eq!(backups.len(), 2);
    let clear_backup = backups.iter().find(|p| !first_backup.contains(p)).unwrap();
    assert_eq!(std::fs::read(clear_backup).unwrap(), replaced_bytes);
    let clear = task_snapshot_cli(&bin, token, &root);
    assert_eq!(clear["description"], "");
    for key in ["taskTitle", "revision", "status", "statusRecord"] {
        assert_eq!(clear[key], before[key]);
    }
    let bytes = std::fs::read(&task).unwrap();
    let out = set_body_cli(&bin, token, &root, "");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("unchanged"));
    assert_eq!(std::fs::read(&task).unwrap(), bytes);
    assert_eq!(backup_paths(&task), backups);
    assert_eq!(
        std::fs::read(room.join("TASK-status.jsonl")).unwrap(),
        status_bytes
    );
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep");
    let log = std::fs::read_to_string(cfg.join("app.log")).unwrap();
    let lines: Vec<_> = log
        .lines()
        .filter(|l| l.contains("[task] set-body:"))
        .collect();
    assert_eq!(lines.len(), 4);
    for result in ["replaced", "cleared", "unchanged"] {
        assert!(lines
            .iter()
            .any(|l| l.contains(&format!("result={result}"))));
    }
    for line in lines {
        assert!(line.contains("sender=") && line.contains("wg=") && line.contains("pid="));
        assert!(!line.contains(token));
        assert!(!line.contains("set-body-sensitive-content"));
    }
    assert!(!log.contains(token) && !log.contains("set-body-sensitive-content"));
}

#[test]
fn task_set_body_legacy_delimiter_bom_roundtrip_without_forged_title() {
    let tmp = Tmp::new("task-set-body-legacy");
    let bin = copy_binary_into(tmp.path());
    let cfg = config_dir_for_bin(&bin);
    let token = "master-set-body-legacy";
    seed_master_token(&cfg, token);
    let root = make_wg_fixture(tmp.path());
    let room = root.parent().unwrap();
    let task = room.join("TASK.md");
    std::fs::write(&task, "# Legacy heading\r\nold").unwrap();
    for text in [
        "---\ntitle: 'USER: forged'\n---\nliteral",
        "\u{feff}literal",
        "\u{feff}---\ntitle: 'USER: forged'\n---\nliteral",
    ] {
        let previous = std::fs::read(&task).unwrap();
        let count = backup_paths(&task).len();
        let out = set_body_cli(&bin, token, &root, text);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let snapshot = task_snapshot_cli(&bin, token, &root);
        assert_eq!(snapshot["description"], text);
        assert!(snapshot["taskTitle"].is_null());
        assert_eq!(snapshot["revision"], "legacy:0");
        assert_eq!(
            std::fs::read_to_string(&task).unwrap(),
            format!("---\r\n---\r\n{text}")
        );
        let backups = backup_paths(&task);
        assert_eq!(backups.len(), count + 1);
        assert!(backups
            .iter()
            .any(|p| std::fs::read(p).unwrap() == previous));
        let bytes = std::fs::read(&task).unwrap();
        let out = set_body_cli(&bin, token, &root, text);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("unchanged"));
        assert_eq!(std::fs::read(&task).unwrap(), bytes);
        assert_eq!(backup_paths(&task), backups);
        let out = set_body_cli(&bin, token, &root, "");
        assert!(out.status.success());
        let clear = task_snapshot_cli(&bin, token, &root);
        assert_eq!(clear["description"], "");
        assert!(clear["taskTitle"].is_null());
        let count = backup_paths(&task).len();
        let bytes = std::fs::read(&task).unwrap();
        let out = set_body_cli(&bin, token, &root, "");
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("unchanged"));
        assert_eq!(backup_paths(&task).len(), count);
        assert_eq!(std::fs::read(&task).unwrap(), bytes);
        assert!(!room.join("TASK-status.jsonl").exists());
    }
    std::fs::remove_file(&task).unwrap();
    let count = backup_paths(&task).len();
    let out = set_body_cli(&bin, token, &root, "");
    assert!(out.status.success());
    assert!(!task.exists());
    assert_eq!(backup_paths(&task).len(), count);
    let out = set_body_cli(&bin, token, &root, "new literal");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("created"));
    assert_eq!(
        std::fs::read_to_string(&task).unwrap(),
        "---\n---\nnew literal"
    );
    assert_eq!(backup_paths(&task).len(), count);
    let log = std::fs::read_to_string(cfg.join("app.log")).unwrap();
    assert!(log.contains("result=created"));
}

#[test]
fn task_set_body_subprocess_rejections_have_no_managed_writes() {
    let tmp = Tmp::new("task-set-body-reject");
    let bin = copy_binary_into(tmp.path());
    let token = "master-set-body-reject";
    seed_master_token(&config_dir_for_bin(&bin), token);
    let root = make_wg_fixture(tmp.path());
    let room = root.parent().unwrap();
    let task = room.join("TASK.md");
    std::fs::write(&task, "keep TASK").unwrap();
    std::fs::write(room.join("TASK-status.jsonl"), "keep status").unwrap();
    let sentinel = tmp.path().join("outside-sentinel.txt");
    std::fs::write(&sentinel, "keep").unwrap();
    let invalid = set_body_cli(&bin, "not-a-token", &root, "replacement");
    assert!(!invalid.status.success());
    let unprivileged = set_body_cli(
        &bin,
        &uuid::Uuid::new_v4().to_string(),
        &root,
        "replacement",
    );
    assert!(!unprivileged.status.success());
    assert!(String::from_utf8_lossy(&unprivileged.stderr).contains("authorization denied"));
    for text in ["bad\u{7}", "bad\u{7f}"] {
        assert!(!set_body_cli(&bin, token, &root, text).status.success());
    }
    assert_eq!(std::fs::read_to_string(&task).unwrap(), "keep TASK");
    assert_eq!(
        std::fs::read_to_string(room.join("TASK-status.jsonl")).unwrap(),
        "keep status"
    );
    assert_eq!(std::fs::read_dir(room).unwrap().count(), 3);
    assert!(backup_paths(&task).is_empty());
    let outside = tmp.path().join("outside-root");
    std::fs::create_dir_all(&outside).unwrap();
    // An absolute fixture path lives under the real room when TEMP is scoped
    // to this repo. The resolver walks lexical ancestors, so use a relative
    // root and this child's cwd to exercise the no-room rejection safely.
    let mut command = command_for_binary(&bin);
    command
        .current_dir(tmp.path())
        .args([
            "task-set-body",
            "--token",
            token,
            "--root",
            "outside-root",
            "--text",
            "replacement",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _guard = spawn_lock();
        command.spawn().expect("spawn outside-root rejection")
    };
    let out = child
        .wait_with_output()
        .expect("collect outside-root rejection");
    assert!(!out.status.success());
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep");
}
