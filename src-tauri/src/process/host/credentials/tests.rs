use super::*;

#[test]
fn std_command_removes_credentials_and_preserves_unrelated_env() {
    let mut command = std::process::Command::new("unused");
    for key in CREDENTIAL_ENV_KEYS {
        command.env(key, "credential");
    }
    command.env("UNRELATED", "preserved");
    scrub_credentials_from_std_command(&mut command);
    let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
    for key in CREDENTIAL_ENV_KEYS {
        assert_eq!(env.get(std::ffi::OsStr::new(key)), Some(&None));
    }
    assert_eq!(
        env.get(std::ffi::OsStr::new("UNRELATED")),
        Some(&Some(std::ffi::OsStr::new("preserved")))
    );
}

#[test]
fn tokio_command_removes_credentials_and_preserves_unrelated_env() {
    let mut command = tokio::process::Command::new("unused");
    for key in CREDENTIAL_ENV_KEYS {
        command.env(key, "credential");
    }
    command.env("UNRELATED", "preserved");
    scrub_credentials_from_tokio_command(&mut command);
    let env: std::collections::BTreeMap<_, _> = command.as_std().get_envs().collect();
    for key in CREDENTIAL_ENV_KEYS {
        assert_eq!(env.get(std::ffi::OsStr::new(key)), Some(&None));
    }
    assert_eq!(
        env.get(std::ffi::OsStr::new("UNRELATED")),
        Some(&Some(std::ffi::OsStr::new("preserved")))
    );
}
