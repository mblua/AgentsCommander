use super::{
    classify_open_error, is_exact_not_found, open_regular_no_follow, parse_utf8_components,
    ManifestFileKind, ManifestPathEncoding, ManifestPathIdentity, ManifestPathKey, ManifestSource,
    OpenedRegularFile, ResourceBoundKind, SeedManifestError, SeedManifestRowWire, SeedManifestWire,
    COVERAGE, COVERAGE_VERSION, MANAGED_HEADER, MAX_FIELD_BYTES, MAX_MANIFEST_BYTES,
    MAX_MANIFEST_ROWS, SCHEMA_VERSION, SEED_MANIFEST_FILENAME,
};
use chrono::{DateTime, SecondsFormat, Utc};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublishedManifestRow {
    pub(crate) path: ManifestPathIdentity,
    pub(crate) kind: ManifestFileKind,
    pub(crate) scope: String,
    pub(crate) source: ManifestSource,
    pub(crate) published_at: DateTime<Utc>,
}

impl PublishedManifestRow {
    pub(crate) fn project_context(
        path: ManifestPathIdentity,
        scope: &str,
        published_at: DateTime<Utc>,
    ) -> Result<Self, SeedManifestError> {
        let row = Self {
            path,
            kind: ManifestFileKind::ProjectContextTemplate,
            scope: scope.to_string(),
            source: ManifestSource::Builtin,
            published_at,
        };
        validate_row(&row)?;
        Ok(row)
    }

    #[cfg(test)]
    pub(crate) fn replica_config(
        path: ManifestPathIdentity,
        scope: String,
        source: ManifestSource,
        published_at: DateTime<Utc>,
    ) -> Result<Self, SeedManifestError> {
        let row = Self {
            path,
            kind: ManifestFileKind::ReplicaConfigFile,
            scope,
            source,
            published_at,
        };
        validate_row(&row)?;
        Ok(row)
    }

    /// #1318 - one coding-agent catalog publication row: `.ac/coding-agents/
    /// agents.json`, `builtin` source, scope `catalog:coding-agents`. Records
    /// the WRITE, not content validity: a verbatim-migrated legacy catalog that
    /// does not parse still records its copy (the read path self-heals).
    pub(crate) fn coding_agent_catalog(
        path: ManifestPathIdentity,
        published_at: DateTime<Utc>,
    ) -> Result<Self, SeedManifestError> {
        let row = Self {
            path,
            kind: ManifestFileKind::CodingAgentCatalog,
            scope: "catalog:coding-agents".to_string(),
            source: ManifestSource::Builtin,
            published_at,
        };
        validate_row(&row)?;
        Ok(row)
    }

    pub(crate) fn to_wire(&self) -> SeedManifestRowWire {
        SeedManifestRowWire {
            path: self.path.serialized.clone(),
            path_encoding: self.path.encoding,
            kind: self.kind,
            scope: self.scope.clone(),
            source: self.source,
            last_seeded_at: canonical_timestamp(self.published_at),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ManifestState {
    pub(crate) rows: BTreeMap<ManifestPathKey, PublishedManifestRow>,
}

impl ManifestState {
    pub(crate) fn from_wire(wire: SeedManifestWire) -> Result<Self, SeedManifestError> {
        if wire.schema_version > SCHEMA_VERSION {
            return Err(SeedManifestError::FutureSchema {
                found: wire.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        if wire.schema_version != SCHEMA_VERSION {
            return Err(SeedManifestError::Validation(format!(
                "schema_version must be {SCHEMA_VERSION}, found {}",
                wire.schema_version
            )));
        }
        if wire.coverage_version != COVERAGE_VERSION {
            return Err(SeedManifestError::Validation(format!(
                "coverage_version must be {COVERAGE_VERSION}, found {}",
                wire.coverage_version
            )));
        }
        let expected_coverage = COVERAGE.map(str::to_string).to_vec();
        if wire.coverage != expected_coverage {
            return Err(SeedManifestError::Validation(format!(
                "coverage must equal {:?} in that order",
                COVERAGE
            )));
        }
        if wire.files.len() > MAX_MANIFEST_ROWS {
            return Err(SeedManifestError::resource_bound(
                ResourceBoundKind::Rows,
                MAX_MANIFEST_ROWS,
                wire.files.len(),
            ));
        }

        let mut rows = BTreeMap::new();
        let mut config_batches: BTreeMap<String, (ManifestSource, String)> = BTreeMap::new();
        for row in wire.files {
            if row.path.len() > MAX_FIELD_BYTES {
                return Err(SeedManifestError::resource_bound(
                    ResourceBoundKind::PathBytes,
                    MAX_FIELD_BYTES,
                    row.path.len(),
                ));
            }
            if row.scope.len() > MAX_FIELD_BYTES {
                return Err(SeedManifestError::resource_bound(
                    ResourceBoundKind::ScopeBytes,
                    MAX_FIELD_BYTES,
                    row.scope.len(),
                ));
            }
            let path = ManifestPathIdentity::parse(row.path_encoding, row.path)?;
            let published_at = parse_canonical_timestamp(&row.last_seeded_at)?;
            let internal = PublishedManifestRow {
                path,
                kind: row.kind,
                scope: row.scope,
                source: row.source,
                published_at,
            };
            validate_row(&internal)?;
            if internal.kind == ManifestFileKind::ReplicaConfigFile {
                let canonical_time = canonical_timestamp(internal.published_at);
                match config_batches.get(&internal.scope) {
                    Some((source, time))
                        if *source != internal.source || *time != canonical_time =>
                    {
                        return Err(SeedManifestError::Validation(format!(
                            "config scope {} contains mixed source or publication time",
                            internal.scope
                        )));
                    }
                    Some(_) => {}
                    None => {
                        config_batches
                            .insert(internal.scope.clone(), (internal.source, canonical_time));
                    }
                }
            }
            let key = internal.path.key();
            if rows.insert(key, internal).is_some() {
                return Err(SeedManifestError::Validation(
                    "duplicate manifest path identity".to_string(),
                ));
            }
        }
        Ok(Self { rows })
    }

    pub(crate) fn canonical_rows(&self) -> impl Iterator<Item = &PublishedManifestRow> {
        self.rows
            .values()
            .filter(|row| row.kind.is_canonical_output())
    }

    pub(crate) fn to_wire(&self) -> SeedManifestWire {
        SeedManifestWire {
            schema_version: SCHEMA_VERSION,
            coverage_version: COVERAGE_VERSION,
            coverage: COVERAGE.map(str::to_string).to_vec(),
            files: self
                .canonical_rows()
                .map(PublishedManifestRow::to_wire)
                .collect(),
        }
    }

    pub(crate) fn upsert(&mut self, row: PublishedManifestRow) -> RowMutationJournal {
        let key = row.path.key();
        let previous = self.rows.insert(key.clone(), row);
        RowMutationJournal { key, previous }
    }

    #[cfg(test)]
    pub(crate) fn replace_scope(
        &mut self,
        scope: &str,
        replacement: Vec<PublishedManifestRow>,
    ) -> ScopeMutationJournal {
        let removed_keys = self
            .rows
            .iter()
            .filter_map(|(key, row)| (row.scope == scope).then_some(key.clone()))
            .collect::<Vec<_>>();
        let mut removed = Vec::with_capacity(removed_keys.len());
        for key in removed_keys {
            if let Some(row) = self.rows.remove(&key) {
                removed.push((key, row));
            }
        }
        let mut inserted = Vec::with_capacity(replacement.len());
        for row in replacement {
            let key = row.path.key();
            self.rows.insert(key.clone(), row);
            inserted.push(key);
        }
        ScopeMutationJournal { removed, inserted }
    }

    pub(crate) fn remove_scope(&mut self, scope: &str) -> RemovalMutationJournal {
        self.remove_matching(|row| row.scope == scope)
    }

    pub(crate) fn remove_matching(
        &mut self,
        predicate: impl Fn(&PublishedManifestRow) -> bool,
    ) -> RemovalMutationJournal {
        let keys = self
            .rows
            .iter()
            .filter_map(|(key, row)| predicate(row).then_some(key.clone()))
            .collect::<Vec<_>>();
        let mut removed = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(row) = self.rows.remove(&key) {
                removed.push((key, row));
            }
        }
        RemovalMutationJournal { removed }
    }
}

#[derive(Debug)]
pub(crate) struct RowMutationJournal {
    pub(crate) key: ManifestPathKey,
    pub(crate) previous: Option<PublishedManifestRow>,
}

impl RowMutationJournal {
    pub(crate) fn changed(&self, state: &ManifestState) -> bool {
        match (&self.previous, state.rows.get(&self.key)) {
            (Some(previous), Some(current)) => previous != current,
            (None, Some(_)) => true,
            _ => false,
        }
    }

    pub(crate) fn rollback(self, state: &mut ManifestState) {
        match self.previous {
            Some(row) => {
                state.rows.insert(self.key, row);
            }
            None => {
                state.rows.remove(&self.key);
            }
        }
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct ScopeMutationJournal {
    pub(crate) removed: Vec<(ManifestPathKey, PublishedManifestRow)>,
    pub(crate) inserted: Vec<ManifestPathKey>,
}

#[cfg(test)]
impl ScopeMutationJournal {
    pub(crate) fn changed(&self, state: &ManifestState) -> bool {
        if self.removed.len() != self.inserted.len() {
            return true;
        }
        self.removed
            .iter()
            .any(|(key, previous)| state.rows.get(key) != Some(previous))
    }

    pub(crate) fn rollback(self, state: &mut ManifestState) {
        for key in self.inserted {
            state.rows.remove(&key);
        }
        for (key, row) in self.removed {
            state.rows.insert(key, row);
        }
    }
}

#[derive(Debug)]
pub(crate) struct RemovalMutationJournal {
    pub(crate) removed: Vec<(ManifestPathKey, PublishedManifestRow)>,
}

impl RemovalMutationJournal {
    pub(crate) fn changed(&self) -> bool {
        !self.removed.is_empty()
    }

    pub(crate) fn rollback(self, state: &mut ManifestState) {
        for (key, row) in self.removed {
            state.rows.insert(key, row);
        }
    }
}

pub(crate) fn validate_row(row: &PublishedManifestRow) -> Result<(), SeedManifestError> {
    if row.path.serialized.len() > MAX_FIELD_BYTES {
        return Err(SeedManifestError::resource_bound(
            ResourceBoundKind::PathBytes,
            MAX_FIELD_BYTES,
            row.path.serialized.len(),
        ));
    }
    if row.scope.len() > MAX_FIELD_BYTES {
        return Err(SeedManifestError::resource_bound(
            ResourceBoundKind::ScopeBytes,
            MAX_FIELD_BYTES,
            row.scope.len(),
        ));
    }

    match row.kind {
        ManifestFileKind::ProjectContextTemplate => {
            if row.path.encoding != ManifestPathEncoding::Utf8
                || row.source != ManifestSource::Builtin
            {
                return Err(SeedManifestError::Validation(
                    "context rows require utf8 encoding and builtin source".to_string(),
                ));
            }
            let expected_scope = match row.path.serialized.as_str() {
                ".ac/Context.AgentsCommander.md" => "context:agentscommander",
                ".ac/Context.coordinator.md" => "context:coordinator",
                // #1605: the per-platform `{{HOST_PLATFORM_RULES}}` files join the
                // project-context scope set (mirrors
                // `manifest_scope_for_project_context_filename` in session_context.rs;
                // literals kept here to avoid a new module arc).
                ".ac/Context.platform.windows.md" => "context:platform",
                ".ac/Context.platform.linux.md" => "context:platform",
                ".ac/Context.platform.macos.md" => "context:platform",
                _ => {
                    return Err(SeedManifestError::Validation(format!(
                        "unsupported project context path {}",
                        row.path.serialized
                    )))
                }
            };
            if row.scope != expected_scope {
                return Err(SeedManifestError::Validation(format!(
                    "context path {} requires scope {}",
                    row.path.serialized, expected_scope
                )));
            }
        }
        ManifestFileKind::ReplicaConfigFile => {
            if row.source == ManifestSource::Builtin {
                return Err(SeedManifestError::Validation(
                    "replica config rows cannot use builtin source".to_string(),
                ));
            }
            let scope_components = parse_config_scope(&row.scope)?;
            if !row.path.is_strict_descendant_of_utf8(&scope_components) {
                return Err(SeedManifestError::Validation(format!(
                    "config row {} is outside scope {}",
                    row.path.serialized, row.scope
                )));
            }
        }
        ManifestFileKind::CodingAgentCatalog => {
            if row.path.encoding != ManifestPathEncoding::Utf8
                || row.source != ManifestSource::Builtin
            {
                return Err(SeedManifestError::Validation(
                    "coding-agent catalog rows require utf8 encoding and builtin source"
                        .to_string(),
                ));
            }
            if row.path.serialized != ".ac/coding-agents/agents.json" {
                return Err(SeedManifestError::Validation(format!(
                    "unsupported coding-agent catalog path {}",
                    row.path.serialized
                )));
            }
            if row.scope != "catalog:coding-agents" {
                return Err(SeedManifestError::Validation(format!(
                    "coding-agent catalog path {} requires scope catalog:coding-agents",
                    row.path.serialized
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn parse_config_scope(scope: &str) -> Result<Vec<String>, SeedManifestError> {
    if scope.len() > MAX_FIELD_BYTES {
        return Err(SeedManifestError::resource_bound(
            ResourceBoundKind::ScopeBytes,
            MAX_FIELD_BYTES,
            scope.len(),
        ));
    }
    let path = scope
        .strip_prefix("config:")
        .ok_or_else(|| SeedManifestError::Validation(format!("invalid config scope {scope:?}")))?;
    let components = parse_utf8_components(path)?;
    if components.len() != 4 {
        return Err(SeedManifestError::Validation(format!(
            "config scope must be config:.ac/<room>/<replica>/<dest>: {scope}"
        )));
    }
    crate::commands::entity_creation::parse_team_from_workgroup_name(&components[1])
        .map_err(SeedManifestError::Validation)?;
    let agent = components[2].strip_prefix("__agent_").ok_or_else(|| {
        SeedManifestError::Validation(format!(
            "config scope replica must be named __agent_<name>: {}",
            components[2]
        ))
    })?;
    crate::commands::entity_creation::validate_existing_name(agent, "Agent")
        .map_err(SeedManifestError::Validation)?;
    crate::config::settings::validate_config_seed_dest(&components[3])
        .map_err(SeedManifestError::Validation)?;
    Ok(components)
}

pub(crate) fn parse_canonical_timestamp(value: &str) -> Result<DateTime<Utc>, SeedManifestError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|error| {
        SeedManifestError::Validation(format!("invalid last_seeded_at {value:?}: {error}"))
    })?;
    let utc = parsed.with_timezone(&Utc);
    if canonical_timestamp(utc) != value {
        return Err(SeedManifestError::Validation(format!(
            "last_seeded_at is not canonical UTC milliseconds: {value:?}"
        )));
    }
    Ok(utc)
}

pub(crate) fn canonical_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(crate) fn checked_add(total: &mut usize, value: usize) -> Result<(), SeedManifestError> {
    *total = total.checked_add(value).ok_or_else(|| {
        SeedManifestError::resource_bound(
            ResourceBoundKind::ArithmeticOverflow,
            usize::MAX,
            usize::MAX,
        )
    })?;
    Ok(())
}

pub(crate) fn toml_basic_string_escaped_len(value: &str) -> Result<usize, SeedManifestError> {
    let mut length = 0_usize;
    for character in value.chars() {
        let encoded = match character {
            '\u{0008}' | '\t' | '\n' | '\u{000c}' | '\r' | '"' | '\\' => 2,
            '\u{0000}'..='\u{001f}' | '\u{007f}' => 6,
            _ => character.len_utf8(),
        };
        checked_add(&mut length, encoded)?;
    }
    Ok(length)
}

/// Exact serialized contribution of one config row in the fixed v1 wire
/// layout. Config staging uses this before retaining another path identity so a
/// successful physical copy never requires an unbounded metadata allocation.
pub(crate) fn exact_config_row_serialized_len(
    project_relative_path: &Path,
    scope: &str,
    source: ManifestSource,
) -> Result<usize, SeedManifestError> {
    let path = ManifestPathIdentity::from_relative_path(project_relative_path)?;
    parse_config_scope(scope)?;
    let mut length = "\n[[files]]\n".len();
    checked_add(&mut length, "path = \"\"\n".len())?;
    checked_add(
        &mut length,
        toml_basic_string_escaped_len(&path.serialized)?,
    )?;
    checked_add(&mut length, "path_encoding = \"\"\n".len())?;
    checked_add(
        &mut length,
        toml_basic_string_escaped_len(path.encoding.as_str())?,
    )?;
    checked_add(&mut length, "kind = \"\"\n".len())?;
    checked_add(
        &mut length,
        toml_basic_string_escaped_len(ManifestFileKind::ReplicaConfigFile.as_str())?,
    )?;
    checked_add(&mut length, "scope = \"\"\n".len())?;
    checked_add(&mut length, toml_basic_string_escaped_len(scope)?)?;
    checked_add(&mut length, "source = \"\"\n".len())?;
    checked_add(&mut length, toml_basic_string_escaped_len(source.as_str())?)?;
    checked_add(&mut length, "last_seeded_at = \"\"\n".len())?;
    checked_add(
        &mut length,
        toml_basic_string_escaped_len("1970-01-01T00:00:00.000Z")?,
    )?;
    Ok(length)
}

pub(crate) fn config_batch_base_serialized_len() -> usize {
    MANAGED_HEADER.len()
        + "schema_version = 1\n".len()
        + "coverage_version = 2\n".len()
        + "coverage = [\"project_context_templates\", \"replica_config_folders\", \
            \"coding_agent_catalog\"]\n"
            .len()
}

pub(crate) fn exact_serialized_len(state: &ManifestState) -> Result<usize, SeedManifestError> {
    exact_serialized_len_with_limit(state, MAX_MANIFEST_BYTES)
}

pub(crate) fn exact_serialized_len_with_limit(
    state: &ManifestState,
    output_limit: u64,
) -> Result<usize, SeedManifestError> {
    let canonical_row_count = state.canonical_rows().count();
    if canonical_row_count > MAX_MANIFEST_ROWS {
        return Err(SeedManifestError::resource_bound(
            ResourceBoundKind::Rows,
            MAX_MANIFEST_ROWS,
            canonical_row_count,
        ));
    }

    let mut length = MANAGED_HEADER.len();
    checked_add(&mut length, "schema_version = 1\n".len())?;
    checked_add(&mut length, "coverage_version = 2\n".len())?;
    checked_add(
        &mut length,
        "coverage = [\"project_context_templates\", \"replica_config_folders\", \
            \"coding_agent_catalog\"]\n"
            .len(),
    )?;
    if canonical_row_count == 0 {
        checked_add(&mut length, "files = []\n".len())?;
    } else {
        for row in state.canonical_rows() {
            checked_add(&mut length, "\n[[files]]\n".len())?;
            checked_add(&mut length, "path = \"\"\n".len())?;
            checked_add(
                &mut length,
                toml_basic_string_escaped_len(&row.path.serialized)?,
            )?;
            checked_add(&mut length, "path_encoding = \"\"\n".len())?;
            checked_add(
                &mut length,
                toml_basic_string_escaped_len(row.path.encoding.as_str())?,
            )?;
            checked_add(&mut length, "kind = \"\"\n".len())?;
            checked_add(
                &mut length,
                toml_basic_string_escaped_len(row.kind.as_str())?,
            )?;
            checked_add(&mut length, "scope = \"\"\n".len())?;
            checked_add(&mut length, toml_basic_string_escaped_len(&row.scope)?)?;
            checked_add(&mut length, "source = \"\"\n".len())?;
            checked_add(
                &mut length,
                toml_basic_string_escaped_len(row.source.as_str())?,
            )?;
            checked_add(&mut length, "last_seeded_at = \"\"\n".len())?;
            checked_add(
                &mut length,
                toml_basic_string_escaped_len(&canonical_timestamp(row.published_at))?,
            )?;
        }
    }
    if u64::try_from(length).unwrap_or(u64::MAX) > output_limit {
        return Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::OutputBytes,
            limit: output_limit,
            observed_at_least: u64::try_from(length).unwrap_or(u64::MAX),
        });
    }
    Ok(length)
}

pub(crate) fn serialize_state(state: &ManifestState) -> Result<Vec<u8>, SeedManifestError> {
    let exact_length = exact_serialized_len(state)?;
    let body = toml::to_string(&state.to_wire())
        .map_err(|error| SeedManifestError::Serialize(error.to_string()))?;
    let body = body.trim_end_matches('\n');
    let mut bytes = Vec::with_capacity(exact_length);
    bytes.extend_from_slice(MANAGED_HEADER.as_bytes());
    bytes.extend_from_slice(body.as_bytes());
    bytes.push(b'\n');
    if bytes.len() != exact_length {
        return Err(SeedManifestError::SerializedSizeMismatch {
            counted: exact_length,
            actual: bytes.len(),
        });
    }
    Ok(bytes)
}

pub(crate) fn parse_manifest_bytes(raw: &[u8]) -> Result<ManifestState, SeedManifestError> {
    let text = std::str::from_utf8(raw).map_err(|error| {
        SeedManifestError::Parse(format!(
            "manifest is not UTF-8 at byte offset {} with invalid_sequence_length={}",
            error.valid_up_to(),
            error.error_len().map_or(0, usize::from)
        ))
    })?;
    let wire: SeedManifestWire = toml::from_str(text).map_err(classify_toml_parse_error)?;
    ManifestState::from_wire(wire).map_err(sanitize_canonical_validation_error)
}

pub(crate) fn classify_toml_parse_error(error: toml::de::Error) -> SeedManifestError {
    let message = error.message();
    let bound = if message.starts_with("path path bytes bound exceeded:") {
        Some(ResourceBoundKind::PathBytes)
    } else if message.starts_with("scope scope bytes bound exceeded:") {
        Some(ResourceBoundKind::ScopeBytes)
    } else if message.starts_with("rows rows bound exceeded:") {
        Some(ResourceBoundKind::Rows)
    } else {
        None
    };
    match bound {
        Some(ResourceBoundKind::PathBytes) => SeedManifestError::resource_bound(
            ResourceBoundKind::PathBytes,
            MAX_FIELD_BYTES,
            MAX_FIELD_BYTES.saturating_add(1),
        ),
        Some(ResourceBoundKind::ScopeBytes) => SeedManifestError::resource_bound(
            ResourceBoundKind::ScopeBytes,
            MAX_FIELD_BYTES,
            MAX_FIELD_BYTES.saturating_add(1),
        ),
        Some(ResourceBoundKind::Rows) => SeedManifestError::resource_bound(
            ResourceBoundKind::Rows,
            MAX_MANIFEST_ROWS,
            MAX_MANIFEST_ROWS.saturating_add(1),
        ),
        _ => match error.span() {
            Some(span) => SeedManifestError::Parse(format!(
                "invalid TOML syntax or schema at byte range {}..{}",
                span.start, span.end
            )),
            None => SeedManifestError::Parse("invalid TOML syntax or schema".to_string()),
        },
    }
}

pub(crate) fn sanitize_canonical_validation_error(error: SeedManifestError) -> SeedManifestError {
    match error {
        SeedManifestError::Validation(_) => SeedManifestError::Validation(
            "canonical fields failed strict v1 validation".to_string(),
        ),
        other => other,
    }
}

pub(crate) fn read_bounded_file(
    file: &mut File,
    known_length: u64,
    limit: u64,
    path: &Path,
) -> Result<Vec<u8>, SeedManifestError> {
    let read_limit = limit.checked_add(1).ok_or_else(|| {
        SeedManifestError::resource_bound(
            ResourceBoundKind::ArithmeticOverflow,
            usize::MAX,
            usize::MAX,
        )
    })?;
    let capacity_u64 = known_length.min(read_limit);
    let capacity = usize::try_from(capacity_u64).map_err(|_| SeedManifestError::ResourceBound {
        kind: ResourceBoundKind::InputBytes,
        limit,
        observed_at_least: capacity_u64,
    })?;
    let mut raw = Vec::with_capacity(capacity);
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.take(read_limit).read_to_end(&mut raw))
        .map_err(|source| SeedManifestError::BoundedRead {
            path: path.to_path_buf(),
            source,
        })?;
    if u64::try_from(raw.len()).unwrap_or(u64::MAX) > limit {
        return Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::InputBytes,
            limit,
            observed_at_least: u64::try_from(raw.len()).unwrap_or(u64::MAX),
        });
    }
    Ok(raw)
}

/// #1968 - narrow READ-ONLY query: does this project's seed manifest carry a
/// canonical coding-agent catalog publication row for
/// `.ac/coding-agents/agents.json`?
///
/// Deliberately not a guard: it takes no lock, creates no directory, upgrades no
/// schema and writes nothing, so a caller may use it to decide whether
/// bookkeeping is missing. It reuses the canonical reader stack
/// ([`open_regular_no_follow`] + [`OpenedRegularFile`] + [`read_bounded_file`] +
/// [`parse_manifest_bytes`]), revalidates the opened identity across the read,
/// and compares the PARSED [`ManifestPathIdentity`] (never raw path text) plus
/// the canonical row kind, the catalog scope and the `Builtin` source.
///
/// * `Ok(true)` - at least one parsed canonical row matches.
/// * `Ok(false)` - the manifest is absent (exact NotFound) or carries no such
///   row. Absence of proof is not proof, so the caller reports an untracked
///   publication rather than assuming the row was ever written.
/// * `Err` - malformed, future, oversized, nonregular, link or unreadable
///   manifests.
pub(crate) fn has_catalog_publication(project_root: &Path) -> Result<bool, SeedManifestError> {
    let canonical_path = project_root
        .join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR)
        .join(SEED_MANIFEST_FILENAME);
    let mut canonical = match open_regular_no_follow(&canonical_path, false, false) {
        Ok(file) => OpenedRegularFile::from_file(&canonical_path, file)?,
        Err(error) if is_exact_not_found(&error) => return Ok(false),
        Err(error) => {
            return Err(classify_open_error(
                &canonical_path,
                error,
                "open canonical manifest",
                false,
            ))
        }
    };
    let raw = read_bounded_file(
        &mut canonical.file,
        canonical.length,
        MAX_MANIFEST_BYTES,
        &canonical_path,
    )?;
    let reopened = OpenedRegularFile::open_existing(&canonical_path, false)?;
    if reopened.identity != canonical.identity {
        return Err(SeedManifestError::UnsafePath {
            path: canonical_path,
            reason: "canonical identity changed during its bounded read".to_string(),
        });
    }
    let state = parse_manifest_bytes(&raw)?;
    let identity =
        ManifestPathIdentity::from_relative_path(Path::new(".ac/coding-agents/agents.json"))?;
    Ok(state.rows.values().any(|row| {
        row.path == identity
            && row.kind == ManifestFileKind::CodingAgentCatalog
            && row.scope == "catalog:coding-agents"
            && row.source == ManifestSource::Builtin
    }))
}
