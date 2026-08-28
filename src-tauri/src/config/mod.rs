pub mod ac_root;
pub mod activity_log;
pub mod agent_command;
pub mod agent_config;
pub mod agent_creation;
pub(crate) mod agent_memory;
pub mod archive_gate;
pub mod coding_agent_mutations;
pub mod coding_agent_profiles;
pub mod coding_agents_catalog;
pub mod config_seed;
pub mod coordinator_clocks;
pub mod daemon_pid;
pub mod injected_messages;
pub(crate) mod instance_artifacts;
pub(crate) mod instance_gitignore;
pub mod local_config_io;
pub mod loops;
pub mod placeholders;
pub mod profile;
pub mod project_settings;
pub mod projects;
pub mod replica_identity;
pub mod root_agent;
pub mod seed_manifest;
pub mod seeded_context_templates;
pub mod session_context;
pub mod sessions_persistence;
pub mod settings;
pub mod teams;

use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// #1273: the Root Agent instance directory name.
///
/// It lives here because `config::instance_gitignore` needs it to write one
/// `.gitignore` rule and `config::root_agent` needs it to build the directory,
/// and `config` is below both. This module already owns the instance-layout
/// facts of the same family: `agent_local_dir_name()`, `config_dir()` and
/// `instance_base()`. `config::root_agent` re-exports it, so every existing
/// reader of `crate::config::root_agent::ROOT_AGENT_DIR_NAME` keeps resolving.
pub const ROOT_AGENT_DIR_NAME: &str = "ac-root-agent";

pub(crate) const TRANSIENT_IO_WINDOWS_BACKOFFS_MS: [u64; 5] = [15, 30, 60, 120, 240];

#[derive(Debug)]
pub(crate) struct RetriedIoError {
    pub(crate) error: io::Error,
    pub(crate) attempts: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryPlatform {
    Windows,
    Other,
}

impl RetryPlatform {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

fn retry_transient_io_with_platform<T, Operation, Sleep, Retrying>(
    platform: RetryPlatform,
    mut operation: Operation,
    mut sleep: Sleep,
    mut retrying: Retrying,
) -> Result<T, RetriedIoError>
where
    Operation: FnMut() -> io::Result<T>,
    Sleep: FnMut(Duration),
    Retrying: FnMut(usize, usize, &io::Error, Option<Duration>),
{
    let mut attempts = 0;
    let mut interrupted_retry_available = true;
    let mut windows_backoff_index = 0;

    loop {
        attempts += 1;
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                if error.kind() == io::ErrorKind::Interrupted && interrupted_retry_available {
                    interrupted_retry_available = false;
                    let maximum_attempts = attempts
                        + (TRANSIENT_IO_WINDOWS_BACKOFFS_MS.len() - windows_backoff_index)
                        + 1;
                    retrying(attempts, maximum_attempts, &error, None);
                    continue;
                }

                if platform == RetryPlatform::Windows
                    && matches!(error.raw_os_error(), Some(5) | Some(32))
                    && windows_backoff_index < TRANSIENT_IO_WINDOWS_BACKOFFS_MS.len()
                {
                    let delay = Duration::from_millis(
                        TRANSIENT_IO_WINDOWS_BACKOFFS_MS[windows_backoff_index],
                    );
                    windows_backoff_index += 1;
                    let maximum_attempts = attempts
                        + (TRANSIENT_IO_WINDOWS_BACKOFFS_MS.len() - windows_backoff_index)
                        + 1;
                    retrying(attempts, maximum_attempts, &error, Some(delay));
                    sleep(delay);
                    continue;
                }

                return Err(RetriedIoError { error, attempts });
            }
        }
    }
}

pub(crate) fn retry_transient_io_with<T, Operation, Sleep, Retrying>(
    operation: Operation,
    sleep: Sleep,
    retrying: Retrying,
) -> Result<T, RetriedIoError>
where
    Operation: FnMut() -> io::Result<T>,
    Sleep: FnMut(Duration),
    Retrying: FnMut(usize, usize, &io::Error, Option<Duration>),
{
    retry_transient_io_with_platform(RetryPlatform::current(), operation, sleep, retrying)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeFailureClass {
    ConclusiveUnwritable,
    Indeterminate,
}

fn classify_io_error_for_platform(platform: RetryPlatform, error: &io::Error) -> ProbeFailureClass {
    if matches!(
        error.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
    ) || (platform == RetryPlatform::Windows
        && matches!(error.raw_os_error(), Some(5) | Some(32)))
    {
        ProbeFailureClass::ConclusiveUnwritable
    } else {
        ProbeFailureClass::Indeterminate
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeOperation {
    MarkerEntryMetadata,
    MarkerTargetMetadata,
    UnsupportedMarkerEntry,
    CreateConfigurationDirectory,
    CreateProbeFile,
    WriteProbeFile,
    DeleteProbeFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProbeFailure {
    pub(crate) operation: ProbeOperation,
    pub(crate) affected_path: PathBuf,
    pub(crate) attempts: usize,
    pub(crate) kind: Option<io::ErrorKind>,
    pub(crate) raw_os_error: Option<i32>,
    pub(crate) os_reason: String,
    pub(crate) class: ProbeFailureClass,
}

impl ProbeFailure {
    fn from_retry(
        platform: RetryPlatform,
        operation: ProbeOperation,
        affected_path: PathBuf,
        failure: RetriedIoError,
    ) -> Self {
        Self {
            operation,
            affected_path,
            attempts: failure.attempts,
            kind: Some(failure.error.kind()),
            raw_os_error: failure.error.raw_os_error(),
            os_reason: failure.error.to_string(),
            class: classify_io_error_for_platform(platform, &failure.error),
        }
    }

    fn unsupported_marker(affected_path: PathBuf) -> Self {
        Self {
            operation: ProbeOperation::UnsupportedMarkerEntry,
            affected_path,
            attempts: 1,
            kind: None,
            raw_os_error: None,
            os_reason: "filesystem metadata reported an unsupported portable marker entry type"
                .to_string(),
            class: ProbeFailureClass::Indeterminate,
        }
    }

    pub(crate) fn reason(&self) -> String {
        match self.operation {
            ProbeOperation::MarkerEntryMetadata => format!(
                "could not inspect portable marker entry metadata \"{}\" after {} attempt(s): {}",
                self.affected_path.display(),
                self.attempts,
                self.os_reason
            ),
            ProbeOperation::MarkerTargetMetadata => format!(
                "could not resolve portable marker symlink target metadata \"{}\" after {} attempt(s): {}",
                self.affected_path.display(),
                self.attempts,
                self.os_reason
            ),
            ProbeOperation::UnsupportedMarkerEntry => self.os_reason.clone(),
            ProbeOperation::CreateConfigurationDirectory => format!(
                "write probe could not create configuration directory \"{}\" after {} attempt(s): {}",
                self.affected_path.display(),
                self.attempts,
                self.os_reason
            ),
            ProbeOperation::CreateProbeFile => format!(
                "write probe could not create probe file \"{}\" after {} attempt(s): {}",
                self.affected_path.display(),
                self.attempts,
                self.os_reason
            ),
            ProbeOperation::WriteProbeFile => format!(
                "write probe could not write probe file \"{}\" after {} attempt(s): {}",
                self.affected_path.display(),
                self.attempts,
                self.os_reason
            ),
            ProbeOperation::DeleteProbeFile => format!(
                "write probe could not delete probe file \"{}\" after {} attempt(s): {}",
                self.affected_path.display(),
                self.attempts,
                self.os_reason
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarkerProbeOutcome {
    NotRun,
    Absent,
    Present,
    Indeterminate(ProbeFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteProbeFailure {
    pub(crate) primary: ProbeFailure,
    pub(crate) cleanup: Option<ProbeFailure>,
    pub(crate) probe_path: Option<PathBuf>,
    pub(crate) probe_may_remain: bool,
    pub(crate) class: ProbeFailureClass,
}

impl WriteProbeFailure {
    fn new(
        primary: ProbeFailure,
        cleanup: Option<ProbeFailure>,
        probe_path: Option<PathBuf>,
        probe_may_remain: bool,
    ) -> Self {
        let class = if primary.class == ProbeFailureClass::ConclusiveUnwritable
            && cleanup
                .as_ref()
                .is_none_or(|failure| failure.class == ProbeFailureClass::ConclusiveUnwritable)
        {
            ProbeFailureClass::ConclusiveUnwritable
        } else {
            ProbeFailureClass::Indeterminate
        };
        Self {
            primary,
            cleanup,
            probe_path,
            probe_may_remain,
            class,
        }
    }

    pub(crate) fn reason(&self) -> String {
        let mut reason = self.primary.reason();
        if let Some(cleanup) = &self.cleanup {
            let probe_path = self
                .probe_path
                .as_deref()
                .unwrap_or(cleanup.affected_path.as_path());
            reason.push_str(&format!(
                ". Cleanup of probe file \"{}\" also failed after {} attempt(s): {}; the probe file may remain.",
                probe_path.display(),
                cleanup.attempts,
                cleanup.os_reason
            ));
        } else if self.probe_may_remain && self.primary.operation == ProbeOperation::DeleteProbeFile
        {
            if let Some(probe_path) = &self.probe_path {
                reason.push_str(&format!(
                    ". The probe file \"{}\" may remain.",
                    probe_path.display()
                ));
            }
        }
        reason
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WriteProbeOutcome {
    NotRun,
    Success,
    Failed(WriteProbeFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdjacentFallbackDiagnostic {
    pub(crate) candidate: PathBuf,
    pub(crate) selected_home: Option<PathBuf>,
    pub(crate) failure: WriteProbeFailure,
}

impl fmt::Display for AdjacentFallbackDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.selected_home {
            Some(selected_home) => write!(
                formatter,
                "configuration directory \"{}\" was conclusively unwritable; using \"{}\" instead: {}",
                self.candidate.display(),
                selected_home.display(),
                self.failure.reason()
            ),
            None => write!(
                formatter,
                "configuration directory \"{}\" was conclusively unwritable and no home directory was available: {}",
                self.candidate.display(),
                self.failure.reason()
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigStartupError {
    AdjacentSelectionBlocked {
        config_dir: PathBuf,
        marker_path: Option<PathBuf>,
        reason: String,
    },
}

impl fmt::Display for ConfigStartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AdjacentSelectionBlocked {
                config_dir,
                marker_path,
                reason,
            } => {
                write!(
                    formatter,
                    "AgentsCommander cannot start because configuration directory \"{}\" could not be safely selected: {}",
                    config_dir.display(),
                    reason
                )?;
                if !reason.ends_with('.') {
                    write!(formatter, ".")?;
                }
                write!(
                    formatter,
                    " Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart."
                )?;
                if let Some(marker_path) = marker_path {
                    write!(
                        formatter,
                        " Portable marker path: \"{}\".",
                        marker_path.display()
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ConfigStartupError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerEntryKind {
    File,
    Directory,
    Symlink,
    Unsupported,
}

fn marker_entry_kind(metadata: &Metadata) -> MarkerEntryKind {
    let file_type = metadata.file_type();
    if file_type.is_file() {
        MarkerEntryKind::File
    } else if file_type.is_dir() {
        MarkerEntryKind::Directory
    } else if file_type.is_symlink() {
        MarkerEntryKind::Symlink
    } else {
        MarkerEntryKind::Unsupported
    }
}

fn probe_portable_marker_with<EntryMetadata, TargetMetadata, Sleep>(
    platform: RetryPlatform,
    marker_path: &Path,
    mut entry_metadata: EntryMetadata,
    mut target_metadata: TargetMetadata,
    mut sleep: Sleep,
) -> MarkerProbeOutcome
where
    EntryMetadata: FnMut(&Path) -> io::Result<MarkerEntryKind>,
    TargetMetadata: FnMut(&Path) -> io::Result<MarkerEntryKind>,
    Sleep: FnMut(Duration),
{
    let entry = retry_transient_io_with_platform(
        platform,
        || entry_metadata(marker_path),
        &mut sleep,
        |_, _, _, _| {},
    );
    let entry = match entry {
        Ok(entry) => entry,
        Err(failure) if failure.error.kind() == io::ErrorKind::NotFound => {
            return MarkerProbeOutcome::Absent;
        }
        Err(failure) => {
            return MarkerProbeOutcome::Indeterminate(ProbeFailure::from_retry(
                platform,
                ProbeOperation::MarkerEntryMetadata,
                marker_path.to_path_buf(),
                failure,
            ));
        }
    };

    match entry {
        MarkerEntryKind::File | MarkerEntryKind::Directory => MarkerProbeOutcome::Present,
        MarkerEntryKind::Unsupported => MarkerProbeOutcome::Indeterminate(
            ProbeFailure::unsupported_marker(marker_path.to_path_buf()),
        ),
        MarkerEntryKind::Symlink => match retry_transient_io_with_platform(
            platform,
            || target_metadata(marker_path),
            &mut sleep,
            |_, _, _, _| {},
        ) {
            Ok(MarkerEntryKind::File | MarkerEntryKind::Directory) => MarkerProbeOutcome::Present,
            Ok(MarkerEntryKind::Symlink | MarkerEntryKind::Unsupported) => {
                MarkerProbeOutcome::Indeterminate(ProbeFailure::unsupported_marker(
                    marker_path.to_path_buf(),
                ))
            }
            Err(failure) => MarkerProbeOutcome::Indeterminate(ProbeFailure::from_retry(
                platform,
                ProbeOperation::MarkerTargetMetadata,
                marker_path.to_path_buf(),
                failure,
            )),
        },
    }
}

fn probe_portable_marker(marker_path: &Path) -> MarkerProbeOutcome {
    probe_portable_marker_with(
        RetryPlatform::current(),
        marker_path,
        |path| fs::symlink_metadata(path).map(|metadata| marker_entry_kind(&metadata)),
        |path| fs::metadata(path).map(|metadata| marker_entry_kind(&metadata)),
        std::thread::sleep,
    )
}

#[allow(clippy::too_many_arguments)]
fn probe_candidate_write_with<Handle, CreateDirectory, CreateFile, WriteFile, RemoveFile, Sleep>(
    platform: RetryPlatform,
    candidate: &Path,
    probe_path: &Path,
    mut create_directory: CreateDirectory,
    mut create_file: CreateFile,
    mut write_file: WriteFile,
    mut remove_file: RemoveFile,
    mut sleep: Sleep,
) -> WriteProbeOutcome
where
    CreateDirectory: FnMut(&Path) -> io::Result<()>,
    CreateFile: FnMut(&Path) -> io::Result<Handle>,
    WriteFile: FnMut(&mut Handle) -> io::Result<()>,
    RemoveFile: FnMut(&Path) -> io::Result<()>,
    Sleep: FnMut(Duration),
{
    if let Err(failure) = retry_transient_io_with_platform(
        platform,
        || create_directory(candidate),
        &mut sleep,
        |_, _, _, _| {},
    ) {
        let primary = ProbeFailure::from_retry(
            platform,
            ProbeOperation::CreateConfigurationDirectory,
            candidate.to_path_buf(),
            failure,
        );
        return WriteProbeOutcome::Failed(WriteProbeFailure::new(primary, None, None, false));
    }

    let mut handle = match retry_transient_io_with_platform(
        platform,
        || create_file(probe_path),
        &mut sleep,
        |_, _, _, _| {},
    ) {
        Ok(handle) => handle,
        Err(failure) => {
            let primary = ProbeFailure::from_retry(
                platform,
                ProbeOperation::CreateProbeFile,
                probe_path.to_path_buf(),
                failure,
            );
            return WriteProbeOutcome::Failed(WriteProbeFailure::new(
                primary,
                None,
                Some(probe_path.to_path_buf()),
                false,
            ));
        }
    };

    let write_result = retry_transient_io_with_platform(
        platform,
        || write_file(&mut handle),
        &mut sleep,
        |_, _, _, _| {},
    );
    drop(handle);

    if let Err(failure) = write_result {
        let primary = ProbeFailure::from_retry(
            platform,
            ProbeOperation::WriteProbeFile,
            probe_path.to_path_buf(),
            failure,
        );
        let cleanup = match retry_transient_io_with_platform(
            platform,
            || remove_file(probe_path),
            &mut sleep,
            |_, _, _, _| {},
        ) {
            Ok(()) => None,
            Err(failure) if failure.error.kind() == io::ErrorKind::NotFound => None,
            Err(failure) => Some(ProbeFailure::from_retry(
                platform,
                ProbeOperation::DeleteProbeFile,
                probe_path.to_path_buf(),
                failure,
            )),
        };
        let probe_may_remain = cleanup.is_some();
        return WriteProbeOutcome::Failed(WriteProbeFailure::new(
            primary,
            cleanup,
            Some(probe_path.to_path_buf()),
            probe_may_remain,
        ));
    }

    match retry_transient_io_with_platform(
        platform,
        || remove_file(probe_path),
        &mut sleep,
        |_, _, _, _| {},
    ) {
        Ok(()) => WriteProbeOutcome::Success,
        Err(failure) => {
            let primary = ProbeFailure::from_retry(
                platform,
                ProbeOperation::DeleteProbeFile,
                probe_path.to_path_buf(),
                failure,
            );
            WriteProbeOutcome::Failed(WriteProbeFailure::new(
                primary,
                None,
                Some(probe_path.to_path_buf()),
                true,
            ))
        }
    }
}

fn probe_candidate_write(candidate: &Path) -> WriteProbeOutcome {
    let probe_path = candidate.join(format!(
        ".agentscommander-write-probe-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    probe_candidate_write_with(
        RetryPlatform::current(),
        candidate,
        &probe_path,
        |path| fs::create_dir_all(path),
        |path| OpenOptions::new().write(true).create_new(true).open(path),
        |file: &mut File| file.write_all(b"AgentsCommander write probe\n"),
        |path| fs::remove_file(path),
        std::thread::sleep,
    )
}

/// #1077: authoritative, once-resolved location of the running AgentsCommander
/// instance. Centralizes the executable-derived facts that `config_dir()`,
/// `agent_local_dir_name()`, and the portable project-path codec must all agree
/// on, so none of them independently re-calls `current_exe()` and drifts.
///
/// Built once by [`instance_location`] from `resolve_instance_location`, the
/// pure helper that carries every branch. Tests call the pure helper directly
/// with injected inputs and never mutate `AGENTSCOMMANDER_TEST_CONFIG_DIR` or
/// depend on the test-runner executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstanceLocation {
    /// The app config directory. Honors the debug `AGENTSCOMMANDER_TEST_CONFIG_DIR`
    /// override verbatim, then the portable `<exe-parent>/.<exe-stem>` form, then
    /// the `$HOME/<profile::config_dir_name()>` fallback. `None` only when every
    /// source is unavailable (no override, no usable `current_exe()` parent/stem,
    /// and no home directory).
    pub config_dir: Option<PathBuf>,
    /// Local agent directory stem derived from the running executable
    /// (`current_exe().file_stem()`), falling back to `"agentscommander"`. This
    /// is NEVER derived from the debug override directory: `agent_local_dir_name()`
    /// returns `format!(".{stem}")`.
    pub local_dir_stem: String,
    /// Optional ABSOLUTE instance base directory used to pair portable,
    /// instance-relative project paths. `Some` only for a packaged/absolute
    /// executable directory (or an absolute debug override's parent). `None` in
    /// every degraded mode: a relative executable, a relative override, the home
    /// fallback, or an unavailable `current_exe()`. Stored raw and
    /// uncanonicalized; the project codec canonicalizes it at its own boundary
    /// and never consults the process CWD.
    pub instance_base: Option<PathBuf>,
    startup_error: Option<ConfigStartupError>,
    fallback_diagnostic: Option<AdjacentFallbackDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdjacentPaths {
    config_dir: PathBuf,
    marker_path: PathBuf,
    instance_base: Option<PathBuf>,
}

fn adjacent_paths(executable: &Path) -> Option<AdjacentPaths> {
    let parent = executable.parent()?;
    let stem = executable.file_stem()?;
    Some(AdjacentPaths {
        config_dir: parent.join(format!(".{}", stem.to_string_lossy())),
        marker_path: parent.join("portable.txt"),
        instance_base: parent.is_absolute().then(|| parent.to_path_buf()),
    })
}

fn nonblank_override(value: Option<&String>) -> Option<&str> {
    value
        .map(String::as_str)
        .filter(|raw| !raw.trim().is_empty())
}

fn override_location(raw: &str, local_dir_stem: String) -> InstanceLocation {
    let override_path = PathBuf::from(raw);
    let instance_base = if override_path.is_absolute() {
        override_path.parent().map(Path::to_path_buf)
    } else {
        None
    };
    InstanceLocation {
        config_dir: Some(override_path),
        local_dir_stem,
        instance_base,
        startup_error: None,
        fallback_diagnostic: None,
    }
}

fn blocked_adjacent_location(
    paths: AdjacentPaths,
    local_dir_stem: String,
    marker_path: Option<PathBuf>,
    reason: String,
) -> InstanceLocation {
    let startup_error = ConfigStartupError::AdjacentSelectionBlocked {
        config_dir: paths.config_dir.clone(),
        marker_path,
        reason,
    };
    InstanceLocation {
        config_dir: Some(paths.config_dir),
        local_dir_stem,
        instance_base: paths.instance_base,
        startup_error: Some(startup_error),
        fallback_diagnostic: None,
    }
}

/// Pure resolver for [`InstanceLocation`]. All branching for the #1077 instance
/// base lives here so it can be unit-tested with injected inputs.
///
/// - `test_override`: the debug `AGENTSCOMMANDER_TEST_CONFIG_DIR` value when set
///   (only threaded through in debug builds). An absolute override selects the
///   config directory verbatim and its parent becomes the instance base; a
///   relative override selects the config directory verbatim but reports NO
///   portable base (never absolutized through CWD).
/// - `current_exe_result`: the outcome of `std::env::current_exe()`.
/// - `home_dir`: `dirs::home_dir()` for the legacy fallback.
pub(crate) fn resolve_instance_location(
    public_override: Option<String>,
    test_override: Option<String>,
    current_exe_result: Result<PathBuf, std::io::Error>,
    home_dir: Option<PathBuf>,
    fallback_config_dir_name: &str,
    marker_probe: MarkerProbeOutcome,
    write_probe: WriteProbeOutcome,
) -> InstanceLocation {
    // Local agent dir stem: from the running executable only. Independent of the
    // debug override so `agent_local_dir_name()` keeps naming replica dirs after
    // the real binary, and falls back to "agentscommander" when unavailable.
    let local_dir_stem = current_exe_result
        .as_ref()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "agentscommander".to_string());

    if let Some(raw) = nonblank_override(public_override.as_ref()) {
        return override_location(raw, local_dir_stem);
    }

    if let Some(raw) = nonblank_override(test_override.as_ref()) {
        return override_location(raw, local_dir_stem);
    }

    let Some(paths) = current_exe_result
        .as_ref()
        .ok()
        .and_then(|path| adjacent_paths(path))
    else {
        return InstanceLocation {
            config_dir: home_dir.map(|home| home.join(fallback_config_dir_name)),
            local_dir_stem,
            instance_base: None,
            startup_error: None,
            fallback_diagnostic: None,
        };
    };

    match marker_probe {
        MarkerProbeOutcome::Indeterminate(failure) => blocked_adjacent_location(
            paths.clone(),
            local_dir_stem,
            Some(paths.marker_path.clone()),
            failure.reason(),
        ),
        MarkerProbeOutcome::Present => match write_probe {
            WriteProbeOutcome::Success => InstanceLocation {
                config_dir: Some(paths.config_dir),
                local_dir_stem,
                instance_base: paths.instance_base,
                startup_error: None,
                fallback_diagnostic: None,
            },
            WriteProbeOutcome::Failed(failure) => blocked_adjacent_location(
                paths.clone(),
                local_dir_stem,
                Some(paths.marker_path.clone()),
                failure.reason(),
            ),
            WriteProbeOutcome::NotRun => blocked_adjacent_location(
                paths.clone(),
                local_dir_stem,
                Some(paths.marker_path.clone()),
                "write probe was not run for a portable configuration directory".to_string(),
            ),
        },
        MarkerProbeOutcome::Absent => match write_probe {
            WriteProbeOutcome::Success => InstanceLocation {
                config_dir: Some(paths.config_dir),
                local_dir_stem,
                instance_base: paths.instance_base,
                startup_error: None,
                fallback_diagnostic: None,
            },
            WriteProbeOutcome::Failed(failure)
                if failure.class == ProbeFailureClass::ConclusiveUnwritable =>
            {
                let selected_home = home_dir.map(|home| home.join(fallback_config_dir_name));
                let fallback_diagnostic = AdjacentFallbackDiagnostic {
                    candidate: paths.config_dir,
                    selected_home: selected_home.clone(),
                    failure,
                };
                InstanceLocation {
                    config_dir: selected_home,
                    local_dir_stem,
                    instance_base: None,
                    startup_error: None,
                    fallback_diagnostic: Some(fallback_diagnostic),
                }
            }
            WriteProbeOutcome::Failed(failure) => {
                blocked_adjacent_location(paths, local_dir_stem, None, failure.reason())
            }
            WriteProbeOutcome::NotRun => blocked_adjacent_location(
                paths,
                local_dir_stem,
                None,
                "write probe was not run for an unmarked configuration directory".to_string(),
            ),
        },
        MarkerProbeOutcome::NotRun => blocked_adjacent_location(
            paths,
            local_dir_stem,
            None,
            "portable marker probe was not run for an executable-adjacent configuration directory"
                .to_string(),
        ),
    }
}

/// Cached, process-wide [`InstanceLocation`], resolved once at first call.
fn instance_location() -> &'static InstanceLocation {
    static LOC: OnceLock<InstanceLocation> = OnceLock::new();
    LOC.get_or_init(|| {
        let public_override = std::env::var("AGENTSCOMMANDER_CONFIG_DIR").ok();
        // The test override is a debug-only affordance; release builds never read it.
        #[cfg(debug_assertions)]
        let test_override = std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").ok();
        #[cfg(not(debug_assertions))]
        let test_override: Option<String> = None;
        let current_exe_result = std::env::current_exe();
        let home_dir = dirs::home_dir();
        let fallback_config_dir_name = profile::config_dir_name_for_executable(
            current_exe_result.as_ref().ok().map(PathBuf::as_path),
        );

        let override_selected = nonblank_override(public_override.as_ref()).is_some()
            || nonblank_override(test_override.as_ref()).is_some();
        let adjacent = current_exe_result
            .as_ref()
            .ok()
            .and_then(|path| adjacent_paths(path));
        let (marker_probe, write_probe) = match (override_selected, adjacent) {
            (true, _) | (_, None) => (MarkerProbeOutcome::NotRun, WriteProbeOutcome::NotRun),
            (false, Some(adjacent)) => {
                let marker_probe = probe_portable_marker(&adjacent.marker_path);
                let write_probe = match marker_probe {
                    MarkerProbeOutcome::Present | MarkerProbeOutcome::Absent => {
                        probe_candidate_write(&adjacent.config_dir)
                    }
                    MarkerProbeOutcome::NotRun | MarkerProbeOutcome::Indeterminate(_) => {
                        WriteProbeOutcome::NotRun
                    }
                };
                (marker_probe, write_probe)
            }
        };

        resolve_instance_location(
            public_override,
            test_override,
            current_exe_result,
            home_dir,
            fallback_config_dir_name,
            marker_probe,
            write_probe,
        )
    })
}

/// Returns the local agent directory name derived from the current binary name.
/// E.g., "agentscommander-stage.exe" → ".agentscommander-stage"
/// E.g., "agentscommander.exe" → ".agentscommander"
///
/// Projects from the cached [`InstanceLocation`]; the stem comes from the running
/// executable, never from the debug config-dir override, and falls back to
/// "agentscommander" when `current_exe()` is unavailable.
pub fn agent_local_dir_name() -> String {
    format!(".{}", instance_location().local_dir_stem)
}

/// Returns the app config directory — portable, next to the binary.
/// Pattern: `<binary_parent_dir>/.<binary_file_stem>/`
/// E.g., `C:\tools\agentscommander_standalone.exe` → `C:\tools\.agentscommander_standalone\`
/// Fallback: `$HOME/<profile::config_dir_name()>` if current_exe() fails.
/// Cached via the shared [`InstanceLocation`] — resolved once at first call.
pub fn config_dir() -> Option<PathBuf> {
    instance_location().config_dir.clone()
}

/// #1077: the authoritative ABSOLUTE instance base for portable project-path
/// pairing, or `None` in any degraded mode. Consumed by the project codec, which
/// canonicalizes it at its own boundary. Returns raw, uncanonicalized bytes.
pub(crate) fn instance_base() -> Option<PathBuf> {
    instance_location().instance_base.clone()
}

pub(crate) fn config_startup_error() -> Option<ConfigStartupError> {
    instance_location().startup_error.clone()
}

pub(crate) fn adjacent_fallback_diagnostic() -> Option<AdjacentFallbackDiagnostic> {
    instance_location().fallback_diagnostic.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{Error, ErrorKind};
    use std::path::PathBuf;

    fn exe_err() -> Result<PathBuf, Error> {
        Err(Error::new(ErrorKind::NotFound, "no current_exe"))
    }

    fn absolute_executable() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\bundle\agentscommander.exe")
        } else {
            PathBuf::from("/opt/bundle/agentscommander")
        }
    }

    fn expected_adjacent() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\bundle\.agentscommander")
        } else {
            PathBuf::from("/opt/bundle/.agentscommander")
        }
    }

    fn expected_marker() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\bundle\portable.txt")
        } else {
            PathBuf::from("/opt/bundle/portable.txt")
        }
    }

    fn failed_write(
        platform: RetryPlatform,
        operation: ProbeOperation,
        path: PathBuf,
        error: io::Error,
        attempts: usize,
    ) -> WriteProbeFailure {
        let primary = ProbeFailure::from_retry(
            platform,
            operation,
            path.clone(),
            RetriedIoError { error, attempts },
        );
        WriteProbeFailure::new(primary, None, Some(path), false)
    }

    #[test]
    fn packaged_absolute_executable_yields_portable_config_and_base() {
        let exe = if cfg!(windows) {
            PathBuf::from(r"C:\bundle\agentscommander.exe")
        } else {
            PathBuf::from("/opt/bundle/agentscommander")
        };
        let loc = resolve_instance_location(
            None,
            None,
            Ok(exe),
            Some(PathBuf::from("/home/u")),
            profile::config_dir_name(),
            MarkerProbeOutcome::Absent,
            WriteProbeOutcome::Success,
        );
        let expected_config = if cfg!(windows) {
            PathBuf::from(r"C:\bundle\.agentscommander")
        } else {
            PathBuf::from("/opt/bundle/.agentscommander")
        };
        let expected_base = if cfg!(windows) {
            PathBuf::from(r"C:\bundle")
        } else {
            PathBuf::from("/opt/bundle")
        };
        assert_eq!(loc.config_dir.as_deref(), Some(expected_config.as_path()));
        assert_eq!(loc.instance_base.as_deref(), Some(expected_base.as_path()));
        assert_eq!(loc.local_dir_stem, "agentscommander");
    }

    #[test]
    fn renamed_executable_keeps_per_stem_config_and_base() {
        let exe = if cfg!(windows) {
            PathBuf::from(r"C:\tools\agentscommander_standalone.exe")
        } else {
            PathBuf::from("/tools/agentscommander_standalone")
        };
        let loc = resolve_instance_location(
            None,
            None,
            Ok(exe),
            Some(PathBuf::from("/home/u")),
            profile::config_dir_name(),
            MarkerProbeOutcome::Absent,
            WriteProbeOutcome::Success,
        );
        let expected_config = if cfg!(windows) {
            PathBuf::from(r"C:\tools\.agentscommander_standalone")
        } else {
            PathBuf::from("/tools/.agentscommander_standalone")
        };
        assert_eq!(loc.config_dir.as_deref(), Some(expected_config.as_path()));
        assert_eq!(loc.local_dir_stem, "agentscommander_standalone");
        assert!(loc.instance_base.is_some());
    }

    #[test]
    fn absolute_debug_override_sets_config_verbatim_and_parent_base() {
        let override_dir = if cfg!(windows) {
            r"C:\bundle\.agentscommander"
        } else {
            "/opt/bundle/.agentscommander"
        };
        let exe = if cfg!(windows) {
            PathBuf::from(r"C:\somewhere\else\ac.exe")
        } else {
            PathBuf::from("/somewhere/else/ac")
        };
        let loc = resolve_instance_location(
            None,
            Some(override_dir.to_string()),
            Ok(exe),
            Some(PathBuf::from("/home/u")),
            profile::config_dir_name(),
            MarkerProbeOutcome::NotRun,
            WriteProbeOutcome::NotRun,
        );
        assert_eq!(loc.config_dir.as_deref(), Some(Path::new(override_dir)));
        let expected_base = if cfg!(windows) {
            PathBuf::from(r"C:\bundle")
        } else {
            PathBuf::from("/opt/bundle")
        };
        assert_eq!(loc.instance_base.as_deref(), Some(expected_base.as_path()));
        // The local dir stem still comes from the executable, not the override.
        assert_eq!(loc.local_dir_stem, "ac");
    }

    #[test]
    fn relative_debug_override_sets_config_but_no_base() {
        let loc = resolve_instance_location(
            None,
            Some("relative/.acdir".to_string()),
            exe_err(),
            Some(PathBuf::from("/home/u")),
            profile::config_dir_name(),
            MarkerProbeOutcome::NotRun,
            WriteProbeOutcome::NotRun,
        );
        assert_eq!(
            loc.config_dir.as_deref(),
            Some(Path::new("relative/.acdir"))
        );
        assert_eq!(loc.instance_base, None);
        assert_eq!(loc.local_dir_stem, "agentscommander");
    }

    #[test]
    fn blank_debug_override_is_ignored() {
        let exe = if cfg!(windows) {
            PathBuf::from(r"C:\bundle\ac.exe")
        } else {
            PathBuf::from("/opt/bundle/ac")
        };
        let loc = resolve_instance_location(
            None,
            Some("   ".to_string()),
            Ok(exe),
            Some(PathBuf::from("/home/u")),
            profile::config_dir_name(),
            MarkerProbeOutcome::Absent,
            WriteProbeOutcome::Success,
        );
        // Falls through to the portable executable-derived config.
        let expected_config = if cfg!(windows) {
            PathBuf::from(r"C:\bundle\.ac")
        } else {
            PathBuf::from("/opt/bundle/.ac")
        };
        assert_eq!(loc.config_dir.as_deref(), Some(expected_config.as_path()));
        assert!(loc.instance_base.is_some());
    }

    #[test]
    fn relative_executable_keeps_relative_config_but_no_base() {
        let loc = resolve_instance_location(
            None,
            None,
            Ok(PathBuf::from("bin/agentscommander")),
            Some(PathBuf::from("/home/u")),
            profile::config_dir_name(),
            MarkerProbeOutcome::Absent,
            WriteProbeOutcome::Success,
        );
        assert_eq!(
            loc.config_dir.as_deref(),
            Some(Path::new("bin/.agentscommander"))
        );
        assert_eq!(
            loc.instance_base, None,
            "relative exe exposes no portable base"
        );
        assert_eq!(loc.local_dir_stem, "agentscommander");
    }

    #[test]
    fn missing_parent_takes_home_fallback() {
        // A bare root path has a stem-less/parent-less shape → home fallback.
        let root = if cfg!(windows) {
            PathBuf::from(r"C:\")
        } else {
            PathBuf::from("/")
        };
        let loc = resolve_instance_location(
            None,
            None,
            Ok(root),
            Some(PathBuf::from("/home/u")),
            profile::config_dir_name(),
            MarkerProbeOutcome::NotRun,
            WriteProbeOutcome::NotRun,
        );
        let expected = PathBuf::from("/home/u").join(profile::config_dir_name());
        assert_eq!(loc.config_dir.as_deref(), Some(expected.as_path()));
        assert_eq!(loc.instance_base, None);
    }

    #[test]
    fn current_exe_failure_uses_home_fallback_and_default_stem() {
        let loc = resolve_instance_location(
            None,
            None,
            exe_err(),
            Some(PathBuf::from("/home/u")),
            profile::config_dir_name(),
            MarkerProbeOutcome::NotRun,
            WriteProbeOutcome::NotRun,
        );
        let expected = PathBuf::from("/home/u").join(profile::config_dir_name());
        assert_eq!(loc.config_dir.as_deref(), Some(expected.as_path()));
        assert_eq!(loc.instance_base, None);
        assert_eq!(loc.local_dir_stem, "agentscommander");
    }

    #[test]
    fn current_exe_failure_and_no_home_yields_none_config() {
        let loc = resolve_instance_location(
            None,
            None,
            exe_err(),
            None,
            profile::config_dir_name(),
            MarkerProbeOutcome::NotRun,
            WriteProbeOutcome::NotRun,
        );
        assert_eq!(loc.config_dir, None);
        assert_eq!(loc.instance_base, None);
    }

    #[test]
    fn issue_1577_public_override_beats_debug_and_probe_failures() {
        let public = if cfg!(windows) {
            r"C:\public config"
        } else {
            "/public config"
        };
        let marker_failure = ProbeFailure::from_retry(
            RetryPlatform::Other,
            ProbeOperation::MarkerEntryMetadata,
            expected_marker(),
            RetriedIoError {
                error: Error::other("marker failed"),
                attempts: 1,
            },
        );
        let loc = resolve_instance_location(
            Some(public.to_string()),
            Some("debug-canary".to_string()),
            Ok(absolute_executable()),
            Some(PathBuf::from("/home/u")),
            profile::config_dir_name(),
            MarkerProbeOutcome::Indeterminate(marker_failure),
            WriteProbeOutcome::Failed(failed_write(
                RetryPlatform::Other,
                ProbeOperation::CreateConfigurationDirectory,
                expected_adjacent(),
                Error::new(ErrorKind::PermissionDenied, "denied"),
                1,
            )),
        );

        assert_eq!(loc.config_dir.as_deref(), Some(Path::new(public)));
        assert!(loc.startup_error.is_none());
        assert!(loc.fallback_diagnostic.is_none());
    }

    #[test]
    fn issue_1577_blank_overrides_fall_through_to_adjacent_success() {
        let loc = resolve_instance_location(
            Some("   ".to_string()),
            Some("\t".to_string()),
            Ok(absolute_executable()),
            Some(PathBuf::from("/home/u")),
            profile::config_dir_name(),
            MarkerProbeOutcome::Absent,
            WriteProbeOutcome::Success,
        );

        assert_eq!(loc.config_dir, Some(expected_adjacent()));
        assert!(loc.startup_error.is_none());
    }

    #[test]
    fn issue_1577_marker_present_success_selects_adjacent() {
        let loc = resolve_instance_location(
            None,
            None,
            Ok(absolute_executable()),
            Some(PathBuf::from("/home/u")),
            profile::config_dir_name(),
            MarkerProbeOutcome::Present,
            WriteProbeOutcome::Success,
        );

        assert_eq!(loc.config_dir, Some(expected_adjacent()));
        assert!(loc.startup_error.is_none());
    }

    #[test]
    fn issue_1577_marker_present_any_write_failure_is_hard() {
        for error in [
            Error::new(ErrorKind::PermissionDenied, "denied"),
            Error::other("unknown"),
        ] {
            let loc = resolve_instance_location(
                None,
                None,
                Ok(absolute_executable()),
                Some(PathBuf::from("/home/u")),
                profile::config_dir_name(),
                MarkerProbeOutcome::Present,
                WriteProbeOutcome::Failed(failed_write(
                    RetryPlatform::Other,
                    ProbeOperation::CreateConfigurationDirectory,
                    expected_adjacent(),
                    error,
                    1,
                )),
            );

            assert_eq!(loc.config_dir, Some(expected_adjacent()));
            assert!(loc.startup_error.is_some());
            assert!(loc.fallback_diagnostic.is_none());
            let message = loc.startup_error.unwrap().to_string();
            assert!(message.contains(&expected_marker().display().to_string()));
        }
    }

    #[test]
    fn issue_1577_unmarked_conclusive_failure_falls_home_with_diagnostic() {
        let failure = failed_write(
            RetryPlatform::Other,
            ProbeOperation::CreateConfigurationDirectory,
            expected_adjacent(),
            Error::new(ErrorKind::PermissionDenied, "permission denied"),
            1,
        );
        let home = PathBuf::from("/home/u");
        let expected_home = home.join(".injected-profile");
        let loc = resolve_instance_location(
            None,
            None,
            Ok(absolute_executable()),
            Some(home),
            ".injected-profile",
            MarkerProbeOutcome::Absent,
            WriteProbeOutcome::Failed(failure.clone()),
        );

        assert_eq!(loc.config_dir, Some(expected_home.clone()));
        assert_eq!(loc.instance_base, None);
        assert!(loc.startup_error.is_none());
        assert_eq!(
            loc.fallback_diagnostic,
            Some(AdjacentFallbackDiagnostic {
                candidate: expected_adjacent(),
                selected_home: Some(expected_home),
                failure,
            })
        );
    }

    #[test]
    fn issue_1577_unmarked_indeterminate_failure_never_relocates() {
        let loc = resolve_instance_location(
            None,
            None,
            Ok(absolute_executable()),
            Some(PathBuf::from("/home/u")),
            ".injected-profile",
            MarkerProbeOutcome::Absent,
            WriteProbeOutcome::Failed(failed_write(
                RetryPlatform::Other,
                ProbeOperation::CreateProbeFile,
                expected_adjacent().join("probe.tmp"),
                Error::new(ErrorKind::AlreadyExists, "collision"),
                1,
            )),
        );

        assert_eq!(loc.config_dir, Some(expected_adjacent()));
        assert!(loc.startup_error.is_some());
        assert!(loc.fallback_diagnostic.is_none());
        assert!(!loc
            .startup_error
            .unwrap()
            .to_string()
            .contains(&PathBuf::from("/home/u").display().to_string()));
    }

    #[test]
    fn issue_1577_conclusive_failure_without_home_preserves_tier_six() {
        let loc = resolve_instance_location(
            None,
            None,
            Ok(absolute_executable()),
            None,
            ".injected-profile",
            MarkerProbeOutcome::Absent,
            WriteProbeOutcome::Failed(failed_write(
                RetryPlatform::Other,
                ProbeOperation::CreateConfigurationDirectory,
                expected_adjacent(),
                Error::new(ErrorKind::PermissionDenied, "denied"),
                1,
            )),
        );

        assert_eq!(loc.config_dir, None);
        assert_eq!(loc.instance_base, None);
        assert!(loc.startup_error.is_none());
        assert!(loc.fallback_diagnostic.is_some());
    }

    #[test]
    fn issue_1577_identical_inputs_produce_identical_complete_location() {
        let resolve = || {
            resolve_instance_location(
                None,
                None,
                Ok(absolute_executable()),
                Some(PathBuf::from("/home/u")),
                ".injected-profile",
                MarkerProbeOutcome::Absent,
                WriteProbeOutcome::Success,
            )
        };
        assert_eq!(resolve(), resolve());
    }

    #[test]
    fn issue_1577_interrupted_gets_one_immediate_retry() {
        let mut calls = 0;
        let mut sleeps = Vec::new();
        let value = retry_transient_io_with_platform(
            RetryPlatform::Other,
            || {
                calls += 1;
                if calls == 1 {
                    Err(Error::new(ErrorKind::Interrupted, "interrupted"))
                } else {
                    Ok(1577)
                }
            },
            |delay| sleeps.push(delay.as_millis() as u64),
            |_, _, _, _| {},
        )
        .unwrap();

        assert_eq!(value, 1577);
        assert_eq!(calls, 2);
        assert!(sleeps.is_empty());

        calls = 0;
        let failure = retry_transient_io_with_platform::<(), _, _, _>(
            RetryPlatform::Other,
            || {
                calls += 1;
                Err(Error::new(ErrorKind::Interrupted, "still interrupted"))
            },
            |_| {},
            |_, _, _, _| {},
        )
        .unwrap_err();
        assert_eq!(calls, 2);
        assert_eq!(failure.attempts, 2);
    }

    #[test]
    fn issue_1577_windows_transient_schedule_and_mixed_upper_bound() {
        for raw_os_error in [5, 32] {
            let mut calls = 0;
            let mut sleeps = Vec::new();
            let failure = retry_transient_io_with_platform::<(), _, _, _>(
                RetryPlatform::Windows,
                || {
                    calls += 1;
                    Err(Error::from_raw_os_error(raw_os_error))
                },
                |delay| sleeps.push(delay.as_millis() as u64),
                |_, _, _, _| {},
            )
            .unwrap_err();
            assert_eq!(calls, 6);
            assert_eq!(failure.attempts, 6);
            assert_eq!(sleeps, vec![15, 30, 60, 120, 240]);
        }

        let mut errors = VecDeque::from([
            Error::new(ErrorKind::Interrupted, "interrupted"),
            Error::from_raw_os_error(5),
            Error::from_raw_os_error(5),
            Error::from_raw_os_error(5),
            Error::from_raw_os_error(5),
            Error::from_raw_os_error(5),
            Error::from_raw_os_error(5),
        ]);
        let mut calls = 0;
        let mut sleeps = Vec::new();
        let failure = retry_transient_io_with_platform::<(), _, _, _>(
            RetryPlatform::Windows,
            || {
                calls += 1;
                Err(errors.pop_front().unwrap())
            },
            |delay| sleeps.push(delay.as_millis() as u64),
            |_, _, _, _| {},
        )
        .unwrap_err();
        assert_eq!(calls, 7);
        assert_eq!(failure.attempts, 7);
        assert_eq!(sleeps, vec![15, 30, 60, 120, 240]);
    }

    #[test]
    fn issue_1577_non_windows_permission_and_read_only_never_sleep() {
        for kind in [ErrorKind::PermissionDenied, ErrorKind::ReadOnlyFilesystem] {
            let mut calls = 0;
            let mut sleeps = Vec::new();
            let failure = retry_transient_io_with_platform::<(), _, _, _>(
                RetryPlatform::Other,
                || {
                    calls += 1;
                    Err(Error::new(kind, "blocked"))
                },
                |delay| sleeps.push(delay),
                |_, _, _, _| {},
            )
            .unwrap_err();
            assert_eq!(calls, 1);
            assert_eq!(failure.attempts, 1);
            assert!(sleeps.is_empty());
        }
    }

    #[test]
    fn issue_1577_conclusive_classifier_is_a_closed_allowlist() {
        for error in [
            Error::new(ErrorKind::PermissionDenied, "permission"),
            Error::new(ErrorKind::ReadOnlyFilesystem, "read only"),
        ] {
            assert_eq!(
                classify_io_error_for_platform(RetryPlatform::Other, &error),
                ProbeFailureClass::ConclusiveUnwritable
            );
        }
        for raw_os_error in [5, 32] {
            assert_eq!(
                classify_io_error_for_platform(
                    RetryPlatform::Windows,
                    &Error::from_raw_os_error(raw_os_error)
                ),
                ProbeFailureClass::ConclusiveUnwritable
            );
        }
        for error in [
            Error::new(ErrorKind::Interrupted, "interrupted"),
            Error::new(ErrorKind::AlreadyExists, "exists"),
            Error::new(ErrorKind::NotFound, "missing"),
            Error::from_raw_os_error(12_345),
        ] {
            assert_eq!(
                classify_io_error_for_platform(RetryPlatform::Other, &error),
                ProbeFailureClass::Indeterminate
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn issue_1577_windows_raw_5_exhaustion_is_conclusive_without_real_acl() {
        let mut calls = 0;
        let mut sleeps = Vec::new();
        let failure = retry_transient_io_with::<(), _, _, _>(
            || {
                calls += 1;
                Err(Error::from_raw_os_error(5))
            },
            |delay| sleeps.push(delay.as_millis() as u64),
            |_, _, _, _| {},
        )
        .unwrap_err();
        let write_failure = failed_write(
            RetryPlatform::Windows,
            ProbeOperation::CreateConfigurationDirectory,
            expected_adjacent(),
            failure.error,
            failure.attempts,
        );
        let expected_home = PathBuf::from(r"C:\Users\tester\.agentscommander-new");
        let loc = resolve_instance_location(
            None,
            None,
            Ok(absolute_executable()),
            Some(PathBuf::from(r"C:\Users\tester")),
            ".agentscommander-new",
            MarkerProbeOutcome::Absent,
            WriteProbeOutcome::Failed(write_failure),
        );
        assert_eq!(calls, 6);
        assert_eq!(sleeps, vec![15, 30, 60, 120, 240]);
        assert_eq!(loc.config_dir, Some(expected_home));
    }

    #[cfg(windows)]
    #[test]
    fn issue_1577_windows_raw_32_exhaustion_matches_raw_5() {
        let mut calls = 0;
        let mut sleeps = Vec::new();
        let failure = retry_transient_io_with::<(), _, _, _>(
            || {
                calls += 1;
                Err(Error::from_raw_os_error(32))
            },
            |delay| sleeps.push(delay.as_millis() as u64),
            |_, _, _, _| {},
        )
        .unwrap_err();
        assert_eq!(calls, 6);
        assert_eq!(failure.attempts, 6);
        assert_eq!(
            classify_io_error_for_platform(RetryPlatform::Windows, &failure.error),
            ProbeFailureClass::ConclusiveUnwritable
        );
        assert_eq!(sleeps, vec![15, 30, 60, 120, 240]);
    }

    #[test]
    fn issue_1577_marker_absent_files_directories_and_symlinks() {
        let marker = Path::new("portable.txt");
        assert_eq!(
            probe_portable_marker_with(
                RetryPlatform::Other,
                marker,
                |_| Err(Error::new(ErrorKind::NotFound, "missing")),
                |_| unreachable!(),
                |_| {}
            ),
            MarkerProbeOutcome::Absent
        );
        for entry in [MarkerEntryKind::File, MarkerEntryKind::Directory] {
            assert_eq!(
                probe_portable_marker_with(
                    RetryPlatform::Other,
                    marker,
                    |_| Ok(entry),
                    |_| unreachable!(),
                    |_| {}
                ),
                MarkerProbeOutcome::Present
            );
        }
        for target in [MarkerEntryKind::File, MarkerEntryKind::Directory] {
            assert_eq!(
                probe_portable_marker_with(
                    RetryPlatform::Other,
                    marker,
                    |_| Ok(MarkerEntryKind::Symlink),
                    |_| Ok(target),
                    |_| {}
                ),
                MarkerProbeOutcome::Present
            );
        }
    }

    #[test]
    fn issue_1577_real_marker_contents_are_never_interpreted() {
        let temp = tempfile::TempDir::new().unwrap();
        let marker = temp.path().join("portable.txt");
        std::fs::write(&marker, []).unwrap();
        assert_eq!(probe_portable_marker(&marker), MarkerProbeOutcome::Present);
        std::fs::write(&marker, b"arbitrary contents").unwrap();
        assert_eq!(probe_portable_marker(&marker), MarkerProbeOutcome::Present);
        std::fs::remove_file(&marker).unwrap();
        std::fs::create_dir(&marker).unwrap();
        assert_eq!(probe_portable_marker(&marker), MarkerProbeOutcome::Present);
    }

    #[test]
    fn issue_1577_marker_metadata_retries_windows_sharing_violation() {
        let mut calls = 0;
        let mut sleeps = Vec::new();
        let outcome = probe_portable_marker_with(
            RetryPlatform::Windows,
            Path::new("portable.txt"),
            |_| {
                calls += 1;
                if calls == 1 {
                    Err(Error::from_raw_os_error(32))
                } else {
                    Ok(MarkerEntryKind::File)
                }
            },
            |_| unreachable!(),
            |delay| sleeps.push(delay.as_millis() as u64),
        );
        assert_eq!(outcome, MarkerProbeOutcome::Present);
        assert_eq!(calls, 2);
        assert_eq!(sleeps, vec![15]);
    }

    #[test]
    fn issue_1577_marker_metadata_permission_and_unsupported_are_indeterminate() {
        let marker = Path::new("portable.txt");
        let outcome = probe_portable_marker_with(
            RetryPlatform::Other,
            marker,
            |_| Err(Error::new(ErrorKind::PermissionDenied, "denied")),
            |_| unreachable!(),
            |_| {},
        );
        let MarkerProbeOutcome::Indeterminate(failure) = outcome else {
            panic!("expected indeterminate metadata error");
        };
        assert_eq!(failure.operation, ProbeOperation::MarkerEntryMetadata);
        assert_eq!(failure.attempts, 1);
        assert_eq!(failure.kind, Some(ErrorKind::PermissionDenied));

        let outcome = probe_portable_marker_with(
            RetryPlatform::Other,
            marker,
            |_| Ok(MarkerEntryKind::Unsupported),
            |_| unreachable!(),
            |_| {},
        );
        let MarkerProbeOutcome::Indeterminate(failure) = outcome else {
            panic!("expected unsupported marker to be indeterminate");
        };
        assert_eq!(failure.operation, ProbeOperation::UnsupportedMarkerEntry);
    }

    #[test]
    fn broken_marker_link_is_indeterminate_and_never_falls_home() {
        let marker_failure = match probe_portable_marker_with(
            RetryPlatform::Other,
            &expected_marker(),
            |_| Ok(MarkerEntryKind::Symlink),
            |_| Err(Error::new(ErrorKind::NotFound, "missing target")),
            |_| {},
        ) {
            MarkerProbeOutcome::Indeterminate(failure) => failure,
            other => panic!("expected indeterminate broken marker, got {other:?}"),
        };
        let loc = resolve_instance_location(
            None,
            None,
            Ok(absolute_executable()),
            Some(PathBuf::from("/home/u")),
            ".injected-profile",
            MarkerProbeOutcome::Indeterminate(marker_failure),
            WriteProbeOutcome::NotRun,
        );

        assert_eq!(loc.config_dir, Some(expected_adjacent()));
        assert!(loc.fallback_diagnostic.is_none());
        let error = loc.startup_error.expect("broken marker must hard-fail");
        let ConfigStartupError::AdjacentSelectionBlocked { marker_path, .. } = error;
        assert_eq!(marker_path, Some(expected_marker()));
    }

    #[test]
    fn issue_1577_real_write_probe_keeps_directory_and_leaves_no_probe_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let existing = temp.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        assert_eq!(probe_candidate_write(&existing), WriteProbeOutcome::Success);
        assert_eq!(std::fs::read_dir(&existing).unwrap().count(), 0);

        let missing = temp.path().join("missing");
        assert_eq!(probe_candidate_write(&missing), WriteProbeOutcome::Success);
        assert!(missing.is_dir());
        assert_eq!(std::fs::read_dir(&missing).unwrap().count(), 0);
    }

    #[test]
    fn issue_1577_non_directory_candidate_is_indeterminate_and_hard() {
        let temp = tempfile::TempDir::new().unwrap();
        let candidate = temp.path().join("candidate");
        std::fs::write(&candidate, b"not a directory").unwrap();
        let WriteProbeOutcome::Failed(failure) = probe_candidate_write(&candidate) else {
            panic!("non-directory candidate must fail");
        };
        assert_eq!(failure.class, ProbeFailureClass::Indeterminate);

        let executable = temp.path().join("agentscommander.exe");
        let loc = resolve_instance_location(
            None,
            None,
            Ok(executable),
            Some(PathBuf::from("/home/u")),
            ".injected-profile",
            MarkerProbeOutcome::Absent,
            WriteProbeOutcome::Failed(failure),
        );
        assert!(loc.startup_error.is_some());
        assert!(loc.fallback_diagnostic.is_none());
    }

    #[test]
    fn issue_1577_cleanup_interrupted_then_success_discards_transient_error() {
        let mut removals = 0;
        let outcome = probe_candidate_write_with(
            RetryPlatform::Other,
            Path::new("candidate"),
            Path::new("candidate/probe.tmp"),
            |_| Ok(()),
            |_| Ok(()),
            |_| Err(Error::new(ErrorKind::PermissionDenied, "write denied")),
            |_| {
                removals += 1;
                if removals == 1 {
                    Err(Error::new(ErrorKind::Interrupted, "interrupted"))
                } else {
                    Ok(())
                }
            },
            |_| {},
        );
        let WriteProbeOutcome::Failed(failure) = outcome else {
            panic!("write failure must be retained");
        };
        assert_eq!(removals, 2);
        assert!(failure.cleanup.is_none());
        assert!(!failure.probe_may_remain);
        assert_eq!(failure.class, ProbeFailureClass::ConclusiveUnwritable);
    }

    #[test]
    fn issue_1577_cleanup_windows_sharing_violation_then_success() {
        let mut removals = 0;
        let mut sleeps = Vec::new();
        let outcome = probe_candidate_write_with(
            RetryPlatform::Windows,
            Path::new("candidate"),
            Path::new("candidate/probe.tmp"),
            |_| Ok(()),
            |_| Ok(()),
            |_| Err(Error::new(ErrorKind::PermissionDenied, "write denied")),
            |_| {
                removals += 1;
                if removals == 1 {
                    Err(Error::from_raw_os_error(32))
                } else {
                    Ok(())
                }
            },
            |delay| sleeps.push(delay.as_millis() as u64),
        );
        let WriteProbeOutcome::Failed(failure) = outcome else {
            panic!("write failure must be retained");
        };
        assert_eq!(removals, 2);
        assert_eq!(sleeps, vec![15]);
        assert!(failure.cleanup.is_none());
        assert!(!failure.probe_may_remain);
    }

    #[test]
    fn issue_1577_persistent_required_delete_retains_debris_diagnostic() {
        let outcome = probe_candidate_write_with(
            RetryPlatform::Other,
            Path::new("candidate"),
            Path::new("candidate/probe.tmp"),
            |_| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
            |_| Err(Error::new(ErrorKind::PermissionDenied, "delete denied")),
            |_| {},
        );
        let WriteProbeOutcome::Failed(failure) = outcome else {
            panic!("required delete must fail");
        };
        assert_eq!(failure.class, ProbeFailureClass::ConclusiveUnwritable);
        assert!(failure.probe_may_remain);
        assert!(failure.cleanup.is_none());
        assert!(failure
            .reason()
            .ends_with("The probe file \"candidate/probe.tmp\" may remain."));
    }

    #[test]
    fn issue_1577_unknown_cleanup_upgrades_conclusive_primary_to_indeterminate() {
        let outcome = probe_candidate_write_with(
            RetryPlatform::Other,
            Path::new("candidate"),
            Path::new("candidate/probe.tmp"),
            |_| Ok(()),
            |_| Ok(()),
            |_| Err(Error::new(ErrorKind::PermissionDenied, "write denied")),
            |_| Err(Error::other("cleanup unknown")),
            |_| {},
        );
        let WriteProbeOutcome::Failed(failure) = outcome else {
            panic!("write and cleanup must fail");
        };
        assert_eq!(
            failure.primary.class,
            ProbeFailureClass::ConclusiveUnwritable
        );
        assert_eq!(failure.class, ProbeFailureClass::Indeterminate);
        assert!(failure.cleanup.is_some());
        assert!(failure.probe_may_remain);
    }

    #[test]
    fn issue_1577_startup_and_diagnostic_formatters_are_exact() {
        let candidate = PathBuf::from("bin/.agentscommander");
        let marker = PathBuf::from("bin/portable.txt");
        let probe = PathBuf::from("bin/.agentscommander/probe.tmp");
        let primary = ProbeFailure::from_retry(
            RetryPlatform::Other,
            ProbeOperation::WriteProbeFile,
            probe.clone(),
            RetriedIoError {
                error: Error::new(ErrorKind::PermissionDenied, "write denied"),
                attempts: 1,
            },
        );
        let cleanup = ProbeFailure::from_retry(
            RetryPlatform::Other,
            ProbeOperation::DeleteProbeFile,
            probe.clone(),
            RetriedIoError {
                error: Error::other("cleanup unknown"),
                attempts: 2,
            },
        );
        let write_failure =
            WriteProbeFailure::new(primary, Some(cleanup), Some(probe.clone()), true);
        assert_eq!(
            write_failure.reason(),
            format!(
                "write probe could not write probe file \"{}\" after 1 attempt(s): write denied. Cleanup of probe file \"{}\" also failed after 2 attempt(s): cleanup unknown; the probe file may remain.",
                probe.display(),
                probe.display()
            )
        );

        let present_error = ConfigStartupError::AdjacentSelectionBlocked {
            config_dir: candidate.clone(),
            marker_path: Some(marker.clone()),
            reason: write_failure.reason(),
        };
        assert_eq!(
            present_error.to_string(),
            format!(
                "AgentsCommander cannot start because configuration directory \"{}\" could not be safely selected: {} Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart. Portable marker path: \"{}\".",
                candidate.display(),
                write_failure.reason(),
                marker.display()
            )
        );

        let marker_failure = ProbeFailure::from_retry(
            RetryPlatform::Other,
            ProbeOperation::MarkerTargetMetadata,
            marker.clone(),
            RetriedIoError {
                error: Error::new(ErrorKind::NotFound, "missing target"),
                attempts: 1,
            },
        );
        assert_eq!(
            marker_failure.reason(),
            format!(
                "could not resolve portable marker symlink target metadata \"{}\" after 1 attempt(s): missing target",
                marker.display()
            )
        );

        let unmarked_error = ConfigStartupError::AdjacentSelectionBlocked {
            config_dir: candidate.clone(),
            marker_path: None,
            reason: "unknown filesystem state".to_string(),
        };
        assert_eq!(
            unmarked_error.to_string(),
            format!(
                "AgentsCommander cannot start because configuration directory \"{}\" could not be safely selected: unknown filesystem state. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart.",
                candidate.display()
            )
        );

        let warning = AdjacentFallbackDiagnostic {
            candidate: candidate.clone(),
            selected_home: Some(PathBuf::from("home/.agentscommander-new")),
            failure: failed_write(
                RetryPlatform::Other,
                ProbeOperation::CreateConfigurationDirectory,
                candidate.clone(),
                Error::new(ErrorKind::PermissionDenied, "permission denied"),
                1,
            ),
        };
        assert_eq!(
            warning.to_string(),
            format!(
                "configuration directory \"{}\" was conclusively unwritable; using \"{}\" instead: write probe could not create configuration directory \"{}\" after 1 attempt(s): permission denied",
                candidate.display(),
                Path::new("home/.agentscommander-new").display(),
                candidate.display()
            )
        );
    }
}
