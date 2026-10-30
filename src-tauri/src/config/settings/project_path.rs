use super::project_path as projects;
#[cfg(test)]
use super::{
    project_state_has_structural, report_settings_save_error, save_settings_value_locked,
    SettingsSaveReportSurface,
};
use super::{
    registered_profiles_for_read, AppSettings, MainWindowDisplayState, WindowGeometry,
    AGENTS_INSTANCE_KEYS, CONFIGURATION_COMMAND_PAIRS,
};
use crate::config::local_overlay::{DerivedIdClosure, LocalSettingsOverlay};
use crate::filesystem::settings_sources;
pub(crate) use resolution::{
    ProjectPathPersistenceState, ProjectSource, RawJsonField, RawPair, RawStringField, RepairKind,
    ResolvedPair, StructuralIssue,
};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod codec;
mod identity;
mod resolution;
pub(crate) use codec::encode_instance_relative;
#[cfg(test)]
pub(crate) use codec::{decode_instance_relative, RelDecodeError};
pub(crate) use identity::display_canonical;
#[cfg(test)]
pub(crate) use identity::DirectoryIdentity;
pub(crate) use identity::{CandidateResolver, FsCandidateResolver, SideOutcome, SideStatus};
pub(crate) use resolution::resolve_registrations;
pub(crate) use resolution::IssueKind;

/// Mirrors the frontend `normalizePath` at
/// `src/sidebar/stores/project.ts:17-19`. Comparison only — the persisted
/// entry retains the original byte sequence.
pub(crate) fn normalize_for_compare(s: &str) -> String {
    let s = crate::path_utils::normalize_windows_verbatim_path(s);
    // Slashes normalised, lowercased, trailing `/` stripped. The trailing
    // strip closes Round-1 IR.3.2 (shell tab-completion appends `\` on dirs;
    // without trim, `C:\foo\` and `C:\foo` would become DIFFERENT entries).
    s.replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string()
}

mod migration;
mod transaction;

pub(crate) use migration::{
    ensure_project_paths_migrated_at_labeled_path, ensure_project_paths_migrated_at_locked_path,
    project_migration_error_labeled, project_paths_path, raw_project_source_from_bytes,
    read_raw_project_source_labeled_with_limit, select_project_source_at_labeled_path_with_limit,
    select_project_source_at_locked_path, select_project_source_at_locked_path_with_limit,
    select_project_source_from_raw, select_project_source_locked, ProjectPathsRepairStage,
    ProjectPathsWriteTransaction,
};
#[cfg(test)]
pub(crate) use migration::{
    ensure_project_paths_migrated_locked, ensure_project_paths_migrated_with,
    project_migration_error, raw_project_projection, read_raw_project_source,
    select_project_source_at_path,
};
pub(crate) use transaction::{
    apply_selected_project_state, begin_project_paths_transaction_at_path,
    empty_project_transaction, repair_fresh_project_paths_at_path,
};

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
    let mut value = settings_sources::text(path)
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
    crate::filesystem::projects::canonical_project_path(&base).ok()
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
    let canonical_raw = crate::filesystem::projects::canonical_project_path(Path::new(path))
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
