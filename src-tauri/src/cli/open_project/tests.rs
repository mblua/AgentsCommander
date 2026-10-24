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
