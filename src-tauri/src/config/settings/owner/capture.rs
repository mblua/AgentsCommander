use super::{
    owner_save_error_text, CapturePolicy, SettingsOperationGuard, SettingsOwnerCounters,
    SettingsOwnerError,
};
use crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME;
use crate::config::local_overlay::LocalSettingsOverlay;
use crate::config::settings::{
    agents_instance_path, compose_settings_parts, io_error_kind_as_str,
    parse_agents_instance_contents, project_paths_path, raw_project_source_from_bytes,
    repair_loaded_response_close_settings, repair_loaded_room_idle_confirmation_settings,
    select_project_source_from_raw, AppSettings, SettingsSaveError, CONFIGURATION_COMMAND_PAIRS,
    OVERLAY_DERIVED_ID_CLOSURES, OVERLAY_INELIGIBLE_DISK_KEYS, OVERLAY_INELIGIBLE_LEGACY_KEYS,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

/// One captured source: verified absence, or its bytes with the identity witness
/// of the object they were read from.
#[derive(Debug, Clone)]
pub(crate) enum CapturedSource {
    Absent,
    Present {
        bytes: Vec<u8>,
        witness: crate::path_identity::VerifiedPathIdentity,
    },
}

impl CapturedSource {
    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Absent => None,
            Self::Present { bytes, .. } => Some(bytes),
        }
    }

    /// A short, non-reversible witness label for DEBUG diagnostics.
    pub(crate) fn witness_label(&self) -> String {
        match self {
            Self::Absent => "absent".to_string(),
            Self::Present { witness, .. } => match witness.content_sha256 {
                Some(digest) => digest[..4]
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                None => format!("len{}", witness.metadata.len),
            },
        }
    }
}

/// The four settings sources, captured once under the transactional lock.
#[derive(Debug, Clone)]
pub(crate) struct CapturedSettingsSources {
    pub(crate) settings_path: PathBuf,
    pub(crate) base: CapturedSource,
    pub(crate) project_paths: CapturedSource,
    pub(crate) agents: CapturedSource,
    pub(crate) overlay: CapturedSource,
}

pub(crate) const SETTINGS_CAPTURE_LIMIT: usize = 16 * 1024 * 1024;

pub(crate) fn capture_source_file(
    path: &Path,
    counters: &SettingsOwnerCounters,
) -> Result<CapturedSource, SettingsOwnerError> {
    counters.source_reads.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CapturedSource::Absent);
        }
        Err(error) => {
            let kind = io_error_kind_as_str(error.kind());
            return Err(SettingsOwnerError::Capture(format!("{name}: {kind}")));
        }
    }
    // A source that vanishes after the metadata probe is a race, not absence.
    crate::path_identity::read_bounded_regular(path, SETTINGS_CAPTURE_LIMIT)
        .map(|(bytes, witness)| CapturedSource::Present { bytes, witness })
        .map_err(|reason| SettingsOwnerError::Capture(format!("{name}: {reason}")))
}

// Test seam: the next owner capture on this thread panics, so the panic is
// raised outside the builder while the operation's guards are held.
#[cfg(test)]
thread_local! {
    pub(crate) static OWNER_CAPTURE_PANIC: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Captures base, project-paths, agents and overlay exactly once while the guard
/// holds the transactional lock. No directory creation, migration or write.
pub(crate) fn capture_sources_locked(
    guard: &SettingsOperationGuard<'_>,
) -> Result<CapturedSettingsSources, SettingsOwnerError> {
    guard.counters.captures.fetch_add(1, Ordering::Relaxed);
    #[cfg(test)]
    OWNER_CAPTURE_PANIC.with(|armed| {
        if armed.replace(false) {
            panic!("injected settings owner capture panic");
        }
    });
    let path = guard.settings_path();
    let captured = CapturedSettingsSources {
        settings_path: path.to_path_buf(),
        base: capture_source_file(path, guard.counters)?,
        project_paths: capture_source_file(&project_paths_path(path), guard.counters)?,
        agents: capture_source_file(&agents_instance_path(path), guard.counters)?,
        overlay: capture_source_file(
            &path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
            guard.counters,
        )?,
    };
    guard.lease.verify()?;
    log::debug!(
        "[settings-owner] captured base={} project={} agents={} overlay={}",
        captured.base.witness_label(),
        captured.project_paths.witness_label(),
        captured.agents.witness_label(),
        captured.overlay.witness_label()
    );
    Ok(captured)
}

pub(crate) fn recover_composition(
    captured: &CapturedSettingsSources,
) -> Result<AppSettings, SettingsOwnerError> {
    compose_captured_sources(captured, CapturePolicy::Recovery)
}

/// Composes captured bytes through the same selector and parser core the loaders
/// use. No second read, no defaults for a present source that fails.
pub(crate) fn compose_captured_sources(
    captured: &CapturedSettingsSources,
    policy: CapturePolicy,
) -> Result<AppSettings, SettingsOwnerError> {
    let path = captured.settings_path.as_path();
    let destination_path = project_paths_path(path);
    let selection_error = |error: SettingsSaveError| {
        SettingsOwnerError::Compose(format!("selection: {}", owner_save_error_text(&error)))
    };
    let raw = |source: &CapturedSource, label: &Path, legacy: bool| {
        source
            .bytes()
            .map(|bytes| raw_project_source_from_bytes(bytes.to_vec(), label, label, legacy))
            .transpose()
    };
    let legacy = raw(&captured.base, path, true).map_err(selection_error)?;
    let destination = raw(&captured.project_paths, &destination_path, false);
    let destination = destination.map_err(selection_error)?;
    let selected = select_project_source_from_raw(
        legacy,
        destination,
        path,
        &destination_path,
        &destination_path,
    )
    .map_err(selection_error)?;
    let agents_label = agents_instance_path(path);
    let agents = match captured.agents.bytes() {
        None => Ok(None),
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| format!("{} is not valid UTF-8", agents_label.display()))
            .and_then(|contents| parse_agents_instance_contents(contents, &agents_label))
            .map(Some),
    };
    if policy == CapturePolicy::Recovery && agents.is_err() {
        let reason = "agents source is invalid during recovery";
        return Err(SettingsOwnerError::Compose(reason.to_string()));
    }
    let source = path.to_string_lossy();
    // Only a verified-absent base composes over serialized defaults.
    let value = match &selected.legacy {
        Some(legacy) => Value::Object(legacy.object.clone()),
        None => serde_json::to_value(AppSettings::default())
            .map_err(|error| SettingsOwnerError::Compose(format!("defaults: {error}")))?,
    };
    let overlay_bytes = captured.overlay.bytes();
    let (mut settings, _legacy_profiles, agents_layer) = compose_settings_parts(
        value,
        &source,
        Some(path),
        Some(&selected.projection),
        Some(agents),
        |value| match overlay_bytes {
            Some(bytes) => LocalSettingsOverlay::from_overlay_bytes_paired(
                value,
                bytes,
                OVERLAY_INELIGIBLE_DISK_KEYS,
                OVERLAY_INELIGIBLE_LEGACY_KEYS,
                OVERLAY_DERIVED_ID_CLOSURES,
                CONFIGURATION_COMMAND_PAIRS,
            ),
            None => LocalSettingsOverlay::default(),
        },
        None,
    )
    .map_err(|_| SettingsOwnerError::Compose("base settings do not decode".to_string()))?;
    let overlay_rejected = settings.local_overlay_state.rejection().is_some();
    if policy == CapturePolicy::Recovery && overlay_rejected {
        let reason = "overlay rejected during recovery";
        return Err(SettingsOwnerError::Compose(reason.to_string()));
    }
    if let Some(state) = agents_layer {
        settings.agents_layer = state;
    }
    repair_loaded_response_close_settings(&mut settings, &source);
    repair_loaded_room_idle_confirmation_settings(&mut settings, &source);
    Ok(settings)
}
