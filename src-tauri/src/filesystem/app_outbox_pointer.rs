use std::path::{Path, PathBuf};

use super::{directories, documents};

const BASENAME: &str = "app-outbox-path.txt";

pub(crate) fn get(config_dir: Option<&Path>) -> Option<PathBuf> {
    let path = config_dir?.join(BASENAME);
    let text = documents::read_cli_outbox_text(&path).ok()?;
    let candidate = PathBuf::from(text.trim());
    if directories::cli_agent_root_is_dir(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

pub(crate) fn directory(config_dir: Option<&Path>, agent_dir: &Path) -> PathBuf {
    get(config_dir).unwrap_or_else(|| agent_dir.join("outbox"))
}

#[cfg(test)]
mod tests;
