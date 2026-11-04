//! Startup preflight: config naming migration and the app outbox.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::config;
use crate::filesystem::config_locations;

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

pub fn preflight_config_startup(owner: &StartupMigrationSummary) -> Result<(), StartupError> {
    if let Some(error) = config::config_startup_error() {
        return Err(StartupError {
            kind: StartupErrorKind::Config(error),
        });
    }
    preflight_config_startup_for(owner, config::config_dir().as_deref())
}

/// #2714 - the scope key of the one naming migration that runs at startup.
pub(crate) const INSTANCE_FAMILIES_SCOPE: &str = "instance-families";

/// The summary `run()` logs once `init_logger` has run: the migration runs
/// before the logger, so `log::*` from inside it goes nowhere.
pub struct StartupMigrationSummary {
    summary: std::sync::Mutex<Option<String>>,
}

impl StartupMigrationSummary {
    pub(crate) fn new() -> Self {
        Self {
            summary: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn record(&self, summary: Option<String>) {
        if let Ok(mut slot) = self.summary.lock() {
            *slot = summary;
        }
    }

    pub(crate) fn take(&self) -> Option<String> {
        self.summary.lock().ok().and_then(|mut slot| slot.take())
    }

    #[cfg(test)]
    pub(crate) fn poison_for_test(&self) {
        let _slot = self.summary.lock().unwrap();
        panic!("poison startup migration summary");
    }
}

pub(crate) fn record_naming_migration_summary(
    owner: &StartupMigrationSummary,
    summary: Option<String>,
) {
    owner.record(summary);
}

pub(crate) fn take_naming_migration_summary(owner: &StartupMigrationSummary) -> Option<String> {
    owner.take()
}

/// Called by both entries right after `init_logger`.
pub fn log_naming_migration_summary(owner: &StartupMigrationSummary) {
    if let Some(summary) = take_naming_migration_summary(owner) {
        log::info!("{summary}");
    }
}

/// Decides the `None` case: with no configuration directory every path this
/// scope renames is absent, so the migration is skipped and startup continues.
pub(crate) fn preflight_config_startup_for(
    owner: &StartupMigrationSummary,
    config_dir: Option<&Path>,
) -> Result<(), StartupError> {
    match config_dir {
        Some(config_dir) => preflight_config_startup_in(owner, config_dir),
        None => {
            record_naming_migration_summary(
                owner,
                Some(format!(
                "[naming-migration] {INSTANCE_FAMILIES_SCOPE}: skipped, no configuration directory"
            )),
            );
            Ok(())
        }
    }
}

/// All of the startup migration work, against an explicit directory.
pub(crate) fn preflight_config_startup_in(
    owner: &StartupMigrationSummary,
    config_dir: &Path,
) -> Result<(), StartupError> {
    let summary = migrate_instance_families(config_dir)?;
    record_naming_migration_summary(owner, summary);
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

pub(crate) fn migrate_instance_families(dir: &Path) -> Result<Option<String>, StartupError> {
    config::settings::migrate_instance_settings_families(dir).map_err(
        |error: config::settings::SettingsNamingError| {
            let (reason, path) = error.into_diagnostic();
            naming_migration_error(reason, path)
        },
    )
}

pub(crate) fn prepare_app_outbox(
    config_dir: &Path,
    instance_id: &str,
) -> Result<(PathBuf, AppOutbox), StartupError> {
    let app_outbox_path = config_dir
        .join(crate::config::instance_artifacts::INSTANCES_DIR_NAME)
        .join(instance_id)
        .join("outbox");
    config_locations::ensure_instance_outbox(&app_outbox_path).map_err(|source| StartupError {
        kind: StartupErrorKind::AppOutboxCreate {
            config_dir: config_dir.to_path_buf(),
            app_outbox_path: app_outbox_path.clone(),
            source,
        },
    })?;
    let app_outbox = AppOutbox::new(app_outbox_path.to_string_lossy().to_string());
    Ok((app_outbox_path, app_outbox))
}
