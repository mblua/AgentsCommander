use std::io;
use std::path::Path;

pub(crate) fn read_team_config_bytes(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn team_directory_present(team_dir: &Path) -> bool {
    team_dir.exists()
}

pub(crate) fn ensure_team_directory(team_dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(team_dir)
}

pub(crate) fn ensure_team_memory_directory(team_dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(team_dir.join("memory"))
}

pub(crate) fn create_team_directory(team_dir: &Path) -> io::Result<()> {
    std::fs::create_dir(team_dir)
}

pub(crate) fn create_team_memory_directory(team_dir: &Path) -> io::Result<()> {
    std::fs::create_dir(team_dir.join("memory"))
}

pub(crate) fn team_conventions_present(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn write_empty_team_conventions(path: &Path) -> io::Result<()> {
    std::fs::write(path, "")
}

pub(crate) fn publish_team_config<P>(path: &Path, bytes: &[u8], publish: P) -> Result<(), String>
where
    P: FnOnce(&Path, &[u8]) -> Result<(), String>,
{
    publish(path, bytes)
}

pub(crate) fn write_created_role(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

pub(crate) fn link_owned_team_config(temporary: &Path, config: &Path) -> io::Result<()> {
    std::fs::hard_link(temporary, config)
}

pub(crate) fn unlink_owned_team_temporary(temporary: &Path) -> io::Result<()> {
    std::fs::remove_file(temporary)
}
