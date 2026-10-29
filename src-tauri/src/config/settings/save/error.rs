use super::super::validate_non_project_settings;
use crate::filesystem::settings_file;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

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
    match settings_file::read_settings_text(path) {
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
