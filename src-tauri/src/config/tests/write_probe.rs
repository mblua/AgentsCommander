use super::*;

#[test]
fn issue_1577_interrupted_gets_one_immediate_retry() {
    let mut calls = 0;
    let mut sleeps = Vec::new();
    let value = retry_transient_io_with_platform(
        RetryPlatform::Other,
        || {
            calls += 1;
            if calls == 1 {
                Err(Error::new(ErrorKind::Interrupted, "interrupted"))
            } else {
                Ok(1577)
            }
        },
        |delay| sleeps.push(delay.as_millis() as u64),
        |_, _, _, _| {},
    )
    .unwrap();

    assert_eq!(value, 1577);
    assert_eq!(calls, 2);
    assert!(sleeps.is_empty());

    calls = 0;
    let failure = retry_transient_io_with_platform::<(), _, _, _>(
        RetryPlatform::Other,
        || {
            calls += 1;
            Err(Error::new(ErrorKind::Interrupted, "still interrupted"))
        },
        |_| {},
        |_, _, _, _| {},
    )
    .unwrap_err();
    assert_eq!(calls, 2);
    assert_eq!(failure.attempts, 2);
}

#[test]
fn issue_1577_windows_transient_schedule_and_mixed_upper_bound() {
    for raw_os_error in [5, 32] {
        let mut calls = 0;
        let mut sleeps = Vec::new();
        let failure = retry_transient_io_with_platform::<(), _, _, _>(
            RetryPlatform::Windows,
            || {
                calls += 1;
                Err(Error::from_raw_os_error(raw_os_error))
            },
            |delay| sleeps.push(delay.as_millis() as u64),
            |_, _, _, _| {},
        )
        .unwrap_err();
        assert_eq!(calls, 6);
        assert_eq!(failure.attempts, 6);
        assert_eq!(sleeps, vec![15, 30, 60, 120, 240]);
    }

    let mut errors = VecDeque::from([
        Error::new(ErrorKind::Interrupted, "interrupted"),
        Error::from_raw_os_error(5),
        Error::from_raw_os_error(5),
        Error::from_raw_os_error(5),
        Error::from_raw_os_error(5),
        Error::from_raw_os_error(5),
        Error::from_raw_os_error(5),
    ]);
    let mut calls = 0;
    let mut sleeps = Vec::new();
    let failure = retry_transient_io_with_platform::<(), _, _, _>(
        RetryPlatform::Windows,
        || {
            calls += 1;
            Err(errors.pop_front().unwrap())
        },
        |delay| sleeps.push(delay.as_millis() as u64),
        |_, _, _, _| {},
    )
    .unwrap_err();
    assert_eq!(calls, 7);
    assert_eq!(failure.attempts, 7);
    assert_eq!(sleeps, vec![15, 30, 60, 120, 240]);
}

#[test]
fn issue_1577_non_windows_permission_and_read_only_never_sleep() {
    for kind in [ErrorKind::PermissionDenied, ErrorKind::ReadOnlyFilesystem] {
        let mut calls = 0;
        let mut sleeps = Vec::new();
        let failure = retry_transient_io_with_platform::<(), _, _, _>(
            RetryPlatform::Other,
            || {
                calls += 1;
                Err(Error::new(kind, "blocked"))
            },
            |delay| sleeps.push(delay),
            |_, _, _, _| {},
        )
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(failure.attempts, 1);
        assert!(sleeps.is_empty());
    }
}

#[test]
fn issue_1577_conclusive_classifier_is_a_closed_allowlist() {
    for error in [
        Error::new(ErrorKind::PermissionDenied, "permission"),
        Error::new(ErrorKind::ReadOnlyFilesystem, "read only"),
    ] {
        assert_eq!(
            classify_io_error_for_platform(RetryPlatform::Other, &error),
            ProbeFailureClass::ConclusiveUnwritable
        );
    }
    for raw_os_error in [5, 32] {
        assert_eq!(
            classify_io_error_for_platform(
                RetryPlatform::Windows,
                &Error::from_raw_os_error(raw_os_error)
            ),
            ProbeFailureClass::ConclusiveUnwritable
        );
    }
    for error in [
        Error::new(ErrorKind::Interrupted, "interrupted"),
        Error::new(ErrorKind::AlreadyExists, "exists"),
        Error::new(ErrorKind::NotFound, "missing"),
        Error::from_raw_os_error(12_345),
    ] {
        assert_eq!(
            classify_io_error_for_platform(RetryPlatform::Other, &error),
            ProbeFailureClass::Indeterminate
        );
    }
}

#[cfg(windows)]
#[test]
fn issue_1577_windows_raw_5_exhaustion_is_conclusive_without_real_acl() {
    let mut calls = 0;
    let mut sleeps = Vec::new();
    let failure = retry_transient_io_with::<(), _, _, _>(
        || {
            calls += 1;
            Err(Error::from_raw_os_error(5))
        },
        |delay| sleeps.push(delay.as_millis() as u64),
        |_, _, _, _| {},
    )
    .unwrap_err();
    let write_failure = failed_write(
        RetryPlatform::Windows,
        ProbeOperation::CreateConfigurationDirectory,
        expected_adjacent(),
        failure.error,
        failure.attempts,
    );
    let loc = resolve_instance_location(
        None,
        None,
        Ok(absolute_executable()),
        Some(PathBuf::from(r"C:\Users\tester")),
        WriteProbeOutcome::Failed(write_failure),
    );
    assert_eq!(calls, 6);
    assert_eq!(sleeps, vec![15, 30, 60, 120, 240]);
    assert_eq!(loc.config_dir, Some(expected_adjacent()));
    assert!(matches!(
        loc.startup_error,
        Some(ConfigStartupError::AdjacentDirectoryUnwritable { .. })
    ));
}

#[cfg(windows)]
#[test]
fn issue_1577_windows_raw_32_exhaustion_matches_raw_5() {
    let mut calls = 0;
    let mut sleeps = Vec::new();
    let failure = retry_transient_io_with::<(), _, _, _>(
        || {
            calls += 1;
            Err(Error::from_raw_os_error(32))
        },
        |delay| sleeps.push(delay.as_millis() as u64),
        |_, _, _, _| {},
    )
    .unwrap_err();
    assert_eq!(calls, 6);
    assert_eq!(failure.attempts, 6);
    assert_eq!(
        classify_io_error_for_platform(RetryPlatform::Windows, &failure.error),
        ProbeFailureClass::ConclusiveUnwritable
    );
    assert_eq!(sleeps, vec![15, 30, 60, 120, 240]);
}

#[test]
fn issue_1577_real_write_probe_keeps_directory_and_leaves_no_probe_file() {
    let temp = tempfile::TempDir::new().unwrap();
    let existing = temp.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    assert_eq!(probe_candidate_write(&existing), WriteProbeOutcome::Success);
    assert_eq!(std::fs::read_dir(&existing).unwrap().count(), 0);

    let missing = temp.path().join("missing");
    assert_eq!(probe_candidate_write(&missing), WriteProbeOutcome::Success);
    assert!(missing.is_dir());
    assert_eq!(std::fs::read_dir(&missing).unwrap().count(), 0);
}

#[test]
fn issue_1577_non_directory_candidate_is_indeterminate_and_hard() {
    let temp = tempfile::TempDir::new().unwrap();
    let candidate = temp.path().join("candidate");
    std::fs::write(&candidate, b"not a directory").unwrap();
    let WriteProbeOutcome::Failed(failure) = probe_candidate_write(&candidate) else {
        panic!("non-directory candidate must fail");
    };
    assert_eq!(failure.class, ProbeFailureClass::Indeterminate);

    let executable = temp.path().join("agentscommander_issue1577.exe");
    let loc = resolve_instance_location(
        None,
        None,
        Ok(executable),
        Some(PathBuf::from("/home/u")),
        WriteProbeOutcome::Failed(failure.clone()),
    );
    assert_eq!(
        loc.config_dir,
        Some(temp.path().join(".agentscommander_issue1577"))
    );
    assert_eq!(
        loc.startup_error,
        Some(ConfigStartupError::AdjacentSelectionBlocked {
            config_dir: temp.path().join(".agentscommander_issue1577"),
            reason: failure.reason(),
        })
    );
}

#[test]
fn issue_1577_cleanup_interrupted_then_success_discards_transient_error() {
    let mut removals = 0;
    let outcome = probe_candidate_write_with(
        RetryPlatform::Other,
        Path::new("candidate"),
        Path::new("candidate/probe.tmp"),
        |_| Ok(()),
        |_| Ok(()),
        |_| Err(Error::new(ErrorKind::PermissionDenied, "write denied")),
        |_| {
            removals += 1;
            if removals == 1 {
                Err(Error::new(ErrorKind::Interrupted, "interrupted"))
            } else {
                Ok(())
            }
        },
        |_| {},
    );
    let WriteProbeOutcome::Failed(failure) = outcome else {
        panic!("write failure must be retained");
    };
    assert_eq!(removals, 2);
    assert!(failure.cleanup.is_none());
    assert!(!failure.probe_may_remain);
    assert_eq!(failure.class, ProbeFailureClass::ConclusiveUnwritable);
}

#[test]
fn issue_1577_cleanup_windows_sharing_violation_then_success() {
    let mut removals = 0;
    let mut sleeps = Vec::new();
    let outcome = probe_candidate_write_with(
        RetryPlatform::Windows,
        Path::new("candidate"),
        Path::new("candidate/probe.tmp"),
        |_| Ok(()),
        |_| Ok(()),
        |_| Err(Error::new(ErrorKind::PermissionDenied, "write denied")),
        |_| {
            removals += 1;
            if removals == 1 {
                Err(Error::from_raw_os_error(32))
            } else {
                Ok(())
            }
        },
        |delay| sleeps.push(delay.as_millis() as u64),
    );
    let WriteProbeOutcome::Failed(failure) = outcome else {
        panic!("write failure must be retained");
    };
    assert_eq!(removals, 2);
    assert_eq!(sleeps, vec![15]);
    assert!(failure.cleanup.is_none());
    assert!(!failure.probe_may_remain);
}

#[test]
fn issue_1577_persistent_required_delete_retains_debris_diagnostic() {
    let outcome = probe_candidate_write_with(
        RetryPlatform::Other,
        Path::new("candidate"),
        Path::new("candidate/probe.tmp"),
        |_| Ok(()),
        |_| Ok(()),
        |_| Ok(()),
        |_| Err(Error::new(ErrorKind::PermissionDenied, "delete denied")),
        |_| {},
    );
    let WriteProbeOutcome::Failed(failure) = outcome else {
        panic!("required delete must fail");
    };
    assert_eq!(failure.class, ProbeFailureClass::ConclusiveUnwritable);
    assert!(failure.probe_may_remain);
    assert!(failure.cleanup.is_none());
    assert!(failure
        .reason()
        .ends_with("The probe file \"candidate/probe.tmp\" may remain."));
}

#[test]
fn issue_1577_unknown_cleanup_upgrades_conclusive_primary_to_indeterminate() {
    let outcome = probe_candidate_write_with(
        RetryPlatform::Other,
        Path::new("candidate"),
        Path::new("candidate/probe.tmp"),
        |_| Ok(()),
        |_| Ok(()),
        |_| Err(Error::new(ErrorKind::PermissionDenied, "write denied")),
        |_| Err(Error::other("cleanup unknown")),
        |_| {},
    );
    let WriteProbeOutcome::Failed(failure) = outcome else {
        panic!("write and cleanup must fail");
    };
    assert_eq!(
        failure.primary.class,
        ProbeFailureClass::ConclusiveUnwritable
    );
    assert_eq!(failure.class, ProbeFailureClass::Indeterminate);
    assert!(failure.cleanup.is_some());
    assert!(failure.probe_may_remain);
}

#[test]
fn issue_1577_startup_and_diagnostic_formatters_are_exact() {
    let candidate = PathBuf::from("bin/.agentscommander");
    let probe = PathBuf::from("bin/.agentscommander/probe.tmp");
    let primary = ProbeFailure::from_retry(
        RetryPlatform::Other,
        ProbeOperation::WriteProbeFile,
        probe.clone(),
        RetriedIoError {
            error: Error::new(ErrorKind::PermissionDenied, "write denied"),
            attempts: 1,
        },
    );
    let cleanup = ProbeFailure::from_retry(
        RetryPlatform::Other,
        ProbeOperation::DeleteProbeFile,
        probe.clone(),
        RetriedIoError {
            error: Error::other("cleanup unknown"),
            attempts: 2,
        },
    );
    let write_failure = WriteProbeFailure::new(primary, Some(cleanup), Some(probe.clone()), true);
    assert_eq!(
        write_failure.reason(),
        format!(
            "write probe could not write probe file \"{}\" after 1 attempt(s): write denied. \
                Cleanup of probe file \"{}\" also failed after 2 attempt(s): cleanup unknown; the \
                probe file may remain.",
            probe.display(),
            probe.display()
        )
    );

    let blocked_error = ConfigStartupError::AdjacentSelectionBlocked {
        config_dir: candidate.clone(),
        reason: write_failure.reason(),
    };
    assert_eq!(
        blocked_error.to_string(),
        format!(
            "AgentsCommander cannot start because configuration directory \"{}\" could not be \
                safely selected: {} Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and \
                restart.",
            candidate.display(),
            write_failure.reason()
        )
    );

    let error = ConfigStartupError::AdjacentDirectoryUnwritable {
        config_dir: candidate.clone(),
        reason: failed_write(
            RetryPlatform::Other,
            ProbeOperation::CreateConfigurationDirectory,
            candidate.clone(),
            Error::new(ErrorKind::PermissionDenied, "permission denied"),
            1,
        )
        .reason(),
    };
    assert_eq!(
        error.to_string(),
        format!(
            "AgentsCommander cannot start because it cannot write its configuration directory \
                \"{}\" next to the executable: write probe could not create configuration \
                directory \"{}\" after 1 attempt(s): permission denied. Move the executable to a \
                writable folder, or set AGENTSCOMMANDER_CONFIG_DIR to a writable directory, and \
                restart.",
            candidate.display(),
            candidate.display()
        )
    );
    let error = ConfigStartupError::AdjacentDirectoryUnwritable {
        config_dir: candidate.clone(),
        reason: "denied.".to_string(),
    };
    assert_eq!(
        error.to_string(),
        format!(
            "AgentsCommander cannot start because it cannot write its configuration directory \
                \"{}\" next to the executable: denied. Move the executable to a writable folder, \
                or set AGENTSCOMMANDER_CONFIG_DIR to a writable directory, and restart.",
            candidate.display()
        )
    );
}
