//! Startup preflight: config naming migration and the app outbox.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::config;

/// Instance-private outbox directory. Only this app instance polls it.
/// Created at startup, path printed to stdout alongside master token.
pub struct AppOutbox(String);

impl AppOutbox {
    pub fn new(path: String) -> Self {
        Self(path)
    }

    pub fn path(&self) -> &str {
        &self.0
    }
}

#[derive(Debug)]
pub struct StartupError {
    pub(crate) kind: StartupErrorKind,
}

#[derive(Debug)]
pub(crate) enum StartupErrorKind {
    Config(config::ConfigStartupError),
    AppOutboxCreate {
        config_dir: PathBuf,
        app_outbox_path: PathBuf,
        source: io::Error,
    },
    /// #2714 - a fatal outcome of the startup naming migration: an expired lock
    /// budget with no winner, an I/O error, or an unreadable journal.
    NamingMigration {
        scope: String,
        reason: String,
        path: PathBuf,
    },
}

impl fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            StartupErrorKind::Config(error) => error.fmt(formatter),
            StartupErrorKind::AppOutboxCreate {
                config_dir,
                app_outbox_path,
                source,
            } => write!(
                formatter,
                "AgentsCommander cannot start because it could not create app outbox directory \
                    \"{}\" for configuration directory \"{}\": {}. Set \
                    AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.",
                app_outbox_path.display(),
                config_dir.display(),
                source
            ),
            StartupErrorKind::NamingMigration {
                scope,
                reason,
                path,
            } => write!(
                formatter,
                "AgentsCommander cannot start because the file-naming migration \"{}\" could \
                    not finish at \"{}\": {}. If another AgentsCommander process is running, \
                    retry once it has finished; otherwise make the directory writable, or move \
                    an unreadable naming-migration journal aside, and restart.",
                scope,
                path.display(),
                reason
            ),
        }
    }
}

impl std::error::Error for StartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            StartupErrorKind::Config(error) => Some(error),
            StartupErrorKind::AppOutboxCreate { source, .. } => Some(source),
            StartupErrorKind::NamingMigration { .. } => None,
        }
    }
}

pub fn preflight_config_startup() -> Result<(), StartupError> {
    if let Some(error) = config::config_startup_error() {
        return Err(StartupError {
            kind: StartupErrorKind::Config(error),
        });
    }
    preflight_config_startup_for(config::config_dir().as_deref())
}

/// #2714 - the scope key of the one naming migration that runs at startup.
pub(crate) const INSTANCE_FAMILIES_SCOPE: &str = "instance-families";

/// The summary `run()` logs once `init_logger` has run: the migration runs
/// before the logger, so `log::*` from inside it goes nowhere.
static NAMING_MIGRATION_SUMMARY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub(crate) fn record_naming_migration_summary(summary: Option<String>) {
    if let Ok(mut slot) = NAMING_MIGRATION_SUMMARY.lock() {
        *slot = summary;
    }
}

pub(crate) fn take_naming_migration_summary() -> Option<String> {
    NAMING_MIGRATION_SUMMARY
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
}

/// Called by both entries right after `init_logger`.
pub fn log_naming_migration_summary() {
    if let Some(summary) = take_naming_migration_summary() {
        log::info!("{summary}");
    }
}

/// Decides the `None` case: with no configuration directory every path this
/// scope renames is absent, so the migration is skipped and startup continues.
pub(crate) fn preflight_config_startup_for(config_dir: Option<&Path>) -> Result<(), StartupError> {
    match config_dir {
        Some(config_dir) => preflight_config_startup_in(config_dir),
        None => {
            record_naming_migration_summary(Some(format!(
                "[naming-migration] {INSTANCE_FAMILIES_SCOPE}: skipped, no configuration directory"
            )));
            Ok(())
        }
    }
}

/// All of the startup migration work, against an explicit directory.
pub(crate) fn preflight_config_startup_in(config_dir: &Path) -> Result<(), StartupError> {
    let summary = migrate_instance_families(config_dir)?;
    record_naming_migration_summary(summary);
    Ok(())
}

pub(crate) fn naming_migration_error(reason: impl Into<String>, path: PathBuf) -> StartupError {
    StartupError {
        kind: StartupErrorKind::NamingMigration {
            scope: INSTANCE_FAMILIES_SCOPE.to_string(),
            reason: reason.into(),
            path,
        },
    }
}

pub(crate) fn refusal_reason(refusal: crate::config::naming_migration::Refusal) -> String {
    match refusal {
        crate::config::naming_migration::Refusal::LockUnavailable => "lock unavailable".to_string(),
        crate::config::naming_migration::Refusal::Io(message) => message,
    }
}

/// #2714 - the pre-migration names of the two single files, in rename order.
pub(crate) const INSTANCE_FAMILIES_RENAMES: [crate::config::naming_migration::Rename; 2] = [
    crate::config::naming_migration::Rename {
        from: "settings.json",
        to: config::instance_artifacts::SETTINGS_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "settings.local.json",
        to: config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME,
    },
];

/// #2714 - the four blocking-menus files, renamed after the settings families.
pub(crate) const BLOCKING_MENUS_RENAMES: [crate::config::naming_migration::Rename; 4] = [
    crate::config::naming_migration::Rename {
        from: "settings-blocking-menus.json",
        to: config::instance_artifacts::BLOCKING_MENUS_SHIPPED_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "settings-blocking-menus.local.json",
        to: config::instance_artifacts::BLOCKING_MENUS_LOCAL_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "settings-blocking-menus.remote.json",
        to: config::instance_artifacts::BLOCKING_MENUS_REMOTE_FILE_NAME,
    },
    crate::config::naming_migration::Rename {
        from: "blocking-menus-remote-check.json",
        to: config::instance_artifacts::BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME,
    },
];

/// The ordered rename list of B1b section 3.5, under both settings locks: the
/// settings file, its personal overlay, the two derived families, then the four
/// blocking-menus files. Returns the post-logger summary, `None` when the scope
/// was already settled.
pub(crate) fn migrate_instance_families(dir: &Path) -> Result<Option<String>, StartupError> {
    use crate::config::naming_migration as nm;
    use config::instance_artifacts as names;

    let scope = INSTANCE_FAMILIES_SCOPE;
    let journal_path = dir.join(names::NAMING_MIGRATION_STATE_NAME);
    let read = || {
        nm::read_journal(Some(dir)).map_err(|refusal| {
            naming_migration_error(refusal_reason(refusal), journal_path.clone())
        })
    };

    // A directory that does not exist yet holds nothing to rename, and it is
    // not created here. Anything else that is not a readable directory is fatal.
    match dir.metadata() {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(naming_migration_error(
                "the configuration directory path is not a directory",
                dir.to_path_buf(),
            ))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Some(format!(
                "[naming-migration] {scope}: skipped, configuration directory {} does not exist",
                dir.display()
            )))
        }
        Err(e) => {
            return Err(naming_migration_error(
                format!("failed to inspect the configuration directory: {e}"),
                dir.to_path_buf(),
            ))
        }
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
            return Err(naming_migration_error(
                "the settings lock was still held after the five-second budget",
                dir.join(names::SETTINGS_LOCK_FILE_NAME),
            ));
        }
        Err(refusal) => {
            return Err(naming_migration_error(
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
        nm::Outcome::Refused(refusal) => Err(naming_migration_error(
            refusal_reason(refusal),
            dir.join(from),
        )),
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

    // A lock sidecar or a stale write temporary is never renamed or deleted,
    // only noted.
    let mut notes = Vec::new();
    if dir.join(names::SETTINGS_RETIRED_LOCK_NAME).exists() {
        notes.push(format!(
            "left on disk: {} (a lock sidecar is never renamed or deleted)",
            names::SETTINGS_RETIRED_LOCK_NAME
        ));
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
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
    let record = nm::update_journal(journal_dir, |journal| {
        for note in &notes {
            journal.note(scope, note);
        }
        journal.set_status(scope, nm::ScopeStatus::Complete);
        journal.scope(scope).cloned()
    })
    .map_err(|refusal| naming_migration_error(refusal_reason(refusal), journal_path.clone()))?
    .flatten();
    drop(held);

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
    Ok(Some(summary))
}

pub(crate) fn prepare_app_outbox(
    config_dir: &Path,
    instance_id: &str,
) -> Result<(PathBuf, AppOutbox), StartupError> {
    let app_outbox_path = config_dir
        .join(crate::config::instance_artifacts::INSTANCES_DIR_NAME)
        .join(instance_id)
        .join("outbox");
    std::fs::create_dir_all(&app_outbox_path).map_err(|source| StartupError {
        kind: StartupErrorKind::AppOutboxCreate {
            config_dir: config_dir.to_path_buf(),
            app_outbox_path: app_outbox_path.clone(),
            source,
        },
    })?;
    let app_outbox = AppOutbox::new(app_outbox_path.to_string_lossy().to_string());
    Ok((app_outbox_path, app_outbox))
}
