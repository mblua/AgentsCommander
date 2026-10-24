//! Instance-label derivation: runtime binary name > compile-time BUILD_PROFILE.
//!
//! Convention: `agentscommander_<suffix>.exe` -> label = `<SUFFIX>` uppercased.
//! - `agentscommander_stage.exe`  -> [STAGE]
//! - `agentscommander_test-1.exe` -> [TEST-1]
//! - `agentscommander.exe` (no underscore) -> prod (no badge)
//!
//! BUILD_PROFILE (set by build.rs) is the fallback for titles, badges, ports
//! and the single-instance mutex when the binary name has no underscore
//! suffix. It does NOT influence the configuration directory name (#1868): an
//! unsuffixed executable always maps to the canonical `.agentscommander`.

use std::path::Path;
use std::sync::OnceLock;

/// Build profile string — "dev", "prod", or "stage".
/// Used as fallback for titles, badges, ports and the mutex name when the
/// binary name has no underscore suffix. Never consulted for the config
/// directory name (#1868).
pub const BUILD_PROFILE: &str = env!("BUILD_PROFILE");

/// Extract suffix from binary name: `agentscommander_foo` -> Some("foo").
/// Returns None for plain `agentscommander` (no underscore = prod).
/// First-underscore parsing: `agentscommander_` is `Some("")` and hyphens are
/// never suffixes. `pub(super)` so the parent `config` resolver can route on
/// the same parser instead of growing a second one (#1868).
pub(super) fn binary_suffix_from_path(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .and_then(|name| name.find('_').map(|index| name[index + 1..].to_string()))
}

/// Extract the suffix from the running executable once per process.
/// Cached via OnceLock — parsed once at first call.
fn binary_suffix() -> Option<&'static str> {
    static SUFFIX: OnceLock<Option<String>> = OnceLock::new();
    SUFFIX
        .get_or_init(|| {
            std::env::current_exe()
                .ok()
                .and_then(|path| binary_suffix_from_path(&path))
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

/// The configuration directory name under $HOME.
///
/// #1868: an executable without an underscore suffix (or no usable executable
/// at all) selects `$HOME/.agentscommander`, independently of BUILD_PROFILE,
/// install location and any portable marker. #1930: this is the only HOME
/// name. A suffixed executable `agentscommander_<suffix>` never selects a
/// HOME directory. This is internal storage, not release identity.
pub fn config_dir_name() -> &'static str {
    ".agentscommander"
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

/// Title suffix appended to secondary windows (spec board, etc.).
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
                "stage" => "agentscommander-stage.exe".to_string(),
                _ => "agentscommander.exe".to_string(),
            })
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

/// Default control-plane API server port (#791). Distinct from the web server
/// port so a dev and a prod build on one host do not collide. Mirrors
/// `web_server_port`'s profile-awareness (dev/stage hardcoded, unknown suffix
/// hashed, prod fallback), offset into the 98xx range above the web ports.
pub fn api_server_port() -> u16 {
    match binary_suffix() {
        Some("dev") => 9886,
        Some("stage") => 9888,
        Some(suffix) => {
            let hash = suffix
                .bytes()
                .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
            9900 + (hash % 20) as u16
        }
        None => match BUILD_PROFILE {
            "dev" => 9886,
            "stage" => 9888,
            _ => 9887, // prod
        },
    }
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
mod tests;
