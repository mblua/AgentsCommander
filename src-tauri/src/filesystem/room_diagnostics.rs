//! Physical room resources for lock diagnostics.

#[cfg(windows)]
use std::path::{Path, PathBuf};

/// Files commonly held by long-running processes inside an AC workgroup:
/// any `*.lock` (git index.lock, `ORIG_HEAD.lock`, packfile locks), plus the
/// lock-prone metadata files inside any `.git/` subtree.
#[cfg(windows)]
fn is_hot_lock_candidate(p: &Path) -> bool {
    let name = match p.file_name().and_then(|n| n.to_str()) {
        Some(s) => s,
        None => return false,
    };
    if name.ends_with(".lock") {
        return true;
    }
    let in_git = p
        .ancestors()
        .any(|a| a.file_name().and_then(|n| n.to_str()) == Some(".git"));
    in_git
        && matches!(
            name,
            "index" | "HEAD" | "ORIG_HEAD" | "FETCH_HEAD" | "MERGE_HEAD" | "packed-refs"
        )
}

/// Top-level WG-child dirs whose handles commonly indicate a blocker:
/// `repo-*` (clones — git operations, IDE workspaces),
/// `__agent_*` (replicas — agent-spawned shells holding cwd),
/// `messaging/` (mailbox — file watchers).
#[cfg(windows)]
fn is_relevant_top_level_dir(name: &str) -> bool {
    name.starts_with("repo-") || name.starts_with("__agent_") || name == "messaging"
}

#[cfg(windows)]
fn scan_room_dir(
    dir: &Path,
    is_wg_root: bool,
    dirs: &mut Vec<PathBuf>,
    hot: &mut Vec<PathBuf>,
    cold: &mut Vec<PathBuf>,
    queue: &mut std::collections::VecDeque<PathBuf>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if is_wg_root {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if is_relevant_top_level_dir(name) {
                        dirs.push(path.clone());
                    }
                }
            }
            queue.push_back(path);
        } else if ft.is_file() {
            if is_hot_lock_candidate(&path) {
                hot.push(path);
            } else {
                cold.push(path);
            }
        }
    }
}

#[cfg(windows)]
pub(crate) fn collect_room_lock_resources(wg_dir: &Path, max_files: usize) -> Vec<PathBuf> {
    use std::collections::VecDeque;

    // Soft ceiling on total walk size — once we've inventoried 4× the probe
    // budget, we have plenty to choose from. Avoids walking gigabytes of
    // `.git/objects/` when the WG is unusually large.
    let walk_soft_ceiling = max_files * 4;

    // Always include the WG root itself. A terminal cwd or IDE workspace open
    // anywhere under the tree usually surfaces via a handle on this directory.
    let mut dirs: Vec<PathBuf> = vec![wg_dir.to_path_buf()];
    let mut hot: Vec<PathBuf> = Vec::new();
    let mut cold: Vec<PathBuf> = Vec::new();
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    queue.push_back(wg_dir.to_path_buf());

    while let Some(dir) = queue.pop_front() {
        if hot.len() + cold.len() + dirs.len() >= walk_soft_ceiling {
            break;
        }
        let is_wg_root = dir == wg_dir;
        scan_room_dir(&dir, is_wg_root, &mut dirs, &mut hot, &mut cold, &mut queue);
    }

    // Dirs first (the new signal — small set, ~5–10 entries; prefer over cold
    // files as the plan dictates), then hot files, then cold files. All
    // capped at max_files total.
    let mut out = dirs;
    out.truncate(max_files);
    let remaining = max_files.saturating_sub(out.len());
    out.extend(hot.into_iter().take(remaining));
    let remaining = max_files.saturating_sub(out.len());
    out.extend(cold.into_iter().take(remaining));
    out
}

#[cfg(windows)]
pub(crate) fn room_lock_resource_exists(path: &Path) -> bool {
    path.exists()
}
