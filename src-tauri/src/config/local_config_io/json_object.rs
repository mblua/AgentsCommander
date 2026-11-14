use super::pair_image::PairTargetGuard;
use super::publish::publish_temp_config;
use super::staging::{acquire_fresh_config_stage, ordinary_staging_error};
use super::write_lock::{
    acquire_config_file_write_lock, lock_local_config_writes, ConfigWriterActive,
    CONFIG_LOCK_TIMEOUT,
};
use crate::filesystem::local_config::{
    ensure_local_config_parent, flush_local_config_stage, local_config_target_exists,
    read_local_config_text, remove_local_config_stage, sync_local_config_stage,
    write_local_config_stage, LocalConfigStage,
};
use serde_json::Map;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// #4547 - internal errors of the local config JSON writer. Each variant keeps the
/// failing io/serde error as `source()`; the text equals the pre-#4547 `format!`.
#[derive(Debug, thiserror::Error)]
pub(super) enum ConfigJsonError {
    #[error("Local config {} has no parent directory", path.display())]
    NoParent { path: PathBuf },
    #[error("Failed to create {}: {source}", parent.display())]
    CreateParent {
        parent: PathBuf,
        source: std::io::Error,
    },
    #[error("Failed to read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Failed to parse {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("Local config {} must be a JSON object", path.display())]
    NotObject { path: PathBuf },
    #[error("Local config {} does not exist", path.display())]
    Missing { path: PathBuf },
    #[error("Failed to serialize {}: {source}", path.display())]
    Serialize {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("Failed to write temp config {}: {source}", tmp.display())]
    WriteStage {
        tmp: PathBuf,
        source: std::io::Error,
    },
    #[error("Failed to flush temp config {}: {source}", tmp.display())]
    FlushStage {
        tmp: PathBuf,
        source: std::io::Error,
    },
    #[error("Failed to sync temp config {}: {source}", tmp.display())]
    SyncStage {
        tmp: PathBuf,
        source: std::io::Error,
    },
}

impl From<ConfigJsonError> for String {
    fn from(error: ConfigJsonError) -> Self {
        error.to_string()
    }
}

pub(super) fn ensure_config_parent(path: &Path) -> Result<(), ConfigJsonError> {
    let parent = path.parent().ok_or_else(|| ConfigJsonError::NoParent {
        path: path.to_path_buf(),
    })?;
    ensure_local_config_parent(parent).map_err(|source| ConfigJsonError::CreateParent {
        parent: parent.to_path_buf(),
        source,
    })
}

pub(super) fn read_config_root(path: &Path, allow_create: bool) -> Result<Value, ConfigJsonError> {
    if !local_config_target_exists(path) {
        if allow_create {
            return Ok(Value::Object(Map::new()));
        }
        return Err(ConfigJsonError::Missing {
            path: path.to_path_buf(),
        });
    }
    let content = read_local_config_text(path).map_err(|source| ConfigJsonError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed: Value =
        serde_json::from_str(&content).map_err(|source| ConfigJsonError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    if !parsed.is_object() {
        return Err(ConfigJsonError::NotObject {
            path: path.to_path_buf(),
        });
    }
    Ok(parsed)
}

fn write_config_stage(
    file: &mut LocalConfigStage,
    tmp: &Path,
    bytes: &[u8],
) -> Result<(), ConfigJsonError> {
    let tmp_buf = || tmp.to_path_buf();
    write_local_config_stage(file, bytes).map_err(|source| ConfigJsonError::WriteStage {
        tmp: tmp_buf(),
        source,
    })?;
    flush_local_config_stage(file).map_err(|source| ConfigJsonError::FlushStage {
        tmp: tmp_buf(),
        source,
    })?;
    sync_local_config_stage(file).map_err(|source| ConfigJsonError::SyncStage {
        tmp: tmp_buf(),
        source,
    })
}

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
    ensure_config_parent(path)?;
    let _file_lock = acquire_config_file_write_lock(path, CONFIG_LOCK_TIMEOUT)?;

    let mut root = read_config_root(path, allow_create)?;

    let obj = root
        .as_object_mut()
        .ok_or_else(|| ConfigJsonError::NotObject {
            path: path.to_path_buf(),
        })?;
    mutate(obj)?;

    let mut json =
        serde_json::to_string_pretty(&root).map_err(|source| ConfigJsonError::Serialize {
            path: path.to_path_buf(),
            source,
        })?;
    json.push('\n');

    let guard = PairTargetGuard::new(&|| Ok(()));
    let (tmp_path, mut file) =
        acquire_fresh_config_stage(path, &guard).map_err(ordinary_staging_error)?;

    let write_result = write_config_stage(&mut file, &tmp_path, json.as_bytes());
    drop(file);
    if let Err(e) = write_result {
        let _ = remove_local_config_stage(&tmp_path);
        return Err(e.into());
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

    ensure_config_parent(path)?;
    let guard = PairTargetGuard::new(&|| Ok(()));
    let (tmp_path, mut file) =
        acquire_fresh_config_stage(path, &guard).map_err(ordinary_staging_error)?;

    let write_result = write_config_stage(&mut file, &tmp_path, bytes);
    drop(file);
    if let Err(e) = write_result {
        let _ = remove_local_config_stage(&tmp_path);
        return Err(e.into());
    }

    if let Err(e) = publish(&tmp_path, path) {
        let _ = remove_local_config_stage(&tmp_path);
        return Err(e);
    }

    Ok(())
}
