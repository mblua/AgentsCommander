use super::{
    acquire_config_file_write_lock, acquire_fresh_config_stage, lock_local_config_writes,
    ordinary_staging_error, publish_temp_config, ConfigWriterActive, PairTargetGuard,
    CONFIG_LOCK_TIMEOUT,
};
use crate::filesystem::local_config::{
    ensure_local_config_parent, flush_local_config_stage, local_config_target_exists,
    read_local_config_text, remove_local_config_stage, sync_local_config_stage,
    write_local_config_stage,
};
use serde_json::Map;
use serde_json::Value;
use std::path::Path;

pub fn update_config_json_object<F>(
    path: &Path,
    allow_create: bool,
    mutate: F,
) -> Result<Value, String>
where
    F: FnOnce(&mut Map<String, Value>) -> Result<(), String>,
{
    update_config_json_object_with_publish(path, allow_create, mutate, publish_temp_config)
}

/// #1938 - the publish step is a parameter so tests can force a publish failure
/// without a second copy of the read-modify-write body (mirrors
/// `write_file_atomic_with_publish`). Production callers keep using
/// `update_config_json_object`, which passes the real atomic publisher.
pub(super) fn update_config_json_object_with_publish<F, P>(
    path: &Path,
    allow_create: bool,
    mutate: F,
    publish: P,
) -> Result<Value, String>
where
    F: FnOnce(&mut Map<String, Value>) -> Result<(), String>,
    P: FnOnce(&Path, &Path) -> Result<(), String>,
{
    let _writer = ConfigWriterActive::enter("update_config_json_object")?;
    let _guard = lock_local_config_writes();

    // #1938 - the parent must exist before the sidecar can be resolved, and the
    // sidecar lock must be held before the existence check, the read, the
    // mutate closure, the temp write and the publish. The whole read-modify-
    // publish cycle therefore runs under one cross-process guard; moving any of
    // it above the lock reopens the lost-update window this phase closes.
    let parent = path
        .parent()
        .ok_or_else(|| format!("Local config {} has no parent directory", path.display()))?;
    ensure_local_config_parent(parent)
        .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    let _file_lock = acquire_config_file_write_lock(path, CONFIG_LOCK_TIMEOUT)?;

    let mut root = if local_config_target_exists(path) {
        let content = read_local_config_text(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let parsed: Value = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
        if !parsed.is_object() {
            return Err(format!(
                "Local config {} must be a JSON object",
                path.display()
            ));
        }
        parsed
    } else if allow_create {
        Value::Object(Map::new())
    } else {
        return Err(format!("Local config {} does not exist", path.display()));
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| format!("Local config {} must be a JSON object", path.display()))?;
    mutate(obj)?;

    let mut json = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize {}: {}", path.display(), e))?;
    json.push('\n');

    let guard = PairTargetGuard::new(&|| Ok(()));
    let (tmp_path, mut file) =
        acquire_fresh_config_stage(path, &guard).map_err(ordinary_staging_error)?;

    let write_result = (|| -> Result<(), String> {
        write_local_config_stage(&mut file, json.as_bytes())
            .map_err(|e| format!("Failed to write temp config {}: {}", tmp_path.display(), e))?;
        flush_local_config_stage(&mut file)
            .map_err(|e| format!("Failed to flush temp config {}: {}", tmp_path.display(), e))?;
        sync_local_config_stage(&file)
            .map_err(|e| format!("Failed to sync temp config {}: {}", tmp_path.display(), e))
    })();
    drop(file);
    if let Err(e) = write_result {
        let _ = remove_local_config_stage(&tmp_path);
        return Err(e);
    }

    if let Err(e) = publish(&tmp_path, path) {
        let _ = remove_local_config_stage(&tmp_path);
        return Err(e);
    }

    Ok(root)
}

pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_file_atomic_with_publish(path, bytes, publish_temp_config)
}

pub(in crate::config) fn write_file_atomic_with_publish<P>(
    path: &Path,
    bytes: &[u8],
    publish: P,
) -> Result<(), String>
where
    P: FnOnce(&Path, &Path) -> Result<(), String>,
{
    let _writer = ConfigWriterActive::enter("write_file_atomic")?;
    let _guard = lock_local_config_writes();

    let parent = path
        .parent()
        .ok_or_else(|| format!("Local config {} has no parent directory", path.display()))?;
    ensure_local_config_parent(parent)
        .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    let guard = PairTargetGuard::new(&|| Ok(()));
    let (tmp_path, mut file) =
        acquire_fresh_config_stage(path, &guard).map_err(ordinary_staging_error)?;

    let write_result = (|| -> Result<(), String> {
        write_local_config_stage(&mut file, bytes)
            .map_err(|e| format!("Failed to write temp config {}: {}", tmp_path.display(), e))?;
        flush_local_config_stage(&mut file)
            .map_err(|e| format!("Failed to flush temp config {}: {}", tmp_path.display(), e))?;
        sync_local_config_stage(&file)
            .map_err(|e| format!("Failed to sync temp config {}: {}", tmp_path.display(), e))
    })();
    drop(file);
    if let Err(e) = write_result {
        let _ = remove_local_config_stage(&tmp_path);
        return Err(e);
    }

    if let Err(e) = publish(&tmp_path, path) {
        let _ = remove_local_config_stage(&tmp_path);
        return Err(e);
    }

    Ok(())
}
