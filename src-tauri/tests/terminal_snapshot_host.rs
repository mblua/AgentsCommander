use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ac-terminal-snapshot-host-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copied_binary(directory: &Path) -> PathBuf {
    let source = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let destination = directory.join(source.file_name().unwrap());
    std::fs::copy(source, &destination).unwrap();
    destination
}

fn run(binary: &Path, arguments: &[&str]) -> Output {
    Command::new(binary).args(arguments).output().unwrap()
}

fn assert_fixed_failure(output: &Output, code: &str) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    assert_eq!(
        stderr,
        format!(
            "terminal_snapshot_error code={code} detail={}\n",
            match code {
                "invalid_request" => "The terminal snapshot request is invalid.",
                "unsafe_path" => "A terminal snapshot path failed confinement checks.",
                _ => panic!("unexpected test code"),
            }
        )
    );
}

#[test]
fn terminal_snapshot_help_is_machine_clean_and_names_target_discovery() {
    let temporary = TempDirectory::new();
    let binary = copied_binary(&temporary.0);
    let output = run(&binary, &["terminal-snapshot", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "--token <TOKEN>",
        "--root <ROOT>",
        "--to <TO>",
        "--format <FORMAT>",
        "--output <OUTPUT>",
        "--timeout <TIMEOUT>",
        "list-peers-lean",
        "--snapshot-targets",
    ] {
        assert!(stdout.contains(expected), "missing {expected} in {stdout}");
    }
}

#[test]
fn terminal_snapshot_semantic_failures_are_fixed_and_secret_free() {
    let temporary = TempDirectory::new();
    let binary = copied_binary(&temporary.0);
    let secret = "sentinel-secret-token-value";
    let root = temporary.0.join("sentinel-secret-root");
    let root_text = root.to_string_lossy();
    let output = run(
        &binary,
        &[
            "terminal-snapshot",
            "--token",
            secret,
            "--root",
            &root_text,
            "--to",
            "project:wg-1-team/member",
            "--timeout",
            "4",
        ],
    );
    assert_fixed_failure(&output, "invalid_request");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains(secret));
    assert!(!stderr.contains(root_text.as_ref()));

    let valid_token = "00000000-0000-4000-8000-000000000117";
    let output = run(
        &binary,
        &[
            "terminal-snapshot",
            "--token",
            valid_token,
            "--root",
            &root_text,
            "--to",
            "project:wg-1-team/member",
            "--format",
            "png",
        ],
    );
    assert_fixed_failure(&output, "invalid_request");

    let output = run(
        &binary,
        &[
            "terminal-snapshot",
            "--token",
            valid_token,
            "--root",
            &root_text,
            "--to",
            "project:wg-1-team/member",
            "--format",
            "png",
            "--output",
            "relative.png",
        ],
    );
    assert_fixed_failure(&output, "unsafe_path");
}

#[test]
fn terminal_snapshot_rejects_a_persisted_static_token_before_publication() {
    let temporary = TempDirectory::new();
    let binary = copied_binary(&temporary.0);
    let stem = binary.file_stem().unwrap().to_string_lossy();
    let config = temporary.0.join(format!(".{stem}"));
    std::fs::create_dir_all(&config).unwrap();
    let token = "00000000-0000-4000-8000-000000000118";
    std::fs::write(
        config.join("settings.json"),
        format!("{{\"rootToken\":\"{token}\"}}"),
    )
    .unwrap();
    let root_text = temporary.0.to_string_lossy();
    let output = run(
        &binary,
        &[
            "terminal-snapshot",
            "--token",
            token,
            "--root",
            &root_text,
            "--to",
            "project:wg-1-team/member",
        ],
    );
    assert_fixed_failure(&output, "invalid_request");
    assert!(!String::from_utf8(output.stderr).unwrap().contains(token));
}
