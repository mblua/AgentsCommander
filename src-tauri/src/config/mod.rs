pub mod activity_log;
pub mod agent_command;
pub mod agent_config;
pub mod agent_creation;
pub mod archive_gate;
pub mod coding_agent_mutations;
pub mod coding_agent_profiles;
pub mod coding_agents_catalog;
pub mod config_seed;
pub mod coordinator_clocks;
pub mod daemon_pid;
pub mod injected_messages;
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
pub mod workspace;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
    test_override: Option<String>,
    current_exe_result: Result<PathBuf, std::io::Error>,
    home_dir: Option<PathBuf>,
) -> InstanceLocation {
    // Local agent dir stem: from the running executable only. Independent of the
    // debug override so `agent_local_dir_name()` keeps naming replica dirs after
    // the real binary, and falls back to "agentscommander" when unavailable.
    let local_dir_stem = current_exe_result
        .as_ref()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "agentscommander".to_string());

    // Debug-only override: selects the config directory exactly as the legacy
    // code did (verbatim `PathBuf::from`, untrimmed). Its parent is the instance
    // base ONLY when the override is absolute; a relative override yields no base.
    if let Some(raw) = test_override.as_deref() {
        if !raw.trim().is_empty() {
            let override_path = PathBuf::from(raw);
            let instance_base = if override_path.is_absolute() {
                override_path.parent().map(Path::to_path_buf)
            } else {
                None
            };
            return InstanceLocation {
                config_dir: Some(override_path),
                local_dir_stem,
                instance_base,
            };
        }
    }

    // Primary: portable config directory next to the native executable.
    if let Ok(exe_path) = current_exe_result {
        match (exe_path.parent(), exe_path.file_stem()) {
            (Some(parent), Some(stem)) => {
                let config_dir = parent.join(format!(".{}", stem.to_string_lossy()));
                // The instance base is the native executable directory, but only
                // when it is absolute. A relative executable (e.g. `bin/ac`) keeps
                // its relative config dir yet exposes no portable base rather than
                // being absolutized through the process CWD.
                let instance_base = if parent.is_absolute() {
                    Some(parent.to_path_buf())
                } else {
                    None
                };
                return InstanceLocation {
                    config_dir: Some(config_dir),
                    local_dir_stem,
                    instance_base,
                };
            }
            _ => {
                log::warn!(
                    "[config_dir] current_exe() path has no parent or stem: {:?}, falling back to $HOME",
                    exe_path
                );
            }
        }
    }

    // Fallback: legacy $HOME-based config. No portable instance base.
    InstanceLocation {
        config_dir: home_dir.map(|home| home.join(profile::config_dir_name())),
        local_dir_stem,
        instance_base: None,
    }
}

/// Cached, process-wide [`InstanceLocation`], resolved once at first call.
fn instance_location() -> &'static InstanceLocation {
    static LOC: OnceLock<InstanceLocation> = OnceLock::new();
    LOC.get_or_init(|| {
        // The test override is a debug-only affordance; release builds never read it.
        #[cfg(debug_assertions)]
        let test_override = std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").ok();
        #[cfg(not(debug_assertions))]
        let test_override: Option<String> = None;
        resolve_instance_location(test_override, std::env::current_exe(), dirs::home_dir())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};
    use std::path::PathBuf;

    fn exe_err() -> Result<PathBuf, Error> {
        Err(Error::new(ErrorKind::NotFound, "no current_exe"))
    }

    #[test]
    fn packaged_absolute_executable_yields_portable_config_and_base() {
        let exe = if cfg!(windows) {
            PathBuf::from(r"C:\bundle\agentscommander.exe")
        } else {
            PathBuf::from("/opt/bundle/agentscommander")
        };
        let loc = resolve_instance_location(None, Ok(exe), Some(PathBuf::from("/home/u")));
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
        let loc = resolve_instance_location(None, Ok(exe), Some(PathBuf::from("/home/u")));
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
            Some(override_dir.to_string()),
            Ok(exe),
            Some(PathBuf::from("/home/u")),
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
            Some("relative/.acdir".to_string()),
            exe_err(),
            Some(PathBuf::from("/home/u")),
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
            Some("   ".to_string()),
            Ok(exe),
            Some(PathBuf::from("/home/u")),
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
            Ok(PathBuf::from("bin/agentscommander")),
            Some(PathBuf::from("/home/u")),
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
        let loc = resolve_instance_location(None, Ok(root), Some(PathBuf::from("/home/u")));
        let expected = PathBuf::from("/home/u").join(profile::config_dir_name());
        assert_eq!(loc.config_dir.as_deref(), Some(expected.as_path()));
        assert_eq!(loc.instance_base, None);
    }

    #[test]
    fn current_exe_failure_uses_home_fallback_and_default_stem() {
        let loc = resolve_instance_location(None, exe_err(), Some(PathBuf::from("/home/u")));
        let expected = PathBuf::from("/home/u").join(profile::config_dir_name());
        assert_eq!(loc.config_dir.as_deref(), Some(expected.as_path()));
        assert_eq!(loc.instance_base, None);
        assert_eq!(loc.local_dir_stem, "agentscommander");
    }

    #[test]
    fn current_exe_failure_and_no_home_yields_none_config() {
        let loc = resolve_instance_location(None, exe_err(), None);
        assert_eq!(loc.config_dir, None);
        assert_eq!(loc.instance_base, None);
    }
}
