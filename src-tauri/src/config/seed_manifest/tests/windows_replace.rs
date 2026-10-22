use super::*;

#[cfg(windows)]
#[test]
fn open_read_write_temp_handle_conflicts_with_replace_even_with_delete_access() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::{
        GetLastError, ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        ReplaceFileW, DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let (_temp, project) = setup_project();
    let canonical = canonical_path(&project);
    std::fs::write(&canonical, b"old canonical").unwrap();
    for include_delete in [false, true] {
        let temp_path = project.join(".ac").join(format!(
            ".seed-manifest.{}-writer-conflict.tmp",
            if include_delete { "delete" } else { "ordinary" }
        ));
        let file = if include_delete {
            OpenOptions::new()
                .read(true)
                .write(true)
                .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .create_new(true)
                .open(&temp_path)
                .unwrap()
        } else {
            open_regular_no_follow(&temp_path, true, true).unwrap()
        };
        let mut opened = OpenedRegularFile::from_file(&temp_path, file).unwrap();
        let replacement = b"replacement";
        opened.file.write_all(replacement).unwrap();
        opened.file.sync_all().unwrap();
        opened.refresh_facts().unwrap();
        let temp_wide = absolute_verbatim_utf16(&temp_path).unwrap();
        let canonical_wide = absolute_verbatim_utf16(&canonical).unwrap();
        let success = unsafe {
            ReplaceFileW(
                canonical_wide.as_ptr(),
                temp_wide.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(success, 0);
        let raw_error = unsafe { GetLastError() };
        assert_eq!(raw_error, ERROR_SHARING_VIOLATION);
        assert_eq!(std::fs::read(&canonical).unwrap(), b"old canonical");
        assert_eq!(std::fs::read(&temp_path).unwrap(), replacement);
        cleanup_current_owned_temp(&opened, Some(replacement)).unwrap();
        drop(opened);
        assert!(!temp_path.exists());
    }
}

#[cfg(windows)]
#[test]
fn zero_access_witness_replace_preserves_old_handle_identity_with_zero_links() {
    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let mut first = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        first.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    first.release();

    let canonical = canonical_path(&project);
    let old = OpenedRegularFile::open_existing(&canonical, false).unwrap();
    let old_identity = old.identity;
    let mut second = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        second.publication_permit().record_file(
            &activation,
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    second.release();

    let final_file = OpenedRegularFile::open_existing(&canonical, false).unwrap();
    assert_ne!(final_file.identity, old_identity);
    let old_facts = handle_facts(&old.file).unwrap();
    assert_eq!(old_facts.identity, old_identity);
    assert_eq!(old_facts.links, 0);
}

#[cfg(windows)]
#[test]
fn destination_share_denial_returns_typed_error_and_preserves_both_states() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        initial.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    initial.release();

    let canonical = canonical_path(&project);
    let original = std::fs::read(&canonical).unwrap();
    let mut desired = read_disk_state(&project);
    let _ = desired.upsert(
        context_row(
            "Context.coordinator.md",
            "context:coordinator",
            "2026-07-16T19:42:00.000Z",
        )
        .unwrap(),
    );
    let desired_bytes = serialize_state(&desired).unwrap();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let blocker = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&canonical)
        .unwrap();

    let error = guard.write_canonical(desired_bytes).unwrap_err();
    match &error {
        SeedManifestError::WindowsNamespaceFailure {
            operation,
            raw_error,
            canonical_state,
            temp_state,
            ..
        } => {
            assert_eq!(*operation, WindowsNamespaceOperation::ReplaceFile);
            assert_eq!(*raw_error, ERROR_SHARING_VIOLATION);
            assert!(matches!(
                canonical_state.as_ref(),
                WindowsPathState::Same {
                    bytes_match: true,
                    ..
                }
            ));
            assert!(matches!(
                temp_state.as_ref(),
                WindowsPathState::Same {
                    links: 1,
                    bytes_match: true,
                    ..
                }
            ));
        }
        other => panic!("expected typed stable-state namespace failure, got {other:?}"),
    }
    assert_eq!(
        error.degraded_reason(),
        ManifestDegradedReason::PersistenceFailure
    );
    guard.release();
    drop(blocker);
    assert_eq!(std::fs::read(&canonical).unwrap(), original);
    assert!(exact_temp_paths(&project).is_empty());
}

#[cfg(windows)]
#[test]
fn writer_supports_non_ascii_verbatim_paths_beyond_max_path() {
    use std::os::windows::ffi::OsStrExt;

    let temp = tempfile::tempdir().unwrap();
    let mut project = temp.path().join("project");
    while project.as_os_str().encode_wide().count() < 430 {
        project.push("長い-path-component-0123456789abcdef");
    }
    std::fs::create_dir_all(project.join(".ac")).unwrap();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    guard.release();
    assert!(canonical_path(&project).is_file());
}

#[cfg(windows)]
#[test]
fn verbatim_conversion_covers_long_unc_paths_and_rejects_interior_nul() {
    use std::os::windows::ffi::OsStringExt;

    let unc = PathBuf::from(format!(
        "\\\\server\\share\\{}",
        "長い-unc-component-0123456789abcdef\\".repeat(12)
    ));
    let converted = absolute_verbatim_utf16(&unc).unwrap();
    let expected_prefix = "\\\\?\\UNC\\server\\share\\"
        .encode_utf16()
        .collect::<Vec<_>>();
    assert!(converted.starts_with(&expected_prefix));
    assert_eq!(converted.last(), Some(&0));
    assert!(converted.len() > 260);

    let with_nul = PathBuf::from(std::ffi::OsString::from_wide(&[
        u16::from(b'C'),
        u16::from(b':'),
        u16::from(b'\\'),
        u16::from(b'a'),
        0,
        u16::from(b'b'),
    ]));
    let error = absolute_verbatim_utf16(&with_nul).unwrap_err();
    assert!(matches!(
        error,
        SeedManifestError::Validation(message)
            if message == "Windows namespace path contains an interior NUL"
    ));
}

#[cfg(windows)]
#[test]
fn windows_namespace_power_loss_durability_is_explicitly_unclaimed() {
    assert!(!std::hint::black_box(
        WINDOWS_NAMESPACE_POWER_LOSS_DURABILITY_CLAIMED
    ));
}

#[cfg(windows)]
#[test]
fn named_stream_injected_into_temp_aborts_and_preserves_candidate() {
    let (_temp, project) = setup_project();
    let hooks = TestFilesystemHooks {
        before_temp_validation: Some(Arc::new(|temp| {
            std::fs::write(windows_ads_path(temp, "injected"), b"foreign stream").unwrap();
        })),
        ..TestFilesystemHooks::default()
    };
    let mut guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
            .unwrap();
    let outcome = guard.publication_permit().record_file(
        &ManifestActivationToken::for_test(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::UnsafePath)
    );
    guard.release();
    assert!(!canonical_path(&project).exists());
    assert_eq!(exact_temp_paths(&project).len(), 1);
}

#[cfg(windows)]
#[test]
fn read_only_canonical_is_preserved_without_creating_a_temp() {
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_READONLY;

    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let mut first = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        first.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    first.release();
    let canonical = canonical_path(&project);
    let original = std::fs::read(&canonical).unwrap();
    let attributes = OpenedRegularFile::open_existing(&canonical, false)
        .unwrap()
        .attributes;
    windows_set_attributes(&canonical, attributes | FILE_ATTRIBUTE_READONLY);

    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let outcome = guard.publication_permit().record_file(
        &activation,
        context_row(
            "Context.coordinator.md",
            "context:coordinator",
            "2026-07-16T19:42:00.000Z",
        )
        .unwrap(),
    );
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ReadOnlyCanonical)
    );
    guard.release();
    assert_eq!(std::fs::read(&canonical).unwrap(), original);
    assert!(exact_temp_paths(&project).is_empty());
    windows_set_attributes(&canonical, attributes & !FILE_ATTRIBUTE_READONLY);
}

#[cfg(windows)]
#[test]
fn injected_replace_errors_follow_1175_1176_and_1177_recovery_policy() {
    use windows_sys::Win32::Foundation::{
        ERROR_UNABLE_TO_MOVE_REPLACEMENT, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
        ERROR_UNABLE_TO_REMOVE_REPLACED,
    };

    for raw_error in [
        ERROR_UNABLE_TO_REMOVE_REPLACED,
        ERROR_UNABLE_TO_MOVE_REPLACEMENT,
        ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
    ] {
        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            initial.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        initial.release();
        let canonical = canonical_path(&project);
        let original = std::fs::read(&canonical).unwrap();
        let renamed_old = project.join(".ac").join("old-destination-recovery");
        let hooks = TestFilesystemHooks {
            windows_namespace_call: Some(Arc::new({
                let renamed_old = renamed_old.clone();
                move |operation, _temp, canonical| {
                    assert_eq!(operation, WindowsNamespaceOperation::ReplaceFile);
                    if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT {
                        std::fs::remove_file(canonical).unwrap();
                    } else if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 {
                        std::fs::rename(canonical, &renamed_old).unwrap();
                    }
                    Err(raw_error)
                }
            })),
            ..TestFilesystemHooks::default()
        };
        let mut guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
                .unwrap();
        let outcome = guard.publication_permit().record_file(
            &activation,
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        );
        guard.release();

        if raw_error == ERROR_UNABLE_TO_REMOVE_REPLACED {
            assert_eq!(
                outcome,
                ManifestRecordOutcome::PublishedUnrecorded(
                    ManifestDegradedReason::PersistenceFailure
                )
            );
            assert_eq!(std::fs::read(&canonical).unwrap(), original);
            assert!(exact_temp_paths(&project).is_empty());
        } else {
            assert_eq!(
                outcome,
                ManifestRecordOutcome::PublishedUnrecorded(
                    ManifestDegradedReason::RecoveryRequired
                )
            );
            let candidates = exact_temp_paths(&project);
            assert_eq!(candidates.len(), 1);
            if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT {
                assert!(!canonical.exists());
                std::fs::write(
                    &canonical,
                    serialize_state(&ManifestState::default()).unwrap(),
                )
                .unwrap();
                let next = ProjectSeedManifestGuard::acquire(&project).unwrap();
                next.release();
                assert!(candidates[0].is_file());
            } else {
                assert!(!canonical.exists());
                assert_eq!(std::fs::read(&renamed_old).unwrap(), original);
            }
        }
    }
}
