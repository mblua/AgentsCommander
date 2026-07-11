use serde::Serialize;
use std::path::Path;
use tauri::State;

use crate::config::settings::SettingsState;

/// Agent detection: label + list of possible markers (files or dirs)
/// An agent is detected if ANY of its markers exist in the repo.
const AGENT_DETECTORS: &[(&str, &[&str])] = &[
    ("Claude", &[".claude", "CLAUDE.md"]),
    ("Codex", &[".codex"]),
    ("OpenCode", &[".opencode", "opencode.json"]),
    ("Cursor", &[".cursor", ".cursorrules"]),
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoMatch {
    pub name: String,
    pub path: String,
    /// Which agent tools are detected in this repo (e.g. ["Claude", "Codex"])
    pub agents: Vec<String>,
}

/// Detect which agent tools are configured in a repo directory
fn detect_agents(repo_path: &Path) -> Vec<String> {
    AGENT_DETECTORS
        .iter()
        .filter(|(_, markers)| markers.iter().any(|m| repo_path.join(m).exists()))
        .map(|(label, _)| label.to_string())
        .collect()
}

/// Derive extended repo name as "parent/repo" from an absolute path.
/// Always uses forward slash as separator regardless of OS.
pub fn derive_repo_name(path: &Path) -> Option<String> {
    let file_name = path.file_name().and_then(|n| n.to_str())?;
    let name = match path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        Some(parent) => format!("{}/{}", parent, file_name),
        None => file_name.to_string(),
    };
    Some(name)
}

/// Add a repo to results if it matches the query and hasn't been seen yet
pub fn try_add_repo(
    path: &Path,
    query_lower: &str,
    seen_paths: &mut std::collections::HashSet<String>,
    results: &mut Vec<RepoMatch>,
) {
    let file_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return,
    };

    // Skip DEPRECATED (check on repo dir name only, not extended name)
    if file_name.to_uppercase().starts_with("DEPRECATED") {
        return;
    }

    let name = match derive_repo_name(path) {
        Some(n) => n,
        None => return,
    };

    if !query_lower.is_empty() && !name.to_lowercase().contains(query_lower) {
        return;
    }

    let path_str = path.to_string_lossy().to_string();
    if seen_paths.insert(path_str.clone()) {
        let agents = detect_agents(path);
        results.push(RepoMatch {
            name,
            path: path_str,
            agents,
        });
    }
}

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
    let cfg = settings.read().await;
    let query_lower = query.to_lowercase();
    let mut results: Vec<RepoMatch> = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    for base_path in &cfg.project_paths {
        let base = Path::new(base_path);
        if !base.is_dir() {
            continue;
        }

        // The folder itself is a potential agent
        try_add_repo(base, &query_lower, &mut seen_paths, &mut results);

        // Its children are also potential agents
        let entries = match std::fs::read_dir(base) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Skip hidden directories
            if name.starts_with('.') {
                continue;
            }

            try_add_repo(&path, &query_lower, &mut seen_paths, &mut results);
        }
    }

    // Sort alphabetically
    results.sort_by_key(|a| a.name.to_lowercase());

    Ok(results)
}

/// #943 - upper bound for the `git remote get-url` call, mirroring
/// `GitWatcher::detect_branch_with_timeout` (src-tauri/src/pty/git_watcher.rs:171).
/// The context menu must never wait on a stalled git.
const GIT_REMOTE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// #943 - drop `user[:password]@` from a scheme-form remote URL.
///
/// `git remote get-url` returns whatever `.git/config` holds, and a repo cloned
/// with a token carries `https://<user>:<ghp_...>@github.com/o/r.git`. The caller
/// only needs host + owner + repo, so the secret is destroyed here and never
/// crosses the IPC boundary into the webview (nor into devtools, nor into any
/// future console log). scp-like remotes (`git@github.com:o/r.git`) are returned
/// untouched: that `git` is an ssh user name, not a credential, and the frontend
/// parser ignores it.
fn strip_userinfo(remote: &str) -> String {
    let Some(scheme_end) = remote.find("://") else {
        return remote.to_string();
    };
    let authority_start = scheme_end + 3;
    let authority_end = remote[authority_start..]
        .find('/')
        .map(|i| authority_start + i)
        .unwrap_or(remote.len());
    let authority = &remote[authority_start..authority_end];
    match authority.rfind('@') {
        Some(at) => format!(
            "{}{}{}",
            &remote[..authority_start],
            &authority[at + 1..],
            &remote[authority_end..]
        ),
        None => remote.to_string(),
    }
}

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
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

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
    if tokio::fs::metadata(Path::new(working_dir).join(".git"))
        .await
        .is_err()
    {
        return None;
    }

    let mut cmd = tokio::process::Command::new("git");
    crate::pty::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    cmd.args(["remote", "get-url", "origin"])
        .current_dir(working_dir)
        .kill_on_drop(true);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    match cmd.output().await {
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
mod tests {
    use super::*;

    /// Run git in `dir`. `false` when git is missing OR the command failed; the
    /// callers that need to tell those apart early-return instead of asserting.
    /// `.output()` (not `.status()`) keeps `git init`'s chatter out of the test log.
    fn git_in(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    const ORIGIN: &str = "https://github.com/mblua/AgentsCommander.git";

    #[test]
    fn strip_userinfo_removes_credentials_from_scheme_urls() {
        assert_eq!(
            strip_userinfo("https://u:tok@github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            strip_userinfo("ssh://git@github.com/o/r.git"),
            "ssh://github.com/o/r.git"
        );
    }

    #[test]
    fn strip_userinfo_leaves_everything_else_alone() {
        // No userinfo.
        assert_eq!(
            strip_userinfo("https://github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        // scp-like: no `://`, and the `git@` is an ssh user name, not a secret.
        assert_eq!(
            strip_userinfo("git@github.com:o/r.git"),
            "git@github.com:o/r.git"
        );
        // `@` in the path, none in the authority: must not be treated as userinfo.
        assert_eq!(
            strip_userinfo("https://github.com/o/r@2.git"),
            "https://github.com/o/r@2.git"
        );
        // Degenerate inputs must not panic.
        assert_eq!(strip_userinfo(""), "");
        assert_eq!(strip_userinfo("https://"), "https://");
    }

    /// T1 - a path that does not exist yields None, and never spawns git: the
    /// `.git` stat fails first.
    #[tokio::test]
    async fn git_remote_url_missing_path_is_none() {
        assert_eq!(
            git_remote_url_inner("C:/definitely/not/here/943").await,
            None
        );
    }

    /// T2 - real repo, real origin.
    #[tokio::test]
    async fn git_remote_url_reads_origin_from_a_temp_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        if !git_in(dir.path(), &["init", "--quiet"]) {
            return; // git unavailable: keep a git-less CI image green
        }
        assert!(git_in(dir.path(), &["remote", "add", "origin", ORIGIN]));

        let url = git_remote_url_inner(&dir.path().to_string_lossy())
            .await
            .expect("origin");
        // Containment, not equality: a dev with a `url.<x>.insteadOf` rewrite gets
        // the `git@github.com:` form back, and an equality assertion would fail
        // locally while CI stayed green.
        assert!(
            url.contains("mblua/AgentsCommander"),
            "unexpected origin: {url}"
        );
    }

    /// T3 - a work tree with no `origin` is "no browse target", not an error.
    #[tokio::test]
    async fn git_remote_url_without_origin_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        if !git_in(dir.path(), &["init", "--quiet"]) {
            return;
        }
        assert_eq!(
            git_remote_url_inner(&dir.path().to_string_lossy()).await,
            None
        );
    }

    /// T4 - the command's only error branch.
    #[tokio::test]
    async fn git_remote_url_rejects_empty_path() {
        assert!(git_remote_url(String::new()).await.is_err());
        assert!(git_remote_url("   ".to_string()).await.is_err());
    }

    /// T5' - pins the S5 fix. A plain dir nested inside a work tree must NOT
    /// inherit the ancestor's origin: without the `.git` gate git walks up, and
    /// Browse Main would confidently open an unrelated GitHub repo.
    #[tokio::test]
    async fn git_remote_url_nested_non_work_tree_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        if !git_in(dir.path(), &["init", "--quiet"]) {
            return;
        }
        assert!(git_in(dir.path(), &["remote", "add", "origin", ORIGIN]));

        let inner = dir.path().join("inner");
        std::fs::create_dir_all(&inner).expect("inner dir");

        assert_eq!(git_remote_url_inner(&inner.to_string_lossy()).await, None);
    }
}
