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
/// Everyone else shares `.agentscommander-new`.
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
/// Runtime suffix: "Agents Commander [SUFFIX]" (uppercased).
/// No suffix: falls back to BUILD_PROFILE behaviour.
pub fn app_title() -> &'static str {
    static TITLE: OnceLock<String> = OnceLock::new();
    TITLE.get_or_init(|| match binary_suffix() {
        Some(suffix) => format!("Agents Commander [{}]", suffix.to_uppercase()),
        None => match BUILD_PROFILE {
            "dev" => "Agents Commander New".to_string(),
            "stage" => "Agents Commander [STAGE]".to_string(),
            _ => "Agents Commander".to_string(),
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
            _ => "Local\\AgentsCommander_SingleInstance\0".to_string(),
        },
    })
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
            "dev" => "Agents Commander New".to_string(),
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
