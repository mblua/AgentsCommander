use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::config::settings::AppSettings;

#[derive(Clone)]
pub(super) struct ValidatedPath {
    pub(super) path: PathBuf,
    pub(super) repo_root: PathBuf,
}

pub(super) async fn resolve_repo_root(
    requested: Option<&str>,
    settings: &AppSettings,
) -> Result<PathBuf, String> {
    let roots = valid_repo_roots(settings).await?;
    if let Some(requested) = requested.filter(|s| !s.trim().is_empty()) {
        let requested_path = PathBuf::from(requested);
        let canonical = canonicalize_existing_dir(&requested_path).await?;
        if roots
            .iter()
            .any(|root| path_compare_key(root) == path_compare_key(&canonical))
        {
            return Ok(canonical);
        }
        if is_valid_repo_root(&canonical).await? {
            return Ok(canonical);
        }
        return Err(format!(
            "Selected path is not a valid repository root: {}",
            requested
        ));
    }
    roots
        .into_iter()
        .next()
        .ok_or_else(|| "No valid repo-* root is loaded for the spec board".to_string())
}

pub(super) async fn valid_repo_roots(settings: &AppSettings) -> Result<Vec<PathBuf>, String> {
    let mut candidates = Vec::new();
    if let Some(path) = &settings.project_path {
        candidates.push(PathBuf::from(path));
    }
    candidates.extend(settings.project_paths.iter().map(PathBuf::from));

    let mut roots = Vec::new();
    for candidate in candidates {
        let Ok(canonical) = canonicalize_existing_dir(&candidate).await else {
            continue;
        };
        if is_valid_repo_root(&canonical).await.unwrap_or(false) {
            push_unique_path(&mut roots, canonical.clone());
        }
        if let Ok(mut entries) = tokio::fs::read_dir(&canonical).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if name.starts_with('.') || !name.starts_with("repo-") {
                    continue;
                }
                let Ok(child) = canonicalize_existing_dir(&path).await else {
                    continue;
                };
                if is_valid_repo_root(&child).await.unwrap_or(false) {
                    push_unique_path(&mut roots, child);
                }
            }
        }
    }
    roots.sort_by_key(|p| path_compare_key(p));
    Ok(roots)
}

pub(super) async fn validate_existing_user_path(
    path: &Path,
    settings: &AppSettings,
) -> Result<ValidatedPath, String> {
    validate_user_path(path, settings, true).await
}

pub(super) async fn validate_new_or_existing_user_path(
    path: &Path,
    settings: &AppSettings,
) -> Result<ValidatedPath, String> {
    validate_user_path(path, settings, false).await
}

pub(super) async fn validate_user_path(
    path: &Path,
    settings: &AppSettings,
    must_exist: bool,
) -> Result<ValidatedPath, String> {
    reject_unsupported_extension(path)?;
    reject_ac_component(path)?;
    if must_exist {
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| format!("Spec path does not exist or is inaccessible: {}", e))?;
        if meta.is_dir() {
            return Err("Spec path points to a directory".to_string());
        }
    }

    let roots = valid_repo_roots(settings).await?;
    if roots.is_empty() {
        return Err("No valid repo-* root is loaded for the spec board".to_string());
    }

    let nearest = nearest_existing_parent(path).await?;
    let canonical_parent = canonicalize_existing_dir(&nearest).await?;
    let suffix = relative_suffix_from_existing_parent(path, &nearest)?;
    let canonical_candidate = if must_exist && path.exists() {
        tokio::fs::canonicalize(path)
            .await
            .map_err(|e| format!("Failed to canonicalize spec path: {}", e))?
    } else {
        canonical_parent.join(suffix)
    };

    for root in roots {
        if path_is_under(&canonical_candidate, &root) {
            reject_link_ancestors(&root, &canonical_candidate).await?;
            return Ok(ValidatedPath {
                path: canonical_candidate,
                repo_root: root,
            });
        }
    }

    Err(format!(
        "Spec path is outside every valid repo root: {}",
        path.display()
    ))
}

pub(super) async fn internal_dirs(
    repo_root: &Path,
    doc_id: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let root = canonicalize_existing_dir(repo_root).await?;
    let base = root.join(".ac").join("mermaids");
    Ok((
        base.join("snapshots").join(doc_id),
        base.join("drafts").join(doc_id),
    ))
}

pub(super) async fn is_valid_repo_root(path: &Path) -> Result<bool, String> {
    if !path.is_dir() {
        return Ok(false);
    }
    if is_link_or_reparse(path).await? {
        return Ok(false);
    }
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return Ok(false);
    };
    Ok(name.starts_with("repo-") || path.join(".git").exists())
}

pub(super) async fn nearest_existing_parent(path: &Path) -> Result<PathBuf, String> {
    let mut current = if path.exists() {
        path.to_path_buf()
    } else {
        path.parent()
            .ok_or_else(|| "Spec path has no parent directory".to_string())?
            .to_path_buf()
    };
    while !current.exists() {
        current = current
            .parent()
            .ok_or_else(|| "Spec path has no existing parent".to_string())?
            .to_path_buf();
    }
    if current.is_file() {
        current = current
            .parent()
            .ok_or_else(|| "Spec path parent could not be resolved".to_string())?
            .to_path_buf();
    }
    Ok(current)
}

pub(super) fn relative_suffix_from_existing_parent(
    path: &Path,
    parent: &Path,
) -> Result<PathBuf, String> {
    let suffix = path
        .strip_prefix(parent)
        .map_err(|_| "Spec path could not be resolved from its existing parent".to_string())?;
    if suffix.as_os_str().is_empty() {
        return Err("Spec path must include a file name".to_string());
    }

    let mut clean = PathBuf::new();
    for component in suffix.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err("Spec path traversal is not allowed".to_string());
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err("Spec path suffix must be relative".to_string());
            }
        }
    }

    if clean.file_name().is_none() {
        return Err("Spec path must include a file name".to_string());
    }
    Ok(clean)
}

pub(super) async fn canonicalize_existing_dir(path: &Path) -> Result<PathBuf, String> {
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|e| format!("Failed to canonicalize directory {}: {}", path.display(), e))?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|e| format!("Failed to inspect directory {}: {}", path.display(), e))?;
    if !metadata.is_dir() {
        return Err(format!("Path is not a directory: {}", path.display()));
    }
    Ok(canonical)
}

pub(super) async fn reject_link_ancestors(root: &Path, target: &Path) -> Result<(), String> {
    let mut current = root.to_path_buf();
    if is_link_or_reparse(&current).await? {
        return Err(format!(
            "Repo root is a link or reparse point: {}",
            root.display()
        ));
    }

    let relative = target
        .parent()
        .unwrap_or(target)
        .strip_prefix(root)
        .map_err(|_| "Spec path is not under the repo root".to_string())?;
    for component in relative.components() {
        current.push(component.as_os_str());
        if current.exists() && is_link_or_reparse(&current).await? {
            return Err(format!(
                "Spec path contains a link or reparse point: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

pub(super) async fn is_link_or_reparse(path: &Path) -> Result<bool, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| format!("Failed to lstat {}: {}", path.display(), e))?;
        if meta.file_type().is_symlink() {
            return Ok(true);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await
    .map_err(|e| format!("Failed to inspect path link state: {}", e))?
}

pub(super) fn reject_unsupported_extension(path: &Path) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("mmd" | "mermaid" | "md") => Ok(()),
        _ => Err("Spec board supports only .mmd, .mermaid, and .md files".to_string()),
    }
}

pub(super) fn reject_ac_component(path: &Path) -> Result<(), String> {
    if path.components().any(|component| match component {
        Component::Normal(part) => part.to_string_lossy().eq_ignore_ascii_case(".ac"),
        _ => false,
    }) {
        return Err("User spec paths under .ac are not allowed".to_string());
    }
    Ok(())
}

pub(super) fn path_is_under(path: &Path, root: &Path) -> bool {
    let path_key = path_compare_key(path);
    let root_key = path_compare_key(root);
    path_key == root_key
        || path_key
            .strip_prefix(&(root_key + std::path::MAIN_SEPARATOR_STR))
            .is_some()
}

pub(super) fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let key = path_compare_key(&path);
    if !paths.iter().any(|p| path_compare_key(p) == key) {
        paths.push(path);
    }
}

pub(super) fn path_compare_key(path: &Path) -> String {
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
