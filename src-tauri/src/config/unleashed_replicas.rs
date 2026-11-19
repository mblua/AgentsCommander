// #4752 P02: Unleash flag store for replicas and temp rooms (#4780). This module is the
// sole owner of `unleashed-replicas.json` in the app config directory. An unleashed
// replica spawns without the Git brakes; the flag persists until the user turns it off (D3).
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::config_dir;
use crate::config::instance_artifacts::UNLEASHED_REPLICAS_FILE_NAME;
use crate::config::local_config_io::update_config_json_object;
use crate::filesystem::unleashed_replicas as unleash_io;
use crate::path_utils::normalize_windows_verbatim_path;

// A replica directory name starts with this prefix; a Root Agent directory never does.
const REPLICA_DIR_PREFIX: &str = "__agent_";
// Store file format version: `{"schemaVersion": 1, "replicas": [...]}`.
const SCHEMA_VERSION: u64 = 1;

// Comparable key for a replica root: forward slashes, no trailing slash, and lowercase
// for Windows-shaped paths (same shape as the frontend `normalizeProjectPathForCompare`).
pub(crate) fn replica_key(path: &str) -> String {
    let mut key = normalize_windows_verbatim_path(path).replace('\\', "/");
    while key.len() > 1 && key.ends_with('/') {
        key.pop();
    }
    let bytes = key.as_bytes();
    let windows_shaped = bytes.get(1) == Some(&b':') || key.starts_with("//");
    if windows_shaped {
        key.make_ascii_lowercase();
    }
    key
}

// True when the last path component is a replica directory.
fn is_replica_dir(path: &str) -> bool {
    replica_key(path)
        .rsplit('/')
        .next()
        .is_some_and(|last| last.starts_with(REPLICA_DIR_PREFIX))
}

// True when `root` may carry a flag in `store`: a replica, or a temp room of the store's
// directory (the store lives in the config dir, which is the temp-room base).
fn is_unleash_target(store: &Path, root: &str) -> bool {
    is_replica_dir(root)
        || store
            .parent()
            .is_some_and(|base| crate::config::temp_rooms::is_temp_room_path_in(root, base))
}

// The store file in the app config directory.
fn store_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join(UNLEASHED_REPLICAS_FILE_NAME))
}

// Reads the stored keys. A missing file is empty; any unreadable or invalid content logs
// one warning and reads as empty. Never rewrites the file.
pub(crate) fn load_from(path: &Path) -> Vec<String> {
    let text = match unleash_io::read_unleashed_store(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            log::warn!("[unleash] cannot read {}: {e}", path.display());
            return Vec::new();
        }
    };
    match parse_replicas(&text) {
        Ok(replicas) => replicas,
        Err(reason) => {
            log::warn!("[unleash] ignoring {}: {reason}", path.display());
            Vec::new()
        }
    }
}

// Parses the store document; any shape error is reported as a reason.
fn parse_replicas(text: &str) -> Result<Vec<String>, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    if root.get("schemaVersion").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err("wrong schemaVersion".to_string());
    }
    let Some(entries) = root.get("replicas").and_then(Value::as_array) else {
        return Err("replicas is not an array".to_string());
    };
    entries
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "non-string replica entry".to_string())
        })
        .collect()
}

// True when `replica_root` is a replica and its key is stored in `path`.
pub(crate) fn is_unleashed_in(path: &Path, replica_root: &str) -> bool {
    is_unleash_target(path, replica_root) && load_from(path).contains(&replica_key(replica_root))
}

// True when `replica_root` is an unleashed replica; false without a config directory.
pub(crate) fn is_unleashed(replica_root: &str) -> bool {
    store_path().is_some_and(|path| is_unleashed_in(&path, replica_root))
}

// Inserts or removes the replica key in `path` (locked read-modify-write, atomic publish).
pub(crate) fn set_unleashed_in(
    path: &Path,
    replica_root: &str,
    unleashed: bool,
) -> Result<(), String> {
    if !is_unleash_target(path, replica_root) {
        return Err("unleashNotAReplica".to_string());
    }
    let key = replica_key(replica_root);
    update_config_json_object(path, true, |obj| {
        let mut replicas: Vec<String> = obj
            .get("replicas")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        replicas.retain(|entry| entry != &key);
        if unleashed {
            replicas.push(key);
        }
        replicas.sort();
        replicas.dedup();
        obj.insert("schemaVersion".to_string(), Value::from(SCHEMA_VERSION));
        obj.insert(
            "replicas".to_string(),
            Value::Array(replicas.into_iter().map(Value::String).collect()),
        );
        Ok(())
    })
    .map(|_| ())
}

// Sets the flag in the app config directory store.
pub(crate) fn set_unleashed(replica_root: &str, unleashed: bool) -> Result<(), String> {
    let Some(path) = store_path() else {
        return Err("unleashNoConfigDir".to_string());
    };
    set_unleashed_in(&path, replica_root, unleashed)
}

// #4752 P07: removes every key at or under `root` from `path`; returns how many. A
// missing store stays absent. No replica-shape check (a room dir is a valid root).
pub(crate) fn forget_under_in(path: &Path, root: &str) -> Result<usize, String> {
    let key = replica_key(root);
    if key.is_empty() || key == "/" {
        return Ok(0);
    }
    if let Err(e) = unleash_io::read_unleashed_store(path) {
        if e.kind() == std::io::ErrorKind::NotFound {
            return Ok(0);
        }
    }
    let prefix = format!("{key}/");
    let mut removed = 0;
    update_config_json_object(path, true, |obj| {
        let entries: Vec<String> = obj
            .get("replicas")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let before = entries.len();
        let kept: Vec<String> = entries
            .into_iter()
            .filter(|entry| entry != &key && !entry.starts_with(&prefix))
            .collect();
        removed = before - kept.len();
        obj.insert("schemaVersion".to_string(), Value::from(SCHEMA_VERSION));
        obj.insert(
            "replicas".to_string(),
            Value::Array(kept.into_iter().map(Value::String).collect()),
        );
        Ok(())
    })?;
    Ok(removed)
}

// #4752 P07: drops the flags under a deleted room or replica. Never fails the delete.
pub(crate) fn forget_under(root: &Path) {
    let Some(store) = store_path() else {
        return;
    };
    match forget_under_in(&store, &root.to_string_lossy()) {
        Ok(n) if n > 0 => {
            log::info!(
                "[unleash] #4752 removed {n} flag(s) under {}",
                root.display()
            );
        }
        Ok(_) => {}
        Err(e) => log::warn!(
            "[unleash] #4752 cannot clear flags under {}: {e}",
            root.display()
        ),
    }
}

#[cfg(test)]
mod tests;
