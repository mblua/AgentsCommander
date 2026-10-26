#[cfg(test)]
use super::publish_temp_config;
use super::{ConfigPublisher, PairTargetGuard, PreparedPairError};
use serde_json::Map;
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

/// #2786 (C1) - one side of the pair as a map: absent is empty, anything that
/// is not a readable JSON object is an error.
pub(super) fn read_pair_side(path: &Path) -> Result<Map<String, Value>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(format!("Failed to read {}: {}", path.display(), e)),
    };
    match serde_json::from_str::<Value>(&content) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!(
            "Local config {} must be a JSON object",
            path.display()
        )),
        Err(e) => Err(format!("Failed to parse {}: {}", path.display(), e)),
    }
}

/// #2786 (C1) - publish the changed sides of one sequence, state first, firing
/// each side's stage only after its write.
pub(super) fn publish_pair_sides(
    decisions: (&Path, &Map<String, Value>, &Map<String, Value>),
    state: (&Path, &Map<String, Value>, &Map<String, Value>),
    on_stage: &dyn Fn(&str),
    [state_stage, decisions_stage]: [&str; 2],
    publish: ConfigPublisher<'_>,
) -> Result<(), String> {
    for ((path, before, after), stage) in [(state, state_stage), (decisions, decisions_stage)] {
        if before != after {
            publish_pair_side_with_publish(path, after, publish)?;
            on_stage(stage);
        }
    }
    Ok(())
}

/// #2786 (C1) - temp plus `publish_temp_config`, never `write_file_atomic`:
/// the pair already holds the process `Mutex`, which is not reentrant.
#[cfg(test)]
pub(super) fn publish_pair_side(path: &Path, map: &Map<String, Value>) -> Result<(), String> {
    publish_pair_side_with_publish(path, map, &publish_temp_config)
}

pub(super) fn publish_pair_side_with_publish(
    path: &Path,
    map: &Map<String, Value>,
    publish: ConfigPublisher<'_>,
) -> Result<(), String> {
    let mut json = serde_json::to_string_pretty(map)
        .map_err(|e| format!("Failed to serialize {}: {}", path.display(), e))?;
    json.push('\n');

    let guard = PairTargetGuard::new(&|| Ok(()));
    let (tmp_path, mut file) =
        acquire_fresh_config_stage(path, &guard).map_err(ordinary_staging_error)?;
    let write_result = (|| -> Result<(), String> {
        file.write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write temp config {}: {}", tmp_path.display(), e))?;
        file.flush()
            .map_err(|e| format!("Failed to flush temp config {}: {}", tmp_path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync temp config {}: {}", tmp_path.display(), e))
    })();
    drop(file);
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    if let Err(e) = publish(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(())
}

pub(super) const MAX_STAGING_ATTEMPTS: usize = 64;
static STAGING_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) fn next_shared_staging_sequence() -> Result<u64, PreparedPairError> {
    next_staging_sequence(&STAGING_SEQUENCE)
}

pub(super) fn next_staging_sequence(
    sequence: &std::sync::atomic::AtomicU64,
) -> Result<u64, PreparedPairError> {
    sequence
        .fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |value| value.checked_add(1),
        )
        .map_err(|_| PreparedPairError::Io("temporary staging sequence exhausted".into()))
}

pub(super) fn fresh_config_stage_path(path: &Path, sequence: u64) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");
    path.with_file_name(format!(".{}.{}.{}.tmp", name, std::process::id(), sequence))
}

pub(super) fn acquire_fresh_config_stage(
    path: &Path,
    guard: &PairTargetGuard<'_>,
) -> Result<(PathBuf, std::fs::File), PreparedPairError> {
    acquire_fresh_config_stage_with_sequence(path, guard, &mut next_shared_staging_sequence)
}

// The private seam selects only a sequence; it cannot supply another path or
// relax validation, exclusivity, collision handling or the attempt bound.
pub(super) fn acquire_fresh_config_stage_with_sequence(
    path: &Path,
    guard: &PairTargetGuard<'_>,
    next: &mut dyn FnMut() -> Result<u64, PreparedPairError>,
) -> Result<(PathBuf, std::fs::File), PreparedPairError> {
    for _ in 0..MAX_STAGING_ATTEMPTS {
        guard.check()?;
        let candidate = fresh_config_stage_path(path, next()?);
        guard.check()?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(PreparedPairError::Io(format!(
                    "Failed to acquire fresh temp config {}: {}",
                    candidate.display(),
                    error,
                )))
            }
        }
    }
    Err(PreparedPairError::Io(format!(
        "Fresh temp config allocation exhausted {} attempts for {}",
        MAX_STAGING_ATTEMPTS,
        path.display(),
    )))
}

pub(super) fn ordinary_staging_error(error: PreparedPairError) -> String {
    match error {
        PreparedPairError::Io(reason) => reason,
        other => other.to_string(),
    }
}

// Legacy names exist only as detached-debris regression fixture inputs.
#[cfg(test)]
pub(in crate::config) fn temp_config_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");
    path.with_file_name(format!(".{}.{}.tmp", file_name, std::process::id()))
}
