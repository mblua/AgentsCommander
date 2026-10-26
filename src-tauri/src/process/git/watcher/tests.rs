use super::*;
use std::collections::BTreeMap;
use std::ffi::OsStr;

#[test]
fn status_preserves_arguments_cwd_credentials_search_and_ceiling() {
    let _path = crate::config::agent_path::SearchPathOverrideGuard::new("watcher-test-path");
    let command = status_command("parent/repo");
    let command = command.as_std();
    assert_eq!(command.get_program(), OsStr::new("git"));
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        [
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--branch",
            "--untracked-files=normal",
            "--ahead-behind",
        ]
    );
    assert_eq!(command.get_current_dir(), Some(Path::new("parent/repo")));
    let env: BTreeMap<_, _> = command.get_envs().collect();
    for key in crate::process::host::credentials::CREDENTIAL_ENV_KEYS {
        assert_eq!(env.get(OsStr::new(key)), Some(&None));
    }
    let ceiling = std::env::join_paths([Path::new("parent")]).unwrap();
    assert_eq!(
        env.get(OsStr::new("GIT_CEILING_DIRECTORIES")),
        Some(&Some(ceiling.as_os_str()))
    );
    if cfg!(windows) {
        assert!(!env.contains_key(OsStr::new("PATH")));
    } else {
        assert_eq!(
            env.get(OsStr::new("PATH")),
            Some(&Some(OsStr::new("watcher-test-path")))
        );
    }
}

#[test]
fn filesystem_root_skips_ceiling_and_unjoinable_parent_keeps_command() {
    #[cfg(windows)]
    let (root, invalid) = (r"C:\", r#"parent"invalid/repo"#);
    #[cfg(not(windows))]
    let (root, invalid) = ("/", "parent:invalid/repo");
    for dir in [root, invalid] {
        let command = status_command(dir);
        assert!(!command
            .as_std()
            .get_envs()
            .any(|(key, _)| key == "GIT_CEILING_DIRECTORIES"));
    }
}

#[cfg(windows)]
#[test]
fn discovery_scrubs_credentials_without_composed_search_or_cwd() {
    let _path = crate::config::agent_path::SearchPathOverrideGuard::new("unused-path");
    let command = discovery_command();
    assert_eq!(command.get_program(), OsStr::new("where.exe"));
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["git.exe"]);
    assert_eq!(command.get_current_dir(), None);
    let env: BTreeMap<_, _> = command.get_envs().collect();
    assert_eq!(
        env.len(),
        crate::process::host::credentials::CREDENTIAL_ENV_KEYS.len()
    );
    for key in crate::process::host::credentials::CREDENTIAL_ENV_KEYS {
        assert_eq!(env.get(OsStr::new(key)), Some(&None));
    }
    let output = discovery_command().output().unwrap();
    let expected = output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
    });
    assert_eq!(resolve_real_git_path(), expected.flatten());
}

#[tokio::test]
async fn status_returns_complete_output_nonzero_status_and_spawn_errors() {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new("git")
        .args(["init", "--initial-branch=watcher-owner"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    std::fs::write(dir.path().join("untracked.txt"), "content").unwrap();
    let path = dir.path().to_str().unwrap();
    let output = watcher_status_output(path).await.unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("watcher-owner"), "{stdout}");
    assert!(stdout.contains("?? untracked.txt"), "{stdout}");
    let empty = tempfile::tempdir().unwrap();
    std::fs::write(empty.path().join(".git"), "gitdir: missing\n").unwrap();
    let output = watcher_status_output(empty.path().to_str().unwrap())
        .await
        .unwrap();
    assert!(!output.status.success());
    assert!(!output.stderr.is_empty());
    assert!(
        watcher_status_output(dir.path().join("missing").to_str().unwrap())
            .await
            .is_err()
    );
}
