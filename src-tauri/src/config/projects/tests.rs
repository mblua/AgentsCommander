use super::*;
use crate::config::settings::AppSettings;
use std::path::Path;

/// Auto-cleaned temp dir; mirrors `cli::task_ops::tests::FixtureRoot`.
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
                .map_or(0, |d| d.as_nanos()),
            h.finish()
        ));
        std::fs::create_dir_all(&path).expect("fixture root");
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

// ── register_existing_project ────────────────────────────────────────

/// CWD is process-wide; restore on Drop. Tests that change it run in
/// their own child process (`crate::test_support::run_case`).
struct CwdGuard(PathBuf);

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

fn create_ac_project(path: &Path) {
    std::fs::create_dir_all(path.join(".ac")).unwrap();
}

fn register_new_project_with_settings_path(
    settings: &mut AppSettings,
    raw_path: &str,
    settings_path: &Path,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
) -> Result<ProjectRegistration, ProjectError> {
    register_new_project_with_store(
        settings,
        raw_path,
        NewProjectSettingsStore::Path(settings_path.to_path_buf()),
        activation,
    )
}

mod open_new;
mod paths_resolve;
mod registry;

// ── instance-relative codec ────────────────────────────────────

#[cfg(not(windows))]
mod codec_posix;
#[cfg(windows)]
mod codec_windows;
mod resolution;
