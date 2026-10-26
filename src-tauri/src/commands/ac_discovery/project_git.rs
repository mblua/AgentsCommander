use crate::config::ac_root::existing_ac_root;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) async fn detect_repo_branch(dir: &str) -> Option<String> {
    crate::process::git::read::detect_repo_branch(dir).await
}

// --- Project file difference (#3060) ---

/// One total bound for every Git read of a single `get_project_file_difference` request.
pub(crate) const PROJECT_FILE_DIFFERENCE_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
pub(crate) use crate::process::git::read::PROJECT_GIT_REDIRECT_ENV;

/// Distinct paths of the Project's own repository that differ from the cached
/// `origin/main`, untracked non-ignored files included. `None` = unknown; never
/// a zero substituted for a failed read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileDifference {
    pub count: Option<u32>,
}

/// Shared by the Tauri command and the WebSocket `BrowserProject` route.
pub async fn get_project_file_difference_inner(path: &str) -> ProjectFileDifference {
    read_project_file_difference(
        Path::new(path),
        PROJECT_FILE_DIFFERENCE_TIMEOUT,
        run_project_git,
    )
    .await
}

#[tauri::command]
pub async fn get_project_file_difference(path: String) -> Result<ProjectFileDifference, String> {
    Ok(get_project_file_difference_inner(&path).await)
}

/// Runner and bound are parameters so tests can inject a stalled or malformed Git;
/// production always passes `run_project_git` and `PROJECT_FILE_DIFFERENCE_TIMEOUT`.
pub(crate) async fn read_project_file_difference<R, Fut>(
    project: &Path,
    limit: Duration,
    run: R,
) -> ProjectFileDifference
where
    R: Fn(PathBuf, Vec<String>) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
{
    let count =
        match tokio::time::timeout(limit, count_project_file_difference(project, &run)).await {
            Ok(Ok(count)) => Some(count),
            Ok(Err(reason)) => {
                log::debug!(
                    "[discovery] project file difference unknown for {}: {reason}",
                    project.display()
                );
                None
            }
            // Dropping the future drops the in-flight child, which `kill_on_drop` terminates.
            Err(_) => {
                log::debug!(
                    "[discovery] project file difference timed out after {limit:?}: {}",
                    project.display()
                );
                None
            }
        };
    ProjectFileDifference { count }
}

pub(crate) async fn count_project_file_difference<R, Fut>(
    project: &Path,
    run: &R,
) -> Result<u32, String>
where
    R: Fn(PathBuf, Vec<String>) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
{
    if existing_ac_root(project).is_none() {
        return Err("not an AC project root".to_string());
    }
    let project_canonical = crate::repos::canonical_git_project(project)
        .map_err(|e| format!("canonicalize project: {e}"))?;
    let git = |args: &[&str]| run(project.to_path_buf(), project_git_args(args));

    // A Project nested inside another repository must not report the parent's count.
    let toplevel = git(&["rev-parse", "--show-toplevel"]).await?;
    let toplevel = std::str::from_utf8(strip_line_terminator(&toplevel))
        .map_err(|_| "toplevel is not UTF-8".to_string())?;
    let toplevel_canonical =
        std::fs::canonicalize(toplevel).map_err(|e| format!("canonicalize toplevel: {e}"))?;
    if toplevel_canonical != project_canonical {
        return Err("project is not its repository toplevel".to_string());
    }

    // Pin the OID so both reads compare against the same commit.
    let oid = git(&["rev-parse", "--verify", "refs/remotes/origin/main^{commit}"]).await?;
    let oid = std::str::from_utf8(strip_line_terminator(&oid))
        .ok()
        .filter(|oid| !oid.is_empty() && oid.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| "origin/main did not resolve to an OID".to_string())?
        .to_string();

    let diff = git(&[
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--name-only",
        "-z",
        &oid,
        "--",
    ])
    .await?;
    let untracked = git(&["ls-files", "--others", "--exclude-standard", "-z", "--"]).await?;

    let mut paths = std::collections::HashSet::new();
    paths.extend(parse_nul_records(&diff)?);
    paths.extend(parse_nul_records(&untracked)?);
    u32::try_from(paths.len()).map_err(|_| "path count exceeds u32".to_string())
}

pub(crate) fn project_git_args(args: &[&str]) -> Vec<String> {
    ["--no-optional-locks", "-c", "core.fsmonitor=false"]
        .iter()
        .chain(args)
        .map(|arg| arg.to_string())
        .collect()
}

/// Strips one trailing LF or CRLF only; spaces in a path are preserved.
pub(crate) fn strip_line_terminator(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

/// NUL-terminated `-z` records as raw bytes. Non-empty output that is not
/// NUL-terminated, or an empty record, is malformed.
pub(crate) fn parse_nul_records(bytes: &[u8]) -> Result<Vec<&[u8]>, String> {
    let Some(body) = bytes.strip_suffix(b"\0") else {
        return if bytes.is_empty() {
            Ok(Vec::new())
        } else {
            Err("git output is not NUL-terminated".to_string())
        };
    };
    let records: Vec<&[u8]> = body.split(|b| *b == 0).collect();
    if records.iter().any(|record| record.is_empty()) {
        return Err("git output has an empty record".to_string());
    }
    Ok(records)
}

pub(crate) async fn run_project_git(cwd: PathBuf, args: Vec<String>) -> Result<Vec<u8>, String> {
    crate::process::git::read::run_project_git(cwd, args).await
}

// --- Discovery Branch Watcher ---
