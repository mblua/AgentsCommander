use crate::config::instance_artifacts::SETTINGS_FILE_NAME;
use crate::config::settings::{AppSettings, SettingsState};
use std::path::Path;
use tauri::{Manager, State};

// ── #191 — shared open/new project commands ──────────────────────────────

pub(crate) async fn mutate_project_paths_with_settings_path<T: Send + 'static>(
    settings: &SettingsState,
    settings_path: Option<&Path>,
    mutate: impl FnOnce(&mut AppSettings) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let path = match settings_path {
        Some(path) => path.to_path_buf(),
        None => crate::config::config_dir()
            .ok_or("Could not determine settings directory")?
            .join(SETTINGS_FILE_NAME),
    };
    let settings = std::sync::Arc::clone(settings);
    crate::config::seed_manifest::run_blocking_owned(move || {
        let mut current = settings.blocking_write();
        let mut transaction =
            crate::config::settings::begin_project_paths_transaction_at_path(&current, &path)?;
        let mut candidate = transaction.fresh_settings().clone();
        transaction.refresh_raw_projects(&mut candidate)?;
        if crate::config::settings::project_state_has_structural(&candidate) {
            return Err("project-paths.json has malformed project metadata".to_string());
        }
        let result = mutate(&mut candidate)?;
        crate::config::settings::resync_project_state_from_runtime(&mut candidate);
        match transaction.commit_project_groups(&candidate, true, true) {
            Ok(written) => {
                *current = written;
                Ok(result)
            }
            Err(error) => {
                *current = transaction.fresh_settings().clone();
                Err(error)
            }
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ArchiveChangeReason {
    Archive,
    Unarchive,
    AutoUnarchive,
    Open,
    Remove,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectArchiveChanged {
    pub path: String,
    pub folder_name: String,
    pub archived: bool,
    pub reason: ArchiveChangeReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
}

pub(crate) fn project_folder_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string()
}

pub(crate) fn broadcast_project_archive_changed<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    path: &str,
    archived: bool,
    reason: ArchiveChangeReason,
    session_name: Option<&str>,
) {
    let payload = serde_json::json!(ProjectArchiveChanged {
        path: path.to_string(),
        folder_name: project_folder_name(path),
        archived,
        reason,
        session_name: session_name.map(|name| name.to_string()),
    });
    crate::web::commands::broadcast_all_r(app, "project_archive_changed", &payload);
}

impl<R: tauri::Runtime> crate::config::archive_gate::ArchiveGateHost for tauri::AppHandle<R> {
    fn archive_settings(&self) -> SettingsState {
        std::sync::Arc::clone(self.state::<SettingsState>().inner())
    }

    fn auto_unarchived(&self, path: &str, session_label: &str) {
        broadcast_project_archive_changed(
            self,
            path,
            false,
            ArchiveChangeReason::AutoUnarchive,
            Some(session_label),
        );
    }
}

/// Validate an existing AC project at `path` and register it in
/// `settings.project_paths`.
pub(crate) async fn open_project_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    open_project_inner_with_event_and_settings_path(app, settings, path, None).await
}

pub(crate) async fn open_project_inner_with_event_and_settings_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    let (result, archived_changed) =
        open_project_inner_with_settings_path(settings, path, settings_path).await?;
    if archived_changed {
        broadcast_project_archive_changed(
            app,
            &result.path,
            false,
            ArchiveChangeReason::Open,
            None,
        );
    }
    // #1318 - a first open of an unseeded project seeds the catalog + masters
    // immediately (no restart needed). The gate acquire (kernel lock, up to
    // DEFAULT_LOCK_TIMEOUT 10s polling) must run on a blocking thread, never on
    // the async executor, hence run_blocking_owned. Fail-soft: any error is
    // logged and the boot loop covers the project at the next boot.
    if result.registered {
        let path = result.path.clone();
        if let Err(error) = crate::config::seed_manifest::run_blocking_owned(move || {
            crate::config::coding_agents_catalog::ensure_seeded_for_project(Path::new(&path));
        })
        .await
        {
            log::warn!(
                "[ac-discovery] catalog seed after open-project failed: {}",
                error
            );
        }
    }
    Ok(result)
}

pub(crate) async fn open_project_inner_with_settings_path(
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<(crate::config::projects::ProjectRegistration, bool), String> {
    let mutation_path = path.to_string();
    mutate_project_paths_with_settings_path(settings, settings_path, move |s| {
        let path = mutation_path.as_str();
        let before = s.archived_project_paths.clone();
        let result = crate::config::projects::register_existing_project(s, path)
            .map_err(|e| e.to_string())?;
        Ok((result, before != s.archived_project_paths))
    })
    .await
}

#[tauri::command]
pub async fn open_project(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    open_project_inner(&app, settings.inner(), &path).await
}

/// Ensure an AC project at `path` (creating `.ac/` if missing) and
/// register it in `settings.project_paths`. Mirrors the ActionBar "New
/// Project" flow at `src/sidebar/components/ActionBar.tsx:58-71`.
#[tauri::command]
pub async fn new_project(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    new_project_inner(&app, settings.inner(), &path).await
}

pub(crate) async fn new_project_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    new_project_inner_with_event_and_settings_path(app, settings, path, None).await
}

pub(crate) async fn new_project_inner_with_event_and_settings_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    let (result, archived_changed) =
        new_project_inner_with_settings_path(settings, path, settings_path).await?;
    if archived_changed {
        broadcast_project_archive_changed(
            app,
            &result.path,
            false,
            ArchiveChangeReason::Open,
            None,
        );
    }
    Ok(result)
}

pub(crate) async fn new_project_inner_with_settings_path(
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<(crate::config::projects::ProjectRegistration, bool), String> {
    new_project_inner_with_settings_path_and_hooks(
        settings,
        path,
        settings_path,
        NewProjectTransactionHooks::default(),
    )
    .await
}

#[cfg(test)]
pub(crate) type NewProjectFinalRevalidationHook = std::sync::Arc<dyn Fn(&Path) + Send + Sync>;

#[derive(Clone, Default)]
pub(crate) struct NewProjectTransactionHooks {
    #[cfg(test)]
    pub(crate) fail_settings_save: bool,
    #[cfg(test)]
    pub(crate) before_final_revalidation: Option<NewProjectFinalRevalidationHook>,
}

impl NewProjectTransactionHooks {
    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "self contiene inyección de fallo en compilación de pruebas; preservar hook y \
                contrato de transacción."
        )
    )]
    pub(crate) fn save_settings(
        &self,
        settings: &crate::config::settings::AppSettings,
        transaction: &mut crate::config::settings::ProjectPathsWriteTransaction,
    ) -> Result<crate::config::settings::AppSettings, String> {
        #[cfg(test)]
        if self.fail_settings_save {
            return Err("injected new-project settings save failure".to_string());
        }
        transaction.commit_project_groups(settings, true, true)
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "self contiene hook en compilación de pruebas; conservar orden y revalidación \
                final."
        )
    )]
    pub(crate) fn before_final_revalidation(&self, ac_root: &Path) {
        #[cfg(test)]
        if let Some(hook) = &self.before_final_revalidation {
            hook(ac_root);
        }
        #[cfg(not(test))]
        let _ = ac_root;
    }
}

pub(crate) async fn new_project_inner_with_settings_path_and_hooks(
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
    hooks: NewProjectTransactionHooks,
) -> Result<(crate::config::projects::ProjectRegistration, bool), String> {
    let path = path.to_string();
    let settings = std::sync::Arc::clone(settings);
    let settings_path = settings_path.map(Path::to_path_buf);
    crate::config::seed_manifest::run_blocking_owned(move || {
        let prepared = crate::config::projects::prepare_new_project(&path)
            .map_err(|error| error.to_string())?;
        let mut current = settings.blocking_write();
        let settings_path = match settings_path {
            Some(path) => path,
            None => crate::config::config_dir()
                .ok_or(
                    "Could not determine settings \
                directory",
                )?
                .join(SETTINGS_FILE_NAME),
        };
        let mut transaction = crate::config::settings::begin_project_paths_transaction_at_path(
            &current,
            &settings_path,
        )?;
        let before_raw = transaction.raw_project_projection()?;
        let mut candidate = transaction.fresh_settings().clone();
        transaction.refresh_raw_projects(&mut candidate)?;
        if crate::config::settings::project_state_has_structural(&candidate) {
            return Err("project-paths.json has malformed project metadata".to_string());
        }
        let before_archived = candidate.archived_project_paths.clone();
        let result =
            crate::config::projects::commit_prepared_new_project(&mut candidate, &prepared);
        crate::config::settings::resync_project_state_from_runtime(&mut candidate);
        prepared.revalidate().map_err(|error| error.to_string())?;
        let written = match hooks.save_settings(&candidate, &mut transaction) {
            Ok(written) => written,
            Err(error) => {
                *current = transaction.fresh_settings().clone();
                return Err(error);
            }
        };
        hooks.before_final_revalidation(prepared.ac_root());
        if let Err(error) = prepared.revalidate() {
            return Err(
                match transaction.restore_raw_project_projection(&before_raw) {
                    Ok(written) => {
                        *current = written;
                        error.to_string()
                    }
                    Err(rollback_error) => {
                        *current = transaction.fresh_settings().clone();
                        current.project_paths_persistence_error = Some(rollback_error.clone());
                        format!(
                            "{error}; failed to roll back project registration settings: \
                        {rollback_error}"
                        )
                    }
                },
            );
        }
        *current = written;
        drop(transaction);
        let archived_changed = before_archived != current.archived_project_paths;
        drop(current);
        // #1318 - capture the project root BEFORE release (`abs` is private;
        // `ac_root()` returns the `.ac` dir).
        let project_root = prepared.ac_root().parent().map(Path::to_path_buf);
        prepared.release();
        // #1318 - a fresh registration seeds the catalog + masters immediately
        // (no restart needed). INSIDE this blocking closure, so the gate acquire
        // runs on the blocking thread. Fail-soft: any error is logged and the
        // boot loop covers the project at the next boot.
        if let Some(root) = project_root {
            crate::config::coding_agents_catalog::ensure_seeded_for_project(&root);
        }
        Ok((result, archived_changed))
    })
    .await
    .map_err(|error| format!("new project blocking transaction failed: {error}"))?
}

/// #778 Part 3: targeted project removal. `project_paths` is disk-authoritative
/// under Design S, so a removal cannot ride a whole-object settings save (the
/// default `save_settings` preserves the on-disk list and would ignore the
/// removal, and without a baseline the backend cannot tell a removal from a stale
/// passthrough). So removal is its own command: reconcile the list from disk
/// (G2: abort on a non-NotFound read error), drop the one entry whose
/// `normalize_for_compare` key matches `path`, re-derive the head, and write the
/// deliberate list verbatim. Removing against the FRESH disk list means a
/// CLI-appended entry the sidebar never showed is preserved, not dropped.
pub(crate) async fn remove_project_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &SettingsState,
    path: &str,
) -> Result<(), String> {
    let archived_changed = remove_project_inner_with_settings_path(settings, path, None).await?;
    if archived_changed {
        broadcast_project_archive_changed(app, path, false, ArchiveChangeReason::Remove, None);
    }
    Ok(())
}

pub(crate) async fn remove_project_inner_with_settings_path(
    settings: &SettingsState,
    path: &str,
    settings_path: Option<&Path>,
) -> Result<bool, String> {
    let mutation_path = path.to_string();
    mutate_project_paths_with_settings_path(settings, settings_path, move |s| {
        let path = mutation_path.as_str();
        let before = s.archived_project_paths.clone();
        crate::config::projects::remove_project_path(s, path);
        Ok(before != s.archived_project_paths)
    })
    .await
}

#[tauri::command]
pub async fn remove_project(
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<(), String> {
    remove_project_inner(&app, settings.inner(), &path).await
}
