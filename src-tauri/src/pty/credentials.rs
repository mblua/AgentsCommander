//! Agent credential environment helpers.
//!
//! Builds the per-session `AGENTSCOMMANDER_*` environment payload for agent PTY
//! children and provides shared scrubbing helpers for child processes that must
//! not inherit parent `AGENTSCOMMANDER_*` values.
//!
//! Credentials are never formatted as visible PTY text.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::path_utils::normalize_windows_verbatim_path;

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

const LEGACY_LOCAL_DIR_WARNING: &str =
    "[credentials] instance local root unresolved; exporting legacy adjacent AGENTSCOMMANDER_LOCAL_DIR";

/// #1115 - resolve the exported AGENTSCOMMANDER_LOCAL_DIR. Fallible; call before any launch residue.
pub fn resolve_local_dir(local_root: Option<&Path>) -> Result<String, String> {
    resolve_local_dir_with(
        local_root,
        || std::env::current_exe().ok(),
        |message| log::warn!("{message}"),
    )
}

fn resolve_local_dir_with(
    local_root: Option<&Path>,
    exe: impl FnOnce() -> Option<PathBuf>,
    warn: impl FnOnce(&str),
) -> Result<String, String> {
    if let Some(path) = local_root {
        let raw = path.to_str().ok_or_else(|| unicode_error(path))?;
        return Ok(normalize_windows_verbatim_path(raw));
    }

    warn(LEGACY_LOCAL_DIR_WARNING);

    let exe = exe();
    let stem = exe
        .as_ref()
        .and_then(|path| path.file_stem())
        .unwrap_or_else(|| OsStr::new("agentscommander"));
    let mut name = OsString::from(".");
    name.push(stem);
    let path = exe
        .as_ref()
        .and_then(|path| path.parent())
        .map(|parent| parent.join(&name))
        .unwrap_or_else(|| PathBuf::from(&name));
    let raw = path.to_str().ok_or_else(|| unicode_error(&path))?;
    Ok(normalize_windows_verbatim_path(raw))
}

fn unicode_error(path: &Path) -> String {
    format!("Cannot start agent session: local root {} is not valid Unicode and cannot be exported as AGENTSCOMMANDER_LOCAL_DIR. Move the AgentsCommander install/config directory to a Unicode path or set AGENTSCOMMANDER_CONFIG_DIR to one.", path.display())
}

pub fn build_credential_values(token: &Uuid, cwd: &str, local_dir: String) -> CredentialValues {
    let exe = std::env::current_exe().ok();
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

    CredentialValues {
        token: token.to_string(),
        root: cwd.to_string(),
        binary,
        binary_path,
        local_dir,
    }
}

pub fn build_credentials_env(token: &Uuid, cwd: &str, local_dir: String) -> Vec<(String, String)> {
    let values = build_credential_values(token, cwd, local_dir);
    vec![
        (ENV_AGENTSCOMMANDER_TOKEN.to_string(), values.token),
        (ENV_AGENTSCOMMANDER_ROOT.to_string(), values.root),
        (ENV_AGENTSCOMMANDER_BINARY.to_string(), values.binary),
        (
            ENV_AGENTSCOMMANDER_BINARY_PATH.to_string(),
            values.binary_path,
        ),
        (ENV_AGENTSCOMMANDER_LOCAL_DIR.to_string(), values.local_dir),
    ]
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
    fn some_home_root_exported_exactly() {
        let local_dir = resolve_local_dir_with(
            Some(Path::new("/home/u/.agentscommander")),
            || panic!("exe must not be read"),
            |_| panic!("must not warn"),
        )
        .expect("unicode root resolves");
        assert_eq!(local_dir, "/home/u/.agentscommander");
    }

    #[test]
    fn some_suffixed_adjacent_exported_exactly() {
        let local_dir = resolve_local_dir_with(
            Some(Path::new("/tools/.agentscommander_dev")),
            || panic!("exe must not be read"),
            |_| panic!("must not warn"),
        )
        .expect("unicode root resolves");
        assert_eq!(local_dir, "/tools/.agentscommander_dev");
    }

    #[test]
    fn some_absolute_and_relative_override_exported_verbatim() {
        for input in ["/srv/cfg", "rel/cfg"] {
            let local_dir = resolve_local_dir_with(
                Some(Path::new(input)),
                || panic!("exe must not be read"),
                |_| panic!("must not warn"),
            )
            .expect("unicode root resolves");
            assert_eq!(local_dir, input);
        }
    }

    #[test]
    fn some_root_never_reads_exe_or_warns() {
        for input in [
            "/home/u/.agentscommander",
            "/tools/.agentscommander_dev",
            "/srv/cfg",
            "rel/cfg",
        ] {
            let local_dir = resolve_local_dir_with(
                Some(Path::new(input)),
                || panic!("exe must not be read"),
                |_| panic!("must not warn"),
            )
            .expect("unicode root resolves");
            assert_eq!(local_dir, input);
        }
    }

    #[test]
    #[cfg(windows)]
    fn some_verbatim_drive_and_unc_normalized() {
        let drive = resolve_local_dir_with(
            Some(Path::new(r"\\?\C:\x\.a")),
            || panic!("exe must not be read"),
            |_| panic!("must not warn"),
        )
        .expect("verbatim drive path resolves");
        assert_eq!(drive, r"C:\x\.a");

        let unc = resolve_local_dir_with(
            Some(Path::new(r"\\?\UNC\srv\share\cfg")),
            || panic!("exe must not be read"),
            |_| panic!("must not warn"),
        )
        .expect("verbatim UNC path resolves");
        assert_eq!(unc, r"\\srv\share\cfg");
    }

    #[test]
    #[cfg(not(windows))]
    fn some_verbatim_like_path_kept_off_windows() {
        let local_dir = resolve_local_dir_with(
            Some(Path::new(r"\\?\C:\x")),
            || panic!("exe must not be read"),
            |_| panic!("must not warn"),
        )
        .expect("path resolves");
        assert_eq!(local_dir, r"\\?\C:\x");
    }

    #[test]
    fn none_uses_legacy_adjacent_and_warns() {
        let exe = PathBuf::from("/opt/x/agentscommander_s");
        let warnings = std::cell::RefCell::new(Vec::<String>::new());
        let local_dir = resolve_local_dir_with(
            None,
            || Some(exe.clone()),
            |message| warnings.borrow_mut().push(message.to_string()),
        )
        .expect("legacy adjacent path resolves");
        let expected = Path::new("/opt/x").join(".agentscommander_s");
        assert_eq!(local_dir, expected.to_str().unwrap());
        assert_eq!(
            *warnings.borrow(),
            vec![LEGACY_LOCAL_DIR_WARNING.to_string()]
        );

        let warnings = std::cell::RefCell::new(Vec::<String>::new());
        let local_dir = resolve_local_dir_with(
            None,
            || None,
            |message| warnings.borrow_mut().push(message.to_string()),
        )
        .expect("fallback adjacent path resolves");
        assert_eq!(local_dir, ".agentscommander");
        assert_eq!(
            *warnings.borrow(),
            vec![LEGACY_LOCAL_DIR_WARNING.to_string()]
        );
    }

    #[test]
    #[cfg(unix)]
    fn some_non_unicode_root_errors() {
        use std::os::unix::ffi::OsStrExt;

        let root = Path::new(OsStr::from_bytes(b"/tmp/\xff"));
        let error = resolve_local_dir_with(
            Some(root),
            || panic!("exe must not be read"),
            |_| panic!("must not warn"),
        )
        .expect_err("non-unicode root must be rejected");
        assert!(error.contains("AGENTSCOMMANDER_LOCAL_DIR"));
    }

    #[test]
    #[cfg(windows)]
    fn some_non_unicode_root_errors() {
        use std::os::windows::ffi::OsStringExt;

        let wide = OsString::from_wide(&[0x43, 0x3A, 0x5C, 0xD800]);
        let root = Path::new(&wide);
        let error = resolve_local_dir_with(
            Some(root),
            || panic!("exe must not be read"),
            |_| panic!("must not warn"),
        )
        .expect_err("non-unicode root must be rejected");
        assert!(error.contains("AGENTSCOMMANDER_LOCAL_DIR"));
    }

    #[test]
    #[cfg(unix)]
    fn none_non_unicode_exe_parent_errors() {
        use std::os::unix::ffi::OsStrExt;

        let exe = PathBuf::from(OsStr::from_bytes(b"/tmp/\xff/agentscommander"));
        let error = resolve_local_dir_with(None, || Some(exe.clone()), |_| {})
            .expect_err("non-unicode exe parent must be rejected");
        assert!(error.contains("AGENTSCOMMANDER_LOCAL_DIR"));
    }

    #[test]
    #[cfg(unix)]
    fn none_non_unicode_exe_stem_errors() {
        use std::os::unix::ffi::OsStrExt;

        let exe = PathBuf::from(OsStr::from_bytes(b"/opt/x/ac\xff"));
        let error = resolve_local_dir_with(None, || Some(exe.clone()), |_| {})
            .expect_err("non-unicode exe stem must be rejected");
        assert!(error.contains("AGENTSCOMMANDER_LOCAL_DIR"));
        assert!(
            error.contains("not valid Unicode"),
            "lossy conversion would have returned Ok with U+FFFD: {error}"
        );
    }

    #[test]
    fn env_contains_expected_keys_and_values() {
        let token = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let env = build_credentials_env(
            &token,
            r"C:\example\root",
            r"C:\example\.agentscommander".to_string(),
        );
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
        assert_eq!(
            map.get(ENV_AGENTSCOMMANDER_LOCAL_DIR).map(String::as_str),
            Some(r"C:\example\.agentscommander")
        );
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
        let extra_env = build_credentials_env(
            &token,
            r"C:\fresh\root",
            r"C:\fresh\.agentscommander".to_string(),
        );
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
