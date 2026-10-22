use super::super::*;
use serde_json::json;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(5);
const SOFT_DEADLINE: Duration = Duration::from_secs(30);

fn instance(base: Option<Value>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(SETTINGS_FILE_NAME);
    if let Some(base) = base {
        write_json(&path, &base);
    }
    (dir, path)
}

fn write_json(path: &Path, value: &Value) {
    let bytes = serde_json::to_vec_pretty(value).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn write_raw(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

fn voice_enabled(owner: &SettingsOwner) -> bool {
    let snapshot = owner.blocking_current_snapshot().unwrap();
    snapshot.voice_to_text_enabled
}

/// A complete, valid base: `defaultShell` and `defaultShellArgs` are required
/// fields, so a partial object is a present-but-undecodable base the owner
/// must refuse, never a fixture.
fn voice(enabled: bool) -> Value {
    let mut base = serde_json::to_value(AppSettings::default()).unwrap();
    base["voiceToTextEnabled"] = json!(enabled);
    base
}

fn counts(owner: &SettingsOwner) -> [u64; 3] {
    let counters = &owner.counters;
    [
        counters.transaction_lock_attempts.load(Ordering::Relaxed),
        counters.captures.load(Ordering::Relaxed),
        counters.source_reads.load(Ordering::Relaxed),
    ]
}

fn overlay_path(path: &Path) -> PathBuf {
    path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME)
}

/// Link-free physical aliases of `dir`: its canonical spelling when that
/// differs textually (Windows verbatim prefix, macOS `/var` -> `/private/var`),
/// and a case variant of its leaf when the filesystem resolves it to the same
/// directory. Each is checked to canonicalize to the same directory.
fn physical_aliases(dir: &Path) -> Vec<PathBuf> {
    let canonical = std::fs::canonicalize(dir).unwrap();
    let mut aliases = Vec::new();
    if canonical != dir {
        aliases.push(canonical.clone());
    }
    let leaf = dir.file_name().unwrap().to_string_lossy().into_owned();
    let upper = leaf.to_ascii_uppercase();
    let flipped = if upper != leaf {
        upper
    } else {
        leaf.to_ascii_lowercase()
    };
    let variant = dir.with_file_name(flipped);
    let same = std::fs::canonicalize(&variant).is_ok_and(|resolved| resolved == canonical);
    if variant != dir && same {
        aliases.push(variant);
    }
    aliases
}

fn is_compose(error: &SettingsOwnerError) -> bool {
    matches!(error, SettingsOwnerError::Compose(_))
}

#[test]
fn e2992_p41_capture_locked_once() {
    // Positive control: only the base exists; the other three sources are
    // legitimately absent and compose without defaults replacing the base.
    let (_dir, path) = instance(Some(voice(true)));
    let owner = SettingsOwner::open_active(&path, WAIT).unwrap();
    assert_eq!(counts(&owner), [1, 1, 4]);
    let snapshot = owner.blocking_current_snapshot().unwrap();
    assert!(snapshot.voice_to_text_enabled);
    assert!(snapshot.project_paths.is_empty());
    assert_eq!(snapshot.agents_layer, AgentsLayerState::Readable);

    // One operation: one transactional lock attempt and one capture; the
    // locked halves consume the guard and never reacquire the lock.
    let report = owner
        .with_operation_blocking("probe", |current, guard| {
            assert!(current.voice_to_text_enabled);
            let reacquire = SettingsFileLock::acquire(guard.settings_path(), Duration::ZERO);
            assert!(reacquire.is_err(), "the guard holds the transactional lock");
            Ok(())
        })
        .unwrap();
    assert_eq!(report.outcome, OperationOutcome::Committed);
    assert_eq!(counts(&owner), [2, 2, 8]);

    // A corrupt present agents file is not absence: Recovery refuses it and
    // leaves its bytes untouched; Startup keeps it as metadata.
    let agents = agents_instance_path(&path);
    std::fs::write(&agents, b"{not json").unwrap();
    let error = owner
        .with_operation_blocking("corrupt-agents", |_, _| Ok(()))
        .unwrap_err();
    assert!(is_compose(&error), "{error:?}");
    assert_eq!(std::fs::read(&agents).unwrap(), b"{not json");
    assert!(voice_enabled(&owner));
    owner.drain_blocking(SOFT_DEADLINE);
    let startup = SettingsOwner::open_active(&path, WAIT).unwrap();
    let snapshot = startup.blocking_current_snapshot().unwrap();
    assert!(matches!(
        snapshot.agents_layer,
        AgentsLayerState::Unreadable { .. }
    ));
    startup.drain_blocking(SOFT_DEADLINE);
    std::fs::remove_file(agents).unwrap();

    // A malformed overlay is a Startup warning and a Recovery failure.
    std::fs::write(overlay_path(&path), b"[1, 2").unwrap();
    let startup = SettingsOwner::open_active(&path, WAIT).unwrap();
    let snapshot = startup.blocking_current_snapshot().unwrap();
    assert!(snapshot.local_overlay_state.rejection().is_some());
    let error = startup
        .with_operation_blocking("malformed-overlay", |_, _| Ok(()))
        .unwrap_err();
    assert!(is_compose(&error), "{error:?}");
    startup.drain_blocking(SOFT_DEADLINE);
    std::fs::remove_file(overlay_path(&path)).unwrap();

    // A corrupt base never becomes defaults.
    let corrupt: &[u8] = b"{\"voiceToTextEnabled\": tru";
    std::fs::write(&path, corrupt).unwrap();
    let error = SettingsOwner::open_active(&path, WAIT).unwrap_err();
    assert!(is_compose(&error), "{error:?}");
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);

    // A source replaced by a non-regular object is a capture error.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let error = SettingsOwner::open_active(&path, WAIT).unwrap_err();
    assert!(matches!(error, SettingsOwnerError::Capture(_)), "{error:?}");
}

#[test]
fn e2992_p41_publisher_recovery() {
    let publishers = [
        SettingsPublisher::PreflightBackup,
        SettingsPublisher::ProjectPathsMigration,
        SettingsPublisher::ProjectPathsBackup,
        SettingsPublisher::ProjectPathsCleanup,
        SettingsPublisher::BlockingMenusExport,
        SettingsPublisher::AgentsExport,
        SettingsPublisher::AgentsSave,
        SettingsPublisher::SettingsSave,
        SettingsPublisher::AuxCache,
        SettingsPublisher::Stamp,
    ];
    for publisher in publishers {
        let (_dir, path) = instance(Some(voice(true)));
        let owner = publisher_partial_cases(&path, publisher);
        publisher_invalid_recapture(&owner, &path, publisher);
    }
    panic_classification_cases();
}

/// Rejected before any publisher, then PartialOrUnknown after `publisher`
/// wrote and the principal failed or panicked.
fn publisher_partial_cases(path: &Path, publisher: SettingsPublisher) -> Arc<SettingsOwner> {
    let owner = SettingsOwner::open_active(path, WAIT).unwrap();
    let epoch = owner.identity().epoch;

    // Failure before any publisher: Rejected, RAM and epoch unchanged.
    let report = owner
        .with_operation_blocking("pre", |_, _| -> Result<(), String> {
            Err("validation".to_string())
        })
        .unwrap();
    assert_eq!(report.outcome, OperationOutcome::Rejected);
    assert_eq!(owner.identity().epoch, epoch);

    // A publisher wrote, then the principal TempWrite failed. The owner
    // keeps its authority through recovery and adopts reality as
    // PartialOrUnknown, never as success or rollback.
    let alias = path.to_path_buf();
    let report = owner
        .with_operation_blocking("partial", move |_, guard| -> Result<(), String> {
            guard.publish(publisher, |settings_path| {
                write_json(settings_path, &voice(false));
                let second = SettingsOwner::open_active(&alias, Duration::ZERO);
                let held = matches!(second, Err(SettingsOwnerError::LifecycleTimedOut));
                assert!(held, "authority is retained during the operation");
                Ok(())
            })?;
            assert_eq!(guard.effect(), OperationEffect::Possible);
            Err("TempWrite".to_string())
        })
        .unwrap();
    assert_eq!(
        report.outcome,
        OperationOutcome::PartialOrUnknown,
        "{publisher:?}"
    );
    assert_eq!(report.availability, SettingsAvailability::Ready);
    assert!(report.error.is_some());
    assert!(!voice_enabled(&owner));

    // A panic after a publisher is classified, never abandoned.
    let report = owner
        .with_operation_blocking("panic", move |_, guard| -> Result<(), String> {
            guard.publish(publisher, |settings_path| {
                write_json(settings_path, &voice(true));
                Ok(())
            })?;
            panic!("injected");
        })
        .unwrap();
    assert_eq!(report.outcome, OperationOutcome::PartialOrUnknown);
    assert!(voice_enabled(&owner));
    owner
}

/// An invalid recapture after `publisher` is sticky Invalid until close.
fn publisher_invalid_recapture(owner: &SettingsOwner, path: &Path, publisher: SettingsPublisher) {
    // An invalid recapture is Invalid: no current read, no default save.
    let report = owner
        .with_operation_blocking("corrupt", move |_, guard| -> Result<(), String> {
            guard.publish(publisher, |settings_path| write_raw(settings_path, b"{"))?;
            Err("ReDecode".to_string())
        })
        .unwrap();
    assert_eq!(report.outcome, OperationOutcome::PartialOrUnknown);
    assert_eq!(report.availability, SettingsAvailability::Invalid);
    assert_eq!(
        owner.blocking_current_snapshot().unwrap_err(),
        SettingsOwnerError::Unavailable(SettingsAvailability::Invalid)
    );
    assert_eq!(std::fs::read(path).unwrap(), b"{");
    let denied = owner.with_operation_blocking("after", |_, _| Ok(()));
    assert!(matches!(
        denied,
        Err(SettingsOwnerError::Unavailable(
            SettingsAvailability::Invalid
        ))
    ));
    // The authority is still held until the owner closes.
    let second = SettingsOwner::open_active(path, Duration::ZERO);
    assert!(matches!(second, Err(SettingsOwnerError::LifecycleTimedOut)));
    owner.drain_blocking(SOFT_DEADLINE);
    assert_eq!(owner.availability(), SettingsAvailability::Closed);
}

/// The async form, then panics in the admission capture, in the recapture
/// and before the guards.
fn panic_classification_cases() {
    // The async form runs the same body on a blocking worker.
    let (_dir, path) = instance(Some(voice(false)));
    let owner = SettingsOwner::open_active(&path, WAIT).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let operation = owner.with_operation("async", |_, guard| {
        guard.publish(SettingsPublisher::SettingsSave, |settings_path| {
            write_json(settings_path, &voice(true));
            Ok(())
        })
    });
    let report = runtime.block_on(operation).unwrap();
    assert_eq!(report.outcome, OperationOutcome::Committed);
    let snapshot = runtime.block_on(owner.current_snapshot()).unwrap();
    assert!(snapshot.voice_to_text_enabled);

    // A panic in the admission capture, outside the builder and before any
    // publisher, is classified under the guards: Rejected, RAM kept, Ready.
    OWNER_CAPTURE_PANIC.with(|armed| armed.set(true));
    let report = owner
        .with_operation_blocking("capture-panic", |_, _| Ok(()))
        .unwrap();
    assert_eq!(report.outcome, OperationOutcome::Rejected);
    assert_eq!(report.availability, SettingsAvailability::Ready);
    assert!(report.error.is_some());
    assert!(voice_enabled(&owner));

    // A panic in the recapture after a publisher cannot adopt reality:
    // PartialOrUnknown and sticky Invalid, never success.
    let report = owner
        .with_operation_blocking("recapture-panic", |_, guard| {
            guard.publish(SettingsPublisher::SettingsSave, |settings_path| {
                write_json(settings_path, &voice(false));
                Ok(())
            })?;
            OWNER_CAPTURE_PANIC.with(|armed| armed.set(true));
            Ok(())
        })
        .unwrap();
    assert_eq!(report.outcome, OperationOutcome::PartialOrUnknown);
    assert_eq!(report.availability, SettingsAvailability::Invalid);
    assert!(report.value.is_none());
    assert!(owner.blocking_current_snapshot().is_err());
    // Every admission was released, so the drain terminates.
    owner.drain_blocking(SOFT_DEADLINE);
    assert_eq!(owner.availability(), SettingsAvailability::Closed);

    // A panic before the guards (a blocking state guard requested from
    // inside a runtime) published nothing: a worker error, Ready kept, and
    // the drain still terminates.
    let (_dir, path) = instance(Some(voice(true)));
    let owner = SettingsOwner::open_active(&path, WAIT).unwrap();
    let in_runtime =
        runtime.block_on(async { owner.with_operation_blocking("in-runtime", |_, _| Ok(())) });
    assert!(
        matches!(in_runtime, Err(SettingsOwnerError::Worker(_))),
        "{in_runtime:?}"
    );
    assert_eq!(owner.availability(), SettingsAvailability::Ready);
    assert!(voice_enabled(&owner));
    owner.drain_blocking(SOFT_DEADLINE);
}

#[test]
fn e2992_p41_memory_zero_io() {
    let (dir, path) = instance(Some(voice(true)));
    let owner = SettingsOwner::open_active(&path, WAIT).unwrap();
    // The owner's transactions lock the physical spelling (macOS temp dirs
    // sit under the `/var` link, which the transactional lock rejects).
    let physical = std::fs::canonicalize(dir.path())
        .unwrap()
        .join(SETTINGS_FILE_NAME);
    let held = SettingsFileLock::acquire(&physical, Duration::from_secs(2)).unwrap();
    let before = counts(&owner);
    for _ in 0..100 {
        assert!(voice_enabled(&owner));
        let retention = owner.blocking_current_retention().unwrap();
        assert_eq!(retention.source_error, None);
        let permit = owner.try_current_read_owned().unwrap();
        assert!(permit.voice_to_text_enabled);
        assert!(Arc::ptr_eq(permit.owner(), &owner));
        drop(permit);
        assert!(owner.blocking_current_read().is_ok());
        assert!(owner.try_current_read().is_ok());
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for _ in 0..100 {
            assert!(owner.current_snapshot().await.is_ok());
            assert!(owner.current_retention().await.is_ok());
        }
    });
    assert_eq!(
        counts(&owner),
        before,
        "memory reads touch no source or lock"
    );

    // Positive control: a mutation does attempt the transactional lock.
    let error = owner
        .with_operation_blocking("apply", |_, _| Ok(()))
        .unwrap_err();
    assert!(
        matches!(error, SettingsOwnerError::TransactionLock(_)),
        "{error:?}"
    );
    assert_eq!(counts(&owner), [before[0] + 1, before[1], before[2]]);
    drop(held);

    // Invalid denies every read without touching the filesystem.
    let report = owner
        .with_operation_blocking("corrupt", |_, guard| {
            guard.publish(SettingsPublisher::SettingsSave, |settings_path| {
                write_raw(settings_path, b"[")
            })
        })
        .unwrap();
    assert_eq!(report.availability, SettingsAvailability::Invalid);
    let before = counts(&owner);
    assert!(owner.blocking_current_snapshot().is_err());
    assert!(owner.blocking_current_retention().is_err());
    assert!(owner.try_current_read_owned().is_err());
    assert!(owner.try_current_read().is_err());
    assert_eq!(counts(&owner), before);
    owner.drain_blocking(SOFT_DEADLINE);

    // StagedCompatibility: the P40 constructor keeps the forwarding surface
    // and refuses managed operations before any lock attempt.
    let staged = new_settings_state(AppSettings::default());
    assert!(!staged.identity().managed_operations_enabled);
    assert_eq!(staged.availability(), SettingsAvailability::Ready);
    staged.blocking_write().voice_to_text_enabled = true;
    assert!(staged.blocking_read().voice_to_text_enabled);
    assert!(voice_enabled(&staged));
    let refused = staged.with_operation_blocking("staged", |_, _| Ok(()));
    assert!(matches!(
        refused,
        Err(SettingsOwnerError::OperationsNotEnabled)
    ));
    assert_eq!(counts(&staged), [0, 0, 0]);
}

#[test]
fn e2992_p41_lifecycle_alias() {
    let (dir, path) = instance(Some(voice(true)));
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let first = SettingsOwner::open_active(&path, WAIT).unwrap();
    let first_id = first.identity().instance_id;
    let sidecar = dir.path().join(SETTINGS_OWNER_LOCK_FILE_NAME);
    assert!(sidecar.is_file());
    let base = std::fs::read(&path).unwrap();
    let before = counts(&first);

    // Physical aliases: different spellings that name the same directory
    // object. Each one meets the same authority, times out, and reads no
    // source.
    #[allow(unused_mut)]
    let mut aliases = physical_aliases(dir.path());
    if cfg!(windows) {
        // NTFS is case-insensitive and canonicalize yields a verbatim
        // spelling, so Windows always has both aliases.
        assert_eq!(aliases.len(), 2, "{aliases:?}");
    }
    // Linux temp directories are usually case-sensitive and link-free, so
    // the test owns one directory symlink to the instance directory. It
    // lives in a separate test-owned tempdir (no cycle inside the target)
    // and is the waiter's alias even when an ambient alias also exists.
    #[cfg(target_os = "linux")]
    let (link_dir, link) = {
        let link_dir = tempfile::tempdir().unwrap();
        let link = link_dir.path().join("owner-alias");
        let target = std::fs::canonicalize(dir.path()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::canonicalize(&link).unwrap(), target);
        assert_ne!(link, dir.path());
        aliases.insert(0, link.clone());
        (link_dir, link)
    };
    assert!(!aliases.is_empty(), "every platform needs a physical alias");
    for alias_dir in &aliases {
        let alias = alias_dir.join(SETTINGS_FILE_NAME);
        let error = SettingsOwner::open_active(&alias, Duration::from_millis(100));
        assert_eq!(error.unwrap_err(), SettingsOwnerError::LifecycleTimedOut);
    }
    let alias = aliases[0].join(SETTINGS_FILE_NAME);

    // An identity that cannot be established is an error, never "free".
    let parent_alias = dir.path().join("sub").join("..").join(SETTINGS_FILE_NAME);
    let error = SettingsOwner::open_active(&parent_alias, Duration::ZERO).unwrap_err();
    assert!(
        matches!(error, SettingsOwnerError::Lifecycle(_)),
        "{error:?}"
    );
    let missing = dir.path().join("missing").join(SETTINGS_FILE_NAME);
    let error = SettingsOwner::open_active(&missing, Duration::ZERO).unwrap_err();
    assert!(
        matches!(error, SettingsOwnerError::Lifecycle(_)),
        "{error:?}"
    );

    // Only the holder touched the sources.
    assert_eq!(counts(&first), before);
    assert_eq!(std::fs::read(&path).unwrap(), base);

    // A second handle waits, then acquires once the holder releases.
    let waiter = std::thread::spawn(move || SettingsOwner::open_active(&alias, WAIT));
    std::thread::sleep(Duration::from_millis(200));
    assert!(!waiter.is_finished());
    first.drain_blocking(SOFT_DEADLINE);
    let second = waiter.join().unwrap().unwrap();
    assert_eq!(second.availability(), SettingsAvailability::Ready);
    assert_ne!(second.identity().instance_id, first_id);
    assert!(voice_enabled(&second));
    second.drain_blocking(SOFT_DEADLINE);
    // The stable sidecar is never unlinked.
    assert!(sidecar.is_file());

    // Remove only the link, never its target, then close its tempdir.
    #[cfg(target_os = "linux")]
    {
        std::fs::remove_file(&link).unwrap();
        let gone = std::fs::symlink_metadata(&link).unwrap_err();
        assert_eq!(gone.kind(), std::io::ErrorKind::NotFound);
        assert!(dir.path().is_dir());
        assert_eq!(std::fs::read(&path).unwrap(), base);
        assert!(sidecar.is_file());
        link_dir.close().unwrap();
    }
}
