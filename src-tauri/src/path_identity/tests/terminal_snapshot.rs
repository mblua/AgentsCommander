use super::*;

#[test]
fn terminal_snapshot_post_validation_create_race_is_output_failed() {
    let directory = tempfile::TempDir::new().unwrap();
    let output = directory.path().join("raced.png");
    let raced = output.clone();
    let error = create_terminal_snapshot_output_inner(&output, move || {
        std::fs::write(raced, b"collision").unwrap();
    })
    .err()
    .unwrap();
    assert_eq!(error, "output_failed");
    assert_eq!(std::fs::read(output).unwrap(), b"collision");
}

#[cfg(windows)]
#[test]
fn terminal_snapshot_parent_replacement_before_create_leaves_no_output() {
    let directory = tempfile::TempDir::new().unwrap();
    let parent = directory.path().join("parent");
    let retired = directory.path().join("retired");
    std::fs::create_dir(&parent).unwrap();
    let output = parent.join("snapshot.png");
    let parent_for_race = parent.clone();
    let retired_for_race = retired.clone();

    let error = create_terminal_snapshot_output_inner(&output, move || {
        std::fs::rename(&parent_for_race, &retired_for_race).unwrap();
        std::fs::create_dir(&parent_for_race).unwrap();
    })
    .err()
    .unwrap();

    assert_eq!(error, "output_failed");
    assert!(
        !output.exists(),
        "replacement parent received an output leaf"
    );
    assert!(
        !retired.join("snapshot.png").exists(),
        "retained parent received an output leaf after identity loss"
    );
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_parent_replacement_before_open_is_confined() {
    let directory = tempfile::TempDir::new().unwrap();
    let parent = directory.path().join("parent");
    let retired = directory.path().join("retired");
    std::fs::create_dir(&parent).unwrap();
    let output = parent.join("snapshot.png");
    let parent_for_race = parent.clone();
    let retired_for_race = retired.clone();

    let error = create_terminal_snapshot_output_with_hooks(
        &output,
        || {},
        move || {
            std::fs::rename(&parent_for_race, &retired_for_race).unwrap();
            std::fs::create_dir(&parent_for_race).unwrap();
        },
    )
    .err()
    .unwrap();

    assert_eq!(error, "output_failed");
    assert!(
        !output.exists(),
        "replacement parent received an output leaf"
    );
    assert!(
        !retired.join("snapshot.png").exists(),
        "retained parent received a leaf after detected identity loss"
    );
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_write_stays_with_retained_parent_object() {
    let directory = tempfile::TempDir::new().unwrap();
    let parent = directory.path().join("parent");
    let retired = directory.path().join("retired");
    std::fs::create_dir(&parent).unwrap();
    let output = parent.join("snapshot.png");
    let file = create_terminal_snapshot_output(&output).unwrap();
    let parent_for_race = parent.clone();
    let retired_for_race = retired.clone();
    let replacement_output = output.clone();

    let error = file
        .write_all_and_sync_inner(
            b"owned bytes",
            move || {
                std::fs::rename(&parent_for_race, &retired_for_race).unwrap();
                std::fs::create_dir(&parent_for_race).unwrap();
                std::fs::write(&replacement_output, b"replacement bytes").unwrap();
            },
            || {},
        )
        .unwrap_err();

    assert_eq!(error, "output_failed");
    assert_eq!(std::fs::read(&output).unwrap(), b"replacement bytes");
    assert_eq!(
        std::fs::read(retired.join("snapshot.png")).unwrap(),
        b"owned bytes"
    );
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_cleanup_final_barrier_preserves_public_replacement() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("response.json");
    std::fs::write(&path, b"tracked response").unwrap();
    let retained = retain_directory(directory.path()).unwrap();
    let expected = retained.verify_regular_file(&path).unwrap();
    let witness = retained.retain_unix_file_witness(&path, &expected).unwrap();

    let outcome = retained.cleanup_unix_tracked_file_with_hook(
        &path,
        &expected,
        &witness,
        Some(std::ffi::OsStr::new(".public-replacement-claim")),
        |stage, source, _| {
            if stage == UnixTrackedCleanupStage::BeforeClaimUnlink {
                std::fs::write(source, b"replacement response").unwrap();
            }
        },
    );

    assert_eq!(outcome, UnixTrackedCleanupOutcome::Removed);
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement response");
    assert_eq!(witness.state(&expected), UnixFileWitnessState::Unlinked);
    let second = retained.cleanup_unix_tracked_file_with_hook(
        &path,
        &expected,
        &witness,
        Some(std::ffi::OsStr::new(".unused-second-claim")),
        |_, _, _| {},
    );
    assert_eq!(second, UnixTrackedCleanupOutcome::AlreadyAbsent);
    assert_eq!(std::fs::read(path).unwrap(), b"replacement response");
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_cleanup_reverifies_private_claim_at_final_barrier() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("response.json");
    let displaced = directory.path().join("displaced-tracked-response");
    std::fs::write(&path, b"tracked response").unwrap();
    let retained = retain_directory(directory.path()).unwrap();
    let expected = retained.verify_regular_file(&path).unwrap();
    let witness = retained.retain_unix_file_witness(&path, &expected).unwrap();
    let displaced_for_hook = displaced.clone();

    let outcome = retained.cleanup_unix_tracked_file_with_hook(
        &path,
        &expected,
        &witness,
        Some(std::ffi::OsStr::new(".substituted-private-claim")),
        move |stage, _, claim| {
            if stage == UnixTrackedCleanupStage::BeforeClaimUnlink {
                std::fs::rename(claim, &displaced_for_hook).unwrap();
                std::fs::write(claim, b"foreign claim").unwrap();
            }
        },
    );

    assert_eq!(outcome, UnixTrackedCleanupOutcome::Uncertain);
    assert_eq!(
        std::fs::read(directory.path().join(".substituted-private-claim")).unwrap(),
        b"foreign claim"
    );
    assert_eq!(std::fs::read(displaced).unwrap(), b"tracked response");
    assert_eq!(witness.state(&expected), UnixFileWitnessState::Linked);
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_cleanup_claim_collision_retains_source_and_collision() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("response.json");
    let claim = directory.path().join(".collided-private-claim");
    std::fs::write(&path, b"tracked response").unwrap();
    std::fs::write(&claim, b"claim collision").unwrap();
    let retained = retain_directory(directory.path()).unwrap();
    let expected = retained.verify_regular_file(&path).unwrap();
    let witness = retained.retain_unix_file_witness(&path, &expected).unwrap();

    let outcome = retained.cleanup_unix_tracked_file_with_hook(
        &path,
        &expected,
        &witness,
        claim.file_name(),
        |_, _, _| {},
    );

    assert_eq!(outcome, UnixTrackedCleanupOutcome::SourceRetained);
    assert_eq!(std::fs::read(path).unwrap(), b"tracked response");
    assert_eq!(std::fs::read(claim).unwrap(), b"claim collision");
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_cleanup_restores_source_substitution_without_deleting_it() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("response.json");
    let displaced = directory.path().join("displaced-tracked-response");
    std::fs::write(&path, b"tracked response").unwrap();
    let retained = retain_directory(directory.path()).unwrap();
    let expected = retained.verify_regular_file(&path).unwrap();
    let witness = retained.retain_unix_file_witness(&path, &expected).unwrap();
    let path_for_hook = path.clone();
    let displaced_for_hook = displaced.clone();

    let outcome = retained.cleanup_unix_tracked_file_with_hook(
        &path,
        &expected,
        &witness,
        Some(std::ffi::OsStr::new(".captured-substitution-claim")),
        move |stage, _, _| {
            if stage == UnixTrackedCleanupStage::BeforeClaimRename {
                std::fs::rename(&path_for_hook, &displaced_for_hook).unwrap();
                std::fs::write(&path_for_hook, b"replacement response").unwrap();
            }
        },
    );

    assert_eq!(outcome, UnixTrackedCleanupOutcome::Uncertain);
    assert_eq!(std::fs::read(path).unwrap(), b"replacement response");
    assert_eq!(std::fs::read(displaced).unwrap(), b"tracked response");
    assert!(!directory
        .path()
        .join(".captured-substitution-claim")
        .exists());
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_cleanup_restore_collision_preserves_every_name() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("response.json");
    let displaced = directory.path().join("displaced-tracked-response");
    let claim = directory.path().join(".restore-collision-claim");
    std::fs::write(&path, b"tracked response").unwrap();
    let retained = retain_directory(directory.path()).unwrap();
    let expected = retained.verify_regular_file(&path).unwrap();
    let witness = retained.retain_unix_file_witness(&path, &expected).unwrap();
    let path_for_hook = path.clone();
    let displaced_for_hook = displaced.clone();

    let outcome = retained.cleanup_unix_tracked_file_with_hook(
        &path,
        &expected,
        &witness,
        claim.file_name(),
        move |stage, source, _| match stage {
            UnixTrackedCleanupStage::BeforeClaimRename => {
                std::fs::rename(&path_for_hook, &displaced_for_hook).unwrap();
                std::fs::write(&path_for_hook, b"captured replacement").unwrap();
            }
            UnixTrackedCleanupStage::BeforeRestore => {
                std::fs::write(source, b"restoration collision").unwrap();
            }
            UnixTrackedCleanupStage::BeforeClaimUnlink => {}
        },
    );

    assert_eq!(outcome, UnixTrackedCleanupOutcome::Uncertain);
    assert_eq!(std::fs::read(path).unwrap(), b"restoration collision");
    assert_eq!(std::fs::read(claim).unwrap(), b"captured replacement");
    assert_eq!(std::fs::read(displaced).unwrap(), b"tracked response");
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_cleanup_uses_retained_parent_object() {
    let directory = tempfile::TempDir::new().unwrap();
    let parent = directory.path().join("parent");
    let retired = directory.path().join("retired");
    std::fs::create_dir(&parent).unwrap();
    let output = parent.join("response.json");
    std::fs::write(&output, b"owned response").unwrap();
    let retained = retain_directory(&parent).unwrap();
    let expected = retained.verify_regular_file(&output).unwrap();

    std::fs::rename(&parent, &retired).unwrap();
    std::fs::create_dir(&parent).unwrap();
    std::fs::write(&output, b"replacement response").unwrap();

    assert!(retained.remove_regular_file_if_same(&output, &expected));
    assert!(!retired.join("response.json").exists());
    assert_eq!(std::fs::read(output).unwrap(), b"replacement response");
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_child_api_rejects_invalid_paths_and_linked_leaves() {
    use std::os::unix::ffi::OsStringExt;

    let directory = tempfile::TempDir::new().unwrap();
    let parent = directory.path().join("parent");
    let wrong_parent = directory.path().join("wrong-parent");
    std::fs::create_dir(&parent).unwrap();
    std::fs::create_dir(&wrong_parent).unwrap();
    let retained = retain_directory(&parent).unwrap();
    let outside = wrong_parent.join("response.json");
    std::fs::write(&outside, b"outside bytes").unwrap();

    for invalid in [
        parent.clone(),
        outside.clone(),
        parent.join("."),
        parent.join(".."),
        parent.join("nested/response.json"),
    ] {
        assert!(retained.checked_unix_child_name(&invalid).is_err());
        assert!(retained.create_new_private_file(&invalid).is_err());
        assert!(!retained.child_is_absent(&invalid));
    }

    let nul_leaf = std::ffi::OsString::from_vec(b"nul\0response.json".to_vec());
    let nul_path = parent.join(nul_leaf);
    assert!(retained.checked_unix_child_name(&nul_path).is_err());
    assert!(!retained.child_is_absent(&nul_path));
    assert_eq!(std::fs::read(outside).unwrap(), b"outside bytes");

    let target = parent.join("target");
    let symlink = parent.join("symlink.json");
    std::fs::write(&target, b"target bytes").unwrap();
    std::os::unix::fs::symlink(&target, &symlink).unwrap();
    assert!(retained.verify_regular_file(&symlink).is_err());
    assert!(!retained.child_is_absent(&symlink));

    let hard_source = parent.join("hard-source");
    let hard_leaf = parent.join("hard.json");
    std::fs::write(&hard_source, b"hard bytes").unwrap();
    std::fs::hard_link(&hard_source, &hard_leaf).unwrap();
    assert!(retained.verify_regular_file(&hard_leaf).is_err());
    assert!(!retained.child_is_absent(&hard_leaf));
    assert!(retained.child_is_absent(&parent.join("missing.json")));
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_absence_classification_uses_originating_error() {
    assert!(unix_child_open_error_is_absent(
        &std::io::Error::from_raw_os_error(libc::ENOENT)
    ));
    for code in [libc::ELOOP, libc::EACCES, libc::EIO, libc::EINVAL] {
        assert!(!unix_child_open_error_is_absent(
            &std::io::Error::from_raw_os_error(code)
        ));
    }
    assert!(!unix_child_open_error_is_absent(&std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "synthetic not-found without errno",
    )));
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_non_utf8_leaf_remains_supported() {
    use std::os::unix::ffi::OsStringExt;

    let directory = tempfile::TempDir::new().unwrap();
    let leaf = std::ffi::OsString::from_vec(b"snapshot-\xff.png".to_vec());
    let output = directory.path().join(leaf);
    create_terminal_snapshot_output(&output)
        .unwrap()
        .write_all_and_sync(b"portable bytes")
        .unwrap();
    assert_eq!(std::fs::read(output).unwrap(), b"portable bytes");
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_symlink_parent_is_rejected() {
    let directory = tempfile::TempDir::new().unwrap();
    let real = directory.path().join("real");
    let linked = directory.path().join("linked");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    let output = linked.join("snapshot.png");

    assert_eq!(
        validate_terminal_snapshot_output_path(&output).unwrap_err(),
        "unsafe_path"
    );
    assert!(!real.join("snapshot.png").exists());
}

#[cfg(windows)]
#[test]
fn terminal_snapshot_windows_namespaces_reject_lexically_without_filesystem_work() {
    for path in [
        r"\\.\C:\snapshot.png",
        r"\\?\GLOBALROOT\Device\HarddiskVolume1\snapshot.png",
        r"\\?\Volume{00000000-0000-0000-0000-000000000000}\snapshot.png",
        r"\\?\PIPE\snapshot.png",
        r"\??\GLOBALROOT\Device\HarddiskVolume1\snapshot.png",
        r"C:\safe\snapshot.png:stream",
        r"C:\safe\CON.png",
        r"C:\safe\CON .png",
        r"C:\safe\CONIN$.png",
        r"C:\safe\COM¹.png",
        r"C:\safe\LPT³.txt.png",
        r"C:\safe\trailing.\snapshot.png",
        "C:\\safe\\control\u{1f}.png",
        r"C:\safe\..\escape.png",
        r"C:\safe\.\snapshot.png",
    ] {
        assert!(
            validate_terminal_snapshot_output_path_inner(Path::new(path), || {
                panic!("lexically rejected namespace reached filesystem validation")
            })
            .is_err(),
            "accepted unsafe Windows spelling: {path}"
        );
    }
    for path in [
        r"C:\safe\snapshot.png",
        r"\\server\share\snapshot.PNG",
        r"\\?\C:\safe\snapshot.png",
        r"\\?\UNC\server\share\snapshot.png",
    ] {
        assert!(
            validate_windows_snapshot_output_path(Path::new(path)).is_ok(),
            "rejected allowed Windows spelling: {path}"
        );
    }
}

#[cfg(windows)]
#[test]
fn retained_directory_has_stable_volume_file_id_and_blocks_replacement() {
    let directory = tempfile::TempDir::new().unwrap();
    let ancestor = directory.path().join("ancestor");
    let parent = ancestor.join("parent");
    let retired_parent = ancestor.join("retired-parent");
    let retired_ancestor = directory.path().join("retired-ancestor");
    std::fs::create_dir(&ancestor).unwrap();
    std::fs::create_dir(&parent).unwrap();
    let retained = retain_directory(&parent).unwrap();
    let identity = retained.identity().object_id;
    let current = verify_directory(&parent).unwrap();
    assert_eq!(identity.volume, current.object_id.volume);
    assert_eq!(identity.file, current.object_id.file);
    assert!(std::fs::rename(&parent, &retired_parent).is_err());
    assert!(std::fs::rename(&ancestor, &retired_ancestor).is_err());
    let retained_clone = retained.clone();
    drop(retained);
    assert!(std::fs::rename(&parent, &retired_parent).is_err());
    retained_clone.verify_current().unwrap();
    drop(retained_clone);
    std::fs::rename(&ancestor, &retired_ancestor).unwrap();
}

#[cfg(windows)]
#[test]
fn terminal_snapshot_output_rejects_junction_and_symlink_parents() {
    let directory = tempfile::TempDir::new().unwrap();
    let real = directory.path().join("real");
    let junction = directory.path().join("junction");
    std::fs::create_dir(&real).unwrap();
    let status = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&real)
        .status()
        .unwrap();
    assert!(status.success(), "failed to create the junction fixture");
    let junction_output = junction.join("snapshot.png");
    assert_eq!(
        validate_terminal_snapshot_output_path(&junction_output).unwrap_err(),
        "unsafe_path"
    );
    assert!(!real.join("snapshot.png").exists());
    std::fs::remove_dir(&junction).unwrap();

    let symlink = directory.path().join("symlink");
    if std::os::windows::fs::symlink_dir(&real, &symlink).is_ok() {
        let symlink_output = symlink.join("snapshot.png");
        assert_eq!(
            validate_terminal_snapshot_output_path(&symlink_output).unwrap_err(),
            "unsafe_path"
        );
        assert!(!real.join("snapshot.png").exists());
        std::fs::remove_dir(&symlink).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn retained_output_parent_and_leaf_block_create_write_sync_swaps() {
    let directory = tempfile::TempDir::new().unwrap();
    let parent = directory.path().join("parent");
    let retired_parent = directory.path().join("retired-parent");
    let displaced_leaf = parent.join("displaced.png");
    let hard_link_leaf = parent.join("hard-link.png");
    std::fs::create_dir(&parent).unwrap();
    let output = parent.join("snapshot.png");
    let parent_before_open = parent.clone();
    let retired_before_open = retired_parent.clone();
    let file = create_terminal_snapshot_output_with_hooks(
        &output,
        || {},
        move || {
            assert!(std::fs::rename(&parent_before_open, &retired_before_open).is_err());
        },
    )
    .unwrap();
    let output_after_write = output.clone();
    let displaced_after_write = displaced_leaf.clone();
    let output_for_link = output.clone();
    let hard_link_after_write = hard_link_leaf.clone();
    let parent_after_sync = parent.clone();
    let retired_after_sync = retired_parent.clone();
    file.write_all_and_sync_inner(
        b"owned bytes",
        move || {
            assert!(
                std::fs::rename(&output_after_write, &displaced_after_write).is_err(),
                "opened output leaf was replaceable after write"
            );
            assert!(
                std::fs::hard_link(&output_for_link, &hard_link_after_write).is_err(),
                "opened output leaf allowed a hard-link alias"
            );
        },
        move || {
            assert!(
                std::fs::rename(&parent_after_sync, &retired_after_sync).is_err(),
                "retained output parent was replaceable after sync"
            );
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), b"owned bytes");
    assert!(!displaced_leaf.exists());
    assert!(!hard_link_leaf.exists());
    assert!(!retired_parent.exists());
}

#[cfg(windows)]
#[test]
fn exact_handle_delete_cannot_remove_a_leaf_swap() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("owned-response.json");
    let displaced = directory.path().join("displaced-response.json");
    std::fs::write(&path, b"owned response").unwrap();
    let expected = verify_regular_file(&path).unwrap();
    let path_for_hook = path.clone();
    let displaced_for_hook = displaced.clone();
    assert!(remove_windows_regular_file_by_handle_inner(
        &path,
        &expected,
        move || {
            assert!(
                std::fs::rename(&path_for_hook, &displaced_for_hook).is_err(),
                "verified delete handle allowed a final leaf swap"
            );
        },
    ));
    assert!(!path.exists());
    assert!(!displaced.exists());
}

#[cfg(windows)]
#[test]
fn terminal_snapshot_output_supports_case_unicode_identity_and_long_verbatim_paths() {
    let directory = tempfile::TempDir::new().unwrap();
    let case_parent = directory.path().join("CaseParent");
    std::fs::create_dir(&case_parent).unwrap();
    let upper_parent = PathBuf::from(case_parent.to_string_lossy().to_uppercase());
    assert_eq!(
        retain_directory(&case_parent).unwrap().identity().object_id,
        retain_directory(&upper_parent)
            .unwrap()
            .identity()
            .object_id
    );

    let composed = directory.path().join("\u{e9}");
    let decomposed = directory.path().join("e\u{301}");
    std::fs::create_dir(&composed).unwrap();
    std::fs::create_dir(&decomposed).unwrap();
    assert_ne!(
        verify_directory(&composed).unwrap().object_id,
        verify_directory(&decomposed).unwrap().object_id
    );

    let mut long_parent = std::fs::canonicalize(directory.path()).unwrap();
    while long_parent.as_os_str().len() < 300 {
        long_parent.push("terminal-snapshot-long-segment");
        std::fs::create_dir(&long_parent).unwrap();
    }
    let output = long_parent.join("snapshot.png");
    create_terminal_snapshot_output(&output)
        .unwrap()
        .write_all_and_sync(b"long path")
        .unwrap();
    assert_eq!(std::fs::read(output).unwrap(), b"long path");
}
