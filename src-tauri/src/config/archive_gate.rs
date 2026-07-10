use std::path::Path;
use std::time::Duration;

use tauri::Manager;

fn folder_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string()
}

fn archived_root_for_cwd(cwd: &str, archived_roots: &[String]) -> Option<String> {
    if archived_roots.is_empty() {
        return None;
    }
    let roots = crate::config::sessions_persistence::normalize_project_roots(archived_roots);
    crate::config::sessions_persistence::first_project_path_containing(cwd, &roots)
}

fn probe_spawn_refusal_for_archived_root(root: &str) -> Result<(), String> {
    if crate::config::workspace::has_workspace_dir(Path::new(root)) {
        return Ok(());
    }
    Err(format!(
        "Cannot start a session in archived project '{}': Project AC Root not found. Open Archived Projects to restore it, or remove it from the list.",
        folder_name(root)
    ))
}

fn auto_unarchive_registration(
    settings: &mut crate::config::settings::AppSettings,
    root: &str,
) -> Result<
    Option<crate::config::projects::ProjectRegistration>,
    crate::config::projects::ProjectError,
> {
    let key = crate::config::projects::normalize_for_compare(root);
    let still_archived = settings
        .archived_project_paths
        .iter()
        .any(|p| crate::config::projects::normalize_for_compare(p) == key);
    if !still_archived {
        return Ok(None);
    }

    crate::config::projects::register_existing_project(settings, root).map(Some)
}

/// #881: read-only preflight used before restart destroys the dormant row.
pub(crate) async fn probe_spawn_refusal<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    cwd: &str,
) -> Result<(), String> {
    let settings = app.state::<crate::config::settings::SettingsState>();
    let archived = { settings.read().await.archived_project_paths.clone() };
    if archived.is_empty() {
        return Ok(());
    }

    let cwd_owned = cwd.to_string();
    let root = tokio::task::spawn_blocking(move || archived_root_for_cwd(&cwd_owned, &archived))
        .await
        .map_err(|e| format!("archive gate probe task failed: {}", e))?;
    let Some(root) = root else {
        return Ok(());
    };

    probe_spawn_refusal_for_archived_root(&root)
}

/// #881: enforce that a session spawn never leaves a live PTY inside an
/// archived project.
pub(crate) async fn enforce_unarchived_for_spawn<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    cwd: &str,
    session_label: &str,
) -> Result<(), String> {
    let settings = app.state::<crate::config::settings::SettingsState>();
    let archived = { settings.read().await.archived_project_paths.clone() };
    if archived.is_empty() {
        return Ok(());
    }

    let cwd_owned = cwd.to_string();
    let root = tokio::task::spawn_blocking(move || archived_root_for_cwd(&cwd_owned, &archived))
        .await
        .map_err(|e| format!("archive gate task failed: {}", e))?;
    let Some(root) = root else {
        return Ok(());
    };

    auto_unarchive_for_activation(app, &root, session_label).await
}

async fn auto_unarchive_for_activation<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    root: &str,
    session_label: &str,
) -> Result<(), String> {
    let settings = app.state::<crate::config::settings::SettingsState>();
    let mut s = tokio::time::timeout(Duration::from_secs(5), settings.write())
        .await
        .map_err(|_| {
            format!(
                "archive gate: timed out taking the settings write lock while un-archiving '{}'. A caller of create_session_inner is holding a SettingsState guard. That is a bug, not a slow disk.",
                root
            )
        })?;

    crate::config::settings::refresh_project_paths_from_disk(&mut s)?;
    let reg = match auto_unarchive_registration(&mut s, root).map_err(|e| {
        format!(
            "Cannot start a session in archived project '{}': {}. Open Archived Projects to restore it, or remove it from the list.",
            folder_name(root),
            e
        )
    })? {
        Some(reg) => reg,
        None => return Ok(()),
    };

    let snapshot = s.clone();
    crate::config::settings::save_settings_with_project_paths(&snapshot)?;
    drop(s);

    log::warn!(
        "[archive] auto-unarchived '{}': session '{}' activated inside it",
        root,
        session_label
    );
    crate::commands::ac_discovery::broadcast_project_archive_changed(
        app,
        &reg.path,
        false,
        crate::commands::ac_discovery::ArchiveChangeReason::AutoUnarchive,
        Some(session_label),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::AppSettings;

    #[test]
    fn first_project_path_containing_returns_the_matching_archived_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archived = temp.path().join("archived");
        let agent = archived.join(".ac").join("wg-1").join("__agent_dev");
        std::fs::create_dir_all(&agent).expect("create agent dir");
        let roots = crate::config::sessions_persistence::normalize_project_roots(&[archived
            .to_string_lossy()
            .to_string()]);

        assert_eq!(
            crate::config::sessions_persistence::first_project_path_containing(
                &agent.to_string_lossy(),
                &roots
            ),
            Some(roots[0].clone())
        );
    }

    #[test]
    fn first_project_path_containing_returns_none_for_a_cwd_outside_every_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archived = temp.path().join("archived");
        let other = temp.path().join("other").join(".ac").join("wg-1");
        std::fs::create_dir_all(&archived).expect("create archived");
        std::fs::create_dir_all(&other).expect("create other");
        let roots = crate::config::sessions_persistence::normalize_project_roots(&[archived
            .to_string_lossy()
            .to_string()]);

        assert_eq!(
            crate::config::sessions_persistence::first_project_path_containing(
                &other.to_string_lossy(),
                &roots
            ),
            None
        );
    }

    #[test]
    fn first_project_path_containing_returns_none_on_an_empty_root_list() {
        assert_eq!(
            crate::config::sessions_persistence::first_project_path_containing(
                "Z:/does/not/exist/x",
                &[]
            ),
            None
        );
    }

    #[test]
    fn probe_spawn_refusal_rejects_an_archived_root_without_a_workspace_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("archived-project");
        std::fs::create_dir_all(&project).expect("create project dir");

        let err = probe_spawn_refusal_for_archived_root(&project.to_string_lossy())
            .expect_err("missing workspace should reject");

        assert!(err.contains("archived-project"), "{err}");
    }

    #[test]
    fn probe_spawn_refusal_accepts_an_archived_root_that_still_has_a_workspace_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("archived-project");
        std::fs::create_dir_all(project.join(".ac")).expect("create workspace");

        probe_spawn_refusal_for_archived_root(&project.to_string_lossy())
            .expect("workspace should accept");
    }

    #[test]
    fn auto_unarchive_registration_coalesces_when_project_is_no_longer_archived() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("archived-project");
        std::fs::create_dir_all(project.join(".ac")).expect("create workspace");
        let project = project.to_string_lossy().to_string();
        let mut settings = AppSettings {
            archived_project_paths: vec![project.clone()],
            ..AppSettings::default()
        };

        let first = auto_unarchive_registration(&mut settings, &project)
            .expect("first registration")
            .expect("first caller should unarchive");
        assert_eq!(first.path, project);
        assert!(settings.archived_project_paths.is_empty());
        assert_eq!(settings.project_paths.len(), 1);

        let second =
            auto_unarchive_registration(&mut settings, &first.path).expect("second registration");
        assert!(second.is_none(), "second caller must coalesce");
    }
}
