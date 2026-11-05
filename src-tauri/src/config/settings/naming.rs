//! The existing instance-settings naming transaction, owned by settings.

use std::io;
use std::path::{Path, PathBuf};

use crate::config::instance_artifacts as names;
use crate::filesystem::naming_journal;

const INSTANCE_FAMILIES_SCOPE: &str = "instance-families";

/// A migration failure projected only to startup diagnostics.
#[derive(Debug)]
pub(crate) struct SettingsNamingError {
    reason: String,
    path: PathBuf,
}

impl SettingsNamingError {
    pub(crate) fn into_diagnostic(self) -> (String, PathBuf) {
        (self.reason, self.path)
    }
}

fn naming_error(reason: impl Into<String>, path: PathBuf) -> SettingsNamingError {
    SettingsNamingError {
        reason: reason.into(),
        path,
    }
}

fn refusal_reason(refusal: crate::config::naming_migration::Refusal) -> String {
    match refusal {
        crate::config::naming_migration::Refusal::LockUnavailable => "lock unavailable".to_string(),
        crate::config::naming_migration::Refusal::Io(message) => message,
    }
}

/// #2714 - the pre-migration names of the two single files, in rename order.
pub(crate) const INSTANCE_FAMILIES_RENAMES: [crate::config::naming_migration::Rename; 2] = [
    crate::config::naming_migration::Rename {
        from: "settings.json",
        to: names::SETTINGS_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "settings.local.json",
        to: names::SETTINGS_LOCAL_OVERRIDE_FILE_NAME,
    },
];

/// #2714 - the four blocking-menus files, renamed after the settings families.
const BLOCKING_MENUS_RENAMES: [crate::config::naming_migration::Rename; 4] = [
    crate::config::naming_migration::Rename {
        from: "settings-blocking-menus.json",
        to: names::BLOCKING_MENUS_SHIPPED_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "settings-blocking-menus.local.json",
        to: names::BLOCKING_MENUS_LOCAL_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "settings-blocking-menus.remote.json",
        to: names::BLOCKING_MENUS_REMOTE_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "blocking-menus-remote-check.json",
        to: names::BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME,
    },
];

/// A directory that does not exist yet holds nothing to rename, and it is
/// not created here: returns the skip summary. Anything else that is not a
/// readable directory is fatal.
fn check_migration_directory(
    dir: &Path,
    scope: &str,
) -> Result<Option<String>, SettingsNamingError> {
    match naming_journal::migration_directory_metadata(dir) {
        Ok(metadata) if metadata.is_dir() => Ok(None),
        Ok(_) => Err(naming_error(
            "the configuration directory path is not a directory",
            dir.to_path_buf(),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Some(format!(
            "[naming-migration] {scope}: skipped, configuration directory {} does not exist",
            dir.display()
        ))),
        Err(e) => Err(naming_error(
            format!("failed to inspect the configuration directory: {e}"),
            dir.to_path_buf(),
        )),
    }
}

/// The ordered rename list of B1b section 3.5, under both settings locks: the
/// settings file, its personal overlay, the two derived families, then the four
/// blocking-menus files. Returns the post-logger summary, `None` when the scope
/// was already settled.
pub(crate) fn migrate_instance_settings_families(
    dir: &Path,
) -> Result<Option<String>, SettingsNamingError> {
    use crate::config::naming_migration as nm;

    let scope = INSTANCE_FAMILIES_SCOPE;
    let journal_path = dir.join(names::NAMING_MIGRATION_STATE_NAME);
    let read = || {
        nm::read_journal(Some(dir))
            .map_err(|refusal| naming_error(refusal_reason(refusal), journal_path.clone()))
    };

    if let Some(skipped) = check_migration_directory(dir, scope)? {
        return Ok(Some(skipped));
    }
    // `scope_is_settled`, never `is_complete`: a `Complete` the disk
    // contradicts is re-run.
    if nm::scope_is_settled(read()?.as_ref(), scope) {
        return Ok(None);
    }
    let held = match nm::lock_scope(
        dir,
        names::SETTINGS_LOCK_FILE_NAME,
        Some(names::SETTINGS_RETIRED_LOCK_NAME),
        nm::MIGRATION_LOCK_BUDGET,
    ) {
        Ok(held) => held,
        Err(nm::Refusal::LockUnavailable) => {
            // Another process may have completed the scope while we waited.
            if nm::scope_is_settled(read()?.as_ref(), scope) {
                return Ok(None);
            }
            return Err(naming_error(
                "the settings lock was still held after the five-second budget",
                dir.join(names::SETTINGS_LOCK_FILE_NAME),
            ));
        }
        Err(refusal) => {
            return Err(naming_error(
                refusal_reason(refusal),
                dir.join(names::SETTINGS_LOCK_FILE_NAME),
            ))
        }
    };
    // Re-check under the lock: a winner of the race leaves nothing to do.
    if nm::scope_is_settled(read()?.as_ref(), scope) {
        return Ok(None);
    }

    let journal_dir = Some(dir);
    let fail = |from: &str, outcome: nm::Outcome| match outcome {
        nm::Outcome::Refused(refusal) => Err(naming_error(refusal_reason(refusal), dir.join(from))),
        _ => Ok(()),
    };
    for rename in &INSTANCE_FAMILIES_RENAMES {
        fail(
            rename.from,
            nm::rename_step(dir, rename, scope, journal_dir, &held),
        )?;
    }
    let pre_glob = names::SETTINGS_MIGRATION_BACKUP_GLOB;
    let (pre_prefix, pre_suffix) = pre_glob.split_once('*').unwrap_or((pre_glob, ""));
    for (old_prefix, new_prefix, suffix) in [
        (
            "settings.backup.",
            names::SETTINGS_BACKUP_PREFIX,
            names::SETTINGS_BACKUP_SUFFIX,
        ),
        ("settings.pre-", pre_prefix, pre_suffix),
    ] {
        for (from, outcome) in nm::rename_prefix_family(
            dir,
            old_prefix,
            new_prefix,
            suffix,
            scope,
            journal_dir,
            &held,
        ) {
            fail(&from, outcome)?;
        }
    }
    for rename in &BLOCKING_MENUS_RENAMES {
        fail(
            rename.from,
            nm::rename_step(dir, rename, scope, journal_dir, &held),
        )?;
    }

    let record = complete_instance_families(dir, &journal_path)?;
    drop(held);

    Ok(Some(instance_families_summary(record)))
}

fn complete_instance_families(
    dir: &Path,
    journal_path: &Path,
) -> Result<Option<crate::config::naming_migration::ScopeRecord>, SettingsNamingError> {
    use crate::config::naming_migration as nm;

    let scope = INSTANCE_FAMILIES_SCOPE;
    let journal_dir = Some(dir);
    // A lock sidecar or a stale write temporary is never renamed or deleted,
    // only noted.
    let mut notes = Vec::new();
    if naming_journal::migration_sidecar_exists(&dir.join(names::SETTINGS_RETIRED_LOCK_NAME)) {
        notes.push(format!(
            "left on disk: {} (a lock sidecar is never renamed or deleted)",
            names::SETTINGS_RETIRED_LOCK_NAME
        ));
    }
    if let Ok(entries) = naming_journal::migration_entries(dir) {
        let mut stale: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with("settings.json.") && name.ends_with(".tmp"))
            .collect();
        stale.sort();
        notes.extend(stale.into_iter().map(|name| {
            format!("left on disk: {name} (a stale write temporary is never renamed or deleted)")
        }));
    }
    Ok(nm::update_journal(journal_dir, |journal| {
        for note in &notes {
            journal.note(scope, note);
        }
        journal.set_status(scope, nm::ScopeStatus::Complete);
        journal.scope(scope).cloned()
    })
    .map_err(|refusal| naming_error(refusal_reason(refusal), journal_path.to_path_buf()))?
    .flatten())
}

fn instance_families_summary(
    record: Option<crate::config::naming_migration::ScopeRecord>,
) -> String {
    use crate::config::naming_migration as nm;
    let scope = INSTANCE_FAMILIES_SCOPE;
    let steps = record.map(|record| record.steps).unwrap_or_default();
    let renamed = steps
        .iter()
        .filter(|step| step.state == nm::StepState::Renamed)
        .count();
    let set_aside: Vec<&str> = steps
        .iter()
        .filter_map(|step| step.set_aside.as_deref())
        .collect();
    let mut summary = format!("[naming-migration] {scope}: complete, {renamed} renamed");
    if !set_aside.is_empty() {
        summary.push_str(&format!(", set aside: {}", set_aside.join(", ")));
    }
    summary
}
