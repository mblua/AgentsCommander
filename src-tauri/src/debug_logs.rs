//! Owner of the textual debug log dump, excluding buffering and transport.

use std::path::PathBuf;

use crate::config::instance_artifacts::DEBUG_LOGS_FILE_NAME;

pub(crate) async fn save_debug_logs(
    config_dir: Option<PathBuf>,
    content: &str,
) -> Result<PathBuf, String> {
    let path = config_dir
        .ok_or("No config dir")?
        .join(DEBUG_LOGS_FILE_NAME);
    crate::filesystem::debug_logs::write_debug_log_dump(&path, content)
        .await
        .map_err(|e| format!("Failed to write logs: {}", e))?;
    Ok(path)
}

#[cfg(test)]
mod tests;
