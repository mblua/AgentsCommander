use std::path::{Path, PathBuf};

/// Async git clone with CREATE_NO_WINDOW on Windows.
pub(super) async fn git_clone_async(url: &str, target: &Path) -> Result<(), String> {
    let git_target = git_cli_path(target);
    let output = crate::process::git::mutation::clone_repository(url, &git_target).await?;

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
        match crate::process::git::mutation::reset_clone_index(&git_target).await {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                log::warn!(
                    "[entity_creation] fallback git reset failed for {}: {}",
                    url,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Err(crate::process::git::mutation::ResetCloneError::Spawn(e)) => {
                log::warn!(
                    "[entity_creation] failed to spawn fallback git reset for {}: {}",
                    url,
                    e
                );
            }
            Err(crate::process::git::mutation::ResetCloneError::TimedOut { seconds }) => {
                log::warn!(
                    "[entity_creation] fallback git reset timed out after {} seconds for {}",
                    seconds,
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
