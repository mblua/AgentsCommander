use super::*;
use std::collections::BTreeMap;
use std::ffi::OsStr;

#[test]
fn command_contracts_preserve_arguments_cwd_and_per_constructor_environment() {
    let path = Path::new("repo target");
    let _search = crate::config::agent_path::SearchPathOverrideGuard::new("clone-path");
    let clone = clone_command("repository-url", path);
    let _later = crate::config::agent_path::SearchPathOverrideGuard::new("later-path");
    let reset = reset_command(path);
    let status = workgroup_status_command(path);
    let unpushed = workgroup_unpushed_command(path);
    let commands = [
        (
            clone.as_std(),
            vec![
                "-c",
                "core.longpaths=true",
                "clone",
                "--depth",
                "1",
                "repository-url",
                "repo target",
            ],
            None,
            "clone-path",
        ),
        (reset.as_std(), vec!["reset"], Some(path), "later-path"),
        (
            &status,
            vec!["status", "--porcelain"],
            Some(path),
            "later-path",
        ),
        (
            &unpushed,
            vec!["log", "@{upstream}..HEAD", "--oneline"],
            Some(path),
            "later-path",
        ),
    ];
    for (command, args, cwd, search) in commands {
        assert_eq!(command.get_program(), OsStr::new("git"));
        assert_eq!(command.get_args().collect::<Vec<_>>(), args);
        assert_eq!(command.get_current_dir(), cwd);
        let env: BTreeMap<_, _> = command.get_envs().collect();
        for key in crate::process::host::credentials::CREDENTIAL_ENV_KEYS {
            assert_eq!(env.get(OsStr::new(key)), Some(&None));
        }
        if cfg!(windows) {
            assert!(!env.contains_key(OsStr::new("PATH")));
        } else {
            assert_eq!(env.get(OsStr::new("PATH")), Some(&Some(OsStr::new(search))));
        }
    }
    assert_eq!(GIT_CLONE_TIMEOUT, Duration::from_secs(600));
    assert_eq!(GIT_RESET_TIMEOUT, Duration::from_secs(30));
}

#[test]
fn reset_spawn_error_preserves_io_error() {
    let error = std::io::Error::from_raw_os_error(2);
    let (kind, raw, display) = (error.kind(), error.raw_os_error(), error.to_string());
    let result = classify_reset_result(Ok(Err(error)), GIT_RESET_TIMEOUT);
    let Err(ResetCloneError::Spawn(error)) = result else {
        panic!("expected spawn error: {result:?}");
    };
    assert_eq!(error.kind(), kind);
    assert_eq!(error.raw_os_error(), raw);
    assert_eq!(error.to_string(), display);
}

#[tokio::test]
async fn reset_timeout_reports_30_seconds() {
    let elapsed = tokio::time::timeout(
        Duration::ZERO,
        std::future::pending::<std::io::Result<Output>>(),
    )
    .await;
    assert!(matches!(
        classify_reset_result(elapsed, GIT_RESET_TIMEOUT),
        Err(ResetCloneError::TimedOut { seconds: 30 })
    ));
}

#[test]
fn reset_nonzero_exit_returns_output() {
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(7)
    };
    #[cfg(unix)]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(7 << 8)
    };
    assert!(!status.success());
    let output = Output {
        status,
        stdout: b"reset stdout".to_vec(),
        stderr: b"reset stderr".to_vec(),
    };
    let actual = classify_reset_result(Ok(Ok(output.clone())), GIT_RESET_TIMEOUT).unwrap();
    assert_eq!(actual.status, output.status);
    assert_eq!(actual.stdout, output.stdout);
    assert_eq!(actual.stderr, output.stderr);
}

#[tokio::test]
async fn clone_timeout_and_spawn_errors_preserve_text() {
    let elapsed = tokio::time::timeout(
        Duration::ZERO,
        std::future::pending::<std::io::Result<Output>>(),
    )
    .await;
    assert_eq!(
        classify_clone_result(elapsed).unwrap_err(),
        "git clone timed out after 600 seconds"
    );
    let error = std::io::Error::from_raw_os_error(2);
    let expected = format!("Failed to spawn git clone: {error}");
    assert_eq!(classify_clone_result(Ok(Err(error))).unwrap_err(), expected);
}

#[tokio::test]
async fn missing_directory_and_non_repository_preserve_reset_and_probe_failures() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".git"), "gitdir: missing-gitdir\n").unwrap();
    let missing = dir.path().join("missing");
    assert!(matches!(
        reset_clone_index(&missing).await,
        Err(ResetCloneError::Spawn(_))
    ));
    let output = reset_clone_index(dir.path()).await.unwrap();
    assert!(!output.status.success());
    assert!(!output.stderr.is_empty());
    for probe in [workgroup_status, workgroup_unpushed] {
        assert!(probe(&missing).is_err());
        let output = probe(dir.path()).unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}
