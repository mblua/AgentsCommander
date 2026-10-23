use std::path::{Path, PathBuf};

/// Async git clone with CREATE_NO_WINDOW on Windows.
pub(super) async fn git_clone_async(url: &str, target: &Path) -> Result<(), String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const GIT_CLONE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
    const GIT_RESET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    let git_target = git_cli_path(target);
    let mut cmd = tokio::process::Command::new("git");
    crate::pty::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut cmd);
    cmd.args(["-c", "core.longpaths=true", "clone", "--depth", "1", url])
        .arg(git_target.as_os_str());
    cmd.kill_on_drop(true);

    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = tokio::time::timeout(GIT_CLONE_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            format!(
                "git clone timed out after {} seconds",
                GIT_CLONE_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("Failed to spawn git clone: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        // Cap error message length to avoid sending huge progress output to frontend
        let capped = if trimmed.len() > 512 {
            &trimmed[..512]
        } else {
            trimmed
        };
        return Err(format!("git clone failed: {}", capped));
    }

    if !target.join(".git").join("index").exists() {
        log::warn!(
            "[entity_creation] .git/index missing after clone for {}, running fallback git reset",
            url
        );
        let mut reset_cmd = tokio::process::Command::new("git");
        crate::pty::credentials::scrub_credentials_from_tokio_command(&mut reset_cmd);
        crate::config::agent_path::apply_search_path_to_tokio_command(&mut reset_cmd);
        reset_cmd.args(["reset"]).current_dir(&git_target);
        reset_cmd.kill_on_drop(true);
        #[cfg(windows)]
        {
            reset_cmd.creation_flags(CREATE_NO_WINDOW);
        }
        match tokio::time::timeout(GIT_RESET_TIMEOUT, reset_cmd.output()).await {
            Ok(Ok(output)) if output.status.success() => {}
            Ok(Ok(output)) => {
                log::warn!(
                    "[entity_creation] fallback git reset failed for {}: {}",
                    url,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Ok(Err(e)) => {
                log::warn!(
                    "[entity_creation] failed to spawn fallback git reset for {}: {}",
                    url,
                    e
                );
            }
            Err(_) => {
                log::warn!(
                    "[entity_creation] fallback git reset timed out after {} seconds for {}",
                    GIT_RESET_TIMEOUT.as_secs(),
                    url
                );
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
pub(super) fn git_cli_path(path: &Path) -> PathBuf {
    crate::path_utils::normalize_windows_verbatim_path_buf(path)
}

#[cfg(not(windows))]
pub(super) fn git_cli_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}
