use std::path::Path;
use std::time::Duration;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// The only place a `git` `Command` is constructed. Pure, like
/// the GitHub owner constructor. Gate 2 of `detect_git_status` (`GIT_CEILING_DIRECTORIES`)
/// lives here so a repo whose `.git` exists but is corrupt cannot walk up and
/// answer with an unrelated ancestor's data. Gate 1 (the async `.git` metadata
/// check) runs once per path in the caller, before any command is built.
fn git_command(path: &str, args: &[String]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(path)
        .arg("--no-optional-locks")
        .args(args)
        .kill_on_drop(true);
    crate::process::host::credentials::scrub_credentials_from_tokio_command(&mut command);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut command);

    if let Some(parent) = Path::new(path).parent() {
        if let Ok(ceiling) = std::env::join_paths(std::iter::once(parent)) {
            command.env("GIT_CEILING_DIRECTORIES", ceiling);
        }
    }

    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(command.as_std_mut(), CREATE_NO_WINDOW);
    command
}

pub(crate) async fn run_local_git(
    path: String,
    args: Vec<String>,
    timeout: Duration,
) -> Result<String, String> {
    let mut command = git_command(&path, &args);
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(Ok(output)) => Err(String::from_utf8_lossy(&output.stderr).to_string()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("timeout".to_string()),
    }
}

#[cfg(test)]
mod tests;
