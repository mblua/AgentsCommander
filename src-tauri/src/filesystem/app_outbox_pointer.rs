//! The `app-outbox-path.txt` pointer in the config directory: `get` reads it and
//! returns the outbox only if it is an existing directory, `directory` falls back to
//! `<agent_dir>/outbox`, and `publish` writes the pointer value.

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

pub(crate) fn publish(config_dir: &std::path::Path, value: &str) -> std::io::Result<()> {
    std::fs::write(config_dir.join(BASENAME), value)
}

#[cfg(test)]
mod tests;
