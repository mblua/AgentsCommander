use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::agent_command::is_safe_instructions_filename;
use crate::config::settings::{
    validate_agent_command_text, validate_config_seed_dest, validate_env_rows,
};

use super::{
    apply_support_gate, catalog_diagnostic, compose_local_layer, local_catalog_path,
    managed_content_sha256, manifest_path, migration_journal_path, parse_catalog_schema2,
    parse_local_layer, parse_migration_journal, parse_strict_json, project_catalog_path,
    read_instance_legacy_source, read_optional_regular_file, sha256_hex,
    supported_shipped_definitions, validate_catalog_key, validate_definition,
    validate_idle_burst_config, validate_install_commands, CatalogDiagnostic, CatalogReadScope,
    CatalogSourceContext, CodingAgentDefinition, LayerDescription, ManagedCatalogMarker,
    CATALOG_SCHEMA_VERSION, KNOWN_CONFIG_SEED_FIELDS, KNOWN_DEFINITION_FIELDS, KNOWN_ENV_FIELDS,
    KNOWN_INSTALL_COMMANDS_FIELDS, KNOWN_ROOT_FIELDS, LOCAL_IDLE_BURST_FIELDS,
    MANAGED_MARKER_FIELDS, MANAGED_OWNER, MANAGED_VERSION, MIGRATION_SOURCE_INSTANCE,
    REPORT_CODE_BASE_INVALID, REPORT_CODE_BASE_UNAVAILABLE, REPORT_CODE_DUPLICATE_KEY,
    REPORT_CODE_INVALID_DEFINITION, REPORT_CODE_LOCAL_INVALID, REPORT_CODE_MANAGED_BASE_EDITED,
    REPORT_CODE_MIGRATION_CONFLICT, REPORT_CODE_MIGRATION_PENDING, REPORT_CODE_PROJECT_INVALID,
    REPORT_CODE_REFRESH_FAILED,
};

// ---------------------------------------------------------------------------
// Base analysis: ownership, content identity and readability.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogBaseKind {
    /// The managed marker carries this build's owner and version.
    Managed,
    /// A `managed` marker exists but is not ours (or is malformed): readable,
    /// but neither refreshed nor migrated.
    ForeignManaged,
    /// No marker at all: legacy user-owned territory.
    Legacy,
}

pub(crate) struct BaseAnalysis {
    pub(crate) kind: CatalogBaseKind,
    pub(crate) marker: Option<ManagedCatalogMarker>,
    pub(crate) entries: Vec<CodingAgentDefinition>,
    pub(crate) edited: bool,
    pub(crate) refresh_blocked: bool,
    pub(crate) warnings: Vec<CatalogDiagnostic>,
}

pub(crate) fn analyze_base_bytes(
    base_path: &Path,
    bytes: &[u8],
) -> Result<BaseAnalysis, CatalogDiagnostic> {
    let value = parse_strict_json(bytes).map_err(|reason| {
        catalog_diagnostic(
            REPORT_CODE_BASE_INVALID,
            base_path,
            format!("the persisted catalog is not valid JSON ({reason})"),
        )
    })?;
    let Some(root) = value.as_object() else {
        return Err(catalog_diagnostic(
            REPORT_CODE_BASE_INVALID,
            base_path,
            "the catalog root must be a JSON object",
        ));
    };
    if root
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        == Some(2)
    {
        let catalog = parse_catalog_schema2(bytes)
            .map_err(|e| catalog_diagnostic(REPORT_CODE_BASE_INVALID, base_path, e.to_string()))?;
        return Ok(BaseAnalysis {
            kind: CatalogBaseKind::ForeignManaged,
            marker: None,
            entries: catalog.agents,
            edited: false,
            refresh_blocked: true,
            warnings: Vec::new(),
        });
    }
    if let Some(version) = root.get("schemaVersion") {
        if version.as_u64() != Some(CATALOG_SCHEMA_VERSION as u64) {
            return Err(catalog_diagnostic(
                REPORT_CODE_BASE_INVALID,
                base_path,
                format!(
                    "unsupported explicit schemaVersion {version}; only schemaVersion 1 is \
                        recognized"
                ),
            ));
        }
    }
    let empty_rows = Vec::new();
    let rows = match root.get("agents") {
        None => &empty_rows,
        Some(serde_json::Value::Array(rows)) => rows,
        Some(_) => {
            return Err(catalog_diagnostic(
                REPORT_CODE_BASE_INVALID,
                base_path,
                "the catalog 'agents' value must be a JSON array",
            ))
        }
    };

    let mut warnings = Vec::new();
    let mut refresh_blocked =
        root.contains_key("profileBindingVersion") || root.contains_key("legacyProfileOrphans");
    if let Some(names) = unknown_field_names(root.keys(), KNOWN_ROOT_FIELDS) {
        warnings.push(catalog_diagnostic(
            REPORT_CODE_MIGRATION_PENDING,
            base_path,
            format!(
                "root field(s) ({names}) are not recognized by the current catalog schema and \
                    require managed-catalog migration"
            ),
        ));
        refresh_blocked = true;
    }

    let (kind, marker) = analyze_managed_marker(root, &mut refresh_blocked);

    let entries = analyze_base_entries(base_path, rows, &mut warnings, &mut refresh_blocked);

    let mut edited = false;
    if let Some(marker) = &marker {
        if managed_content_sha256(&entries) != marker.content_sha256 {
            edited = true;
            warnings.push(catalog_diagnostic(
                REPORT_CODE_MANAGED_BASE_EDITED,
                base_path,
                "the managed catalog content does not match its recorded contentSha256; it stays \
                    readable but is never auto-refreshed or auto-migrated",
            ));
        }
    }

    Ok(BaseAnalysis {
        kind,
        marker,
        entries,
        edited,
        refresh_blocked,
        warnings,
    })
}

fn analyze_base_entries(
    base_path: &Path,
    rows: &[serde_json::Value],
    warnings: &mut Vec<CatalogDiagnostic>,
    refresh_blocked: &mut bool,
) -> Vec<CodingAgentDefinition> {
    let mut entries: Vec<CodingAgentDefinition> = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();
    for (index, raw) in rows.iter().enumerate() {
        let Some(raw_object) = raw.as_object() else {
            warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                base_path,
                format!("entry {index} is not a JSON object and was omitted"),
            ));
            continue;
        };
        if let Some(reason) =
            raw_update_commands_problem(raw).or_else(|| raw_install_commands_problem(raw))
        {
            warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                base_path,
                format!("entry {index} was omitted: {reason}"),
            ));
            continue;
        }
        let definition: CodingAgentDefinition = match serde_json::from_value(raw.clone()) {
            Ok(definition) => definition,
            Err(_) => {
                warnings.push(catalog_diagnostic(
                    REPORT_CODE_INVALID_DEFINITION,
                    base_path,
                    format!(
                        "entry {index} does not match the coding-agent definition schema and was \
                            omitted"
                    ),
                ));
                continue;
            }
        };
        if validate_definition(&definition).is_err() {
            let label = if validate_catalog_key(&definition.key).is_ok() {
                format!("coding-agent '{}'", definition.key)
            } else {
                format!("entry {index}")
            };
            warnings.push(catalog_diagnostic(
                REPORT_CODE_INVALID_DEFINITION,
                base_path,
                format!(
                    "{label} was omitted: {}",
                    definition_problem_reason(&definition)
                ),
            ));
            continue;
        }
        if !seen_keys.insert(definition.key.clone()) {
            warnings.push(catalog_diagnostic(
                REPORT_CODE_DUPLICATE_KEY,
                base_path,
                format!(
                    "coding-agent '{}' was omitted: its key duplicates an earlier entry (the first \
                        entry wins)",
                    definition.key
                ),
            ));
            continue;
        }
        if !raw_object.contains_key("updateCommands") {
            warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                base_path,
                format!(
                    "coding-agent '{}' has no persisted updateCommands; absent update commands are \
                        suppressed during reads and require managed-catalog migration on a \
                        supported restart",
                    definition.key
                ),
            ));
        }
        if push_unknown_field_warnings(raw_object, &definition.key, base_path, warnings) {
            *refresh_blocked = true;
        }
        entries.push(definition);
    }

    entries
}

fn analyze_managed_marker(
    root: &serde_json::Map<String, serde_json::Value>,
    refresh_blocked: &mut bool,
) -> (CatalogBaseKind, Option<ManagedCatalogMarker>) {
    match root.get("managed") {
        None => (CatalogBaseKind::Legacy, None),
        Some(serde_json::Value::Object(object)) => {
            if unknown_field_names(object.keys(), MANAGED_MARKER_FIELDS).is_some() {
                *refresh_blocked = true;
            }
            let owner = object.get("owner").and_then(serde_json::Value::as_str);
            let version = object.get("version").and_then(serde_json::Value::as_u64);
            if owner == Some(MANAGED_OWNER) && version == Some(MANAGED_VERSION as u64) {
                match (
                    object.get("revision").and_then(serde_json::Value::as_str),
                    object
                        .get("contentSha256")
                        .and_then(serde_json::Value::as_str),
                ) {
                    (Some(revision), Some(content_sha256)) => (
                        CatalogBaseKind::Managed,
                        Some(ManagedCatalogMarker {
                            owner: MANAGED_OWNER.to_string(),
                            version: MANAGED_VERSION,
                            revision: revision.to_string(),
                            content_sha256: content_sha256.to_string(),
                        }),
                    ),
                    _ => (CatalogBaseKind::ForeignManaged, None),
                }
            } else {
                (CatalogBaseKind::ForeignManaged, None)
            }
        }
        Some(_) => (CatalogBaseKind::ForeignManaged, None),
    }
}

// ---------------------------------------------------------------------------
// Read snapshots and resolution.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CatalogSnapshot {
    pub(crate) base: Result<Option<Vec<u8>>, String>,
    // None is explicit exclusion: Instance/Direct never observe the project path.
    pub(crate) project: Option<Result<Option<Vec<u8>>, String>>,
    pub(crate) local: Result<Option<Vec<u8>>, String>,
    pub(crate) journal: Result<Option<Vec<u8>>, String>,
}

/// Bounded consecutive observations, not atomic publication by external editors.
pub(crate) fn read_catalog_snapshot(
    ac_dir: &Path,
    scope: CatalogReadScope,
) -> Result<CatalogSnapshot, String> {
    read_catalog_snapshot_with(ac_dir, scope, read_optional_regular_file)
}

pub(crate) fn read_catalog_snapshot_with(
    ac_dir: &Path,
    scope: CatalogReadScope,
    mut read: impl FnMut(&Path, &str) -> Result<Option<Vec<u8>>, String>,
) -> Result<CatalogSnapshot, String> {
    let mut previous = None;
    for _attempt in 0..3 {
        let snapshot = CatalogSnapshot {
            base: read(&manifest_path(ac_dir), "persisted catalog"),
            project: if scope == CatalogReadScope::Project {
                Some(read(
                    &project_catalog_path(ac_dir),
                    "project coding-agent catalog",
                ))
            } else {
                None
            },
            local: read(&local_catalog_path(ac_dir), "local coding-agent catalog"),
            journal: read(
                &migration_journal_path(ac_dir),
                "coding-agent migration journal",
            ),
        };
        if previous.as_ref() == Some(&snapshot) {
            return Ok(snapshot);
        }
        previous = Some(snapshot);
    }
    Err(if scope == CatalogReadScope::Project {
        "the catalog source, project overrides, local overrides and migration journal kept \
            changing while reading; retry the read"
    } else {
        "the catalog source, local overrides and migration journal kept changing while reading; \
            retry the read"
    }
    .to_string())
}

pub(crate) struct ResolvedCatalog {
    pub(crate) catalog: Vec<CodingAgentDefinition>,
    pub(crate) warnings: Vec<CatalogDiagnostic>,
    pub(crate) unavailable: Option<CatalogDiagnostic>,
    pub(crate) base_verified_managed: bool,
}

/// A local file that has NOT reached a managed base does not silently become
/// effective: it is diagnosed, and a legacy read stays legacy.
pub(crate) fn describe_local_presence(
    local: &Result<Option<Vec<u8>>, String>,
    local_path: &Path,
    warnings: &mut Vec<CatalogDiagnostic>,
    description: LayerDescription,
) {
    let layer = description.name();
    match local {
        Ok(None) => return,
        Ok(Some(local_bytes)) => {
            if let Err(reason) = parse_local_layer(local_bytes, description) {
                warnings.push(catalog_diagnostic(
                    description.invalid_code(),
                    local_path,
                    reason,
                ));
            }
        }
        Err(reason) => warnings.push(catalog_diagnostic(
            description.invalid_code(),
            local_path,
            reason.clone(),
        )),
    }
    warnings.push(catalog_diagnostic(
        REPORT_CODE_MIGRATION_PENDING,
        local_path,
        format!(
            "a {layer} overrides file exists while the base is not AC-managed; ownership \
            transfer is blocked until the managed migration completes and the {layer} layer is not \
            applied on top of a non-managed base"
        ),
    ));
}

/// Serve the verified legacy instance source recorded by an interrupted
/// instance import, when no managed base has been published yet.
pub(crate) fn journal_legacy_source(
    journal_bytes: &[u8],
) -> Result<Option<Vec<CodingAgentDefinition>>, String> {
    let journal = parse_migration_journal(journal_bytes)?;
    if journal.source_kind != MIGRATION_SOURCE_INSTANCE {
        return Ok(None);
    }
    let source_path = PathBuf::from(&journal.source_path);
    let Some(source_bytes) = read_instance_legacy_source(&source_path)? else {
        return Ok(None);
    };
    if sha256_hex(&source_bytes) != journal.source_sha256 {
        return Err(
            "the recorded migration source no longer matches the journal hash; recovery is blocked"
                .to_string(),
        );
    }
    let analysis = analyze_base_bytes(&source_path, &source_bytes).map_err(|diagnostic| {
        format!(
            "the recorded migration source is not readable: {}",
            diagnostic.code
        )
    })?;
    if analysis.kind != CatalogBaseKind::Legacy {
        return Ok(None);
    }
    Ok(Some(analysis.entries))
}

/// The single eligibility predicate for "this managed base is behind (or ahead
/// of) the current shipped revision": recognized ownership, a verified content
/// hash (not edited), no unknown fields blocking refresh, and a recorded
/// revision that differs from this build's shipped revision. The read-path
/// `refreshFailed` warning and `refresh_managed_base` both derive from this
/// predicate, so a warning can never promise a refresh the initializer would
/// refuse to perform.
pub(crate) fn managed_base_is_stale(analysis: &BaseAnalysis, shipped_revision: &str) -> bool {
    matches!(analysis.kind, CatalogBaseKind::Managed)
        && !analysis.edited
        && !analysis.refresh_blocked
        && analysis
            .marker
            .as_ref()
            .is_some_and(|marker| marker.revision != shipped_revision)
}

pub(crate) fn resolve_catalog_snapshot(
    ac_dir: &Path,
    snapshot: CatalogSnapshot,
    context: CatalogSourceContext,
) -> ResolvedCatalog {
    let base_path = manifest_path(ac_dir);
    let local_path = local_catalog_path(ac_dir);
    let journal_path = migration_journal_path(ac_dir);
    let mut resolved = ResolvedCatalog {
        catalog: Vec::new(),
        warnings: Vec::new(),
        unavailable: None,
        base_verified_managed: false,
    };

    let base_bytes = match &snapshot.base {
        Ok(Some(bytes)) => Some(bytes.clone()),
        Ok(None) => None,
        Err(reason) => {
            resolved.unavailable = Some(catalog_diagnostic(
                REPORT_CODE_BASE_UNAVAILABLE,
                &base_path,
                reason.clone(),
            ));
            return resolved;
        }
    };
    let Some(base_bytes) = base_bytes else {
        return resolve_missing_base(
            ac_dir,
            &snapshot,
            &base_path,
            &local_path,
            &journal_path,
            resolved,
        );
    };

    let analysis = match analyze_base_bytes(&base_path, &base_bytes) {
        Ok(analysis) => analysis,
        Err(diagnostic) => {
            resolved.unavailable = Some(diagnostic);
            return resolved;
        }
    };
    resolved.warnings.extend(analysis.warnings.iter().cloned());
    resolved.base_verified_managed = analysis.kind == CatalogBaseKind::Managed && !analysis.edited;
    // Read-path counterpart of the initialization refresh: a verified base at a
    // different shipped revision keeps serving its persisted entries (and the
    // local layer) while the restart guidance names the surface that can
    // actually act on it. A failed refresh needs no persistent marker - the
    // revision comparison itself recomputes this warning on every read.
    if managed_base_is_stale(
        &analysis,
        &managed_content_sha256(&supported_shipped_definitions()),
    ) {
        resolved.warnings.push(catalog_diagnostic(
            REPORT_CODE_REFRESH_FAILED,
            &base_path,
            context.stale_revision_reason(),
        ));
    }

    match analysis.kind {
        CatalogBaseKind::Managed => {
            let effective = compose_managed_layers(
                ac_dir,
                &snapshot,
                &local_path,
                analysis.entries,
                &mut resolved.warnings,
            );
            resolved.catalog = apply_support_gate(effective, &base_path, &mut resolved.warnings);
        }
        CatalogBaseKind::ForeignManaged => {
            resolved.warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_CONFLICT,
                &base_path,
                "the persisted catalog carries an unrecognized managed ownership marker; this \
                    build neither refreshes nor migrates it",
            ));
            if let Some(project) = &snapshot.project {
                describe_local_presence(
                    project,
                    &project_catalog_path(ac_dir),
                    &mut resolved.warnings,
                    LayerDescription::Project,
                );
            }
            describe_local_presence(
                &snapshot.local,
                &local_path,
                &mut resolved.warnings,
                LayerDescription::Local,
            );
            resolved.catalog =
                apply_support_gate(analysis.entries, &base_path, &mut resolved.warnings);
        }
        CatalogBaseKind::Legacy => {
            if let Some(project) = &snapshot.project {
                describe_local_presence(
                    project,
                    &project_catalog_path(ac_dir),
                    &mut resolved.warnings,
                    LayerDescription::Project,
                );
            }
            describe_local_presence(
                &snapshot.local,
                &local_path,
                &mut resolved.warnings,
                LayerDescription::Local,
            );
            resolved.catalog =
                apply_support_gate(analysis.entries, &base_path, &mut resolved.warnings);
        }
    }

    resolved
}

fn resolve_missing_base(
    ac_dir: &Path,
    snapshot: &CatalogSnapshot,
    base_path: &Path,
    local_path: &Path,
    journal_path: &Path,
    mut resolved: ResolvedCatalog,
) -> ResolvedCatalog {
    let journal_present = matches!(&snapshot.journal, Ok(Some(_)));
    if snapshot.journal.is_err() {
        let reason = snapshot.journal.as_ref().err().cloned().unwrap_or_default();
        resolved.warnings.push(catalog_diagnostic(
            REPORT_CODE_MIGRATION_CONFLICT,
            journal_path,
            reason,
        ));
    }
    if journal_present {
        resolved.warnings.push(catalog_diagnostic(
            REPORT_CODE_MIGRATION_PENDING,
            journal_path,
            "an interrupted managed-catalog migration is pending; it resumes on the next \
                initialization",
        ));
        let journal_bytes = snapshot
            .journal
            .as_ref()
            .ok()
            .and_then(|value| value.as_deref());
        if let Some(journal_bytes) = journal_bytes {
            match journal_legacy_source(journal_bytes) {
                Ok(Some(entries)) => {
                    resolved.catalog =
                        apply_support_gate(entries, base_path, &mut resolved.warnings);
                    return resolved;
                }
                Ok(None) => {}
                Err(reason) => resolved.warnings.push(catalog_diagnostic(
                    REPORT_CODE_MIGRATION_CONFLICT,
                    journal_path,
                    reason,
                )),
            }
        }
    }
    if let Some(project) = &snapshot.project {
        match project {
            Ok(None) => {}
            Ok(Some(_)) => resolved.warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                &project_catalog_path(ac_dir),
                "a project overrides file exists but no managed base has been published yet; \
                the managed base is created on the next initialization",
            )),
            Err(reason) => resolved.warnings.push(catalog_diagnostic(
                REPORT_CODE_PROJECT_INVALID,
                &project_catalog_path(ac_dir),
                reason.clone(),
            )),
        }
    }
    match &snapshot.local {
        Ok(None) => {}
        Ok(Some(_)) => resolved.warnings.push(catalog_diagnostic(
            REPORT_CODE_MIGRATION_PENDING,
            local_path,
            "a local overrides file exists but no managed base has been published yet; the \
                managed base is created on the next initialization",
        )),
        Err(reason) => resolved.warnings.push(catalog_diagnostic(
            REPORT_CODE_LOCAL_INVALID,
            local_path,
            reason.clone(),
        )),
    }
    resolved.unavailable = Some(catalog_diagnostic(
        REPORT_CODE_BASE_UNAVAILABLE,
        base_path,
        "no persisted catalog exists at this path; no embedded defaults are substituted",
    ));
    resolved
}

fn compose_managed_layers(
    ac_dir: &Path,
    snapshot: &CatalogSnapshot,
    local_path: &Path,
    mut effective: Vec<CodingAgentDefinition>,
    warnings: &mut Vec<CatalogDiagnostic>,
) -> Vec<CodingAgentDefinition> {
    for (observation, path, description) in snapshot
        .project
        .as_ref()
        .map(|value| {
            (
                value,
                project_catalog_path(ac_dir),
                LayerDescription::Project,
            )
        })
        .into_iter()
        .chain(std::iter::once((
            &snapshot.local,
            local_path.to_path_buf(),
            LayerDescription::Local,
        )))
    {
        let result = match observation {
            Ok(Some(bytes)) => parse_local_layer(bytes, description)
                .and_then(|layer| compose_local_layer(&effective, &layer, description)),
            Ok(None) => continue,
            Err(reason) => Err(reason.clone()),
        };
        match result {
            Ok(candidate) => effective = candidate,
            Err(reason) => warnings.push(catalog_diagnostic(
                description.invalid_code(),
                &path,
                reason,
            )),
        }
    }
    effective
}

/// Per-item validation of a raw `updateCommands` value BEFORE deserialization,
/// so a non-string or unsafe item reports on that definition instead of failing
/// the whole file as malformed JSON. Returns a reason on the first offending
/// item; the offending text is never echoed. Accepted strings are preserved
/// exactly by serde (spaces, quoting, shell operators and flags stay intact).
pub(crate) fn raw_update_commands_problem(raw: &serde_json::Value) -> Option<String> {
    let value = raw.get("updateCommands")?;
    let serde_json::Value::Array(items) = value else {
        return Some(
            "its updateCommands value must be an array of complete command strings".to_string(),
        );
    };
    for (index, item) in items.iter().enumerate() {
        let serde_json::Value::String(command) = item else {
            return Some(format!("its updateCommands item {index} is not a string"));
        };
        if command.trim().is_empty() {
            return Some(format!("its updateCommands item {index} is blank"));
        }
        if command
            .chars()
            .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
        {
            return Some(format!(
                "its updateCommands item {index} contains a Unicode control character, U+2028 or \
                    U+2029"
            ));
        }
    }
    None
}

/// #2736 - the raw `installCommands` counterpart of
/// `raw_update_commands_problem`: a sanitized reason for a present value that
/// is not an object, lacks a string `default`, carries a non-string per-OS
/// value, or a blank or control-character value. Text is never echoed.
pub(crate) fn raw_install_commands_problem(raw: &serde_json::Value) -> Option<String> {
    let value = raw.get("installCommands")?;
    let serde_json::Value::Object(object) = value else {
        return Some("its installCommands value must be an object".to_string());
    };
    if !matches!(object.get("default"), Some(serde_json::Value::String(_))) {
        return Some("its installCommands.default must be a string".to_string());
    }
    for name in KNOWN_INSTALL_COMMANDS_FIELDS {
        let Some(item) = object.get(*name) else {
            continue;
        };
        let serde_json::Value::String(command) = item else {
            return Some(format!("its installCommands.{name} is not a string"));
        };
        if command.trim().is_empty() {
            return Some(format!("its installCommands.{name} is blank"));
        }
        if command
            .chars()
            .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
        {
            return Some(format!(
                "its installCommands.{name} contains a Unicode control character, U+2028 or U+2029"
            ));
        }
    }
    None
}

/// A sanitized reason for a definition the existing validator rejects. The
/// validator's own message can embed the command text, which must never appear
/// in a diagnostic; this classifier only names the failing field category.
pub(crate) fn definition_problem_reason(def: &CodingAgentDefinition) -> String {
    if validate_catalog_key(&def.key).is_err() {
        return "its key is invalid (keys must be non-empty and match ^[a-z0-9-]+$)".to_string();
    }
    let context = format!("Coding agent '{}'", def.key);
    if validate_agent_command_text(&context, &def.command).is_err() {
        return "its command is rejected by the current coding-agent command rules".to_string();
    }
    if validate_env_rows(&def.envs, &context).is_err() {
        return "its environment rows are invalid (duplicate or unsafe keys)".to_string();
    }
    if let Some(name) = def.instructions_filename.as_deref() {
        if !is_safe_instructions_filename(name) {
            return "its instructions filename is not a safe file name".to_string();
        }
    }
    if let Some(cfg) = def.config_seed.as_ref() {
        if !cfg.dest.trim().is_empty() && validate_config_seed_dest(&cfg.dest).is_err() {
            return "its config-seed destination is not a valid folder name".to_string();
        }
    }
    if let Some(burst) = def.idle_burst.as_ref() {
        if validate_idle_burst_config(burst, "Coding agent").is_err() {
            return "its idleBurst values are invalid (numbers must be finite and >= 0)"
                .to_string();
        }
    }
    if let Some(install) = def.install_commands.as_ref() {
        if validate_install_commands(install, "Coding agent").is_err() {
            return "its installCommands values are invalid (blank or control characters)"
                .to_string();
        }
    }
    "it failed the current catalog definition validation rules".to_string()
}

/// The unknown field NAMES of `keys` relative to `known`, joined for a reason
/// string; `None` when every field is known. Values are never read here.
pub(crate) fn unknown_field_names<'a>(
    keys: impl Iterator<Item = &'a String>,
    known: &[&str],
) -> Option<String> {
    let names: Vec<&str> = keys
        .map(String::as_str)
        .filter(|name| !known.contains(name))
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

/// migrationPending warnings for unknown fields on an ACCEPTED row: the row
/// itself, its `configSeed` object and its `envs` rows. Only field names are
/// reported; field VALUES (which may be commands or environment values) never
/// appear.
pub(crate) fn push_unknown_field_warnings(
    raw: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    path: &Path,
    warnings: &mut Vec<CatalogDiagnostic>,
) -> bool {
    let mut found = false;
    if let Some(names) = unknown_field_names(raw.keys(), KNOWN_DEFINITION_FIELDS) {
        found = true;
        warnings.push(catalog_diagnostic(
            REPORT_CODE_MIGRATION_PENDING,
            path,
            format!(
                "coding-agent '{key}' carries unknown field(s) ({names}) that require \
                    managed-catalog migration before ownership transfer"
            ),
        ));
    }
    if let Some(serde_json::Value::Object(seed)) = raw.get("configSeed") {
        if let Some(names) = unknown_field_names(seed.keys(), KNOWN_CONFIG_SEED_FIELDS) {
            found = true;
            warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                path,
                format!(
                    "coding-agent '{key}' configSeed carries unknown field(s) ({names}) that \
                        require managed-catalog migration"
                ),
            ));
        }
    }
    if let Some(serde_json::Value::Object(burst)) = raw.get("idleBurst") {
        if let Some(names) = unknown_field_names(burst.keys(), LOCAL_IDLE_BURST_FIELDS) {
            found = true;
            warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                path,
                format!(
                    "coding-agent '{key}' idleBurst carries unknown field(s) ({names}) that \
                        require managed-catalog migration"
                ),
            ));
        }
    }
    if let Some(serde_json::Value::Object(install)) = raw.get("installCommands") {
        if let Some(names) = unknown_field_names(install.keys(), KNOWN_INSTALL_COMMANDS_FIELDS) {
            found = true;
            warnings.push(catalog_diagnostic(
                REPORT_CODE_MIGRATION_PENDING,
                path,
                format!(
                    "coding-agent '{key}' installCommands carries unknown field(s) ({names}) that \
                        require managed-catalog migration"
                ),
            ));
        }
    }
    if let Some(serde_json::Value::Array(envs)) = raw.get("envs") {
        for (index, entry) in envs.iter().enumerate() {
            let serde_json::Value::Object(map) = entry else {
                continue;
            };
            if let Some(names) = unknown_field_names(map.keys(), KNOWN_ENV_FIELDS) {
                found = true;
                warnings.push(catalog_diagnostic(
                    REPORT_CODE_MIGRATION_PENDING,
                    path,
                    format!(
                        "coding-agent '{key}' envs[{index}] carries unknown field(s) ({names}) \
                            that require managed-catalog migration"
                    ),
                ));
            }
        }
    }
    found
}
