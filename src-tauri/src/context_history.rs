//! Context history records (#3532): pure DTOs and their validation.
//!
//! This parent only declares its children. `config` owns `.ac/heatmap.json`
//! and `store` owns `.ac/context-history/`; both import these DTOs, and this
//! module never imports, calls or re-exports them.
// Dormant until the history observation (#3532 P02) produces records.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(crate) mod config;
pub(crate) mod store;

/// Record schema written by this build.
pub(crate) const SCHEMA_VERSION: u32 = 2;
/// Maximum UTF-8 bytes of one identity string (paths, keys, FQN, team ids).
pub(crate) const MAX_IDENTITY_BYTES: usize = 1024;
/// Maximum UTF-8 bytes of one launch metadata string.
pub(crate) const MAX_METADATA_BYTES: usize = 256;
/// Maximum number of team ids on one record.
pub(crate) const MAX_TEAM_IDS: usize = 128;
/// Maximum serialized JSON size of one record.
pub(crate) const MAX_RECORD_JSON_BYTES: usize = 16 * 1024;

const REPLICA_PREFIX: &str = "__agent_";
const ORIGIN_PREFIX: &str = "_agent_";
const TEAM_PREFIX: &str = "_team_";

/// Declares a closed string enum with its stable storage spelling.
macro_rules! history_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub(crate) enum $name {
            $(#[serde(rename = $text)] $variant),+
        }

        impl $name {
            /// Stable storage and wire spelling.
            pub(crate) fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            /// Parse the stable spelling; `None` for any other text.
            pub(crate) fn parse(text: &str) -> Option<Self> {
                match text {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

history_enum!(
    /// Whether the agent is a room replica or an origin Agent Matrix.
    MatrixType { Replica => "replica", Origin => "origin" }
);
history_enum!(
    /// How many teams the agent verifiably belonged to when observed.
    TeamAttribution { Known => "known", Unknown => "unknown", Ambiguous => "ambiguous" }
);
history_enum!(
    /// Where coding agent, model and effort came from.
    MetadataSource { LaunchCommand => "launch_command", Unavailable => "unavailable" }
);
history_enum!(
    /// A positive context change or a session boundary.
    RecordKind { Change => "change", Boundary => "boundary" }
);
history_enum!(
    /// Why a boundary record ends the logical session.
    BoundaryReason {
        ObservedZero => "observed_zero",
        Lifecycle => "lifecycle",
        ObservationGap => "observation_gap",
    }
);
history_enum!(
    /// How the logical session's initial record was established.
    InitialKind {
        ConfirmedZeroTransition => "confirmed_zero_transition",
        FirstObserved => "first_observed",
        Unknown => "unknown",
    }
);

/// One persisted context-history observation, schema version 2.
///
/// `logical_started_at_utc_ms`, `initial_kind` and `initial_record_id` describe
/// the logical session segment and are repeated, unchanged, on every record
/// of that segment so retention never reclassifies an old session as new.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HistoryRecord {
    pub(crate) schema_version: u32,
    pub(crate) record_id: Uuid,
    pub(crate) app_run_id: Uuid,
    pub(crate) runtime_session_id: Uuid,
    pub(crate) logical_session_id: Uuid,
    pub(crate) sequence: u64,
    pub(crate) ordinal: u64,
    pub(crate) observed_at_utc_ms: i64,
    pub(crate) logical_started_at_utc_ms: Option<i64>,
    pub(crate) partial_start: bool,
    pub(crate) partial_end: bool,
    pub(crate) initial_record_id: Option<Uuid>,
    pub(crate) initial_kind: InitialKind,
    pub(crate) continuity_known: bool,
    pub(crate) project_path: String,
    pub(crate) project_name: String,
    pub(crate) agent_key: String,
    pub(crate) instance_fqn: String,
    pub(crate) room: Option<String>,
    pub(crate) matrix_type: MatrixType,
    pub(crate) team_attribution: TeamAttribution,
    pub(crate) team_ids: Vec<String>,
    pub(crate) coding_agent_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) metadata_source: MetadataSource,
    pub(crate) context_window: Option<u64>,
    pub(crate) kind: RecordKind,
    pub(crate) percent: Option<u8>,
    pub(crate) boundary_reason: Option<BoundaryReason>,
}

fn check_text(field: &str, value: &str, max_bytes: usize) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field}_empty"));
    }
    if value.len() > max_bytes {
        return Err(format!("{field}_too_long"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{field}_control_character"));
    }
    Ok(())
}

fn check_optional_text(field: &str, value: Option<&str>, max_bytes: usize) -> Result<(), String> {
    value.map_or(Ok(()), |value| check_text(field, value, max_bytes))
}

fn check_non_negative(field: &str, value: i64) -> Result<(), String> {
    if value < 0 {
        return Err(format!("{field}_negative"));
    }
    Ok(())
}

fn check_storable(field: &str, value: u64) -> Result<(), String> {
    if i64::try_from(value).is_err() {
        return Err(format!("{field}_out_of_range"));
    }
    Ok(())
}

/// The bare agent name: exact layout text, without matrix prefixes or padding.
fn check_agent_key(agent_key: &str) -> Result<(), String> {
    check_text("agent_key", agent_key, MAX_IDENTITY_BYTES)?;
    if agent_key.starts_with(REPLICA_PREFIX) || agent_key.starts_with(ORIGIN_PREFIX) {
        return Err("agent_key_matrix_prefix".to_string());
    }
    if agent_key.trim() != agent_key || agent_key.contains(['/', '\\']) {
        return Err("agent_key_not_bare".to_string());
    }
    Ok(())
}

fn check_teams(attribution: TeamAttribution, team_ids: &[String]) -> Result<(), String> {
    if team_ids.len() > MAX_TEAM_IDS {
        return Err("team_ids_too_many".to_string());
    }
    for team_id in team_ids {
        check_text("team_id", team_id, MAX_IDENTITY_BYTES)?;
        if team_id.starts_with(TEAM_PREFIX) {
            return Err("team_id_team_prefix".to_string());
        }
    }
    if team_ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("team_ids_not_sorted_unique".to_string());
    }
    let consistent = match attribution {
        TeamAttribution::Unknown => team_ids.is_empty(),
        TeamAttribution::Known => team_ids.len() == 1,
        TeamAttribution::Ambiguous => team_ids.len() >= 2,
    };
    if !consistent {
        return Err("team_attribution_mismatch".to_string());
    }
    Ok(())
}

fn check_value(record: &HistoryRecord) -> Result<(), String> {
    let percent = record.percent;
    let valid = match (record.kind, record.boundary_reason) {
        (RecordKind::Change, None) => percent.is_some_and(|value| (1..=100).contains(&value)),
        (RecordKind::Boundary, Some(BoundaryReason::ObservedZero)) => percent == Some(0),
        (
            RecordKind::Boundary,
            Some(BoundaryReason::Lifecycle | BoundaryReason::ObservationGap),
        ) => percent.is_none(),
        _ => false,
    };
    if !valid {
        return Err("kind_percent_reason_mismatch".to_string());
    }
    if record.context_window.is_some() {
        return Err("context_window_not_null".to_string());
    }
    Ok(())
}

/// A start that was lost is never invented: it must be marked partial/unknown.
fn check_start(record: &HistoryRecord) -> Result<(), String> {
    match record.logical_started_at_utc_ms {
        Some(started) => check_non_negative("logical_started_at_utc_ms", started),
        None if record.partial_start && record.initial_kind == InitialKind::Unknown => Ok(()),
        None => Err("missing_start_not_partial".to_string()),
    }
}

impl HistoryRecord {
    /// Reject an invalid record before anything is written. Identity is never
    /// truncated; an oversized value is an error.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err("schema_version_unsupported".to_string());
        }
        check_storable("sequence", self.sequence)?;
        check_storable("ordinal", self.ordinal)?;
        check_non_negative("observed_at_utc_ms", self.observed_at_utc_ms)?;
        check_start(self)?;
        check_text("project_path", &self.project_path, MAX_IDENTITY_BYTES)?;
        check_text("project_name", &self.project_name, MAX_IDENTITY_BYTES)?;
        check_agent_key(&self.agent_key)?;
        check_text("instance_fqn", &self.instance_fqn, MAX_IDENTITY_BYTES)?;
        check_optional_text("room", self.room.as_deref(), MAX_IDENTITY_BYTES)?;
        check_teams(self.team_attribution, &self.team_ids)?;
        check_optional_text(
            "coding_agent_id",
            self.coding_agent_id.as_deref(),
            MAX_METADATA_BYTES,
        )?;
        check_optional_text("model", self.model.as_deref(), MAX_METADATA_BYTES)?;
        check_optional_text("effort", self.effort.as_deref(), MAX_METADATA_BYTES)?;
        check_value(self)?;
        let json = serde_json::to_vec(self)
            .map_err(|error| format!("record_not_serializable: {error}"))?;
        if json.len() > MAX_RECORD_JSON_BYTES {
            return Err("record_too_large".to_string());
        }
        Ok(())
    }
}
