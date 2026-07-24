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
use std::process::Command;

mod support;

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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("make tmp dir private");
        }
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

/// Copy the bin under test into `tmp` so its `config_dir()` lands in
/// `<tmp>/.<stem>/`, isolated from every other consumer of the dev tree.
fn copy_binary_into(tmp: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let file_name = src.file_name().expect("binary has a file name");
    let dst = tmp.join(file_name);
    support::copy_executable(src, &dst);
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

    let out = Command::new(&bin)
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
        .output()
        .expect("spawn binary");

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

    let out = Command::new(&bin)
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
        .output()
        .expect("spawn binary");

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
