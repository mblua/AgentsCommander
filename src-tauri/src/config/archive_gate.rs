use std::path::Path;
use std::time::Duration;

use crate::config::instance_artifacts::SETTINGS_FILE_NAME;
use crate::config::settings::SettingsState;

pub(crate) trait ArchiveGateHost {
    fn archive_settings(&self) -> SettingsState;
    fn auto_unarchived(&self, path: &str, session_label: &str);
}

fn folder_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string()
}

fn archived_root_for_cwd(cwd: &str, archived_roots: &[String]) -> Option<String> {
    crate::config::sessions_persistence::raw_project_path_containing(cwd, archived_roots)
}

fn probe_spawn_refusal_for_archived_root(root: &str) -> Result<(), String> {
    if crate::config::ac_root::has_ac_root(Path::new(root)) {
        return Ok(());
    }
    Err(format!(
        "Cannot start a session in archived project '{}': Project AC Root not found. Open \
         Archived Projects to restore it, or remove it from the list.",
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
pub(crate) async fn probe_spawn_refusal<H: ArchiveGateHost + ?Sized>(
    app: &H,
    cwd: &str,
) -> Result<(), String> {
    let settings = app.archive_settings();
    let archived = {
        let current = settings.read().await;
        if let Some(error) = &current.project_paths_persistence_error {
            return Err(error.clone());
        }
        current.archived_project_paths.clone()
    };
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
pub(crate) async fn enforce_unarchived_for_spawn<H, W, WF, A, AF, P, F>(
    app: &H,
    cwd: &str,
    session_label: &str,
    prepare: W,
    admit: A,
    publish_settings: F,
) -> Result<(), String>
where
    H: ArchiveGateHost + ?Sized,
    W: FnMut() -> WF,
    WF: std::future::Future<Output = Result<(), String>>,
    A: FnMut() -> AF,
    AF: std::future::Future<Output = Result<Option<P>, String>>,
    P: Send + 'static,
    F: FnOnce(&crate::config::settings::AppSettings),
{
    let settings = app.archive_settings();
    let archived = {
        let current = settings.read().await;
        if let Some(error) = &current.project_paths_persistence_error {
            return Err(error.clone());
        }
        current.archived_project_paths.clone()
    };
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

    auto_unarchive_for_activation(app, &root, session_label, prepare, admit, publish_settings).await
}

async fn auto_unarchive_for_activation<H, W, WF, A, AF, P, F>(
    app: &H,
    root: &str,
    session_label: &str,
    mut prepare: W,
    mut admit: A,
    publish_settings: F,
) -> Result<(), String>
where
    H: ArchiveGateHost + ?Sized,
    W: FnMut() -> WF,
    WF: std::future::Future<Output = Result<(), String>>,
    A: FnMut() -> AF,
    AF: std::future::Future<Output = Result<Option<P>, String>>,
    P: Send + 'static,
    F: FnOnce(&crate::config::settings::AppSettings),
{
    let settings = app.archive_settings();
    let (mut s, permission, path) = loop {
        // Target acquisition never waits while a settings guard is held.
        prepare().await?;
        let s = tokio::time::timeout(Duration::from_secs(5), settings.write())
            .await
            .map_err(|_| {
                format!(
                "archive gate: timed out taking the settings write lock while un-archiving '{}'. \
                 A caller of create_session_inner is holding a SettingsState guard. That is a \
                 bug, not a slow disk.",
                root
            )
            })?;
        let path = crate::config::config_dir()
            .ok_or("Could not determine settings directory")?
            .join(SETTINGS_FILE_NAME);
        if let Some(permission) = admit().await? {
            break (s, permission, path);
        }
        drop(s);
        tokio::task::yield_now().await;
    };

    let root_owned = root.to_string();
    let current = s.clone();
    // The managed guard stays owned by this future; the blocking closure owns
    // only the file token and captures, and releases it before the await ends.
    let (permission, written) = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let mut transaction =
            crate::config::settings::begin_project_paths_transaction_at_path(&current, &path)?;
        let mut candidate = transaction.fresh_settings().clone();
        if crate::config::settings::project_state_has_structural(&candidate) {
            return Err(format!(
                "Cannot start a session in archived project '{}': settings.json has malformed \
                 project metadata. Fix or remove the corrupt project fields first.",
                folder_name(&root_owned)
            ));
        }
        transaction.refresh_raw_projects(&mut candidate)?;
        let reg = auto_unarchive_registration(&mut candidate, &root_owned).map_err(|error| {
            format!(
                "Cannot start a session in archived project '{}': {}. Open Archived Projects to \
                 restore it, or remove it from the list.",
                folder_name(&root_owned),
                error
            )
        })?;
        let Some(reg) = reg else {
            return Ok((permission, (candidate, Ok(None))));
        };
        crate::config::settings::resync_project_state_from_runtime(&mut candidate);
        match transaction.commit_project_groups(&candidate, true, true) {
            Ok(written) => Ok((permission, (written, Ok(Some(reg))))),
            Err(error) => Ok((
                permission,
                (transaction.fresh_settings().clone(), Err(error)),
            )),
        }
    })
    .await
    .map_err(|error| format!("archive gate task failed: {error}"))??;
    // Every actual tuple, including the nested commit error, publishes the
    // effective pair synchronously before this settings adoption.
    publish_settings(&written.0);
    *s = written.0;
    drop(permission);
    let Some(reg) = written.1? else {
        return Ok(());
    };
    drop(s);

    log::warn!(
        "[archive] auto-unarchived '{}': session '{}' activated inside it",
        root,
        session_label
    );
    app.auto_unarchived(&reg.path, session_label);
    Ok(())
}

#[cfg(test)]
mod tests;
