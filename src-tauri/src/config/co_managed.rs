//! Per-room Co-managed configuration and effective state (#2265, epic #2232).
//!
//! This module is a deliberate **leaf**: it must stay outside the 88-member
//! module cycle recorded in `src-tauri/module-arcs.txt`. It therefore depends
//! only on [`crate::config::ac_root`]-class leaves ([`crate::config::entity_prefix`]),
//! `serde`, `serde_json` and `std`. Every SCC-owned value the effective state
//! needs (the API key, the orchestrator flag, the capture-support flag) arrives
//! as a **parameter**; the reading is done by the caller
//! (the gathering function `co_managed_effective_state_for_session`, which
//! lives in the session command module and is already an SCC member). Do not
//! reference the settings, teams, phone, session or command modules from this
//! file: phase 2 acceptance criterion 7 greps for exactly those paths here.
//!
//! On-disk contract, under `<room-root>/.co-managed/`:
//!
//! - `config.json`: `{ "enabled": false, "catalogPath": null }`. Unknown keys
//!   are preserved across rewrites so a newer build's key survives an older
//!   build's save. A malformed file is repaired to defaults on load, never
//!   fatally rejected (the `normalize_groups_config` convention in
//!   `config/project_settings.rs`).
//! - `lock`: one advisory lock file governing `config.json` and the `state.json`
//!   phase 3 adds. One lock for both; phase 3 does not add a second.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Directory holding the per-room Co-managed files, directly under the Room root.
pub const CO_MANAGED_DIR_NAME: &str = ".co-managed";
/// The opt-in flag file. Unknown keys survive a rewrite.
pub const CONFIG_FILE_NAME: &str = "config.json";
/// Advisory lock file governing `config.json` (and the phase-3 `state.json`).
pub const LOCK_FILE_NAME: &str = "lock";
/// The phase-7 provenance queue directory, directly under `.co-managed/`.
pub const QUEUE_DIR_NAME: &str = "queue";

const ENABLED_KEY: &str = "enabled";
const CATALOG_PATH_KEY: &str = "catalogPath";

/// Whole acquire deadline for the advisory lock, per phase 2 section 11: a lock
/// held longer than this fails fast instead of blocking the caller forever.
const LOCK_WAIT_BUDGET: Duration = Duration::from_secs(2);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The per-room opt-in record. `enabled` is the flag; `catalogPath` is the
/// optional catalog, relative to the room root when relative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CoManagedConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub catalog_path: Option<String>,
}

/// "Is Co-managed effective for this room right now, and if not, why not".
///
/// Both enums use serde's **default external tagging** and **no** `rename` /
/// `rename_all`, so the wire shape is exactly:
/// `"Ready"`, `{"Off":{"reason":"RoomFlagOff"}}` and
/// `{"Off":{"reason":{"UnsupportedProvider":{"agent":"pi"}}}}`.
/// Phase 9 encodes its TypeScript types against these literals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoManagedState {
    Off { reason: OffReason },
    Ready,
}

/// The single reason a room is not effective. Variant names are the wire form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OffReason {
    GlobalSwitchOff,
    NotAnOrchestrator,
    UnsupportedProvider { agent: String },
    RoomFlagOff,
    NoApiKey,
    NoCatalogFile,
    CatalogUnreadable,
}

/// Walk up from `path` to the nearest Room root, `room-<N>-*`.
///
/// This mirrors the rule the messaging module's `workgroup_root` uses,
/// reimplemented here because that function is an SCC member and this module may
/// not call it. The `<N>` run is decimal digits, exactly as `is_wg_dir` requires.
pub fn room_root_for_path(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| {
            ancestor
                .file_name()
                .and_then(|name| name.to_str())
                .map(is_room_dir_name)
                .unwrap_or(false)
        })
        .map(Path::to_path_buf)
}

fn is_room_dir_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(crate::config::entity_prefix::ROOM_DIR_PREFIX) else {
        return false;
    };
    let Some((digits, _)) = rest.split_once('-') else {
        return false;
    };
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// `<room-root>/.co-managed`.
pub fn co_managed_dir(room_root: &Path) -> PathBuf {
    room_root.join(CO_MANAGED_DIR_NAME)
}

/// `<room-root>/.co-managed/config.json`.
pub fn config_path(room_root: &Path) -> PathBuf {
    co_managed_dir(room_root).join(CONFIG_FILE_NAME)
}

/// `<room-root>/.co-managed/lock`.
pub fn lock_path(room_root: &Path) -> PathBuf {
    co_managed_dir(room_root).join(LOCK_FILE_NAME)
}

/// `<room-root>/.co-managed/queue` (phase 7).
///
/// A **sibling** of `config.json` and `state.json`, never a subdirectory of any
/// outbox: the mailbox sweep classifies a message's origin by directory and
/// skips subdirectories. The phase-7 supervisor writes here and nothing else
/// may; `cli::send` rejects this path as an `--outbox` target.
pub fn queue_dir(room_root: &Path) -> PathBuf {
    co_managed_dir(room_root).join(QUEUE_DIR_NAME)
}

/// Read the config, repairing anything malformed to defaults.
///
/// A missing directory, a missing file, unreadable bytes, invalid JSON and
/// wrong-typed fields all yield [`CoManagedConfig::default`]; a bad hand edit
/// must never be able to break the room. Never creates anything on disk.
pub fn load_config(room_root: &Path) -> CoManagedConfig {
    let path = config_path(room_root);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return CoManagedConfig::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return CoManagedConfig::default();
    };
    config_from_value(&value)
}

fn config_from_value(value: &Value) -> CoManagedConfig {
    CoManagedConfig {
        enabled: value
            .get(ENABLED_KEY)
            .and_then(Value::as_bool)
            .unwrap_or(false),
        catalog_path: value
            .get(CATALOG_PATH_KEY)
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Persist `enabled`, preserving every other key in `config.json` verbatim.
///
/// Read-modify-write under the advisory lock; the publish is a temp file in the
/// same directory followed by a rename, so a crash mid-write cannot produce a
/// half file. Creates `.co-managed/` lazily on first write.
pub fn set_enabled(room_root: &Path, enabled: bool) -> Result<CoManagedConfig, String> {
    let dir = co_managed_dir(room_root);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("coManagedDirCreateFailed: {}: {}", dir.display(), e))?;
    let _lock = acquire_lock(&dir)?;

    let path = config_path(room_root);
    let mut object = read_config_object(&path);
    object.insert(ENABLED_KEY.to_string(), Value::Bool(enabled));
    write_config_atomic(&dir, &path, &Value::Object(object))?;
    Ok(load_config(room_root))
}

fn read_config_object(path: &Path) -> Map<String, Value> {
    match std::fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Object(object)) => object,
            _ => Map::new(),
        },
        Err(_) => Map::new(),
    }
}

/// #2525 whether Co-managed is available globally: the conjunction of the
/// negations of `effective_state`'s `GlobalSwitchOff` and `NoApiKey` legs,
/// with the same `trim()` rule. `Ready` implies it; the converse is not
/// claimed, since a room can still be Off for a per-room reason.
pub fn globally_available(globally_enabled: bool, api_key: &str) -> bool {
    globally_enabled && !api_key.trim().is_empty()
}

/// #2756 U2a: whether Co-managed can run a reader globally. Only the switch;
/// the key is a `Ready` input, not a capture input. `globally_available`
/// keeps serving routing unchanged.
pub fn globally_capturable(globally_enabled: bool) -> bool {
    globally_enabled
}

/// #2756 U2a: whether this session may hold a reader and keep a candidate.
/// Exactly the first four checks of `effective_state`, from the same source;
/// it never reads the key or the catalog.
pub fn capturable(
    room_root: &Path,
    globally_enabled: bool,
    is_orchestrator: bool,
    capture_supported: bool,
) -> bool {
    capture_prefix(
        room_root,
        globally_enabled,
        is_orchestrator,
        capture_supported,
        "",
    )
    .is_ok()
}

/// The shared prefix of `capturable` and `effective_state`: switch,
/// orchestrator, provider, room flag. On success it hands back the config it
/// loaded so `effective_state` does not read it twice.
fn capture_prefix(
    room_root: &Path,
    globally_enabled: bool,
    is_orchestrator: bool,
    capture_supported: bool,
    agent_label: &str,
) -> Result<CoManagedConfig, OffReason> {
    if !globally_enabled {
        return Err(OffReason::GlobalSwitchOff);
    }
    if !is_orchestrator {
        return Err(OffReason::NotAnOrchestrator);
    }
    if !capture_supported {
        return Err(OffReason::UnsupportedProvider {
            agent: agent_label.to_string(),
        });
    }
    let config = load_config(room_root);
    if !config.enabled {
        return Err(OffReason::RoomFlagOff);
    }
    Ok(config)
}

/// The single answer phase 2 exists to provide. Check order is fixed so the
/// reason a user sees is stable:
/// `GlobalSwitchOff` -> `NotAnOrchestrator` -> `UnsupportedProvider` -> `RoomFlagOff` -> `NoApiKey`
/// -> `NoCatalogFile` -> `CatalogUnreadable`.
///
/// Every SCC-owned input arrives as a parameter; this function reads only the
/// room's own files. There is no third value: absence is a typed state.
pub fn effective_state(
    room_root: &Path,
    globally_enabled: bool,
    api_key: &str,
    is_orchestrator: bool,
    capture_supported: bool,
    agent_label: &str,
) -> CoManagedState {
    let config = match capture_prefix(
        room_root,
        globally_enabled,
        is_orchestrator,
        capture_supported,
        agent_label,
    ) {
        Ok(config) => config,
        Err(reason) => return CoManagedState::Off { reason },
    };
    if api_key.trim().is_empty() {
        return CoManagedState::Off {
            reason: OffReason::NoApiKey,
        };
    }
    let Some(catalog) = config.catalog_path.as_deref() else {
        return CoManagedState::Off {
            reason: OffReason::NoCatalogFile,
        };
    };
    let catalog_path = resolve_catalog_path(room_root, catalog);
    if !catalog_path.is_file() {
        return CoManagedState::Off {
            reason: OffReason::NoCatalogFile,
        };
    }
    match std::fs::read_to_string(&catalog_path) {
        Ok(raw) if serde_json::from_str::<Value>(&raw).is_ok() => CoManagedState::Ready,
        _ => CoManagedState::Off {
            reason: OffReason::CatalogUnreadable,
        },
    }
}

fn resolve_catalog_path(room_root: &Path, catalog: &str) -> PathBuf {
    let candidate = Path::new(catalog);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        room_root.join(candidate)
    }
}

/// A held advisory lock. The `File` IS the lock: dropping this guard closes the
/// handle and releases the OS lock. The lock file itself is left on disk.
struct CoManagedLock {
    _file: std::fs::File,
}

fn acquire_lock(dir: &Path) -> Result<CoManagedLock, String> {
    let path = dir.join(LOCK_FILE_NAME);
    // The lock file is a pure advisory token: it carries no content, and a
    // concurrent holder may have it open, so it is created if absent but never
    // truncated (`suspicious_open_options`).
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| format!("coManagedLockOpenFailed: {}: {}", path.display(), e))?;

    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(CoManagedLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() >= LOCK_WAIT_BUDGET => {
                return Err(format!(
                    "coManagedLockTimeout: waited {} ms for {}",
                    LOCK_WAIT_BUDGET.as_millis(),
                    path.display()
                ));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                let remaining = LOCK_WAIT_BUDGET.saturating_sub(started.elapsed());
                std::thread::sleep(LOCK_POLL_INTERVAL.min(remaining));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(format!("coManagedLockFailed: {}: {}", path.display(), e));
            }
        }
    }
}

fn write_config_atomic(dir: &Path, path: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| format!("coManagedSerializeFailed: {}: {}", path.display(), e))?;
    bytes.push(b'\n');

    let tmp_path = temp_path(dir);
    let write_result = (|| -> Result<(), String> {
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("coManagedTempCreateFailed: {}: {}", tmp_path.display(), e))?;
        file.write_all(&bytes)
            .map_err(|e| format!("coManagedTempWriteFailed: {}: {}", tmp_path.display(), e))?;
        file.flush()
            .map_err(|e| format!("coManagedTempFlushFailed: {}: {}", tmp_path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("coManagedTempSyncFailed: {}: {}", tmp_path.display(), e))
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    std::fs::rename(&tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!(
            "coManagedPublishFailed: {} -> {}: {}",
            tmp_path.display(),
            path.display(),
            e
        )
    })
}

fn temp_path(dir: &Path) -> PathBuf {
    dir.join(format!("{}.{}.tmp", CONFIG_FILE_NAME, std::process::id()))
}

#[cfg(test)]
mod tests;
