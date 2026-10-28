use std::path::Path;
use tauri::State;

use crate::config::settings::SettingsState;
// Preserve existing command-module import paths after moving the implementation.
#[allow(unused_imports)]
pub(crate) use crate::repos::derive_repo_name;
use crate::repos::strip_userinfo;

#[allow(unused_imports)]
pub use crate::repos::{try_add_repo, RepoMatch};

/// Scan configured paths for potential agents.
/// Each configured path is treated as both:
/// - A potential agent folder itself, AND
/// - A parent folder whose children are also potential agents
///
/// For each folder found, detects agent tooling (.claude, .codex, .cursor).
#[tauri::command]
pub async fn search_repos(
    settings: State<'_, SettingsState>,
    query: String,
) -> Result<Vec<RepoMatch>, String> {
    let cfg = settings
        .current_read()
        .await
        .map_err(|error| error.to_string())?;
    let results = crate::repos::search_repositories(&cfg.project_paths, &query);
    Ok(results)
}

/// #943 - upper bound for the `git remote get-url` call, mirroring
/// the git sweeper's `detect_status_with_timeout` (src-tauri/src/pty/git_watcher.rs).
/// The context menu must never wait on a stalled git.
const GIT_REMOTE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// #943 - `origin`'s URL for a repo working dir, userinfo stripped, or `None`.
///
/// Every failure mode (path missing, not a work tree root, no `origin`, git not on
/// PATH, timeout) collapses to `None` on purpose: the caller's only decision is
/// "show Browse items or not". Bounded by GIT_REMOTE_TIMEOUT with
/// `.kill_on_drop(true)` so a hung `git.exe` is reaped instead of leaked.
pub async fn git_remote_url_inner(path: &str) -> Option<String> {
    match tokio::time::timeout(GIT_REMOTE_TIMEOUT, run_git_remote_get_url(path)).await {
        Ok(result) => result,
        Err(_) => {
            log::warn!(
                "[repos] git_remote_url timed out for {} (>{}s)",
                path,
                GIT_REMOTE_TIMEOUT.as_secs()
            );
            None
        }
    }
}

async fn run_git_remote_get_url(working_dir: &str) -> Option<String> {
    // #943 - require a work tree ROOT, do not let git discover a repo by walking
    // up. `git remote get-url` climbs to the nearest ancestor `.git`, so a
    // sourcePath that exists but is not a repo (failed clone, deleted `.git`, a
    // plain folder the user configured) would answer with an UNRELATED
    // repository's origin, and Browse Main would confidently open the wrong page
    // on github.com. `.git` is checked with `metadata`, not `is_dir`: linked
    // worktrees and submodules use a `.git` FILE.
    //
    // This also makes GIT_CEILING_DIRECTORIES unnecessary. Do not add it back:
    // `git_ceiling_directories_for_session_root` returns None for any repo path
    // (`is_agent_dir` gate, config/session_context.rs:1195-1196), i.e. it is a
    // no-op that reads like a guard.
    //
    // `tokio::fs::metadata` (not `Path::is_dir`) because a sync stat inside an
    // async fn cannot be cancelled by `tokio::time::timeout`: on a dead network
    // share it would wedge a runtime worker for the SMB timeout and make the 2s
    // cap a lie.
    if !crate::repos::git_root_present(Path::new(working_dir)).await {
        return None;
    }

    match crate::process::git::read::remote_origin_output(working_dir).await {
        Ok(out) if out.status.success() => {
            let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if url.is_empty() {
                None
            } else {
                Some(strip_userinfo(&url))
            }
        }
        // Non-zero exit: no `origin`, or git refusing the repo entirely
        // ("detected dubious ownership", git >= 2.35.2, common on Windows after a
        // drive move or an elevated clone). Without this arm the symptom is
        // "Browse is silently absent forever, with nothing in app.log". Debug
        // level and menu-driven, so it cannot flood the log the way the 5s
        // GitWatcher poll would. stderr never contains the URL, so no leak.
        Ok(out) => {
            log::debug!(
                "[repos] git remote get-url origin failed in {} (exit {:?}): {}",
                working_dir,
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
            None
        }
        Err(e) => {
            log::debug!("[repos] failed to spawn git in {}: {}", working_dir, e);
            None
        }
    }
}

/// #943 - read-only. `Ok(None)` means "no Browse items"; see `git_remote_url_inner`.
#[tauri::command]
pub async fn git_remote_url(path: String) -> Result<Option<String>, String> {
    if path.trim().is_empty() {
        return Err("git_remote_url: empty path".to_string());
    }
    Ok(git_remote_url_inner(&path).await)
}

#[cfg(test)]
mod tests;
