use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CHILD_CASES: [(&str, &[&str]); 8] = [
    (
        "assert_eq_model_failure",
        &["TerminalScreenModel", "rows: 1", "cells: 1", "text_bytes"],
    ),
    (
        "assert_ne_document_failure",
        &["TerminalSnapshotDocument", "schema_version: 1"],
    ),
    (
        "expect_render_error_failure",
        &["RenderedTerminalPng", "bytes:", "Invariant"],
    ),
    (
        "panic_payload_failure",
        &["Png", "declared_bytes", "decoded_bytes"],
    ),
    (
        "result_returning_protocol_error_failure",
        &["Invalid", "source=none"],
    ),
    (
        "tokio_assert_render_failure",
        &["RenderedTerminalPng", "fallback_glyph_count"],
    ),
    (
        "tokio_task_wire_failure",
        &["TerminalSnapshotApiSuccess", "result"],
    ),
    (
        "unwrap_wire_failure",
        &["TerminalSnapshotHostResponse", "has_detail: true"],
    ),
];

struct ChildCanaries {
    environment: Vec<(&'static str, String)>,
    forbidden: Vec<Vec<u8>>,
}

impl ChildCanaries {
    fn new() -> Self {
        let osc_marker = ["ACSNAP", "OSC", "1173", "Q2V7"].join("_");
        let environment = vec![
            (
                "AC_SNAPSHOT_DIAG_CELL_LEFT",
                ["ACSNAP", "CELL", "LEFT", "1173", "K7Q2"].join("_"),
            ),
            (
                "AC_SNAPSHOT_DIAG_CELL_RIGHT",
                ["ACSNAP", "CELL", "RIGHT", "1173", "M4N8"].join("_"),
            ),
            (
                "AC_SNAPSHOT_DIAG_OSC",
                format!("\u{1b}]52;c;{osc_marker}\u{7}"),
            ),
            (
                "AC_SNAPSHOT_DIAG_BASE64",
                ["QUNTTkFQX0JBU0U2NF8xMTczX1Y1TjI", "="].concat(),
            ),
            (
                "AC_SNAPSHOT_DIAG_PNG",
                ["ACSNAP", "PNG", "BYTES", "1173", "G4R8"].join("_"),
            ),
            (
                "AC_SNAPSHOT_DIAG_AUTH",
                ["ACSNAP", "AUTH", "TOKEN", "1173", "A8F6"].join("_"),
            ),
            (
                "AC_SNAPSHOT_DIAG_PATH",
                [
                    r"C:\Users\".to_string(),
                    ["ACSNAP", "PATH", "1173", "P3M9"].join("_"),
                    r"\snapshot.png".to_string(),
                ]
                .concat(),
            ),
            (
                "AC_SNAPSHOT_DIAG_WIRE",
                ["ACSNAP", "WIRE", "DETAIL", "1173", "W6D4"].join("_"),
            ),
        ];
        let mut forbidden = environment
            .iter()
            .map(|(_, value)| value.as_bytes().to_vec())
            .collect::<Vec<_>>();
        forbidden.push(osc_marker.into_bytes());
        Self {
            environment,
            forbidden,
        }
    }
}

struct ScratchDirectory {
    path: Option<PathBuf>,
}

impl ScratchDirectory {
    fn create() -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .parent()
            .and_then(Path::parent)
            .expect("renderer crate has a workspace root");
        let path = workspace_root
            .join("target")
            .join(format!(".tsd-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self { path: Some(path) })
    }

    fn path(&self) -> &Path {
        self.path.as_deref().expect("scratch directory path")
    }

    fn remove(mut self) -> io::Result<()> {
        if let Some(path) = self.path.take() {
            fs::remove_dir_all(path)?;
        }
        Ok(())
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[derive(Default)]
struct ScanStats {
    files: usize,
    bytes: u64,
}

#[test]
fn deliberately_failing_cargo_harness_diagnostics_are_payload_free() {
    let scratch = ScratchDirectory::create().expect("create diagnostic harness scratch directory");
    let target = scratch.path().join("target");
    let temporary = scratch.path().join("temp");
    let persisted = scratch.path().join("persisted");
    fs::create_dir_all(&target).expect("create isolated Cargo target directory");
    fs::create_dir_all(&temporary).expect("create isolated child temporary directory");
    fs::create_dir_all(&persisted).expect("create persisted diagnostic directory");

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("diagnostic-failure-harness")
        .join("Cargo.toml");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let canaries = ChildCanaries::new();
    let mut command = Command::new(cargo);
    command
        .args([
            "test",
            "--manifest-path",
            manifest
                .to_str()
                .expect("diagnostic fixture manifest is UTF-8"),
            "--locked",
            "--test",
            "diagnostic_failures",
            "--",
            "--test-threads=1",
        ])
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TERM_COLOR", "never")
        .env("NO_COLOR", "1")
        .env("RUST_BACKTRACE", "1")
        .env("AC_SNAPSHOT_DIAG_PERSIST_DIR", &persisted)
        .env("TMP", &temporary)
        .env("TEMP", &temporary)
        .env("TMPDIR", &temporary);
    for (name, value) in &canaries.environment {
        command.env(name, value);
    }

    let output = command
        .output()
        .expect("run deliberately failing Cargo test fixture");
    assert_canaries_absent(
        "Cargo test child stdout",
        &output.stdout,
        &canaries.forbidden,
    );
    assert_canaries_absent(
        "Cargo test child stderr",
        &output.stderr,
        &canaries.forbidden,
    );
    let scan = scan_tree(scratch.path(), &canaries.forbidden)
        .expect("raw-scan isolated child temporary and artifact files");
    assert!(
        !output.status.success(),
        "deliberately failing Cargo test fixture unexpectedly succeeded"
    );
    assert!(
        !output.stdout.is_empty(),
        "Cargo test fixture produced no harness stdout; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.stderr.is_empty(),
        "Cargo test fixture produced no harness stderr"
    );

    let mut combined = output.stdout.clone();
    combined.push(b'\n');
    combined.extend_from_slice(&output.stderr);
    let combined = String::from_utf8_lossy(&combined);
    for required in [
        "assertion `left == right` failed",
        "assertion `left != right` failed",
        "called `Result::unwrap()` on an `Err` value",
        "fixed renderer expectation: Invariant",
        "Error: Invalid",
        "tokio task snapshot result: TerminalSnapshotApiSuccess",
        "failures:",
        "test result: FAILED. 0 passed; 8 failed",
    ] {
        assert!(
            combined.contains(required),
            "Cargo test output omitted required structural diagnostic {required:?}"
        );
    }

    let persisted_files = fs::read_dir(&persisted)
        .expect("read persisted diagnostic directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .count();
    assert_eq!(persisted_files, CHILD_CASES.len());
    for (case, required) in CHILD_CASES {
        assert!(
            combined.contains(&format!("test {case} ... FAILED")),
            "failure summary omitted case {case}"
        );
        assert!(
            combined.contains(&format!("captured_diagnostic case={case}")),
            "captured log omitted case {case}"
        );
        let diagnostic = fs::read(persisted.join(format!("{case}.diagnostic")))
            .unwrap_or_else(|_| panic!("read persisted diagnostic for case {case}"));
        assert_canaries_absent(
            "persisted child failure diagnostic",
            &diagnostic,
            &canaries.forbidden,
        );
        let diagnostic = String::from_utf8_lossy(&diagnostic);
        assert!(diagnostic.contains(&format!("case={case}")));
        for fragment in required {
            assert!(
                diagnostic.contains(fragment),
                "persisted diagnostic for {case} omitted {fragment:?}"
            );
        }
    }

    let scratch_path = scratch.path().to_path_buf();
    scratch
        .remove()
        .expect("remove isolated diagnostic harness artifacts");
    assert!(!scratch_path.exists());
    eprintln!(
        "snapshot_diagnostic_harness_evidence cases={} artifact_files={} artifact_bytes={} stdout_bytes={} stderr_bytes={}",
        CHILD_CASES.len(),
        scan.files,
        scan.bytes,
        output.stdout.len(),
        output.stderr.len()
    );
}

fn assert_canaries_absent(surface: &str, bytes: &[u8], forbidden: &[Vec<u8>]) {
    for (index, canary) in forbidden.iter().enumerate() {
        assert!(
            !contains_bytes(bytes, canary),
            "forbidden terminal snapshot canary index {index} reached {surface}"
        );
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|candidate| candidate == needle)
}

fn scan_tree(root: &Path, forbidden: &[Vec<u8>]) -> io::Result<ScanStats> {
    let mut stack = vec![root.to_path_buf()];
    let mut stats = ScanStats::default();
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let rendered_path = path.to_string_lossy();
            assert_canaries_absent(
                "isolated artifact path",
                rendered_path.as_bytes(),
                forbidden,
            );
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                scan_file(&path, forbidden)?;
                let length = entry.metadata()?.len();
                stats.files = stats.files.saturating_add(1);
                stats.bytes = stats.bytes.saturating_add(length);
            } else if file_type.is_symlink() {
                let target = fs::read_link(path)?;
                assert_canaries_absent(
                    "isolated artifact symlink target",
                    target.to_string_lossy().as_bytes(),
                    forbidden,
                );
            }
        }
    }
    Ok(stats)
}

fn scan_file(path: &Path, forbidden: &[Vec<u8>]) -> io::Result<()> {
    let overlap = forbidden
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .saturating_sub(1);
    let mut file = fs::File::open(path)?;
    let mut retained = Vec::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        retained.extend_from_slice(&buffer[..count]);
        assert_canaries_absent("isolated child artifact bytes", &retained, forbidden);
        if retained.len() > overlap {
            retained.drain(..retained.len() - overlap);
        }
    }
    Ok(())
}
