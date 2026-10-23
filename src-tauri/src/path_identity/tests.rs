use super::*;

mod terminal_snapshot;

#[test]
fn duplicate_json_keys_reject_recursively() {
    assert!(parse_json_no_duplicates(br#"{"a":1,"a":2}"#).is_err());
    assert!(parse_json_no_duplicates(br#"{"a":{"b":1,"b":2}}"#).is_err());
    assert!(parse_json_no_duplicates(br#"{"a":[{"b":1}]}"#).is_ok());
}

#[test]
fn scan_detects_escaped_and_overwritten_privileged_occurrences() {
    let document =
        scan_json_document(br#"{"action":"pty\u002dinput","action":"ordinary","body":"x"}"#)
            .unwrap();
    assert!(document.duplicate_keys);
    assert!(document.privileged_candidate);
    assert_eq!(document.value["action"], "ordinary");

    let standard = scan_json_document(br#"{"body":"pty-input","body":"ordinary"}"#).unwrap();
    assert!(standard.duplicate_keys);
    assert!(!standard.privileged_candidate);
}

#[test]
fn bounded_read_returns_content_fingerprint() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("value.json");
    std::fs::write(&path, b"{}" as &[u8]).unwrap();
    let (bytes, identity) = read_bounded_regular(&path, 16).unwrap();
    assert_eq!(bytes, b"{}");
    assert!(identity.content_sha256.is_some());
    assert!(read_bounded_regular(&path, 1).is_err());
}

#[test]
fn bounded_read_rejects_same_length_in_place_mutation_between_snapshots() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("identity.json");
    std::fs::write(&path, b"first").unwrap();
    let replacement = path.clone();
    let result = read_bounded_regular_inner(&path, 16, move || {
        std::fs::write(replacement, b"other").unwrap();
    });
    assert!(result.is_err());
}

#[test]
fn same_object_removal_rechecks_and_preserves_a_replacement() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("owned-response.json");
    let displaced = directory.path().join("displaced-owned-response");
    std::fs::write(&path, b"owned response").unwrap();
    let expected = verify_regular_file(&path).unwrap();
    let raced_path = path.clone();
    let raced_displaced = displaced.clone();

    assert!(!remove_regular_file_if_same_inner(
        &path,
        &expected,
        move || {
            std::fs::rename(&raced_path, &raced_displaced).unwrap();
            std::fs::write(&raced_path, b"replacement response").unwrap();
        },
    ));
    assert_eq!(std::fs::read(path).unwrap(), b"replacement response");
    assert_eq!(std::fs::read(displaced).unwrap(), b"owned response");
}

#[test]
fn atomic_new_file_publication_never_clobbers_an_existing_destination() {
    let dir = tempfile::TempDir::new().unwrap();
    let source = dir.path().join("source.tmp");
    let destination = dir.path().join("destination.json");
    std::fs::write(&source, b"new").unwrap();
    std::fs::write(&destination, b"existing").unwrap();

    assert!(publish_new_file_atomic(&source, &destination).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"new");
    assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
}

#[test]
fn atomic_regular_replacement_is_bounded_and_leaves_a_complete_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("marker.json");
    std::fs::write(&path, b"old").unwrap();
    let before = verify_regular_file(&path).unwrap();
    let (_, expected) = read_bounded_regular(&path, 32).unwrap();
    let after = replace_regular_file_atomic(&path, &expected, b"new marker", 32).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"new marker");
    assert!(!same_object(&before, &after));
    assert!(replace_regular_file_atomic(&path, &after, &[b'x'; 33], 32).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"new marker");

    let (_, expected) = read_bounded_regular(&path, 32).unwrap();
    std::fs::write(&path, b"other value").unwrap();
    assert!(replace_regular_file_atomic(&path, &expected, b"marker", 32).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"other value");
}

#[cfg(windows)]
#[test]
fn atomic_regular_replacement_supports_long_windows_paths() {
    use std::os::windows::ffi::OsStrExt;

    let directory = tempfile::TempDir::new().unwrap();
    let absolute = std::fs::canonicalize(directory.path()).unwrap();
    let spelling = absolute.to_str().unwrap();
    let mut parent = if let Some(unc) = spelling.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else {
        PathBuf::from(spelling.strip_prefix(r"\\?\").unwrap_or(spelling))
    };
    assert!(parent.is_absolute());
    while parent.as_os_str().encode_wide().count() <= 260 {
        parent.push("long-path-component-abcdefghijklmnopqrstuvwxyz-0123456789");
    }
    std::fs::create_dir_all(&parent).unwrap();
    let source = parent.join(".ac-pty-input-0123456789abcdef0123456789abcdef.tmp");
    let destination = parent.join("marker.json");
    for (name, path) in [("source", &source), ("destination", &destination)] {
        let length = path.as_os_str().encode_wide().count();
        assert!(length > 260);
        assert!(!path.to_str().unwrap().starts_with(r"\\?\"));
        eprintln!(
            "{name}: UTF16 length={length}, ordinary input={}",
            path.display()
        );
    }
    std::fs::write(&source, b"direct replacement").unwrap();
    std::fs::write(&destination, b"old marker").unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), b"direct replacement");
    assert_eq!(std::fs::read(&destination).unwrap(), b"old marker");
    verify_regular_file(&source).unwrap();
    verify_regular_file(&destination).unwrap();
    eprintln!("setup and ordinary-input assertions passed; calling atomic_replace_existing");
    atomic_replace_existing(&source, &destination)
        .expect("direct atomic_replace_existing must support ordinary long paths");
    assert_eq!(std::fs::read(&destination).unwrap(), b"direct replacement");
    assert!(!source.exists());

    let (_, expected) = read_bounded_regular(&destination, 64).unwrap();
    let after =
        replace_regular_file_atomic(&destination, &expected, b"public replacement", 64).unwrap();
    let (bytes, fresh) = read_bounded_regular(&destination, 64).unwrap();
    assert_eq!(bytes, b"public replacement");
    assert!(!same_object(&expected, &after));
    assert_eq!(after.content_sha256, fresh.content_sha256);
    assert!(after.content_sha256.is_some());
    assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1);
    assert_eq!(
        replace_regular_file_atomic(&destination, &after, &[b'x'; 65], 64).unwrap_err(),
        "capacity_exceeded"
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"public replacement");
    let (_, expected) = read_bounded_regular(&destination, 64).unwrap();
    std::fs::write(&destination, b"tamper replacement").unwrap();
    assert_eq!(
        replace_regular_file_atomic(&destination, &expected, b"new marker", 64).unwrap_err(),
        "unsafe_path"
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"tamper replacement");
    assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1);
}

#[test]
fn regular_file_verification_rejects_hard_links() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("identity.json");
    let alias = dir.path().join("identity-alias.json");
    std::fs::write(&path, b"{}").unwrap();
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(verify_regular_file(&path).is_err());
    assert!(read_bounded_regular(&path, 16).is_err());
}

#[test]
fn directory_identity_detects_same_spelling_replacement() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("replica");
    let retired = dir.path().join("retired");
    std::fs::create_dir(&path).unwrap();
    let before = verify_directory(&path).unwrap();
    std::fs::rename(&path, &retired).unwrap();
    std::fs::create_dir(&path).unwrap();
    let after = verify_directory(&path).unwrap();
    assert!(!same_object(&before, &after));
}

#[test]
fn component_chain_rejects_a_linked_ancestor_when_supported() {
    let dir = tempfile::TempDir::new().unwrap();
    let real = dir.path().join("real");
    let link = dir.path().join("link");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("identity.json"), b"{}").unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&real, &link).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
    #[cfg(not(any(unix, windows)))]
    let linked = false;
    if linked {
        assert!(verify_directory(&link).is_err());
        assert!(read_bounded_regular(&link.join("identity.json"), 16).is_err());
    }
}

#[test]
fn terminal_snapshot_output_is_absolute_png_create_new() {
    let directory = tempfile::TempDir::new().unwrap();
    let output = directory.path().join("snapshot.PNG");
    let file = create_terminal_snapshot_output(&output).unwrap();
    file.write_all_and_sync(b"owned bytes").unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), b"owned bytes");
    assert!(create_terminal_snapshot_output(&output).is_err());
    assert!(validate_terminal_snapshot_output_path(Path::new("relative.png")).is_err());
    assert!(validate_terminal_snapshot_output_path(&directory.path().join("wrong.txt")).is_err());
}

// ── #2228 phase 2: named path rules ──

#[test]
fn verify_directory_reason_names_the_failing_rule() {
    let temp = tempfile::TempDir::new().unwrap();
    let missing = temp.path().join("absent");
    assert_eq!(
        verify_directory_reason(&missing),
        Err(PathRule::ComponentUnreadable)
    );

    let file = temp.path().join("regular");
    std::fs::write(&file, b"x").unwrap();
    assert_eq!(verify_directory_reason(&file), Err(PathRule::NotADirectory));

    let parent = temp.path().join("child").join("..");
    std::fs::create_dir_all(temp.path().join("child")).unwrap();
    assert_eq!(
        verify_directory_reason(&parent),
        Err(PathRule::ParentComponent)
    );
}

/// The executable form of "a path can never become a rule name".
#[test]
fn path_rule_vocabulary_is_closed_and_literal() {
    for rule in [
        PathRule::ParentComponent,
        PathRule::ComponentUnreadable,
        PathRule::ComponentIsLink,
        PathRule::NotADirectory,
        PathRule::OpenFailed,
        PathRule::HandleIdentityUnavailable,
        PathRule::CanonicalizeFailed,
        PathRule::IdentityChangedDuringCheck,
    ] {
        let rendered = rule.as_str();
        assert!(!rendered.is_empty(), "{rule:?} renders empty");
        let mut characters = rendered.chars();
        let first = characters.next().unwrap();
        assert!(
            first.is_ascii_lowercase(),
            "{rendered:?} must start with [a-z]"
        );
        assert!(
            rendered
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{rendered:?} must match ^[a-z][a-z0-9_]*$"
        );
        for forbidden in ['/', '\\', ':', '.', ' '] {
            assert!(
                !rendered.contains(forbidden),
                "{rendered:?} must not contain {forbidden:?}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn verify_directory_reason_rejects_symlinked_component() {
    let temp = tempfile::TempDir::new().unwrap();
    let real = temp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(
        verify_directory_reason(&link),
        Err(PathRule::ComponentIsLink)
    );
}

// Simulated flags exercise the shared rejection algorithm without creating
// links. This does not cover real Windows reparse metadata or no-follow opens.
fn simulated_reparse_inspector(marked: &Path, path: &Path) -> Result<bool, PathRule> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| PathRule::ComponentUnreadable)?;
    Ok(link_flags_indicate_link_or_reparse(
        metadata.file_type().is_symlink(),
        if path == marked { 0x0400 } else { 0 },
    ))
}

#[test]
fn verify_directory_reason_rejects_simulated_reparse_component() {
    let temp = tempfile::TempDir::new().unwrap();
    let ancestor = temp.path().join("ancestor");
    let descendant = ancestor.join("descendant");
    std::fs::create_dir_all(&descendant).unwrap();
    for marked in [&ancestor, &descendant] {
        assert_eq!(
            verify_directory_with_inspector(&descendant, |path| {
                simulated_reparse_inspector(marked, path)
            }),
            Err(PathRule::ComponentIsLink)
        );
    }
}

#[test]
fn component_chain_stops_at_simulated_reparse() {
    let temp = tempfile::TempDir::new().unwrap();
    let ancestor = temp.path().join("ancestor");
    let descendant = ancestor.join("descendant");
    std::fs::create_dir_all(&descendant).unwrap();
    for marked in [&ancestor, &descendant] {
        let mut expected: Vec<_> = marked
            .ancestors()
            .filter(|path| path.file_name().is_some())
            .map(Path::to_path_buf)
            .collect();
        expected.reverse();
        let mut visited = Vec::new();
        assert_eq!(
            verify_component_chain_with_inspector(&descendant, |path| {
                visited.push(path.to_path_buf());
                simulated_reparse_inspector(marked, path)
            }),
            Err(PathRule::ComponentIsLink)
        );
        assert_eq!(visited, expected);
    }
}

#[test]
fn link_flags_classify_simulated_reparse() {
    assert!(!link_flags_indicate_link_or_reparse(false, 0));
    assert!(!link_flags_indicate_link_or_reparse(false, 0x0080));
    assert!(link_flags_indicate_link_or_reparse(false, 0x0400));
    assert!(link_flags_indicate_link_or_reparse(false, 0x0480));
    assert!(link_flags_indicate_link_or_reparse(true, 0));
}

#[test]
fn verify_directory_reason_accepts_ordinary_directory_simulated_reparse() {
    let temp = tempfile::TempDir::new().unwrap();
    let ordinary = temp.path().join("ordinary");
    std::fs::create_dir(&ordinary).unwrap();
    let absent_marker = temp.path().join("unmarked");
    let identity = verify_directory_with_inspector(&ordinary, |path| {
        simulated_reparse_inspector(&absent_marker, path)
    })
    .unwrap();
    assert!(identity.metadata.is_dir);
    assert_eq!(identity, verify_directory_reason(&ordinary).unwrap());
}

#[test]
fn component_chain_preserves_unreadable_simulated_reparse() {
    let temp = tempfile::TempDir::new().unwrap();
    let missing = temp.path().join("missing");
    assert_eq!(
        verify_component_chain_with_inspector(&missing, |path| {
            simulated_reparse_inspector(&missing, path)
        }),
        Err(PathRule::ComponentUnreadable)
    );
}

#[test]
fn component_chain_preserves_parent_component_simulated_reparse() {
    let temp = tempfile::TempDir::new().unwrap();
    let parent = temp.path().join("..");
    let absent_marker = temp.path().join("unmarked");
    assert_eq!(
        verify_component_chain_with_inspector(&parent, |path| {
            simulated_reparse_inspector(&absent_marker, path)
        }),
        Err(PathRule::ParentComponent)
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn terminal_snapshot_atomic_publish_preserves_collision_missing_source_and_raw_names() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let directory = tempfile::tempdir().unwrap();
    let source = directory
        .path()
        .join(OsString::from_vec(b"source-\xff".to_vec()));
    let destination = directory
        .path()
        .join(OsString::from_vec(b"destination-\xfe".to_vec()));
    std::fs::write(&source, b"source bytes").unwrap();
    std::fs::write(&destination, b"destination bytes").unwrap();

    assert_eq!(
        publish_new_file_atomic(&source, &destination),
        Err("atomic_publish_failed".to_string())
    );
    assert_eq!(std::fs::read(&source).unwrap(), b"source bytes");
    assert_eq!(std::fs::read(&destination).unwrap(), b"destination bytes");

    std::fs::remove_file(&destination).unwrap();
    assert_eq!(publish_new_file_atomic(&source, &destination), Ok(()));
    assert!(!source.exists());
    assert_eq!(std::fs::read(&destination).unwrap(), b"source bytes");

    let missing_source = directory.path().join("missing-source");
    let missing_destination = directory.path().join("missing-destination");
    assert_eq!(
        publish_new_file_atomic(&missing_source, &missing_destination),
        Err("atomic_publish_failed".to_string())
    );
    assert!(!missing_destination.exists());
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
#[test]
fn terminal_snapshot_atomic_publish_is_stably_unsupported_on_other_unix() {
    let result = publish_new_file_atomic(
        std::path::Path::new("source"),
        std::path::Path::new("destination"),
    );
    assert_eq!(result, Err("atomic_publish_unsupported".to_string()));
}
