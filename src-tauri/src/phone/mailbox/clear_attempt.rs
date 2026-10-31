//! Mailbox-only durable no-replay boundary. A marker never proves successful delivery.

use crate::filesystem::phone_mailbox as mailbox_disk;
use crate::phone::types::{OutboxMessage, PTY_INPUT_HOST_ENVELOPE_MAX_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use uuid::Uuid;

const MEMBER: &str = "_acCodexClearAttempt";
pub(crate) const REPLAY_REASON: &str =
    "codex_clear_execution_error: prior attempt outcome unavailable; not resubmitted";

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ClearAttempt {
    version: u32,
    message_id: String,
    session_id: Uuid,
}

#[derive(Debug)]
pub(crate) enum AttemptError {
    InvalidDocument,
    Inspection(String),
    Io(std::io::Error),
}

impl std::fmt::Display for AttemptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDocument => formatter.write_str("invalid Codex clear attempt document"),
            Self::Inspection(error) => write!(formatter, "clear attempt inspection: {error}"),
            Self::Io(error) => write!(formatter, "clear attempt persistence: {error}"),
        }
    }
}

impl std::error::Error for AttemptError {}

impl From<std::io::Error> for AttemptError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Absence alone permits ordinary processing. Any malformed reserved member is terminal.
pub(crate) fn probe_attempt(content: &str) -> Result<Option<ClearAttempt>, AttemptError> {
    let document: Value =
        serde_json::from_str(content).map_err(|_| AttemptError::InvalidDocument)?;
    let Some(marker) = document.get(MEMBER) else {
        return Ok(None);
    };
    let attempt: ClearAttempt =
        serde_json::from_value(marker.clone()).map_err(|_| AttemptError::InvalidDocument)?;
    if attempt.version != 1
        || document.get("id").and_then(Value::as_str) != Some(&attempt.message_id)
    {
        return Err(AttemptError::InvalidDocument);
    }
    Ok(Some(attempt))
}

/// Called only after authorization and while the delivery owns the source claim.
pub(crate) fn persist_attempt(
    source: &Path,
    authorized: &OutboxMessage,
    session_id: Uuid,
) -> Result<(), AttemptError> {
    let parent = source.parent().ok_or(AttemptError::InvalidDocument)?;
    let parent_identity =
        crate::path_identity::verify_directory(parent).map_err(AttemptError::Inspection)?;
    let (bytes, identity) =
        crate::path_identity::read_bounded_regular(source, PTY_INPUT_HOST_ENVELOPE_MAX_BYTES)
            .map_err(AttemptError::Inspection)?;
    let content = decode_document(&bytes)?;
    if probe_attempt(&content)?.is_some() {
        return Err(AttemptError::InvalidDocument);
    }
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(AttemptError::InvalidDocument)?;
    let logical_name = name.strip_suffix(".in-flight").unwrap_or(name);
    if logical_name.strip_suffix(".json") != Some(authorized.id.as_str()) {
        return Err(AttemptError::InvalidDocument);
    }
    let mut document: Value =
        serde_json::from_str(&content).map_err(|_| AttemptError::InvalidDocument)?;
    let actual: OutboxMessage =
        serde_json::from_value(document.clone()).map_err(|_| AttemptError::InvalidDocument)?;
    if actual.id != authorized.id
        || actual.to != authorized.to
        || actual.command != authorized.command
    {
        return Err(AttemptError::InvalidDocument);
    }
    document[MEMBER] = serde_json::to_value(ClearAttempt {
        version: 1,
        message_id: authorized.id.clone(),
        session_id,
    })
    .map_err(|_| AttemptError::InvalidDocument)?;
    let marked = serde_json::to_vec(&document).map_err(|_| AttemptError::InvalidDocument)?;
    if marked.len() > PTY_INPUT_HOST_ENVELOPE_MAX_BYTES {
        return Err(AttemptError::InvalidDocument);
    }
    let stage = parent.join(format!(".{name}.{}.codex-clear-stage", Uuid::new_v4()));
    // Once create_new succeeds, this writer owns exactly this stage, including on error.
    let mut file = mailbox_disk::open_pty_metadata_stage(&stage)?;
    let result = (|| {
        mailbox_disk::write_pty_metadata_stage(&mut file, &marked)?;
        crate::path_identity::verify_opened_regular_file(&stage, &file, false)
            .map_err(AttemptError::Inspection)?;
        let current_parent =
            crate::path_identity::verify_directory(parent).map_err(AttemptError::Inspection)?;
        let (current_bytes, current) =
            crate::path_identity::read_bounded_regular(source, PTY_INPUT_HOST_ENVELOPE_MAX_BYTES)
                .map_err(AttemptError::Inspection)?;
        if !crate::path_identity::same_object(&parent_identity, &current_parent)
            || !crate::path_identity::same_object(&identity, &current)
            || current_bytes != bytes
        {
            return Err(AttemptError::InvalidDocument);
        }
        mailbox_disk::rename_mailbox_document(&stage, source)?;
        if let Some(result) = mailbox_disk::sync_pty_metadata_parent(parent) {
            result?;
        }
        Ok(())
    })();
    drop(file);
    if result.is_err() {
        if let Err(error) = mailbox_disk::remove_mailbox_artifact(&stage) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!("[mailbox] clear attempt stage cleanup failed: {error}");
            }
        }
    }
    result
}

fn decode_document(bytes: &[u8]) -> Result<String, AttemptError> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let body = &bytes[2..];
        if !body.len().is_multiple_of(2) {
            return Err(AttemptError::InvalidDocument);
        }
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| {
                if bytes[0] == 0xff {
                    u16::from_le_bytes(*chunk)
                } else {
                    u16::from_be_bytes(*chunk)
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| AttemptError::InvalidDocument)
    } else {
        let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        String::from_utf8(body.to_vec()).map_err(|_| AttemptError::InvalidDocument)
    }
}

#[cfg(test)]
mod tests;
