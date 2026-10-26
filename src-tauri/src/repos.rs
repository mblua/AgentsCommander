use serde::Serialize;
use std::path::{Path, PathBuf};

/// Derive extended repo name as "parent/repo" from an absolute path.
/// Always uses forward slash as separator regardless of OS.
pub(crate) fn derive_repo_name(path: &Path) -> Option<String> {
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

/// #943 - drop `user[:password]@` from a scheme-form remote URL.
///
/// `git remote get-url` returns whatever `.git/config` holds, and a repo cloned
/// with a token carries `https://<user>:<ghp_...>@github.com/o/r.git`. The caller
/// only needs host + owner + repo, so the secret is destroyed here and never
/// crosses the IPC boundary into the webview (nor into devtools, nor into any
/// future console log). scp-like remotes (`git@github.com:o/r.git`) are returned
/// untouched: that `git` is an ssh user name, not a credential, and the frontend
/// parser ignores it.
pub(crate) fn strip_userinfo(remote: &str) -> String {
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
        .filter(|(_, markers)| {
            markers
                .iter()
                .any(|m| crate::filesystem::repositories::agent_marker_exists(&repo_path.join(m)))
        })
        .map(|(label, _)| label.to_string())
        .collect()
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

pub(crate) fn search_repositories(project_paths: &[String], query: &str) -> Vec<RepoMatch> {
    let query_lower = query.to_lowercase();
    let mut results: Vec<RepoMatch> = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    for base_path in project_paths {
        let base = Path::new(base_path);
        if !crate::filesystem::repositories::repo_search_is_dir(base) {
            continue;
        }

        // The folder itself is a potential agent
        try_add_repo(base, &query_lower, &mut seen_paths, &mut results);

        // Its children are also potential agents
        let _ = crate::filesystem::repositories::visit_repo_search_children(base, |path| {
            if !crate::filesystem::repositories::repo_search_is_dir(&path) {
                return;
            }

            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => return,
            };

            // Skip hidden directories
            if name.starts_with('.') {
                return;
            }

            try_add_repo(&path, &query_lower, &mut seen_paths, &mut results);
        });
    }

    // Sort alphabetically
    results.sort_by_key(|a| a.name.to_lowercase());

    results
}

pub(crate) async fn git_root_present(path: &Path) -> bool {
    crate::filesystem::repositories::git_root_metadata(&path.join(".git"))
        .await
        .is_ok()
}

pub(crate) async fn valid_spec_repo_roots(
    project_path: Option<&str>,
    project_paths: &[String],
) -> Result<Vec<PathBuf>, String> {
    let mut candidates = Vec::new();
    if let Some(path) = project_path {
        candidates.push(PathBuf::from(path));
    }
    candidates.extend(project_paths.iter().map(PathBuf::from));

    let mut roots = Vec::new();
    for candidate in candidates {
        let Ok(canonical) = canonicalize_spec_repo_directory(&candidate).await else {
            continue;
        };
        if is_valid_spec_repo_root(&canonical).await.unwrap_or(false) {
            push_unique_spec_path(&mut roots, canonical.clone());
        }
        if let Ok(mut entries) =
            crate::filesystem::repositories::open_spec_repo_children(&canonical).await
        {
            while let Some(path) = entries.next_path().await {
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if name.starts_with('.') || !name.starts_with("repo-") {
                    continue;
                }
                let Ok(child) = canonicalize_spec_repo_directory(&path).await else {
                    continue;
                };
                if is_valid_spec_repo_root(&child).await.unwrap_or(false) {
                    push_unique_spec_path(&mut roots, child);
                }
            }
        }
    }
    roots.sort_by_key(|p| spec_path_compare_key(p));
    Ok(roots)
}

pub(crate) async fn is_valid_spec_repo_root(path: &Path) -> Result<bool, String> {
    if !crate::filesystem::repositories::spec_repo_is_dir(path) {
        return Ok(false);
    }
    if crate::filesystem::repositories::spec_repo_link_state(path).await? {
        return Ok(false);
    }
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return Ok(false);
    };
    Ok(name.starts_with("repo-")
        || crate::filesystem::repositories::spec_repo_git_marker_exists(&path.join(".git")))
}

async fn canonicalize_spec_repo_directory(path: &Path) -> Result<PathBuf, String> {
    let canonical = crate::filesystem::repositories::canonical_spec_repo_path(path)
        .await
        .map_err(|e| format!("Failed to canonicalize directory {}: {}", path.display(), e))?;
    let metadata = crate::filesystem::repositories::spec_repo_metadata(&canonical)
        .await
        .map_err(|e| format!("Failed to inspect directory {}: {}", path.display(), e))?;
    if !metadata.is_dir() {
        return Err(format!("Path is not a directory: {}", path.display()));
    }
    Ok(canonical)
}

fn push_unique_spec_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let key = spec_path_compare_key(&path);
    if !paths.iter().any(|p| spec_path_compare_key(p) == key) {
        paths.push(path);
    }
}

fn spec_path_compare_key(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('/', "\\");
    if let Some(stripped) = s.strip_prefix(r"\\?\UNC\") {
        s = format!(r"\\{}", stripped);
    } else if let Some(stripped) = s.strip_prefix(r"\\?\") {
        s = stripped.to_string();
    }
    while s.ends_with('\\') && s.len() > 3 {
        s.pop();
    }
    if cfg!(windows) {
        s.to_ascii_lowercase()
    } else {
        s
    }
}

#[cfg(test)]
mod tests;
