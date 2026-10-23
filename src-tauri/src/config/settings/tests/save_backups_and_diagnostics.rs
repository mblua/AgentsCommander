use super::*;

pub(super) fn assert_no_issue_1330_temp_files(directory: &std::path::Path) {
    let entries = std::fs::read_dir(directory).unwrap();
    for entry in entries {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        assert!(
            !(name.starts_with("settings.30.instance.no-git.json.") && name.ends_with(".tmp")),
            "unexpected settings temp file: {name}"
        );
    }
}

/// #2058 - put `settings` on disk in exactly the bytes a production save
/// produces, and return them. The file is absent beforehand, so D3's
/// `disk_read == None` arm skips rotation and no slot exists on return.
fn seed_settings_with_production_bytes(
    path: &std::path::Path,
    settings: &super::super::AppSettings,
) -> Vec<u8> {
    super::super::save_settings_to_path_preserving_project_paths(settings, path).unwrap();
    let bytes = std::fs::read(path).unwrap();
    assert!(
        !path
            .with_file_name("settings.30.instance.no-git.backup.1.json")
            .exists(),
        "seeding must not rotate"
    );
    bytes
}

fn assert_no_backup_slots(path: &std::path::Path) {
    for slot in 1..=super::super::SETTINGS_BACKUP_KEEP {
        assert!(
            !path
                .with_file_name(format!("settings.30.instance.no-git.backup.{slot}.json"))
                .exists(),
            "settings.30.instance.no-git.backup.{slot}.json must not exist"
        );
    }
}

#[test]
fn issue_2058_save_archives_the_replaced_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = super::super::AppSettings {
        root_token: Some("a".to_string()),
        ..super::super::AppSettings::default()
    };
    let seed = seed_settings_with_production_bytes(&path, &a);
    let b = super::super::AppSettings {
        root_token: Some("b".to_string()),
        ..super::super::AppSettings::default()
    };
    super::super::save_settings_to_path_preserving_project_paths(&b, &path).unwrap();
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap(),
        seed,
        "slot 1 holds the replaced bytes A"
    );
    assert_ne!(
        std::fs::read(&path).unwrap(),
        seed,
        "settings.json holds the new bytes"
    );
    assert!(
        !path
            .with_file_name("settings.30.instance.no-git.backup.2.json")
            .exists(),
        "slot 2 must not exist after a single rotation"
    );
}

#[test]
fn issue_2058_first_save_with_no_existing_file_writes_no_backup() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = super::super::AppSettings {
        root_token: Some("a".to_string()),
        ..super::super::AppSettings::default()
    };
    super::super::save_settings_to_path_preserving_project_paths(&a, &path).unwrap();
    assert_no_backup_slots(&path);
    // Positive control: a second save with a different value creates slot 1.
    let first_bytes = std::fs::read(&path).unwrap();
    let b = super::super::AppSettings {
        root_token: Some("b".to_string()),
        ..super::super::AppSettings::default()
    };
    super::super::save_settings_to_path_preserving_project_paths(&b, &path).unwrap();
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap(),
        first_bytes,
        "positive control: slot 1 holds the first save's bytes"
    );
}

#[test]
fn issue_2058_identical_save_does_not_rotate() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = super::super::AppSettings {
        root_token: Some("a".to_string()),
        ..super::super::AppSettings::default()
    };
    let seed = seed_settings_with_production_bytes(&path, &a);
    // Save the same settings value again: bytes equal, D3 skips rotation.
    super::super::save_settings_to_path_preserving_project_paths(&a, &path).unwrap();
    assert_no_backup_slots(&path);
    // Positive control: a third save with a changed value creates slot 1.
    let c = super::super::AppSettings {
        root_token: Some("c".to_string()),
        ..super::super::AppSettings::default()
    };
    super::super::save_settings_to_path_preserving_project_paths(&c, &path).unwrap();
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap(),
        seed,
        "positive control: slot 1 holds exactly A"
    );
}

#[test]
fn issue_2058_keeps_five_generations_and_drops_the_oldest() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut written: Vec<Vec<u8>> = Vec::new();
    for n in 0..7u32 {
        let settings = super::super::AppSettings {
            root_token: Some(format!("v{n}")),
            ..super::super::AppSettings::default()
        };
        super::super::save_settings_to_path_preserving_project_paths(&settings, &path).unwrap();
        written.push(std::fs::read(&path).unwrap());
    }
    // Slots 1..=5 hold the written bytes of v5, v4, v3, v2, v1.
    for (slot, n) in (1..=5u32).zip((1..=5u32).rev()) {
        assert_eq!(
            std::fs::read(super::super::settings_backup_path(&path, slot)).unwrap(),
            written[n as usize],
            "slot {slot} must hold the written bytes of v{n}"
        );
    }
    assert!(
        !path
            .with_file_name("settings.30.instance.no-git.backup.6.json")
            .exists(),
        "slot 6 must not exist"
    );
    for slot in 1..=5u32 {
        assert_ne!(
            std::fs::read(super::super::settings_backup_path(&path, slot)).unwrap(),
            written[0],
            "no slot may hold v0's bytes"
        );
    }
}

#[test]
fn issue_2058_non_regular_slot_aborts_rotation() {
    // A non-regular slot 1 (a directory) aborts rotation (D8) without
    // fataling the save (D7).
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = super::super::AppSettings {
        root_token: Some("a".to_string()),
        ..super::super::AppSettings::default()
    };
    let seed = seed_settings_with_production_bytes(&path, &a);
    std::fs::create_dir(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap();
    let b = super::super::AppSettings {
        root_token: Some("b".to_string()),
        ..super::super::AppSettings::default()
    };
    super::super::save_settings_to_path_preserving_project_paths(&b, &path).unwrap();
    let slot_one = path.with_file_name("settings.30.instance.no-git.backup.1.json");
    assert!(slot_one.is_dir(), "slot 1 is still a directory");
    assert!(
        slot_one.read_dir().unwrap().next().is_none(),
        "slot 1 is empty"
    );
    assert_ne!(
        std::fs::read(&path).unwrap(),
        seed,
        "settings.json holds B's bytes"
    );
    assert!(
        !path
            .with_file_name("settings.30.instance.no-git.backup.2.json")
            .exists(),
        "slot 2 must not exist"
    );
    // Positive control: the same sequence without the directory creates slot 1.
    let ctrl_temp = tempfile::tempdir().unwrap();
    let ctrl_path = ctrl_temp.path().join("settings.json");
    let ctrl_seed = seed_settings_with_production_bytes(&ctrl_path, &a);
    super::super::save_settings_to_path_preserving_project_paths(&b, &ctrl_path).unwrap();
    assert_eq!(
        std::fs::read(ctrl_path.with_file_name("settings.30.instance.no-git.backup.1.json"))
            .unwrap(),
        ctrl_seed,
        "positive control: slot 1 holds A"
    );
}

#[test]
fn issue_2058_rotation_write_failure_is_not_fatal() {
    let temp = tempfile::tempdir().unwrap();
    let absent = temp.path().join("absent-dir").join("settings.json");
    super::super::rotate_settings_backups(&absent, b"previous");
    assert!(
        !temp.path().join("absent-dir").exists(),
        "the absent parent directory was never created"
    );
}

#[test]
fn issue_2058_rotation_leaves_the_pre_384_backup_alone() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let legacy = LEGACY_PROFILES_SETTINGS_FIXTURE;
    std::fs::write(&path, legacy).unwrap();
    let b = super::super::AppSettings {
        root_token: Some("b".to_string()),
        ..super::super::AppSettings::default()
    };
    super::super::save_settings_to_path_preserving_project_paths(&b, &path).unwrap();
    let b_bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(path.with_file_name("settings.30.instance.no-git.pre-384-v1.json"))
            .unwrap(),
        legacy,
        "pre-384 backup holds the legacy bytes"
    );
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap(),
        legacy.as_bytes(),
        "slot 1 holds the legacy bytes"
    );
    assert_ne!(
        std::fs::read_to_string(&path).unwrap(),
        legacy,
        "settings.json holds B's bytes, not the legacy"
    );
    // Third save with a different value C.
    let c = super::super::AppSettings {
        root_token: Some("c".to_string()),
        ..super::super::AppSettings::default()
    };
    super::super::save_settings_to_path_preserving_project_paths(&c, &path).unwrap();
    assert_eq!(
        std::fs::read_to_string(path.with_file_name("settings.30.instance.no-git.pre-384-v1.json"))
            .unwrap(),
        legacy,
        "pre-384 backup is still byte-identical to legacy"
    );
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.1.json")).unwrap(),
        b_bytes,
        "slot 1 now holds B's bytes"
    );
    assert_eq!(
        std::fs::read(path.with_file_name("settings.30.instance.no-git.backup.2.json")).unwrap(),
        legacy.as_bytes(),
        "slot 2 holds the legacy bytes"
    );
}

#[test]
fn issue_2058_rotation_creates_exactly_one_new_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let a = super::super::AppSettings {
        root_token: Some("a".to_string()),
        ..super::super::AppSettings::default()
    };
    seed_settings_with_production_bytes(&path, &a);
    let before: std::collections::HashSet<String> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let b = super::super::AppSettings {
        root_token: Some("b".to_string()),
        ..super::super::AppSettings::default()
    };
    super::super::save_settings_to_path_preserving_project_paths(&b, &path).unwrap();
    let after: std::collections::HashSet<String> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let added: Vec<&String> = after.difference(&before).collect();
    let removed: Vec<&String> = before.difference(&after).collect();
    assert_eq!(added.len(), 1, "exactly one file added: {added:?}");
    assert_eq!(
        added[0].as_str(),
        "settings.30.instance.no-git.backup.1.json"
    );
    assert!(removed.is_empty(), "no file removed: {removed:?}");
    assert_no_issue_1330_temp_files(temp.path());
}

pub(super) struct SettingsSaveDiagnosticCaptureGuard {
    previous: Option<super::super::SettingsSaveDiagnosticCapture>,
}

impl SettingsSaveDiagnosticCaptureGuard {
    pub(super) fn install(capture: impl Fn(&str) + 'static) -> Self {
        let previous = super::super::SETTINGS_SAVE_DIAGNOSTIC_CAPTURE
            .with(|slot| slot.replace(Some(Box::new(capture))));
        Self { previous }
    }
}

impl Drop for SettingsSaveDiagnosticCaptureGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        super::super::SETTINGS_SAVE_DIAGNOSTIC_CAPTURE.with(|slot| {
            slot.replace(previous);
        });
    }
}

pub(super) fn capture_settings_save_diagnostics() -> (
    std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    SettingsSaveDiagnosticCaptureGuard,
) {
    let records = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let captured = std::rc::Rc::clone(&records);
    let guard = SettingsSaveDiagnosticCaptureGuard::install(move |payload| {
        captured.borrow_mut().push(payload.to_string());
    });
    (records, guard)
}

pub(super) trait CapturedRecordsSnapshot {
    fn snapshot(&self) -> Vec<String>;
}

impl CapturedRecordsSnapshot for std::rc::Rc<std::cell::RefCell<Vec<String>>> {
    fn snapshot(&self) -> Vec<String> {
        self.borrow().clone()
    }
}

impl CapturedRecordsSnapshot for std::cell::Ref<'_, Vec<String>> {
    fn snapshot(&self) -> Vec<String> {
        self.to_vec()
    }
}

impl CapturedRecordsSnapshot for Vec<String> {
    fn snapshot(&self) -> Vec<String> {
        self.clone()
    }
}

pub(super) fn parse_settings_save_diagnostic(
    payload: &str,
) -> std::collections::BTreeMap<String, String> {
    payload
        .split_whitespace()
        .skip(1)
        .map(|field| {
            let (key, value) = field.split_once('=').unwrap();
            (key.to_string(), value.to_string())
        })
        .collect()
}

#[cfg(windows)]
pub(super) fn expected_rendered_diagnostic_path(path: &std::path::Path) -> String {
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

pub(super) fn expected_io_kind_from_literal_table(kind: std::io::ErrorKind) -> &'static str {
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

#[test]
fn settings_save_error_preserves_sources_and_separates_diagnostic_from_outward() {
    for raw in [5, 32] {
        let target = std::path::PathBuf::from("C:/issue-1330/target.json");
        let temporary = std::path::PathBuf::from("C:/issue-1330/settings.json.1.2.tmp");
        let legacy = format!("legacy-only-sentinel-{raw}");
        let error = super::super::SettingsSaveError::io(
            super::super::SettingsSaveStage::PreserveDiskGate,
            &target,
            Some(&temporary),
            std::io::Error::from_raw_os_error(raw),
            super::super::SettingsSaveLegacyOutward::DiskRead(legacy.clone()),
        );
        let (source_raw, source_kind, source_display) = match &error.cause {
            super::super::SettingsSaveCause::Io(source) => {
                (source.raw_os_error(), source.kind(), source.to_string())
            }
            _ => panic!("expected retained I/O source"),
        };
        assert_eq!(source_raw, Some(raw));
        if let super::super::SettingsSaveCause::Io(retained_source) = &error.cause {
            let exposed_source = std::error::Error::source(&error).unwrap();
            let exposed_io = exposed_source.downcast_ref::<std::io::Error>().unwrap();
            assert!(std::ptr::eq(exposed_io, retained_source));
        }
        assert_eq!(
            error.stage,
            super::super::SettingsSaveStage::PreserveDiskGate
        );
        assert_eq!(error.target_path.as_path(), target.as_path());
        assert_eq!(
            error.temp_path.as_deref().map(|path| path.as_path()),
            Some(temporary.as_path())
        );
        assert_eq!(error.pid, std::process::id());
        assert_eq!(
            error.general_settings_diagnostic_code(),
            "settings_save_failed"
        );
        assert_eq!(error.to_string(), "settings_save_failed");

        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let outward = super::super::report_settings_save_error(
            error,
            super::super::SettingsSaveReportSurface::GeneralSettings,
        );
        assert_eq!(outward, legacy);
        let records = CapturedRecordsSnapshot::snapshot(&records);
        assert_eq!(records.len(), 1);
        assert!(!records[0].contains("legacy-only-sentinel"));
        assert!(!records[0].contains(&source_display));
        let fields = parse_settings_save_diagnostic(&records[0]);
        assert_eq!(fields["code"], "settings_save_failed");
        assert_eq!(fields["source_kind"], "io");
        assert_eq!(fields["reason"], "none");
        assert_eq!(fields["raw_os_error"], raw.to_string());
        assert_eq!(
            fields["io_kind"],
            expected_io_kind_from_literal_table(source_kind)
        );
        assert_eq!(fields["pid"], std::process::id().to_string());
        assert_eq!(fields["target_path"], "\"C:/issue-1330/target.json\"");
        assert_eq!(
            fields["temp_path"],
            "\"C:/issue-1330/settings.json.1.2.tmp\""
        );
    }

    let lock_error = super::super::SettingsSaveError::io(
        super::super::SettingsSaveStage::LockAcquire,
        std::path::Path::new("C:/issue-1330/lock.json"),
        None,
        std::io::Error::from_raw_os_error(5),
        super::super::SettingsSaveLegacyOutward::SettingsLockUnavailable,
    );
    assert!(std::error::Error::source(&lock_error).is_some());
    assert_eq!(lock_error.to_string(), "settings_lock_unavailable");
    let (records, _capture_guard) = capture_settings_save_diagnostics();
    let outward = super::super::report_settings_save_error(
        lock_error,
        super::super::SettingsSaveReportSurface::GeneralSettings,
    );
    assert_eq!(outward, "settings_lock_unavailable");
    assert_eq!(
        parse_settings_save_diagnostic(&records.borrow()[0])["code"],
        "settings_lock_unavailable"
    );

    for stage in [
        super::super::SettingsSaveStage::LockAcquire,
        super::super::SettingsSaveStage::AtomicReplace,
    ] {
        let error = super::super::SettingsSaveError::semantic(
            stage,
            std::path::Path::new("C:/issue-1330/terminal.json"),
            None,
            super::super::SettingsSaveReason::LockTimedOut,
            super::super::SettingsSaveLegacyOutward::DiskValidation(
                "general-legacy-sentinel".to_string(),
            ),
        );
        let expected_display = if stage == super::super::SettingsSaveStage::LockAcquire {
            "settings_lock_unavailable"
        } else {
            "settings_save_failed"
        };
        assert_eq!(error.to_string(), expected_display);
        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let outward = super::super::report_settings_save_error(
            error,
            super::super::SettingsSaveReportSurface::TerminalSnapshotCompareAndSet,
        );
        assert_eq!(outward, "terminal_snapshot_setting_save_failed");
        let fields = parse_settings_save_diagnostic(&records.borrow()[0]);
        assert_eq!(fields["code"], "terminal_snapshot_setting_save_failed");
        assert!(!records.borrow()[0].contains("general-legacy-sentinel"));
    }

    let json_source = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    let json_error = super::super::SettingsSaveError::json(
        super::super::SettingsSaveStage::Serialize,
        std::path::Path::new("C:/issue-1330/json.json"),
        None,
        json_source,
        super::super::SettingsSaveLegacyOutward::Serialize("json legacy".to_string()),
    );
    assert!(std::error::Error::source(&json_error).is_some());
    let semantic_error = super::super::SettingsSaveError::semantic(
        super::super::SettingsSaveStage::Serialize,
        std::path::Path::new("C:/issue-1330/semantic.json"),
        None,
        super::super::SettingsSaveReason::SerializedJsonNotObject,
        super::super::SettingsSaveLegacyOutward::SerializedJsonNotObject(
            "semantic legacy".to_string(),
        ),
    );
    assert!(std::error::Error::source(&semantic_error).is_none());
}

#[test]
fn settings_save_io_kind_allowlist_matches_stable_1_97_1() {
    let rows = [
        (std::io::ErrorKind::NotFound, "not_found"),
        (std::io::ErrorKind::PermissionDenied, "permission_denied"),
        (std::io::ErrorKind::ConnectionRefused, "connection_refused"),
        (std::io::ErrorKind::ConnectionReset, "connection_reset"),
        (std::io::ErrorKind::HostUnreachable, "host_unreachable"),
        (
            std::io::ErrorKind::NetworkUnreachable,
            "network_unreachable",
        ),
        (std::io::ErrorKind::ConnectionAborted, "connection_aborted"),
        (std::io::ErrorKind::NotConnected, "not_connected"),
        (std::io::ErrorKind::AddrInUse, "addr_in_use"),
        (std::io::ErrorKind::AddrNotAvailable, "addr_not_available"),
        (std::io::ErrorKind::NetworkDown, "network_down"),
        (std::io::ErrorKind::BrokenPipe, "broken_pipe"),
        (std::io::ErrorKind::AlreadyExists, "already_exists"),
        (std::io::ErrorKind::WouldBlock, "would_block"),
        (std::io::ErrorKind::NotADirectory, "not_a_directory"),
        (std::io::ErrorKind::IsADirectory, "is_a_directory"),
        (std::io::ErrorKind::DirectoryNotEmpty, "directory_not_empty"),
        (
            std::io::ErrorKind::ReadOnlyFilesystem,
            "read_only_filesystem",
        ),
        (
            std::io::ErrorKind::StaleNetworkFileHandle,
            "stale_network_file_handle",
        ),
        (std::io::ErrorKind::InvalidInput, "invalid_input"),
        (std::io::ErrorKind::InvalidData, "invalid_data"),
        (std::io::ErrorKind::TimedOut, "timed_out"),
        (std::io::ErrorKind::WriteZero, "write_zero"),
        (std::io::ErrorKind::StorageFull, "storage_full"),
        (std::io::ErrorKind::NotSeekable, "not_seekable"),
        (std::io::ErrorKind::QuotaExceeded, "quota_exceeded"),
        (std::io::ErrorKind::FileTooLarge, "file_too_large"),
        (std::io::ErrorKind::ResourceBusy, "resource_busy"),
        (
            std::io::ErrorKind::ExecutableFileBusy,
            "executable_file_busy",
        ),
        (std::io::ErrorKind::Deadlock, "deadlock"),
        (std::io::ErrorKind::CrossesDevices, "crosses_devices"),
        (std::io::ErrorKind::TooManyLinks, "too_many_links"),
        (std::io::ErrorKind::InvalidFilename, "invalid_filename"),
        (
            std::io::ErrorKind::ArgumentListTooLong,
            "argument_list_too_long",
        ),
        (std::io::ErrorKind::Interrupted, "interrupted"),
        (std::io::ErrorKind::Unsupported, "unsupported"),
        (std::io::ErrorKind::UnexpectedEof, "unexpected_eof"),
        (std::io::ErrorKind::OutOfMemory, "out_of_memory"),
        (std::io::ErrorKind::Other, "other"),
    ];
    assert_eq!(rows.len(), 39);
    for (kind, expected) in rows {
        assert_eq!(super::super::io_error_kind_as_str(kind), expected);
        assert_eq!(expected_io_kind_from_literal_table(kind), expected);
    }
}

#[test]
fn terminal_snapshot_setting_invalid_defensive_projection_is_closed() {
    for stage in [
        super::super::SettingsSaveStage::PreserveDiskGate,
        super::super::SettingsSaveStage::ReconcileDiskGate,
    ] {
        let error = super::super::SettingsSaveError::semantic(
            stage,
            std::path::Path::new("C:/issue-1330/settings.json"),
            None,
            super::super::SettingsSaveReason::TerminalSnapshotSettingInvalid,
            super::super::SettingsSaveLegacyOutward::TerminalSnapshotSettingInvalid(
                "terminal_snapshot_setting_invalid".to_string(),
            ),
        );
        let diagnostic =
            error.safe_diagnostic(super::super::SettingsSaveReportSurface::GeneralSettings);
        assert_eq!(diagnostic.reason, "terminal_snapshot_setting_invalid");
        assert_eq!(diagnostic.code, "settings_save_failed");
        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let outward = super::super::report_settings_save_error(
            error,
            super::super::SettingsSaveReportSurface::GeneralSettings,
        );
        assert_eq!(outward, "terminal_snapshot_setting_invalid");
        assert_eq!(records.borrow().len(), 1);
    }
}

#[test]
fn general_settings_outward_compatibility_matrix() {
    fn assert_general_outward(error: super::super::SettingsSaveError, expected: &str) {
        let diagnostic_code = error.general_settings_diagnostic_code();
        let (records, _capture_guard) = capture_settings_save_diagnostics();
        let outward = super::super::report_settings_save_error(
            error,
            super::super::SettingsSaveReportSurface::GeneralSettings,
        );
        assert_eq!(outward, expected);
        let records = records.borrow();
        assert_eq!(records.len(), 1);
        assert_eq!(
            parse_settings_save_diagnostic(&records[0])["code"],
            diagnostic_code,
        );
    }

    let path = std::path::Path::new("C:/issue-1330/settings.json");
    assert_general_outward(
        super::super::SettingsSaveError::semantic(
            super::super::SettingsSaveStage::LockAcquire,
            path,
            None,
            super::super::SettingsSaveReason::LockTimedOut,
            super::super::SettingsSaveLegacyOutward::SettingsLockUnavailable,
        ),
        "settings_lock_unavailable",
    );
    assert_general_outward(
        super::super::SettingsSaveError::semantic(
            super::super::SettingsSaveStage::PrepareTarget,
            path,
            None,
            super::super::SettingsSaveReason::TargetNotRegularFile,
            super::super::SettingsSaveLegacyOutward::SettingsSaveFailed,
        ),
        "settings_save_failed",
    );
    assert_general_outward(
        super::super::SettingsSaveError::io(
            super::super::SettingsSaveStage::PreserveDiskGate,
            path,
            None,
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "reader source"),
            super::super::SettingsSaveLegacyOutward::DiskRead(
                "Failed to read C:/issue-1330/settings.json for a project-preserving save \
                        (aborting to avoid dropping a project): reader source"
                    .to_string(),
            ),
        ),
        "Failed to read C:/issue-1330/settings.json for a project-preserving save (aborting to \
                avoid dropping a project): reader source",
    );
    assert_general_outward(
        super::super::SettingsSaveError::json(
            super::super::SettingsSaveStage::PreserveDiskGate,
            path,
            None,
            serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
            super::super::SettingsSaveLegacyOutward::DiskJson(
                "Refusing to overwrite C:/issue-1330/settings.json: the existing settings file \
                        is not valid JSON (EOF while parsing an object at line 1 column \
                        1)"
                .to_string(),
            ),
        ),
        "Refusing to overwrite C:/issue-1330/settings.json: the existing settings file is not \
                valid JSON (EOF while parsing an object at line 1 column 1)",
    );
    assert_general_outward(
        super::super::SettingsSaveError::semantic(
            super::super::SettingsSaveStage::PreserveDiskGate,
            path,
            None,
            super::super::SettingsSaveReason::DiskJsonNotObject,
            super::super::SettingsSaveLegacyOutward::DiskJsonNotObject(
                "Refusing to overwrite C:/issue-1330/settings.json: the existing settings root \
                        is not a JSON object"
                    .to_string(),
            ),
        ),
        "Refusing to overwrite C:/issue-1330/settings.json: the existing settings root is not \
                a JSON object",
    );
    assert_general_outward(
        super::super::SettingsSaveError::semantic(
            super::super::SettingsSaveStage::ReconcileDiskGate,
            path,
            None,
            super::super::SettingsSaveReason::DiskSettingsValidationRejected,
            super::super::SettingsSaveLegacyOutward::DiskValidation(
                "Refusing to overwrite present settings whose non-project fields are invalid: \
                        invalid type: string \"invalid\", expected a boolean"
                    .to_string(),
            ),
        ),
        "Refusing to overwrite present settings whose non-project fields are invalid: invalid \
                type: string \"invalid\", expected a boolean",
    );
    let serialize_source = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    assert_general_outward(
        super::super::SettingsSaveError::json(
            super::super::SettingsSaveStage::Serialize,
            path,
            None,
            serialize_source,
            super::super::SettingsSaveLegacyOutward::Serialize(
                "Failed to serialize settings: independent source".to_string(),
            ),
        ),
        "Failed to serialize settings: independent source",
    );
    assert_general_outward(
        super::super::SettingsSaveError::semantic(
            super::super::SettingsSaveStage::Serialize,
            path,
            None,
            super::super::SettingsSaveReason::SerializedJsonNotObject,
            super::super::SettingsSaveLegacyOutward::SerializedJsonNotObject(
                "settings did not serialize to a JSON object".to_string(),
            ),
        ),
        "settings did not serialize to a JSON object",
    );
    assert_general_outward(
        super::super::SettingsSaveError::json(
            super::super::SettingsSaveStage::ReDecode,
            path,
            None,
            serde_json::from_str::<serde_json::Value>("{").unwrap_err(),
            super::super::SettingsSaveLegacyOutward::ReDecode(
                "Failed to re-decode written settings: independent source".to_string(),
            ),
        ),
        "Failed to re-decode written settings: independent source",
    );
}

#[test]
fn settings_save_diagnostic_redacts_values_and_encodes_untrusted_paths() {
    let target =
        std::path::PathBuf::from("C:\\target path\t\r\n\0\"=source_kind=json injected=true-ñ");
    let temporary = std::path::PathBuf::from("C:\\temp path\t\r\n\0\"=code=injected");
    let error = super::super::SettingsSaveError::io(
        super::super::SettingsSaveStage::TempWrite,
        &target,
        Some(&temporary),
        std::io::Error::other(
            "source-root-secret api-secret provider-secret serialized-settings-secret",
        ),
        super::super::SettingsSaveLegacyOutward::DiskRead(
            "legacy-root-secret api-secret provider-secret rootToken apiKey providerCredential"
                .to_string(),
        ),
    );
    let payload = super::super::render_settings_save_diagnostic(
        &error.safe_diagnostic(super::super::SettingsSaveReportSurface::GeneralSettings),
    );
    assert_eq!(payload.matches("target_path=").count(), 1);
    assert_eq!(payload.matches("temp_path=").count(), 1);
    for forbidden in [
        "source-root-secret",
        "api-secret",
        "provider-secret",
        "serialized-settings-secret",
        "legacy-root-secret",
        "rootToken",
        "apiKey",
        "providerCredential",
        "injected=true",
        "code=injected",
    ] {
        assert!(
            !payload.contains(forbidden),
            "leaked {forbidden}: {payload}"
        );
    }
    for required in [
        r"\u{20}", r"\u{9}", r"\u{d}", r"\u{a}", r"\u{0}", r"\u{22}", r"\u{3d}", r"\u{5c}",
        r"\u{f1}",
    ] {
        assert!(payload.contains(required), "missing {required}: {payload}");
    }
    let fields = parse_settings_save_diagnostic(&payload);
    for key in ["target_path", "temp_path"] {
        let value = &fields[key];
        assert!(!value.chars().any(char::is_whitespace));
        assert!(!value[1..value.len() - 1].contains('='));
        assert!(!value[1..value.len() - 1].contains('"'));
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]));
        assert!(super::super::render_diagnostic_path(&path).contains(r"\u{fffd}"));
    }
}
