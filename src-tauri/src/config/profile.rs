//! Instance-label derivation: runtime binary name > compile-time BUILD_PROFILE.
//!
//! Convention: `agentscommander_<suffix>.exe` -> label = `<SUFFIX>` uppercased.
//! - `agentscommander_stage.exe`  -> [STAGE]
//! - `agentscommander_test-1.exe` -> [TEST-1]
//! - `agentscommander.exe` (no underscore) -> prod (no badge)
//!
//! BUILD_PROFILE (set by build.rs) is the fallback for `cargo run` where the
//! binary name has no underscore suffix.

use std::sync::OnceLock;

/// Build profile string — "dev", "prod", or "stage".
/// Used as fallback when binary name has no underscore suffix.
pub const BUILD_PROFILE: &str = env!("BUILD_PROFILE");

/// Extract suffix from binary name: `agentscommander_foo` -> Some("foo").
/// Returns None for plain `agentscommander` (no underscore = prod).
/// Cached via OnceLock — parsed once at first call.
fn binary_suffix() -> Option<&'static str> {
    static SUFFIX: OnceLock<Option<String>> = OnceLock::new();
    SUFFIX
        .get_or_init(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                .and_then(|name| name.find('_').map(|i| name[i + 1..].to_string()))
        })
        .as_deref()
}

/// Capitalize the first character of a suffix: "stage" -> "Stage", "test-1" -> "Test-1".
fn capitalize_suffix(suffix: &str) -> String {
    let mut chars = suffix.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Config directory name under $HOME.
/// Only "dev" (via suffix or BUILD_PROFILE) gets a separate dir.
/// Everyone else shares the legacy `.agentscommander-new` data directory for
/// settings compatibility. This is internal storage, not release identity.
pub fn config_dir_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        let is_dev =
            binary_suffix() == Some("dev") || (binary_suffix().is_none() && BUILD_PROFILE == "dev");
        if is_dev {
            ".agentscommander-new-dev".to_string()
        } else {
            ".agentscommander-new".to_string()
        }
    })
}

/// Application title for the sidebar window.
/// Runtime suffix: "AC [SUFFIX]" (uppercased).
/// No suffix: falls back to BUILD_PROFILE behaviour.
/// The short "AC" prefix keeps the OS taskbar tooltip compact (issue #606);
/// only this literal prefix changed, the suffix-derivation logic is unchanged.
pub fn app_title() -> &'static str {
    static TITLE: OnceLock<String> = OnceLock::new();
    TITLE.get_or_init(|| match binary_suffix() {
        Some(suffix) => format!("AC [{}]", suffix.to_uppercase()),
        None => match BUILD_PROFILE {
            "dev" => "AC".to_string(),
            "stage" => "AC [STAGE]".to_string(),
            _ => "AC".to_string(),
        },
    })
}

/// Title suffix appended to secondary windows (guide, etc.).
/// Delegates to app_title().
pub fn app_title_suffix() -> &'static str {
    app_title()
}

/// Windows single-instance mutex name. Each suffix gets a unique mutex
/// so different instances can run simultaneously.
pub fn mutex_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| match binary_suffix() {
        Some(suffix) => format!(
            "Local\\AgentsCommander_SingleInstance_{}\0",
            capitalize_suffix(suffix)
        ),
        None => match BUILD_PROFILE {
            "dev" => "Local\\AgentsCommander_SingleInstance_New_Dev\0".to_string(),
            "stage" => "Local\\AgentsCommander_SingleInstance_Stage\0".to_string(),
            _ => "Local\\AgentsCommander_SingleInstance\0".to_string(),
        },
    })
}

/// #786: side-effect-free probe of whether a GUI instance for THIS binary
/// identity is already running, by OPENING (not creating) the single-instance
/// mutex. `OpenMutexW` never creates the mutex, so unlike `CreateMutexW` this
/// cannot race a starting GUI into refusing to launch. The CLI uses it to route
/// coding-agent mutations: GUI up -> queue through the poller; GUI down ->
/// direct write. Windows-only; returns false off-Windows (single-instance is
/// not enforced there, so a running GUI is not detected and callers fall back to
/// the direct write path - a documented platform gap).
#[cfg(target_os = "windows")]
pub fn gui_instance_running() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::OpenMutexW;

    // The standard SYNCHRONIZE access right (0x0010_0000), defined inline rather
    // than imported from windows_sys: the module that re-exports the bare
    // constant may sit behind a Cargo feature we do not enable, and adding one is
    // out of scope. Mirrors the inline `const ERROR_ALREADY_EXISTS` in main.rs.
    const SYNCHRONIZE: u32 = 0x0010_0000;

    let name: Vec<u16> = mutex_name().encode_utf16().collect();
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer (the trailing `\0` is
    // embedded in `mutex_name()`) valid for the duration of the call. A non-null
    // handle is owned by us and closed immediately; a null handle means the mutex
    // does not exist (no GUI) or the open failed - both treated as "not running".
    unsafe {
        let handle = OpenMutexW(SYNCHRONIZE, 0, name.as_ptr());
        if handle.is_null() {
            false
        } else {
            let _ = CloseHandle(handle);
            true
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn gui_instance_running() -> bool {
    false
}

/// Executable name — actual binary filename from current_exe().
/// Falls back to BUILD_PROFILE mapping if current_exe() fails.
pub fn exe_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
            .unwrap_or_else(|| match BUILD_PROFILE {
                // Internal dev fallback for direct cargo runs. Tauri production
                // bundles set mainBinaryName to `agentscommander`.
                "dev" => "agentscommander-new.exe".to_string(),
                "stage" => "agentscommander-stage.exe".to_string(),
                _ => "agentscommander.exe".to_string(),
            })
    })
}

/// Product name as installed in LOCALAPPDATA (matches Tauri productName).
/// Runtime suffix: "Agents Commander <Suffix>".
/// Falls back to BUILD_PROFILE mapping.
pub fn product_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| match binary_suffix() {
        Some(suffix) => format!("Agents Commander {}", capitalize_suffix(suffix)),
        None => match BUILD_PROFILE {
            "dev" => "Agents Commander".to_string(),
            "stage" => "Agents Commander Stage".to_string(),
            _ => "Agents Commander".to_string(),
        },
    })
}

/// Default web server port. Each instance gets a distinct port.
/// Known suffixes get hardcoded ports; unknown suffixes get a deterministic hash
/// in the 9880-9899 range.
pub fn web_server_port() -> u16 {
    match binary_suffix() {
        Some("dev") => 9876,
        Some("stage") => 9878,
        Some(suffix) => {
            let hash = suffix
                .bytes()
                .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
            9880 + (hash % 20) as u16
        }
        None => match BUILD_PROFILE {
            "dev" => 9876,
            "stage" => 9878,
            _ => 9877, // prod
        },
    }
}

/// Whether this is the STAGE profile (via suffix or BUILD_PROFILE fallback).
pub fn is_stage() -> bool {
    binary_suffix() == Some("stage") || (binary_suffix().is_none() && BUILD_PROFILE == "stage")
}

/// Runtime instance label for the titlebar badge.
/// Returns "STAGE", "STANDALONE", etc. or empty string for prod (no badge).
pub fn instance_label() -> &'static str {
    static LABEL: OnceLock<String> = OnceLock::new();
    LABEL.get_or_init(|| match binary_suffix() {
        Some(suffix) => suffix.to_uppercase(),
        None => match BUILD_PROFILE {
            "stage" => "STAGE".to_string(),
            _ => String::new(), // prod and dev-via-Vite (DEV badge handles that)
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutex_name_is_nul_terminated_and_scoped() {
        // #786 T9: the probe encodes exactly this name, including the embedded
        // trailing NUL. If this shape ever changes, gui_instance_running() would
        // silently open a different name and defeat the whole safe-write design.
        let name = mutex_name();
        assert!(name.ends_with('\0'), "mutex name must be NUL-terminated");
        assert!(
            name.starts_with("Local\\AgentsCommander_SingleInstance"),
            "unexpected mutex name shape: {name:?}"
        );
    }

    /// #786 T9: in-process round-trip. Create the single-instance mutex under the
    /// SAME `mutex_name()` the acquirer uses, assert the probe detects it, then
    /// close it and assert it no longer detects it. Windows-only (the probe is a
    /// no-op false off-Windows). Nothing else holds this name during `cargo test`
    /// (the test exe's suffix scopes the name), but the negative assertion is
    /// guarded on the pre-existing state to stay robust under parallel tests.
    #[cfg(target_os = "windows")]
    #[test]
    fn gui_instance_running_probe_roundtrip() {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let was_running_before = gui_instance_running();

        let name: Vec<u16> = mutex_name().encode_utf16().collect();
        // SAFETY: `name` is a NUL-terminated UTF-16 buffer valid for the call;
        // the returned handle is closed below.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        assert!(!handle.is_null(), "CreateMutexW failed");

        // The named mutex now exists, so the OpenMutexW probe must see it.
        assert!(
            gui_instance_running(),
            "probe should detect the held single-instance mutex"
        );

        // SAFETY: `handle` is a valid mutex handle from the CreateMutexW above.
        unsafe {
            let _ = CloseHandle(handle);
        }

        // Once our handle is closed and nothing else held the name, the probe
        // must no longer detect it.
        if !was_running_before {
            assert!(
                !gui_instance_running(),
                "probe should not detect the mutex after it is released"
            );
        }
    }
}
