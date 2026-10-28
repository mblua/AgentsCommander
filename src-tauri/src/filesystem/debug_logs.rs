//! Physical writes of debug log dumps, excluding path and filename selection.

use std::path::Path;

pub(crate) async fn write_debug_log_dump(path: &Path, content: &str) -> std::io::Result<()> {
    tokio::fs::write(path, content).await
}
