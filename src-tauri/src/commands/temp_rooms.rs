//! Temp-room IPC (#4779): list, create and delete `temp-room-trNN` folders in the
//! app config dir. The seams take the base dir explicitly; the commands only
//! resolve `config_dir()` and delegate.

use crate::commands::entity_creation::{
    ensure_no_live_sessions_under_manager, try_atomic_delete_wg, WgDeleteOutcome,
};
use crate::config::temp_rooms::{parse_temp_room_number, temp_room_dir_name};
use crate::filesystem::temp_rooms::{create_dir_exclusive, list_child_entries};
use crate::session::manager::SessionManager;
use serde::Serialize;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::State;

const CREATE_ATTEMPTS: usize = 64;
const TEMP_ROOM_COMMANDS: [&str; 3] = ["list_temp_rooms", "create_temp_room", "delete_temp_room"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TempRoomInfo {
    name: String,
    path: String,
}

fn info(base: &Path, name: String) -> TempRoomInfo {
    let dir = base.join(&name);
    TempRoomInfo {
        name,
        path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(&dir),
    }
}

/// Lowest n >= 1 not taken by a canonical temp-room name; freed numbers are reused.
fn lowest_free_temp_room_number<'a>(names: impl IntoIterator<Item = &'a str>) -> u32 {
    let taken: HashSet<u32> = names
        .into_iter()
        .filter_map(parse_temp_room_number)
        .collect();
    (1..).find(|n| !taken.contains(n)).unwrap_or(1)
}

fn temp_rooms_base() -> Result<PathBuf, String> {
    crate::config::config_dir().ok_or_else(|| "Could not resolve app config directory".to_string())
}

/// Real temp-room folders of `base`, sorted by number.
pub(crate) fn list_temp_rooms_in(base: &Path) -> Result<Vec<TempRoomInfo>, String> {
    let mut rooms: Vec<(u32, String)> = list_child_entries(base)?
        .into_iter()
        .filter(|entry| entry.real_dir)
        .filter_map(|entry| parse_temp_room_number(&entry.name).map(|n| (n, entry.name)))
        .collect();
    rooms.sort_by_key(|(n, _)| *n);
    Ok(rooms
        .into_iter()
        .map(|(_, name)| info(base, name))
        .collect())
}

/// Create the lowest free temp room. Every child name (dir, file or link) takes
/// its number; an `AlreadyExists` race marks the number taken and retries.
pub(crate) fn create_next_with(
    base: &Path,
    mut create: impl FnMut(&Path) -> io::Result<()>,
) -> Result<TempRoomInfo, String> {
    let mut taken: HashSet<u32> = HashSet::new();
    for _ in 0..CREATE_ATTEMPTS {
        let listed: Vec<String> = list_child_entries(base)?
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        let reserved: Vec<String> = taken.iter().map(|n| temp_room_dir_name(*n)).collect();
        let n =
            lowest_free_temp_room_number(listed.iter().chain(reserved.iter()).map(String::as_str));
        let name = temp_room_dir_name(n);
        match create(&base.join(&name)) {
            Ok(()) => return Ok(info(base, name)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                taken.insert(n);
            }
            Err(e) => return Err(format!("Failed to create temp room folder: {e}")),
        }
    }
    Err("Could not create a temp room folder: too many name collisions".to_string())
}

fn delete_outcome_result(outcome: WgDeleteOutcome) -> Result<(), String> {
    match outcome {
        WgDeleteOutcome::Deleted => Ok(()),
        WgDeleteOutcome::Partial { orphan_path, error } => Err(format!(
            "Partial temp room delete: orphan '{}' could not be removed: {error}",
            orphan_path.display()
        )),
        WgDeleteOutcome::Blocked(e) => Err(format!("Temp room folder is in use: {e}")),
        WgDeleteOutcome::Other(e) => Err(format!("Failed to delete temp room folder: {e}")),
    }
}

/// Delete `base/name` when it is a real temp-room folder with no live session inside.
pub(crate) async fn delete_temp_room_in(
    base: &Path,
    name: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    if parse_temp_room_number(name).is_none() {
        return Err("Invalid temp room name".to_string());
    }
    let dir = base.join(name);
    crate::filesystem::projects::entities::deletion::validate_delete_root(&dir)?;
    ensure_no_live_sessions_under_manager(&dir, session_mgr).await?;
    let outcome =
        crate::config::seed_manifest::run_blocking_owned(move || try_atomic_delete_wg(&dir))
            .await
            .map_err(|e| format!("Temp room delete failed to run: {e}"))?;
    delete_outcome_result(outcome)
}

#[tauri::command]
pub(crate) async fn list_temp_rooms() -> Result<Vec<TempRoomInfo>, String> {
    list_temp_rooms_in(&temp_rooms_base()?)
}

#[tauri::command]
pub(crate) async fn create_temp_room() -> Result<TempRoomInfo, String> {
    let base = temp_rooms_base()?;
    tokio::task::spawn_blocking(move || create_next_with(&base, create_dir_exclusive))
        .await
        .map_err(|e| format!("Temp room create failed to run: {e}"))?
}

#[tauri::command]
pub(crate) async fn delete_temp_room(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    name: String,
) -> Result<(), String> {
    delete_temp_room_in(&temp_rooms_base()?, &name, session_mgr.inner()).await
}

/// Route the three temp-room commands to their handler; every other command
/// goes to `fallback` exactly once and keeps its result.
pub(crate) fn dispatch<R, F>(invoke: tauri::ipc::Invoke<R>, fallback: &F) -> bool
where
    R: tauri::Runtime,
    F: Fn(tauri::ipc::Invoke<R>) -> bool,
{
    let temp_rooms: fn(tauri::ipc::Invoke<R>) -> bool =
        tauri::generate_handler![list_temp_rooms, create_temp_room, delete_temp_room];
    match invoke.message.command() {
        name if TEMP_ROOM_COMMANDS.contains(&name) => temp_rooms(invoke),
        _ => fallback(invoke),
    }
}

#[cfg(test)]
mod tests;
