use super::super::projects;
use super::{
    agents_instance_groups, agents_instance_path, apply_project_decode_to_value,
    apply_selected_project_state, capture_registered_profile_gate,
    captured_registered_profile_object, coding_agent_identity_with_physical_overlay,
    empty_project_transaction, ensure_project_paths_migrated_at_locked_path, finalize_agent_order,
    legacy_profiles_shape_present, merge_agents_instance_file, normalize_agent_order,
    physical_identity_violations, physical_profile_rows, prepare_registered_profile_owner,
    production_instance_base, project_physical_profiles, read_agents_instance_file,
    recheck_registered_profile_gate, refresh_agent_id_hashes, refresh_pair_id_hashes,
    registered_profiles_for_read, repair_loaded_response_close_settings,
    repair_loaded_room_idle_confirmation_settings, replace_project_projection,
    restore_agents_groups_for_redecode, restore_codex_home_carrier,
    restrict_split_profile_mutation_rows, retain_configuration_commands,
    retain_configuration_commands_with_startup, retain_selected_legacy_profile_aliases,
    retain_unknown_registered_agent_fields, rotate_settings_backups, save_agents_instance_file,
    select_project_source_at_locked_path, select_project_source_at_locked_path_with_limit,
    select_project_source_locked, selected_registered_profile_owner, set_agents_layer_state,
    settings_path, source_general_field_names, validate_non_project_settings,
    validate_physical_identity_delta, validate_registered_overlay_profile_edits,
    validate_split_canonical_profile_edits, write_identity_rows, write_pre_384_v1_backup,
    AppSettings, MainWindowDisplayState, MainWindowPlacementWriteMode, ProjectWriteMode,
    RegisteredProfileDocument, StartupAgentRows, TerminalSnapshotGateWriteMode, WindowGeometry,
    AGENTS_INSTANCE_KEYS, FIELD_MAIN_DISPLAY_STATE, FIELD_MAIN_GEOMETRY, FIELD_PROJECT_PATH,
    FIELD_PROJECT_PATHS, FIELD_PROJECT_PATHS_REL, FIELD_PROJECT_PATH_REL,
    FIELD_TERMINAL_SNAPSHOTS_ENABLED, OVERLAY_KEY_MAIN_DISPLAY_STATE, OVERLAY_KEY_MAIN_GEOMETRY,
};
#[cfg(test)]
use super::{
    project_migration_error, reconcile_project_state_to_path, SETTINGS_PRESERVE_LOCK_HOOK,
};
use crate::config::instance_artifacts::{SETTINGS_FILE_NAME, SETTINGS_LOCK_FILE_NAME};
use crate::config::local_overlay::LocalSettingsOverlay;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// #774: counter feeding the per-call unique temp filename for settings saves.
/// Combined with the PID it makes `settings.json.<pid>.<op_id>.tmp` distinct from
/// any concurrent cross-process save (GUI startup, CLI verbs, closing flush) and
/// from any leftover temp written by a prior crashed run. Mirrors
/// `sessions_persistence::SAVE_OP_ID` (sessions_persistence.rs:47); a separate
/// counter so the two can be reasoned about independently in diagnostics.
pub(crate) static SAVE_OP_ID: AtomicU64 = AtomicU64::new(0);

/// Fresh-read the whole disk object for a project-preserving/reconciling write.
/// `None` = absent (materialize). `Err` = present-but-unreadable/invalid JSON,
/// non-object root, or non-project settings that fail to deserialize — in which
pub(crate) const SETTINGS_SAVE_FAILED_CODE: &str = "settings_save_failed";
pub(crate) const SETTINGS_LOCK_UNAVAILABLE_CODE: &str = "settings_lock_unavailable";
pub(crate) const TERMINAL_SNAPSHOT_SETTING_SAVE_FAILED_CODE: &str =
    "terminal_snapshot_setting_save_failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSaveReportSurface {
    GeneralSettings,
    TerminalSnapshotCompareAndSet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSaveStage {
    LockAcquire,
    PreserveDiskGate,
    ReconcileDiskGate,
    ProjectPathsRefreshDiskGate,
    Serialize,
    PrepareTarget,
    TempCreate,
    TempWrite,
    TempFlush,
    TempSync,
    TempVerify,
    AtomicReplace,
    #[cfg(unix)]
    TargetPermissions,
    #[cfg(unix)]
    ParentDirectorySync,
    PostWriteRead,
    PostWriteVerify,
    ReDecode,
}

impl SettingsSaveStage {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::LockAcquire => "lock_acquire",
            Self::PreserveDiskGate => "preserve_disk_gate",
            Self::ReconcileDiskGate => "reconcile_disk_gate",
            Self::ProjectPathsRefreshDiskGate => "project_paths_refresh_disk_gate",
            Self::Serialize => "serialize",
            Self::PrepareTarget => "prepare_target",
            Self::TempCreate => "temp_create",
            Self::TempWrite => "temp_write",
            Self::TempFlush => "temp_flush",
            Self::TempSync => "temp_sync",
            Self::TempVerify => "temp_verify",
            Self::AtomicReplace => "atomic_replace",
            #[cfg(unix)]
            Self::TargetPermissions => "target_permissions",
            #[cfg(unix)]
            Self::ParentDirectorySync => "parent_directory_sync",
            Self::PostWriteRead => "post_write_read",
            Self::PostWriteVerify => "post_write_verify",
            Self::ReDecode => "re_decode",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSaveReason {
    ProjectPathsConflict,
    ProjectPathsBackupMismatch,
    ProjectPathsSourceRejected,
    LockTimedOut,
    LockDeadlineOverflow,
    LockFileNotRegular,
    #[cfg(unix)]
    LockLinkCountRejected,
    #[cfg(not(any(unix, windows)))]
    LockPlatformUnsupported,
    LockIdentityRejected,
    MissingParentDirectory,
    DiskJsonNotObject,
    DiskSettingsValidationRejected,
    SerializedJsonNotObject,
    TargetNotRegularFile,
    TargetIdentityRejected,
    TerminalSnapshotSettingInvalid,
    SerializedPayloadTooLarge,
    TempIdentityRejected,
    PostWriteReadRejected,
    WrittenBytesMismatch,
}

impl SettingsSaveReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ProjectPathsConflict => "project_paths_conflict",
            Self::ProjectPathsBackupMismatch => "project_paths_backup_mismatch",
            Self::ProjectPathsSourceRejected => "project_paths_source_rejected",
            Self::LockTimedOut => "lock_timed_out",
            Self::LockDeadlineOverflow => "lock_deadline_overflow",
            Self::LockFileNotRegular => "lock_file_not_regular",
            #[cfg(unix)]
            Self::LockLinkCountRejected => "lock_link_count_rejected",
            #[cfg(not(any(unix, windows)))]
            Self::LockPlatformUnsupported => "lock_platform_unsupported",
            Self::LockIdentityRejected => "lock_identity_rejected",
            Self::MissingParentDirectory => "missing_parent_directory",
            Self::DiskJsonNotObject => "disk_json_not_object",
            Self::DiskSettingsValidationRejected => "disk_settings_validation_rejected",
            Self::SerializedJsonNotObject => "serialized_json_not_object",
            Self::TargetNotRegularFile => "target_not_regular_file",
            Self::TargetIdentityRejected => "target_identity_rejected",
            Self::TerminalSnapshotSettingInvalid => "terminal_snapshot_setting_invalid",
            Self::SerializedPayloadTooLarge => "serialized_payload_too_large",
            Self::TempIdentityRejected => "temp_identity_rejected",
            Self::PostWriteReadRejected => "post_write_read_rejected",
            Self::WrittenBytesMismatch => "written_bytes_mismatch",
        }
    }
}

pub(crate) enum SettingsSaveCause {
    Io(std::io::Error),
    Json(serde_json::Error),
    Semantic(SettingsSaveReason),
}

pub(crate) enum SettingsSaveLegacyOutward {
    SettingsLockUnavailable,
    SettingsSaveFailed,
    DiskRead(String),
    DiskJson(String),
    DiskJsonNotObject(String),
    DiskValidation(String),
    Serialize(String),
    SerializedJsonNotObject(String),
    TerminalSnapshotSettingInvalid(String),
    ReDecode(String),
}

impl SettingsSaveLegacyOutward {
    pub(crate) fn into_general_string(self) -> String {
        match self {
            Self::SettingsLockUnavailable => SETTINGS_LOCK_UNAVAILABLE_CODE.to_string(),
            Self::SettingsSaveFailed => SETTINGS_SAVE_FAILED_CODE.to_string(),
            Self::DiskRead(value)
            | Self::DiskJson(value)
            | Self::DiskJsonNotObject(value)
            | Self::DiskValidation(value)
            | Self::Serialize(value)
            | Self::SerializedJsonNotObject(value)
            | Self::TerminalSnapshotSettingInvalid(value)
            | Self::ReDecode(value) => value,
        }
    }
}

pub(crate) struct SettingsSaveError {
    pub(crate) stage: SettingsSaveStage,
    pub(crate) target_path: Box<PathBuf>,
    pub(crate) temp_path: Option<Box<PathBuf>>,
    pub(crate) pid: u32,
    pub(crate) display_target: Option<Box<PathBuf>>,
    pub(crate) display_temp: Option<Box<PathBuf>>,
    pub(crate) cause: SettingsSaveCause,
    pub(crate) legacy_outward: SettingsSaveLegacyOutward,
}

impl SettingsSaveError {
    pub(crate) fn io(
        stage: SettingsSaveStage,
        target_path: &Path,
        temp_path: Option<&Path>,
        source: std::io::Error,
        legacy_outward: SettingsSaveLegacyOutward,
    ) -> Self {
        Self {
            stage,
            target_path: Box::new(target_path.to_path_buf()),
            temp_path: temp_path.map(|path| Box::new(path.to_path_buf())),
            pid: std::process::id(),
            display_target: None,
            display_temp: None,
            cause: SettingsSaveCause::Io(source),
            legacy_outward,
        }
    }

    pub(crate) fn json(
        stage: SettingsSaveStage,
        target_path: &Path,
        temp_path: Option<&Path>,
        source: serde_json::Error,
        legacy_outward: SettingsSaveLegacyOutward,
    ) -> Self {
        Self {
            stage,
            target_path: Box::new(target_path.to_path_buf()),
            temp_path: temp_path.map(|path| Box::new(path.to_path_buf())),
            pid: std::process::id(),
            display_target: None,
            display_temp: None,
            cause: SettingsSaveCause::Json(source),
            legacy_outward,
        }
    }

    pub(crate) fn semantic(
        stage: SettingsSaveStage,
        target_path: &Path,
        temp_path: Option<&Path>,
        reason: SettingsSaveReason,
        legacy_outward: SettingsSaveLegacyOutward,
    ) -> Self {
        Self {
            stage,
            target_path: Box::new(target_path.to_path_buf()),
            temp_path: temp_path.map(|path| Box::new(path.to_path_buf())),
            pid: std::process::id(),
            display_target: None,
            display_temp: None,
            cause: SettingsSaveCause::Semantic(reason),
            legacy_outward,
        }
    }

    pub(crate) fn with_capture_labels(mut self, settings_label: &Path) -> Self {
        self.display_target = self
            .target_path
            .file_name()
            .map(|name| Box::new(settings_label.with_file_name(name)));
        self.display_temp = self.temp_path.as_deref().and_then(|physical| {
            physical
                .file_name()
                .map(|name| Box::new(settings_label.with_file_name(name)))
        });
        self
    }

    pub(crate) fn general_settings_diagnostic_code(&self) -> &'static str {
        match self.stage {
            SettingsSaveStage::LockAcquire => SETTINGS_LOCK_UNAVAILABLE_CODE,
            _ => SETTINGS_SAVE_FAILED_CODE,
        }
    }

    pub(crate) fn safe_diagnostic(
        &self,
        surface: SettingsSaveReportSurface,
    ) -> SettingsSaveDiagnostic {
        let (source_kind, reason, io_kind, raw_os_error, json_category, json_line, json_column) =
            match &self.cause {
                SettingsSaveCause::Io(source) => (
                    "io",
                    "none",
                    Some(io_error_kind_as_str(source.kind())),
                    source.raw_os_error(),
                    None,
                    None,
                    None,
                ),
                SettingsSaveCause::Json(source) => (
                    "json",
                    "none",
                    None,
                    None,
                    Some(json_error_category_as_str(source.classify())),
                    Some(source.line()),
                    Some(source.column()),
                ),
                SettingsSaveCause::Semantic(reason) => {
                    ("semantic", reason.as_str(), None, None, None, None, None)
                }
            };

        SettingsSaveDiagnostic {
            code: surface.diagnostic_code(self),
            stage: self.stage.as_str(),
            reason,
            pid: self.pid,
            target_path: render_diagnostic_path(
                self.display_target.as_deref().unwrap_or(&self.target_path),
            ),
            temp_path: self
                .display_temp
                .as_deref()
                .or(self.temp_path.as_deref())
                .map(|path| render_diagnostic_path(path)),
            source_kind,
            io_kind,
            raw_os_error,
            json_category,
            json_line,
            json_column,
        }
    }
}

impl std::fmt::Debug for SettingsSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let diagnostic = self.safe_diagnostic(SettingsSaveReportSurface::GeneralSettings);
        formatter
            .debug_struct("SettingsSaveError")
            .field("code", &diagnostic.code)
            .field("stage", &diagnostic.stage)
            .field("reason", &diagnostic.reason)
            .field("pid", &diagnostic.pid)
            .field("source_kind", &diagnostic.source_kind)
            .field("io_kind", &diagnostic.io_kind)
            .field("raw_os_error", &diagnostic.raw_os_error)
            .field("json_category", &diagnostic.json_category)
            .field("json_line", &diagnostic.json_line)
            .field("json_column", &diagnostic.json_column)
            .finish()
    }
}

impl std::fmt::Display for SettingsSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.general_settings_diagnostic_code())
    }
}

impl std::error::Error for SettingsSaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.cause {
            SettingsSaveCause::Io(source) => Some(source),
            SettingsSaveCause::Json(source) => Some(source),
            SettingsSaveCause::Semantic(_) => None,
        }
    }
}

impl SettingsSaveReportSurface {
    pub(crate) fn diagnostic_code(self, error: &SettingsSaveError) -> &'static str {
        match self {
            Self::GeneralSettings => error.general_settings_diagnostic_code(),
            Self::TerminalSnapshotCompareAndSet => TERMINAL_SNAPSHOT_SETTING_SAVE_FAILED_CODE,
        }
    }

    pub(crate) fn into_outward_string(self, error: SettingsSaveError) -> String {
        match self {
            Self::GeneralSettings => error.legacy_outward.into_general_string(),
            Self::TerminalSnapshotCompareAndSet => {
                drop(error);
                TERMINAL_SNAPSHOT_SETTING_SAVE_FAILED_CODE.to_string()
            }
        }
    }
}

pub(crate) struct SettingsSaveDiagnostic {
    pub(crate) code: &'static str,
    pub(crate) stage: &'static str,
    pub(crate) reason: &'static str,
    pub(crate) pid: u32,
    pub(crate) target_path: String,
    pub(crate) temp_path: Option<String>,
    pub(crate) source_kind: &'static str,
    pub(crate) io_kind: Option<&'static str>,
    pub(crate) raw_os_error: Option<i32>,
    pub(crate) json_category: Option<&'static str>,
    pub(crate) json_line: Option<usize>,
    pub(crate) json_column: Option<usize>,
}

pub(crate) fn io_error_kind_as_str(kind: std::io::ErrorKind) -> &'static str {
    match kind {
        std::io::ErrorKind::NotFound => "not_found",
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        std::io::ErrorKind::ConnectionRefused => "connection_refused",
        std::io::ErrorKind::ConnectionReset => "connection_reset",
        std::io::ErrorKind::HostUnreachable => "host_unreachable",
        std::io::ErrorKind::NetworkUnreachable => "network_unreachable",
        std::io::ErrorKind::ConnectionAborted => "connection_aborted",
        std::io::ErrorKind::NotConnected => "not_connected",
        std::io::ErrorKind::AddrInUse => "addr_in_use",
        std::io::ErrorKind::AddrNotAvailable => "addr_not_available",
        std::io::ErrorKind::NetworkDown => "network_down",
        std::io::ErrorKind::BrokenPipe => "broken_pipe",
        std::io::ErrorKind::AlreadyExists => "already_exists",
        std::io::ErrorKind::WouldBlock => "would_block",
        std::io::ErrorKind::NotADirectory => "not_a_directory",
        std::io::ErrorKind::IsADirectory => "is_a_directory",
        std::io::ErrorKind::DirectoryNotEmpty => "directory_not_empty",
        std::io::ErrorKind::ReadOnlyFilesystem => "read_only_filesystem",
        std::io::ErrorKind::StaleNetworkFileHandle => "stale_network_file_handle",
        std::io::ErrorKind::InvalidInput => "invalid_input",
        std::io::ErrorKind::InvalidData => "invalid_data",
        std::io::ErrorKind::TimedOut => "timed_out",
        std::io::ErrorKind::WriteZero => "write_zero",
        std::io::ErrorKind::StorageFull => "storage_full",
        std::io::ErrorKind::NotSeekable => "not_seekable",
        std::io::ErrorKind::QuotaExceeded => "quota_exceeded",
        std::io::ErrorKind::FileTooLarge => "file_too_large",
        std::io::ErrorKind::ResourceBusy => "resource_busy",
        std::io::ErrorKind::ExecutableFileBusy => "executable_file_busy",
        std::io::ErrorKind::Deadlock => "deadlock",
        std::io::ErrorKind::CrossesDevices => "crosses_devices",
        std::io::ErrorKind::TooManyLinks => "too_many_links",
        std::io::ErrorKind::InvalidFilename => "invalid_filename",
        std::io::ErrorKind::ArgumentListTooLong => "argument_list_too_long",
        std::io::ErrorKind::Interrupted => "interrupted",
        std::io::ErrorKind::Unsupported => "unsupported",
        std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
        std::io::ErrorKind::OutOfMemory => "out_of_memory",
        std::io::ErrorKind::Other => "other",
        _ => "other",
    }
}

pub(crate) fn json_error_category_as_str(category: serde_json::error::Category) -> &'static str {
    match category {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    }
}

pub(crate) fn render_diagnostic_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    let mut rendered = String::with_capacity(path.len() + 2);
    rendered.push('"');
    for character in path.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '/' | ':') {
            rendered.push(character);
        } else {
            rendered.extend(character.escape_unicode());
        }
    }
    rendered.push('"');
    rendered
}

pub(crate) fn render_settings_save_diagnostic(diagnostic: &SettingsSaveDiagnostic) -> String {
    let raw_os_error = diagnostic
        .raw_os_error
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    let json_line = diagnostic
        .json_line
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    let json_column = diagnostic
        .json_column
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    format!(
        "settings_save_failure code={} stage={} reason={} pid={} target_path={} temp_path={} \
            source_kind={} io_kind={} raw_os_error={} json_category={} json_line={} json_column={}",
        diagnostic.code,
        diagnostic.stage,
        diagnostic.reason,
        diagnostic.pid,
        diagnostic.target_path,
        diagnostic.temp_path.as_deref().unwrap_or("none"),
        diagnostic.source_kind,
        diagnostic.io_kind.unwrap_or("none"),
        raw_os_error,
        diagnostic.json_category.unwrap_or("none"),
        json_line,
        json_column,
    )
}

#[cfg(test)]
pub(crate) type SettingsSaveDiagnosticCapture = Box<dyn Fn(&str)>;

#[cfg(test)]
thread_local! {
    pub(crate) static SETTINGS_SAVE_DIAGNOSTIC_CAPTURE: std::cell::RefCell<
        Option<SettingsSaveDiagnosticCapture>,
    > = std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn capture_settings_save_diagnostic(payload: &str) {
    SETTINGS_SAVE_DIAGNOSTIC_CAPTURE.with(|slot| {
        if let Some(capture) = slot.borrow().as_ref() {
            capture(payload);
        }
    });
}

pub(crate) fn report_settings_save_error(
    error: SettingsSaveError,
    surface: SettingsSaveReportSurface,
) -> String {
    let payload = render_settings_save_diagnostic(&error.safe_diagnostic(surface));
    #[cfg(test)]
    capture_settings_save_diagnostic(&payload);
    log::error!("{}", payload);
    surface.into_outward_string(error)
}

pub(crate) fn read_disk_object_for_write_typed(
    path: &Path,
    stage: SettingsSaveStage,
) -> Result<Option<Map<String, Value>>, SettingsSaveError> {
    read_disk_object_and_contents_for_write_typed(path, stage).map(|disk| disk.map(|(map, _)| map))
}

/// Same read and gates as `read_disk_object_for_write_typed`, plus the raw file
/// contents, so a Preserve save can back up a legacy-shaped file verbatim.
#[allow(clippy::type_complexity)]
pub(crate) fn read_disk_object_and_contents_for_write_typed(
    path: &Path,
    stage: SettingsSaveStage,
) -> Result<Option<(Map<String, Value>, String)>, SettingsSaveError> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => {
            let outward = format!(
                "Failed to read {} for a project-preserving save (aborting to avoid dropping a \
                    project): {source}",
                path.display()
            );
            Err(SettingsSaveError::io(
                stage,
                path,
                None,
                source,
                SettingsSaveLegacyOutward::DiskRead(outward),
            ))
        }
        Ok(contents) => {
            let value: Value = serde_json::from_str(&contents).map_err(|source| {
                let outward = format!(
                    "Refusing to overwrite {}: the existing settings file is not valid JSON \
                        ({source})",
                    path.display()
                );
                SettingsSaveError::json(
                    stage,
                    path,
                    None,
                    source,
                    SettingsSaveLegacyOutward::DiskJson(outward),
                )
            })?;
            match value {
                Value::Object(map) => {
                    if let Err(outward) = validate_non_project_settings(&map) {
                        return Err(SettingsSaveError::semantic(
                            stage,
                            path,
                            None,
                            SettingsSaveReason::DiskSettingsValidationRejected,
                            SettingsSaveLegacyOutward::DiskValidation(outward),
                        ));
                    }
                    Ok(Some((map, contents)))
                }
                _ => {
                    let outward = format!(
                        "Refusing to overwrite {}: the existing settings root is not a JSON object",
                        path.display()
                    );
                    Err(SettingsSaveError::semantic(
                        stage,
                        path,
                        None,
                        SettingsSaveReason::DiskJsonNotObject,
                        SettingsSaveLegacyOutward::DiskJsonNotObject(outward),
                    ))
                }
            }
        }
    }
}

// Diagnostic transport is prepared before kernel ownership. The worker is the
// only place that formats or writes logs; an owner never waits for its sink.
pub(crate) const SETTINGS_LOCK_DEBUG_CAPACITY: usize = 256;
pub(crate) const SETTINGS_LOCK_DEBUG_PATH_BYTES: usize = 4096;
pub(crate) static SETTINGS_LOCK_DEBUG_TRANSPORT: OnceLock<
    Option<std::sync::mpsc::SyncSender<SettingsLockDebugEvent>>,
> = OnceLock::new();
pub(crate) static SETTINGS_LOCK_DEBUG_INSTANCE: OnceLock<uuid::Uuid> = OnceLock::new();
pub(crate) static SETTINGS_LOCK_DEBUG_ATTEMPT: AtomicU64 = AtomicU64::new(0);
pub(crate) static SETTINGS_LOCK_DEBUG_LOST: AtomicU64 = AtomicU64::new(0);

pub(crate) struct SettingsLockDebugContext {
    pub(crate) instance: uuid::Uuid,
    pub(crate) pid: u32,
    pub(crate) attempt: u64,
    pub(crate) operation: &'static str,
    pub(crate) caller: &'static std::panic::Location<'static>,
    pub(crate) thread: std::thread::ThreadId,
    pub(crate) target: String,
    pub(crate) sidecar: String,
    pub(crate) path_truncated: bool,
}

pub(crate) struct SettingsLockDebugEvent {
    pub(crate) context: Arc<SettingsLockDebugContext>,
    pub(crate) phase: &'static str,
    pub(crate) utc: chrono::DateTime<chrono::Utc>,
    pub(crate) thread: std::thread::ThreadId,
    pub(crate) preparation_ms: Option<f64>,
    pub(crate) wait_ms: Option<f64>,
    pub(crate) hold_ms: Option<f64>,
    pub(crate) reason: Option<&'static str>,
    pub(crate) raw_os_error: Option<i32>,
}

pub(crate) struct SettingsLockDebugAttempt {
    pub(crate) context: Arc<SettingsLockDebugContext>,
    pub(crate) sender: std::sync::mpsc::SyncSender<SettingsLockDebugEvent>,
    pub(crate) started: std::time::Instant,
    pub(crate) retry_started: Option<std::time::Instant>,
    pub(crate) acquired_at: Option<std::time::Instant>,
}

// Bound the escaped representation, including quotes, without splitting UTF-8.
// A truncated prefix is intentionally not a complete identity for a lock.
pub(crate) fn settings_lock_debug_path(path: &Path) -> (String, bool) {
    let mut escaped = String::with_capacity(SETTINGS_LOCK_DEBUG_PATH_BYTES);
    escaped.push('"');
    for character in path.to_string_lossy().chars() {
        let piece = match character {
            '"' => "\\\"".to_string(),
            '\\' => "\\\\".to_string(),
            '\n' => "\\n".to_string(),
            '\r' => "\\r".to_string(),
            '\t' => "\\t".to_string(),
            c if c.is_control() => format!("\\u{:04x}", c as u32),
            c => c.to_string(),
        };
        if escaped.len() + piece.len() + 1 > SETTINGS_LOCK_DEBUG_PATH_BYTES {
            escaped.push('"');
            return (escaped, true);
        }
        escaped.push_str(&piece);
    }
    escaped.push('"');
    (escaped, false)
}

pub(crate) fn settings_lock_debug_transport(
    slot: &OnceLock<Option<std::sync::mpsc::SyncSender<SettingsLockDebugEvent>>>,
    enabled: bool,
    spawn: impl FnOnce(std::sync::mpsc::Receiver<SettingsLockDebugEvent>) -> std::io::Result<()>,
) -> Option<std::sync::mpsc::SyncSender<SettingsLockDebugEvent>> {
    // In particular, INFO must not freeze None before a runtime change to DEBUG.
    if !enabled {
        return None;
    }
    slot.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::sync_channel(SETTINGS_LOCK_DEBUG_CAPACITY);
        spawn(receiver).ok().map(|()| sender)
    })
    .clone()
}

pub(crate) fn settings_lock_debug_worker(
    receiver: std::sync::mpsc::Receiver<SettingsLockDebugEvent>,
) {
    for event in receiver {
        #[cfg(test)]
        settings_lock_debug_test_before_emit(&event);
        settings_lock_debug_emit(&event);
        #[cfg(test)]
        settings_lock_debug_test_after_emit(&event);
    }
}

#[cfg(test)]
pub(crate) struct SettingsLockDebugTestHooks {
    pub(crate) gate_phase: &'static str,
    pub(crate) gate_once: std::sync::atomic::AtomicBool,
    pub(crate) gated: std::sync::mpsc::Sender<()>,
    pub(crate) resume: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    pub(crate) processed: std::sync::mpsc::Sender<serde_json::Value>,
}

#[cfg(test)]
pub(crate) static SETTINGS_LOCK_DEBUG_TEST_HOOKS: OnceLock<SettingsLockDebugTestHooks> =
    OnceLock::new();

#[cfg(test)]
thread_local! {
    pub(crate) static SETTINGS_LOCK_DEBUG_TEST_UNLOCK_ERROR: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn settings_lock_debug_test_before_emit(event: &SettingsLockDebugEvent) {
    let Some(hooks) = SETTINGS_LOCK_DEBUG_TEST_HOOKS.get() else {
        return;
    };
    if event.phase == hooks.gate_phase && hooks.gate_once.swap(false, Ordering::Relaxed) {
        hooks.gated.send(()).expect("gate observer disconnected");
        hooks
            .resume
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("gate not resumed by harness");
    }
}

#[cfg(test)]
pub(crate) fn settings_lock_debug_test_after_emit(event: &SettingsLockDebugEvent) {
    if let Some(hooks) = SETTINGS_LOCK_DEBUG_TEST_HOOKS.get() {
        let _ = hooks.processed.send(serde_json::json!({
            "phase": event.phase,
            "instance": event.context.instance.to_string(),
            "pid": event.context.pid,
            "attempt": event.context.attempt,
            "operation": event.context.operation,
            "sidecar": event.context.sidecar,
            "target": event.context.target,
            "reason": event.reason,
            "raw_os_error": event.raw_os_error,
            "wait_ms": event.wait_ms,
            "hold_ms": event.hold_ms,
            "preparation_ms": event.preparation_ms,
            "acquisition_thread": format!("{:?}", event.context.thread),
            "event_thread": format!("{:?}", event.thread),
            "event_utc": event.utc.to_rfc3339(),
            "lost_events": SETTINGS_LOCK_DEBUG_LOST.load(Ordering::Relaxed),
        }));
    }
}

pub(crate) fn settings_lock_debug_emit(event: &SettingsLockDebugEvent) {
    // Reevaluate effective app/CLI filtering after a potentially stalled sink.
    if !log::log_enabled!(target: module_path!(), log::Level::Debug) {
        return;
    }
    let context = &event.context;
    log::debug!(target: module_path!(),
        "settings-lock phase={} pid={} instance={} attempt={} operation={} origin={}:{}:{} \
            acquisition_thread={:?} event_thread={:?} target={} sidecar={} path_truncated={} \
            event_utc={} emitted_utc={} preparation_ms={:?} wait_ms={:?} hold_ms={:?} stage={} \
            reason={:?} raw_os_error={:?} handle_closed={} lost_events={}",
        event.phase, context.pid, context.instance, context.attempt,
        context.operation, context.caller.file(), context.caller.line(),
        context.caller.column(), context.thread, event.thread, context.target,
        context.sidecar, context.path_truncated, event.utc.to_rfc3339(),
        chrono::Utc::now().to_rfc3339(), event.preparation_ms, event.wait_ms,
        event.hold_ms, if event.phase == "release" { "unlock" } else { "lock_acquir\
            e" }, event.reason, event.raw_os_error,
        matches!(event.phase, "release" | "timeout" | "acquire-error"),
        SETTINGS_LOCK_DEBUG_LOST.load(Ordering::Relaxed));
}

impl SettingsLockDebugAttempt {
    pub(crate) fn prepare(
        settings_path: &Path,
        lock_name: &str,
        operation: &'static str,
        caller: &'static std::panic::Location<'static>,
    ) -> Option<Self> {
        let sender = settings_lock_debug_transport(
            &SETTINGS_LOCK_DEBUG_TRANSPORT,
            log::log_enabled!(target: module_path!(), log::Level::Debug),
            |receiver| {
                std::thread::Builder::new()
                    .name("settings-lock-debug".into())
                    .spawn(move || settings_lock_debug_worker(receiver))
                    .map(|_| ())
            },
        )?;
        let attempt = SETTINGS_LOCK_DEBUG_ATTEMPT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .ok()?
            + 1;
        let (target, target_truncated) = settings_lock_debug_path(settings_path);
        let lock_path = settings_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(lock_name);
        let (sidecar, sidecar_truncated) = settings_lock_debug_path(&lock_path);
        let debug = Self {
            context: Arc::new(SettingsLockDebugContext {
                instance: *SETTINGS_LOCK_DEBUG_INSTANCE.get_or_init(uuid::Uuid::new_v4),
                pid: std::process::id(),
                attempt,
                operation,
                caller,
                thread: std::thread::current().id(),
                target,
                sidecar,
                path_truncated: target_truncated || sidecar_truncated,
            }),
            sender,
            started: std::time::Instant::now(),
            retry_started: None,
            acquired_at: None,
        };
        debug.event("acquire-start", None, None);
        Some(debug)
    }

    pub(crate) fn event(
        &self,
        phase: &'static str,
        reason: Option<&'static str>,
        raw_os_error: Option<i32>,
    ) {
        self.event_at(phase, reason, raw_os_error, std::time::Instant::now());
    }

    pub(crate) fn event_at(
        &self,
        phase: &'static str,
        reason: Option<&'static str>,
        raw_os_error: Option<i32>,
        at: std::time::Instant,
    ) {
        let event = SettingsLockDebugEvent {
            context: Arc::clone(&self.context),
            phase,
            utc: chrono::Utc::now(),
            thread: std::thread::current().id(),
            preparation_ms: (phase != "acquire-start").then(|| {
                self.retry_started
                    .unwrap_or(at)
                    .duration_since(self.started)
                    .as_secs_f64()
                    * 1000.0
            }),
            wait_ms: self.retry_started.map(|retry| {
                self.acquired_at
                    .unwrap_or(at)
                    .duration_since(retry)
                    .as_secs_f64()
                    * 1000.0
            }),
            hold_ms: self
                .acquired_at
                .filter(|_| phase == "release")
                .map(|acquired| at.duration_since(acquired).as_secs_f64() * 1000.0),
            reason,
            raw_os_error,
        };
        if self.sender.try_send(event).is_err() {
            // Saturation avoids diagnostic counter wraparound too.
            let _ = SETTINGS_LOCK_DEBUG_LOST.fetch_update(
                Ordering::Relaxed,
                Ordering::Relaxed,
                |value| Some(value.saturating_add(1)),
            );
        }
    }

    pub(crate) fn failure(&self, error: &SettingsSaveError) {
        let (reason, raw) = match &error.cause {
            SettingsSaveCause::Io(source) => ("io_error", source.raw_os_error()),
            SettingsSaveCause::Semantic(reason) => (reason.as_str(), None),
            SettingsSaveCause::Json(_) => ("json_error", None),
        };
        self.event(
            if reason == "lock_timed_out" {
                "timeout"
            } else {
                "acquire-error"
            },
            Some(reason),
            raw,
        );
    }
}

pub(crate) struct SettingsFileLock {
    pub(crate) file: Option<std::fs::File>,
    pub(crate) debug: Option<SettingsLockDebugAttempt>,
}

impl SettingsFileLock {
    #[cfg(test)]
    #[track_caller]
    pub(crate) fn acquire(
        settings_path: &Path,
        timeout: std::time::Duration,
    ) -> Result<Self, SettingsSaveError> {
        Self::acquire_contextual(settings_path, timeout, "test-lock")
    }

    #[track_caller]
    pub(crate) fn acquire_contextual(
        settings_path: &Path,
        timeout: std::time::Duration,
        operation: &'static str,
    ) -> Result<Self, SettingsSaveError> {
        Self::acquire_named(settings_path, SETTINGS_LOCK_FILE_NAME, timeout, operation)
    }

    /// The same acquisition over another stable sidecar beside the settings file.
    #[track_caller]
    pub(crate) fn acquire_named(
        settings_path: &Path,
        lock_name: &str,
        timeout: std::time::Duration,
        operation: &'static str,
    ) -> Result<Self, SettingsSaveError> {
        let mut debug = SettingsLockDebugAttempt::prepare(
            settings_path,
            lock_name,
            operation,
            std::panic::Location::caller(),
        );
        // Inner failure drops every opened file before a diagnostic is enqueued.
        let file = match Self::acquire_file(settings_path, lock_name, timeout, &mut debug) {
            Ok(file) => file,
            Err(error) => {
                if let Some(debug) = &debug {
                    debug.failure(&error);
                }
                return Err(error);
            }
        };
        Ok(Self {
            file: Some(file),
            debug,
        })
    }

    pub(crate) fn acquire_file(
        settings_path: &Path,
        lock_name: &str,
        timeout: std::time::Duration,
        debug: &mut Option<SettingsLockDebugAttempt>,
    ) -> Result<std::fs::File, SettingsSaveError> {
        let parent = settings_path.parent().ok_or_else(|| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::MissingParentDirectory,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        std::fs::create_dir_all(parent).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        crate::path_identity::verify_component_chain(parent).map_err(|_| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockIdentityRejected,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        let lock_path = parent.join(lock_name);
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
            };
            options
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options.open(&lock_path).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        let metadata = file.metadata().map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        if !metadata.is_file() {
            return Err(SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockFileNotRegular,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.nlink() != 1 {
                return Err(SettingsSaveError::semantic(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    SettingsSaveReason::LockLinkCountRejected,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                ));
            }
            std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).map_err(
                |source| {
                    SettingsSaveError::io(
                        SettingsSaveStage::LockAcquire,
                        settings_path,
                        None,
                        source,
                        SettingsSaveLegacyOutward::SettingsLockUnavailable,
                    )
                },
            )?;
        }
        #[cfg(windows)]
        crate::path_identity::verify_regular_file(&lock_path).map_err(|_| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockIdentityRejected,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;

        let retry_started = std::time::Instant::now();
        if let Some(debug) = debug.as_mut() {
            debug.retry_started = Some(retry_started);
        }
        let deadline = retry_started.checked_add(timeout).ok_or_else(|| {
            SettingsSaveError::semantic(
                SettingsSaveStage::LockAcquire,
                settings_path,
                None,
                SettingsSaveReason::LockDeadlineOverflow,
                SettingsSaveLegacyOutward::SettingsLockUnavailable,
            )
        })?;
        loop {
            #[cfg(any(unix, windows))]
            let locked = try_lock_settings_file(&file).map_err(|source| {
                SettingsSaveError::io(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    source,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                )
            })?;
            #[cfg(not(any(unix, windows)))]
            let locked = try_lock_settings_file(&file).map_err(|reason| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    reason,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                )
            })?;
            if locked {
                let acquired_at = std::time::Instant::now();
                if let Some(debug) = debug.as_mut() {
                    debug.acquired_at = Some(acquired_at);
                    debug.event("acquired", None, None);
                }
                return Ok(file);
            }
            if std::time::Instant::now() >= deadline {
                return Err(SettingsSaveError::semantic(
                    SettingsSaveStage::LockAcquire,
                    settings_path,
                    None,
                    SettingsSaveReason::LockTimedOut,
                    SettingsSaveLegacyOutward::SettingsLockUnavailable,
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for SettingsFileLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let unlock_at = std::time::Instant::now();
            let result = unlock_settings_file(&file);
            #[cfg(test)]
            let result = if SETTINGS_LOCK_DEBUG_TEST_UNLOCK_ERROR.with(|flag| flag.replace(false)) {
                Err(std::io::Error::from_raw_os_error(5))
            } else {
                result
            };
            drop(file);
            if let Some(debug) = &self.debug {
                let (reason, raw) = match result {
                    Ok(()) => ("unlock_ok", None),
                    Err(error) => ("unlock_error", error.raw_os_error()),
                };
                debug.event_at("release", Some(reason), raw, unlock_at);
            }
        }
    }
}

#[cfg(unix)]
pub(crate) fn try_lock_settings_file(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    let source = std::io::Error::last_os_error();
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    // Compared rather than matched: `EWOULDBLOCK` and `EAGAIN` are the same
    // value on Linux and macOS, which makes the second pattern unreachable.
    let raw = error.raw_os_error();
    if raw == Some(libc::EWOULDBLOCK) || raw == Some(libc::EAGAIN) {
        Ok(false)
    } else {
        Err(source)
    }
}

#[cfg(unix)]
pub(crate) fn unlock_settings_file(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
pub(crate) fn try_lock_settings_file(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
    let result = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if result != 0 {
        return Ok(true);
    }
    const ERROR_LOCK_VIOLATION: i32 = 33;
    let source = std::io::Error::last_os_error();
    if source.raw_os_error() == Some(ERROR_LOCK_VIOLATION) {
        Ok(false)
    } else {
        Err(source)
    }
}

#[cfg(windows)]
pub(crate) fn unlock_settings_file(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
    if unsafe { UnlockFileEx(file.as_raw_handle(), 0, u32::MAX, u32::MAX, &mut overlapped) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn try_lock_settings_file(_file: &std::fs::File) -> Result<bool, SettingsSaveReason> {
    Err(SettingsSaveReason::LockPlatformUnsupported)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn unlock_settings_file(_file: &std::fs::File) -> std::io::Result<()> {
    Ok(())
}

/// #2348 - serialize one placement component with the same error shape as the
/// whole-object serializer, so a (theoretically impossible) serialization failure
/// aborts the save instead of writing a partial pair.
pub(crate) fn serialize_settings_component<T: serde::Serialize>(
    value: &T,
    path: &Path,
) -> Result<Value, SettingsSaveError> {
    serde_json::to_value(value).map_err(|source| {
        let outward = format!("Failed to serialize settings: {source}");
        SettingsSaveError::json(
            SettingsSaveStage::Serialize,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::Serialize(outward),
        )
    })
}

/// #2348 - central `Preserve` handling for the placement pair. A present disk
/// VALUE is authoritative; a missing key and an explicit `null` both mean "no
/// disk truth" (the same reading `read_project_paths_from_disk` uses for the
/// `Option` fields), so neither erases a caller value synthesized by the
/// load-time `terminalGeometry` -> `mainGeometry` migration. Split out so each
/// key's disk-present/absent x caller-present/absent combinations are testable
/// without building an `AppSettings`.
pub(crate) fn preserve_main_window_placement_keys(
    out: &mut Map<String, Value>,
    disk: &Map<String, Value>,
) {
    for key in [FIELD_MAIN_GEOMETRY, FIELD_MAIN_DISPLAY_STATE] {
        match disk.get(key) {
            None | Some(Value::Null) => {}
            Some(value) => {
                out.insert(key.to_string(), value.clone());
            }
        }
    }
}

pub(crate) fn preflight_duplicate_agents(
    settings: &AppSettings,
    path: &Path,
    disk: Option<&Map<String, Value>>,
    startup: Option<&StartupAgentRows>,
) -> Result<Option<Value>, SettingsSaveError> {
    let rejected = |reason: String| {
        SettingsSaveError::semantic(
            SettingsSaveStage::PreserveDiskGate,
            path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    };
    let sidecar = read_agents_instance_file(&agents_instance_path(path)).map_err(&rejected)?;
    let sidecar_owner = sidecar
        .as_ref()
        .is_some_and(|groups| groups.contains_key("agents"));
    let raw = sidecar
        .as_ref()
        .and_then(|groups| groups.get("agents"))
        .or_else(|| disk.and_then(|disk| disk.get("agents")))
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    if let Some(context) = startup {
        context
            .validate_source(&raw, sidecar_owner)
            .map_err(&rejected)?;
    }
    let mut ids = HashSet::new();
    let has_duplicates = raw
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .any(|id| !ids.insert(id));
    let mut candidate_ids = HashSet::new();
    let newly_ambiguous = settings
        .agents
        .iter()
        .any(|agent| !candidate_ids.insert(agent.id.as_str()) && ids.contains(agent.id.as_str()));
    // Complete the set when the duplicate scan short-circuited.
    if !has_duplicates && !newly_ambiguous {
        return Ok(None);
    }
    let mut candidate = serialize_settings_component(settings, path)?;
    let object = candidate
        .as_object_mut()
        .expect("serialized settings object");
    write_identity_rows(object, &settings.local_overlay_state);
    settings.local_overlay_state.restore_base(object);
    let before = serde_json::json!({"agents":raw});
    retain_configuration_commands_with_startup(
        &mut candidate,
        &before,
        "id",
        false,
        true,
        false,
        startup,
    )
    .map_err(rejected)?;
    Ok(candidate.get("agents").cloned())
}

/// The #1077 project-aware atomic writer. Builds the output object per `mode`,
/// writes it atomically, then re-decodes the exact written value so the returned
/// `AppSettings` carries fresh runtime projections + hidden state (never the
/// possibly-stale caller state). See §4.2/§4.3.
pub(crate) fn save_settings_value(
    settings: &AppSettings,
    path: &Path,
    mode: ProjectWriteMode,
) -> Result<AppSettings, SettingsSaveError> {
    save_settings_value_with_startup(settings, path, mode, None)
}

pub(crate) fn save_settings_value_with_startup(
    settings: &AppSettings,
    path: &Path,
    mode: ProjectWriteMode,
    startup: Option<&StartupAgentRows>,
) -> Result<AppSettings, SettingsSaveError> {
    let _lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "save_settings_value",
    )?;
    #[cfg(test)]
    SETTINGS_PRESERVE_LOCK_HOOK.with(|hook| {
        if let Some(callback) = hook.borrow().as_ref() {
            callback(path);
        }
    });
    save_settings_value_locked_with_startup(
        settings,
        path,
        mode,
        TerminalSnapshotGateWriteMode::Preserve,
        MainWindowPlacementWriteMode::Preserve,
        startup,
    )
}

/// Locked half of the project-aware writer. Assumes `settings.json.lock` is
/// already held by the caller (both production entry points acquire it before
/// reaching here), and performs #2058 backup rotation after a successful write.
pub(crate) fn save_settings_value_locked(
    settings: &AppSettings,
    path: &Path,
    mode: ProjectWriteMode,
    terminal_snapshot_gate_mode: TerminalSnapshotGateWriteMode,
    main_window_placement_mode: MainWindowPlacementWriteMode,
) -> Result<AppSettings, SettingsSaveError> {
    save_settings_value_locked_with_startup(
        settings,
        path,
        mode,
        terminal_snapshot_gate_mode,
        main_window_placement_mode,
        None,
    )
}

pub(crate) fn restore_protected_agents_instance_groups(
    out: &mut Map<String, Value>,
    disk: Option<&Map<String, Value>>,
    overlay: &LocalSettingsOverlay,
    agents_groups: &Map<String, Value>,
) {
    let identity_rows = out
        .get("codingAgentProfiles")
        .and_then(|profiles| profiles.get("identityByAgent"))
        .cloned();
    for key in AGENTS_INSTANCE_KEYS {
        if !overlay.owns_top_level(key) || agents_groups.contains_key(key) {
            continue;
        }
        match disk.and_then(|disk| disk.get(key)) {
            Some(raw) => {
                out.insert(key.to_owned(), raw.clone());
            }
            None => {
                out.remove(key);
            }
        }
        // Only legacy MAIN identities retain the existing #2450 writer output.
        // All other fields of a protected profiles group remain raw disk values.
        if key == "codingAgentProfiles" {
            if out
                .get(key)
                .and_then(|profiles| profiles.get("profileBindingVersion"))
                == Some(&Value::from(1))
            {
                continue;
            }
            // No projected row grants deletion of raw identities.
            if let Some(rows) = &identity_rows {
                let profiles = out
                    .entry(key.to_owned())
                    .or_insert_with(|| Value::Object(Map::new()));
                let physical_ids: HashSet<_> = disk
                    .and_then(|disk| disk.get("agents"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|row| row.get("id").and_then(Value::as_str))
                    .collect();
                let identities = profiles
                    .as_object_mut()
                    .expect("validated profiles object")
                    .entry("identityByAgent")
                    .or_insert_with(|| serde_json::json!({}));
                if let (Some(identities), Some(rows)) =
                    (identities.as_object_mut(), rows.as_object())
                {
                    for (id, value) in rows {
                        if physical_ids.contains(id.as_str()) {
                            identities.insert(id.clone(), value.clone());
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn retain_untransferred_registered_main_groups(
    out: &mut Map<String, Value>,
    disk: Option<&Map<String, Value>>,
    published: &Map<String, Value>,
    sidecar_before: Option<&Map<String, Value>>,
    overlay: &LocalSettingsOverlay,
    path: &Path,
) {
    for key in AGENTS_INSTANCE_KEYS {
        let Some(destination) = published.get(key) else {
            continue;
        };
        let raw = disk.and_then(|map| map.get(key));
        let selected_main = !sidecar_before.is_some_and(|map| map.contains_key(key));
        if !overlay.owns_top_level(key) && (selected_main || raw == Some(destination)) {
            // Called only after the sidecar publisher confirmed this packet.
            out.remove(key);
        } else if let Some(raw) = raw {
            if !overlay.owns_top_level(key) && raw != destination {
                log::warn!(
                    "[profiles] {}: retaining unselected shadowed {key}; raw value differs \
                    from registered sidecar",
                    path.display()
                );
            }
            out.insert(key.into(), raw.clone());
        } else {
            out.remove(key);
        }
    }
}

pub(crate) type RegisteredMainPreparedGroups<'a> = (
    Option<&'a Value>,
    Option<&'a Value>,
    Option<&'a Map<String, Value>>,
    &'a Path,
);

pub(crate) fn prepare_registered_main_agent_groups(
    mut out: Map<String, Value>,
    settings: &AppSettings,
    disk: Option<&Map<String, Value>>,
    agents_groups: &Map<String, Value>,
    profiles_in_sidecar: bool,
    prepared_groups: RegisteredMainPreparedGroups<'_>,
    mode: &ProjectWriteMode,
) -> Result<Map<String, Value>, String> {
    let (prepared_profiles, prepared_duplicate_agents, sidecar_before, path) = prepared_groups;
    #[cfg(test)]
    if matches!(mode, ProjectWriteMode::Reconcile { .. }) {
        for key in AGENTS_INSTANCE_KEYS {
            match disk.and_then(|disk| disk.get(key)) {
                Some(raw) if !agents_groups.contains_key(key) => {
                    out.insert(key.into(), raw.clone());
                }
                _ => {
                    out.remove(key);
                }
            }
        }
        return Ok(out);
    }
    #[cfg(not(test))]
    let _ = mode;
    retain_untransferred_registered_main_groups(
        &mut out,
        disk,
        agents_groups,
        sidecar_before,
        &settings.local_overlay_state,
        path,
    );
    if profiles_in_sidecar
        && !settings
            .local_overlay_state
            .owns_top_level("codingAgentProfiles")
        && disk.is_none_or(|main| !main.contains_key("codingAgentProfiles"))
        && prepared_profiles.is_some_and(|prepared| prepared.get("codingAgentProfiles").is_none())
    {
        // Ambiguous agents without a physical profile group do not grant
        // authority to materialize the caller's repaired/default profile view.
        out.remove("codingAgentProfiles");
    }
    if !profiles_in_sidecar {
        if let Some(prepared) = prepared_profiles {
            if let Some(profiles) = prepared.get("codingAgentProfiles") {
                out.insert("codingAgentProfiles".into(), profiles.clone());
            }
            if !agents_groups.contains_key("agents") {
                if let Some(rows) = prepared.get("agents") {
                    out.insert("agents".into(), rows.clone());
                }
            }
        }
    }

    // Partial profile ownership can keep these groups in settings.json instead
    // of the sidecar. Retain its locked legacy leaves after raw overlay restore;
    // complete overlay-owned groups stay outside this writer's authority.
    if let Some(disk) = disk {
        let profiles_owned = settings
            .local_overlay_state
            .owned_paths()
            .iter()
            .any(|path| path.len() == 1 && path[0] == "codingAgentProfiles");
        let mut retained = Value::Object(out);
        if retained.get("agents").is_some() {
            if let Some(prepared) = prepared_duplicate_agents {
                retained["agents"] = prepared.clone();
            }
        }
        retain_configuration_commands(
            &mut retained,
            &Value::Object(disk.clone()),
            "id",
            false,
            !settings.local_overlay_state.owns_top_level("agents")
                && !agents_groups.contains_key("agents")
                && prepared_duplicate_agents.is_none(),
            !profiles_owned && !agents_groups.contains_key("codingAgentProfiles"),
        )?;
        out = retained
            .as_object()
            .expect("retained settings object")
            .clone();
    }

    // A synthesized legacy state (direct-constructed AppSettings) has no dirty
    // repair, so the eligible branches above never fire for it; its groups are
    // written verbatim from the live settings/disk, matching pre-#1077 behavior.
    // Restore complete protected groups after every retention/repair stage.
    // Sidecar key presence, including empty values, takes precedence over main.
    restore_protected_agents_instance_groups(
        &mut out,
        disk,
        &settings.local_overlay_state,
        agents_groups,
    );
    if !profiles_in_sidecar {
        restore_codex_home_carrier(
            &mut out,
            disk.and_then(|disk| disk.get("codingAgentProfiles")),
        );
    }
    retain_untransferred_registered_main_groups(
        &mut out,
        disk,
        agents_groups,
        sidecar_before,
        &settings.local_overlay_state,
        path,
    );
    Ok(out)
}

pub(crate) fn apply_registered_main_disk_fields(
    out: &mut Map<String, Value>,
    disk: Option<&Map<String, Value>>,
    path: &Path,
    disk_gate_stage: SettingsSaveStage,
    terminal_snapshot_gate_mode: TerminalSnapshotGateWriteMode,
    main_window_placement_mode: MainWindowPlacementWriteMode,
) -> Result<(), SettingsSaveError> {
    if let Some(raw) = disk {
        // Use Serde's existing field table without requiring the physical main
        // object to contain groups legitimately owned by the sidecar.
        let known = source_general_field_names();
        for (key, value) in raw {
            if !known.contains(&key.as_str()) {
                out.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
    replace_project_projection(out, &Map::new());

    // #1173: a whole-settings writer cannot opt in or re-enable an explicit
    // value, and a present malformed gate is an error. An absent file or a
    // valid object with an absent legacy key materializes true: the default-on
    // phase deliberately reverses the shipped absent-key rule. Only the
    // dedicated CAS uses Explicit.
    let terminal_snapshots_enabled = match terminal_snapshot_gate_mode {
        TerminalSnapshotGateWriteMode::Explicit(enabled) => enabled,
        TerminalSnapshotGateWriteMode::Preserve => match disk {
            Some(disk) => match disk.get(FIELD_TERMINAL_SNAPSHOTS_ENABLED) {
                Some(Value::Bool(enabled)) => *enabled,
                None => true,
                Some(_) => {
                    return Err(SettingsSaveError::semantic(
                        disk_gate_stage,
                        path,
                        None,
                        SettingsSaveReason::TerminalSnapshotSettingInvalid,
                        SettingsSaveLegacyOutward::TerminalSnapshotSettingInvalid(
                            "terminal_snapshot_setting_invalid".to_string(),
                        ),
                    ));
                }
            },
            None => true,
        },
    };
    out.insert(
        FIELD_TERMINAL_SNAPSHOTS_ENABLED.to_string(),
        Value::Bool(terminal_snapshots_enabled),
    );

    // #2348: main-window placement is owned by the narrow placement command.
    // `Preserve` never regresses it: for each key independently, a PRESENT disk
    // value is authoritative, and an absent disk key leaves the caller value
    // alone. An absent legacy `mainGeometry`
    // must not erase the value the load-time migration synthesized in memory.
    // `Explicit` writes the pair exactly as the placement command received it.
    // Runs before `restore_base`, which stays the last writer over the base file.
    match main_window_placement_mode {
        MainWindowPlacementWriteMode::Preserve => {
            if let Some(disk) = disk {
                preserve_main_window_placement_keys(out, disk);
            }
        }
        MainWindowPlacementWriteMode::Explicit {
            geometry,
            display_state,
        } => {
            out.insert(
                FIELD_MAIN_GEOMETRY.to_string(),
                serialize_settings_component(&geometry, path)?,
            );
            out.insert(
                FIELD_MAIN_DISPLAY_STATE.to_string(),
                serialize_settings_component(&display_state, path)?,
            );
        }
    }

    Ok(())
}

pub(crate) fn save_settings_value_locked_with_startup(
    settings: &AppSettings,
    path: &Path,
    mode: ProjectWriteMode,
    terminal_snapshot_gate_mode: TerminalSnapshotGateWriteMode,
    main_window_placement_mode: MainWindowPlacementWriteMode,
    startup: Option<&StartupAgentRows>,
) -> Result<AppSettings, SettingsSaveError> {
    let disk_gate_stage = match &mode {
        ProjectWriteMode::Preserve => SettingsSaveStage::PreserveDiskGate,
        #[cfg(test)]
        ProjectWriteMode::Reconcile { .. } => SettingsSaveStage::ReconcileDiskGate,
    };
    let initial_disk = read_disk_object_for_write_typed(path, disk_gate_stage)?;
    let reject_profiles = |reason: String| {
        SettingsSaveError::semantic(
            disk_gate_stage,
            path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    };
    let profile_gate = capture_registered_profile_gate(path).map_err(&reject_profiles)?;
    let physical_sidecar =
        read_agents_instance_file(&agents_instance_path(path)).map_err(&reject_profiles)?;
    // Preparation and ownership use the original bytes checked by this gate,
    // rather than independently captured generations of the two files.
    let captured_main =
        captured_registered_profile_object(&profile_gate, path).map_err(&reject_profiles)?;
    let captured_sidecar =
        captured_registered_profile_object(&profile_gate, &agents_instance_path(path))
            .map_err(&reject_profiles)?;
    if initial_disk != captured_main || physical_sidecar != captured_sidecar {
        return Err(reject_profiles(format!(
            "staleProfileSource: {}",
            path.display()
        )));
    }
    let initial_disk = captured_main;
    let physical_sidecar = captured_sidecar;
    let profiles_in_sidecar = physical_sidecar.as_ref().is_some_and(|map| {
        map.contains_key("codingAgentProfiles")
            || (map.contains_key("agents")
                && initial_disk
                    .as_ref()
                    .is_none_or(|main| !main.contains_key("codingAgentProfiles")))
    });
    let profiles_unprotected = !settings
        .local_overlay_state
        .owns_top_level("codingAgentProfiles");
    let profile_before = selected_registered_profile_owner(
        &initial_disk.clone().unwrap_or_default(),
        physical_sidecar.as_ref(),
        profiles_unprotected,
    );
    let directional_split = profiles_in_sidecar
        && physical_sidecar
            .as_ref()
            .is_some_and(|map| !map.contains_key("agents"))
        && profile_before.pointer("/codingAgentProfiles/profileBindingVersion")
            == Some(&Value::from(1));
    let consolidate_legacy = profiles_unprotected
        && profile_before
            .get("codingAgentProfiles")
            .is_some_and(|profiles| profiles.get("profileBindingVersion").is_none())
        && physical_profile_rows(&profile_before, true).is_ok();
    let profiles_in_sidecar = profiles_in_sidecar
        || (profiles_unprotected
            && profile_before
                .get("codingAgentProfiles")
                .is_none_or(|profiles| profiles.get("profileBindingVersion").is_none()));
    // Prove whole-row correspondence before any ID-keyed copying or conversion.
    let prepared_duplicate_agents = if matches!(mode, ProjectWriteMode::Preserve)
        && !settings.local_overlay_state.owns_top_level("agents")
    {
        preflight_duplicate_agents(settings, path, initial_disk.as_ref(), startup)?
    } else {
        None
    };
    let prepared_profiles = if matches!(mode, ProjectWriteMode::Preserve) {
        let (profile_incoming, owner_has_active_agents, duplicate_rows) =
            prepare_registered_profile_incoming(
                (settings, path),
                &profile_before,
                &physical_sidecar,
                &initial_disk,
                &profile_gate,
                (
                    directional_split,
                    profiles_unprotected,
                    profiles_in_sidecar,
                    consolidate_legacy,
                ),
                prepared_duplicate_agents.as_ref(),
            )
            .map_err(&reject_profiles)?;
        let mut prepared_profiles = prepare_registered_profile_owner(
            settings,
            path,
            &profile_before,
            &profile_incoming,
            profiles_in_sidecar,
            startup,
            (owner_has_active_agents && prepared_duplicate_agents.is_some()) || duplicate_rows,
        )
        .map_err(&reject_profiles)?;
        // Original main bytes are backed up below before any cleanup publish.
        if !physical_sidecar
            .as_ref()
            .is_some_and(|map| map.contains_key("codingAgentProfiles"))
            && owner_has_active_agents
        {
            if let Some(profiles) = prepared_profiles
                .as_mut()
                .and_then(|prepared| prepared.get_mut("codingAgentProfiles"))
                .and_then(Value::as_object_mut)
            {
                for key in ["letters", "matrix", "agentDefaults"] {
                    profiles.remove(key);
                }
            }
        }
        if let Some(prepared) = &prepared_profiles {
            validate_physical_identity_delta(
                profile_before.as_object().unwrap(),
                prepared.as_object().unwrap(),
                !settings.local_overlay_state.owns_top_level("agents"),
                !directional_split,
            )
            .map_err(&reject_profiles)?;
            if directional_split {
                validate_directional_prepared_identity(&profile_before, prepared)
                    .map_err(&reject_profiles)?;
            }
        }
        if directional_split || settings.local_overlay_state.owns_top_level("agents") {
            if let Some(prepared) = prepared_profiles.as_mut().and_then(Value::as_object_mut) {
                prepared.remove("agents");
            }
        }
        prepared_profiles
    } else {
        None
    };
    // Validate the main-row packet as well before any migration, backup or
    // sidecar publisher. The real publishers below retain their existing order.
    prevalidate_registered_main_packet(
        (settings, path),
        &initial_disk,
        &physical_sidecar,
        prepared_profiles.as_ref(),
        prepared_duplicate_agents.as_ref(),
        (
            &mode,
            disk_gate_stage,
            terminal_snapshot_gate_mode,
            &main_window_placement_mode,
            directional_split,
            profiles_in_sidecar,
        ),
    )?;
    recheck_registered_profile_gate(&profile_gate).map_err(&reject_profiles)?;
    ensure_project_paths_migrated_at_locked_path(path, &mut |_, _| Ok(()))?;
    let selected_projection = select_project_source_at_locked_path(path)?.projection;
    let base = production_instance_base();
    let disk_gate_stage = match &mode {
        ProjectWriteMode::Preserve => SettingsSaveStage::PreserveDiskGate,
        #[cfg(test)]
        ProjectWriteMode::Reconcile { .. } => SettingsSaveStage::ReconcileDiskGate,
    };
    let disk_read = read_disk_object_and_contents_for_write_typed(path, disk_gate_stage)?;
    backup_legacy_profiles_before_write(
        path,
        disk_gate_stage,
        &mode,
        disk_read.as_ref().map(|(map, _)| map),
        disk_read.as_ref().map(|(_, contents)| contents.as_str()),
    )?;
    // #2058: the exact bytes this save is about to replace, captured before the
    // tuple is reduced to its object half.
    let previous_contents: Option<String> = disk_read.as_ref().map(|(_, c)| c.clone());
    let disk = disk_read.map(|(map, _)| map);
    let serialize_object = || -> Result<Map<String, Value>, SettingsSaveError> {
        match serde_json::to_value(settings).map_err(|source| {
            let outward = format!("Failed to serialize settings: {source}");
            SettingsSaveError::json(
                SettingsSaveStage::Serialize,
                path,
                None,
                source,
                SettingsSaveLegacyOutward::Serialize(outward),
            )
        })? {
            Value::Object(m) => Ok(m),
            _ => Err(SettingsSaveError::semantic(
                SettingsSaveStage::Serialize,
                path,
                None,
                SettingsSaveReason::SerializedJsonNotObject,
                SettingsSaveLegacyOutward::SerializedJsonNotObject(
                    "settings did not serialize to a JSON object".to_string(),
                ),
            )),
        }
    };

    // Base object: Preserve starts from the incoming settings (non-project edits
    // apply); Reconcile is project-only and starts from the fresh disk object
    // (or the live settings when the file is absent).
    let mut out: Map<String, Value> = match mode {
        ProjectWriteMode::Preserve => serialize_object()?,
        #[cfg(test)]
        ProjectWriteMode::Reconcile { .. } => match &disk {
            Some(m) => m.clone(),
            None => serialize_object()?,
        },
    };

    // E2432: only the dedicated preference publisher changes menu choices.
    out.remove("agentFileSourceByMenu");
    if let Some(preferences) = disk
        .as_ref()
        .and_then(|disk| disk.get("agentFileSourceByMenu"))
    {
        out.insert("agentFileSourceByMenu".into(), preferences.clone());
    }
    apply_registered_main_disk_fields(
        &mut out,
        disk.as_ref(),
        path,
        disk_gate_stage,
        terminal_snapshot_gate_mode,
        main_window_placement_mode,
    )?;

    // #1737: the effective view the caller must adopt for overlay-owned paths. `out`
    // is not a usable source: in Reconcile mode it was seeded from the disk object,
    // which already holds base values. Skipped entirely when no overlay is in force,
    // which is what keeps the no-overlay write path byte-identical.
    let effective: Option<Map<String, Value>> = if settings.local_overlay_state.is_empty() {
        None
    } else {
        Some(serialize_object()?)
    };
    // #1737: last writer over `out`, so no earlier stage can leak an overlay value
    // into the base file. Runs in both project write modes: Reconcile seeds `out`
    // from the live settings when the file is absent. The overlay-ineligible sets
    // (D7a, D7b) already exclude every key the #1077 and #1173 stages touch.
    if matches!(mode, ProjectWriteMode::Preserve) {
        write_identity_rows(&mut out, &settings.local_overlay_state);
    }
    settings.local_overlay_state.restore_base(&mut out);

    // #2716 (B3): the agents file is written first; only the keys it now holds
    // leave `out`, in both modes. A failure returns before the settings write.
    let agents_groups = save_agents_instance_file(
        settings,
        path,
        &mode,
        #[cfg(test)]
        disk.as_ref(),
        &out,
        disk_gate_stage,
        (
            prepared_duplicate_agents.as_ref(),
            prepared_profiles.as_ref().filter(|_| profiles_in_sidecar),
            Some(profiles_in_sidecar),
        ),
    )?;
    out = prepare_registered_main_agent_groups(
        out,
        settings,
        disk.as_ref(),
        &agents_groups,
        profiles_in_sidecar,
        (
            prepared_profiles.as_ref(),
            prepared_duplicate_agents.as_ref(),
            physical_sidecar.as_ref(),
            path,
        ),
        &mode,
    )
    .map_err(|reason| {
        SettingsSaveError::semantic(
            disk_gate_stage,
            path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    })?;
    let value = Value::Object(out);
    let written_bytes = write_value_atomic(&value, path)?;
    if let Some(previous) = previous_contents {
        if previous.as_bytes() != written_bytes.as_slice() {
            rotate_settings_backups(path, previous.as_bytes());
        }
    }

    let mut written_value = value;
    replace_project_projection(written_value.as_object_mut().unwrap(), &selected_projection);
    // #2716 (B3): the re-decode sees what a load would, the agents file's groups
    // in the base object, under the overlay reapplied below.
    let physical_groups = agents_groups.keys().cloned().collect::<Vec<_>>();
    let profile_owner = selected_registered_profile_owner(
        written_value.as_object().expect("written settings object"),
        Some(&agents_groups),
        !settings
            .local_overlay_state
            .owns_top_level("codingAgentProfiles"),
    );
    let profile_owner_view = registered_profiles_for_read(&profile_owner);
    let base_profiles = profile_owner.get("codingAgentProfiles").cloned();
    restore_agents_groups_for_redecode(&mut written_value, agents_groups);
    let identity_physical =
        coding_agent_identity_with_physical_overlay(written_value.clone(), path, &physical_groups);
    if let Some(profiles) = profile_owner_view.get("codingAgentProfiles") {
        written_value["codingAgentProfiles"] = profiles.clone();
    }
    // #1737: disk holds the base, memory holds the effective value. Without this the
    // caller would adopt base values for every overlay-owned key on the first save.
    if let (Some(effective), Value::Object(object)) = (&effective, &mut written_value) {
        settings.local_overlay_state.reapply_from(effective, object);
    }
    restore_codex_home_carrier(
        written_value.as_object_mut().unwrap(),
        base_profiles.as_ref(),
    );
    let fresh_state = apply_project_decode_to_value(
        &mut written_value,
        base.as_deref(),
        &projects::FsCandidateResolver,
    );
    let mut written: AppSettings = serde_json::from_value(written_value).map_err(|source| {
        let outward = format!("Failed to re-decode written settings: {source}");
        SettingsSaveError::json(
            SettingsSaveStage::ReDecode,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::ReDecode(outward),
        )
    })?;
    refresh_agent_id_hashes(&mut written.agents);
    refresh_pair_id_hashes(&written.agents, &mut written.coding_agent_profiles);
    written.project_path_state = Arc::new(fresh_state);
    written.coding_agent_identity_physical = Some(Arc::new(identity_physical));
    written.local_overlay_state = settings.local_overlay_state.clone();
    repair_loaded_response_close_settings(&mut written, &path.display().to_string());
    repair_loaded_room_idle_confirmation_settings(&mut written, &path.display().to_string());
    Ok(written)
}

pub(crate) fn prevalidate_registered_main_packet(
    owner: (&AppSettings, &Path),
    initial_disk: &Option<Map<String, Value>>,
    physical_sidecar: &Option<Map<String, Value>>,
    prepared_profiles: Option<&Value>,
    prepared_duplicate_agents: Option<&Value>,
    controls: (
        &ProjectWriteMode,
        SettingsSaveStage,
        TerminalSnapshotGateWriteMode,
        &MainWindowPlacementWriteMode,
        bool,
        bool,
    ),
) -> Result<(), SettingsSaveError> {
    let (settings, path) = owner;
    let (
        mode,
        disk_gate_stage,
        terminal_snapshot_gate_mode,
        main_window_placement_mode,
        directional_split,
        profiles_in_sidecar,
    ) = controls;
    let reject_profiles = |reason: String| {
        SettingsSaveError::semantic(
            disk_gate_stage,
            path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    };
    if prepared_profiles.is_some()
        && (directional_split || settings.local_overlay_state.owns_top_level("agents"))
    {
        let mut main_packet = serde_json::to_value(settings)
            .map_err(|error| reject_profiles(error.to_string()))?
            .as_object()
            .ok_or_else(|| reject_profiles("invalidSettingsObject".into()))?
            .clone();
        apply_registered_main_disk_fields(
            &mut main_packet,
            initial_disk.as_ref(),
            path,
            disk_gate_stage,
            terminal_snapshot_gate_mode,
            main_window_placement_mode.clone(),
        )?;
        settings.local_overlay_state.restore_base(&mut main_packet);
        let mut sidecar_packet = physical_sidecar.clone().unwrap_or_default();
        if let Some(profiles) =
            prepared_profiles.and_then(|prepared| prepared.get("codingAgentProfiles"))
        {
            sidecar_packet.insert("codingAgentProfiles".into(), profiles.clone());
        }
        let main_packet = prepare_registered_main_agent_groups(
            main_packet,
            settings,
            initial_disk.as_ref(),
            &sidecar_packet,
            profiles_in_sidecar,
            (
                prepared_profiles,
                prepared_duplicate_agents,
                physical_sidecar.as_ref(),
                path,
            ),
            mode,
        )
        .map_err(&reject_profiles)?;
        if !settings.local_overlay_state.owns_top_level("agents") {
            validate_physical_identity_delta(
                &initial_disk.clone().unwrap_or_default(),
                &main_packet,
                true,
                false,
            )
            .map_err(&reject_profiles)?;
        }
    }
    Ok(())
}

pub(crate) fn backup_legacy_profiles_before_write(
    path: &Path,
    disk_gate_stage: SettingsSaveStage,
    mode: &ProjectWriteMode,
    map: Option<&Map<String, Value>>,
    contents: Option<&str>,
) -> Result<(), SettingsSaveError> {
    // #2015: Preserve builds from the struct, which ignores legacy pre-v2
    // profile fields; keep the original once before any save drops them.
    if matches!(mode, ProjectWriteMode::Preserve) {
        if let (Some(map), Some(contents)) = (map, contents) {
            if legacy_profiles_shape_present(map) {
                write_pre_384_v1_backup(path, contents).map_err(|e| {
                    SettingsSaveError::io(
                        disk_gate_stage,
                        path,
                        None,
                        std::io::Error::other(e.clone()),
                        SettingsSaveLegacyOutward::DiskRead(format!(
                            "Refusing to overwrite {}: legacy settings backup failed ({e})",
                            path.display()
                        )),
                    )
                })?;
            }
        }
    }
    Ok(())
}

pub(crate) fn prepare_registered_profile_incoming(
    owner: (&AppSettings, &Path),
    profile_before: &Value,
    physical_sidecar: &Option<Map<String, Value>>,
    initial_disk: &Option<Map<String, Value>>,
    profile_gate: &[RegisteredProfileDocument],
    flags: (bool, bool, bool, bool),
    prepared_duplicate_agents: Option<&Value>,
) -> Result<(Value, bool, bool), String> {
    let (settings, path) = owner;
    let (directional_split, profiles_unprotected, profiles_in_sidecar, consolidate_legacy) = flags;
    let mut profile_incoming = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    validate_registered_overlay_profile_edits(
        settings,
        profile_before,
        physical_sidecar,
        initial_disk,
        path,
        profile_gate,
    )?;
    settings
        .local_overlay_state
        .restore_base(profile_incoming.as_object_mut().unwrap());
    // The directional view and initial preparation use selected physical
    // rows. Their borrowed rows are stripped before sidecar publication.
    let owner_has_active_agents = if directional_split
        || (profiles_unprotected
            && profile_before.pointer("/codingAgentProfiles/profileBindingVersion")
                != Some(&Value::from(1)))
    {
        true
    } else if profiles_in_sidecar {
        physical_sidecar
            .as_ref()
            .is_some_and(|map| map.contains_key("agents"))
    } else {
        !physical_sidecar
            .as_ref()
            .is_some_and(|map| map.contains_key("agents"))
    };
    if !owner_has_active_agents {
        if profile_before.pointer("/codingAgentProfiles/profileBindingVersion")
            == Some(&Value::from(1))
        {
            let selected_rows = physical_sidecar
                .as_ref()
                .and_then(|map| map.get("agents"))
                .or_else(|| initial_disk.as_ref().and_then(|map| map.get("agents")));
            validate_split_canonical_profile_edits(settings, profile_before, selected_rows)?;
        }
        match profile_before.get("agents") {
            Some(rows) => {
                profile_incoming["agents"] = rows.clone();
            }
            None => {
                profile_incoming.as_object_mut().unwrap().remove("agents");
            }
        }
    }
    if settings.local_overlay_state.owns_top_level("agents") {
        match profile_before.get("agents") {
            Some(rows) => profile_incoming["agents"] = rows.clone(),
            None => {
                profile_incoming.as_object_mut().unwrap().remove("agents");
            }
        }
    }
    if consolidate_legacy {
        retain_selected_legacy_profile_aliases(
            profile_before,
            profile_before,
            &mut profile_incoming,
        )?;
    }
    if owner_has_active_agents {
        if let Some(rows) = prepared_duplicate_agents {
            profile_incoming["agents"] = rows.clone();
        } else {
            retain_unknown_registered_agent_fields(profile_before, &mut profile_incoming)?;
        }
    }
    let mut incoming_ids = HashSet::new();
    let duplicate_rows = profile_incoming
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .any(|id| !incoming_ids.insert(id));
    if directional_split {
        restrict_split_profile_mutation_rows(settings, profile_before, &mut profile_incoming)?;
    }
    Ok((profile_incoming, owner_has_active_agents, duplicate_rows))
}

pub(crate) fn validate_directional_prepared_identity(
    profile_before: &Value,
    prepared: &Value,
) -> Result<(), String> {
    // A BASE fork carries its old complete set, including historical
    // duplicate profiles. Compare profile edits under the proposed
    // row grouping; configuration admission above stays independent.
    let mut previous = project_physical_profiles(profile_before, true)?;
    let old_rows = physical_profile_rows(profile_before, true)?;
    let mut target_baseline = profile_before.clone();
    target_baseline["agents"] = prepared["agents"].clone();
    let target_baseline = project_physical_profiles(&target_baseline, true)?;
    for id in physical_profile_rows(prepared, true)?.keys() {
        if !old_rows.contains_key(id) {
            for key in ["profilesByAgent", "profileLabelsByAgent"] {
                previous["codingAgentProfiles"][key][id] =
                    target_baseline["codingAgentProfiles"][key][id].clone();
            }
        }
    }
    previous["agents"] = prepared["agents"].clone();
    let proposed = project_physical_profiles(prepared, true)?;
    let old = physical_identity_violations(previous.as_object().unwrap(), false, true)?;
    let new = physical_identity_violations(proposed.as_object().unwrap(), false, true)?;
    for (signature, (count, error)) in new {
        if count > old.get(&signature).map_or(0, |(count, _)| *count) {
            return Err(error.to_string());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiskProjectLists {
    pub project_paths: Vec<String>,
    pub project_path: Option<String>,
    pub archived_project_paths: Option<Vec<String>>,
}

/// Read-only privileged source capture. The shared settings lock covers both
/// objects and their conflict evaluation; this reader never migrates or repairs.
pub(crate) fn read_pty_input_project_paths_strict() -> Result<Option<Vec<String>>, String> {
    let Some(path) = settings_path() else {
        return Err("settings_unavailable".to_string());
    };
    read_pty_input_project_paths_strict_from_path(&path)
}

pub(crate) fn read_pty_input_project_paths_strict_from_path(
    path: &Path,
) -> Result<Option<Vec<String>>, String> {
    let _lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "read_pty_input_project_paths_strict_from_path",
    )
    .map_err(|_| "unsafe_path".to_string())?;
    let selected =
        select_project_source_at_locked_path_with_limit(path, 1024 * 1024).map_err(|error| {
            match &error.legacy_outward {
                SettingsSaveLegacyOutward::DiskRead(reason) if reason.ends_with("unsafe_path") => {
                    "unsafe_path".to_string()
                }
                _ => "settings_invalid".to_string(),
            }
        })?;
    if selected.legacy.is_none() && selected.destination.is_none() {
        return Ok(None);
    }
    let object = selected.projection;
    let Some(project_paths) = object.get("projectPaths") else {
        return Ok(Some(Vec::new()));
    };
    if project_paths.is_null() {
        return Ok(Some(Vec::new()));
    }
    let project_paths = project_paths
        .as_array()
        .filter(|paths| paths.len() <= 1_024)
        .ok_or_else(|| "settings_invalid".to_string())?;
    let mut strict = Vec::with_capacity(project_paths.len());
    for path in project_paths {
        let path = path
            .as_str()
            .filter(|path| !path.is_empty() && path.len() <= 32 * 1024 && !path.contains('\0'))
            .ok_or_else(|| "settings_invalid".to_string())?;
        strict.push(path.to_string());
    }
    Ok(Some(strict))
}

pub(crate) async fn read_pty_input_project_paths_strict_offloaded(
) -> Result<Option<Vec<String>>, String> {
    tokio::task::spawn_blocking(read_pty_input_project_paths_strict)
        .await
        .map_err(|_| "settings_unavailable".to_string())?
}

pub(crate) fn read_terminal_snapshot_project_paths_strict() -> Result<Vec<String>, String> {
    let path = settings_path().ok_or_else(|| "snapshot_settings_invalid".to_string())?;
    let _lock = SettingsFileLock::acquire_contextual(
        &path,
        std::time::Duration::from_secs(2),
        "read_terminal_snapshot_project_paths_strict",
    )
    .map_err(|_| "snapshot_settings_invalid".to_string())?;
    let selected = select_project_source_at_locked_path_with_limit(&path, 1024 * 1024)
        .map_err(|_| "snapshot_settings_invalid".to_string())?;
    if selected.legacy.is_none() && selected.destination.is_none() {
        return Err("snapshot_settings_invalid".to_string());
    }
    let object = selected.projection;
    let Some(paths) = object.get(FIELD_PROJECT_PATHS) else {
        return Ok(Vec::new());
    };
    let paths = paths
        .as_array()
        .filter(|paths| paths.len() <= 4_096)
        .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
    let mut project_paths = Vec::with_capacity(paths.len());
    let mut aggregate_bytes = 0usize;
    for value in paths {
        let path = value
            .as_str()
            .filter(|path| !path.is_empty() && path.len() <= 32 * 1024 && !path.contains('\0'))
            .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
        aggregate_bytes = aggregate_bytes
            .checked_add(path.len())
            .filter(|bytes| *bytes <= 4 * 1024 * 1024)
            .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
        project_paths.push(path.to_string());
    }
    Ok(project_paths)
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TerminalSnapshotSecuritySettings {
    pub terminal_snapshots_enabled: bool,
    pub project_paths: Vec<String>,
}

impl std::fmt::Debug for TerminalSnapshotSecuritySettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let project_path_bytes = self
            .project_paths
            .iter()
            .fold(0usize, |total, path| total.saturating_add(path.len()));
        formatter
            .debug_struct("TerminalSnapshotSecuritySettings")
            .field(
                "terminal_snapshots_enabled",
                &self.terminal_snapshots_enabled,
            )
            .field("project_paths", &self.project_paths.len())
            .field("project_path_bytes", &project_path_bytes)
            .finish()
    }
}

/// Read only the security-bearing snapshot gate and active project list. This
/// path never repairs, migrates, or writes settings. Every ambiguity fails
/// closed and callers map it to the fixed disabled response.
pub(crate) fn read_terminal_snapshot_security_settings_strict(
) -> Result<TerminalSnapshotSecuritySettings, String> {
    let path = settings_path().ok_or_else(|| "snapshot_settings_invalid".to_string())?;
    read_terminal_snapshot_security_settings_strict_from_path(&path)
}

pub(crate) fn read_terminal_snapshot_security_settings_strict_from_path(
    path: &Path,
) -> Result<TerminalSnapshotSecuritySettings, String> {
    let _lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "read_terminal_snapshot_security_settings_strict_from_path",
    )
    .map_err(|_| "snapshot_settings_invalid".to_string())?;
    let selected = select_project_source_at_locked_path_with_limit(path, 1024 * 1024)
        .map_err(|_| "snapshot_settings_invalid".to_string())?;
    let object = selected
        .legacy
        .as_ref()
        .map(|source| &source.object)
        .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
    let enabled = match object.get(FIELD_TERMINAL_SNAPSHOTS_ENABLED) {
        Some(Value::Bool(enabled)) => *enabled,
        Some(_) => return Err("snapshot_settings_invalid".to_string()),
        None => true,
    };
    let paths = match selected.projection.get(FIELD_PROJECT_PATHS) {
        Some(value) => value
            .as_array()
            .filter(|paths| paths.len() <= 4_096)
            .ok_or_else(|| "snapshot_settings_invalid".to_string())?,
        None if [
            FIELD_PROJECT_PATH,
            FIELD_PROJECT_PATH_REL,
            FIELD_PROJECT_PATHS_REL,
        ]
        .iter()
        .all(|key| !selected.projection.contains_key(*key)) =>
        {
            return Ok(TerminalSnapshotSecuritySettings {
                terminal_snapshots_enabled: enabled,
                project_paths: Vec::new(),
            });
        }
        None => return Err("snapshot_settings_invalid".to_string()),
    };
    let mut project_paths = Vec::with_capacity(paths.len());
    let mut aggregate_bytes = 0usize;
    for value in paths {
        let path = value
            .as_str()
            .filter(|path| !path.is_empty() && path.len() <= 32 * 1024 && !path.contains('\0'))
            .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
        aggregate_bytes = aggregate_bytes
            .checked_add(path.len())
            .filter(|bytes| *bytes <= 4 * 1024 * 1024)
            .ok_or_else(|| "snapshot_settings_invalid".to_string())?;
        project_paths.push(path.to_string());
    }
    Ok(TerminalSnapshotSecuritySettings {
        terminal_snapshots_enabled: enabled,
        project_paths,
    })
}

pub(crate) async fn read_terminal_snapshot_security_settings_strict_offloaded(
) -> Result<TerminalSnapshotSecuritySettings, String> {
    let joined = tokio::task::spawn_blocking(|| {
        crate::logging::catch_payload_unwind(read_terminal_snapshot_security_settings_strict)
    })
    .await;
    match crate::logging::collapse_payload_task(joined) {
        Ok(result) => result,
        Err(_) => {
            log::error!("[terminal-snapshot] stage=gate_read_task code=internal");
            Err("snapshot_settings_invalid".to_string())
        }
    }
}

/// The sole persistence owner for the #1173 disclosure gate. The caller holds
/// the managed settings write guard while this function serializes every AC
/// process with the settings file lock and compares against current disk truth.
pub(crate) fn compare_and_set_terminal_snapshots_enabled(
    current: &AppSettings,
    expected: bool,
    enabled: bool,
) -> Result<AppSettings, String> {
    let path =
        settings_path().ok_or_else(|| "terminal_snapshot_setting_save_failed".to_string())?;
    compare_and_set_terminal_snapshots_enabled_at_path(current, &path, expected, enabled)
}

pub(crate) fn compare_and_set_terminal_snapshots_enabled_at_path(
    current: &AppSettings,
    path: &Path,
    expected: bool,
    enabled: bool,
) -> Result<AppSettings, String> {
    let report_typed_persistence_failure = |error| {
        report_settings_save_error(
            error,
            SettingsSaveReportSurface::TerminalSnapshotCompareAndSet,
        )
    };
    let lock_guard = match SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "compare_and_set_terminal_snapshots_enabled_at_path",
    ) {
        Ok(lock_guard) => lock_guard,
        Err(error) => return Err(report_typed_persistence_failure(error)),
    };
    let disk = match read_disk_object_for_write_typed(path, SettingsSaveStage::PreserveDiskGate) {
        Ok(disk) => disk,
        Err(error) => {
            drop(lock_guard);
            return Err(report_typed_persistence_failure(error));
        }
    };
    if let Err(error) = ensure_project_paths_migrated_at_locked_path(path, &mut |_, _| Ok(())) {
        drop(lock_guard);
        return Err(report_typed_persistence_failure(error));
    }
    let disk_file_absent = disk.is_none();
    // An absent file compares as the default-on value, but its absence is
    // tracked separately: the idempotent early return below may not report
    // success before a valid settings file exists, because the strict reader
    // cannot authorize anything without one.
    let disk_gate = match disk
        .as_ref()
        .and_then(|object| object.get(FIELD_TERMINAL_SNAPSHOTS_ENABLED))
    {
        Some(Value::Bool(value)) => *value,
        None => true,
        Some(_) => return Err("terminal_snapshot_setting_save_failed".to_string()),
    };
    if disk_gate != expected && disk_gate != enabled {
        return Err("terminal_snapshot_setting_conflict".to_string());
    }

    // #1737 (D17): the disk-decoded candidate must carry the overlay VALUES, not just
    // the overlay state, because the `disk_gate == enabled` early return below adopts
    // it with no save and no re-decode. The `None` arm clones the live settings and
    // already carries both.
    let effective: Map<String, Value> = if current.local_overlay_state.is_empty() {
        Map::new()
    } else {
        match serde_json::to_value(current) {
            Ok(Value::Object(object)) => object,
            _ => Map::new(),
        }
    };
    let mut candidate = match disk {
        Some(object) => decode_disk_settings_for_terminal_snapshot_cas(
            object,
            path,
            &current.local_overlay_state,
            &effective,
        )
        .map_err(|_| "terminal_snapshot_setting_save_failed".to_string())?,
        None => {
            let mut fresh = current.clone();
            let projection = select_project_source_at_locked_path(path)
                .map_err(|error| {
                    report_settings_save_error(
                        error,
                        SettingsSaveReportSurface::TerminalSnapshotCompareAndSet,
                    )
                })?
                .projection;
            apply_selected_project_state(&mut fresh, &projection);
            fresh
        }
    };
    candidate.local_overlay_state = current.local_overlay_state.clone();
    candidate.terminal_snapshots_enabled = enabled;
    if !disk_file_absent && disk_gate == enabled {
        return Ok(candidate);
    }

    // #2306 P1 - renumber in the candidate's current (effective) vector order
    // right before the direct write; never re-sort from stored ordinals.
    normalize_agent_order(&mut candidate.agents);
    let written = match save_settings_value_locked(
        &candidate,
        path,
        ProjectWriteMode::Preserve,
        TerminalSnapshotGateWriteMode::Explicit(enabled),
        MainWindowPlacementWriteMode::Preserve,
    ) {
        Ok(written) => written,
        Err(error) => {
            drop(lock_guard);
            return Err(report_typed_persistence_failure(error));
        }
    };
    let (bytes, _) = crate::path_identity::read_bounded_regular(path, 1024 * 1024)
        .map_err(|_| "terminal_snapshot_setting_save_failed".to_string())?;
    let value = crate::path_identity::parse_json_no_duplicates(&bytes)
        .map_err(|_| "terminal_snapshot_setting_save_failed".to_string())?;
    if value
        .as_object()
        .and_then(|object| object.get(FIELD_TERMINAL_SNAPSHOTS_ENABLED))
        .and_then(Value::as_bool)
        != Some(enabled)
    {
        return Err("terminal_snapshot_setting_save_failed".to_string());
    }
    Ok(written)
}

pub(crate) fn decode_disk_settings_for_terminal_snapshot_cas(
    object: Map<String, Value>,
    settings_path: &Path,
    overlay: &LocalSettingsOverlay,
    effective: &Map<String, Value>,
) -> Result<AppSettings, String> {
    let base = production_instance_base();
    let mut value = Value::Object(object);
    let selected = select_project_source_at_locked_path(settings_path).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    replace_project_projection(value.as_object_mut().unwrap(), &selected.projection);
    // #2716 (B3) 4.2b: the same merge as `parse_settings_json`, in the same layer
    // order, under the overlay reapplied below. The decoded value is adopted and
    // written, so without it an empty `agents` would reach the agents file.
    let main_profile_owner = value.as_object().expect("settings object").clone();
    let agents_layer = merge_agents_instance_file(settings_path, &mut value);
    let physical_sidecar = read_agents_instance_file(&agents_instance_path(settings_path))
        .ok()
        .flatten();
    let profile_owner = selected_registered_profile_owner(
        &main_profile_owner,
        physical_sidecar.as_ref(),
        !overlay.owns_top_level("codingAgentProfiles"),
    );
    let physical_groups = physical_sidecar
        .map(|groups| {
            agents_instance_groups(&groups)
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let identity_physical =
        coding_agent_identity_with_physical_overlay(value.clone(), settings_path, &physical_groups);
    let base_profiles = value.get("codingAgentProfiles").cloned();
    if let Some(profiles) = registered_profiles_for_read(&profile_owner).get("codingAgentProfiles")
    {
        value["codingAgentProfiles"] = profiles.clone();
    }
    // #1737 (D17): re-apply the overlay to the disk-decoded object, so the
    // `disk_gate == enabled` early return does not hand the caller base values.
    // Pinned before the project decode, and therefore strictly before
    // `candidate.terminal_snapshots_enabled = enabled` runs on the typed value.
    if let Value::Object(map) = &mut value {
        overlay.reapply_from(effective, map);
    }
    restore_codex_home_carrier(value.as_object_mut().unwrap(), base_profiles.as_ref());
    let state =
        apply_project_decode_to_value(&mut value, base.as_deref(), &projects::FsCandidateResolver);
    let mut settings: AppSettings =
        serde_json::from_value(value).map_err(|_| "settings_invalid".to_string())?;
    settings.project_path_state = Arc::new(state);
    set_agents_layer_state(&mut settings, settings_path, agents_layer);
    settings.coding_agent_identity_physical = Some(Arc::new(identity_physical));
    // #2306 P1 - finalize before the caller's no-op return or direct write, so
    // both the returned and the written snapshot carry contiguous ordinals.
    // Overlay values stay in memory: `save_settings_value_locked` restores the
    // base `agents` array before writing the file.
    finalize_agent_order(&mut settings.agents);
    refresh_agent_id_hashes(&mut settings.agents);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);
    repair_loaded_response_close_settings(&mut settings, &settings_path.display().to_string());
    repair_loaded_room_idle_confirmation_settings(
        &mut settings,
        &settings_path.display().to_string(),
    );
    Ok(settings)
}

/// #2348 - the sole persistence owner for the main-window placement pair. The
/// caller holds the managed settings write guard while this function serializes
/// every AC process with the settings file lock. Returns the re-decoded settings
/// the caller must adopt.
pub(crate) fn set_main_window_placement(
    current: &AppSettings,
    geometry: &WindowGeometry,
    display_state: MainWindowDisplayState,
) -> Result<AppSettings, String> {
    let path = settings_path().ok_or_else(|| "main_window_placement_save_failed".to_string())?;
    set_main_window_placement_at_path(current, &path, geometry, display_state)
}

pub(crate) fn set_main_window_placement_at_path(
    current: &AppSettings,
    path: &Path,
    geometry: &WindowGeometry,
    display_state: MainWindowDisplayState,
) -> Result<AppSettings, String> {
    // #1737 (D7c): `restore_base` is the last writer over the base file, so a
    // placement write beneath an overlay that owns either placement key would be
    // silently reverted. Refuse with the stable signal instead of pretending.
    if current
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_GEOMETRY)
        || current
            .local_overlay_state
            .owns_top_level(OVERLAY_KEY_MAIN_DISPLAY_STATE)
    {
        return Err("main_window_placement_overlay_pinned".to_string());
    }
    let _lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "set_main_window_placement_at_path",
    )
    .map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    save_settings_value_locked(
        current,
        path,
        ProjectWriteMode::Preserve,
        TerminalSnapshotGateWriteMode::Preserve,
        MainWindowPlacementWriteMode::Explicit {
            geometry: geometry.clone(),
            display_state,
        },
    )
    .map_err(|error| report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings))
}

pub(crate) fn read_project_paths_from_disk(
    path: &Path,
) -> Result<Option<DiskProjectLists>, String> {
    let transaction =
        empty_project_transaction(path, "read_project_paths_from_disk").map_err(|error| {
            report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
        })?;
    let selected = select_project_source_locked(&transaction).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    if selected.legacy.is_none() && selected.destination.is_none() {
        return Ok(None);
    }
    project_lists_from_projection(&selected.projection)
}

pub(crate) fn project_lists_from_projection(
    projection: &Map<String, Value>,
) -> Result<Option<DiskProjectLists>, String> {
    let value = Value::Object(projection.clone());
    let path = Path::new("project-paths.json");
    let project_paths = match value.get("projectPaths") {
        None | Some(serde_json::Value::Null) => value
            .get(FIELD_PROJECT_PATH)
            .and_then(Value::as_str)
            .map(str::to_string)
            .into_iter()
            .collect(),
        // #881: non-string elements are dropped here, but rejected in the
        // archivedProjectPaths arm below. Deliberate. `projectPaths` only feeds
        // discovery; `archivedProjectPaths` feeds session retention, where a
        // silently-emptied list deletes sessions. Tightening this one is
        // #888's call, not #881's.
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect::<Vec<_>>(),
        Some(other) => {
            return Err(format!(
                "{}: projectPaths is {}, not an array (aborting save)",
                path.display(),
                other
            ));
        }
    };
    let project_path = value
        .get("projectPath")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let archived_project_paths = match value.get("archivedProjectPaths") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Array(arr)) => {
            // #881 R2-G5: a non-string element is corruption, not an entry to
            // skip. The `projectPaths` arm above silently drops non-strings;
            // do not copy that here. `[123]` would read as `Some(vec![])`,
            // meaning disk authoritatively says nothing is archived.
            let mut archived = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(archived_path) = item.as_str() else {
                    return Err(format!(
                        "{}: archivedProjectPaths contains {}, not a string (aborting save)",
                        path.display(),
                        item
                    ));
                };
                archived.push(archived_path.to_string());
            }
            Some(archived)
        }
        Some(other) => {
            return Err(format!(
                "{}: archivedProjectPaths is {}, not an array (aborting save)",
                path.display(),
                other
            ));
        }
    };
    Ok(Some(DiskProjectLists {
        project_paths,
        project_path,
        archived_project_paths,
    }))
}

/// #778/#881: overwrite `settings`' in-memory project lists with the
/// disk-authoritative ones. The deliberate list mutators
/// (`open_project`/`new_project`/`remove_project`)
/// call this under the `SettingsState` write lock BEFORE they add/remove, so a
/// concurrent CLI append is reconciled into the list rather than clobbered by the
/// following verbatim write. Aborts (propagates `Err`) on a non-`NotFound` read
/// error per G2; a missing home dir is a no-op (the following save surfaces it).
pub fn refresh_project_paths_from_disk(settings: &mut AppSettings) -> Result<(), String> {
    let Some(path) = settings_path() else {
        return Ok(());
    };
    refresh_project_paths_from_path(settings, &path)
}

pub(crate) fn refresh_project_paths_from_path(
    settings: &mut AppSettings,
    path: &Path,
) -> Result<(), String> {
    let lists = read_project_paths_from_disk(path).inspect_err(|error| {
        settings.project_paths_persistence_error = Some(error.clone());
    })?;
    if let Some(lists) = lists {
        settings.project_paths = lists.project_paths;
        settings.project_path = lists.project_path;
        settings.archived_project_paths = lists.archived_project_paths.unwrap_or_default();
    }
    settings.project_paths_persistence_error = None;
    Ok(())
}

/// #1077 §4.3 step 2: fresh-read and fully re-decode the six disk fields (so a
/// post-startup CLI write is authoritative), replacing the runtime lists with the
/// SELECTED canonical paths and installing the fresh hidden pair state. Aborts
/// (`Err`) on a present-but-invalid whole object; keeps the live runtime/hidden
/// state when the file is absent.
#[cfg(test)]
pub(crate) fn refresh_and_decode_project_paths_from_path(
    settings: &mut AppSettings,
    path: &Path,
) -> Result<(), String> {
    let lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "refresh_and_decode_project_paths_from_path",
    )
    .map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    let projection = (|| {
        read_disk_object_for_write_typed(path, SettingsSaveStage::ProjectPathsRefreshDiskGate)?;
        select_project_source_at_locked_path(path).map(|selected| selected.projection)
    })();
    drop(lock);
    let projection = projection.map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    apply_selected_project_state(settings, &projection);
    Ok(())
}

/// Whether the current hidden project state carries structural corruption, which
/// must block any explicit project-list mutation (§4.2).
pub(crate) fn project_state_has_structural(settings: &AppSettings) -> bool {
    settings.project_paths_persistence_error.is_some()
        || settings.project_path_state.has_structural()
}

/// Save unrelated settings under the shared settings lock. Migration completes
/// first; afterward the sidecar bytes are preserved exactly and all six project
/// keys are excluded from managed settings. The returned runtime view is decoded
/// from the authoritative sidecar, never from a renderer's project draft.
pub fn save_settings(settings: &AppSettings) -> Result<AppSettings, String> {
    let dir = super::super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join(SETTINGS_FILE_NAME);
    save_settings_to_path_preserving_project_paths(settings, &path)
}

#[cfg(test)]
thread_local! {
    pub(crate) static PROJECT_MIGRATION_ATOMIC_FAULT: std::cell::RefCell<
        Option<(PathBuf, SettingsSaveStage)>,
    > = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn project_atomic_checkpoint(
    path: &Path,
    stage: SettingsSaveStage,
) -> Result<(), SettingsSaveError> {
    PROJECT_MIGRATION_ATOMIC_FAULT.with(|slot| {
        if slot
            .borrow()
            .as_ref()
            .is_some_and(|(target, point)| target == path && *point == stage)
        {
            Err(project_migration_error(
                path,
                stage,
                "injected migration atomic failure",
            ))
        } else {
            Ok(())
        }
    })
}

/// #1077: atomic tmp+rename writer over an already-built JSON `Value`. Shared by
/// the raw and the project-aware writers. Preserves the #774 unique-temp +
/// `rename_with_retry` behavior. The temp IS fsynced
/// (`temporary.sync_all()`), and the parent directory is fsynced on unix after
/// the rename. The caller owns #2058 backup rotation of the returned bytes.
pub(crate) fn write_value_atomic(value: &Value, path: &Path) -> Result<Vec<u8>, SettingsSaveError> {
    write_value_atomic_labeled(value, path, path)
}

pub(crate) fn write_value_atomic_labeled(
    value: &Value,
    path: &Path,
    label: &Path,
) -> Result<Vec<u8>, SettingsSaveError> {
    use std::io::Write as _;

    let dir = path.parent().ok_or_else(|| {
        SettingsSaveError::semantic(
            SettingsSaveStage::PrepareTarget,
            path,
            None,
            SettingsSaveReason::MissingParentDirectory,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    std::fs::create_dir_all(dir).map_err(|source| {
        SettingsSaveError::io(
            SettingsSaveStage::PrepareTarget,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    crate::path_identity::verify_component_chain(dir).map_err(|_| {
        SettingsSaveError::semantic(
            SettingsSaveStage::PrepareTarget,
            path,
            None,
            SettingsSaveReason::TargetIdentityRejected,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(SettingsSaveError::semantic(
                    SettingsSaveStage::PrepareTarget,
                    path,
                    None,
                    SettingsSaveReason::TargetNotRegularFile,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                ));
            }
            crate::path_identity::verify_regular_file(path).map_err(|_| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::PrepareTarget,
                    path,
                    None,
                    SettingsSaveReason::TargetIdentityRejected,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(SettingsSaveError::io(
                SettingsSaveStage::PrepareTarget,
                path,
                None,
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            ));
        }
    }

    let json = serde_json::to_vec_pretty(value).map_err(|source| {
        SettingsSaveError::json(
            SettingsSaveStage::Serialize,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    })?;
    if json.len() > 16 * 1024 * 1024 {
        return Err(SettingsSaveError::semantic(
            SettingsSaveStage::Serialize,
            path,
            None,
            SettingsSaveReason::SerializedPayloadTooLarge,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        ));
    }

    let op_id = SAVE_OP_ID.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    // Settings keep their existing instance-qualified temp prefix, including
    // callers supplying a fixture path. The fixed sidecar uses its own name.
    let temp_base = match path.file_name() {
        Some(name)
            if name
                == std::ffi::OsStr::new(
                    super::super::instance_artifacts::PROJECT_PATHS_FILE_NAME,
                ) =>
        {
            name
        }
        _ => std::ffi::OsStr::new(SETTINGS_FILE_NAME),
    };
    let mut temp_name = temp_base.to_os_string();
    temp_name.push(format!(".{pid}.{op_id}.tmp"));
    let tmp_path = dir.join(temp_name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }

    let result: Result<(), SettingsSaveError> = (|| {
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::TempCreate)?;
        let mut temporary = options.open(&tmp_path).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::TempCreate,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::TempWrite)?;
        temporary.write_all(&json).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::TempWrite,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        temporary.flush().map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::TempFlush,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::TempSync)?;
        temporary.sync_all().map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::TempSync,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        crate::path_identity::verify_opened_regular_file(&tmp_path, &temporary, false).map_err(
            |_| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::TempVerify,
                    path,
                    Some(&tmp_path),
                    SettingsSaveReason::TempIdentityRejected,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            },
        )?;
        drop(temporary);

        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::AtomicReplace)?;
        replace_settings_file_atomic(&tmp_path, path).map_err(|source| {
            SettingsSaveError::io(
                SettingsSaveStage::AtomicReplace,
                path,
                Some(&tmp_path),
                source,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(
                |source| {
                    SettingsSaveError::io(
                        SettingsSaveStage::TargetPermissions,
                        path,
                        Some(&tmp_path),
                        source,
                        SettingsSaveLegacyOutward::SettingsSaveFailed,
                    )
                },
            )?;
            std::fs::File::open(dir)
                .and_then(|directory| directory.sync_all())
                .map_err(|source| {
                    SettingsSaveError::io(
                        SettingsSaveStage::ParentDirectorySync,
                        path,
                        Some(&tmp_path),
                        source,
                        SettingsSaveLegacyOutward::SettingsSaveFailed,
                    )
                })?;
        }
        #[cfg(test)]
        project_atomic_checkpoint(path, SettingsSaveStage::PostWriteRead)?;
        let (written, _) = crate::path_identity::read_bounded_regular(path, 16 * 1024 * 1024)
            .map_err(|_| {
                SettingsSaveError::semantic(
                    SettingsSaveStage::PostWriteRead,
                    path,
                    Some(&tmp_path),
                    SettingsSaveReason::PostWriteReadRejected,
                    SettingsSaveLegacyOutward::SettingsSaveFailed,
                )
            })?;
        if written != json {
            return Err(SettingsSaveError::semantic(
                SettingsSaveStage::PostWriteVerify,
                path,
                Some(&tmp_path),
                SettingsSaveReason::WrittenBytesMismatch,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            ));
        }
        Ok(())
    })();
    if result.is_err() && tmp_path.exists() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result?;

    log::debug!("Saved settings to {:?}", label);
    Ok(json)
}

#[cfg(windows)]
pub(crate) fn replace_settings_file_atomic_with_retry<Replace, Sleep>(
    replace: Replace,
    sleep: Sleep,
) -> std::io::Result<()>
where
    Replace: FnMut() -> std::io::Result<()>,
    Sleep: FnMut(std::time::Duration),
{
    crate::config::retry_transient_io_with(
        replace,
        sleep,
        |failed_attempt, total_attempts, error, delay| {
            if let Some(delay) = delay {
                let raw_os_error = error.raw_os_error().unwrap_or_default();
                log::warn!(
                    "Windows settings atomic replace attempt {} of {} failed with raw OS error {}; \
                        retrying in {} ms",
                    failed_attempt,
                    total_attempts,
                    raw_os_error,
                    delay.as_millis()
                );
            }
        },
    )
    .map_err(|failure| failure.error)
}

#[cfg(windows)]
pub(crate) fn replace_settings_file_atomic(
    source: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    replace_settings_file_atomic_with_retry(
        || {
            let result = unsafe {
                MoveFileExW(
                    source.as_ptr(),
                    destination.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            };
            if result == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        },
        std::thread::sleep,
    )
}

#[cfg(not(windows))]
pub(crate) fn replace_settings_file_atomic(
    source: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

/// Snapshot fixture/migration writer only. Production operations own a token
/// spanning fresh reads, mutation, sidecar commit, revalidation and rollback.
#[cfg(test)]
pub fn save_settings_with_project_paths(settings: &AppSettings) -> Result<(), String> {
    let dir = super::super::config_dir().ok_or("Could not determine home directory")?;
    let path = dir.join(SETTINGS_FILE_NAME);
    save_settings_with_project_paths_to_path(settings, &path)
}

#[cfg(test)]
pub(crate) fn save_settings_with_project_paths_to_path(
    settings: &AppSettings,
    path: &Path,
) -> Result<(), String> {
    reconcile_project_state_to_path(settings, path, true, true).map(|_| ())
}

/// #778/#881 + #1077: the preserve-disk wrapper behind the default
/// `save_settings`. Preserves all six on-disk project fields (materializing a
/// group only when disk has no truth), then hands off to the #774-hardened
/// atomic writer and returns the fresh-decoded settings.
pub(crate) fn save_settings_to_path_preserving_project_paths(
    settings: &AppSettings,
    path: &Path,
) -> Result<AppSettings, String> {
    save_settings_to_path_preserving_project_paths_typed(settings, path).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })
}

pub(crate) fn save_settings_to_path_preserving_project_paths_typed(
    settings: &AppSettings,
    path: &Path,
) -> Result<AppSettings, SettingsSaveError> {
    save_settings_value(settings, path, ProjectWriteMode::Preserve)
}

/// Raw writer: serialize `AppSettings` verbatim (three primary project fields;
/// the companion fields are `#[serde(skip)]`, yielding a legacy-shaped file).
/// Used only for test seeding; production writes go through the preserve/
/// reconcile modes so companions are materialized.
#[cfg(test)]
pub(crate) fn save_settings_to_path(
    settings: &AppSettings,
    path: &Path,
) -> Result<(), SettingsSaveError> {
    let value = serde_json::to_value(settings).map_err(|source| {
        let outward = format!("Failed to serialize settings: {}", source);
        SettingsSaveError::json(
            SettingsSaveStage::Serialize,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::Serialize(outward),
        )
    })?;
    write_value_atomic(&value, path).map(|_| ())
}
