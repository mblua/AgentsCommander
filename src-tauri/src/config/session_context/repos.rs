use super::{
    canonical_or_original, display_path, find_ac_root, is_replica_agent_dir,
    managed_template_local_override, resolve_replica_matrix_root, simple_hash,
    strip_yaml_frontmatter, ROLE_MD_FILENAME,
};
use crate::config::instance_artifacts::CONTEXT_CACHE_DIR_NAME;
use std::ffi::OsStr;
use std::path::Path;

pub(crate) fn write_combined_context_file(
    cwd: &str,
    resolved_paths: &[(String, std::path::PathBuf)],
    filename_prefix: &str,
) -> Result<String, String> {
    let mut combined = String::new();
    let mut first = true;

    for (label, path) in resolved_paths {
        // #1737 (D2): the base read still runs and still surfaces its error even when
        // an override exists, so an unreadable, non-UTF-8 or directory-shaped base is
        // reported exactly as it is today.
        let base_content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read context file {}: {}", path.display(), e))?;
        let content = managed_template_local_override(path).unwrap_or(base_content);
        let content: &str = if path.file_name() == Some(OsStr::new(ROLE_MD_FILENAME)) {
            strip_yaml_frontmatter(&content)
        } else {
            &content
        };
        if first {
            combined.push_str(content);
            first = false;
        } else {
            combined.push_str(&format!("\n\n---\n\n# Context: {}\n\n", label));
            combined.push_str(content);
        }
    }

    let config_dir = super::super::config_dir()
        .ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let context_dir = config_dir.join(CONTEXT_CACHE_DIR_NAME);
    std::fs::create_dir_all(&context_dir)
        .map_err(|e| format!("Failed to create context-cache dir: {}", e))?;

    let hash = simple_hash(cwd);
    let file_path = context_dir.join(format!("{}-{}.md", filename_prefix, hash));
    std::fs::write(&file_path, &combined)
        .map_err(|e| format!("Failed to write combined context file: {}", e))?;

    Ok(file_path.to_string_lossy().to_string())
}

pub(crate) fn resolved_paths_include_path(
    resolved_paths: &[(String, std::path::PathBuf)],
    candidate: &std::path::Path,
) -> bool {
    resolved_paths.iter().any(|(_, path)| {
        if path == candidate {
            return true;
        }

        match (
            std::fs::canonicalize(path),
            std::fs::canonicalize(candidate),
        ) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
    })
}

pub(crate) fn has_agent_matrix_dir_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("_agent_"))
        .unwrap_or(false)
}

pub(crate) fn path_parent_is_ac_root(path: &Path) -> bool {
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .map(crate::config::ac_root::is_ac_root_name)
        .unwrap_or(false)
}

/// True only for an ORIGIN Agent Matrix: a real (non-symlink) `_agent_*`
/// directory whose canonical parent is a workspace directory.
///
/// #1172's memory rotation (`config::agent_memory`) is the second consumer and
/// depends on both halves of that contract. It relies on the `_agent_` prefix to
/// reject every `__agent_*` replica, including the `role_experiment` replicas
/// that do carry a `memory/` of their own (`cli/role_experiment.rs:2259-2267`),
/// and on the `ac-root-agent` name failing the prefix so the Root Agent is never
/// rotated (D6). Widening either half silently widens what rotation may move.
pub(crate) fn is_canonical_agent_matrix_dir(cwd: &str) -> bool {
    let path = Path::new(cwd);
    if !has_agent_matrix_dir_name(path) {
        return false;
    }

    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    let file_type = metadata.file_type();
    if !metadata.is_dir() || file_type.is_symlink() {
        return false;
    }

    let Ok(canonical_path) = std::fs::canonicalize(path) else {
        return false;
    };
    has_agent_matrix_dir_name(&canonical_path) && path_parent_is_ac_root(&canonical_path)
}

pub(crate) fn is_agent_dir(cwd: &str) -> bool {
    is_replica_agent_dir(cwd)
        || is_canonical_agent_matrix_dir(cwd)
        || super::super::root_agent::is_root_agent_dir_name(cwd)
}

/// Build the GIT_CEILING_DIRECTORIES value for agent sessions rooted in a Project AC Root.
/// This blocks Git from traversing upward into the parent project repo when the
/// current directory is an agent matrix, a WG replica, or a descendant of those roots.
pub fn git_ceiling_directories_for_session_root(cwd: &str) -> Option<String> {
    if !is_agent_dir(cwd) {
        return None;
    }

    let cwd_path = std::path::Path::new(cwd);
    let mut ordered: Vec<std::path::PathBuf> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut push_unique = |path: std::path::PathBuf| {
        let canonical = canonical_or_original(&path);
        let key = display_path(&canonical);
        if seen.insert(key) {
            ordered.push(canonical);
        }
    };

    if let Some(ac_root) = find_ac_root(cwd_path) {
        push_unique(ac_root);
    }

    push_unique(cwd_path.to_path_buf());

    match resolve_replica_matrix_root(cwd) {
        Ok(Some(matrix_root)) => push_unique(std::path::PathBuf::from(matrix_root)),
        Ok(None) => {}
        Err(e) => {
            log::warn!(
                "[session_context] Rejected invalid WG replica identity while building Git ceiling \
                    for '{}': {}",
                cwd,
                e
            );
        }
    }

    if ordered.is_empty() {
        return None;
    }

    std::env::join_paths(ordered.iter())
        .ok()
        .map(|paths| paths.to_string_lossy().to_string())
        .or_else(|| {
            Some(
                ordered
                    .iter()
                    .map(|p| display_path(p))
                    .collect::<Vec<_>>()
                    .join(if cfg!(windows) { ";" } else { ":" }),
            )
        })
}

/// #979 G5: the Root prologue is Root-specific, so its empty case and its intro
/// sentence name the Root Agent instead of a workgroup replica. `is_root_agent`
/// is `is_root_agent_path`, which no matrix or WG root satisfies, so every
/// non-root rendering stays byte-identical to before.
pub(crate) fn agent_repos_empty_block(is_root_agent: bool) -> &'static str {
    if is_root_agent {
        "# Agent Repos\n\nNo repos configured for the Root Agent.\n"
    } else {
        "# Agent Repos\n\nNo repos configured for this replica.\n"
    }
}

pub(crate) fn agent_repos_header(is_root_agent: bool) -> &'static str {
    if is_root_agent {
        "# Agent Repos\n\n\
         You are the Root Agent. Your code repos are listed below; you MUST change into the \
         appropriate repo directory before any code work (git, file edits, builds).\n\n\
         ## Repos\n\n"
    } else {
        "# Agent Repos\n\n\
         You are working inside a room replica. Your code repos are listed below; you MUST change \
         into the \
         appropriate repo directory before any code work (git, file edits, builds).\n\n\
         ## Repos\n\n"
    }
}

pub(crate) fn render_agent_repos_string(
    cwd_path: &std::path::Path,
    config: Option<&serde_json::Value>,
    repo_mounts: Option<&crate::pty::container_repos::RepoMountResolution>,
    is_root_agent: bool,
) -> String {
    // #935 - a containerized session renders CONTAINER paths from the ONE
    // resolution session.rs already built (plan Sec 5), so the injected context
    // and the Docker mounts cannot disagree. A local session (repo_mounts = None)
    // keeps today's host-path rendering, byte-for-byte.
    if let Some(resolution) = repo_mounts {
        return render_agent_repos_containerized(resolution, is_root_agent);
    }
    let repos = config
        .and_then(|config| config.get("repos"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if repos.is_empty() {
        return agent_repos_empty_block(is_root_agent).to_string();
    }

    let mut md = String::from(agent_repos_header(is_root_agent));

    for repo_val in &repos {
        let rel = match repo_val.as_str() {
            Some(s) => s,
            None => continue,
        };

        let resolved = cwd_path.join(rel);
        // Canonicalize to get a clean absolute path (strip \\?\ on Windows)
        let abs_path = std::fs::canonicalize(&resolved)
            .map(|p| display_path(&p))
            .unwrap_or_else(|_| resolved.to_string_lossy().to_string());

        let repo_name = resolved.file_name().and_then(|n| n.to_str()).unwrap_or(rel);

        if !resolved.exists() {
            md.push_str(&format!(
                "- **{}** — Path: `{}` — **(NOT FOUND)**\n",
                repo_name, abs_path
            ));
            continue;
        }

        let branch = detect_git_branch(&abs_path).unwrap_or_else(|| "unknown".to_string());
        md.push_str(&format!(
            "- **{}** — Path: `{}` — Branch: `{}`\n",
            repo_name, abs_path, branch
        ));
    }

    md
}

/// #935 - render the injected "Agent Repos" block for a CONTAINER session
/// from the resolved mounts. Container paths (/repos/repo-X) are shown, never the
/// Windows host path, which does not resolve inside the container (defect D2). The
/// branch is still detected host-side from the canonical host path.
pub(crate) fn render_agent_repos_containerized(
    resolution: &crate::pty::container_repos::RepoMountResolution,
    is_root_agent: bool,
) -> String {
    use crate::pty::container_repos::RepoOutcome;

    if resolution.entries.is_empty() {
        return agent_repos_empty_block(is_root_agent).to_string();
    }

    let mut md = String::from(agent_repos_header(is_root_agent));

    for entry in &resolution.entries {
        match &entry.outcome {
            RepoOutcome::Mounted {
                name,
                host_path,
                container_path,
            } => {
                let branch = detect_git_branch(&host_path.to_string_lossy())
                    .unwrap_or_else(|| "unknown".to_string());
                md.push_str(&format!(
                    "- **{}** — Path: `{}` — Branch: `{}`\n",
                    name, container_path, branch
                ));
            }
            RepoOutcome::NotFound => {
                let name = std::path::Path::new(&entry.config_entry)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&entry.config_entry);
                md.push_str(&format!(
                    "- **{}** — Path: `{}/{}` — **(NOT FOUND)**\n",
                    name,
                    crate::pty::container_repos::CONTAINER_REPOS_ROOT,
                    name
                ));
            }
        }
    }

    md
}

/// Detect git branch for a given directory path.
pub(crate) fn detect_git_branch(dir: &str) -> Option<String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = std::process::Command::new("git");
    crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_std_command(&mut cmd);
    cmd.args(["-C", dir, "branch", "--show-current"]);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.output() {
        Ok(out) if out.status.success() => {
            let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if branch.is_empty() || branch == "HEAD" {
                None
            } else {
                Some(branch)
            }
        }
        _ => None,
    }
}
