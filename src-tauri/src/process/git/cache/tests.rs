use super::*;

fn shell(script: &str, timeout: Duration) -> Result<Output, String> {
    #[cfg(windows)]
    let (program, args) = ("powershell.exe", vec!["-NoProfile", "-Command", script]);
    #[cfg(not(windows))]
    let (program, args) = ("sh", vec!["-c", script]);
    run_process_with_timeout(program, None, &args, timeout)
}

#[test]
fn success_joins_both_readers_and_keeps_complete_output() {
    #[cfg(windows)]
    let script = "[Console]::Out.Write('o' * 131072); [Console]::Error.Write('e' * 131072)";
    #[cfg(not(windows))]
    let script = "head -c 131072 /dev/zero | tr '\0' o; head -c 131072 /dev/zero | tr '\0' e >&2";
    let output = shell(script, Duration::from_secs(30)).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, vec![b'o'; 131072]);
    assert_eq!(output.stderr, vec![b'e'; 131072]);
}

#[test]
fn nonzero_exit_is_output_rather_than_spawn_or_timeout_error() {
    #[cfg(windows)]
    let script = "[Console]::Out.Write('out'); [Console]::Error.Write('err'); exit 7";
    #[cfg(not(windows))]
    let script = "printf out; printf err >&2; exit 7";
    let output = shell(script, Duration::from_secs(30)).unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"out");
    assert_eq!(output.stderr, b"err");
}

#[test]
fn spawn_failure_and_timeout_have_distinct_errors() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-program");
    let error =
        run_process_with_timeout(missing.to_str().unwrap(), None, &[], Duration::from_secs(1))
            .unwrap_err();
    assert!(error.starts_with("spawn failed for "), "{error}");
    #[cfg(windows)]
    let script = "Start-Sleep -Seconds 5";
    #[cfg(not(windows))]
    let script = "exec sleep 5";
    let error = shell(script, Duration::from_millis(100)).unwrap_err();
    assert!(error.contains("timed out"), "{error}");
    assert!(error.contains("killed and reaped"), "{error}");
    assert!(error.contains("stdout_bytes="), "{error}");
}

#[test]
fn reader_join_reports_read_failure_and_panic() {
    let reader = std::thread::spawn(|| (Err(std::io::Error::other("reader failed")), vec![b'x']));
    assert_eq!(
        join_reader(reader, "stdout").unwrap_err(),
        "stdout read failed: reader failed"
    );
    let reader = std::thread::spawn(|| panic!("reader panic"));
    assert_eq!(
        join_reader(reader, "stderr").unwrap_err(),
        "stderr reader thread panicked"
    );
}

#[test]
fn agency_git_returns_nonzero_status_in_isolated_non_repository() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".git"), "gitdir: missing-gitdir\n").unwrap();
    let output = run_agency_git(
        Some(dir.path()),
        &["status", "--porcelain"],
        Duration::from_secs(30),
    )
    .unwrap();
    assert!(!output.status.success());
    assert!(!output.stderr.is_empty());
}
