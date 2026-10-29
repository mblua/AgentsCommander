//! Tauri commands for the per-room Co-managed opt-in (#2265, epic #2232).
//!
//! Thin wrappers only, by design: the on-disk contract lives in
//! `config::co_managed` (a leaf outside the 88-member SCC) and the SCC-owned
//! gathering work lives in `commands::session`. This module must keep **zero
//! incoming arcs** from SCC members; the only callers are the wrappers here,
//! `generate_handler!` (invisible to the module-arc instrument) and the
//! phase-9 frontend. See phase 2 section 4.1.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::capture::catalog::Catalog;
use crate::capture::jev::{ClassifyOutcome, JevSettings};
use crate::capture::secrets;
use crate::config::co_managed::{self, CoManagedConfig, CoManagedState};
use crate::config::entity_prefix::ROOM_DIR_PREFIX;
use crate::config::settings::SettingsState;
use crate::filesystem::projects::{canonical_project_path, project_path_is_dir};
use crate::network::OutboundNetwork;

/// Canonicalise `room_root` and prove it is a Room of a registered project
/// before any read or write. A path failing either check returns an error and
/// touches nothing, so a caller cannot make AC create `.co-managed/` in an
/// arbitrary location (phase 2 section 7.3).
async fn canonical_room_root<R: tauri::Runtime>(
    app: &AppHandle<R>,
    room_root: &str,
) -> Result<PathBuf, String> {
    let canonical = canonical_project_path(Path::new(room_root))
        .map_err(|e| format!("coManagedRoomRootInvalid: {room_root}: {e}"))?;
    if !project_path_is_dir(&canonical) {
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
        let guard = settings
            .current_read()
            .await
            .map_err(|error| error.to_string())?;
        guard.project_paths.clone()
    };
    let candidates =
        crate::config::projects::enumerate_registered_project_candidates(&project_paths);
    let registered = candidates.iter().any(|candidate| {
        let candidate_ac = candidate
            .path
            .join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
        canonical_project_path(&candidate_ac)
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

    let mut toggled = 0_usize;
    for session_id in live_sessions_in_room(&app, &root).await {
        toggled += 1;
        if enabled {
            crate::commands::session::raise_room_reader_demand_in(&app, &root, session_id).await;
        } else {
            crate::commands::session::release_room_reader_demand(&app, session_id).await;
        }
    }
    log::info!(
        "[co-managed] toggle: room {} enabled={enabled}; reader demand {} \
         for {toggled} live session(s)",
        root.display(),
        if enabled { "raised" } else { "released" },
    );

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
                .is_some_and(|root| {
                    // `room_root` is canonical (see `canonical_room_root`) while the
                    // candidate is derived from the session's stored cwd, which is not.
                    // On Windows `canonicalize` yields a `\\?\` verbatim path, so
                    // comparing a raw candidate with a canonical root drops every live
                    // session and the toggle starts no reader (#2267 phase 4, Windows
                    // CI `toggling_the_flag_on_a_live_session_starts_and_stops_the_reader`).
                    let candidate = canonical_project_path(&root).unwrap_or(root);
                    crate::path_identity::paths_equivalent(&candidate, room_root)
                })
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

/// Phase-6 dry run: the pre-egress detector, then the single Jev call, with no
/// side effects.
///
/// A seeded secret returns before the network client is even built, so nothing
/// is written, nothing is enqueued and no request is issued (plan section 7);
/// the outcome carries only the detector's reason and the candidate length.
///
/// #2232 phase 6: NOT a #[tauri::command]. This phase's tests call it. The phase-7
/// supervisor calls capture::jev directly, never commands::co_managed. Registering it
/// would add `lib.rs` to this phase and
/// would put a network-calling entry point on the IPC surface with no UI asking for it.
#[allow(dead_code)] // Test-only helper; --all-targets also compiles the lib without cfg(test).
pub(crate) async fn classify_dry_run(
    catalog: &Catalog,
    text: &str,
    settings: &JevSettings,
    owner: &secrets::SecretRules,
) -> Result<ClassifyOutcome, String> {
    if let Some(detection) = secrets::detect(owner, text) {
        return Ok(ClassifyOutcome::abstained(detection.reason()));
    }
    let network = OutboundNetwork::new()
        .map_err(|error| format!("classifyDryRunNetworkInitFailed: {error}"))?;
    Ok(crate::capture::jev::classify(&network, settings, catalog, text, "dry-run").await)
}

#[cfg(test)]
mod tests;
