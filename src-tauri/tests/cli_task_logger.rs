//! Plan #137 follow-up — pin runtime emission of `[task]` audit log lines
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
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let file_name = src.file_name().expect("binary has a file name");
    let dst = tmp.join(file_name);
    std::fs::copy(src, &dst).expect("copy binary under test into tmp dir");
    dst
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

    let stem = bin
        .file_stem()
        .expect("bin has stem")
        .to_string_lossy()
        .to_string();
    let cfg_dir = tmp.path().join(format!(".{}", stem));
    std::fs::create_dir_all(&cfg_dir).expect("create config dir");

    // Pre-seed master-token so `validate_cli_token` returns is_root=true and
    // the coordinator gate is bypassed without needing a teams fixture.
    let master = "test-master-token-cli-logger".to_string();
    std::fs::write(cfg_dir.join("master-token.txt"), &master).expect("write master-token.txt");

    let agent_root = make_wg_fixture(tmp.path());

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
        // false-fails — production behavior is unchanged.
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
    // disk from a prior test run — we copied to a fresh tmp dir, but be
    // defensive about future test refactors).
    let task_path = agent_root
        .parent()
        .expect("agent root has wg parent")
        .join("TASK.md");
    assert!(
        task_path.exists(),
        "TASK.md was not created at {} — log line may be from an unrelated path",
        task_path.display(),
    );
}
