use super::super::{
    project_lists_from_projection, refresh_coding_agent_identity_at_locked_path,
    replace_project_projection, report_settings_save_error, write_value_atomic, AppSettings,
    SettingsFileLock, SettingsSaveError, SettingsSaveReportSurface, SettingsSaveStage,
};
use super::migration::{
    ensure_project_paths_migrated_locked, project_migration_error, project_paths_path,
    read_raw_project_source, select_project_source_locked, ProjectPathsRepairStage,
    ProjectPathsWriteTransaction,
};
use super::{
    decode_project_state, hidden_state_for_write, production_instance_base, write_active_group,
    write_archived_group,
};
use crate::config::projects;
use serde_json::{Map, Value};
use std::path::Path;
use std::sync::Arc;

#[track_caller]
pub(crate) fn empty_project_transaction(
    path: &Path,
    operation: &'static str,
) -> Result<ProjectPathsWriteTransaction, SettingsSaveError> {
    Ok(ProjectPathsWriteTransaction {
        _lock: SettingsFileLock::acquire_contextual(
            path,
            std::time::Duration::from_secs(2),
            operation,
        )?,
        path: path.to_path_buf(),
        fresh: AppSettings::default(),
        revalidated_after_failure: false,
    })
}

pub(crate) fn apply_selected_project_state(
    settings: &mut AppSettings,
    projection: &Map<String, Value>,
) {
    let state = decode_project_state(
        projection,
        production_instance_base().as_deref(),
        &projects::FsCandidateResolver,
    );
    settings.project_paths = state.active_selected();
    settings.project_path = state.selected_head.clone();
    settings.archived_project_paths = state.archived_management_paths();
    settings.project_path_state = Arc::new(state);
    settings.project_paths_persistence_error = None;
}

pub(crate) fn begin_project_paths_transaction_at_path(
    current: &AppSettings,
    settings_path: &Path,
) -> Result<ProjectPathsWriteTransaction, String> {
    let mut transaction =
        empty_project_transaction(settings_path, "begin_project_paths_transaction_at_path")
            .map_err(|error| {
                report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
            })?;
    ensure_project_paths_migrated_locked(&transaction).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    transaction.fresh = current.clone();
    refresh_coding_agent_identity_at_locked_path(&mut transaction.fresh, settings_path);
    let selected = select_project_source_locked(&transaction).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    apply_selected_project_state(&mut transaction.fresh, &selected.projection);
    Ok(transaction)
}

impl ProjectPathsWriteTransaction {
    pub(crate) fn fresh_settings(&self) -> &AppSettings {
        &self.fresh
    }

    pub(crate) fn commit_project_groups(
        &mut self,
        candidate: &AppSettings,
        active: bool,
        archived: bool,
    ) -> Result<AppSettings, String> {
        if let Some(message) = &candidate.project_paths_persistence_error {
            return Err(message.clone());
        }
        self.revalidated_after_failure = false;
        let result = self.write_project_groups(candidate, active, archived);
        self.handle_commit_result(result)
    }

    pub(crate) fn refresh_raw_projects(&self, settings: &mut AppSettings) -> Result<(), String> {
        let selected = select_project_source_locked(self).map_err(|error| {
            report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
        })?;
        apply_selected_project_state(settings, &selected.projection);
        if settings.project_path_state.has_structural() {
            return Err("project metadata is malformed".to_string());
        }
        if let Some(lists) = project_lists_from_projection(&selected.projection)? {
            settings.project_paths = lists.project_paths;
            settings.project_path = lists.project_path;
            settings.archived_project_paths = lists.archived_project_paths.unwrap_or_default();
        }
        Ok(())
    }

    pub(crate) fn raw_project_projection(&self) -> Result<Map<String, Value>, String> {
        select_project_source_locked(self)
            .map(|selected| selected.projection)
            .map_err(|error| {
                report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
            })
    }

    pub(crate) fn restore_raw_project_projection(
        &mut self,
        projection: &Map<String, Value>,
    ) -> Result<AppSettings, String> {
        let result = (|| {
            let selected = select_project_source_locked(self)?;
            let mut object = selected
                .destination
                .map(|source| source.object)
                .unwrap_or_default();
            replace_project_projection(&mut object, projection);
            write_value_atomic(&Value::Object(object), &project_paths_path(&self.path))?;
            let selected = select_project_source_locked(self)?;
            let mut written = self.fresh.clone();
            refresh_coding_agent_identity_at_locked_path(&mut written, &self.path);
            apply_selected_project_state(&mut written, &selected.projection);
            Ok(written)
        })();
        self.handle_commit_result(result)
    }

    pub(crate) fn write_project_groups(
        &self,
        candidate: &AppSettings,
        active: bool,
        archived: bool,
    ) -> Result<AppSettings, SettingsSaveError> {
        let selected = select_project_source_locked(self)?;
        let destination = project_paths_path(&self.path);
        let mut out = selected
            .destination
            .map(|source| source.object)
            .unwrap_or_default();
        let state = hidden_state_for_write(candidate);
        if state.has_structural() {
            return Err(project_migration_error(
                &destination,
                SettingsSaveStage::ReconcileDiskGate,
                "project metadata is malformed",
            ));
        }
        let base = production_instance_base();
        if active && (state.runtime_authoritative || state.active_reconcile_eligible) {
            write_active_group(&mut out, &state, base.as_deref());
        }
        if archived && (state.runtime_authoritative || state.archived_reconcile_eligible) {
            write_archived_group(&mut out, &state, base.as_deref());
        }
        if out
            != read_raw_project_source(&destination)?
                .map(|source| source.object)
                .unwrap_or_default()
        {
            write_value_atomic(&Value::Object(out), &destination)?;
        }
        let projection = select_project_source_locked(self)?.projection;
        let mut written = candidate.clone();
        refresh_coding_agent_identity_at_locked_path(&mut written, &self.path);
        apply_selected_project_state(&mut written, &projection);
        Ok(written)
    }

    pub(crate) fn handle_commit_result(
        &mut self,
        result: Result<AppSettings, SettingsSaveError>,
    ) -> Result<AppSettings, String> {
        match result {
            Ok(mut written) => {
                written.project_paths_persistence_error = None;
                self.fresh = written.clone();
                Ok(written)
            }
            Err(error) => {
                let uncertain = match error.stage {
                    SettingsSaveStage::AtomicReplace
                    | SettingsSaveStage::PostWriteRead
                    | SettingsSaveStage::PostWriteVerify
                    | SettingsSaveStage::ReDecode => true,
                    #[cfg(unix)]
                    SettingsSaveStage::TargetPermissions
                    | SettingsSaveStage::ParentDirectorySync => true,
                    _ => false,
                };
                let message =
                    report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings);
                if uncertain {
                    // Re-read under this same lock; never roll back a write
                    // whose outcome cannot be established.
                    let validated = select_project_source_locked(self)
                        .map(|selected| {
                            let mut settings = self.fresh.clone();
                            refresh_coding_agent_identity_at_locked_path(&mut settings, &self.path);
                            apply_selected_project_state(&mut settings, &selected.projection);
                            settings
                        })
                        .map_err(|error| {
                            report_settings_save_error(
                                error,
                                SettingsSaveReportSurface::GeneralSettings,
                            )
                        });
                    match validated {
                        Ok(mut settings) => {
                            settings.local_overlay_state = self.fresh.local_overlay_state.clone();
                            settings.project_paths_persistence_error = None;
                            self.fresh = settings;
                        }
                        Err(reason) => {
                            self.fresh.project_paths_persistence_error = Some(format!(
                                "{message}; project state revalidation failed: {reason}"
                            ))
                        }
                    }
                    self.revalidated_after_failure = true;
                }
                Err(message)
            }
        }
    }
}

/// Validate fresh authority even if the cached state reports no repair.
pub(crate) fn repair_fresh_project_paths_at_path(
    current: &mut AppSettings,
    path: &Path,
) -> Result<(), (ProjectPathsRepairStage, String)> {
    let mut transaction = match begin_project_paths_transaction_at_path(current, path) {
        Ok(transaction) => transaction,
        Err(message) => {
            current.project_paths_persistence_error = Some(message.clone());
            return Err((ProjectPathsRepairStage::Read, message));
        }
    };
    let candidate = transaction.fresh_settings().clone();
    let state = &candidate.project_path_state;
    let active = state.active_reconcile_eligible;
    let archived = state.archived_reconcile_eligible;
    if state.has_structural() || !(active || archived) {
        *current = candidate;
        return Ok(());
    }
    match transaction.commit_project_groups(&candidate, active, archived) {
        Ok(written) => {
            *current = written;
            Ok(())
        }
        Err(message) => {
            if transaction.revalidated_after_failure {
                *current = transaction.fresh_settings().clone();
            }
            Err((ProjectPathsRepairStage::Write, message))
        }
    }
}
