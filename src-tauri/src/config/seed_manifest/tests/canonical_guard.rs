use super::*;

#[test]
fn fixed_buffer_stream_comparison_detects_content_and_length_changes() {
    let temp = tempfile::NamedTempFile::new().unwrap();
    let bytes = vec![b'a'; RAW_COMPARE_BUFFER_BYTES * 2 + 7];
    std::fs::write(temp.path(), &bytes).unwrap();
    let mut file = File::open(temp.path()).unwrap();
    assert!(stream_matches(&mut file, &bytes).unwrap());
    let mut changed = bytes.clone();
    changed[RAW_COMPARE_BUFFER_BYTES + 1] = b'b';
    assert!(!stream_matches(&mut file, &changed).unwrap());
    assert!(!stream_matches(&mut file, &bytes[..bytes.len() - 1]).unwrap());
}

#[test]
fn independent_handles_contend_and_persistent_lock_releases() {
    let (_temp, project) = setup_project();
    let first = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let started = Instant::now();
    let error =
        ProjectSeedManifestGuard::acquire_with_timeout(&project, Duration::from_millis(120))
            .unwrap_err();
    assert!(matches!(error, SeedManifestError::BusyTimeout { .. }));
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(started.elapsed() < Duration::from_secs(1));
    let lock_path = project.join(".ac").join(SEED_MANIFEST_LOCK_FILENAME);
    assert!(lock_path.is_file());
    first.release();

    let second =
        ProjectSeedManifestGuard::acquire_with_timeout(&project, Duration::from_millis(250))
            .unwrap();
    second.release();
    assert!(lock_path.is_file(), "unlock must not delete the lock file");
}

#[cfg(windows)]
#[test]
fn lock_share_denial_is_precontention_lock_io_and_preserves_the_lock_file() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let (_temp, project) = setup_project();
    let lock_path = project.join(".ac").join(SEED_MANIFEST_LOCK_FILENAME);
    let original = b"persistent lock sentinel";
    std::fs::write(&lock_path, original).unwrap();
    let canonical_lock_path = std::fs::canonicalize(&lock_path).unwrap();
    let blocker = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&lock_path)
        .unwrap();

    let error =
        ProjectSeedManifestGuard::acquire_with_timeout(&project, Duration::from_millis(100))
            .unwrap_err();
    match &error {
        SeedManifestError::LockIo {
            path,
            saw_contention,
            source,
        } => {
            assert_eq!(path, &canonical_lock_path);
            assert!(!saw_contention);
            assert_eq!(
                source
                    .raw_os_error()
                    .and_then(|raw| u32::try_from(raw).ok()),
                Some(ERROR_SHARING_VIOLATION)
            );
        }
        other => panic!("expected pre-contention LockIo, got {other:?}"),
    }
    assert_eq!(
        error.degraded_reason(),
        ManifestDegradedReason::LockUnavailable
    );
    drop(blocker);
    assert_eq!(std::fs::read(&lock_path).unwrap(), original);
}

#[test]
fn canonical_diagnostics_and_guard_debug_never_expose_manifest_contents() {
    let secret = "PRIVATE-SEED-CONTENT-7f9c1a";
    let syntax_bytes = format!("{secret} = [\n");
    let syntax_error = parse_manifest_bytes(syntax_bytes.as_bytes()).unwrap_err();
    assert!(matches!(syntax_error, SeedManifestError::Parse(_)));
    assert!(!format!("{syntax_error}").contains(secret));
    assert!(!format!("{syntax_error:?}").contains(secret));
    assert!(!format!("{syntax_error}").contains("1 |"));

    let invalid_wire = format!(
        concat!(
            "schema_version = 1\n",
            "coverage_version = 2\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\", \
                \"coding_agent_catalog\"]\n",
            "\n",
            "[[files]]\n",
            "path = \".ac/{secret}\"\n",
            "path_encoding = \"utf8\"\n",
            "kind = \"project_context_template\"\n",
            "scope = \"context:agentscommander\"\n",
            "source = \"builtin\"\n",
            "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n"
        ),
        secret = secret
    );
    let validation_error = parse_manifest_bytes(invalid_wire.as_bytes()).unwrap_err();
    assert!(matches!(validation_error, SeedManifestError::Validation(_)));
    assert!(!format!("{validation_error}").contains(secret));
    assert!(!format!("{validation_error:?}").contains(secret));

    let (_invalid_temp, invalid_project) = setup_project();
    std::fs::write(canonical_path(&invalid_project), syntax_bytes.as_bytes()).unwrap();
    let mut invalid_guard = ProjectSeedManifestGuard::acquire(&invalid_project).unwrap();
    let invalid_debug = format!("{invalid_guard:?}");
    assert!(!invalid_debug.contains(secret));
    assert_eq!(
        invalid_guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
    );
    invalid_guard.release();
    assert_eq!(
        std::fs::read(canonical_path(&invalid_project)).unwrap(),
        syntax_bytes.as_bytes()
    );

    let (_temp, project) = setup_project();
    std::fs::write(
        canonical_path(&project),
        serialize_raw_legacy_fixture(&populated_state()),
    )
    .unwrap();
    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let held = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(held.rows.len(), 3);
    assert!(held
        .rows
        .values()
        .any(|row| row.kind == ManifestFileKind::ReplicaConfigFile));
    let debug = format!("{guard:?}");
    assert!(debug.contains("raw_len"));
    assert!(debug.contains("row_count"));
    for content in [
        "Context.AgentsCommander.md",
        "settings.json",
        "context:agentscommander",
        "workspace_base",
    ] {
        assert!(!debug.contains(content), "debug leaked {content}");
    }
    guard.release();
}

#[test]
fn corrupt_and_future_canonical_bytes_are_preserved_read_only() {
    for (name, bytes, expected_reason) in [
        (
            "corrupt",
            b"<<<<<<< conflict\n".as_slice(),
            ManifestDegradedReason::InvalidCanonical,
        ),
        (
            "future",
            concat!(
                "schema_version = 2\n",
                "coverage_version = 1\n",
                "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
                "files = []\n"
            )
            .as_bytes(),
            ManifestDegradedReason::FutureSchema,
        ),
    ] {
        let (_temp, project) = setup_project();
        let path = canonical_path(&project);
        std::fs::write(&path, bytes).unwrap();
        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
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
            ManifestRecordOutcome::PublishedUnrecorded(expected_reason),
            "case {name}"
        );
        guard.release();
        assert_eq!(std::fs::read(path).unwrap(), bytes, "case {name}");
    }
}

#[test]
fn nonregular_canonical_fails_closed() {
    let (_temp, project) = setup_project();
    std::fs::create_dir(canonical_path(&project)).unwrap();
    let error = ProjectSeedManifestGuard::acquire(&project).unwrap_err();
    assert!(matches!(error, SeedManifestError::UnsafePath { .. }));
}

#[cfg(unix)]
#[test]
fn symlink_canonical_and_lock_fail_closed_without_following() {
    use std::os::unix::fs::symlink;

    for filename in [SEED_MANIFEST_FILENAME, SEED_MANIFEST_LOCK_FILENAME] {
        let (_temp, project) = setup_project();
        let outside = project.join("outside");
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, project.join(".ac").join(filename)).unwrap();
        let error = ProjectSeedManifestGuard::acquire(&project).unwrap_err();
        assert!(matches!(error, SeedManifestError::UnsafePath { .. }));
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
    }
}

#[test]
fn prior_temp_policy_is_platform_specific_and_preserves_lookalikes() {
    let (_temp, project) = setup_project();
    let ac = project.join(".ac");
    std::fs::write(
        canonical_path(&project),
        serialize_state(&ManifestState::default()).unwrap(),
    )
    .unwrap();
    let valid = ac.join(".seed-manifest.00000000-0000-0000-0000-000000000000.tmp");
    let uppercase = ac.join(".seed-manifest.00000000-0000-0000-0000-00000000000A.tmp");
    let malformed = ac.join(".seed-manifest.not-a-uuid.tmp");
    let directory = ac.join(".seed-manifest.11111111-1111-1111-1111-111111111111.tmp");
    std::fs::write(&valid, b"stale").unwrap();
    std::fs::write(&uppercase, b"user").unwrap();
    std::fs::write(&malformed, b"user").unwrap();
    std::fs::create_dir(&directory).unwrap();

    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    guard.release();
    #[cfg(unix)]
    assert!(!valid.exists());
    #[cfg(windows)]
    assert!(valid.is_file());
    assert!(uppercase.is_file());
    assert!(malformed.is_file());
    assert!(directory.is_dir());
}

#[test]
fn second_hard_link_on_temp_blocks_publication() {
    let (_temp, project) = setup_project();
    let alias = project.join(".ac").join("temp-alias");
    let hooks = TestFilesystemHooks {
        before_temp_validation: Some(Arc::new({
            let alias = alias.clone();
            move |temp| {
                std::fs::hard_link(temp, &alias).expect("create adversarial temp hard link");
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
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::UnsafePath)
    );
    assert!(!canonical_path(&project).exists());
    assert!(alias.is_file());
    guard.release();
}

#[test]
fn raw_external_edit_conflict_preserves_external_bytes() {
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

    let external = b"external lock-unaware edit\n".to_vec();
    let hooks = TestFilesystemHooks {
        before_raw_conflict_check: Some(Arc::new({
            let external = external.clone();
            move |canonical| std::fs::write(canonical, &external).unwrap()
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
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ExternalEdit)
    );
    guard.release();
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), external);
}
