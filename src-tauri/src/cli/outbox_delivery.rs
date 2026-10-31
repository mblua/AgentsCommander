use std::path::Path;
use std::time::{Duration, Instant};

use crate::filesystem::{directories, documents};
use crate::phone::types::OutboxMessage;

pub(crate) enum ConfirmationError {
    Rejected(String),
    Timeout,
}

pub(crate) fn confirm(outbox: &Path, id: &str) -> Result<(), ConfirmationError> {
    let delivered = outbox.join("delivered").join(format!("{id}.json"));
    let rejected = outbox.join("rejected").join(format!("{id}.reason.txt"));
    let start = Instant::now();
    while !directories::cli_outbox_artifact_exists(&delivered) {
        if directories::cli_outbox_artifact_exists(&rejected) {
            let reason = documents::read_cli_outbox_text(&rejected)
                .unwrap_or_else(|_| "unknown reason".to_string());
            return Err(ConfirmationError::Rejected(reason.trim().to_string()));
        }
        if start.elapsed() >= Duration::from_secs(30) {
            return Err(ConfirmationError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

pub(crate) fn deliver(
    outbox: &Path,
    message: &OutboxMessage,
    action: &str,
) -> Result<(), String> {
    directories::ensure_cli_outbox(outbox)
        .map_err(|e| format!("failed to create outbox directory: {e}"))?;
    let file = outbox.join(format!("{}.json", message.id));
    let json = serde_json::to_string_pretty(message)
        .map_err(|e| format!("failed to serialize message: {e}"))?;
    documents::write_cli_outbox_message(&file, json.as_bytes())
        .map_err(|e| format!("failed to write outbox file: {e}"))?;
    confirm(outbox, &message.id).map_err(|error| match error {
        ConfirmationError::Rejected(reason) => format!("{action} rejected - {reason}"),
        ConfirmationError::Timeout => format!(
            "delivery confirmation timeout after 30s (request {} may still be pending)",
            message.id
        ),
    })
}

#[cfg(test)]
mod tests;
