use super::super::projects;
use super::super::projects::{
    ProjectPathPersistenceState, ProjectSource, RawJsonField, RawPair, RawStringField, RepairKind,
    ResolvedPair, SideOutcome, SideStatus, StructuralIssue,
};
use super::{
    project_lists_from_projection, refresh_coding_agent_identity_at_locked_path,
    registered_profiles_for_read, replace_project_projection, report_settings_save_error,
    rotate_settings_backups_labeled, write_value_atomic, write_value_atomic_labeled, AppSettings,
    MainWindowDisplayState, SettingsFileLock, SettingsSaveError, SettingsSaveLegacyOutward,
    SettingsSaveReason, SettingsSaveReportSurface, SettingsSaveStage, WindowGeometry,
    AGENTS_INSTANCE_KEYS, CONFIGURATION_COMMAND_PAIRS, SAVE_OP_ID,
};
#[cfg(test)]
use super::{project_state_has_structural, save_settings_value_locked};
use crate::config::local_overlay::{DerivedIdClosure, LocalSettingsOverlay};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

// ── #1077 six-field project-path codec (JSON extraction / serialization) ────
//
// The three legacy primary fields keep their names/types; each gains a paired
// `…RelativeToInstance` companion (string|null, array-aligned). On load the
// codec resolves both candidates, replaces the runtime fields with the SELECTED
// canonical paths, and stashes the raw pairs/outcomes in the hidden
// `AppSettings.project_path_state`. On write, the preserve/reconcile modes
// re-inject all six fields (companions are `#[serde(skip)]`, so a plain
// AppSettings serialization would otherwise drop them). See plan #1077 §3.2-§4.3.

/// An absent-side outcome (used when synthesizing a legacy hidden state).
pub(crate) fn absent_side() -> SideOutcome {
    SideOutcome {
        status: SideStatus::Absent,
        syntactic_path: None,
        canonical_path: None,
        identity: None,
    }
}

pub(crate) const FIELD_PROJECT_PATH: &str = "projectPath";
pub(crate) const FIELD_PROJECT_PATH_REL: &str = "projectPathRelativeToInstance";
pub(crate) const FIELD_PROJECT_PATHS: &str = "projectPaths";
pub(crate) const FIELD_PROJECT_PATHS_REL: &str = "projectPathsRelativeToInstance";
pub(crate) const FIELD_ARCHIVED: &str = "archivedProjectPaths";
pub(crate) const FIELD_ARCHIVED_REL: &str = "archivedProjectPathsRelativeToInstance";
pub(crate) const FIELD_TERMINAL_SNAPSHOTS_ENABLED: &str = "terminalSnapshotsEnabled";
/// #2348 - the two keys owned by `commands::window::set_main_window_placement`.
/// A whole-settings `Preserve` write copies a PRESENT disk value for each key
/// independently, and never removes one that disk does not have.
pub(crate) const FIELD_MAIN_GEOMETRY: &str = "mainGeometry";
pub(crate) const FIELD_MAIN_DISPLAY_STATE: &str = "mainWindowDisplayState";

/// #1737 (D7a) - top-level settings keys whose authority is the on-disk file, not
/// the in-memory struct, and which `settings.local.json` therefore may not override.
pub(crate) const OVERLAY_INELIGIBLE_DISK_KEYS: &[&str] = &[
    "agentFileSourceByMenu",
    FIELD_ARCHIVED,
    FIELD_ARCHIVED_REL,
    FIELD_PROJECT_PATH,
    FIELD_PROJECT_PATH_REL,
    FIELD_PROJECT_PATHS,
    FIELD_PROJECT_PATHS_REL,
    "rootToken",
    FIELD_TERMINAL_SNAPSHOTS_ENABLED,
];

/// #1737 (D7b) - legacy keys that exist only as a one-time migration source. AC
/// translates each into a different key, and one of them is removed from memory by
/// the migration itself, so an override would write a key the restore plan does not
/// own or would resurrect a key the #248 migration exists to delete.
pub(crate) const OVERLAY_INELIGIBLE_LEGACY_KEYS: &[&str] = &[
    "agentFileSourceByMenu",
    "sidebarAlwaysOnTop",
    "sidebarZoom",
    "startOnlyCoordinators",
    "terminalGeometry",
];

/// #1737 (D13) - a repair that creates one object per element id of a source
/// array. When the overlay introduces an id into `source_key` that the base
/// array did not carry, `<derived_prefix>/<id>` joins the restore plan with the
/// base file's value at that path, so a repair driven by an overlay value can
/// never be persisted into the base file. Deliberately narrower than the whole
/// `codingAgentProfiles` key: that key has three other production writers whose
/// payloads the operator did not override. See plan D13 and evidence 2.11.
/// #2450: `identityByAgent` needs its own closure, since a sibling key does not
/// inherit the protection.
pub(crate) const OVERLAY_DERIVED_ID_CLOSURES: &[DerivedIdClosure] = &[
    DerivedIdClosure {
        source_key: "agents",
        id_field: "id",
        derived_prefix: &["codingAgentProfiles", "profilesByAgent"],
    },
    DerivedIdClosure {
        source_key: "agents",
        id_field: "id",
        derived_prefix: &["codingAgentProfiles", "identityByAgent"],
    },
];

/// #1737 (D7c) - migration destination keys. Each names a top-level key that a
/// migration WRITES after the merge, so a migration whose destination the overlay
/// owns would silently overwrite the override in memory. Owning the destination
/// suppresses the migration (plan D7c, evidence 2.11b). Four of the five are
/// written from a legacy source key on the typed struct; `agents` is the
/// exception, rewritten in place by `apply_issue_1757_migration` (#1757 D12).
/// Every suppression names one of these constants; the array is what S29 pins
/// and what a future author greps.
pub(crate) const OVERLAY_KEY_AGENTS: &str = "agents";
pub(crate) const OVERLAY_KEY_MAIN_ALWAYS_ON_TOP: &str = "mainAlwaysOnTop";
pub(crate) const OVERLAY_KEY_MAIN_GEOMETRY: &str = "mainGeometry";
/// #2348 - the placement command refuses while the overlay owns this key or
/// `mainGeometry`, because `restore_base` is the last writer over the base file.
pub(crate) const OVERLAY_KEY_MAIN_DISPLAY_STATE: &str = "mainWindowDisplayState";
pub(crate) const OVERLAY_KEY_MAIN_ZOOM: &str = "mainZoom";
pub(crate) const OVERLAY_KEY_RESTORE_COORDINATOR_WAKE_STATE: &str = "restoreCoordinatorWakeState";
// Read only from test code: the suppressions name the five constants individually.
#[allow(dead_code)]
pub(crate) const OVERLAY_MIGRATION_DESTINATION_KEYS: &[&str] = &[
    OVERLAY_KEY_AGENTS,
    OVERLAY_KEY_MAIN_ALWAYS_ON_TOP,
    OVERLAY_KEY_MAIN_GEOMETRY,
    OVERLAY_KEY_MAIN_ZOOM,
    OVERLAY_KEY_RESTORE_COORDINATOR_WAKE_STATE,
];

/// #1737 (D16) - top-level keys whose value before the project decode differs from
/// the value `parse_settings_json` produces, so `read_merged_top_level_key` may not
/// serve them. The six project keys are rewritten or removed by
/// `apply_project_decode_to_value`. S26 derives this set from that function and fails
/// if it starts writing a key that is not listed.
pub(crate) const OVERLAY_PREMIGRATION_UNSAFE_KEYS: &[&str] = &[
    FIELD_ARCHIVED,
    FIELD_ARCHIVED_REL,
    FIELD_PROJECT_PATH,
    FIELD_PROJECT_PATH_REL,
    FIELD_PROJECT_PATHS,
    FIELD_PROJECT_PATHS_REL,
];

/// #1737 (D22) - the single site in this file that renders overlay diagnostics.
/// `local_overlay` returns typed state and never logs, which is what lets the
/// tests assert the level and the text of every record without installing a
/// global logger. `session_context.rs` has the twin of this three-line match for
/// the Markdown side; those two matches are the only deliberately unasserted
/// code this design leaves.
pub(crate) fn report_overlay_diagnostics(source: &str, overlay: &LocalSettingsOverlay) {
    use crate::config::local_overlay::OverlayDiagnosticLevel;
    for diagnostic in overlay.diagnostics(source) {
        match diagnostic.level() {
            OverlayDiagnosticLevel::Error => log::error!("{}", diagnostic.render()),
            OverlayDiagnosticLevel::Info => log::info!("{}", diagnostic.render()),
        }
    }
}

/// #1737 (D16) - one top-level key of the effective (base plus overlay) settings
/// object, read without migrations, without auto-token-gen and without any save.
/// Deliberately pre-migration AND pre-project-decode, exactly as these readers
/// were before #1737. Valid ONLY for keys outside
/// `OVERLAY_PREMIGRATION_UNSAFE_KEYS`: `apply_project_decode_to_value` rewrites
/// or removes the six project keys, so for those six the value here is not the
/// value `parse_settings_json` produces. Renders no diagnostics: the load path already
/// reports the same records and this runs twice per startup, and
/// `read_log_level_only` additionally runs before the logger exists.
pub(crate) fn read_merged_top_level_key(path: &Path, key: &str) -> Option<Value> {
    debug_assert!(
        !OVERLAY_PREMIGRATION_UNSAFE_KEYS.contains(&key),
        "#1737 D16: {key} is rewritten after the merge; read it through parse_settings_json"
    );
    let mut value = std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let _ = LocalSettingsOverlay::load_and_merge_paired(
        path,
        &mut value,
        OVERLAY_INELIGIBLE_DISK_KEYS,
        OVERLAY_INELIGIBLE_LEGACY_KEYS,
        OVERLAY_DERIVED_ID_CLOSURES,
        CONFIGURATION_COMMAND_PAIRS,
    );
    value.get(key).cloned()
}

/// The authoritative instance base for path pairing, canonicalized at the codec
/// boundary. `None` in any degraded mode (no base, or a base that fails to
/// canonicalize); never falls back to the process CWD.
pub(crate) fn production_instance_base() -> Option<PathBuf> {
    let base = super::super::instance_base()?;
    std::fs::canonicalize(&base).ok()
}

/// Presence + JSON value of a raw field (absent vs null vs value).
pub(crate) fn json_field(v: Option<&Value>) -> RawJsonField {
    match v {
        None => RawJsonField {
            present: false,
            value: None,
        },
        Some(Value::Null) => RawJsonField {
            present: true,
            value: None,
        },
        Some(other) => RawJsonField {
            present: true,
            value: Some(other.clone()),
        },
    }
}

/// Extract one plural group (primary + companion arrays) into ordered raw pairs,
/// returning the companion-present bit and any structural corruption. On
/// corruption, no entry is exposed (empty pairs) and the raw values are
/// preserved via the returned `StructuralIssue`.
pub(crate) fn extract_group(
    root: &Map<String, Value>,
    source: ProjectSource,
    primary_key: &str,
    companion_key: &str,
) -> (Vec<RawPair>, bool, Option<StructuralIssue>) {
    let primary = root.get(primary_key);
    let companion = root.get(companion_key);
    let companion_present = companion.is_some();
    let raw_absolute = json_field(primary);
    let raw_relative = json_field(companion);
    let structural = |reason: &str| StructuralIssue {
        source,
        reason: reason.to_string(),
        raw_absolute: raw_absolute.clone(),
        raw_relative: raw_relative.clone(),
    };
    let corrupt = |reason: &str| (Vec::new(), companion_present, Some(structural(reason)));

    // Companion present while primary absent/null → structural corruption.
    if companion_present {
        match primary {
            None => return corrupt("companion present while primary field is absent"),
            Some(Value::Null) => return corrupt("companion present while primary field is null"),
            _ => {}
        }
    }

    // Interpret the primary field.
    let primary_list: Vec<String> = match primary {
        // Absent or null → no active disk truth (a valid empty group here).
        None | Some(Value::Null) => return (Vec::new(), companion_present, None),
        Some(Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for el in arr {
                match el {
                    Value::String(s) => out.push(s.clone()),
                    _ => return corrupt("plural primary contains a non-string element"),
                }
            }
            out
        }
        Some(_) => return corrupt("plural primary is not an array of strings"),
    };

    // Interpret the companion field.
    let companion_list: Option<Vec<RawStringField>> = match companion {
        None => None,
        Some(Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for el in arr {
                match el {
                    Value::String(s) => out.push(RawStringField::string(s.clone())),
                    Value::Null => out.push(RawStringField::null()),
                    _ => return corrupt("plural companion contains a non-string/non-null element"),
                }
            }
            Some(out)
        }
        Some(Value::Null) => return corrupt("plural companion is a present null"),
        Some(_) => return corrupt("plural companion is not an array"),
    };

    if let Some(ref comp) = companion_list {
        if comp.len() != primary_list.len() {
            return corrupt("companion array length differs from primary array");
        }
    }

    let pairs = primary_list
        .into_iter()
        .enumerate()
        .map(|(i, abs)| RawPair {
            source,
            index: Some(i),
            absolute: RawStringField::string(abs),
            relative: companion_list
                .as_ref()
                .map(|c| c[i].clone())
                .unwrap_or_else(RawStringField::absent),
        })
        .collect();

    (pairs, companion_present, None)
}

/// Extract the legacy singular pair, or a structural corruption.
pub(crate) fn extract_singular(
    root: &Map<String, Value>,
) -> (Option<RawPair>, Option<StructuralIssue>) {
    let primary = root.get(FIELD_PROJECT_PATH);
    let companion = root.get(FIELD_PROJECT_PATH_REL);
    let raw_absolute = json_field(primary);
    let raw_relative = json_field(companion);
    let structural = |reason: &str| StructuralIssue {
        source: ProjectSource::ProjectPath,
        reason: reason.to_string(),
        raw_absolute: raw_absolute.clone(),
        raw_relative: raw_relative.clone(),
    };

    let primary_field = match primary {
        None => RawStringField::absent(),
        Some(Value::Null) => RawStringField::null(),
        Some(Value::String(s)) => RawStringField::string(s.clone()),
        Some(_) => {
            return (
                None,
                Some(structural("projectPath is not a string or null")),
            )
        }
    };
    let companion_field = match companion {
        None => RawStringField::absent(),
        Some(Value::Null) => RawStringField::null(),
        Some(Value::String(s)) => RawStringField::string(s.clone()),
        Some(_) => {
            return (
                None,
                Some(structural(
                    "projectPathRelativeToInstance is not a string or null",
                )),
            )
        }
    };
    if companion.is_some() && primary.is_none() {
        return (
            None,
            Some(structural(
                "singular companion present while projectPath is absent",
            )),
        );
    }
    if matches!(companion, Some(Value::String(_))) && matches!(primary, Some(Value::Null)) {
        return (
            None,
            Some(structural(
                "non-null singular companion paired with null projectPath",
            )),
        );
    }

    // A pair exists only when the primary is an actual string candidate; a
    // null/absent primary is the valid empty singular.
    let has_primary = primary_field.value.is_some();
    let pair = has_primary.then_some(RawPair {
        source: ProjectSource::ProjectPath,
        index: None,
        absolute: primary_field,
        relative: companion_field,
    });
    (pair, None)
}

/// Decode the six raw project fields from a settings object into the hidden
/// persistence state (selected paths, raw pairs, outcomes, structural issues).
pub(crate) fn decode_project_state(
    root: &Map<String, Value>,
    base: Option<&Path>,
    resolver: &dyn projects::CandidateResolver,
) -> ProjectPathPersistenceState {
    let (active_pairs, active_companion, active_structural) = extract_group(
        root,
        ProjectSource::ProjectPaths,
        FIELD_PROJECT_PATHS,
        FIELD_PROJECT_PATHS_REL,
    );
    let (archived_pairs, archived_companion, archived_structural) = extract_group(
        root,
        ProjectSource::ArchivedProjectPaths,
        FIELD_ARCHIVED,
        FIELD_ARCHIVED_REL,
    );
    let (singular, singular_structural) = extract_singular(root);

    // A structurally-corrupt active group exposes no active entry at all,
    // including its singular mirror.
    let effective_singular = if active_structural.is_some() {
        None
    } else {
        singular
    };

    let mut state = projects::resolve_registrations(
        &active_pairs,
        effective_singular,
        &archived_pairs,
        base,
        active_companion,
        archived_companion,
        resolver,
    );

    let mut structural = Vec::new();
    structural.extend(active_structural);
    structural.extend(archived_structural);
    structural.extend(singular_structural);
    if !structural.is_empty() {
        // Any structural corruption blocks all reconciliation/mutation.
        state.active_reconcile_eligible = false;
        state.archived_reconcile_eligible = false;
    }
    state.structural_issues = structural;
    state
}

/// Decode the six fields from `value`, then overwrite the three primary runtime
/// fields with the selected values and drop the companion keys so the remainder
/// of `AppSettings` deserializes cleanly even when a project field was
/// wrong-typed. Returns the hidden state to attach.
pub(crate) fn apply_project_decode_to_value(
    value: &mut Value,
    base: Option<&Path>,
    resolver: &dyn projects::CandidateResolver,
) -> ProjectPathPersistenceState {
    let state = match value.as_object() {
        Some(root) => decode_project_state(root, base, resolver),
        None => ProjectPathPersistenceState::default(),
    };
    if let Some(root) = value.as_object_mut() {
        root.insert(
            FIELD_PROJECT_PATH.to_string(),
            state
                .selected_head
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        root.insert(
            FIELD_PROJECT_PATHS.to_string(),
            Value::Array(
                state
                    .active_selected()
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        root.insert(
            FIELD_ARCHIVED.to_string(),
            Value::Array(
                state
                    .archived_management_paths()
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        // Companion keys are not AppSettings fields; strip so nothing lingers.
        root.remove(FIELD_PROJECT_PATH_REL);
        root.remove(FIELD_PROJECT_PATHS_REL);
        root.remove(FIELD_ARCHIVED_REL);
    }
    state
}

/// The project-field write mode. `Preserve` copies the six raw fields from the
/// fresh disk object (materializing a group only when disk has no truth for it);
/// `Reconcile` rebuilds the requested eligible group(s) from hidden state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectWriteMode {
    Preserve,
    #[cfg(test)]
    Reconcile {
        active: bool,
        archived: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSnapshotGateWriteMode {
    Preserve,
    Explicit(bool),
}

/// #2348 - the main-window placement write mode, parallel to
/// `TerminalSnapshotGateWriteMode`. `Preserve` is for every whole-settings
/// caller: a present disk value wins per key, while an absent one does not erase
/// the caller value (the load-time `terminalGeometry` -> `mainGeometry`
/// migration synthesizes a value the disk object may not have). `Explicit` is
/// used only by the narrow placement command and writes both keys from its own
/// arguments.
#[derive(Debug, Clone)]
pub(crate) enum MainWindowPlacementWriteMode {
    Preserve,
    Explicit {
        geometry: WindowGeometry,
        display_state: MainWindowDisplayState,
    },
}

/// A slot value for a retained (unresolved/conflict/missing) raw field: string
/// stays a string, absent/null becomes null (inside an array there is no absent).
pub(crate) fn raw_slot_value(field: &RawStringField) -> Value {
    match &field.value {
        Some(s) => Value::String(s.clone()),
        None => Value::Null,
    }
}

/// The companion wire value for a selected pair: encode against the base when
/// available; otherwise preserve any existing raw companion (or null).
pub(crate) fn companion_slot_value(pair: &ResolvedPair, base: Option<&Path>) -> Value {
    match base {
        Some(base) => match &pair.selected_canonical_raw {
            Some(canon) => match projects::encode_instance_relative(Path::new(canon), base) {
                Some(wire) => Value::String(wire),
                None => Value::Null,
            },
            None => raw_slot_value(&pair.raw_relative),
        },
        None => raw_slot_value(&pair.raw_relative),
    }
}

/// Rebuild one group's (primary array, companion array) from its resolved pairs:
/// emit selected pairs (repaired), retain unresolved pairs value-for-value, and
/// drop silent duplicates.
pub(crate) fn rebuild_group_arrays(
    pairs: &[ResolvedPair],
    base: Option<&Path>,
) -> (Vec<Value>, Vec<Value>) {
    let mut primary = Vec::new();
    let mut companion = Vec::new();
    for pair in pairs {
        if let Some(sel) = &pair.selected {
            primary.push(Value::String(sel.clone()));
            companion.push(companion_slot_value(pair, base));
        } else if pair.issue.is_some() {
            primary.push(raw_slot_value(&pair.raw_absolute));
            companion.push(raw_slot_value(&pair.raw_relative));
        }
        // else: silent dedupe drop — emit nothing.
    }
    (primary, companion)
}

/// Insert the four active fields from hidden state.
pub(crate) fn write_active_group(
    out: &mut Map<String, Value>,
    state: &ProjectPathPersistenceState,
    base: Option<&Path>,
) {
    let (primary, companion) = rebuild_group_arrays(state.active_pairs(), base);
    out.insert(
        FIELD_PROJECT_PATH.to_string(),
        primary.first().cloned().unwrap_or(Value::Null),
    );
    out.insert(
        FIELD_PROJECT_PATH_REL.to_string(),
        companion.first().cloned().unwrap_or(Value::Null),
    );
    out.insert(FIELD_PROJECT_PATHS.to_string(), Value::Array(primary));
    out.insert(FIELD_PROJECT_PATHS_REL.to_string(), Value::Array(companion));
}

/// Insert the two archived fields from hidden state.
pub(crate) fn write_archived_group(
    out: &mut Map<String, Value>,
    state: &ProjectPathPersistenceState,
    base: Option<&Path>,
) {
    let (primary, companion) = rebuild_group_arrays(state.archived_pairs(), base);
    out.insert(FIELD_ARCHIVED.to_string(), Value::Array(primary));
    out.insert(FIELD_ARCHIVED_REL.to_string(), Value::Array(companion));
}

/// If `AppSettings` was constructed directly (empty hidden state) yet carries
/// runtime project lists, synthesize a legacy absolute-only state so the writers
/// behave like a legacy-decoded file (companions null, all entries selected).
pub(crate) fn hidden_state_for_write(
    settings: &AppSettings,
) -> std::borrow::Cow<'_, ProjectPathPersistenceState> {
    let state = &settings.project_path_state;
    let empty = state.pairs.is_empty() && state.structural_issues.is_empty();
    let runtime_nonempty = settings.project_path.is_some()
        || !settings.project_paths.is_empty()
        || !settings.archived_project_paths.is_empty();
    if empty && runtime_nonempty {
        std::borrow::Cow::Owned(synthesize_legacy_state(settings))
    } else {
        std::borrow::Cow::Borrowed(state)
    }
}

pub(crate) fn legacy_selected_pair(
    source: ProjectSource,
    index: Option<usize>,
    path: &str,
) -> ResolvedPair {
    ResolvedPair {
        source,
        index,
        raw_absolute: RawStringField::string(path),
        raw_relative: RawStringField::absent(),
        absolute_side: absent_side(),
        relative_side: absent_side(),
        selected: Some(path.to_string()),
        selected_canonical_raw: None,
        selected_identity: None,
        issue: None,
        repair: RepairKind::None,
    }
}

pub(crate) fn synthesize_legacy_state(settings: &AppSettings) -> ProjectPathPersistenceState {
    let mut pairs: Vec<ResolvedPair> = settings
        .project_paths
        .iter()
        .enumerate()
        .map(|(i, p)| legacy_selected_pair(ProjectSource::ProjectPaths, Some(i), p))
        .collect();
    let active_registration_count = pairs.len();
    for (i, p) in settings.archived_project_paths.iter().enumerate() {
        pairs.push(legacy_selected_pair(
            ProjectSource::ArchivedProjectPaths,
            Some(i),
            p,
        ));
    }
    ProjectPathPersistenceState {
        pairs,
        selected_head: settings.project_paths.first().cloned(),
        active_registration_count,
        archived_registration_count: settings.archived_project_paths.len(),
        active_companion_present: false,
        archived_companion_present: false,
        has_genuine_singular: false,
        active_reconcile_eligible: false,
        archived_reconcile_eligible: false,
        structural_issues: Vec::new(),
        runtime_authoritative: true,
    }
}

/// A selected pair for a runtime path, canonicalizing so the reconcile write can
/// encode a real instance-relative companion (a non-existent path yields a null
/// companion).
pub(crate) fn resynced_selected_pair(
    source: ProjectSource,
    index: usize,
    path: &str,
) -> ResolvedPair {
    let canonical_raw = std::fs::canonicalize(path)
        .ok()
        .and_then(|c| c.to_str().map(str::to_string));
    ResolvedPair {
        source,
        index: Some(index),
        raw_absolute: RawStringField::string(path),
        raw_relative: RawStringField::absent(),
        absolute_side: absent_side(),
        relative_side: absent_side(),
        selected: Some(path.to_string()),
        selected_canonical_raw: canonical_raw,
        selected_identity: None,
        issue: None,
        repair: RepairKind::PopulateCompanion,
    }
}

/// #1077: after an explicit project mutation updated the three runtime lists,
/// rebuild the hidden state to match while retaining any unresolved
/// (conflict/missing/invalid) pairs from the pre-mutation state value-for-value,
/// so a register/remove/archive/unarchive never drops a preserved conflict or
/// missing record. The result is runtime-authoritative: a Reconcile write emits
/// the runtime selection plus the retained raw records. Callers must reject a
/// structurally-corrupt or conflict-scoped mutation BEFORE calling this.
pub(crate) fn resync_project_state_from_runtime(settings: &mut AppSettings) {
    let old = settings.project_path_state.clone();
    let retained_active: Vec<ResolvedPair> = old
        .active_pairs()
        .iter()
        .filter(|p| p.issue.is_some())
        .cloned()
        .collect();
    let retained_archived: Vec<ResolvedPair> = old
        .archived_pairs()
        .iter()
        .filter(|p| p.issue.is_some())
        .cloned()
        .collect();

    let active = reconcile_group_with_retained(
        &settings.project_paths,
        ProjectSource::ProjectPaths,
        retained_active,
    );
    let active_registration_count = active.len();
    let archived = reconcile_group_with_retained(
        &settings.archived_project_paths,
        ProjectSource::ArchivedProjectPaths,
        retained_archived,
    );
    let archived_registration_count = archived.len();

    let mut pairs = active;
    pairs.extend(archived);

    settings.project_path_state = Arc::new(ProjectPathPersistenceState {
        pairs,
        selected_head: settings.project_paths.first().cloned(),
        active_registration_count,
        archived_registration_count,
        active_companion_present: old.active_companion_present,
        archived_companion_present: old.archived_companion_present,
        has_genuine_singular: false,
        active_reconcile_eligible: false,
        archived_reconcile_eligible: false,
        structural_issues: old.structural_issues.clone(),
        runtime_authoritative: true,
    });
}

/// Rebuild one group's pairs from the mutated `runtime` list while reconciling
/// the pre-mutation `retained` unresolved (missing/conflict/invalid) records:
///
/// - a runtime entry that matches a retained record by normalized key reuses
///   that record IN PLACE (preserving its raw values, companion, and issue), so
///   a preserved conflict/missing record is never rebuilt as "selected" nor
///   duplicated;
/// - a runtime entry with no retained match becomes a fresh selected pair;
/// - a retained record whose key is absent from the runtime list was removed by
///   the mutation and is dropped (so "Remove from list" actually removes it).
pub(crate) fn reconcile_group_with_retained(
    runtime: &[String],
    source: ProjectSource,
    mut retained: Vec<ResolvedPair>,
) -> Vec<ResolvedPair> {
    let mut out = Vec::with_capacity(runtime.len());
    for (i, path) in runtime.iter().enumerate() {
        let key = projects::normalize_for_compare(path);
        let matched = retained.iter().position(|r| {
            r.raw_absolute
                .value
                .as_deref()
                .map(projects::normalize_for_compare)
                == Some(key.clone())
        });
        match matched {
            Some(pos) => {
                let mut pair = retained.remove(pos);
                pair.source = source;
                pair.index = Some(i);
                out.push(pair);
            }
            None => out.push(resynced_selected_pair(source, i, path)),
        }
    }
    // Any retained record not matched by the runtime list was removed; drop it.
    out
}

/// Confirm the non-project part of a disk object still deserializes as
/// `AppSettings` (after neutralizing the six project fields), so a present file
/// with corrupt non-project settings is never silently overwritten.
pub(crate) fn validate_non_project_settings(disk: &Map<String, Value>) -> Result<(), String> {
    let mut probe = Value::Object(disk.clone());
    if let Some(root) = probe.as_object_mut() {
        root.insert(FIELD_PROJECT_PATH.to_string(), Value::Null);
        root.insert(FIELD_PROJECT_PATHS.to_string(), Value::Array(Vec::new()));
        root.insert(FIELD_ARCHIVED.to_string(), Value::Array(Vec::new()));
        root.remove(FIELD_PROJECT_PATH_REL);
        root.remove(FIELD_PROJECT_PATHS_REL);
        root.remove(FIELD_ARCHIVED_REL);
        // #2716 (B3) trap four: `agents` lives in the agents file once migrated.
        root.entry(AGENTS_INSTANCE_KEYS[0])
            .or_insert_with(|| Value::Array(Vec::new()));
    }
    serde_json::from_value::<AppSettings>(registered_profiles_for_read(&probe))
        .map(|_| ())
        .map_err(|e| {
            format!(
                "Refusing to overwrite present settings whose non-project fields are invalid: {e}"
            )
        })
}

/// #1077 automatic reconciliation boundary (§4.3): reconcile the requested
/// eligible project group(s) from `settings`' hidden state to `path`, returning
/// the fresh-decoded settings. A structurally-corrupt or no-eligible-repair
/// state performs no write and returns the settings unchanged.
#[cfg(test)]
pub(crate) fn reconcile_project_state_to_path(
    settings: &AppSettings,
    path: &Path,
    active: bool,
    archived: bool,
) -> Result<AppSettings, String> {
    let mut transaction = begin_project_paths_transaction_at_path(settings, path)?;
    let written = if project_state_has_structural(settings) {
        transaction.fresh_settings().clone()
    } else {
        transaction.commit_project_groups(settings, active, archived)?
    };
    // Snapshot fixture helper only: preserve historical unrelated fixture seeding.
    // Production project operations call the token's sidecar-only commit directly.
    save_settings_value_locked(
        &written,
        path,
        ProjectWriteMode::Reconcile { active, archived },
        TerminalSnapshotGateWriteMode::Preserve,
        MainWindowPlacementWriteMode::Preserve,
    )
    .map_err(|error| report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings))
}

// Sole project registration authority; callers hold the shared settings lock.
pub(crate) fn project_paths_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(super::super::instance_artifacts::PROJECT_PATHS_FILE_NAME)
}

pub(crate) fn raw_project_projection(object: &Map<String, Value>) -> Map<String, Value> {
    [
        FIELD_PROJECT_PATH,
        FIELD_PROJECT_PATH_REL,
        FIELD_PROJECT_PATHS,
        FIELD_PROJECT_PATHS_REL,
        FIELD_ARCHIVED,
        FIELD_ARCHIVED_REL,
    ]
    .into_iter()
    .filter_map(|key| {
        object
            .get(key)
            .map(|value| (key.to_string(), value.clone()))
    })
    .collect()
}

pub(crate) fn project_migration_error(
    path: &Path,
    stage: SettingsSaveStage,
    reason: &str,
) -> SettingsSaveError {
    project_migration_error_labeled(path, path, stage, reason)
}

pub(crate) fn project_migration_error_labeled(
    path: &Path,
    label: &Path,
    stage: SettingsSaveStage,
    reason: &str,
) -> SettingsSaveError {
    let kind = if reason.starts_with("project_paths_conflict") {
        SettingsSaveReason::ProjectPathsConflict
    } else if reason.starts_with("project_paths_backup_mismatch") {
        SettingsSaveReason::ProjectPathsBackupMismatch
    } else {
        SettingsSaveReason::ProjectPathsSourceRejected
    };
    SettingsSaveError::semantic(
        stage,
        path,
        None,
        kind,
        SettingsSaveLegacyOutward::DiskRead(format!("{}: {reason}", label.display())),
    )
}

pub(crate) fn read_project_source_bytes_labeled_with_limit(
    path: &Path,
    label: &Path,
    limit: usize,
) -> Result<Option<Vec<u8>>, SettingsSaveError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(SettingsSaveError::io(
                SettingsSaveStage::ProjectPathsRefreshDiskGate,
                path,
                None,
                error,
                SettingsSaveLegacyOutward::SettingsSaveFailed,
            ))
        }
        Ok(_) => {}
    }
    crate::path_identity::read_bounded_regular(path, limit)
        .map(|(bytes, _)| Some(bytes))
        .map_err(|reason| {
            project_migration_error_labeled(
                path,
                label,
                SettingsSaveStage::ProjectPathsRefreshDiskGate,
                &reason,
            )
        })
}

#[derive(Debug)]
pub(crate) struct RawProjectSource {
    pub(crate) object: Map<String, Value>,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) fn read_raw_project_source(
    path: &Path,
) -> Result<Option<RawProjectSource>, SettingsSaveError> {
    read_raw_project_source_with_limit(path, 16 * 1024 * 1024, false)
}

pub(crate) fn read_raw_project_source_with_limit(
    path: &Path,
    limit: usize,
    legacy_settings_diagnostic: bool,
) -> Result<Option<RawProjectSource>, SettingsSaveError> {
    read_raw_project_source_labeled_with_limit(path, path, limit, legacy_settings_diagnostic)
}

pub(crate) fn read_raw_project_source_labeled_with_limit(
    path: &Path,
    label: &Path,
    limit: usize,
    legacy_settings_diagnostic: bool,
) -> Result<Option<RawProjectSource>, SettingsSaveError> {
    let Some(bytes) = read_project_source_bytes_labeled_with_limit(path, label, limit)? else {
        return Ok(None);
    };
    raw_project_source_from_bytes(bytes, path, label, legacy_settings_diagnostic).map(Some)
}

/// The decode half of the raw project-source reader, shared with the owner's
/// single capture so selection judges captured bytes exactly like read ones.
pub(crate) fn raw_project_source_from_bytes(
    bytes: Vec<u8>,
    path: &Path,
    label: &Path,
    legacy_settings_diagnostic: bool,
) -> Result<RawProjectSource, SettingsSaveError> {
    let value = crate::path_identity::parse_json_no_duplicates(&bytes).map_err(|reason| {
        let mut error = project_migration_error_labeled(
            path,
            label,
            SettingsSaveStage::ProjectPathsRefreshDiskGate,
            &reason,
        );
        if legacy_settings_diagnostic {
            error.legacy_outward = SettingsSaveLegacyOutward::DiskRead(format!(
                "settings.json exists but could not be parsed ({}: {reason}); refusing to modify \
                    it - fix or remove the file first",
                label.display()
            ));
        }
        error
    })?;
    let Value::Object(object) = value else {
        return Err(project_migration_error_labeled(
            path,
            label,
            SettingsSaveStage::ProjectPathsRefreshDiskGate,
            "project_paths_source_not_object",
        ));
    };
    Ok(RawProjectSource { object, bytes })
}

#[derive(Debug)]
pub(crate) struct SelectedProjectSource {
    pub(crate) legacy: Option<RawProjectSource>,
    pub(crate) destination: Option<RawProjectSource>,
    pub(crate) projection: Map<String, Value>,
}

// Capture both immutable objects only while the original settings token is held.
pub(crate) fn select_project_source_locked(
    transaction: &ProjectPathsWriteTransaction,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    select_project_source_at_locked_path(&transaction.path)
}

pub(crate) fn select_project_source_at_locked_path(
    path: &Path,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    select_project_source_at_locked_path_with_limit(path, 16 * 1024 * 1024)
}

pub(crate) fn select_project_source_at_locked_path_with_limit(
    path: &Path,
    limit: usize,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    select_project_source_at_labeled_path_with_limit(path, path, limit)
}

pub(crate) fn select_project_source_at_labeled_path_with_limit(
    path: &Path,
    label: &Path,
    limit: usize,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    let legacy = read_raw_project_source_labeled_with_limit(path, label, limit, true)?;
    let destination_path = project_paths_path(path);
    let destination_label = project_paths_path(label);
    let destination = read_raw_project_source_labeled_with_limit(
        &destination_path,
        &destination_label,
        limit,
        false,
    )?;
    select_project_source_from_raw(
        legacy,
        destination,
        label,
        &destination_path,
        &destination_label,
    )
}

/// The pure selector over both raw sources: the destination wins, and a non-empty
/// legacy projection that differs from it is a conflict, never a silent choice.
pub(crate) fn select_project_source_from_raw(
    legacy: Option<RawProjectSource>,
    destination: Option<RawProjectSource>,
    label: &Path,
    destination_path: &Path,
    destination_label: &Path,
) -> Result<SelectedProjectSource, SettingsSaveError> {
    let legacy_projection = legacy
        .as_ref()
        .map(|source| raw_project_projection(&source.object))
        .unwrap_or_default();
    let projection = match &destination {
        None => legacy_projection,
        Some(source) => {
            let projection = raw_project_projection(&source.object);
            if !legacy_projection.is_empty() && legacy_projection != projection {
                return Err(project_migration_error_labeled(
                    destination_path,
                    destination_label,
                    SettingsSaveStage::ProjectPathsRefreshDiskGate,
                    &format!(
                        "project_paths_conflict; source={}; destination={}; preserve both \
                        files and reconcile raw projections to equality",
                        label.display(),
                        destination_label.display()
                    ),
                ));
            }
            projection
        }
    };
    Ok(SelectedProjectSource {
        legacy,
        destination,
        projection,
    })
}

// Read-only selection acquires before even checking either source's existence.
#[cfg(test)]
pub(crate) fn select_project_source_at_path(
    path: &Path,
) -> Result<Map<String, Value>, SettingsSaveError> {
    let lock = SettingsFileLock::acquire_contextual(
        path,
        std::time::Duration::from_secs(2),
        "select_project_source_at_path",
    )?;
    let transaction = ProjectPathsWriteTransaction {
        _lock: lock,
        path: path.to_path_buf(),
        fresh: AppSettings::default(),
        revalidated_after_failure: false,
    };
    Ok(select_project_source_locked(&transaction)?.projection)
}

pub(crate) fn publish_project_migration_backup_labeled(
    settings_path: &Path,
    label: &Path,
    bytes: &[u8],
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    use std::io::Write as _;
    let backup = settings_path
        .with_file_name(super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    let backup_label =
        label.with_file_name(super::super::instance_artifacts::PROJECT_PATHS_BACKUP_FILE_NAME);
    let mismatch = || {
        project_migration_error_labeled(
            &backup,
            &backup_label,
            SettingsSaveStage::PostWriteVerify,
            &format!(
                "project_paths_backup_mismatch; source={}; destination={}; backup={}; stop all \
            AC/old processes, preserve copies and orphan temps, rename backup without overwrite to \
            project-paths.pre-migration.recovered-<UTC>-<unique>.no-git.json, verify intended \
            legacy projection and \
            retry",
                label.display(),
                project_paths_path(label).display(),
                backup_label.display()
            ),
        )
    };
    if let Some(existing) =
        read_project_source_bytes_labeled_with_limit(&backup, &backup_label, 16 * 1024 * 1024)?
    {
        return if existing == bytes {
            Ok(())
        } else {
            Err(mismatch())
        };
    }
    let parent = backup.parent().ok_or_else(|| {
        project_migration_error_labeled(
            &backup,
            &backup_label,
            SettingsSaveStage::PrepareTarget,
            "missing parent",
        )
    })?;
    crate::path_identity::verify_component_chain(parent).map_err(|reason| {
        project_migration_error_labeled(
            &backup,
            &backup_label,
            SettingsSaveStage::PrepareTarget,
            &reason,
        )
    })?;
    let mut name = backup.file_name().unwrap().to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        SAVE_OP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = parent.join(name);
    checkpoint("backup_create", &temp)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let io_error = |stage, source| {
        SettingsSaveError::io(
            stage,
            &backup,
            Some(&temp),
            source,
            SettingsSaveLegacyOutward::SettingsSaveFailed,
        )
    };
    let mut file = options
        .open(&temp)
        .map_err(|error| io_error(SettingsSaveStage::TempCreate, error))?;
    crate::path_identity::verify_opened_regular_file(&temp, &file, false).map_err(|reason| {
        project_migration_error_labeled(
            &temp,
            &label.with_file_name(temp.file_name().unwrap()),
            SettingsSaveStage::TempVerify,
            &reason,
        )
    })?;
    let prefix = bytes.len() / 2;
    file.write_all(&bytes[..prefix])
        .map_err(|error| io_error(SettingsSaveStage::TempWrite, error))?;
    checkpoint("backup_prefix", &temp)?;
    file.write_all(&bytes[prefix..])
        .map_err(|error| io_error(SettingsSaveStage::TempWrite, error))?;
    checkpoint("backup_write", &temp)?;
    file.sync_all()
        .map_err(|error| io_error(SettingsSaveStage::TempSync, error))?;
    checkpoint("backup_sync", &temp)?;
    drop(file);
    if read_project_source_bytes_labeled_with_limit(
        &temp,
        &label.with_file_name(temp.file_name().unwrap()),
        16 * 1024 * 1024,
    )?
    .as_deref()
        != Some(bytes)
    {
        return Err(mismatch());
    }
    checkpoint("backup_before_rename", &temp)?;
    // Never use the replacing atomic writer for the immutable fixed name.
    if let Some(existing) =
        read_project_source_bytes_labeled_with_limit(&backup, &backup_label, 16 * 1024 * 1024)?
    {
        return if existing == bytes {
            Ok(())
        } else {
            Err(mismatch())
        };
    }
    if let Err(error) = std::fs::rename(&temp, &backup) {
        match read_project_source_bytes_labeled_with_limit(
            &backup,
            &backup_label,
            16 * 1024 * 1024,
        )? {
            Some(existing) if existing == bytes => {}
            Some(_) => return Err(mismatch()),
            None => return Err(io_error(SettingsSaveStage::AtomicReplace, error)),
        }
    }
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(SettingsSaveStage::ParentDirectorySync, error))?;
    if read_project_source_bytes_labeled_with_limit(&backup, &backup_label, 16 * 1024 * 1024)?
        .as_deref()
        != Some(bytes)
    {
        return Err(mismatch());
    }
    checkpoint("backup_published", &backup)
}

pub(crate) fn ensure_project_paths_migrated_locked(
    transaction: &ProjectPathsWriteTransaction,
) -> Result<(), SettingsSaveError> {
    ensure_project_paths_migrated_at_locked_path(&transaction.path, &mut |_, _| Ok(()))
}

#[cfg(test)]
pub(crate) fn ensure_project_paths_migrated_with(
    transaction: &ProjectPathsWriteTransaction,
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    ensure_project_paths_migrated_at_locked_path(&transaction.path, checkpoint)
}

pub(crate) fn ensure_project_paths_migrated_at_locked_path(
    path: &Path,
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    ensure_project_paths_migrated_at_labeled_path(path, path, checkpoint)
}

pub(crate) fn ensure_project_paths_migrated_at_labeled_path(
    path: &Path,
    label: &Path,
    checkpoint: &mut impl FnMut(&str, &Path) -> Result<(), SettingsSaveError>,
) -> Result<(), SettingsSaveError> {
    let selected = select_project_source_at_labeled_path_with_limit(path, label, 16 * 1024 * 1024)?;
    let destination = project_paths_path(path);
    let destination_label = project_paths_path(label);
    let legacy_fields = selected
        .legacy
        .as_ref()
        .is_some_and(|source| !raw_project_projection(&source.object).is_empty());
    if selected.destination.is_none() {
        if legacy_fields {
            publish_project_migration_backup_labeled(
                path,
                label,
                &selected.legacy.as_ref().unwrap().bytes,
                checkpoint,
            )?;
        }
        checkpoint("destination_before_write", &destination)?;
        write_value_atomic_labeled(
            &Value::Object(selected.projection.clone()),
            &destination,
            &destination_label,
        )?;
        checkpoint("destination_published", &destination)?;
    }
    if !legacy_fields {
        return Ok(());
    }
    checkpoint("destination_readback", &destination)?;
    let readback = read_raw_project_source_labeled_with_limit(
        &destination,
        &destination_label,
        16 * 1024 * 1024,
        false,
    )?
    .ok_or_else(|| {
        project_migration_error_labeled(
            &destination,
            &destination_label,
            SettingsSaveStage::PostWriteRead,
            "project_paths_destination_disappeared",
        )
    })?;
    if raw_project_projection(&readback.object) != selected.projection {
        return Err(project_migration_error_labeled(
            &destination,
            &destination_label,
            SettingsSaveStage::PostWriteVerify,
            "project_paths_readback_mismatch",
        ));
    }
    checkpoint("source_cleanup", path)?;
    let original = selected.legacy.unwrap();
    let mut cleaned = original.object;
    for key in selected.projection.keys() {
        cleaned.remove(key);
    }
    let written = write_value_atomic_labeled(&Value::Object(cleaned), path, label)?;
    if original.bytes != written {
        rotate_settings_backups_labeled(path, label, &original.bytes);
    }
    checkpoint("cleanup_complete", path)
}

/// Owns the shared file lock throughout a synchronous project operation.
/// Never carry this token across an await.
pub(crate) struct ProjectPathsWriteTransaction {
    pub(crate) _lock: SettingsFileLock,
    pub(crate) path: PathBuf,
    pub(crate) fresh: AppSettings,
    pub(crate) revalidated_after_failure: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectPathsRepairStage {
    Read,
    Write,
}

#[track_caller]
pub(crate) fn empty_project_transaction(
    path: &Path,
    operation: &'static str,
) -> Result<ProjectPathsWriteTransaction, SettingsSaveError> {
    Ok(ProjectPathsWriteTransaction {
        _lock: SettingsFileLock::acquire_contextual(
            path,
            std::time::Duration::from_secs(2),
            operation,
        )?,
        path: path.to_path_buf(),
        fresh: AppSettings::default(),
        revalidated_after_failure: false,
    })
}

pub(crate) fn apply_selected_project_state(
    settings: &mut AppSettings,
    projection: &Map<String, Value>,
) {
    let state = decode_project_state(
        projection,
        production_instance_base().as_deref(),
        &projects::FsCandidateResolver,
    );
    settings.project_paths = state.active_selected();
    settings.project_path = state.selected_head.clone();
    settings.archived_project_paths = state.archived_management_paths();
    settings.project_path_state = Arc::new(state);
    settings.project_paths_persistence_error = None;
}

pub(crate) fn begin_project_paths_transaction_at_path(
    current: &AppSettings,
    settings_path: &Path,
) -> Result<ProjectPathsWriteTransaction, String> {
    let mut transaction =
        empty_project_transaction(settings_path, "begin_project_paths_transaction_at_path")
            .map_err(|error| {
                report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
            })?;
    ensure_project_paths_migrated_locked(&transaction).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    transaction.fresh = current.clone();
    refresh_coding_agent_identity_at_locked_path(&mut transaction.fresh, settings_path);
    let selected = select_project_source_locked(&transaction).map_err(|error| {
        report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
    })?;
    apply_selected_project_state(&mut transaction.fresh, &selected.projection);
    Ok(transaction)
}

impl ProjectPathsWriteTransaction {
    pub(crate) fn fresh_settings(&self) -> &AppSettings {
        &self.fresh
    }

    pub(crate) fn commit_project_groups(
        &mut self,
        candidate: &AppSettings,
        active: bool,
        archived: bool,
    ) -> Result<AppSettings, String> {
        if let Some(message) = &candidate.project_paths_persistence_error {
            return Err(message.clone());
        }
        self.revalidated_after_failure = false;
        let result = self.write_project_groups(candidate, active, archived);
        self.handle_commit_result(result)
    }

    pub(crate) fn refresh_raw_projects(&self, settings: &mut AppSettings) -> Result<(), String> {
        let selected = select_project_source_locked(self).map_err(|error| {
            report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
        })?;
        apply_selected_project_state(settings, &selected.projection);
        if settings.project_path_state.has_structural() {
            return Err("project metadata is malformed".to_string());
        }
        if let Some(lists) = project_lists_from_projection(&selected.projection)? {
            settings.project_paths = lists.project_paths;
            settings.project_path = lists.project_path;
            settings.archived_project_paths = lists.archived_project_paths.unwrap_or_default();
        }
        Ok(())
    }

    pub(crate) fn raw_project_projection(&self) -> Result<Map<String, Value>, String> {
        select_project_source_locked(self)
            .map(|selected| selected.projection)
            .map_err(|error| {
                report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
            })
    }

    pub(crate) fn restore_raw_project_projection(
        &mut self,
        projection: &Map<String, Value>,
    ) -> Result<AppSettings, String> {
        let result = (|| {
            let selected = select_project_source_locked(self)?;
            let mut object = selected
                .destination
                .map(|source| source.object)
                .unwrap_or_default();
            replace_project_projection(&mut object, projection);
            write_value_atomic(&Value::Object(object), &project_paths_path(&self.path))?;
            let selected = select_project_source_locked(self)?;
            let mut written = self.fresh.clone();
            refresh_coding_agent_identity_at_locked_path(&mut written, &self.path);
            apply_selected_project_state(&mut written, &selected.projection);
            Ok(written)
        })();
        self.handle_commit_result(result)
    }

    pub(crate) fn write_project_groups(
        &self,
        candidate: &AppSettings,
        active: bool,
        archived: bool,
    ) -> Result<AppSettings, SettingsSaveError> {
        let selected = select_project_source_locked(self)?;
        let destination = project_paths_path(&self.path);
        let mut out = selected
            .destination
            .map(|source| source.object)
            .unwrap_or_default();
        let state = hidden_state_for_write(candidate);
        if state.has_structural() {
            return Err(project_migration_error(
                &destination,
                SettingsSaveStage::ReconcileDiskGate,
                "project metadata is malformed",
            ));
        }
        let base = production_instance_base();
        if active && (state.runtime_authoritative || state.active_reconcile_eligible) {
            write_active_group(&mut out, &state, base.as_deref());
        }
        if archived && (state.runtime_authoritative || state.archived_reconcile_eligible) {
            write_archived_group(&mut out, &state, base.as_deref());
        }
        if out
            != read_raw_project_source(&destination)?
                .map(|source| source.object)
                .unwrap_or_default()
        {
            write_value_atomic(&Value::Object(out), &destination)?;
        }
        let projection = select_project_source_locked(self)?.projection;
        let mut written = candidate.clone();
        refresh_coding_agent_identity_at_locked_path(&mut written, &self.path);
        apply_selected_project_state(&mut written, &projection);
        Ok(written)
    }

    pub(crate) fn handle_commit_result(
        &mut self,
        result: Result<AppSettings, SettingsSaveError>,
    ) -> Result<AppSettings, String> {
        match result {
            Ok(mut written) => {
                written.project_paths_persistence_error = None;
                self.fresh = written.clone();
                Ok(written)
            }
            Err(error) => {
                let uncertain = match error.stage {
                    SettingsSaveStage::AtomicReplace
                    | SettingsSaveStage::PostWriteRead
                    | SettingsSaveStage::PostWriteVerify
                    | SettingsSaveStage::ReDecode => true,
                    #[cfg(unix)]
                    SettingsSaveStage::TargetPermissions
                    | SettingsSaveStage::ParentDirectorySync => true,
                    _ => false,
                };
                let message =
                    report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings);
                if uncertain {
                    // Re-read under this same lock; never roll back a write
                    // whose outcome cannot be established.
                    let validated = select_project_source_locked(self)
                        .map(|selected| {
                            let mut settings = self.fresh.clone();
                            refresh_coding_agent_identity_at_locked_path(&mut settings, &self.path);
                            apply_selected_project_state(&mut settings, &selected.projection);
                            settings
                        })
                        .map_err(|error| {
                            report_settings_save_error(
                                error,
                                SettingsSaveReportSurface::GeneralSettings,
                            )
                        });
                    match validated {
                        Ok(mut settings) => {
                            settings.local_overlay_state = self.fresh.local_overlay_state.clone();
                            settings.project_paths_persistence_error = None;
                            self.fresh = settings;
                        }
                        Err(reason) => {
                            self.fresh.project_paths_persistence_error = Some(format!(
                                "{message}; project state revalidation failed: {reason}"
                            ))
                        }
                    }
                    self.revalidated_after_failure = true;
                }
                Err(message)
            }
        }
    }
}

/// Validate fresh authority even if the cached state reports no repair.
pub(crate) fn repair_fresh_project_paths_at_path(
    current: &mut AppSettings,
    path: &Path,
) -> Result<(), (ProjectPathsRepairStage, String)> {
    let mut transaction = match begin_project_paths_transaction_at_path(current, path) {
        Ok(transaction) => transaction,
        Err(message) => {
            current.project_paths_persistence_error = Some(message.clone());
            return Err((ProjectPathsRepairStage::Read, message));
        }
    };
    let candidate = transaction.fresh_settings().clone();
    let state = &candidate.project_path_state;
    let active = state.active_reconcile_eligible;
    let archived = state.archived_reconcile_eligible;
    if state.has_structural() || !(active || archived) {
        *current = candidate;
        return Ok(());
    }
    match transaction.commit_project_groups(&candidate, active, archived) {
        Ok(written) => {
            *current = written;
            Ok(())
        }
        Err(message) => {
            if transaction.revalidated_after_failure {
                *current = transaction.fresh_settings().clone();
            }
            Err((ProjectPathsRepairStage::Write, message))
        }
    }
}
