//! Discovery-side entry points that keep the Project AC Root `.gitignore` complete.
//!
//! Resolves the configured agents' instructions filenames from saved settings (test builds
//! use none) and delegates. The pattern list and the file write live in
//! `config::projects::ac_root_gitignore`; this module holds no state.

use std::path::Path;

/// Ensure the Project AC Root .gitignore exists and contains all required exclusion patterns.
/// Called during project creation, workgroup creation, and opportunistically during discovery.
///
/// #2434 - also ignores every configured agent's instructions filename, read
/// from the saved settings. Callers that already hold an `AppSettings` call
/// [`ensure_ac_root_gitignore_with_names`] directly.
pub(crate) fn ensure_ac_root_gitignore(ac_root: &Path) -> Result<(), String> {
    // Test builds never read the developer's real `settings.json` through this
    // writer: they get the legacy floor rows only.
    #[cfg(not(test))]
    let configured = crate::config::agent_command::managed_instructions_filenames(
        &crate::config::settings::load_settings_for_cli_strict()?,
    );
    #[cfg(test)]
    let configured: Vec<String> = Vec::new();
    ensure_ac_root_gitignore_with_names(ac_root, &configured)
}

/// Ensure root exclusions using the already-resolved instructions filenames.
pub(crate) fn ensure_ac_root_gitignore_with_names(
    ac_root: &Path,
    configured: &[String],
) -> Result<(), String> {
    crate::config::projects::ac_root_gitignore::ensure_ac_root_gitignore_with_names(
        ac_root, configured,
    )
}
