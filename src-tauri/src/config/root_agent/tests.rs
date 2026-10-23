use super::*;

mod agency_migration;
mod config_merge;
mod frozen_snapshots;
mod provisioning;
mod skill_snapshots;
mod tracked_state;

#[cfg(unix)]
fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn try_symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn try_symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

// ---------------------------------------------------------------------------------------
// #909: the frozen snapshot, and the two pins that keep it honest.
// ---------------------------------------------------------------------------------------

/// Look a default up by `dir_name`. Never index `DEFAULT_ROOT_SKILLS` positionally: the four
/// pre-existing assertions do, and that is four too many.
fn default_root_skill(dir_name: &str) -> &'static DefaultRootSkill {
    DEFAULT_ROOT_SKILLS
        .iter()
        .find(|skill| skill.dir_name == dir_name)
        .expect("default skill must be in the table")
}

// ---------------------------------------------------------------------------------------
// #909: the migration. Every enum assertion is paired with a content assertion. The enum
// proves which branch ran; it does not prove no bytes were written. The pair is the proof.
// ---------------------------------------------------------------------------------------

struct AgencyFixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    skills_root: PathBuf,
    path: PathBuf,
}
