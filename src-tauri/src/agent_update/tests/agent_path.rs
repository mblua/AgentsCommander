use super::*;

#[cfg(unix)]
fn agent_path_2589_executable(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Test-chosen inherited PATH (never the runner's): enough for `sh` and
/// `/usr/bin/env`, and holding no fixture binary.
#[cfg(unix)]
fn agent_path_2589_compose(extra: &[&Path]) -> std::ffi::OsString {
    crate::config::agent_path::compose(&crate::config::agent_path::SearchPathInputs {
        inherited: Some(std::ffi::OsString::from("/bin:/usr/bin")),
        home: None,
        login_shell_path: (!extra.is_empty())
            .then(|| std::env::join_paths(extra).expect("join test paths")),
    })
}

#[cfg(unix)]
#[tokio::test]
async fn agent_path_2589_update_step_finds_a_user_bin_agent_and_fails_without_it() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("user-bin");
    agent_path_2589_executable(
        &bin,
        "ac-2589-fake",
        "#!/bin/sh
exit 0
",
    );

    // `output()`, not `status()`: tokio's `status()` drops the piped
    // stdout/stderr, so `sh` dies of SIGPIPE writing "not found" (macOS)
    // instead of exiting 127.
    let with_dir = agent_path_2589_compose(&[&bin]);
    let status = build_update_step_command("ac-2589-fake", tmp.path(), &with_dir)
        .output()
        .await
        .expect("spawn update step")
        .status;
    assert_eq!(
        status.code(),
        Some(0),
        "the added dir makes the agent resolvable"
    );

    let without_dir = agent_path_2589_compose(&[]);
    let status = build_update_step_command("ac-2589-fake", tmp.path(), &without_dir)
        .output()
        .await
        .expect("spawn update step")
        .status;
    assert_eq!(
        status.code(),
        Some(127),
        "without the dir the agent is not found"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn agent_path_2589_install_probe_reports_installed_for_a_user_bin_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("agent-bin");
    let agent = agent_path_2589_executable(
        &bin,
        "ac-2589-fake-agent",
        "#!/bin/sh
echo 'ac-2589-fake-agent 1.2.3'
",
    );
    crate::agent_version::with_version_probe_args_for_test(
        "ac-2589-fake-agent",
        &["--version"],
        async {
            let state = probe_command_install_state_in(
                "ac-2589-fake-agent",
                &agent_path_2589_compose(&[&bin]),
            )
            .await;
            assert_eq!(state.status, InstallStatus::Installed, "{state:?}");
            assert_eq!(state.version.as_deref(), Some("1.2.3"));
            assert_eq!(
                state.path.as_deref(),
                Some(agent.to_string_lossy().as_ref())
            );

            let state =
                probe_command_install_state_in("ac-2589-fake-agent", &agent_path_2589_compose(&[]))
                    .await;
            assert_eq!(state.status, InstallStatus::Missing, "{state:?}");
            assert!(
                state
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("was not found on PATH")),
                "{state:?}"
            );
        },
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn agent_path_2589_install_probe_resolves_an_env_shebang_interpreter() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_bin = tmp.path().join("agent-bin");
    let interp_bin = tmp.path().join("interp-bin");
    agent_path_2589_executable(
        &agent_bin,
        "ac-2589-fake-agent",
        "#!/usr/bin/env ac-2589-fake-interp
",
    );
    agent_path_2589_executable(
        &interp_bin,
        "ac-2589-fake-interp",
        "#!/bin/sh
echo 'ac-2589-fake-agent 4.5.6'
",
    );
    crate::agent_version::with_version_probe_args_for_test(
        "ac-2589-fake-agent",
        &["--version"],
        async {
            let state = probe_command_install_state_in(
                "ac-2589-fake-agent",
                &agent_path_2589_compose(&[&agent_bin, &interp_bin]),
            )
            .await;
            assert_eq!(state.status, InstallStatus::Installed, "{state:?}");
            assert_eq!(state.version.as_deref(), Some("4.5.6"));

            let state = probe_command_install_state_in(
                "ac-2589-fake-agent",
                &agent_path_2589_compose(&[&agent_bin]),
            )
            .await;
            assert_eq!(
                state.status,
                InstallStatus::ProbeFailed,
                "the agent resolves but its interpreter does not: {state:?}"
            );
        },
    )
    .await;
}
