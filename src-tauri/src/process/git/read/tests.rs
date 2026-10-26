use super::*;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;

#[test]
fn command_contracts_preserve_arguments_cwd_and_environment_policy() {
    let _path = crate::config::agent_path::SearchPathOverrideGuard::new("owner-test-path");
    let args: Vec<String> = [
        "--no-optional-locks",
        "-c",
        "core.fsmonitor=false",
        "status",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let commands = [
        (
            branch_command("repo"),
            vec!["rev-parse", "--abbrev-ref", "HEAD"],
            false,
        ),
        (
            project_command(Path::new("repo"), &args),
            args.iter().map(String::as_str).collect(),
            true,
        ),
        (
            remote_origin_command("repo"),
            vec!["remote", "get-url", "origin"],
            false,
        ),
    ];
    for (command, expected, redirects_removed) in commands {
        let command = command.as_std();
        assert_eq!(command.get_program(), OsStr::new("git"));
        assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
        assert_eq!(command.get_current_dir(), Some(Path::new("repo")));
        let env: BTreeMap<_, _> = command.get_envs().collect();
        for key in crate::process::host::credentials::CREDENTIAL_ENV_KEYS {
            assert_eq!(env.get(OsStr::new(key)), Some(&None));
        }
        for key in PROJECT_GIT_REDIRECT_ENV {
            assert_eq!(env.get(OsStr::new(key)), redirects_removed.then_some(&None));
        }
        if cfg!(windows) {
            assert!(!env.contains_key(OsStr::new("PATH")));
        } else {
            assert_eq!(
                env.get(OsStr::new("PATH")),
                Some(&Some(OsStr::new("owner-test-path")))
            );
        }
    }
    assert_eq!(BRANCH_DETECT_TIMEOUT, Duration::from_secs(2));
}

#[tokio::test]
async fn missing_directory_preserves_operation_failure_results() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    let path = missing.to_str().unwrap();
    assert_eq!(detect_repo_branch(path).await, None);
    assert!(remote_origin_output(path).await.is_err());
    let args = [
        "--no-optional-locks",
        "-c",
        "core.fsmonitor=false",
        "status",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let error = run_project_git(missing, args).await.unwrap_err();
    assert!(error.starts_with("spawn git status: "), "{error}");
}

#[tokio::test]
async fn real_repository_preserves_branch_remote_and_project_output() {
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "--initial-branch=owner-test"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(status.status.success());
    let path = dir.path().to_str().unwrap();
    assert_eq!(detect_repo_branch(path).await, None);
    let remote = remote_origin_output(path).await.unwrap();
    assert!(!remote.status.success());
    let args = [
        "--no-optional-locks",
        "-c",
        "core.fsmonitor=false",
        "symbolic-ref",
        "--short",
        "HEAD",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let output = run_project_git(dir.path().to_path_buf(), args)
        .await
        .unwrap();
    assert_eq!(String::from_utf8(output).unwrap().trim(), "owner-test");
    let status = std::process::Command::new("git")
        .current_dir(dir.path())
        .args([
            "remote",
            "add",
            "origin",
            "https://example.invalid/owner.git",
        ])
        .output()
        .unwrap();
    assert!(status.status.success());
    let remote = remote_origin_output(path).await.unwrap();
    assert!(remote.status.success());
    assert_eq!(
        String::from_utf8(remote.stdout).unwrap().trim(),
        "https://example.invalid/owner.git"
    );
}
