use crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME;
use crate::config::local_overlay::{DerivedIdClosure, LocalSettingsOverlay, PairedLeaf};
use serde_json::Value;
use std::path::Path;

/// Resolve the personal overlay beside the main settings file at the owner boundary.
pub(super) fn load_settings_overlay(
    settings_path: &Path,
    base: &mut Value,
    ineligible_disk: &[&str],
    ineligible_legacy: &[&str],
    derived: &[DerivedIdClosure],
    pairs: &[PairedLeaf],
) -> LocalSettingsOverlay {
    let overlay_path = settings_path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
    LocalSettingsOverlay::load_and_merge_overlay_path_paired(
        &overlay_path,
        base,
        ineligible_disk,
        ineligible_legacy,
        derived,
        pairs,
    )
}
