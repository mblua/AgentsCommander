use super::*;

/// T1 - every patch field that resets the schedule is exactly a field the
/// delivery comparison sees. The exhaustive destructure makes a new patch
/// field a compile error here.
#[test]
fn t1_patch_reset_fields_match_delivery_comparison() {
    let base = sample_config();
    let LoopUpdatePatch {
        name,
        expr,
        workgroup,
        prompt_body,
        busy_coordinator,
        session_start,
        enabled,
    } = LoopUpdatePatch {
        name: Some("Other name".to_string()),
        expr: Some("30 9 * * 1-5".to_string()),
        workgroup: Some("wg-2-dev-team".to_string()),
        prompt_body: Some("Other prompt".to_string()),
        busy_coordinator: Some(BusyCoordinatorPolicy::ForceInject),
        session_start: Some(LoopSessionStart::Accumulate),
        enabled: Some(false),
    };
    let patches = [
        (
            "name",
            LoopUpdatePatch {
                name,
                ..LoopUpdatePatch::default()
            },
        ),
        (
            "expr",
            LoopUpdatePatch {
                expr,
                ..LoopUpdatePatch::default()
            },
        ),
        (
            "workgroup",
            LoopUpdatePatch {
                workgroup,
                ..LoopUpdatePatch::default()
            },
        ),
        (
            "prompt_body",
            LoopUpdatePatch {
                prompt_body,
                ..LoopUpdatePatch::default()
            },
        ),
        (
            "busy_coordinator",
            LoopUpdatePatch {
                busy_coordinator,
                ..LoopUpdatePatch::default()
            },
        ),
        (
            "session_start",
            LoopUpdatePatch {
                session_start,
                ..LoopUpdatePatch::default()
            },
        ),
        (
            "enabled",
            LoopUpdatePatch {
                enabled,
                ..LoopUpdatePatch::default()
            },
        ),
    ];
    for (field, patch) in patches {
        let mut patched = base.clone();
        let reset = apply_loop_update_patch(&mut patched, patch).expect("patch applies");
        let matches = loop_delivery_config_matches(&base, &patched);
        assert_eq!(
            reset, !matches,
            "{field}: reset_schedule must equal !matches"
        );
        if field == "name" {
            assert!(!reset && matches, "name must neither reset nor differ");
        } else {
            assert!(reset, "{field} must reset the schedule");
        }
    }
}

/// T4 - a real holder on state.json: two positive controls, then the
/// retry succeeding and giving up.
#[cfg(windows)]
#[test]
fn t4_replace_retry_under_a_real_holder() {
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let tmp = fixture_project();
    let dir = write_loop_config(&tmp.path().join(".ac"), &sample_config()).expect("config");
    let state_path = dir.join(LOOP_STATE_FILE);
    write_loop_state_atomic(&dir, &LoopState::default()).expect("initial state");

    for (label, share) in [
        ("default", None),
        ("FILE_SHARE_READ", Some(FILE_SHARE_READ)),
    ] {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        if let Some(mode) = share {
            options.share_mode(mode);
        }
        let holder = options.open(&state_path).expect("holder");
        let src = dir.join("control.tmp");
        std::fs::write(&src, "{}").expect("control tmp");
        let err = replace_file(&src, &state_path)
            .expect_err("positive control: a held destination must fail the replace");
        println!(
            "T4 positive control ({label}): {err} raw={:?}",
            err.raw_os_error()
        );
        assert_eq!(err.raw_os_error(), Some(5), "{label} holder");
        drop(holder);
        std::fs::remove_file(&src).expect("remove control tmp");
    }

    let next = LoopState {
        last_checked_at: Some(Utc::now()),
        ..LoopState::default()
    };
    let releaser = hold_open_for(&state_path, Duration::from_millis(40));
    let started = Instant::now();
    write_loop_state_atomic(&dir, &next).expect("retry succeeds after a 40 ms hold");
    println!("T4 success leg: Ok after {:?}", started.elapsed());
    releaser.join().expect("releaser");
    let (read_back, _) = read_loop_state_with_raw(&dir).expect("state parses");
    assert_eq!(read_back.last_checked_at, next.last_checked_at);

    let holder = std::fs::OpenOptions::new()
        .read(true)
        .open(&state_path)
        .expect("long holder");
    let started = Instant::now();
    let err = write_loop_state_atomic(&dir, &LoopState::default()).expect_err("gives up");
    let elapsed = started.elapsed();
    drop(holder);
    println!("T4 give-up leg: {err} after {elapsed:?}");
    assert!(err.starts_with("Failed to finalize Loop state"), "{err}");
    assert!(elapsed >= Duration::from_millis(150), "{elapsed:?}");
    assert!(tmp_files_in(&dir).is_empty(), "{:?}", tmp_files_in(&dir));
}

/// T5 - the transient classifier.
#[test]
fn t5_transient_replace_error_classifier() {
    #[cfg(windows)]
    {
        assert!(is_transient_replace_error(
            &std::io::Error::from_raw_os_error(5)
        ));
        assert!(is_transient_replace_error(
            &std::io::Error::from_raw_os_error(32)
        ));
    }
    #[cfg(not(windows))]
    assert!(is_transient_replace_error(&std::io::Error::from(
        std::io::ErrorKind::PermissionDenied
    )));
    assert!(!is_transient_replace_error(&std::io::Error::from(
        std::io::ErrorKind::NotFound
    )));
}

/// T6 - config writes are atomic and leave no tmp file.
#[test]
fn t6_atomic_config_write_leaves_no_tmp() {
    let tmp = fixture_project();
    let ac_root = tmp.path().join(".ac");
    let mut config = sample_config();
    let dir = write_loop_config(&ac_root, &config).expect("first write");
    assert!(dir.join(LOOP_CONFIG_FILE).is_file());
    assert!(tmp_files_in(&dir).is_empty());
    assert_eq!(
        toml::to_string(&read_loop_config(&dir).expect("read")).expect("toml"),
        toml::to_string(&config).expect("toml")
    );

    config.loop_def.name = "Second write".to_string();
    write_loop_config(&ac_root, &config).expect("second write");
    assert!(tmp_files_in(&dir).is_empty());
    assert_eq!(
        read_loop_config(&dir).expect("read").loop_def.name,
        "Second write"
    );

    #[cfg(windows)]
    {
        let releaser = hold_open_for(
            &dir.join(LOOP_CONFIG_FILE),
            std::time::Duration::from_millis(40),
        );
        config.loop_def.name = "Held write".to_string();
        write_loop_config(&ac_root, &config).expect("retry covers the config path");
        releaser.join().expect("releaser");
        assert_eq!(
            read_loop_config(&dir).expect("read").loop_def.name,
            "Held write"
        );
        assert!(tmp_files_in(&dir).is_empty());
    }
}

/// A failing tmp write removes the tmp file and leaves config.toml intact.
/// The failure is real: the tmp path already exists as a read-only file,
/// so opening it for writing is refused.
#[test]
fn failed_config_tmp_write_leaves_no_tmp() {
    let tmp = fixture_project();
    let config = sample_config();
    let dir = write_loop_config(&tmp.path().join(".ac"), &config).expect("config");
    let config_path = dir.join(LOOP_CONFIG_FILE);
    let before = std::fs::read(&config_path).expect("before");

    let tmp_path = dir.join(format!("{}.blocked.tmp", LOOP_CONFIG_FILE));
    std::fs::write(&tmp_path, "partial").expect("pre-create tmp");
    let mut perms = std::fs::metadata(&tmp_path).expect("meta").permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&tmp_path, perms).expect("read-only");
    assert!(
        std::fs::write(&tmp_path, "probe").is_err(),
        "positive control: the read-only tmp must refuse a write"
    );

    let err =
        write_config_via_tmp(&tmp_path, &config_path, "new = 1").expect_err("tmp write fails");
    assert!(err.starts_with("Failed to write Loop config"), "{err}");
    assert!(!tmp_path.exists(), "tmp file must be removed on failure");
    assert!(tmp_files_in(&dir).is_empty());
    assert_eq!(std::fs::read(&config_path).expect("after"), before);
}

#[test]
fn read_loop_config_if_present_distinguishes_absent_from_malformed() {
    let tmp = fixture_project();
    let dir = tmp.path().join(".ac").join("loop-missing");
    assert!(read_loop_config_if_present(&dir)
        .expect("absent dir")
        .is_none());
    std::fs::create_dir_all(&dir).expect("dir");
    assert!(read_loop_config_if_present(&dir)
        .expect("absent file")
        .is_none());
    std::fs::write(dir.join(LOOP_CONFIG_FILE), "not = [toml").expect("bad");
    let err = read_loop_config_if_present(&dir).expect_err("malformed");
    assert!(err.starts_with("Failed to parse"), "{err}");
}

/// T11 - the compare-and-swap primitive.
#[test]
fn t11_state_compare_and_swap() {
    let tmp = fixture_project();
    let dir = write_loop_config(&tmp.path().join(".ac"), &sample_config()).expect("config");
    let state_path = dir.join(LOOP_STATE_FILE);

    let (state, raw) = read_loop_state_with_raw(&dir).expect("absent");
    assert!(raw.is_none());
    assert_eq!(
        serde_json::to_string(&state).expect("json"),
        serde_json::to_string(&LoopState::default()).expect("json")
    );

    let first = LoopState {
        last_checked_at: Some(Utc::now()),
        ..LoopState::default()
    };
    assert_eq!(
        write_loop_state_if_unchanged(&dir, &first, None).expect("absent/absent"),
        LoopStateWrite::Written
    );
    let (_, raw) = read_loop_state_with_raw(&dir).expect("present");
    let raw = raw.expect("raw present");
    assert_eq!(raw, std::fs::read_to_string(&state_path).expect("bytes"));

    assert_eq!(
        write_loop_state_if_unchanged(&dir, &LoopState::default(), Some(&raw)).expect("match"),
        LoopStateWrite::Written
    );

    let before = std::fs::read(&state_path).expect("before");
    assert_eq!(
        write_loop_state_if_unchanged(&dir, &first, Some(&raw)).expect("changed underneath"),
        LoopStateWrite::Stale
    );
    assert_eq!(
        write_loop_state_if_unchanged(&dir, &first, None).expect("expected absent"),
        LoopStateWrite::Stale
    );
    assert_eq!(std::fs::read(&state_path).expect("after"), before);
    assert!(tmp_files_in(&dir).is_empty());
}
