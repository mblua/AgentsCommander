//! Agent credential environment helpers.
//!
//! Builds the per-session `AGENTSCOMMANDER_*` environment payload for agent PTY
//! children and provides shared scrubbing helpers for child processes that must
//! not inherit parent `AGENTSCOMMANDER_*` values.
//!
//! Credentials are never formatted as visible PTY text.

use std::path::PathBuf;
use uuid::Uuid;

pub const ENV_AGENTSCOMMANDER_TOKEN: &str = "AGENTSCOMMANDER_TOKEN";
pub const ENV_AGENTSCOMMANDER_ROOT: &str = "AGENTSCOMMANDER_ROOT";
pub const ENV_AGENTSCOMMANDER_BINARY: &str = "AGENTSCOMMANDER_BINARY";
pub const ENV_AGENTSCOMMANDER_BINARY_PATH: &str = "AGENTSCOMMANDER_BINARY_PATH";
pub const ENV_AGENTSCOMMANDER_LOCAL_DIR: &str = "AGENTSCOMMANDER_LOCAL_DIR";

pub const CREDENTIAL_ENV_KEYS: [&str; 5] = [
    ENV_AGENTSCOMMANDER_TOKEN,
    ENV_AGENTSCOMMANDER_ROOT,
    ENV_AGENTSCOMMANDER_BINARY,
    ENV_AGENTSCOMMANDER_BINARY_PATH,
    ENV_AGENTSCOMMANDER_LOCAL_DIR,
];

#[derive(Clone, PartialEq, Eq)]
pub struct CredentialValues {
    pub token: String,
    pub root: String,
    pub binary: String,
    pub binary_path: String,
    pub local_dir: String,
}

fn fallback_binary_path() -> &'static str {
    if cfg!(windows) {
        "agentscommander.exe"
    } else {
        "agentscommander"
    }
}

fn lossless_path_string(path: &std::path::Path, field: &str) -> Result<String, String> {
    let raw = path.to_str().ok_or_else(|| {
        format!(
            "{field} path cannot be represented losslessly in the child environment: {:?}",
            path
        )
    })?;
    Ok(raw.strip_prefix(r"\\?\").unwrap_or(raw).to_string())
}

pub(crate) fn build_credential_values_from(
    token: &Uuid,
    cwd: &str,
    exe: Option<PathBuf>,
    authoritative_config_dir: Option<PathBuf>,
) -> Result<CredentialValues, String> {
    if exe.is_none() {
        log::warn!(
            "[credentials] current_exe() unavailable; credential env will use fallback \
             binary path/name. Agent may be unable to invoke the CLI."
        );
    }

    let binary = exe
        .as_ref()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "agentscommander".to_string());

    let binary_path = {
        let raw = exe
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| fallback_binary_path().to_string());
        raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
    };

    let local_dir_path = if let Some(config_dir) = authoritative_config_dir {
        config_dir
    } else {
        log::warn!(
            "[credentials] authoritative config directory unavailable; falling back to \
             executable-adjacent AGENTSCOMMANDER_LOCAL_DIR"
        );
        exe.as_ref()
            .and_then(|path| path.parent())
            .map(|parent| parent.join(format!(".{}", binary)))
            .unwrap_or_else(|| PathBuf::from(format!(".{}", binary)))
    };
    let local_dir = lossless_path_string(&local_dir_path, "AGENTSCOMMANDER_LOCAL_DIR")?;

    Ok(CredentialValues {
        token: token.to_string(),
        root: cwd.to_string(),
        binary,
        binary_path,
        local_dir,
    })
}

pub fn build_credential_values(token: &Uuid, cwd: &str) -> Result<CredentialValues, String> {
    build_credential_values_from(
        token,
        cwd,
        std::env::current_exe().ok(),
        crate::config::config_dir(),
    )
}

pub fn build_credentials_env(token: &Uuid, cwd: &str) -> Result<Vec<(String, String)>, String> {
    let values = build_credential_values(token, cwd)?;
    Ok(vec![
        (ENV_AGENTSCOMMANDER_TOKEN.to_string(), values.token),
        (ENV_AGENTSCOMMANDER_ROOT.to_string(), values.root),
        (ENV_AGENTSCOMMANDER_BINARY.to_string(), values.binary),
        (
            ENV_AGENTSCOMMANDER_BINARY_PATH.to_string(),
            values.binary_path,
        ),
        (ENV_AGENTSCOMMANDER_LOCAL_DIR.to_string(), values.local_dir),
    ])
}

pub fn apply_credential_env_to_pty_command(
    command: &mut portable_pty::CommandBuilder,
    extra_env: &[(String, String)],
) {
    for key in CREDENTIAL_ENV_KEYS {
        command.env_remove(key);
    }

    for (key, value) in extra_env {
        command.env(key.as_str(), value.as_str());
    }
}

pub fn scrub_credentials_from_std_command(command: &mut std::process::Command) {
    for key in CREDENTIAL_ENV_KEYS {
        command.env_remove(key);
    }
}

pub fn scrub_credentials_from_tokio_command(command: &mut tokio::process::Command) {
    scrub_credentials_from_std_command(command.as_std_mut());
}

#[cfg(test)]
mod tests {
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
    fn authoritative_config_directory_is_exported_exactly() {
        let token = Uuid::nil();
        let values = build_credential_values_from(
            &token,
            "/work",
            Some(PathBuf::from("/usr/bin/agentscommander")),
            Some(PathBuf::from("/tmp/xdg/agentscommander")),
        )
        .expect("credential values");
        assert_eq!(values.binary, "agentscommander");
        assert_eq!(values.binary_path, "/usr/bin/agentscommander");
        assert_eq!(values.local_dir, "/tmp/xdg/agentscommander");
    }

    #[test]
    fn missing_authoritative_config_retains_executable_adjacent_fallback() {
        let token = Uuid::nil();
        let values = build_credential_values_from(
            &token,
            "/work",
            Some(PathBuf::from("/opt/ac/agentscommander")),
            None,
        )
        .expect("credential values");
        assert_eq!(values.local_dir, "/opt/ac/.agentscommander");
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_authoritative_config_is_rejected_without_replacement() {
        use std::os::unix::ffi::OsStringExt;

        let token = Uuid::nil();
        let config = PathBuf::from(std::ffi::OsString::from_vec(
            b"/tmp/xdg-\xff/agentscommander".to_vec(),
        ));
        let error = match build_credential_values_from(
            &token,
            "/work",
            Some(PathBuf::from("/usr/bin/agentscommander")),
            Some(config),
        ) {
            Err(error) => error,
            Ok(_) => panic!("non-UTF-8 config path must be visible"),
        };
        assert!(error.contains("cannot be represented losslessly"));
        assert!(!error.contains('\u{fffd}'));
    }

    #[test]
    fn env_contains_expected_keys_and_values() {
        let token = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let env = build_credentials_env(&token, r"C:\example\root").unwrap();
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
        let extra_env = build_credentials_env(&token, r"C:\fresh\root").unwrap();
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
}
