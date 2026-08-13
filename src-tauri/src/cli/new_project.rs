//! `new-project <PATH>` CLI verb — ensure an AC project structure at PATH
//! (creating `.ac/` if missing) and register it in
//! `settings.project_paths`. Shares the registration logic with the Tauri
//! command at `commands::ac_discovery::new_project` via the
//! `config::projects` module.
//!
//! Same GUI concurrency caveat as `open-project` — see that file.

use clap::Args;
use std::path::{Path, PathBuf};

use crate::cli::workgroup::write_project_registration_refresh;
use crate::config::projects::register_new_project;
use crate::config::settings::{
    load_settings_for_cli_strict, project_state_has_structural, refresh_project_paths_from_disk,
    resync_project_state_from_runtime, save_settings_with_project_paths,
};

#[derive(Args)]
#[command(after_help = "\
PURPOSE: Create an AC project at PATH (mkdir-p `.ac/` and write its \
`.gitignore` if no Project AC Root exists) and register it in the GUI sidebar's project list.\n\n\
PATH: Absolute or relative — relative paths are resolved against the current \
working directory. The folder is created if it does not yet exist. The \
registration is persisted both as the canonical absolute path and as a portable \
path relative to the AgentsCommander executable's directory (so the project \
relocates with the install folder); a project on a different drive or share is \
stored absolute-only.\n\n\
IDEMPOTENCY: Re-running on a folder that already has `.ac/` is safe; \
the selected Project AC Root gitignore is swept (missing patterns appended), and the registration step \
deduplicates against any prior entry.")]
pub struct NewProjectArgs {
    /// Path to make into an AC project (folder created if missing)
    #[arg(value_name = "PATH")]
    pub path: String,
}

pub fn execute(args: NewProjectArgs) -> i32 {
    // #786/#1077: strict CLI loader so we never trigger a spurious root_token
    // write, and we refuse to touch a present-but-unparseable settings.json.
    let mut settings = match load_settings_for_cli_strict() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    // #1077: reject structural project-metadata corruption BEFORE any filesystem
    // effect (directory / .ac creation), so a corrupt settings file never causes
    // a half-registered project.
    if project_state_has_structural(&settings) {
        eprintln!(
            "Error: settings.json has malformed project metadata; refusing to modify the project list. Fix or remove the corrupt project fields first."
        );
        return 1;
    }
    // #1077: reconcile the runtime list from the RAW disk fields (before any
    // filesystem effect) so a stored-but-missing project the strict decode
    // filters out is preserved in place by the resync rather than dropped.
    if let Err(e) = refresh_project_paths_from_disk(&mut settings) {
        eprintln!("Error: {}", e);
        return 1;
    }
    let result = match register_new_project(&mut settings, &args.path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    // Save when we either created `.ac` or appended a new path entry.
    // (A pure no-op call still prints the status lines.)
    if result.created || result.registered {
        // #1077: rebuild the hidden pair state from the mutated runtime lists so
        // the paired reconcile serializer records both persisted forms instead of
        // dropping the append.
        resync_project_state_from_runtime(&mut settings);
        if let Err(e) = save_settings_with_project_paths(&settings) {
            eprintln!("Error: failed to persist settings: {}", e);
            return 1;
        }
        // #1318 - a CLI registration seeds the catalog + masters immediately
        // (no restart needed). Fail-soft: any error is logged and the boot loop
        // covers the project at the next boot.
        crate::config::coding_agents_catalog::ensure_seeded_for_project(Path::new(&result.path));
        write_project_registration_refresh(&PathBuf::from(&result.path), "projectRegistered");
    }
    if result.created {
        crate::cli_println!("Created AC project at {}", result.path);
    } else {
        crate::cli_println!("AC project already exists at {}", result.path);
    }
    if result.registered {
        crate::cli_println!("Registered project: {}", result.path);
    } else {
        crate::cli_println!("Project already registered: {}", result.path);
    }
    log::info!(
        "[cli] new-project: path={} created={} registered={}",
        result.path,
        result.created,
        result.registered
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    struct FixtureRoot(PathBuf);
    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl FixtureRoot {
        fn new(prefix: &str) -> Self {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::process::id().hash(&mut h);
            std::thread::current().id().hash(&mut h);
            let path = std::env::temp_dir().join(format!(
                "{}-{}-{}",
                prefix,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                h.finish()
            ));
            std::fs::create_dir_all(&path).expect("fixture root");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    #[test]
    fn new_project_returns_1_when_path_is_a_file() {
        let fix = FixtureRoot::new("cli-new-isfile");
        let f = fix.path().join("note.txt");
        std::fs::write(&f, b"x").unwrap();
        let code = execute(NewProjectArgs {
            path: f.to_string_lossy().into(),
        });
        assert_eq!(code, 1);
    }

    #[test]
    fn help_text_documents_new_project() {
        use clap::CommandFactory;
        let help = crate::cli::Cli::command().render_help().to_string();
        assert!(help.contains("new-project"), "help missing verb: {}", help);
    }
}
