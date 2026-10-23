use super::*;

#[test]
fn issue_3068_readonly_source_bytes_only() {
    let (_tmp, project, source, destination) = issue_3068_fixture();
    let file = source.join("Role.md");
    struct RestorePermissions(PathBuf, std::fs::Permissions);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            std::fs::set_permissions(&self.0, self.1.clone()).unwrap();
        }
    }
    let original = std::fs::metadata(&file).unwrap().permissions();
    let _restore = RestorePermissions(file.clone(), original.clone());
    let mut readonly = original;
    readonly.set_readonly(true);
    std::fs::set_permissions(&file, readonly).unwrap();
    let before = issue_3068_inventory(&source);
    let owned = copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_copy").unwrap();
    assert!(std::fs::metadata(&file).unwrap().permissions().readonly());
    assert!(!std::fs::metadata(owned.root.join("Role.md"))
        .unwrap()
        .permissions()
        .readonly());
    assert_eq!(before, issue_3068_inventory(&source));
    cleanup_owned_copy_artifacts(&owned).unwrap();
    assert!(!owned.root.exists());
    assert!(std::fs::metadata(&file).unwrap().permissions().readonly());
}

#[test]
fn issue_3068_collision_and_invalid_reference_never_clobber() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let collision = destination.join("_agent_copy");
    std::fs::create_dir(&collision).unwrap();
    std::fs::write(collision.join("sentinel"), b"external").unwrap();
    assert!(copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_copy").is_err());
    assert_eq!(
        std::fs::read(collision.join("sentinel")).unwrap(),
        b"external"
    );
    for invalid in ["../alpha", "/alpha", "a/b", r"a\b", "C:alpha"] {
        assert!(copy_agent_matrix_strict(&project, invalid, &destination, "_agent_new").is_err());
        assert!(copy_agent_matrix_strict(&project, "alpha", &destination, invalid).is_err());
    }
    assert!(!destination.join("_agent_new").exists());
}

#[test]
fn issue_3068_injected_failures_cleanup_owned_output() {
    for failure_stage in [
        MatrixCopyStage::Reserved,
        MatrixCopyStage::Streaming,
        MatrixCopyStage::ConfigWritten,
        MatrixCopyStage::Provenance,
        MatrixCopyStage::ProvenanceWritten,
    ] {
        let (_tmp, project, source, destination) = issue_3068_fixture();
        std::fs::write(source.join("skills/large.bin"), vec![77u8; 192 * 1024]).unwrap();
        let mut triggered = false;
        let mut callback = |stage, path: &Path| {
            if stage == failure_stage
                && (stage != MatrixCopyStage::Streaming || path.ends_with("large.bin"))
            {
                if stage == MatrixCopyStage::Streaming {
                    let size = std::fs::metadata(destination.join("_agent_copy/skills/large.bin"))
                        .unwrap()
                        .len();
                    assert!(size > 0 && size < 192 * 1024);
                }
                triggered = true;
                return Err("injected primary failure".into());
            }
            Ok(())
        };
        let error = copy_agent_matrix_strict_with_hooks(
            &project,
            "alpha",
            &destination,
            "_agent_copy",
            &mut MatrixCopyHooks {
                callback: Some(&mut callback),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(triggered);
        assert!(error.contains("injected primary failure"));
        assert!(!destination.join("_agent_copy").exists(), "{error}");
    }
}

#[test]
fn issue_3068_source_changes_are_detected() {
    for mutation in 0..7 {
        let (_tmp, project, _source, destination) = issue_3068_fixture();
        let mut callback = |stage, source: &Path| {
            if stage == MatrixCopyStage::BeforeVerify {
                match mutation {
                    0 => std::fs::write(source.join("skills/added"), b"new").unwrap(),
                    1 => std::fs::remove_file(source.join("Role.md")).unwrap(),
                    2 => std::fs::write(source.join("Role.md"), b"changed bytes").unwrap(),
                    3 => {
                        std::fs::remove_file(source.join("Role.md")).unwrap();
                        std::fs::create_dir(source.join("Role.md")).unwrap();
                    }
                    4 => std::fs::create_dir(source.join("new-default")).unwrap(),
                    5 => std::fs::remove_dir_all(source.join("default-tools")).unwrap(),
                    _ => std::fs::write(source.join("skills/added-other"), b"selected addition")
                        .unwrap(),
                }
            }
            Ok(())
        };
        assert!(copy_agent_matrix_strict_with_hooks(
            &project,
            "alpha",
            &destination,
            "_agent_copy",
            &mut MatrixCopyHooks {
                callback: Some(&mut callback),
                ..Default::default()
            }
        )
        .is_err());
        assert!(!destination.join("_agent_copy").exists());
    }
}

#[test]
fn issue_3068_classifier_rejects_links_reparse_and_specials() {
    for classification in [
        (true, 0, false, true),
        (false, FILE_ATTRIBUTE_REPARSE_POINT, true, false),
        (false, 0, false, false),
    ] {
        let (_tmp, project, _source, destination) = issue_3068_fixture();
        let error = copy_agent_matrix_strict_with_hooks(
            &project,
            "alpha",
            &destination,
            "_agent_copy",
            &mut MatrixCopyHooks {
                classification: Some(classification),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("rejects"), "{error}");
        assert!(!destination.join("_agent_copy").exists());
    }
}

#[cfg(unix)]
#[test]
fn issue_3068_real_special_file_is_rejected() {
    let (_tmp, project, source, destination) = issue_3068_fixture();
    let _socket = std::os::unix::net::UnixListener::bind(source.join("skills/socket")).unwrap();
    let error =
        copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_copy").unwrap_err();
    assert!(error.contains("special"), "{error}");
    assert!(!destination.join("_agent_copy").exists());
}

#[test]
fn issue_3068_cleanup_preserves_external_added_and_modified_files() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let mut callback = |stage, path: &Path| {
        if stage == MatrixCopyStage::Provenance {
            return Err("primary metadata failure".into());
        }
        if stage == MatrixCopyStage::BeforeCleanup {
            std::fs::write(path.join("external"), b"external").unwrap();
            std::fs::write(path.join("Role.md"), b"external modification").unwrap();
        }
        Ok(())
    };
    let error = copy_agent_matrix_strict_with_hooks(
        &project,
        "alpha",
        &destination,
        "_agent_copy",
        &mut MatrixCopyHooks {
            callback: Some(&mut callback),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        error.contains("primary metadata failure") && error.contains("residues"),
        "{error}"
    );
    let root = destination.join("_agent_copy");
    assert_eq!(std::fs::read(root.join("external")).unwrap(), b"external");
    assert_eq!(
        std::fs::read(root.join("Role.md")).unwrap(),
        b"external modification"
    );
    assert!(!root.join("config.json").exists());
}

fn issue_3068_replaced_directory_cleanup() {
    for replace_root in [false, true] {
        let (_tmp, project, _source, destination) = issue_3068_fixture();
        let mut callback = |stage, root: &Path| {
            if stage == MatrixCopyStage::Provenance {
                return Err("primary metadata failure".into());
            }
            if stage == MatrixCopyStage::BeforeCleanup {
                let selected = if replace_root {
                    root.to_path_buf()
                } else {
                    root.join("default-tools")
                };
                let moved = destination.join("moved-owned-directory");
                std::fs::rename(&selected, &moved).unwrap();
                std::fs::create_dir(&selected).unwrap();
                if !replace_root {
                    std::fs::create_dir(selected.join(".hidden")).unwrap();
                    std::fs::write(selected.join(".hidden/data"), b"unknown").unwrap();
                }
                std::fs::write(selected.join("sentinel"), b"external").unwrap();
            }
            Ok(())
        };
        let error = copy_agent_matrix_strict_with_hooks(
            &project,
            "alpha",
            &destination,
            "_agent_copy",
            &mut MatrixCopyHooks {
                callback: Some(&mut callback),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            error.contains("primary metadata failure") && error.contains("residues"),
            "{error}"
        );
        let selected = if replace_root {
            destination.join("_agent_copy")
        } else {
            destination.join("_agent_copy/default-tools")
        };
        assert_eq!(
            std::fs::read(selected.join("sentinel")).unwrap(),
            b"external"
        );
        if !replace_root {
            assert_eq!(
                std::fs::read(selected.join(".hidden/data")).unwrap(),
                b"unknown"
            );
        }
        assert!(destination.join("moved-owned-directory").exists());
    }
}

#[cfg(windows)]
#[test]
fn issue_3068_cleanup_preserves_replaced_directory_windows() {
    issue_3068_replaced_directory_cleanup();
}

#[cfg(unix)]
#[test]
fn issue_3068_cleanup_preserves_replaced_directory_unix() {
    issue_3068_replaced_directory_cleanup();
}

#[test]
fn issue_3068_cleanup_preserves_same_bytes_replacement_and_uncertain_identity() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let mut owned =
        copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_copy").unwrap();
    let path = owned.root.join("Role.md");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::rename(&path, destination.join("moved-file")).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let error = cleanup_owned_copy_artifacts(&owned).unwrap_err();
    assert!(error.contains("replaced"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    // An uncertain identity must preserve even a byte-identical root.
    owned.entries[0].identity = MatrixCopyIdentity(0, 0);
    assert!(cleanup_owned_copy_artifacts(&owned)
        .unwrap_err()
        .contains("residues"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn issue_3068_provenance_destination_is_created_exclusively() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let mut callback = |stage, root: &Path| {
        if stage == MatrixCopyStage::Provenance {
            std::fs::write(root.join(COPY_PROVENANCE_FILE), b"foreign provenance").unwrap();
        }
        Ok(())
    };
    let error = copy_agent_matrix_strict_with_hooks(
        &project,
        "alpha",
        &destination,
        "_agent_copy",
        &mut MatrixCopyHooks {
            callback: Some(&mut callback),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.contains("residues"));
    assert_eq!(
        std::fs::read(destination.join("_agent_copy").join(COPY_PROVENANCE_FILE)).unwrap(),
        b"foreign provenance"
    );
}

#[test]
fn issue_3068_hardlinked_source_creates_independent_output_files() {
    let (_tmp, project, source, destination) = issue_3068_fixture();
    std::fs::hard_link(source.join("Role.md"), source.join("skills/Role-linked.md")).unwrap();
    let before = issue_3068_inventory(&source);
    let owned = copy_agent_matrix_strict(&project, "alpha", &destination, "_agent_copy").unwrap();
    let first = File::open(owned.root.join("Role.md")).unwrap();
    let second = File::open(owned.root.join("skills/Role-linked.md")).unwrap();
    let original = File::open(source.join("Role.md")).unwrap();
    assert_ne!(
        OwnedCopyArtifacts::handle_identity(&first).unwrap(),
        OwnedCopyArtifacts::handle_identity(&second).unwrap()
    );
    assert_ne!(
        OwnedCopyArtifacts::handle_identity(&first).unwrap(),
        OwnedCopyArtifacts::handle_identity(&original).unwrap()
    );
    drop((first, second, original));
    std::fs::write(owned.root.join("skills/Role-linked.md"), b"copy changed").unwrap();
    assert_eq!(before, issue_3068_inventory(&source));
    assert_ne!(
        std::fs::read(owned.root.join("skills/Role-linked.md")).unwrap(),
        std::fs::read(owned.root.join("Role.md")).unwrap()
    );
}

#[test]
fn issue_3068_source_changes_during_streaming_fail() {
    let (_tmp, project, source, destination) = issue_3068_fixture();
    std::fs::write(source.join("skills/large.bin"), vec![8u8; 192 * 1024]).unwrap();
    let mut changed = false;
    let mut callback = |stage, path: &Path| {
        if stage == MatrixCopyStage::Streaming && path.ends_with("large.bin") && !changed {
            use std::io::Write;
            OpenOptions::new()
                .append(true)
                .open(path)
                .unwrap()
                .write_all(b"mutation")
                .unwrap();
            changed = true;
        }
        Ok(())
    };
    let error = copy_agent_matrix_strict_with_hooks(
        &project,
        "alpha",
        &destination,
        "_agent_copy",
        &mut MatrixCopyHooks {
            callback: Some(&mut callback),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(changed);
    assert!(error.contains("changed"), "{error}");
    assert!(!destination.join("_agent_copy").exists());
}

#[test]
fn issue_3068_replaced_output_root_stops_child_creation() {
    let (_tmp, project, _source, destination) = issue_3068_fixture();
    let mut callback = |stage, root: &Path| {
        if stage == MatrixCopyStage::Reserved {
            std::fs::rename(root, destination.join("moved-reservation")).unwrap();
            std::fs::create_dir(root).unwrap();
            std::fs::write(root.join("sentinel"), b"external").unwrap();
        }
        Ok(())
    };
    let error = copy_agent_matrix_strict_with_hooks(
        &project,
        "alpha",
        &destination,
        "_agent_copy",
        &mut MatrixCopyHooks {
            callback: Some(&mut callback),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        error.contains("replaced") && error.contains("residues"),
        "{error}"
    );
    let root = destination.join("_agent_copy");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    assert_eq!(std::fs::read(root.join("sentinel")).unwrap(), b"external");
}
