#[cfg(test)]
use super::run_pre_publish_hook;
use super::{
    atomic_replace_existing, cleanup_temp_role, create_default_skill_temp_file,
    normalize_role_text, publish_missing_default_skill_file, sync_role_dir,
    validate_default_skill_directory, validate_default_skill_file, validate_root_agent_root_path,
    validate_root_agent_skills_root, MAX_DEFAULT_SKILL_TRIM_SLACK_BYTES, SKILL_REPAIR_WARNED,
};
use crate::filesystem::agent_files;
use std::path::Path;

/// Ensures the on-disk default skill file exists and, if it is a pristine copy of a defective
/// version we shipped ourselves, repairs it in place (#909).
///
/// The two arms are deliberately asymmetric and must stay that way. The **NotFound arm** creates
/// the
/// file and publishes with `hard_link`, which is create-only, and it propagates `Err`, because a
/// root
/// agent with no skill file is genuinely broken. The **exists arm** repairs through
/// `atomic_replace_existing`, and its failures are logged and swallowed, because a root agent with
/// a
/// stale skill file merely keeps today's behavior until the next context build retries.
///
/// Do NOT unify the two arms onto one publish primitive: `hard_link` returns `AlreadyExists`
/// against
/// an existing destination, so reusing it for the repair would make the migration a silent no-op on
/// every broken install.
pub(crate) fn ensure_default_skill_file(
    root_dir: &Path,
    skills_root: &Path,
    path: &Path,
    content: &str,
    legacy_snapshots: &[&str],
) -> Result<(), String> {
    match agent_files::inspect_root_agent_entry(path) {
        Ok(metadata) => {
            // An inode-type violation is structural and stays fatal. The `?` is deliberate.
            validate_default_skill_file(path, &metadata)?;

            // A failed repair must never fail a session start, so log and continue.
            //
            // `warn_once_for_path`, not `log::warn!`: a sticky failure we cannot detect in advance
            // (a parent directory denying `FILE_DELETE_CHILD`, say) would otherwise emit a WARN on
            // every root-agent context build, forever, and bury real warnings. The read-only
            // pre-check inside `migrate_default_skill_file` only covers the one instance of that
            // class we could measure.
            //
            // `migrate_default_skill_file` takes no `Metadata` parameter. It stats for itself,
            // immediately before its own open.
            if let Err(e) =
                migrate_default_skill_file(root_dir, skills_root, path, content, legacy_snapshots)
            {
                warn_once_for_path(
                    path,
                    &format!(
                        "Could not repair root agent default skill {}: {}",
                        path.display(),
                        e
                    ),
                );
            }
            return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to inspect root agent default skill file {}: {}",
                path.display(),
                e
            ));
        }
    }

    validate_root_agent_root_path(root_dir)?;
    validate_root_agent_skills_root(skills_root)?;
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for root agent default skill {}",
            path.display()
        )
    })?;
    validate_default_skill_directory(parent)?;
    let (temp_path, mut file) = create_default_skill_temp_file(path)?;

    let write_result = (|| -> Result<(), String> {
        agent_files::write_root_agent_stage(&mut file, content.as_bytes())
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
        agent_files::flush_root_agent_stage(&mut file)
            .map_err(|e| format!("Failed to flush {}: {}", path.display(), e))?;
        agent_files::sync_root_agent_stage(&file)
            .map_err(|e| format!("Failed to sync {}: {}", path.display(), e))
    })();
    if let Err(e) = write_result {
        drop(file);
        cleanup_temp_role(&temp_path);
        return Err(e);
    }
    drop(file);

    validate_root_agent_root_path(root_dir)?;
    validate_root_agent_skills_root(skills_root)?;
    validate_default_skill_directory(parent)?;
    let published = match publish_missing_default_skill_file(&temp_path, path) {
        Ok(published) => published,
        Err(e) => {
            cleanup_temp_role(&temp_path);
            return Err(e);
        }
    };

    cleanup_temp_role(&temp_path);
    if published {
        sync_role_dir(parent);
    }
    Ok(())
}

// -------------------------------------------------------------------------------------------
// #909: repair of pristine stale default skill files.
// -------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SkipReason {
    /// This default ships no frozen snapshot, so it is never repaired.
    NoSnapshots,
    /// Already `content`, modulo line endings and surrounding whitespace.
    AlreadyCurrent,
    /// Not any snapshot. Someone else's bytes. Never touched.
    UserEdit,
    /// Larger than anything that could normalize-equal a snapshot.
    TooLarge,
    /// Not valid UTF-8. Snapshots are `&'static str`, so this can never be repairable. Do not fold
    /// it into `UserEdit`: a merged variant is a lying enum.
    NotUtf8,
    /// Windows only: `FILE_ATTRIBUTE_READONLY` blocks the delete inside `ReplaceFileW`. On Unix,
    /// `rename(2)` checks the directory rather than the file mode, so this is never produced.
    #[cfg_attr(not(windows), allow(dead_code))]
    ReadOnly,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SkillMigration {
    Skipped(SkipReason),
    Repaired,
}

pub(crate) enum Publish {
    Published,
    AlreadyCurrent,
}

/// Outcome of a bounded read. `Err` is reserved for genuine I/O failures, which are transient and
/// deserve a warning. `TooLarge` and `NotUtf8` are classifications, not errors: neither can ever be
/// repairable, so warning about them on every context build would be noise forever.
pub(crate) enum BoundedRead {
    Text(String),
    TooLarge,
    NotUtf8,
}

/// A conservative cap, NOT a provable bound: `trim` removes an unbounded number of bytes, so the
/// pre-image of a match is unbounded. Anything larger is declared a user edit.
///
/// The one consequence, stated plainly because the plan originally hid it: this is the only
/// mechanism in the design that can permanently strand a repairable pristine default. A
/// whitespace-padded copy above the cap matches semantically, is skipped as too large, and stays
/// broken forever. Nothing on disk is in that state.
///
/// Build-machine dependent, because `content.len()` is `include_str!`'s length, and
/// `.gitattributes`
/// carries no `*.md` rule (#914). No fixture may hard-code the result.
pub(crate) fn max_migratable_len(content: &str, legacy_snapshots: &[&str]) -> u64 {
    let longest = legacy_snapshots
        .iter()
        .map(|snapshot| snapshot.len())
        .chain(std::iter::once(content.len()))
        .max()
        .unwrap_or(0);
    (longest as u64) * 2 + MAX_DEFAULT_SKILL_TRIM_SLACK_BYTES
}

/// Reads at most `bound + 1` bytes, relying on no prior `stat`.
///
/// The `+ 1` is what makes a truncated prefix unreachable. `Take::read_to_end` stops only at the
/// take limit or at EOF. If it stopped at the limit, `buf.len() == bound + 1 > bound` and we
/// classify `TooLarge`. So `buf.len() <= bound` implies EOF, which implies `buf` is the whole file,
/// and decoding therefore never sees a split multi-byte character.
///
/// `take(bound)` without the `+ 1` and the rejection is a DATA-LOSS BUG: `SNAPSHOT` + N spaces +
/// `MY NOTES` would read back as a prefix that trims to exactly the snapshot, match, and overwrite
/// the user's notes. Do not write it.
pub(crate) fn read_bounded(path: &Path, bound: u64) -> Result<BoundedRead, String> {
    let file = agent_files::open_root_agent_skill(path)
        .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
    let buf = agent_files::read_root_agent_skill_prefix(file, bound.saturating_add(1))
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    if buf.len() as u64 > bound {
        return Ok(BoundedRead::TooLarge);
    }
    match String::from_utf8(buf) {
        Ok(text) => Ok(BoundedRead::Text(text)),
        Err(_) => Ok(BoundedRead::NotUtf8),
    }
}

pub(crate) fn warn_once_for_path(path: &Path, message: &str) {
    let first = {
        let mut seen = SKILL_REPAIR_WARNED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        seen.insert(path.to_path_buf())
    };
    if first {
        log::warn!("[skills] {} (logged once per process)", message);
    } else {
        log::debug!("[skills] {}", message);
    }
}

pub(crate) fn short_sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    digest[..16].to_string()
}

/// #909: an on-disk default skill file that is a byte-exact copy (modulo line endings and
/// surrounding whitespace) of a previously shipped, defective version is repaired in place.
/// Anything
/// else is treated as a user edit and left alone.
///
/// Classification and write live in this one function on purpose. Hoisting the branches into a
/// `classify()` that the caller acts on would turn `SkillMigration` into decoration and send the
/// tests back to proving nothing.
///
/// Best-effort by contract: every failure returns `Err` for the CALLER to log and swallow. A failed
/// repair must never fail a root-agent context build.
pub(crate) fn migrate_default_skill_file(
    root_dir: &Path,
    skills_root: &Path,
    path: &Path,
    content: &str,
    legacy_snapshots: &[&str],
) -> Result<SkillMigration, String> {
    if legacy_snapshots.is_empty() {
        return Ok(SkillMigration::Skipped(SkipReason::NoSnapshots));
    }

    // A fresh stat, taken as late as possible and immediately before the open. This narrows, and
    // does not close, the window in which a symlink or a FIFO can be planted at `path`. It also
    // supplies the read-only bit consumed below, which is why this function takes no `Metadata`
    // parameter: that parameter was proposed for the size guard (wrong window) and then for the
    // read-only check (wrong position), and neither use survived.
    let metadata = agent_files::inspect_root_agent_entry(path)
        .map_err(|e| format!("Failed to inspect {}: {}", path.display(), e))?;
    validate_default_skill_file(path, &metadata)?;

    let bound = max_migratable_len(content, legacy_snapshots);
    let existing = match read_bounded(path, bound)? {
        BoundedRead::TooLarge => return Ok(SkillMigration::Skipped(SkipReason::TooLarge)),
        BoundedRead::NotUtf8 => return Ok(SkillMigration::Skipped(SkipReason::NotUtf8)),
        BoundedRead::Text(text) => text,
    };

    let existing_normalized = normalize_role_text(&existing);
    let content_normalized = normalize_role_text(content);

    if existing_normalized == content_normalized {
        return Ok(SkillMigration::Skipped(SkipReason::AlreadyCurrent));
    }
    if !legacy_snapshots
        .iter()
        .any(|snapshot| normalize_role_text(snapshot) == existing_normalized)
    {
        return Ok(SkillMigration::Skipped(SkipReason::UserEdit));
    }

    // The file is stale and repairable. ONLY NOW does read-only mean anything: above this line it
    // would warn about healthy files, and the enum would stop distinguishing `ReadOnly` from
    // `AlreadyCurrent` and `UserEdit`, which is the exact defect `SkipReason` exists to kill.
    //
    // Measured, not reasoned: `ReplaceFileW` and `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` both
    // return os error 5 against a read-only destination under every flag combination, because
    // `ReplaceFileW` must delete the replaced file and `FILE_ATTRIBUTE_READONLY` blocks
    // `DeleteFileW`. It is an attribute, not a security descriptor, so no ACL grant and no
    // elevation bypasses it. A deny-write DACL does NOT block the repair (the delete is authorised
    // by the parent's `FILE_DELETE_CHILD`), so the attribute check covers the sticky case exactly.
    //
    // Advisory, not a correctness gate: a file that turns read-only after this stat still fails at
    // `ReplaceFileW`, is swallowed, and is warned once by `warn_once_for_path`.
    //
    // On Unix, `rename(2)` checks the DIRECTORY, not the file mode, so a read-only file repairs
    // fine and must not be rejected here.
    #[cfg(windows)]
    if metadata.permissions().readonly() {
        warn_once_for_path(
            path,
            &format!(
                "Root agent default skill {} is read-only; leaving it unrepaired",
                path.display()
            ),
        );
        return Ok(SkillMigration::Skipped(SkipReason::ReadOnly));
    }

    match atomic_write_default_skill(
        root_dir,
        skills_root,
        path,
        content,
        bound,
        &existing_normalized,
        &content_normalized,
    )? {
        Publish::AlreadyCurrent => Ok(SkillMigration::Skipped(SkipReason::AlreadyCurrent)),
        Publish::Published => {
            // This line requires the runtime log level at Info or lower. The default is Info
            // (`init_logger_inner`: `read_log_level_only().unwrap_or("info")`), so it normally
            // appears. But if the user has lowered verbosity to Warn or Error, the `info!` macro
            // short-circuits before the gate ever sees it, because `max_filter_for`
            // (`logging.rs:66`) leaves the global max level at Warn. The repair still happened;
            // only
            // the record of it is gone. Do not read an absent line as a failed repair: check the
            // file, not the log.
            log::info!(
                "[skills] #909 repaired stale default skill {}: {} -> {}",
                path.display(),
                short_sha256(&existing_normalized),
                short_sha256(&content_normalized),
            );
            Ok(SkillMigration::Repaired)
        }
    }
}

/// Writes `content` VERBATIM (never the normalized form: four pre-existing tests assert
/// `read_to_string(seeded) == content` byte for byte) through a temp file, and publishes with
/// `atomic_replace_existing`, the #664 overwrite primitive.
pub(crate) fn atomic_write_default_skill(
    root_dir: &Path,
    skills_root: &Path,
    path: &Path,
    content: &str,
    bound: u64,
    expected_normalized: &str,
    content_normalized: &str,
) -> Result<Publish, String> {
    validate_root_agent_root_path(root_dir)?;
    validate_root_agent_skills_root(skills_root)?;
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for root agent default skill {}",
            path.display()
        )
    })?;
    validate_default_skill_directory(parent)?;

    let (temp_path, mut file) = create_default_skill_temp_file(path)?;

    let write_result = (|| -> Result<(), String> {
        agent_files::write_root_agent_stage(&mut file, content.as_bytes())
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
        agent_files::flush_root_agent_stage(&mut file)
            .map_err(|e| format!("Failed to flush {}: {}", path.display(), e))?;
        agent_files::sync_root_agent_stage(&file)
            .map_err(|e| format!("Failed to sync {}: {}", path.display(), e))
    })();
    if let Err(e) = write_result {
        drop(file);
        cleanup_temp_role(&temp_path);
        return Err(e);
    }
    drop(file);

    // Every `?` below would otherwise abandon the temp file, hence the closure and the
    // unconditional `cleanup_temp_role` after it.
    let published = (|| -> Result<Publish, String> {
        validate_root_agent_root_path(root_dir)?;
        validate_root_agent_skills_root(skills_root)?;
        validate_default_skill_directory(parent)?;

        // Simulates a writer that mutates the destination after our temp is durable and before we
        // publish. This is the window the re-read below exists to observe, so the hook MUST run
        // before that re-read: placed after it, the three tests it drives prove nothing and one of
        // them silently destroys a user edit inside its own scenario.
        #[cfg(test)]
        run_pre_publish_hook(path)?;

        // Re-verify the destination after the fsync, immediately before publishing. The plan's
        // central promise is "we never touch a user edit", and without this it is made before the
        // fsync and cashed after it. The window shrinks to `stat` + `open` + `ReplaceFileW`. It
        // cannot be closed: Windows has no compare-and-swap-file primitive.
        //
        // Re-validate the inode type too. The NotFound arm has this defence at
        // `publish_missing_default_skill_file_with`; the repair path validated the directory three
        // times and the file never. It cannot close the race, but it rejects a reparse point that
        // has PERSISTED since the stat above, which is the only variant an ordinary user produces
        // by accident.
        let fresh_metadata = agent_files::inspect_root_agent_entry(path)
            .map_err(|e| format!("Failed to re-inspect {}: {}", path.display(), e))?;
        validate_default_skill_file(path, &fresh_metadata)?;

        match read_bounded(path, bound)? {
            // Deliberately NOT symmetric with the first read, which classifies these silently. A
            // file that grows past the bound or turns non-UTF-8 mid-repair is an anomaly worth
            // exactly one warning, and it is self-limiting: the next context build classifies it
            // silently. Do not "harmonise" these.
            BoundedRead::TooLarge | BoundedRead::NotUtf8 => {
                return Err(format!(
                    "{} changed unreadably while it was being repaired; left alone",
                    path.display()
                ))
            }
            BoundedRead::Text(fresh) => {
                let fresh_normalized = normalize_role_text(&fresh);
                if fresh_normalized == content_normalized {
                    // Another writer got there first. Publishing would be a redundant
                    // `ReplaceFileW` over identical bytes.
                    return Ok(Publish::AlreadyCurrent);
                }
                if fresh_normalized != expected_normalized {
                    return Err(format!(
                        "{} changed while it was being repaired; left alone",
                        path.display()
                    ));
                }
            }
        }

        atomic_replace_existing(&temp_path, path)?;
        Ok(Publish::Published)
    })();

    cleanup_temp_role(&temp_path);
    let published = published?;
    if matches!(published, Publish::Published) {
        // NOTE: a silent no-op on Windows, where `File::open` on a directory fails. Do not claim
        // directory-entry durability here. `REPLACEFILE_WRITE_THROUGH` already flushed the file's
        // own data, which is what we need.
        sync_role_dir(parent);
    }
    Ok(published)
}
