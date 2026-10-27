use super::*;

use crate::tests::snapshot_temp_root;

/// Deliberately keeps the raw `tempfile` root so it can compare it against the
/// canonical one. Every other fixture must use `snapshot_temp_root`.
#[test]
fn terminal_snapshot_temp_root_confinement_diagnostic() {
    let temporary = tempfile::tempdir().unwrap();
    let raw = temporary.path().to_path_buf();
    let canonical = std::fs::canonicalize(&raw).unwrap();

    let mut report = format!("raw={raw:?}\ncanonical={canonical:?}\n");
    let mut current = std::path::PathBuf::new();
    for component in raw.components() {
        current.push(component.as_os_str());
        if matches!(
            component,
            std::path::Component::Prefix(_) | std::path::Component::RootDir
        ) {
            continue;
        }
        let line = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => format!(
                "{current:?} dir={} symlink={}\n",
                metadata.is_dir(),
                metadata.file_type().is_symlink()
            ),
            Err(error) => format!("{current:?} lstat_error={:?}\n", error.kind()),
        };
        report.push_str(&line);
    }
    report.push_str(&format!(
        "raw_chain={:?}\ncanonical_chain={:?}\nraw_output={:?}\ncanonical_output={:?}",
        verify_snapshot_directory_chain(&raw),
        verify_snapshot_directory_chain(&canonical),
        validate_snapshot_output_path(&raw.join("diagnostic.png")),
        validate_snapshot_output_path(&canonical.join("diagnostic.png")),
    ));
    eprintln!("{report}");
    assert_eq!(
        validate_snapshot_output_path(&canonical.join("diagnostic.png")),
        Ok(()),
        "{report}"
    );
}

#[test]
fn terminal_snapshot_output_is_create_new_and_identity_stable() {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let (_temporary, root) = snapshot_temp_root();
    let output = root.join("snapshot.png");
    let file = create_snapshot_output(&output).unwrap();
    file.write_and_verify(b"not-yet-a-png").unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), b"not-yet-a-png");
    assert!(matches!(
        create_snapshot_output(&output),
        Err(C::UnsafePath)
    ));
    assert_eq!(
        validate_snapshot_output_path(&root.join("snapshot.txt")).unwrap_err(),
        C::UnsafePath
    );
    assert_eq!(
        validate_snapshot_output_path(std::path::Path::new("relative.png")).unwrap_err(),
        C::UnsafePath
    );
}

#[test]
fn terminal_snapshot_output_rejects_same_spelling_parent_replacement() {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let (_temporary, root) = snapshot_temp_root();
    let parent = root.join("parent");
    let retired = root.join("retired");
    std::fs::create_dir(&parent).unwrap();
    let output = parent.join("snapshot.png");
    let parent_for_race = parent.clone();
    let retired_for_race = retired.clone();
    let error = create_snapshot_output_inner(&output, move || {
        std::fs::rename(&parent_for_race, &retired_for_race).unwrap();
        std::fs::create_dir(&parent_for_race).unwrap();
    })
    .err()
    .unwrap();
    assert_eq!(error, C::OutputFailed);
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
fn terminal_snapshot_unix_helper_parent_replacement_before_open_is_confined() {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let (_temporary, root) = snapshot_temp_root();
    let parent = root.join("parent");
    let retired = root.join("retired");
    std::fs::create_dir(&parent).unwrap();
    let output = parent.join("snapshot.png");
    let parent_for_race = parent.clone();
    let retired_for_race = retired.clone();

    let error = create_snapshot_output_with_hooks(
        &output,
        || {},
        move || {
            std::fs::rename(&parent_for_race, &retired_for_race).unwrap();
            std::fs::create_dir(&parent_for_race).unwrap();
        },
    )
    .err()
    .unwrap();

    assert_eq!(error, C::OutputFailed);
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
fn terminal_snapshot_unix_helper_write_stays_with_retained_parent_object() {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let (_temporary, root) = snapshot_temp_root();
    let parent = root.join("parent");
    let retired = root.join("retired");
    std::fs::create_dir(&parent).unwrap();
    let output = parent.join("snapshot.png");
    let file = create_snapshot_output(&output).unwrap();
    let parent_for_race = parent.clone();
    let retired_for_race = retired.clone();
    let replacement_output = output.clone();

    let error = file
        .write_and_verify_inner(
            b"owned bytes",
            move || {
                std::fs::rename(&parent_for_race, &retired_for_race).unwrap();
                std::fs::create_dir(&parent_for_race).unwrap();
                std::fs::write(&replacement_output, b"replacement bytes").unwrap();
            },
            || {},
        )
        .unwrap_err();

    assert_eq!(error, C::OutputFailed);
    assert_eq!(std::fs::read(&output).unwrap(), b"replacement bytes");
    assert_eq!(
        std::fs::read(retired.join("snapshot.png")).unwrap(),
        b"owned bytes"
    );
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_helper_parent_link_churn_preserves_object_proof() {
    use std::os::unix::fs::MetadataExt;

    let (_temporary, root) = snapshot_temp_root();
    let output = root.join("snapshot.png");
    let unrelated = root.join("unrelated");
    let unrelated_after_write = unrelated.clone();
    let unrelated_after_sync = unrelated.clone();
    let parent_after_write = root.clone();
    let file = create_snapshot_output(&output).unwrap();

    file.write_and_verify_inner(
        b"portable bytes",
        move || {
            // Directory link accounting is platform-specific. Linux counts only
            // subdirectories, so a parent returns to its earlier count once a child
            // directory is removed; macOS does not follow that convention. The churn
            // this fixture needs is proven by the count moving, not by any fixed
            // arithmetic, and the removal is proven by the entry being gone below.
            let before_churn = std::fs::metadata(&parent_after_write).unwrap().nlink();
            std::fs::create_dir(&unrelated_after_write).unwrap();
            let during_churn = std::fs::metadata(&parent_after_write).unwrap().nlink();
            assert_ne!(
                during_churn, before_churn,
                "a sibling directory left the parent link count unchanged"
            );
        },
        move || {
            std::fs::remove_dir(&unrelated_after_sync).unwrap();
        },
    )
    .unwrap();

    assert_eq!(std::fs::read(output).unwrap(), b"portable bytes");
    assert!(!unrelated.exists());
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_helper_leaf_substitution_is_detected() {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let (_temporary, root) = snapshot_temp_root();
    let output = root.join("snapshot.png");
    let displaced = root.join("displaced.png");
    let file = create_snapshot_output(&output).unwrap();
    let output_for_race = output.clone();
    let displaced_for_race = displaced.clone();

    let error = file
        .write_and_verify_inner(
            b"owned bytes",
            move || {
                std::fs::rename(&output_for_race, &displaced_for_race).unwrap();
                std::fs::write(&output_for_race, b"replacement bytes").unwrap();
            },
            || {},
        )
        .unwrap_err();

    assert_eq!(error, C::OutputFailed);
    assert_eq!(std::fs::read(output).unwrap(), b"replacement bytes");
    assert_eq!(std::fs::read(displaced).unwrap(), b"owned bytes");
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_helper_rejects_invalid_child_paths_before_open() {
    use std::os::unix::ffi::OsStringExt;
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let (_temporary, root) = snapshot_temp_root();
    let parent = root.join("parent");
    let wrong_parent = root.join("wrong-parent");
    std::fs::create_dir(&parent).unwrap();
    std::fs::create_dir(&wrong_parent).unwrap();
    let outside = wrong_parent.join("snapshot.png");
    std::fs::write(&outside, b"outside bytes").unwrap();
    let proof = snapshot_directory_identity(&parent, C::OutputFailed).unwrap();

    for invalid in [
        parent.clone(),
        outside.clone(),
        parent.join("."),
        parent.join(".."),
        parent.join("nested/snapshot.png"),
    ] {
        assert_eq!(
            snapshot_child_name(&proof, &invalid).err().unwrap().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            open_snapshot_leaf_in_parent(&proof, &invalid)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }

    let nul_leaf = std::ffi::OsString::from_vec(b"nul\0snapshot.png".to_vec());
    let nul_path = parent.join(nul_leaf);
    assert_eq!(
        snapshot_child_name(&proof, &nul_path).err().unwrap().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(std::fs::read(outside).unwrap(), b"outside bytes");
    assert!(!parent.join("snapshot.png").exists());
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_helper_linked_leaves_fail_closed() {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let (_temporary, root) = snapshot_temp_root();
    let parent = root.as_path();
    let proof = snapshot_directory_identity(parent, C::OutputFailed).unwrap();
    let target = parent.join("target");
    let symlink = parent.join("symlink.png");
    std::fs::write(&target, b"target bytes").unwrap();
    std::os::unix::fs::symlink(&target, &symlink).unwrap();

    let symlink_error = open_snapshot_leaf_in_parent(&proof, &symlink).unwrap_err();
    assert_eq!(symlink_error.raw_os_error(), Some(libc::ELOOP));
    assert!(matches!(
        create_snapshot_output(&symlink),
        Err(C::UnsafePath)
    ));

    let hard_leaf = parent.join("hard.png");
    let hard_alias = parent.join("hard-alias.png");
    let hard_leaf_for_hook = hard_leaf.clone();
    let hard_alias_for_hook = hard_alias.clone();
    let hard_error = create_snapshot_output(&hard_leaf)
        .unwrap()
        .write_and_verify_inner(
            b"hard bytes",
            move || {
                std::fs::hard_link(&hard_leaf_for_hook, &hard_alias_for_hook).unwrap();
            },
            || {},
        )
        .unwrap_err();
    assert_eq!(hard_error, C::OutputFailed);
    assert_eq!(std::fs::read(&hard_leaf).unwrap(), b"hard bytes");
    assert_eq!(std::fs::read(&hard_alias).unwrap(), b"hard bytes");
}

/// The creation-level contract for a non-UTF-8 leaf, which is not the same on every
/// Unix. Established by the macOS evidence at `a06a3cf6`: the filesystem refused to
/// create an entry with these bytes, and the refusal came from creation rather than
/// from confinement, because the reason code was `OutputFailed` and not
/// `UnsafePath`. Inferred, unconfirmed, and deliberately asserted nowhere below:
/// that the refusal is APFS enforcing UTF-8 filenames and reporting `EILSEQ`.
///
/// Both arms compile everywhere `cfg(unix)` applies and only the expected outcome
/// differs. Neither arm tolerates the other's outcome, so a platform that changes
/// behaviour fails here instead of passing quietly.
#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_helper_non_utf8_output_leaf_matches_platform_contract() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let (_temporary, root) = snapshot_temp_root();
    let leaf = std::ffi::OsString::from_vec(b"snapshot-\xff.png".to_vec());
    let output = root.join(&leaf);
    let entries_before = std::fs::read_dir(&root).unwrap().count();
    assert_eq!(entries_before, 0, "the temporary root did not start empty");
    let created = create_snapshot_output(&output);

    if cfg!(target_os = "macos") {
        let error = created
            .err()
            .expect("macOS accepted a non-UTF-8 output leaf");
        assert_eq!(error, C::OutputFailed);
        // Stated separately although the equality above already implies it: this is
        // the assertion carrying the claim that confinement accepted the name and
        // the refusal came from creation.
        assert_ne!(error, C::UnsafePath);
        assert_eq!(
            std::fs::symlink_metadata(&output).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            entries_before,
            "a refused creation left an entry behind"
        );
    } else {
        created
            .unwrap()
            .write_and_verify(b"portable bytes")
            .unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"portable bytes");
        let names: Vec<std::ffi::OsString> = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), entries_before + 1);
        assert_eq!(names[0].as_bytes(), leaf.as_bytes());
    }
}

#[cfg(unix)]
#[test]
fn terminal_snapshot_unix_helper_rejects_symlink_parent() {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let (_temporary, root) = snapshot_temp_root();
    let real = root.join("real");
    let linked = root.join("linked");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    let output = linked.join("snapshot.png");

    assert_eq!(
        validate_snapshot_output_path(&output).unwrap_err(),
        C::UnsafePath
    );
    assert!(!real.join("snapshot.png").exists());
}

#[cfg(windows)]
#[test]
fn terminal_snapshot_helper_retains_real_parent_and_leaf_through_output() {
    let (_temporary, root) = snapshot_temp_root();
    let ancestor = root.join("ancestor");
    let parent = ancestor.join("parent");
    let retired_parent = ancestor.join("retired-parent");
    let retired_ancestor = root.join("retired-ancestor");
    let displaced_leaf = parent.join("displaced.png");
    let hard_link_leaf = parent.join("hard-link.png");
    std::fs::create_dir(&ancestor).unwrap();
    std::fs::create_dir(&parent).unwrap();
    let output = parent.join("snapshot.png");
    let parent_before_open = parent.clone();
    let retired_before_open = retired_parent.clone();
    let file = create_snapshot_output_with_hooks(
        &output,
        || {},
        move || {
            assert!(std::fs::rename(&parent_before_open, &retired_before_open).is_err());
        },
    )
    .unwrap();
    assert!(
        file.parent.identity
            == snapshot_handle_identity(&file.parent.file, true)
                .unwrap()
                .object_id
    );
    let output_after_write = output.clone();
    let displaced_after_write = displaced_leaf.clone();
    let output_for_link = output.clone();
    let hard_link_after_write = hard_link_leaf.clone();
    let ancestor_after_sync = ancestor.clone();
    let retired_after_sync = retired_ancestor.clone();
    file.write_and_verify_inner(
        b"owned bytes",
        move || {
            assert!(
                std::fs::rename(&output_after_write, &displaced_after_write).is_err(),
                "helper output leaf was replaceable after write"
            );
            assert!(
                std::fs::hard_link(&output_for_link, &hard_link_after_write).is_err(),
                "helper output leaf allowed a hard-link alias"
            );
        },
        move || {
            assert!(
                std::fs::rename(&ancestor_after_sync, &retired_after_sync).is_err(),
                "helper output ancestor was replaceable after sync"
            );
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), b"owned bytes");
    assert!(!displaced_leaf.exists());
    assert!(!hard_link_leaf.exists());
    assert!(!retired_parent.exists());
    assert!(!retired_ancestor.exists());
}

#[cfg(windows)]
#[test]
fn terminal_snapshot_helper_rejects_windows_aliases_before_filesystem_work() {
    for path in [
        r"\\.\C:\snapshot.png",
        r"\\?\GLOBALROOT\Device\HarddiskVolume1\snapshot.png",
        r"\\?\Volume{00000000-0000-0000-0000-000000000000}\snapshot.png",
        r"\\?\PIPE\snapshot.png",
        r"C:\safe\snapshot.png:stream",
        r"C:\safe\CON .png",
        r"C:\safe\CONOUT$.png",
        r"C:\safe\COM¹.png",
        r"C:\safe\LPT³.txt.png",
        r"C:\safe\trailing.\snapshot.png",
        r"C:\safe\..\escape.png",
        r"C:\safe\.\snapshot.png",
    ] {
        assert!(
            validate_snapshot_output_path_inner(std::path::Path::new(path), || {
                panic!("lexically rejected helper path reached filesystem validation")
            })
            .is_err(),
            "accepted unsafe helper Windows spelling: {path}"
        );
    }
    for path in [
        r"C:\safe\snapshot.png",
        r"\\server\share\snapshot.PNG",
        r"\\?\C:\safe\snapshot.png",
        r"\\?\UNC\server\share\snapshot.png",
    ] {
        assert!(
            validate_windows_snapshot_path(std::path::Path::new(path)).is_ok(),
            "rejected allowed helper Windows spelling: {path}"
        );
    }
}

#[cfg(windows)]
#[test]
fn terminal_snapshot_helper_rejects_junction_and_symlink_parents() {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let (_temporary, root) = snapshot_temp_root();
    let real = root.join("real");
    let junction = root.join("junction");
    std::fs::create_dir(&real).unwrap();
    let status = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&real)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "failed to create the helper junction fixture"
    );
    let junction_output = junction.join("snapshot.png");
    assert_eq!(
        validate_snapshot_output_path(&junction_output).unwrap_err(),
        C::UnsafePath
    );
    assert!(!real.join("snapshot.png").exists());
    std::fs::remove_dir(&junction).unwrap();

    let symlink = root.join("symlink");
    if std::os::windows::fs::symlink_dir(&real, &symlink).is_ok() {
        let symlink_output = symlink.join("snapshot.png");
        assert_eq!(
            validate_snapshot_output_path(&symlink_output).unwrap_err(),
            C::UnsafePath
        );
        assert!(!real.join("snapshot.png").exists());
        std::fs::remove_dir(&symlink).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn terminal_snapshot_helper_supports_long_verbatim_and_distinct_normalized_names() {
    use terminal_snapshot_renderer::TerminalSnapshotReasonCode as C;

    let (_temporary, root) = snapshot_temp_root();
    let case_parent = root.join("CaseParent");
    std::fs::create_dir(&case_parent).unwrap();
    let upper_parent = std::path::PathBuf::from(case_parent.to_string_lossy().to_uppercase());
    assert!(
        snapshot_directory_identity(&case_parent, C::UnsafePath)
            .unwrap()
            .identity
            == snapshot_directory_identity(&upper_parent, C::UnsafePath)
                .unwrap()
                .identity
    );

    let composed = root.join("\u{e9}");
    let decomposed = root.join("e\u{301}");
    std::fs::create_dir(&composed).unwrap();
    std::fs::create_dir(&decomposed).unwrap();
    assert!(
        snapshot_directory_identity(&composed, C::UnsafePath)
            .unwrap()
            .identity
            != snapshot_directory_identity(&decomposed, C::UnsafePath)
                .unwrap()
                .identity
    );

    let mut long_parent = root.clone();
    while long_parent.as_os_str().len() < 300 {
        long_parent.push("terminal-snapshot-helper-long-segment");
        std::fs::create_dir(&long_parent).unwrap();
    }
    let output = long_parent.join("snapshot.png");
    create_snapshot_output(&output)
        .unwrap()
        .write_and_verify(b"long path")
        .unwrap();
    assert_eq!(std::fs::read(output).unwrap(), b"long path");
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
#[test]
fn terminal_snapshot_output_is_stably_unsupported_on_other_unix() {
    assert_eq!(
        unsupported_snapshot_output_error().kind(),
        std::io::ErrorKind::Unsupported
    );
}
