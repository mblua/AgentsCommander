//! `new-project <PATH>` CLI verb - ensure an AC project structure at PATH
//! (creating `.ac-new/` if missing) and register it in
//! `settings.project_paths`. Shares the registration logic with the Tauri
//! command at `commands::ac_discovery::new_project` via the
//! `config::projects` module.
//!
//! Same GUI concurrency caveat as `open-project` - see that file.

use clap::Args;

use crate::config::projects::register_new_project;
use crate::config::settings::{load_settings_for_cli, save_settings};

#[derive(Args)]
#[command(after_help = "\
PURPOSE: Create an AC project at PATH (mkdir-p `.ac-new/` and write its \
`.gitignore` if missing) and register it in the GUI sidebar's project list.\n\n\
PATH: Absolute or relative - relative paths are resolved against the current \
working directory. The folder is created if it does not yet exist.\n\n\
IDEMPOTENCY: Re-running on a folder that already has `.ac-new/` is safe - the \
gitignore is swept (missing patterns appended), and the registration step \
deduplicates against any prior entry.")]
pub struct NewProjectArgs {
    /// Path to make into an AC project (folder created if missing)
    #[arg(value_name = "PATH")]
    pub path: String,
}

pub fn execute(args: NewProjectArgs) -> i32 {
    // Round-1 G5: use the CLI-specific loader so we never trigger a spurious
    // root_token write on first-boot or error-path invocations.
    let mut settings = load_settings_for_cli();
    let result = match register_new_project(&mut settings, &args.path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    // Save when we either created `.ac-new` or appended a new path entry.
    // (A pure no-op call still prints the status lines.)
    if result.created || result.registered {
        if let Err(e) = save_settings(&settings) {
            eprintln!("Error: failed to persist settings: {}", e);
            return 1;
        }
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
