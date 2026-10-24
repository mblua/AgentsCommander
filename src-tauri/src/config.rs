pub(crate) mod ac_root;
pub(crate) mod activity_log;
pub(crate) mod agency_cache;
pub mod agent_command;
pub(crate) mod agent_config;
pub(crate) mod agent_creation;
pub(crate) mod agent_memory;
pub(crate) mod agent_path;
pub(crate) mod archive_gate;
pub(crate) mod co_managed;
pub(crate) mod coding_agent_mutations;
pub(crate) mod coding_agent_profiles;
pub mod coding_agents_catalog;
pub(crate) mod config_seed;
pub(crate) mod coordinator_clocks;
pub(crate) mod daemon_pid;
pub(crate) mod entity_prefix;
pub(crate) mod injected_messages;
pub(crate) mod instance_artifacts;
pub(crate) mod instance_gitignore;
pub mod local_config_io;
pub(crate) mod local_overlay;
pub mod loops;
pub(crate) mod naming_migration;
mod placeholders;
pub mod profile;
pub(crate) mod project_settings;
pub(crate) mod projects;
pub(crate) mod remote_activity_cache;
pub(crate) mod replica_identity;
pub(crate) mod role_templates;
pub(crate) mod root_agent;
pub(crate) mod seed_manifest;
pub mod seeded_context_templates;
pub mod session_context;
pub mod sessions_persistence;
pub mod settings;
pub(crate) mod shared_locations;
pub mod teams;

use std::fmt;
use std::fs::{self, File, OpenOptions};
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

#[cfg(windows)]
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

    pub(crate) fn reason(&self) -> String {
        match self.operation {
            ProbeOperation::CreateConfigurationDirectory => format!(
                "write probe could not create configuration directory \"{}\" after {} attempt(s): \
                    {}",
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
                ". Cleanup of probe file \"{}\" also failed after {} attempt(s): {}; the probe \
                    file may remain.",
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
pub(crate) enum ConfigStartupError {
    /// #1577: an executable-adjacent configuration directory could not be
    /// selected because the write probe gave an indeterminate result (or did not
    /// run). #1930: a suffixed executable never falls back to HOME, so startup
    /// stops and the message names the only remedy.
    AdjacentSelectionBlocked { config_dir: PathBuf, reason: String },
    /// #1930: a suffixed executable's unmarked adjacent directory is
    /// conclusively unwritable. There is no HOME fallback, so startup stops and
    /// the message names both remedies.
    AdjacentDirectoryUnwritable { config_dir: PathBuf, reason: String },
}

impl fmt::Display for ConfigStartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AdjacentSelectionBlocked { config_dir, reason } => {
                write!(
                    formatter,
                    "AgentsCommander cannot start because configuration directory \"{}\" could \
                        not be safely selected: {}",
                    config_dir.display(),
                    reason
                )?;
                if !reason.ends_with('.') {
                    write!(formatter, ".")?;
                }
                write!(
                    formatter,
                    " Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart."
                )
            }
            Self::AdjacentDirectoryUnwritable { config_dir, reason } => {
                write!(
                    formatter,
                    "AgentsCommander cannot start because it cannot write its configuration \
                        directory \"{}\" next to the executable: {}",
                    config_dir.display(),
                    reason
                )?;
                if !reason.ends_with('.') {
                    write!(formatter, ".")?;
                }
                write!(
                    formatter,
                    " Move the executable to a writable folder, or set AGENTSCOMMANDER_CONFIG_DIR \
                        to a writable directory, and restart."
                )
            }
        }
    }
}

impl std::error::Error for ConfigStartupError {}

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
    /// The app config directory. Honors the public `AGENTSCOMMANDER_CONFIG_DIR`
    /// override verbatim, then the debug `AGENTSCOMMANDER_TEST_CONFIG_DIR`
    /// override verbatim. Without an effective override an executable whose
    /// stem has no underscore suffix selects the canonical
    /// `$HOME/.agentscommander` (#1868) and never an adjacent directory; a
    /// suffixed executable takes the portable `<exe-parent>/.<exe-stem>` form
    /// with its write-probe table and never a HOME directory (#1930). `None`
    /// only when no override applies, the executable is unsuffixed or
    /// unavailable, and no home directory exists.
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdjacentPaths {
    config_dir: PathBuf,
    instance_base: Option<PathBuf>,
}

fn adjacent_paths(executable: &Path) -> Option<AdjacentPaths> {
    let parent = executable.parent()?;
    let stem = executable.file_stem()?;
    Some(AdjacentPaths {
        config_dir: parent.join(format!(".{}", stem.to_string_lossy())),
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
    }
}

/// The HOME location: `$HOME/<profile::config_dir_name()>` with no instance
/// base and no startup error; `None` when no home directory is available.
/// Reached only by an executable without an underscore suffix (#1868) or an
/// unavailable `current_exe()`. A suffixed path always has a parent and a
/// stem, so a suffixed executable never reaches it (#1930).
fn home_location(home_dir: Option<PathBuf>, local_dir_stem: String) -> InstanceLocation {
    InstanceLocation {
        config_dir: home_dir.map(|home| home.join(profile::config_dir_name())),
        local_dir_stem,
        instance_base: None,
        startup_error: None,
    }
}

fn blocked_adjacent_location(
    paths: AdjacentPaths,
    local_dir_stem: String,
    reason: String,
) -> InstanceLocation {
    let startup_error = ConfigStartupError::AdjacentSelectionBlocked {
        config_dir: paths.config_dir.clone(),
        reason,
    };
    InstanceLocation {
        config_dir: Some(paths.config_dir),
        local_dir_stem,
        instance_base: paths.instance_base,
        startup_error: Some(startup_error),
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
/// - `home_dir`: `dirs::home_dir()` for the HOME location.
///
/// #1868: after the overrides, an executable without an underscore suffix
/// returns the HOME location immediately. It ignores BUILD_PROFILE, install
/// location and whatever probe outcomes were supplied; no startup error and no
/// fallback diagnostic can arise on that route. Only suffixed executables reach
/// the adjacent write-probe table.
///
/// #1930: that table never selects HOME. A conclusively unwritable candidate
/// becomes `ConfigStartupError::AdjacentDirectoryUnwritable`; an indeterminate
/// write result keeps `ConfigStartupError::AdjacentSelectionBlocked`.
pub(crate) fn resolve_instance_location(
    public_override: Option<String>,
    test_override: Option<String>,
    current_exe_result: Result<PathBuf, std::io::Error>,
    home_dir: Option<PathBuf>,
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

    // #1868: route on the same first-underscore parser the profile uses. An
    // unsuffixed executable never constructs adjacent candidates; it selects
    // the canonical HOME location before `adjacent_paths` is consulted.
    let suffixed_executable = current_exe_result
        .as_ref()
        .ok()
        .filter(|path| profile::binary_suffix_from_path(path).is_some());
    let Some(paths) = suffixed_executable.and_then(|path| adjacent_paths(path)) else {
        return home_location(home_dir, local_dir_stem);
    };

    // #1868/#1930: the adjacent table runs the write probe and never HOME.
    match write_probe {
        WriteProbeOutcome::Success => InstanceLocation {
            config_dir: Some(paths.config_dir),
            local_dir_stem,
            instance_base: paths.instance_base,
            startup_error: None,
        },
        WriteProbeOutcome::Failed(failure)
            if failure.class == ProbeFailureClass::ConclusiveUnwritable =>
        {
            // #1930: a suffixed executable never falls back to HOME.
            let startup_error = ConfigStartupError::AdjacentDirectoryUnwritable {
                config_dir: paths.config_dir.clone(),
                reason: failure.reason(),
            };
            InstanceLocation {
                config_dir: Some(paths.config_dir),
                local_dir_stem,
                instance_base: paths.instance_base,
                startup_error: Some(startup_error),
            }
        }
        WriteProbeOutcome::Failed(failure) => {
            blocked_adjacent_location(paths, local_dir_stem, failure.reason())
        }
        WriteProbeOutcome::NotRun => blocked_adjacent_location(
            paths,
            local_dir_stem,
            "write probe was not run for a portable configuration directory".to_string(),
        ),
    }
}

/// #1868: the lazy production orchestration behind [`instance_location`].
/// Decides whether the adjacent probe runs at all, runs the write probe only
/// when a suffixed executable has an adjacent candidate, and hands the outcome
/// to [`resolve_instance_location`].
///
/// An effective override or an executable without an underscore suffix never
/// constructs the adjacent candidate and never invokes the probe; its outcome
/// stays `NotRun`. The probe is a closure so tests can drive this exact
/// production path with a counting or panicking probe and no filesystem. It
/// keeps no state and abstracts no I/O of its own.
fn resolve_instance_location_with_probes<WriteProbe>(
    public_override: Option<String>,
    test_override: Option<String>,
    current_exe_result: Result<PathBuf, std::io::Error>,
    home_dir: Option<PathBuf>,
    mut write_probe: WriteProbe,
) -> InstanceLocation
where
    WriteProbe: FnMut(&Path) -> WriteProbeOutcome,
{
    let override_selected = nonblank_override(public_override.as_ref()).is_some()
        || nonblank_override(test_override.as_ref()).is_some();
    let adjacent = if override_selected {
        None
    } else {
        current_exe_result
            .as_ref()
            .ok()
            .filter(|path| profile::binary_suffix_from_path(path).is_some())
            .and_then(|path| adjacent_paths(path))
    };
    let write_outcome = match adjacent {
        None => WriteProbeOutcome::NotRun,
        Some(adjacent) => write_probe(&adjacent.config_dir),
    };

    resolve_instance_location(
        public_override,
        test_override,
        current_exe_result,
        home_dir,
        write_outcome,
    )
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

        resolve_instance_location_with_probes(
            public_override,
            test_override,
            current_exe_result,
            home_dir,
            probe_candidate_write,
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

/// Returns the app config directory.
/// Unsuffixed executable (e.g. `agentscommander.exe`): `$HOME/.agentscommander` (#1868).
/// Suffixed executable: portable `<binary_parent_dir>/.<binary_file_stem>/`,
/// e.g. `C:\tools\agentscommander_standalone.exe` → `C:\tools\.agentscommander_standalone\`,
/// and never a HOME directory (#1930).
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

#[cfg(test)]
mod tests;
