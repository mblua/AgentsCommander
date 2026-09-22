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

/// The single answer phase 2 exists to provide. Check order is fixed so the
/// reason a user sees is stable:
/// `NotAnOrchestrator` -> `UnsupportedProvider` -> `RoomFlagOff` -> `NoApiKey`
/// -> `NoCatalogFile` -> `CatalogUnreadable`.
///
/// Every SCC-owned input arrives as a parameter; this function reads only the
/// room's own files. There is no third value: absence is a typed state.
pub fn effective_state(
    room_root: &Path,
    api_key: &str,
    is_orchestrator: bool,
    capture_supported: bool,
    agent_label: &str,
) -> CoManagedState {
    if !is_orchestrator {
        return CoManagedState::Off {
            reason: OffReason::NotAnOrchestrator,
        };
    }
    if !capture_supported {
        return CoManagedState::Off {
            reason: OffReason::UnsupportedProvider {
                agent: agent_label.to_string(),
            },
        };
    }
    let config = load_config(room_root);
    if !config.enabled {
        return CoManagedState::Off {
            reason: OffReason::RoomFlagOff,
        };
    }
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
mod tests {
    use super::*;

    fn room_dir(temp: &tempfile::TempDir) -> PathBuf {
        let room = temp.path().join("room-1-dev-team");
        std::fs::create_dir_all(&room).expect("room dir");
        room
    }

    fn write_config(room: &Path, raw: &str) {
        let dir = co_managed_dir(room);
        std::fs::create_dir_all(&dir).expect("co-managed dir");
        std::fs::write(config_path(room), raw).expect("config.json");
    }

    fn write_catalog(room: &Path, name: &str, raw: &str) -> PathBuf {
        let path = room.join(name);
        std::fs::write(&path, raw).expect("catalog");
        path
    }

    /// Test 1: an absent `.co-managed/` directory is `RoomFlagOff`, creates nothing.
    #[test]
    fn absent_directory_is_flag_off_and_creates_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        let state = effective_state(&room, "key", true, true, "claude");
        assert_eq!(
            state,
            CoManagedState::Off {
                reason: OffReason::RoomFlagOff
            }
        );
        assert!(!co_managed_dir(&room).exists());
    }

    /// Test 2: enabled with an empty API key is `NoApiKey`.
    #[test]
    fn enabled_with_empty_key_is_no_api_key() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        write_config(&room, r#"{"enabled":true,"catalogPath":null}"#);
        assert_eq!(
            effective_state(&room, "", true, true, "claude"),
            CoManagedState::Off {
                reason: OffReason::NoApiKey
            }
        );
    }

    /// Test 3: enabled, key present, null catalog path is `NoCatalogFile`.
    #[test]
    fn enabled_without_catalog_path_is_no_catalog_file() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        write_config(&room, r#"{"enabled":true,"catalogPath":null}"#);
        assert_eq!(
            effective_state(&room, "key", true, true, "claude"),
            CoManagedState::Off {
                reason: OffReason::NoCatalogFile
            }
        );
    }

    /// Test 4: fully configured and parseable is `Ready`.
    #[test]
    fn fully_configured_room_is_ready() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        write_catalog(&room, "catalog.json", "{}");
        write_config(&room, r#"{"enabled":true,"catalogPath":"catalog.json"}"#);
        assert_eq!(
            effective_state(&room, "key", true, true, "claude"),
            CoManagedState::Ready
        );
    }

    /// A catalog that exists but does not parse is `CatalogUnreadable`.
    #[test]
    fn unparseable_catalog_is_catalog_unreadable() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        write_catalog(&room, "catalog.json", "not json at all");
        write_config(&room, r#"{"enabled":true,"catalogPath":"catalog.json"}"#);
        assert_eq!(
            effective_state(&room, "key", true, true, "claude"),
            CoManagedState::Off {
                reason: OffReason::CatalogUnreadable
            }
        );
    }

    /// A missing catalog is `NoCatalogFile`, not `CatalogUnreadable`.
    #[test]
    fn missing_catalog_is_no_catalog_file() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        write_config(
            &room,
            r#"{"enabled":true,"catalogPath":"missing-catalog.json"}"#,
        );
        assert_eq!(
            effective_state(&room, "key", true, true, "claude"),
            CoManagedState::Off {
                reason: OffReason::NoCatalogFile
            }
        );
    }

    /// Test 5: a non-orchestrator is rejected even when everything else is set.
    #[test]
    fn non_orchestrator_is_rejected_even_when_configured() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        write_catalog(&room, "catalog.json", "{}");
        write_config(&room, r#"{"enabled":true,"catalogPath":"catalog.json"}"#);
        assert_eq!(
            effective_state(&room, "key", false, true, "claude"),
            CoManagedState::Off {
                reason: OffReason::NotAnOrchestrator
            }
        );
    }

    /// Test 6: unknown keys survive `set_enabled`.
    #[test]
    fn set_enabled_preserves_unknown_keys() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        write_config(&room, r#"{"enabled":true,"futureKey":42}"#);
        let config = set_enabled(&room, false).expect("write succeeds");
        assert!(!config.enabled);

        let raw = std::fs::read_to_string(config_path(&room)).expect("config.json");
        let value: Value = serde_json::from_str(&raw).expect("valid json");
        assert_eq!(value[ENABLED_KEY], Value::Bool(false));
        assert_eq!(value["futureKey"], serde_json::json!(42));
    }

    /// Test 7: malformed JSON loads as defaults instead of an error.
    #[test]
    fn malformed_config_loads_as_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        write_config(&room, "not json at all");
        let config = load_config(&room);
        assert!(!config.enabled);
        assert_eq!(config.catalog_path, None);
    }

    /// Test 10: two concurrent writers serialise through the lock and never
    /// leave a truncated file.
    #[test]
    fn concurrent_set_enabled_serialises_through_the_lock() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        write_config(&room, r#"{"enabled":false,"futureKey":42}"#);

        let first_room = room.clone();
        let second_room = room.clone();
        let first = std::thread::spawn(move || set_enabled(&first_room, true));
        let second = std::thread::spawn(move || set_enabled(&second_room, false));
        let first = first.join().expect("first thread").expect("first write");
        let second = second.join().expect("second thread").expect("second write");

        let raw = std::fs::read_to_string(config_path(&room)).expect("config.json");
        let value: Value = serde_json::from_str(&raw).expect("never a truncated file");
        let final_enabled = value[ENABLED_KEY].as_bool().expect("boolean");
        assert!(
            final_enabled == first.enabled || final_enabled == second.enabled,
            "the survivor must be exactly one of the two written values"
        );
        assert_eq!(value["futureKey"], serde_json::json!(42));
    }

    /// Test 11: an unsupported provider reports the offending agent label.
    #[test]
    fn unsupported_provider_carries_the_agent_label() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        write_catalog(&room, "catalog.json", "{}");
        write_config(&room, r#"{"enabled":true,"catalogPath":"catalog.json"}"#);
        assert_eq!(
            effective_state(&room, "key", true, false, "pi"),
            CoManagedState::Off {
                reason: OffReason::UnsupportedProvider {
                    agent: "pi".to_string()
                }
            }
        );
    }

    /// Test 12: the fixed check order wins ties.
    #[test]
    fn non_orchestrator_wins_over_unsupported_provider() {
        let temp = tempfile::tempdir().unwrap();
        let room = room_dir(&temp);
        assert_eq!(
            effective_state(&room, "", false, false, "pi"),
            CoManagedState::Off {
                reason: OffReason::NotAnOrchestrator
            }
        );
    }

    /// Test 16: the wire shape is exactly the three literals phase 9 codes against.
    #[test]
    fn wire_shape_is_exactly_the_pinned_literals() {
        let ready = CoManagedState::Ready;
        let flag_off = CoManagedState::Off {
            reason: OffReason::RoomFlagOff,
        };
        let unsupported = CoManagedState::Off {
            reason: OffReason::UnsupportedProvider {
                agent: "pi".to_string(),
            },
        };

        assert_eq!(serde_json::to_string(&ready).unwrap(), "\"Ready\"");
        assert_eq!(
            serde_json::to_string(&flag_off).unwrap(),
            "{\"Off\":{\"reason\":\"RoomFlagOff\"}}"
        );
        assert_eq!(
            serde_json::to_string(&unsupported).unwrap(),
            "{\"Off\":{\"reason\":{\"UnsupportedProvider\":{\"agent\":\"pi\"}}}}"
        );

        for state in [ready, flag_off, unsupported] {
            let json = serde_json::to_string(&state).unwrap();
            let back: CoManagedState = serde_json::from_str(&json).unwrap();
            assert_eq!(back, state);
        }
    }

    /// `CoManagedConfig` keeps its camelCase wire names.
    #[test]
    fn config_wire_shape_is_camel_case() {
        let config = CoManagedConfig {
            enabled: true,
            catalog_path: Some("catalog.json".to_string()),
        };
        assert_eq!(
            serde_json::to_string(&config).unwrap(),
            "{\"enabled\":true,\"catalogPath\":\"catalog.json\"}"
        );
        let default = serde_json::to_string(&CoManagedConfig::default()).unwrap();
        assert_eq!(default, "{\"enabled\":false,\"catalogPath\":null}");
    }

    /// #2232 phase 7: the queue is a sibling of `config.json` and `state.json`,
    /// directly under `.co-managed/`, never a subdirectory of an outbox.
    #[test]
    fn queue_dir_is_a_sibling_of_the_config_and_state_files() {
        let room = Path::new("/tmp/proj-a/.ac/room-1-dev-team");
        let queue = queue_dir(room);
        assert_eq!(queue, room.join(".co-managed").join("queue"));
        assert_eq!(queue.parent(), Some(co_managed_dir(room).as_path()));
        assert_eq!(queue.parent(), config_path(room).parent());
        assert_eq!(queue.parent(), lock_path(room).parent());
    }

    /// The room-root walk mirrors the messaging rule (`room-<digits>-*`).
    #[test]
    fn room_root_walk_matches_the_messaging_rule() {
        let temp = tempfile::tempdir().unwrap();
        let room = temp
            .path()
            .join("project-a")
            .join(".ac")
            .join("room-7-dev-team");
        let replica = room.join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).unwrap();

        assert_eq!(room_root_for_path(&replica), Some(room.clone()));
        assert_eq!(room_root_for_path(&room), Some(room.clone()));
        assert_eq!(
            room_root_for_path(&temp.path().join("project-a")),
            None,
            "a project dir is not a room root"
        );
        assert_eq!(
            room_root_for_path(&temp.path().join("room-notes")),
            None,
            "the numeric run is required"
        );
    }
}
