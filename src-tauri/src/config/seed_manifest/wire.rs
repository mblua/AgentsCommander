use super::{ResourceBoundKind, MAX_FIELD_BYTES, MAX_MANIFEST_ROWS};
use serde::de::{SeqAccess, Visitor};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestPathEncoding {
    UnixBytesHex,
    Utf8,
    WindowsUtf16Hex,
}

impl ManifestPathEncoding {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UnixBytesHex => "unix_bytes_hex",
            Self::Utf8 => "utf8",
            Self::WindowsUtf16Hex => "windows_utf16_hex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestFileKind {
    ProjectContextTemplate,
    ReplicaConfigFile,
    /// #1318 - the coding-agent catalog manifest published per project at
    /// `.ac/coding-agents/agents.json` (scope `catalog:coding-agents`).
    CodingAgentCatalog,
}

impl ManifestFileKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ProjectContextTemplate => "project_context_template",
            Self::ReplicaConfigFile => "replica_config_file",
            Self::CodingAgentCatalog => "coding_agent_catalog",
        }
    }

    pub(crate) fn is_canonical_output(self) -> bool {
        match self {
            Self::ProjectContextTemplate => true,
            Self::ReplicaConfigFile => false,
            Self::CodingAgentCatalog => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestSource {
    Builtin,
    WorkspaceProfile,
    WorkspaceBase,
    MatrixProfile,
    MatrixBase,
    CatalogDefault,
}

impl ManifestSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::WorkspaceProfile => "workspace_profile",
            Self::WorkspaceBase => "workspace_base",
            Self::MatrixProfile => "matrix_profile",
            Self::MatrixBase => "matrix_base",
            Self::CatalogDefault => "catalog_default",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeedManifestWire {
    pub(crate) schema_version: u32,
    pub(crate) coverage_version: u32,
    pub(crate) coverage: Vec<String>,
    #[serde(deserialize_with = "deserialize_rows")]
    pub(crate) files: Vec<SeedManifestRowWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeedManifestRowWire {
    #[serde(deserialize_with = "deserialize_path_string")]
    pub(crate) path: String,
    pub(crate) path_encoding: ManifestPathEncoding,
    pub(crate) kind: ManifestFileKind,
    #[serde(deserialize_with = "deserialize_scope_string")]
    pub(crate) scope: String,
    pub(crate) source: ManifestSource,
    pub(crate) last_seeded_at: String,
}

pub(crate) fn deserialize_path_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_string(LimitedStringVisitor {
        label: "path",
        bound: ResourceBoundKind::PathBytes,
    })
}

pub(crate) fn deserialize_scope_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_string(LimitedStringVisitor {
        label: "scope",
        bound: ResourceBoundKind::ScopeBytes,
    })
}

pub(crate) struct LimitedStringVisitor {
    pub(crate) label: &'static str,
    pub(crate) bound: ResourceBoundKind,
}

impl Visitor<'_> for LimitedStringVisitor {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "a {} string no longer than {} UTF-8 bytes",
            self.label, MAX_FIELD_BYTES
        )
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.len() > MAX_FIELD_BYTES {
            return Err(E::custom(format!(
                "{} {} bound exceeded: limit={}, observed_at_least={}",
                self.label,
                self.bound,
                MAX_FIELD_BYTES,
                value.len()
            )));
        }
        Ok(value.to_string())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.len() > MAX_FIELD_BYTES {
            return Err(E::custom(format!(
                "{} {} bound exceeded: limit={}, observed_at_least={}",
                self.label,
                self.bound,
                MAX_FIELD_BYTES,
                value.len()
            )));
        }
        Ok(value)
    }
}

pub(crate) fn deserialize_rows<'de, D>(
    deserializer: D,
) -> Result<Vec<SeedManifestRowWire>, D::Error>
where
    D: Deserializer<'de>,
{
    struct RowsVisitor;

    impl<'de> Visitor<'de> for RowsVisitor {
        type Value = Vec<SeedManifestRowWire>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "at most {} seed-manifest rows",
                MAX_MANIFEST_ROWS
            )
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let capacity = sequence.size_hint().unwrap_or(0).min(MAX_MANIFEST_ROWS);
            let mut rows = Vec::with_capacity(capacity);
            while let Some(row) = sequence.next_element()? {
                if rows.len() == MAX_MANIFEST_ROWS {
                    return Err(de::Error::custom(format!(
                        "rows {} bound exceeded: limit={}, observed_at_least={}",
                        ResourceBoundKind::Rows,
                        MAX_MANIFEST_ROWS,
                        MAX_MANIFEST_ROWS + 1
                    )));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }

    deserializer.deserialize_seq(RowsVisitor)
}
