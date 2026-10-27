use super::*;

#[tokio::test]
async fn welcome_status_command_2736_returns_err_for_an_unavailable_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let state = state_for(AppSettings {
        project_paths: vec![temp.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    });
    let catalog_err = super::coding_agent_catalog_inner(&state)
        .await
        .expect_err("no persisted catalog");
    let welcome_err = super::coding_agent_welcome_status_inner(&state)
        .await
        .expect_err("no persisted catalog");
    assert_eq!(welcome_err, catalog_err);
}

#[tokio::test]
async fn welcome_status_command_2736_releases_the_settings_lock_before_filesystem_work() {
    let state = state_for(AppSettings::default());
    let probe = state.clone();
    let mut observed = None;
    // The loader runs AT the filesystem step; `try_write` succeeds only if
    // no settings read guard is alive at that moment.
    let rows = super::coding_agent_welcome_status_inner_with(&state, |_| {
        observed = Some(probe.try_write().is_ok());
        Ok(Vec::new())
    })
    .await;
    assert_eq!(
        observed,
        Some(true),
        "settings lock held during filesystem work"
    );
    assert_eq!(rows, Ok(Vec::new()));
}

fn install_fixture_entry_2736(
    key: &str,
    install_commands: Option<crate::config::coding_agents_catalog::InstallCommands>,
) -> crate::config::coding_agents_catalog::CodingAgentDefinition {
    crate::config::coding_agents_catalog::CodingAgentDefinition {
        key: key.to_string(),
        label: key.to_string(),
        description: String::new(),
        color: String::new(),
        command: key.to_string(),
        instructions_filename: None,
        envs: Vec::new(),
        isolated_home: false,
        config_seed: None,
        removable: true,
        update_commands: Vec::new(),
        auto_update: false,
        idle_burst: None,
        install_commands,
    }
}

fn install_mock_app_2736() -> tauri::App {
    crate::test_support::test_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app")
}

#[tokio::test]
async fn install_coding_agent_2736_rejects_an_unknown_key() {
    let app = install_mock_app_2736();
    let state = state_for(AppSettings::default());
    let result = super::install_coding_agent_inner_with(
        app.handle(),
        &state,
        "nope-2736".to_string(),
        |_| Ok(vec![install_fixture_entry_2736("claude", None)]),
    )
    .await;
    assert_eq!(result, Err("unknown coding agent 'nope-2736'".to_string()));
}

#[tokio::test]
async fn install_coding_agent_2736_rejects_a_key_with_no_install_command() {
    let app = install_mock_app_2736();
    let state = state_for(AppSettings::default());
    let result = super::install_coding_agent_inner_with(
        app.handle(),
        &state,
        "bare-2736".to_string(),
        |_| Ok(vec![install_fixture_entry_2736("bare-2736", None)]),
    )
    .await;
    assert_eq!(
        result,
        Err("no install command is available for 'bare-2736' on this platform".to_string())
    );
}

#[tokio::test]
async fn install_coding_agent_2736_reports_catalog_unavailability_verbatim() {
    let app = install_mock_app_2736();
    let temp = tempfile::tempdir().unwrap();
    let state = state_for(AppSettings {
        project_paths: vec![temp.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    });
    let catalog_err = super::coding_agent_catalog_inner(&state)
        .await
        .expect_err("no persisted catalog");
    let install_err = super::install_coding_agent_inner(app.handle(), &state, "claude".to_string())
        .await
        .expect_err("no persisted catalog");
    assert_eq!(install_err, catalog_err);
}

#[test]
fn install_coding_agent_2736_resolves_the_cwd_with_the_updater_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let with_project = AppSettings {
        project_paths: vec![temp.path().to_string_lossy().to_string()],
        ..AppSettings::default()
    };
    assert_eq!(super::install_cwd_for(&with_project), temp.path());

    let fallback = crate::config::config_dir().expect("test config dir");
    assert_eq!(super::install_cwd_for(&AppSettings::default()), fallback);
}

#[tokio::test]
async fn install_command_guard_is_shared_by_native_app_facades() {
    let root = crate::AppOwners::new();
    let independent_owner = crate::agent_update::AgentInstallState::new();
    let command = "pass3-install-dedup-fixture";
    let guard = crate::agent_update::InstallInFlight::acquire(root.installs.clone(), command)
        .expect("fixture command acquired");
    let state = state_for(AppSettings::default());
    for key in ["first-key", "second-key"] {
        let app = crate::test_support::test_builder_with_owners(root.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("same-root app facade");
        let result =
            super::install_coding_agent_inner_with(app.handle(), &state, key.into(), |_| {
                Ok(vec![install_fixture_entry_2736(
                    key,
                    Some(crate::config::coding_agents_catalog::InstallCommands {
                        default: command.into(),
                        windows: None,
                        macos: None,
                        linux: None,
                    }),
                )])
            })
            .await;
        assert_eq!(
            result,
            Err("an install is already running for that command".into())
        );
    }
    assert!(crate::agent_update::contains_install_command(
        &root.installs,
        command
    ));
    assert!(!crate::agent_update::contains_install_command(
        &independent_owner,
        command
    ));
    drop(guard);
    assert!(!crate::agent_update::contains_install_command(
        &root.installs,
        command
    ));
}
