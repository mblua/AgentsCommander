//! Git child lifetimes for repository watching and Windows executable discovery.

use std::path::Path;

pub(crate) async fn watcher_status_output(
    working_dir: &str,
) -> std::io::Result<std::process::Output> {
    status_command(working_dir).output().await
}

fn status_command(working_dir: &str) -> tokio::process::Command {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = tokio::process::Command::new("git");
    crate::process::host::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut cmd);
    cmd.args([
        "--no-optional-locks",
        "status",
        "--porcelain",
        "--branch",
        "--untracked-files=normal",
        "--ahead-behind",
    ])
    .current_dir(working_dir)
    .kill_on_drop(true);

    // Gate 2: no ancestor ascent. `join_paths` emits the platform separator, matching
    // the existing ceiling code. With no parent (a filesystem root), skip the env
    // entirely rather than set an empty value.
    if let Some(parent) = Path::new(working_dir).parent() {
        match std::env::join_paths(std::iter::once(parent)) {
            Ok(ceiling) => {
                cmd.env("GIT_CEILING_DIRECTORIES", ceiling);
            }
            Err(e) => {
                // Only reachable if the path contains the path separator itself.
                // Gate 1 still stands; log rather than abort the detection.
                log::debug!(
                    "[git] Could not build GIT_CEILING_DIRECTORIES for {}: {}",
                    working_dir,
                    e
                );
            }
        }
    }

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd
}

#[cfg(windows)]
pub(crate) fn resolve_real_git_path() -> Option<String> {
    let mut cmd = discovery_command();
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.to_string())
}

#[cfg(windows)]
fn discovery_command() -> std::process::Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut cmd = std::process::Command::new("where.exe");
    crate::process::host::credentials::scrub_credentials_from_std_command(&mut cmd);
    cmd.arg("git.exe").creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(test)]
mod tests;
