//! `open-project <PATH>` CLI verb — validate an existing AC project and
//! register it in `settings.project_paths`. Shares the registration logic
//! with the Tauri command at `commands::ac_discovery::open_project` via the
//! `config::projects` module.
//!
//! No `--token` requirement: project registration mutates the user-local
//! `settings.json`, which any process with shell access can already write to.
//! Adding token gating would not change the security boundary; it would only
//! diverge the CLI UX from `git init` / `npm init`.
//!
//! GUI concurrency caveat: when AC's GUI is running, its in-memory
//! `SettingsState` is the source of truth. A subsequent GUI `update_settings`
//! built from a stale snapshot can clobber a CLI-registered entry. Documented
//! in the plan §6 — a watcher/reload story is a follow-up issue.

use clap::Args;

use crate::config::projects::{register_existing_project, ProjectError};
use crate::config::settings::{load_settings_for_cli, save_settings};

#[derive(Args)]
#[command(after_help = "\
PURPOSE: Register an existing AC project so it appears in the GUI sidebar on \
next launch. The folder must already contain `.ac/` (use `new-project` to \
create one).\n\n\
PATH: Absolute or relative — relative paths are resolved against the current \
working directory. The persisted entry is the absolute form.\n\n\
IDEMPOTENCY: Re-registering the same path is a no-op; the verb prints \
\"Project already registered\" and exits 0.")]
pub struct OpenProjectArgs {
    /// Path to an existing AC project folder (must contain `.ac/`)
    #[arg(value_name = "PATH")]
    pub path: String,
}

pub fn execute(args: OpenProjectArgs) -> i32 {
    // Use the CLI-specific loader (Round-1 G5): unlike `load_settings`, this
    // does NOT auto-generate or persist a `root_token`, so error paths and
    // pre-validation reads do not silently rewrite settings.json.
    let mut settings = load_settings_for_cli();
    let result = match register_existing_project(&mut settings, &args.path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            // Append CLI-specific guidance when the user pointed at a folder
            // without an AC workspace. The bare error string is GUI-friendly
            // (Round-1 G8); only the CLI knows about `new-project`.
            if matches!(e, ProjectError::WorkspaceMissing(_)) {
                eprintln!("Hint: use `new-project <PATH>` to create the .ac structure.");
            }
            return 1;
        }
    };
    if result.registered {
        if let Err(e) = save_settings(&settings) {
            eprintln!("Error: failed to persist settings: {}", e);
            return 1;
        }
        crate::cli_println!("Registered project: {}", result.path);
    } else {
        crate::cli_println!("Project already registered: {}", result.path);
    }
    log::info!(
        "[cli] open-project: path={} registered={}",
        result.path,
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

    // The CLI execute() touches the live settings.json on disk. These two
    // tests exercise the arg parsing + early-error paths only — the
    // persistence-mutating success paths are covered by `config::projects`
    // unit tests, which use an in-memory AppSettings.

    #[test]
    fn open_project_returns_1_when_path_missing() {
        let fix = FixtureRoot::new("cli-open-missing");
        let bogus = fix.path().join("does-not-exist");
        let code = execute(OpenProjectArgs {
            path: bogus.to_string_lossy().into(),
        });
        assert_eq!(code, 1);
    }

    #[test]
    fn open_project_returns_1_when_no_ac() {
        let fix = FixtureRoot::new("cli-open-noac");
        let code = execute(OpenProjectArgs {
            path: fix.path().to_string_lossy().into(),
        });
        assert_eq!(code, 1);
    }

    #[test]
    fn help_text_documents_open_project() {
        use clap::CommandFactory;
        let help = crate::cli::Cli::command().render_help().to_string();
        assert!(help.contains("open-project"), "help missing verb: {}", help);
    }
}
