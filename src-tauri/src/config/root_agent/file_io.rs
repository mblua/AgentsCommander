use super::{
    is_link_or_reparse, ROOT_AGENT_DEFAULT_CONTEXT, ROOT_AGENT_OLD_DEFAULT_CONTEXT,
    ROOT_AGENT_PRE_979_DEFAULT_CONTEXT, ROOT_ROLE_TEMP_COUNTER, SKILL_MD_FILENAME,
};
#[cfg(test)]
use super::{FAIL_ROOT_ROLE_WRITE_MARKER, FAIL_ROOT_ROLE_WRITE_ONCE};
use crate::filesystem::agent_files;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

pub(crate) fn write_role_file(
    file: &mut agent_files::RootAgentStage,
    role_path: &Path,
    content: &str,
) -> Result<(), String> {
    #[cfg(test)]
    if content.contains(FAIL_ROOT_ROLE_WRITE_MARKER)
        && FAIL_ROOT_ROLE_WRITE_ONCE.swap(false, Ordering::SeqCst)
    {
        return Err(format!(
            "Failed to write {}: injected failure",
            role_path.display()
        ));
    }

    agent_files::write_root_agent_stage(file, content.as_bytes())
        .map_err(|e| format!("Failed to write {}: {}", role_path.display(), e))?;
    agent_files::flush_root_agent_stage(file)
        .map_err(|e| format!("Failed to flush {}: {}", role_path.display(), e))?;
    agent_files::sync_root_agent_stage(file)
        .map_err(|e| format!("Failed to sync {}: {}", role_path.display(), e))
}

pub(crate) fn unique_role_temp_path(role_path: &Path) -> std::path::PathBuf {
    let parent = role_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = role_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Role.md");
    let counter = ROOT_ROLE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        counter
    ))
}

pub(crate) fn create_default_skill_temp_file(
    path: &Path,
) -> Result<(PathBuf, agent_files::RootAgentStage), String> {
    create_default_skill_temp_file_with(path, unique_default_skill_temp_path)
}

pub(crate) fn create_default_skill_temp_file_with<F>(
    path: &Path,
    mut next_temp_path: F,
) -> Result<(PathBuf, agent_files::RootAgentStage), String>
where
    F: FnMut(&Path) -> PathBuf,
{
    const TEMP_CREATE_ATTEMPTS: usize = 16;

    for _ in 0..TEMP_CREATE_ATTEMPTS {
        let temp_path = next_temp_path(path);
        match agent_files::create_root_agent_stage(&temp_path) {
            Ok(file) => return Ok((temp_path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to create temporary root agent skill file {}: {}",
                    temp_path.display(),
                    e
                ));
            }
        }
    }

    Err(format!(
        "Failed to create temporary root agent skill file for {} after {} attempts",
        path.display(),
        TEMP_CREATE_ATTEMPTS
    ))
}

pub(crate) fn unique_default_skill_temp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(SKILL_MD_FILENAME);
    let counter = ROOT_ROLE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        counter
    ))
}

pub(crate) fn cleanup_temp_role(path: &Path) {
    if let Err(e) = agent_files::remove_root_agent_temp(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!(
                "Failed to remove temporary role file {}: {}",
                path.display(),
                e
            );
        }
    }
}

pub(crate) fn sync_role_dir(parent: &Path) {
    if let Ok(dir) = agent_files::open_root_agent_directory(parent) {
        if let Err(e) = agent_files::sync_root_agent_directory(&dir) {
            log::warn!(
                "Failed to sync root agent role directory {}: {}",
                parent.display(),
                e
            );
        }
    }
}

pub(crate) fn publish_missing_role_file(
    temp_path: &Path,
    role_path: &Path,
) -> Result<bool, String> {
    match agent_files::publish_root_agent_missing(temp_path, role_path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(format!(
            "Failed to publish missing role file {} from {}: {}",
            role_path.display(),
            temp_path.display(),
            e
        )),
    }
}

pub(crate) fn validate_default_skill_file(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<(), String> {
    if is_link_or_reparse(metadata) || !metadata.is_file() {
        return Err(format!(
            "Root agent default skill file {} exists but is not a regular file",
            path.display()
        ));
    }
    Ok(())
}

pub(crate) fn publish_missing_default_skill_file(
    temp_path: &Path,
    path: &Path,
) -> Result<bool, String> {
    publish_missing_default_skill_file_with(temp_path, path, |temp_path, path| {
        agent_files::publish_root_agent_missing(temp_path, path)
    })
}

pub(crate) fn publish_missing_default_skill_file_with<F>(
    temp_path: &Path,
    path: &Path,
    publish: F,
) -> Result<bool, String>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    match publish(temp_path, path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = agent_files::inspect_root_agent_entry(path).map_err(|inspect_err| {
                format!(
                    "Root agent default skill file {} appeared during publish but could not be \
                        inspected: {}",
                    path.display(),
                    inspect_err
                )
            })?;
            validate_default_skill_file(path, &metadata)?;
            Ok(false)
        }
        Err(e) => Err(format!(
            "Failed to publish root agent default skill file {} from {}: {}",
            path.display(),
            temp_path.display(),
            e
        )),
    }
}

#[cfg(not(windows))]
pub(crate) fn replace_role_file(temp_path: &Path, role_path: &Path) -> Result<(), String> {
    agent_files::rename_root_agent_stage(temp_path, role_path).map_err(|e| {
        format!(
            "Failed to replace {} with {}: {}",
            role_path.display(),
            temp_path.display(),
            e
        )
    })
}

#[cfg(windows)]
pub(crate) fn replace_role_file(temp_path: &Path, role_path: &Path) -> Result<(), String> {
    if !agent_files::root_agent_target_exists(role_path) {
        return agent_files::rename_root_agent_stage(temp_path, role_path).map_err(|e| {
            format!(
                "Failed to publish {} from {}: {}",
                role_path.display(),
                temp_path.display(),
                e
            )
        });
    }

    replace_existing_file_windows(temp_path, role_path)
}

#[cfg(windows)]
pub(crate) fn replace_existing_file_windows(
    temp_path: &Path,
    role_path: &Path,
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    // Normalize the parent for extended-length Windows APIs without following
    // the leaf that is about to be replaced.
    let canonical_leaf = |path: &Path| -> Result<Vec<u16>, String> {
        let leaf = path.file_name().ok_or_else(|| {
            format!(
                "Failed to normalize replacement path {}: missing file name",
                path.display()
            )
        })?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let canonical_parent = agent_files::canonical_root_agent_path(parent).map_err(|error| {
            format!(
                "Failed to normalize replacement parent for {}: {}",
                path.display(),
                error
            )
        })?;
        Ok(canonical_parent
            .join(leaf)
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect())
    };
    let role_wide = canonical_leaf(role_path)?;
    let temp_wide = canonical_leaf(temp_path)?;
    agent_files::replace_root_agent_once(&role_wide, &temp_wide).map_err(|error| {
        format!(
            "Failed to replace {} with {}: {}",
            role_path.display(),
            temp_path.display(),
            error
        )
    })
}

/// #664: shared atomic replace-existing primitive. Delegates to the vetted
/// role-file replace path (plain rename on Unix; rename-if-absent else
/// `ReplaceFileW(REPLACEFILE_WRITE_THROUGH)` on Windows). The CALLER is
/// responsible for creating, writing, fsyncing, dropping the temp handle, and
/// cleaning up `temp`; this only publishes `temp` -> `dest`.
pub(crate) fn atomic_replace_existing(temp: &Path, dest: &Path) -> Result<(), String> {
    replace_role_file(temp, dest)
}

pub(crate) fn normalize_role_text(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_string()
}

pub(crate) fn merge_root_agent_config(config_path: &Path) -> Result<(), String> {
    crate::config::agent_config::update_agent_config(config_path, |obj, _state| {
        obj.entry("tooling".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));

        // #979: strip every exact global sentinel from the Root `context[]`.
        // Compute the replacement from the immutable borrow FIRST so it ends
        // before `obj.insert`. Exact-default recognition runs before the generic
        // filter, so the oldest `[token, Role.md]` form gains the Root supplement
        // instead of degrading to `[Role.md]`.
        let context = obj.get("context").and_then(|v| v.as_array());
        let replacement = match context {
            // Missing, non-array, or empty: the existing repair for an unusable
            // canonical config.
            None => Some(serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT)),
            Some(arr) if arr.is_empty() => Some(serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT)),
            Some(arr)
                if context_array_matches(arr, ROOT_AGENT_PRE_979_DEFAULT_CONTEXT)
                    || context_array_matches(arr, ROOT_AGENT_OLD_DEFAULT_CONTEXT) =>
            {
                Some(serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT))
            }
            Some(arr) => {
                // Exact means case-sensitive and byte-for-byte: a near-token such
                // as `"$AGENTSCOMMANDER_CONTEXT "` stays an ordinary custom entry
                // and is later resolved as a path. Non-string values are preserved.
                let filtered: Vec<Value> = arr
                    .iter()
                    .filter(|value| value.as_str() != Some(super::CONTEXT_TOKEN_GLOBAL))
                    .cloned()
                    .collect();
                if filtered.len() == arr.len() {
                    None
                } else if filtered.is_empty() {
                    // Token-only array: converge straight to the default. Writing
                    // `[]` here would be repaired to the default on the NEXT run,
                    // which is not idempotent.
                    Some(serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT))
                } else {
                    Some(Value::Array(filtered))
                }
            }
        };
        if let Some(replacement) = replacement {
            obj.insert("context".to_string(), replacement);
        }
        Ok(())
    })?;
    Ok(())
}

pub(crate) fn context_array_matches(arr: &[Value], expected: &[&str]) -> bool {
    arr.len() == expected.len()
        && arr
            .iter()
            .zip(expected)
            .all(|(value, expected)| value.as_str() == Some(*expected))
}

pub fn read_last_coding_agent(root_dir: &str) -> Option<String> {
    // #2786 (C1) - the raw loader with field-level extraction, as before C1:
    // an unrelated malformed field must not hide `lastCodingAgent`.
    crate::config::agent_config::read_agent_local_config_json(Path::new(root_dir))
        .ok()??
        .get("tooling")?
        .get("lastCodingAgent")?
        .as_str()
        .map(ToString::to_string)
}

pub(crate) fn paths_equivalent(left: &Path, right: &Path) -> bool {
    match (
        crate::path_identity::verify_directory(left),
        crate::path_identity::verify_directory(right),
    ) {
        (Ok(left), Ok(right)) => left.object_id == right.object_id,
        _ => false,
    }
}

pub(crate) fn display_path(path: &Path) -> String {
    crate::path_utils::path_to_string_without_windows_verbatim_prefix(path)
}
