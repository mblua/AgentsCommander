//! #1172 - rotate an origin Agent Matrix's `memory/` to a timestamped sibling at
//! session spawn, so a fresh agent starts with a clean write target and the
//! previous session's memory is preserved rather than deleted.

use std::path::{Path, PathBuf};

/// Mirrors the `"memory"` entry of
/// `crate::commands::entity_creation::AGENT_MATRIX_DIRS` (`entity_creation.rs:128`).
const MEMORY_DIR_NAME: &str = "memory";

/// Deliberate independent twin of `phone::mailbox::ARCHIVE_TIMESTAMP_FORMAT`
/// (`mailbox.rs:848`), per D3. That constant is part of the `self-clear/`
/// naming contract the #749 resume prompt and the CLI help expose to agents.
/// The two must stay free to diverge, so the literal is duplicated instead of
/// exported from `phone` into `config`.
const MEMORY_ROTATION_TIMESTAMP_FORMAT: &str = "%Y%m%d_%H%M%S";

/// Two jobs, both load-bearing: it renders a path for logs so operators never
/// read `\\?\C:\...` in `app.log`, and it performs the `Path` to `String`
/// conversion that `is_canonical_agent_matrix_dir` requires. Same private
/// one-liner as `replica_identity.rs:23-25` and `root_agent.rs:1924`. The
/// conversion goes through `to_string_lossy`, so it is lossy in principle; that
/// is fail-safe here, because a degraded path fails `canonicalize` and the
/// caller rotates nothing.
fn display_path(path: &Path) -> String {
    crate::path_utils::path_to_string_without_windows_verbatim_prefix(path)
}

/// Create `path` as an empty directory, treating "someone else already made it"
/// as success. `AlreadyExists` means a concurrent session won the race to the
/// same desired end state, so warning about it would report a failure where
/// nothing failed (D9 sub-decision).
fn ensure_dir(path: &Path) -> std::io::Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// Launch-path entry point (D5). Returns nothing and never propagates an error:
/// every internal failure path warns and returns, so a rotation cannot abort a
/// session launch (acceptance criterion 3).
///
/// The FRESH-versus-resume decision is the caller's, not this function's. The
/// call site in `commands/session.rs` gates on `skip_auto_resume`; see D5 for
/// why that flag is the signal and what each entry point passes.
pub(crate) fn rotate_origin_memory_at_spawn(agent_root: &str) {
    let Some(matrix_root) = resolve_rotatable_matrix_root(agent_root) else {
        return;
    };
    let timestamp = chrono::Local::now()
        .format(MEMORY_ROTATION_TIMESTAMP_FORMAT)
        .to_string();
    match rotate_memory_dir(&matrix_root, &timestamp) {
        Ok(Some(dst)) => log::info!(
            "[memory] #1172: rotated {} -> {} at fresh session spawn",
            // 11.3.1: the source is a pure function of `matrix_root`.
            display_path(&matrix_root.join(MEMORY_DIR_NAME)),
            display_path(&dst)
        ),
        // Absent `memory/` (repaired in place, D9), or present and empty (D2).
        // Acceptance criterion 2 requires nothing at info level or above here.
        Ok(None) => {}
        Err(e) => log::warn!(
            "[memory] #1172: memory rotation failed for {} (non-fatal; the session still launches): {}",
            display_path(&matrix_root),
            e
        ),
    }
}

/// Resolve the origin Agent Matrix whose `memory/` this session rotates, or
/// `None` when nothing may be rotated.
///
/// D7: the replica identity is read with the read-only reader.
/// `resolve_replica_matrix_root` is not used because it rewrites `config.json`
/// on every call through `local_config_io::update_config_json_object`, and the
/// context path has already read and repaired that file by the time this runs.
///
/// D6: `ac-root-agent` cannot satisfy `is_canonical_agent_matrix_dir`, so the
/// Root Agent is excluded by the filter itself.
fn resolve_rotatable_matrix_root(agent_root: &str) -> Option<PathBuf> {
    let candidate = if crate::config::session_context::is_replica_agent_dir(agent_root) {
        let (_config, identity) =
            crate::config::replica_identity::read_wg_replica_config_read_only(Path::new(
                agent_root,
            ))
            .ok()?;
        display_path(&identity.matrix_dir)
    } else {
        agent_root.to_string()
    };
    // Re-validate whatever the identity resolved to. A replica pointing at
    // something that is not a canonical matrix rotates nothing.
    if !crate::config::session_context::is_canonical_agent_matrix_dir(&candidate) {
        return None;
    }
    // Canonical (verbatim on Windows) is what the filesystem calls below use;
    // logging normalizes it (11.3.2).
    std::fs::canonicalize(&candidate).ok()
}

/// The deterministic core, modeled on `phone::mailbox::archive_root_md`
/// (`mailbox.rs:1595-1617`). The timestamp is injected by the caller, so this is
/// testable without mocking the clock.
///
/// `Ok(None)` means no rotation happened: `memory/` was absent (and has been
/// recreated empty, D9) or was present and empty (D2).
/// `Ok(Some(dst))` is the rotated directory.
fn rotate_memory_dir(matrix_root: &Path, timestamp: &str) -> std::io::Result<Option<PathBuf>> {
    let src = matrix_root.join(MEMORY_DIR_NAME);
    let Ok(metadata) = std::fs::symlink_metadata(&src) else {
        // D9: absent is repaired, not merely skipped. Without this, a failed
        // recreate below would leave the matrix permanently without a write
        // target, because this branch is what every later launch takes.
        // Best-effort: a failure here warns and still reports "no rotation".
        if let Err(e) = ensure_dir(&src) {
            log::warn!(
                "[memory] #1172: could not restore a missing memory/ at {} (non-fatal): {}",
                display_path(&src),
                e
            );
        }
        return Ok(None);
    };
    // 11.3.4: `is_symlink()` alone is FALSE for a Windows junction, which is
    // exactly the case this rejects. Renaming a junction would move the link
    // entry and the recreate below would replace the user's link with a real
    // directory. Same call shape as `mailbox.rs:89-91`. This reaches for the
    // `pub(crate)` helper in `entity_creation` rather than the private copy in
    // `session_context.rs:2322`, whose doc forbids refactoring the inlined
    // checks into a shared helper *there*; reusing an already-shared
    // `pub(crate)` one from another module is what `mailbox.rs:90` does.
    if !metadata.is_dir()
        || crate::commands::entity_creation::metadata_is_link_or_reparse(&metadata)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "memory is not a real directory",
        ));
    }
    // D2: an empty `memory/` is not rotated.
    if std::fs::read_dir(&src)?.next().is_none() {
        return Ok(None);
    }
    let dst = matrix_root.join(format!("{}_{}", MEMORY_DIR_NAME, timestamp));
    // Refuse to clobber (`mailbox.rs:1606-1613`). `symlink_metadata` rather
    // than `exists()`, so a dangling link at the destination also blocks. This
    // is a best-effort fast path, NOT an atomic guarantee; see section 7 for
    // what actually makes data loss impossible in the residual race.
    if std::fs::symlink_metadata(&dst).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "memory rotation target already exists",
        ));
    }
    // No retry loop, per `mailbox.rs:1615` and its rationale at `:1590-1593`.
    std::fs::rename(&src, &dst)?;
    // Restore the write target so the layout contract of `entity_creation.rs:128`
    // stays complete. The rotation itself already succeeded, so this warns
    // rather than turning into `Err`; D9's self-repair above is what keeps a
    // failure here from being permanent.
    if let Err(e) = ensure_dir(&src) {
        log::warn!(
            "[memory] #1172: rotated {} but failed to recreate an empty memory/ at {} (the next fresh launch restores it): {}",
            display_path(&dst),
            display_path(&src),
            e
        );
    }
    Ok(Some(dst))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rotated sibling of `memory/`, sorted, as directory names.
    fn rotated_entries(matrix_root: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(matrix_root)
            .expect("read matrix root")
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
        // destination are siblings and therefore always on one volume.
        write_file(&memory.join("notes").join("deep.md"), "nested");

        let got = rotate_memory_dir(matrix_root, "20260102_030405").expect("rotation succeeds");

        let expected_dst = matrix_root.join("memory_20260102_030405");
        assert_eq!(got, Some(expected_dst.clone()));
        assert_eq!(
            std::fs::read_to_string(expected_dst.join("MEMORY.md")).unwrap(),
            "remembered bytes",
            "the archive keeps the original bytes"
        );
        assert!(
            expected_dst.join("notes").join("deep.md").is_file(),
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
        let existing = matrix_root.join("memory_20260102_030405");
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
            std::fs::read_to_string(matrix.join(&rotated[0]).join("MEMORY.md")).unwrap(),
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
        assert_eq!(rotated.len(), 1, "the rotation landed in the origin matrix");
        assert_eq!(
            std::fs::read_to_string(matrix.join(&rotated[0]).join("MEMORY.md")).unwrap(),
            "origin memory"
        );
        let memory = matrix.join("memory");
        assert!(memory.is_dir(), "the matrix keeps an empty memory/");
        assert_eq!(entry_count(&memory), 0);
        assert!(
            !replica.join("memory").exists() && rotated_entries(&replica).is_empty(),
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
}
