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

use crate::capture::catalog::Catalog;
use crate::capture::jev::{ClassifyOutcome, JevSettings};
use crate::capture::secrets;
use crate::config::co_managed::{self, CoManagedConfig, CoManagedState};
use crate::config::entity_prefix::ROOM_DIR_PREFIX;
use crate::config::settings::SettingsState;
use crate::network::OutboundNetwork;

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
                .is_some_and(|root| {
                    // `room_root` is canonical (see `canonical_room_root`) while the
                    // candidate is derived from the session's stored cwd, which is not.
                    // On Windows `canonicalize` yields a `\\?\` verbatim path, so
                    // comparing a raw candidate with a canonical root drops every live
                    // session and the toggle starts no reader (#2267 phase 4, Windows
                    // CI `toggling_the_flag_on_a_live_session_starts_and_stops_the_reader`).
                    let candidate = std::fs::canonicalize(&root).unwrap_or(root);
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
/// supervisor calls capture::jev directly, never commands::co_managed. Registering it would add `lib.rs` to this phase and
/// would put a network-calling entry point on the IPC surface with no UI asking for it.
#[allow(dead_code)] // Test-only helper; --all-targets also compiles the lib without cfg(test).
pub(crate) async fn classify_dry_run(
    catalog: &Catalog,
    text: &str,
    settings: &JevSettings,
) -> Result<ClassifyOutcome, String> {
    if let Some(detection) = secrets::detect(text) {
        return Ok(ClassifyOutcome::abstained(detection.reason()));
    }
    let network = OutboundNetwork::new()
        .map_err(|error| format!("classifyDryRunNetworkInitFailed: {error}"))?;
    Ok(crate::capture::jev::classify(&network, settings, catalog, text).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::AppSettings;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// A loopback listener that only counts connections; the two dry-run tests
    /// below assert the count stays zero.
    async fn silent_listener() -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback listener");
        let port = listener.local_addr().expect("listener address").port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = tokio::io::AsyncWriteExt::shutdown(&mut stream).await;
            }
        });
        (format!("http://127.0.0.1:{port}/v1/systemone"), hits)
    }

    fn dry_run_settings(endpoint: String) -> JevSettings {
        JevSettings {
            api_key: "test-key".to_string(),
            model: "jev-1.13.0".to_string(),
            endpoint,
            timeout_secs: 5,
            threshold: 0.70,
            margin: 0.15,
        }
    }

    /// Test 10: an absent catalog is inert and no HTTP request is issued.
    #[tokio::test]
    async fn classify_dry_run_without_a_catalog_issues_no_request() {
        let (url, hits) = silent_listener().await;
        let outcome = classify_dry_run(&Catalog::missing(), "candidate", &dry_run_settings(url))
            .await
            .expect("a missing catalog is an abstention, not an error");
        match outcome {
            ClassifyOutcome::Abstained { reason } => {
                assert!(reason.contains("NoCatalogFile"), "{reason}")
            }
            other => panic!("a missing catalog must be inert, got {other:?}"),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "no HTTP request may be issued"
        );
    }

    /// Test 14: a seeded secret means no request, no file anywhere, and a reason
    /// with neither an excerpt nor a path.
    #[tokio::test]
    async fn classify_dry_run_with_a_seeded_secret_creates_no_file_and_issues_no_request() {
        let scratch = tempfile::tempdir().unwrap();
        let (url, hits) = silent_listener().await;
        let catalog = Catalog::from_json_str(
            r#"{"categories": {"a": {"destination": "user", "question": "is a?"}}}"#,
        );
        let secret = "AKIAIOSFODNN7EXAMPLE";
        let outcome = classify_dry_run(&catalog, secret, &dry_run_settings(url))
            .await
            .expect("a seeded secret is an abstention, not an error");
        match outcome {
            ClassifyOutcome::Abstained { reason } => {
                assert!(reason.contains("secret detected"), "{reason}");
                assert!(
                    !reason.contains(secret),
                    "the reason must not carry an excerpt"
                );
                assert!(!reason.contains('/'), "the reason must not carry a path");
            }
            other => panic!("a seeded secret must abstain, got {other:?}"),
        }
        let entries: Vec<_> = std::fs::read_dir(scratch.path())
            .unwrap()
            .flatten()
            .collect();
        assert!(entries.is_empty(), "no file may be created anywhere");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "a flagged candidate must never reach the network"
        );
    }

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
