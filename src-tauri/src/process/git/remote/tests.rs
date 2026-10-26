use super::*;

#[test]
fn every_child_command_is_credential_scrubbed() {
    let git_command = git_command("C:/repo-a", &["rev-parse".to_string(), "HEAD".to_string()]);

    let mut expected: Vec<&str> = crate::process::host::credentials::CREDENTIAL_ENV_KEYS.to_vec();
    expected.sort_unstable();

    let mut removed: Vec<&str> = git_command
        .as_std()
        .get_envs()
        .filter(|(_key, value)| value.is_none())
        .map(|(key, _value)| key.to_str().expect("utf-8 env key"))
        .collect();
    removed.sort_unstable();
    assert_eq!(removed, expected, "whole-vector equality");
}

#[test]
fn preserves_arguments_and_parent_ceiling() {
    let command = git_command("parent/repo-a", &["rev-parse".into(), "HEAD".into()]);
    let args: Vec<_> = command.as_std().get_args().collect();
    assert_eq!(
        args,
        [
            "-C",
            "parent/repo-a",
            "--no-optional-locks",
            "rev-parse",
            "HEAD"
        ]
    );
    let ceiling = command
        .as_std()
        .get_envs()
        .find(|(key, _)| *key == "GIT_CEILING_DIRECTORIES")
        .expect("parent ceiling");
    assert_eq!(ceiling.1, Some(std::ffi::OsStr::new("parent")));
}

#[tokio::test]
async fn returns_success_and_failure_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().to_string();
    let output = run_local_git(
        path.clone(),
        vec!["--version".into()],
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert!(output.starts_with("git version "));
    let error = run_local_git(
        path,
        vec!["invalid-command-3243".into()],
        Duration::from_secs(10),
    )
    .await
    .unwrap_err();
    assert!(error.contains("invalid-command-3243"));
}
