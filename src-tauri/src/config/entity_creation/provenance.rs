//! Copy provenance format, validation and history.
use super::names::validate_existing_name;
use crate::filesystem::projects::entities::copies::{
    self as disk, MatrixCopyHooks, MatrixCopyStage, OwnedCopyArtifacts,
};
use std::path::Path;

// Strict Matrix copy and team publication share ephemeral output ownership.
pub(crate) const COPY_PROVENANCE_FILE: &str = ".ac-copy-provenance.json";

pub(crate) fn validate_copy_provenance(
    history: Option<&[u8]>,
) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = match history {
        Some(bytes) => {
            serde_json::from_slice(bytes).map_err(|e| format!("Invalid copy provenance: {e}"))?
        }
        None => serde_json::json!({"schemaVersion": 1, "copies": []}),
    };
    if value.get("schemaVersion").and_then(|v| v.as_u64()) != Some(1) {
        return Err("Unsupported copy provenance schemaVersion".into());
    }
    let copies = value
        .get("copies")
        .and_then(|v| v.as_array())
        .ok_or("Invalid copy provenance copies")?;
    for event in copies {
        for key in [
            "sourceProjectName",
            "sourceProjectPath",
            "sourceAgentRef",
            "copiedAt",
        ] {
            if event
                .get(key)
                .and_then(|v| v.as_str())
                .is_none_or(|v| v.trim().is_empty())
            {
                return Err(format!("Invalid copy provenance {key}"));
            }
        }
        let reference = event["sourceAgentRef"].as_str().expect("validated string");
        let bare = reference
            .strip_prefix("_agent_")
            .ok_or("Invalid provenance agent ref")?;
        validate_existing_name(bare, "Agent")?;
        if reference.contains(['/', '\\', ':', '\0']) {
            return Err("Invalid provenance agent ref".into());
        }
        let time = chrono::DateTime::parse_from_rfc3339(
            event["copiedAt"].as_str().expect("validated string"),
        )
        .map_err(|e| format!("Invalid copy provenance time: {e}"))?;
        if time.offset().local_minus_utc() != 0 {
            return Err("Copy provenance time must be UTC".into());
        }
    }
    Ok(value)
}

pub(crate) fn append_copy_provenance(
    mut value: serde_json::Value,
    source_project: &Path,
    source_ref: &str,
) -> Result<serde_json::Value, String> {
    let name = source_project
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("Invalid source project name")?;
    value["copies"]
        .as_array_mut()
        .ok_or("Invalid copy provenance copies")?
        .push(serde_json::json!({
            "sourceProjectName": name,
            "sourceProjectPath":
                crate::path_utils::path_to_string_without_windows_verbatim_prefix(source_project),
            "sourceAgentRef": source_ref,
            "copiedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        }));
    Ok(value)
}

pub(crate) fn read_copy_provenance(root: &Path) -> Result<serde_json::Value, String> {
    let history = disk::read_copy_history(root, Path::new(COPY_PROVENANCE_FILE))?;
    validate_copy_provenance(Some(&history))
}

pub(crate) fn write_copy_provenance(
    owned: &mut OwnedCopyArtifacts,
    provenance: &serde_json::Value,
    hooks: &mut MatrixCopyHooks<'_>,
) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(provenance).map_err(|e| e.to_string())?;
    hooks.run(MatrixCopyStage::Provenance, &owned.root)?;
    let root = owned.root.clone();
    let mut output = owned.create_payload(Path::new(COPY_PROVENANCE_FILE))?;
    output.write_payload(&bytes, "Provenance write made no progress", || {
        hooks.run(MatrixCopyStage::ProvenanceWritten, &root)
    })
}
