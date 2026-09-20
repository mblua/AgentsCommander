//! Tauri commands for the per-room Co-managed opt-in (#2265, epic #2232).
//!
//! Thin wrappers only, by design: the on-disk contract lives in
//! `config::co_managed` (a leaf outside the 88-member SCC) and the SCC-owned
//! gathering work lives in `commands::session`. This module must keep **zero
//! incoming arcs** from SCC members; the only callers are the wrappers here,
//! `generate_handler!` (invisible to the module-arc instrument) and the
//! phase-9 frontend. See phase 2 section 4.1.

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::config::co_managed::{self, CoManagedConfig, CoManagedState};
use crate::config::entity_prefix::ROOM_DIR_PREFIX;
use crate::config::settings::SettingsState;

/// Canonicalise `room_root` and prove it is a Room of a registered project
/// before any read or write. A path failing either check returns an error and
/// touches nothing, so a caller cannot make AC create `.co-managed/` in an
/// arbitrary location (phase 2 section 7.3).
async fn canonical_room_root<R: tauri::Runtime>(
    app: &AppHandle<R>,
    room_root: &str,
) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(room_root)
        .map_err(|e| format!("coManagedRoomRootInvalid: {room_root}: {e}"))?;
    if !canonical.is_dir() {
        return Err(format!(
            "coManagedRoomRootInvalid: {} is not a directory",
            canonical.display()
        ));
    }
    let Some(name) = canonical.file_name().and_then(|name| name.to_str()) else {
        return Err(format!(
            "coManagedRoomRootInvalid: {} has no usable file name",
            canonical.display()
        ));
    };
    if !name.starts_with(ROOM_DIR_PREFIX) {
        return Err(format!(
            "coManagedRoomRootInvalid: {} is not a {ROOM_DIR_PREFIX}* directory",
            canonical.display()
        ));
    }
    let Some(ac_root) = canonical.parent() else {
        return Err(format!(
            "coManagedRoomRootInvalid: {} has no parent directory",
            canonical.display()
        ));
    };

    let project_paths = {
        let settings = app.state::<SettingsState>();
        let guard = settings.read().await;
        guard.project_paths.clone()
    };
    let candidates =
        crate::config::projects::enumerate_registered_project_candidates(&project_paths);
    let registered = candidates.iter().any(|candidate| {
        let candidate_ac = candidate
            .path
            .join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
        std::fs::canonicalize(&candidate_ac)
            .map(|path| crate::path_identity::paths_equivalent(&path, ac_root))
            .unwrap_or(false)
    });
    if !registered {
        return Err(format!(
            "coManagedRoomRootUnregistered: {} is not a room of a registered project",
            canonical.display()
        ));
    }
    Ok(canonical)
}

/// Read the room's Co-managed config, repairing a malformed file to defaults.
#[tauri::command]
pub async fn co_managed_get<R: tauri::Runtime>(
    app: AppHandle<R>,
    room_root: String,
) -> Result<CoManagedConfig, String> {
    let root = canonical_room_root(&app, &room_root).await?;
    Ok(co_managed::load_config(&root))
}

/// Set the room's opt-in flag, preserving every other key in `config.json`.
///
/// The read-modify-write is a blocking, lock-bearing operation, so it runs on
/// the blocking pool rather than stalling the async runtime.
/// Toggling the flag on a **live** session must take effect immediately
/// (#2232 phase 4 section 5.1, epic section 3.2): without this, a user turning
/// the flag on for a running orchestrator would get no reader until the next
/// spawn. Every raise and release goes through the SCC-member functions in
/// `commands::session`, so this module keeps **outgoing arcs only**.
#[tauri::command]
pub async fn co_managed_set_enabled<R: tauri::Runtime>(
    app: AppHandle<R>,
    room_root: String,
    enabled: bool,
) -> Result<CoManagedConfig, String> {
    let root = canonical_room_root(&app, &room_root).await?;
    let write_root = root.clone();
    let config =
        tauri::async_runtime::spawn_blocking(move || co_managed::set_enabled(&write_root, enabled))
            .await
            .map_err(|e| format!("coManagedSetEnabledTaskFailed: {e}"))??;

    for session_id in live_sessions_in_room(&app, &root).await {
        if enabled {
            crate::commands::session::raise_room_reader_demand_in(&app, &root, session_id).await;
        } else {
            crate::commands::session::release_room_reader_demand(&app, session_id).await;
        }
    }

    Ok(config)
}

/// Every live session whose working directory sits inside `room_root`.
async fn live_sessions_in_room<R: tauri::Runtime>(
    app: &AppHandle<R>,
    room_root: &std::path::Path,
) -> Vec<uuid::Uuid> {
    let Some(manager) = app
        .try_state::<std::sync::Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>(
        )
    else {
        return Vec::new();
    };
    let sessions = {
        let guard = manager.read().await;
        guard.list_sessions().await
    };
    sessions
        .into_iter()
        .filter(|session| {
            co_managed::room_root_for_path(std::path::Path::new(&session.working_directory))
                .is_some_and(|root| crate::path_identity::paths_equivalent(&root, room_root))
        })
        .filter_map(|session| uuid::Uuid::parse_str(&session.id).ok())
        .collect()
}

/// Answer "is Co-managed effective for this room right now, and if not, why".
///
/// Delegates to the SCC-member gathering function in `commands::session`, which
/// reads the settings key and derives the orchestrator and capture-support
/// flags; this module never calls back into `config::settings` or
/// `config::teams` on its own behalf for the answer.
#[tauri::command]
pub async fn co_managed_effective_state<R: tauri::Runtime>(
    app: AppHandle<R>,
    room_root: String,
    session_id: String,
) -> Result<CoManagedState, String> {
    let root = canonical_room_root(&app, &room_root).await?;
    crate::commands::session::co_managed_effective_state_for_session(&app, &root, &session_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::AppSettings;

    fn test_app(project_paths: Vec<String>) -> tauri::App<tauri::test::MockRuntime> {
        let settings = AppSettings {
            project_paths,
            ..AppSettings::default()
        };
        tauri::test::mock_builder()
            .manage(std::sync::Arc::new(tokio::sync::RwLock::new(settings)))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build co-managed test app")
    }

    /// Test 13: a real directory that is not a `room-*` directory is rejected
    /// and nothing is created.
    #[tokio::test]
    async fn get_rejects_a_directory_that_is_not_a_room() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project-a");
        let ac_root = project.join(".ac");
        let notes = ac_root.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let app = test_app(vec![project.to_string_lossy().to_string()]);

        let error = co_managed_get(app.handle().clone(), notes.to_string_lossy().to_string())
            .await
            .expect_err("a non-room directory must be rejected");
        assert!(error.contains("coManagedRoomRootInvalid"), "{error}");

        let entries: Vec<_> = std::fs::read_dir(&notes).unwrap().flatten().collect();
        assert!(
            entries.is_empty(),
            "nothing may be created under a rejected directory"
        );
    }

    /// Test 14: a `room-*` directory outside every registered project is
    /// rejected and writes nothing.
    #[tokio::test]
    async fn set_enabled_rejects_a_room_outside_registered_projects() {
        let temp = tempfile::tempdir().unwrap();
        let registered = temp.path().join("registered-project");
        std::fs::create_dir_all(registered.join(".ac")).unwrap();
        let outside_room = temp
            .path()
            .join("outside-project")
            .join(".ac")
            .join("room-1-dev-team");
        std::fs::create_dir_all(&outside_room).unwrap();
        let app = test_app(vec![registered.to_string_lossy().to_string()]);

        let error = co_managed_set_enabled(
            app.handle().clone(),
            outside_room.to_string_lossy().to_string(),
            true,
        )
        .await
        .expect_err("an unregistered room must be rejected");
        assert!(error.contains("coManagedRoomRootUnregistered"), "{error}");

        let entries: Vec<_> = std::fs::read_dir(&outside_room)
            .unwrap()
            .flatten()
            .collect();
        assert!(
            entries.is_empty(),
            "nothing may be written under an unregistered room"
        );
    }

    /// The happy path: a registered room accepts the toggle and reads it back.
    #[tokio::test]
    async fn set_enabled_round_trips_for_a_registered_room() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project-a");
        let room = project.join(".ac").join("room-1-dev-team");
        std::fs::create_dir_all(&room).unwrap();
        let app = test_app(vec![project.to_string_lossy().to_string()]);
        let room_text = room.to_string_lossy().to_string();

        let enabled = co_managed_set_enabled(app.handle().clone(), room_text.clone(), true)
            .await
            .expect("registered room accepts the toggle");
        assert!(enabled.enabled);
        assert!(co_managed::config_path(&room).is_file());

        let fetched = co_managed_get(app.handle().clone(), room_text.clone())
            .await
            .expect("registered room reads back");
        assert_eq!(fetched, enabled);

        let disabled = co_managed_set_enabled(app.handle().clone(), room_text, false)
            .await
            .expect("toggle back");
        assert!(!disabled.enabled);
    }
}
