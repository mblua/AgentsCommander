use super::*;

#[test]
fn fallback_binary_path_is_platform_specific() {
    let p = super::fallback_binary_path();
    if cfg!(windows) {
        assert_eq!(p, "agentscommander.exe");
    } else {
        assert_eq!(p, "agentscommander");
        assert!(!p.ends_with(".exe"));
    }
}

#[test]
fn env_contains_expected_keys_and_values() {
    let token = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let env = build_credentials_env(&token, r"C:\example\root");
    let map: std::collections::HashMap<_, _> = env.into_iter().collect();

    assert_eq!(map.len(), 5);
    assert_eq!(
        map.get(ENV_AGENTSCOMMANDER_TOKEN).map(String::as_str),
        Some("00000000-0000-0000-0000-000000000001")
    );
    assert_eq!(
        map.get(ENV_AGENTSCOMMANDER_ROOT).map(String::as_str),
        Some(r"C:\example\root")
    );
    assert!(map
        .get(ENV_AGENTSCOMMANDER_BINARY)
        .is_some_and(|v| !v.is_empty()));
    assert!(map
        .get(ENV_AGENTSCOMMANDER_BINARY_PATH)
        .is_some_and(|v| !v.is_empty()));
    assert!(map
        .get(ENV_AGENTSCOMMANDER_LOCAL_DIR)
        .is_some_and(|v| !v.is_empty()));
}

#[test]
fn pty_apply_helper_removes_stale_credentials_when_extra_env_empty() {
    let mut command = portable_pty::CommandBuilder::new("agent.exe");
    for key in CREDENTIAL_ENV_KEYS {
        command.env(key, "stale-parent-value");
    }

    apply_credential_env_to_pty_command(&mut command, &[]);

    for key in CREDENTIAL_ENV_KEYS {
        assert!(
            command.get_env(key).is_none(),
            "{key} should be removed from non-agent PTY children"
        );
    }
}

#[test]
fn pty_apply_helper_overrides_stale_credentials_when_extra_env_present() {
    let token = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
    let extra_env = build_credentials_env(&token, r"C:\fresh\root");
    let mut command = portable_pty::CommandBuilder::new("agent.exe");

    for key in CREDENTIAL_ENV_KEYS {
        command.env(key, "stale-parent-value");
    }

    apply_credential_env_to_pty_command(&mut command, &extra_env);

    for (key, value) in extra_env {
        assert_eq!(
            command.get_env(key.as_str()).and_then(|v| v.to_str()),
            Some(value.as_str())
        );
    }
}

#[test]
fn std_and_tokio_scrub_helpers_remove_explicit_credentials() {
    fn explicit_env_is_removed(command: &std::process::Command, key: &str) -> bool {
        command
            .get_envs()
            .any(|(env_key, value)| env_key == std::ffi::OsStr::new(key) && value.is_none())
    }

    let mut std_cmd = std::process::Command::new("git");
    for key in CREDENTIAL_ENV_KEYS {
        std_cmd.env(key, "stale-parent-value");
    }
    scrub_credentials_from_std_command(&mut std_cmd);
    for key in CREDENTIAL_ENV_KEYS {
        assert!(explicit_env_is_removed(&std_cmd, key));
    }

    let mut tokio_cmd = tokio::process::Command::new("git");
    for key in CREDENTIAL_ENV_KEYS {
        tokio_cmd.env(key, "stale-parent-value");
    }
    scrub_credentials_from_tokio_command(&mut tokio_cmd);
    for key in CREDENTIAL_ENV_KEYS {
        assert!(explicit_env_is_removed(tokio_cmd.as_std(), key));
    }
}
