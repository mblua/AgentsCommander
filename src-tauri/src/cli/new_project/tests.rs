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
