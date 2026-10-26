//! Agent credential environment helpers.
//!
//! Builds the per-session `AGENTSCOMMANDER_*` environment payload for agent PTY
//! children and provides shared scrubbing helpers for child processes that must
//! not inherit parent `AGENTSCOMMANDER_*` values.
//!
//! Credentials are never formatted as visible PTY text.

use uuid::Uuid;

#[cfg(test)]
pub use crate::process::host::credentials::scrub_credentials_from_std_command;
#[cfg(test)]
pub use crate::process::host::credentials::scrub_credentials_from_tokio_command;
pub use crate::process::host::credentials::{
    CREDENTIAL_ENV_KEYS, ENV_AGENTSCOMMANDER_BINARY, ENV_AGENTSCOMMANDER_BINARY_PATH,
    ENV_AGENTSCOMMANDER_LOCAL_DIR, ENV_AGENTSCOMMANDER_ROOT, ENV_AGENTSCOMMANDER_TOKEN,
};

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

pub fn build_credential_values(token: &Uuid, cwd: &str) -> CredentialValues {
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

    let local_dir = exe
        .as_ref()
        .and_then(|p| p.parent())
        .map(|parent| {
            parent
                .join(format!(".{}", binary))
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_else(|| format!(".{}", binary));

    CredentialValues {
        token: token.to_string(),
        root: cwd.to_string(),
        binary,
        binary_path,
        local_dir: local_dir
            .strip_prefix(r"\\?\")
            .unwrap_or(&local_dir)
            .to_string(),
    }
}

pub fn build_credentials_env(token: &Uuid, cwd: &str) -> Vec<(String, String)> {
    let values = build_credential_values(token, cwd);
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

#[cfg(test)]
mod tests;
