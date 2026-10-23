#[cfg(windows)]
use super::*;

#[cfg(windows)]
#[test]
fn injected_replace_error_matrix_retains_raw_code_and_never_retries() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_UNABLE_TO_MOVE_REPLACEMENT,
        ERROR_UNABLE_TO_MOVE_REPLACEMENT_2, ERROR_UNABLE_TO_REMOVE_REPLACED,
    };

    #[derive(Clone, Copy, Debug)]
    enum Mutation {
        StableNames,
        RemoveCanonical,
        RenameCanonical,
    }

    #[derive(Clone, Copy, Debug)]
    enum ExpectedError {
        StableFailure,
        RecoveryRequired,
        Partial(WindowsReplacePartial),
    }

    #[derive(Clone, Copy, Debug)]
    struct Case {
        label: &'static str,
        raw_error: u32,
        mutation: Mutation,
        expected: ExpectedError,
        canonical_present: bool,
        temp_present: bool,
        renamed_old_present: bool,
    }

    let cases = [
        Case {
            label: "1175 documented stable names",
            raw_error: ERROR_UNABLE_TO_REMOVE_REPLACED,
            mutation: Mutation::StableNames,
            expected: ExpectedError::StableFailure,
            canonical_present: true,
            temp_present: false,
            renamed_old_present: false,
        },
        Case {
            label: "1175 contradictory missing canonical",
            raw_error: ERROR_UNABLE_TO_REMOVE_REPLACED,
            mutation: Mutation::RemoveCanonical,
            expected: ExpectedError::RecoveryRequired,
            canonical_present: false,
            temp_present: true,
            renamed_old_present: false,
        },
        Case {
            label: "1176 documented removed canonical",
            raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT,
            mutation: Mutation::RemoveCanonical,
            expected: ExpectedError::Partial(
                WindowsReplacePartial::CanonicalRemovedReplacementAtTemp,
            ),
            canonical_present: false,
            temp_present: true,
            renamed_old_present: false,
        },
        Case {
            label: "1176 contradictory stable names",
            raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT,
            mutation: Mutation::StableNames,
            expected: ExpectedError::Partial(
                WindowsReplacePartial::CanonicalRemovedReplacementAtTemp,
            ),
            canonical_present: true,
            temp_present: true,
            renamed_old_present: false,
        },
        Case {
            label: "1177 documented renamed old destination",
            raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
            mutation: Mutation::RenameCanonical,
            expected: ExpectedError::Partial(
                WindowsReplacePartial::ReplacementEnrichedOldDestinationRenamed,
            ),
            canonical_present: false,
            temp_present: true,
            renamed_old_present: true,
        },
        Case {
            label: "1177 contradictory stable names",
            raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
            mutation: Mutation::StableNames,
            expected: ExpectedError::Partial(
                WindowsReplacePartial::ReplacementEnrichedOldDestinationRenamed,
            ),
            canonical_present: true,
            temp_present: true,
            renamed_old_present: false,
        },
        Case {
            label: "other access denied stable names",
            raw_error: ERROR_ACCESS_DENIED,
            mutation: Mutation::StableNames,
            expected: ExpectedError::StableFailure,
            canonical_present: true,
            temp_present: false,
            renamed_old_present: false,
        },
        Case {
            label: "other sharing error contradictory state",
            raw_error: ERROR_SHARING_VIOLATION,
            mutation: Mutation::RemoveCanonical,
            expected: ExpectedError::RecoveryRequired,
            canonical_present: false,
            temp_present: true,
            renamed_old_present: false,
        },
    ];

    for case in cases {
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
            ManifestRecordOutcome::Recorded,
            "{}",
            case.label
        );
        initial.release();

        let canonical = canonical_path(&project);
        let original = std::fs::read(&canonical).unwrap();
        let renamed_old = project.join(".ac").join("renamed-old-destination");
        let call_count = Arc::new(AtomicUsize::new(0));
        let hooks = TestFilesystemHooks {
            windows_namespace_call: Some(Arc::new({
                let call_count = Arc::clone(&call_count);
                let renamed_old = renamed_old.clone();
                move |operation, _temp, canonical| {
                    assert_eq!(operation, WindowsNamespaceOperation::ReplaceFile);
                    call_count.fetch_add(1, Ordering::SeqCst);
                    match case.mutation {
                        Mutation::StableNames => {}
                        Mutation::RemoveCanonical => {
                            std::fs::remove_file(canonical).unwrap();
                        }
                        Mutation::RenameCanonical => {
                            std::fs::rename(canonical, &renamed_old).unwrap();
                        }
                    }
                    Err(case.raw_error)
                }
            })),
            ..TestFilesystemHooks::default()
        };
        let mut guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
                .unwrap();
        let mut desired = read_disk_state(&project);
        let _ = desired.upsert(
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        );
        let error = guard
            .write_canonical(serialize_state(&desired).unwrap())
            .unwrap_err();
        assert_eq!(call_count.load(Ordering::SeqCst), 1, "{}", case.label);

        let observed_raw = match &error {
            SeedManifestError::WindowsNamespaceFailure {
                raw_error,
                canonical_state,
                temp_state,
                ..
            } => {
                assert!(matches!(case.expected, ExpectedError::StableFailure));
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
                        bytes_match: true,
                        ..
                    }
                ));
                *raw_error
            }
            SeedManifestError::WindowsReplaceRecoveryRequired { raw_error, .. } => {
                assert!(matches!(case.expected, ExpectedError::RecoveryRequired));
                *raw_error
            }
            SeedManifestError::WindowsReplacePartial {
                raw_error, partial, ..
            } => {
                let ExpectedError::Partial(expected_partial) = case.expected else {
                    panic!("{} returned an unexpected partial error", case.label);
                };
                assert_eq!(*partial, expected_partial);
                *raw_error
            }
            other => panic!("{} returned unexpected error {other:?}", case.label),
        };
        assert_eq!(observed_raw, case.raw_error, "{}", case.label);
        let display = format!("{error}");
        assert!(
            display.contains(&case.raw_error.to_string()),
            "{}",
            case.label
        );
        assert!(
            display.contains(&format!("0x{:08x}", case.raw_error)),
            "{}",
            case.label
        );
        guard.release();

        assert_eq!(canonical.exists(), case.canonical_present, "{}", case.label);
        if case.canonical_present {
            assert_eq!(
                std::fs::read(&canonical).unwrap(),
                original,
                "{}",
                case.label
            );
        }
        assert_eq!(
            exact_temp_paths(&project).len(),
            usize::from(case.temp_present),
            "{}",
            case.label
        );
        assert_eq!(
            renamed_old.exists(),
            case.renamed_old_present,
            "{}",
            case.label
        );
        if case.renamed_old_present {
            assert_eq!(
                std::fs::read(&renamed_old).unwrap(),
                original,
                "{}",
                case.label
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn injected_move_failures_clean_only_the_exact_prepublication_state() {
    use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

    for move_before_error in [false, true] {
        let (_temp, project) = setup_project();
        let hooks = TestFilesystemHooks {
            windows_namespace_call: Some(Arc::new(move |operation, temp, canonical| {
                assert_eq!(operation, WindowsNamespaceOperation::MoveFileEx);
                if move_before_error {
                    std::fs::rename(temp, canonical).unwrap();
                }
                Err(ERROR_ACCESS_DENIED)
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
        guard.release();
        if move_before_error {
            assert_eq!(
                outcome,
                ManifestRecordOutcome::PublishedUnrecorded(
                    ManifestDegradedReason::RecoveryRequired
                )
            );
            assert!(canonical_path(&project).is_file());
        } else {
            assert_eq!(
                outcome,
                ManifestRecordOutcome::PublishedUnrecorded(
                    ManifestDegradedReason::PersistenceFailure
                )
            );
            assert!(!canonical_path(&project).exists());
            assert!(exact_temp_paths(&project).is_empty());
        }
    }
}

#[cfg(windows)]
#[test]
fn successful_call_with_substituted_final_identity_is_not_recorded_or_cleaned() {
    let (_temp, project) = setup_project();
    let foreign = b"foreign final object".to_vec();
    let hooks = TestFilesystemHooks {
        windows_namespace_call: Some(Arc::new({
            let foreign = foreign.clone();
            move |operation, temp, canonical| {
                assert_eq!(operation, WindowsNamespaceOperation::MoveFileEx);
                std::fs::rename(temp, canonical).unwrap();
                std::fs::remove_file(canonical).unwrap();
                std::fs::write(canonical, &foreign).unwrap();
                Ok(())
            }
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
    guard.release();
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::IdentityConflict)
    );
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), foreign);
}

#[cfg(windows)]
#[test]
fn handle_bound_cleanup_never_deletes_a_substituted_path() {
    let (_temp, project) = setup_project();
    let path = project
        .join(".ac")
        .join(".seed-manifest.00000000-0000-0000-0000-000000000001.tmp");
    let mut opened = OpenedRegularFile::create_new(&path).unwrap();
    opened.file.write_all(b"owned").unwrap();
    opened.file.sync_all().unwrap();
    opened.refresh_facts().unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"substitution").unwrap();
    assert!(matches!(
        cleanup_current_owned_temp(&opened, Some(b"owned")),
        Err(SeedManifestError::UnsafePath { .. })
    ));
    drop(opened);
    assert_eq!(std::fs::read(path).unwrap(), b"substitution");
}

#[cfg(windows)]
#[test]
fn replace_preserves_certified_ntfs_metadata_surface_and_both_compression_directions() {
    use windows_sys::Win32::Storage::FileSystem::{
        COMPRESSION_FORMAT_DEFAULT, COMPRESSION_FORMAT_NONE, FILE_ATTRIBUTE_COMPRESSED,
    };

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
    let protected_dacl = windows_protect_existing_dacl(&canonical);
    let ads = windows_ads_path(&canonical, "agentscommander-stage-a");
    std::fs::write(&ads, b"destination stream").unwrap();
    windows_set_compression(&canonical, COMPRESSION_FORMAT_DEFAULT);
    let before = OpenedRegularFile::open_existing(&canonical, false).unwrap();
    assert_ne!(before.attributes & FILE_ATTRIBUTE_COMPRESSED, 0);
    let creation_time = before.creation_time;
    drop(before);

    let mut compressed = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        compressed.publication_permit().record_file(
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
    compressed.release();
    let after_compressed = OpenedRegularFile::open_existing(&canonical, false).unwrap();
    assert_eq!(after_compressed.creation_time, creation_time);
    assert_ne!(after_compressed.attributes & FILE_ATTRIBUTE_COMPRESSED, 0);
    assert_eq!(windows_dacl_bytes(&canonical), protected_dacl);
    assert_eq!(std::fs::read(&ads).unwrap(), b"destination stream");
    drop(after_compressed);

    windows_set_compression(&canonical, COMPRESSION_FORMAT_NONE);
    assert_eq!(
        OpenedRegularFile::open_existing(&canonical, false)
            .unwrap()
            .attributes
            & FILE_ATTRIBUTE_COMPRESSED,
        0
    );
    let mut uncompressed = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        uncompressed.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:43:00.000Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    uncompressed.release();
    assert_eq!(
        OpenedRegularFile::open_existing(&canonical, false)
            .unwrap()
            .attributes
            & FILE_ATTRIBUTE_COMPRESSED,
        0
    );
    assert_eq!(std::fs::read(&ads).unwrap(), b"destination stream");
}

#[cfg(windows)]
#[test]
fn ordinary_attribute_inputs_are_observed_without_a_preservation_assertion() {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_ATTRIBUTE_SYSTEM,
    };

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
    let selected = FILE_ATTRIBUTE_ARCHIVE
        | FILE_ATTRIBUTE_HIDDEN
        | FILE_ATTRIBUTE_SYSTEM
        | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED;
    windows_set_attributes(&canonical, selected);
    let mut set_case = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        set_case.publication_permit().record_file(
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
    set_case.release();
    let final_set = OpenedRegularFile::open_existing(&canonical, false)
        .unwrap()
        .attributes
        & selected;

    windows_set_attributes(&canonical, FILE_ATTRIBUTE_NORMAL);
    let mut clear_case = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        clear_case.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:43:00.000Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    clear_case.release();
    let final_clear = OpenedRegularFile::open_existing(&canonical, false)
        .unwrap()
        .attributes
        & selected;
    eprintln!(
        "ordinary attribute non-guarantee observation set=0x{final_set:08x} \
            clear=0x{final_clear:08x}"
    );
}
