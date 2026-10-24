use super::*;

/// Every nested rotation, sorted, as directory names.
fn rotated_entries(matrix_root: &Path) -> Vec<String> {
    let entries = match std::fs::read_dir(matrix_root.join("memory-archive")) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => panic!("read memory archive: {err}"),
    };
    let mut names: Vec<String> = entries
        .map(|entry| entry.expect("dir entry").file_name())
        .filter_map(|name| name.to_str().map(str::to_string))
        .filter(|name| name.starts_with("memory_"))
        .collect();
    names.sort();
    names
}

fn entry_count(dir: &Path) -> usize {
    std::fs::read_dir(dir).expect("read dir").count()
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(path, contents).expect("write file");
}

// T1 - D9 and acceptance criterion 2. Pass 1 asserted the opposite (that
// nothing is created), which pinned the terminal state G4 describes.
#[test]
fn rotate_memory_dir_absent_memory_is_repaired_not_rotated() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix_root = tmp.path();

    let got = rotate_memory_dir(matrix_root, "20260102_030405").expect("absent memory is Ok");

    assert!(
        got.is_none(),
        "an absent memory/ must not report a rotation"
    );
    let memory = matrix_root.join("memory");
    assert!(memory.is_dir(), "D9: an absent memory/ is created empty");
    assert_eq!(entry_count(&memory), 0, "the restored memory/ starts empty");
    assert!(!matrix_root.join("memory-archive").exists());
    assert!(
        rotated_entries(matrix_root).is_empty(),
        "nothing is rotated when there was no memory/"
    );
}

// T2 - D2.
#[test]
fn rotate_memory_dir_empty_memory_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix_root = tmp.path();
    let memory = matrix_root.join("memory");
    std::fs::create_dir(&memory).unwrap();

    let got = rotate_memory_dir(matrix_root, "20260102_030405").expect("empty memory is Ok");

    assert!(got.is_none(), "D2: an empty memory/ is not rotated");
    assert!(memory.is_dir(), "the empty memory/ stays where it is");
    assert_eq!(entry_count(&memory), 0);
    assert!(!matrix_root.join("memory-archive").exists());
    assert!(
        rotated_entries(matrix_root).is_empty(),
        "D2: no empty rotation directory is created"
    );
}

// T3 - acceptance criterion 1 and D1.
#[test]
fn rotate_memory_dir_moves_contents_and_recreates_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix_root = tmp.path();
    let memory = matrix_root.join("memory");
    write_file(&memory.join("MEMORY.md"), "remembered bytes");
    // `std::fs::rename` moves the whole subtree in one call: source and
    // destination share the Matrix filesystem.
    write_file(&memory.join("notes").join("deep.md"), "nested");

    let got = rotate_memory_dir(matrix_root, "20260102_030405").expect("rotation succeeds");

    let expected_dst = matrix_root
        .join("memory-archive")
        .join("memory_20260102_030405");
    assert_eq!(got, Some(expected_dst.clone()));
    assert!(!matrix_root.join("memory_20260102_030405").exists());
    assert_eq!(
        std::fs::read_to_string(expected_dst.join("MEMORY.md")).unwrap(),
        "remembered bytes",
        "the archive keeps the original bytes"
    );
    assert!(
        std::fs::read(expected_dst.join("notes/deep.md")).unwrap() == b"nested",
        "the whole subtree moves with the rename"
    );
    assert!(memory.is_dir(), "D1: memory/ is recreated");
    assert_eq!(
        entry_count(&memory),
        0,
        "D1: the recreated memory/ is empty"
    );
}

// T4 - acceptance criterion 4.
#[test]
fn rotate_memory_dir_refuses_existing_target_without_clobber() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix_root = tmp.path();
    let existing = matrix_root
        .join("memory-archive")
        .join("memory_20260102_030405");
    write_file(&existing.join("sentinel.md"), "earlier archive");
    write_file(&matrix_root.join("memory").join("MEMORY.md"), "live memory");

    let err = rotate_memory_dir(matrix_root, "20260102_030405")
        .expect_err("an existing target must refuse");

    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        std::fs::read_to_string(matrix_root.join("memory").join("MEMORY.md")).unwrap(),
        "live memory",
        "the live memory/ is left exactly where it was"
    );
    assert_eq!(
        std::fs::read_to_string(existing.join("sentinel.md")).unwrap(),
        "earlier archive",
        "the pre-existing archive is never clobbered"
    );
}

#[test]
fn nested_rotation_leaves_same_timestamp_legacy_archive_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix = tmp.path();
    let legacy = matrix.join("memory_20260102_030405");
    write_file(&legacy.join("sentinel.md"), "legacy bytes");
    write_file(&matrix.join("memory/MEMORY.md"), "live bytes");
    let dst = rotate_memory_dir(matrix, "20260102_030405")
        .unwrap()
        .unwrap();
    assert_eq!(dst, matrix.join("memory-archive/memory_20260102_030405"));
    assert_eq!(
        std::fs::read(legacy.join("sentinel.md")).unwrap(),
        b"legacy bytes"
    );
    assert_eq!(std::fs::read(dst.join("MEMORY.md")).unwrap(), b"live bytes");
    assert_eq!(entry_count(&matrix.join("memory")), 0);
}

#[test]
fn nested_rotation_refuses_an_occupied_file() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix = tmp.path();
    let dst = matrix.join("memory-archive/memory_20260102_030405");
    write_file(&dst, "occupied bytes");
    write_file(&matrix.join("memory/MEMORY.md"), "live bytes");
    let err = rotate_memory_dir(matrix, "20260102_030405").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&dst).unwrap(), b"occupied bytes");
    assert_eq!(
        std::fs::read(matrix.join("memory/MEMORY.md")).unwrap(),
        b"live bytes"
    );
}

#[test]
fn nested_rotation_refuses_a_file_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix = tmp.path();
    write_file(&matrix.join("memory-archive"), "parent bytes");
    write_file(&matrix.join("memory/MEMORY.md"), "live bytes");
    let err = rotate_memory_dir(matrix, "20260102_030405").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(
        std::fs::read(matrix.join("memory-archive")).unwrap(),
        b"parent bytes"
    );
    assert_eq!(
        std::fs::read(matrix.join("memory/MEMORY.md")).unwrap(),
        b"live bytes"
    );
}

#[test]
fn nested_rotation_accepts_an_existing_directory_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix = tmp.path();
    write_file(&matrix.join("memory-archive/sentinel.md"), "parent bytes");
    write_file(&matrix.join("memory/MEMORY.md"), "live bytes");
    let dst = rotate_memory_dir(matrix, "20260102_030405")
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read(dst.join("MEMORY.md")).unwrap(), b"live bytes");
    assert_eq!(
        std::fs::read(matrix.join("memory-archive/sentinel.md")).unwrap(),
        b"parent bytes"
    );
    assert!(!matrix.join("memory_20260102_030405").exists());
}

#[cfg(windows)]
#[test]
fn nested_rotation_refuses_parent_and_destination_junctions() {
    for parent_link in [true, false] {
        let tmp = tempfile::tempdir().unwrap();
        let matrix = tmp.path();
        let target = matrix.join("real_target");
        write_file(&target.join("sentinel.md"), "linked bytes");
        write_file(&matrix.join("memory/MEMORY.md"), "live bytes");
        let parent = matrix.join("memory-archive");
        let link = if parent_link {
            parent
        } else {
            std::fs::create_dir(&parent).unwrap();
            parent.join("memory_20260102_030405")
        };
        let output = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&link)
            .arg(&target)
            .output()
            .expect("invoke mklink");
        assert!(
            output.status.success(),
            "junction setup failed: {:?}",
            output
        );
        let err = rotate_memory_dir(matrix, "20260102_030405").unwrap_err();
        assert_eq!(
            err.kind(),
            if parent_link {
                std::io::ErrorKind::InvalidInput
            } else {
                std::io::ErrorKind::AlreadyExists
            }
        );
        assert!(std::fs::symlink_metadata(&link).is_ok());
        assert_eq!(
            std::fs::read(target.join("sentinel.md")).unwrap(),
            b"linked bytes"
        );
        assert_eq!(
            std::fs::read(matrix.join("memory/MEMORY.md")).unwrap(),
            b"live bytes"
        );
        assert!(!matrix.join("memory_20260102_030405").exists());
    }
}

// T5 - this exercises the `!metadata.is_dir()` half of the condition ONLY.
// T14 is what guards the reparse half; a regular file is rejected by
// `is_dir()` even with the original `is_symlink()` bug in place.
#[test]
fn rotate_memory_dir_refuses_non_directory_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix_root = tmp.path();
    let memory = matrix_root.join("memory");
    std::fs::write(&memory, "not a directory").unwrap();

    let err = rotate_memory_dir(matrix_root, "20260102_030405")
        .expect_err("a non-directory memory must refuse");

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(
        std::fs::read_to_string(&memory).unwrap(),
        "not a directory",
        "the file is untouched"
    );
}

// T6 - the decided return shape: raw canonical (verbatim on Windows),
// normalized only for logging (11.3.2).
#[test]
fn resolve_rotatable_matrix_root_accepts_canonical_matrix() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix = tmp.path().join(".ac").join("_agent_x");
    std::fs::create_dir_all(&matrix).unwrap();

    let got = resolve_rotatable_matrix_root(matrix.to_str().unwrap());

    assert_eq!(got, Some(std::fs::canonicalize(&matrix).unwrap()));
}

// T7 - acceptance criterion 5. The `role_experiment` shape: a replica that
// does carry a `memory/` but has no `config.json`, so the identity read
// fails and nothing is rotated.
#[test]
fn resolve_rotatable_matrix_root_rejects_replica_without_config() {
    let tmp = tempfile::tempdir().unwrap();
    let replica = tmp.path().join(".ac").join("wg-1-team").join("__agent_x");
    write_file(&replica.join("memory").join("MEMORY.md"), "replica memory");

    assert_eq!(
        resolve_rotatable_matrix_root(replica.to_str().unwrap()),
        None,
        "a replica with no config.json resolves to nothing"
    );

    rotate_origin_memory_at_spawn(replica.to_str().unwrap());

    assert_eq!(
        std::fs::read_to_string(replica.join("memory").join("MEMORY.md")).unwrap(),
        "replica memory",
        "a replica's own memory/ is never touched"
    );
    assert!(
        rotated_entries(&replica).is_empty(),
        "no rotation directory is created inside a replica"
    );
    assert!(!replica.join("memory-archive").exists());
}

// T8.
#[test]
fn resolve_rotatable_matrix_root_rejects_plain_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("just-a-folder");
    std::fs::create_dir_all(&plain).unwrap();

    assert_eq!(resolve_rotatable_matrix_root(plain.to_str().unwrap()), None);
}

// T9 - criteria 1 and 3 through the entry point, on the real clock.
#[test]
fn rotate_origin_memory_at_spawn_rotates_a_direct_matrix_session() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix = tmp.path().join(".ac").join("_agent_x");
    write_file(&matrix.join("memory").join("MEMORY.md"), "direct matrix");

    rotate_origin_memory_at_spawn(matrix.to_str().unwrap());

    let rotated = rotated_entries(&matrix);
    assert!(std::fs::read_dir(&matrix).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("memory_")));
    assert_eq!(rotated.len(), 1, "exactly one rotation directory");
    let suffix = rotated[0]
        .strip_prefix("memory_")
        .expect("rotated name keeps the memory_ prefix");
    // `%Y%m%d_%H%M%S` is 8 + 1 + 6. Byte slices rather than `regex`, which
    // has no business being pulled into a unit test.
    assert_eq!(suffix.len(), 15, "timestamp suffix is 8 + 1 + 6 characters");
    let bytes = suffix.as_bytes();
    assert!(bytes[..8].iter().all(u8::is_ascii_digit), "date is digits");
    assert_eq!(
        bytes[8], b'_',
        "date and time are separated by an underscore"
    );
    assert!(bytes[9..].iter().all(u8::is_ascii_digit), "time is digits");
    assert_eq!(
        std::fs::read_to_string(
            matrix
                .join("memory-archive")
                .join(&rotated[0])
                .join("MEMORY.md")
        )
        .unwrap(),
        "direct matrix"
    );
    let memory = matrix.join("memory");
    assert!(memory.is_dir());
    assert_eq!(entry_count(&memory), 0);
}

// T12 - the D7 happy path, the one every real session takes. T7 exercises
// only its failing branch.
#[test]
fn resolve_rotatable_matrix_root_follows_a_replica_to_its_origin_matrix() {
    let tmp = tempfile::tempdir().unwrap();
    let ac_root = tmp.path().join(".ac");
    let matrix = ac_root.join("_agent_x");
    write_file(&matrix.join("memory").join("MEMORY.md"), "origin memory");
    let replica = ac_root.join("wg-1-team").join("__agent_x");
    // `expected_wg_replica_identity` derives the matrix from the layout and
    // compares only the persisted string's agent name, so the value has to
    // name `_agent_x`.
    write_file(
        &replica.join("config.json"),
        "{\"identity\": \"../../_agent_x\"}",
    );

    rotate_origin_memory_at_spawn(replica.to_str().unwrap());

    let rotated = rotated_entries(&matrix);
    assert!(std::fs::read_dir(&matrix).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("memory_")));
    assert_eq!(rotated.len(), 1, "the rotation landed in the origin matrix");
    assert_eq!(
        std::fs::read_to_string(
            matrix
                .join("memory-archive")
                .join(&rotated[0])
                .join("MEMORY.md")
        )
        .unwrap(),
        "origin memory"
    );
    let memory = matrix.join("memory");
    assert!(memory.is_dir(), "the matrix keeps an empty memory/");
    assert_eq!(entry_count(&memory), 0);
    assert!(
        !replica.join("memory").exists()
            && !replica.join("memory-archive").exists()
            && rotated_entries(&replica).is_empty(),
        "the replica itself gains no memory* entry"
    );
}

// T13 - D6 is an explicit, user-visible exclusion and currently rests on
// nothing but the `_agent_` prefix inside a predicate in another module.
#[test]
fn rotate_origin_memory_at_spawn_leaves_the_root_agent_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root_agent = tmp.path().join(".ac").join("ac-root-agent");
    write_file(&root_agent.join("memory").join("MEMORY.md"), "root memory");

    rotate_origin_memory_at_spawn(root_agent.to_str().unwrap());

    assert_eq!(
        std::fs::read_to_string(root_agent.join("memory").join("MEMORY.md")).unwrap(),
        "root memory",
        "D6: the Root Agent is never rotated"
    );
    assert!(!root_agent.join("memory-archive").exists());
    assert!(rotated_entries(&root_agent).is_empty());
}

// T14 - THE ONLY GUARD on the reparse-point correction (11.3.4). Without
// it, a later "simplification" back to `metadata.file_type().is_symlink()`
// passes `cargo test`, `cargo clippy`, and review, and silently starts
// renaming junctions, replacing the user's link with a real directory.
// Junction creation needs no elevation, unlike `std::os::windows::fs::symlink_dir`,
// which is why this runs in CI; `commands/session.rs:5210` is the precedent.
#[cfg(windows)]
#[test]
fn rotate_memory_dir_refuses_a_junction_at_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let matrix_root = tmp.path();
    let target = matrix_root.join("real_target");
    write_file(&target.join("sentinel.md"), "linked bytes");
    let link = matrix_root.join("memory");
    let output = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(&link)
        .arg(&target)
        .output()
        .expect("failed to invoke mklink");
    assert!(
        output.status.success(),
        "failed to create junction link='{}' target='{}': stdout={} stderr={}",
        link.display(),
        target.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let err = rotate_memory_dir(matrix_root, "20260102_030405")
        .expect_err("a junction at memory must refuse");

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "the junction entry still exists"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("sentinel.md")).unwrap(),
        "linked bytes",
        "the junction target is untouched"
    );
    assert!(rotated_entries(matrix_root).is_empty());
}
