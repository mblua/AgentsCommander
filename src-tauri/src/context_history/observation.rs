//! Pure context-history observation (#3532 P02).
//!
//! Three pure pieces shared by capture and the sidebar: the history scope of a
//! session's working directory, the launch metadata parsed from its exact
//! launch command, and the per-instance transition machine that turns scrape
//! outcomes into change and boundary records. Nothing here reads files, the
//! clock, settings or the environment, and nothing takes a lock.

use std::path::Path;

use uuid::Uuid;

use super::{
    BoundaryReason, HistoryRecord, InitialKind, MatrixType, MetadataSource, RecordKind,
    TeamAttribution, MAX_METADATA_BYTES, SCHEMA_VERSION,
};
use crate::config::ac_root::is_ac_root_name;
use crate::config::entity_prefix::has_entity_prefix;

const REPLICA_PREFIX: &str = "__agent_";
const ORIGIN_PREFIX: &str = "_agent_";
const TEAM_PREFIX: &str = "_team_";
const VERBATIM_PREFIX: &str = "//?/";

// ---- Scope -------------------------------------------------------------------------------

/// Where a session's working directory sits in a validated agent layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryLayout {
    pub(crate) project_path: String,
    pub(crate) project_name: String,
    pub(crate) ac_root: String,
    pub(crate) agent_key: String,
    pub(crate) room: Option<String>,
    pub(crate) matrix_type: MatrixType,
    /// The origin Agent Matrix (`<ac_root>/_agent_<agent_key>`) teams list.
    pub(crate) origin_matrix: String,
}

/// One discovered team: its exact name and every member and coordinator path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TeamMembers {
    pub(crate) team_id: String,
    pub(crate) member_paths: Vec<String>,
}

/// Everything scope resolution needs, prepared by the runtime beforehand.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScopeInput<'a> {
    pub(crate) working_directory: &'a str,
    pub(crate) is_root_agent: bool,
    /// The layout's Project AC Root is the authoritative one of its project.
    pub(crate) authoritative_ac_root: bool,
    /// Teams discovered in the layout's project only.
    pub(crate) teams: &'a [TeamMembers],
}

/// The canonical schema-2 identity of one observed agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryScope {
    pub(crate) layout: HistoryLayout,
    pub(crate) instance_fqn: String,
    pub(crate) team_attribution: TeamAttribution,
    pub(crate) team_ids: Vec<String>,
}

/// A session is either scoped or excluded from history, with a stable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScopeResolution {
    Scoped(HistoryScope),
    Excluded(&'static str),
}

fn name_of(path: &Path) -> Option<&str> {
    path.file_name().and_then(std::ffi::OsStr::to_str)
}

fn path_text(path: &Path) -> Option<String> {
    path.to_str()
        .map(str::to_string)
        .filter(|text| !text.is_empty())
}

/// Separator-normalized text for ordinal path comparison; never lowercased.
fn comparable(path: &str) -> String {
    let forward = path.replace('\\', "/");
    let bare = forward.strip_prefix(VERBATIM_PREFIX).unwrap_or(&forward);
    bare.trim_end_matches('/').to_string()
}

fn bare_agent_key(key: &str) -> Option<String> {
    let usable = !key.is_empty()
        && key.trim() == key
        && !key.contains(['/', '\\'])
        && !key.starts_with('_')
        && !key.chars().any(char::is_control);
    usable.then(|| key.to_string())
}

/// Parse `<project>/.ac/<room>/__agent_<key>` (replica) or
/// `<project>/.ac/_agent_<key>` (origin). Any other shape is not an agent.
pub(crate) fn parse_history_layout(working_directory: &str) -> Result<HistoryLayout, &'static str> {
    let agent_dir = Path::new(working_directory);
    let agent_name = name_of(agent_dir).ok_or("not_agent_layout")?;
    let parent = agent_dir.parent().ok_or("not_agent_layout")?;
    let (key, room, ac_root) = if let Some(key) = agent_name.strip_prefix(REPLICA_PREFIX) {
        let room = name_of(parent).filter(|name| has_entity_prefix(name));
        let room = room.ok_or("not_agent_layout")?;
        (
            key,
            Some(room.to_string()),
            parent.parent().ok_or("not_agent_layout")?,
        )
    } else if let Some(key) = agent_name.strip_prefix(ORIGIN_PREFIX) {
        (key, None, parent)
    } else {
        return Err("not_agent_layout");
    };
    if !name_of(ac_root).is_some_and(is_ac_root_name) {
        return Err("not_agent_layout");
    }
    let project = ac_root.parent().ok_or("not_agent_layout")?;
    let agent_key = bare_agent_key(key).ok_or("agent_key_not_bare")?;
    let origin_matrix = ac_root.join(format!("{ORIGIN_PREFIX}{agent_key}"));
    Ok(HistoryLayout {
        project_path: path_text(project).ok_or("not_agent_layout")?,
        project_name: name_of(project).ok_or("not_agent_layout")?.to_string(),
        ac_root: path_text(ac_root).ok_or("not_agent_layout")?,
        agent_key,
        matrix_type: if room.is_some() {
            MatrixType::Replica
        } else {
            MatrixType::Origin
        },
        room,
        origin_matrix: path_text(&origin_matrix).ok_or("not_agent_layout")?,
    })
}

/// Every team listing the exact origin matrix as member or coordinator.
fn attributed_teams(origin_matrix: &str, teams: &[TeamMembers]) -> Vec<String> {
    let wanted = comparable(origin_matrix);
    let mut ids: Vec<String> = teams
        .iter()
        .filter(|team| {
            team.member_paths
                .iter()
                .any(|path| comparable(path) == wanted)
        })
        .map(|team| {
            let id = team
                .team_id
                .strip_prefix(TEAM_PREFIX)
                .unwrap_or(&team.team_id);
            id.to_string()
        })
        .filter(|id| !id.is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Resolve the history scope of one session. Root, plain sessions, unsafe
/// layouts and non-authoritative AC roots are excluded; nothing is inferred
/// from labels or display FQNs.
pub(crate) fn resolve_history_scope(input: &ScopeInput<'_>) -> ScopeResolution {
    if input.is_root_agent {
        return ScopeResolution::Excluded("root_agent");
    }
    let layout = match parse_history_layout(input.working_directory) {
        Ok(layout) => layout,
        Err(reason) => return ScopeResolution::Excluded(reason),
    };
    if !input.authoritative_ac_root {
        return ScopeResolution::Excluded("ac_root_not_authoritative");
    }
    let team_ids = attributed_teams(&layout.origin_matrix, input.teams);
    let team_attribution = match team_ids.len() {
        0 => TeamAttribution::Unknown,
        1 => TeamAttribution::Known,
        _ => TeamAttribution::Ambiguous,
    };
    let instance_fqn = match &layout.room {
        Some(room) => format!("{}:{}/{}", layout.project_name, room, layout.agent_key),
        None => format!("{}/{}", layout.project_name, layout.agent_key),
    };
    ScopeResolution::Scoped(HistoryScope {
        layout,
        instance_fqn,
        team_attribution,
        team_ids,
    })
}

// ---- Launch metadata ---------------------------------------------------------------------

/// Coding agent, model and effort parsed from the exact launch command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchMetadata {
    pub(crate) coding_agent_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) source: MetadataSource,
}

impl LaunchMetadata {
    pub(crate) const UNAVAILABLE: Self = Self {
        coding_agent_id: None,
        model: None,
        effort: None,
        source: MetadataSource::Unavailable,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Provider {
    Claude,
    Codex,
    Pi,
}

impl Provider {
    fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
        }
    }

    /// Only a direct launch of the provider executable; wrappers are unknown.
    fn of(shell: &str) -> Option<Self> {
        let stem = Path::new(shell).file_stem()?.to_str()?;
        [Self::Claude, Self::Codex, Self::Pi]
            .into_iter()
            .find(|provider| stem.eq_ignore_ascii_case(provider.id()))
    }
}

fn usable(value: &str) -> Option<String> {
    let ok = !value.is_empty()
        && value.len() <= MAX_METADATA_BYTES
        && !value.chars().any(char::is_control);
    ok.then(|| value.to_string())
}

/// The value of `name VALUE` or `name=VALUE` at `args[index]`, if it is that flag.
fn flag_value<'a>(args: &'a [String], index: usize, name: &str, joined: bool) -> Option<&'a str> {
    let arg = args[index].as_str();
    if arg == name {
        return Some(args.get(index + 1).map_or("", String::as_str));
    }
    let rest = arg.strip_prefix(name)?.strip_prefix('=')?;
    joined.then_some(rest)
}

/// Strip exactly one layer of matching single or double quotes.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// Codex `model_reasoning_effort=VALUE` from a `-c`/`--config` value.
fn codex_effort(config: &str) -> Option<String> {
    let value = config.strip_prefix("model_reasoning_effort=")?;
    usable(unquote(value))
}

fn last_valid(slot: &mut Option<String>, candidate: Option<String>) {
    if candidate.is_some() {
        *slot = candidate;
    }
}

fn parse_flags(provider: Provider, args: &[String]) -> (Option<String>, Option<String>) {
    let (mut model, mut effort) = (None, None);
    for index in 0..args.len() {
        let model_flags: &[(&str, bool)] = match provider {
            Provider::Codex => &[("--model", true), ("-m", false)],
            Provider::Claude | Provider::Pi => &[("--model", true)],
        };
        for (name, joined) in model_flags {
            last_valid(
                &mut model,
                flag_value(args, index, name, *joined).and_then(usable),
            );
        }
        match provider {
            Provider::Claude => {
                let value = flag_value(args, index, "--effort", true);
                last_valid(&mut effort, value.and_then(usable));
            }
            Provider::Pi => {
                let value = flag_value(args, index, "--thinking", true);
                last_valid(&mut effort, value.and_then(usable));
            }
            Provider::Codex => {
                for (name, joined) in [("-c", false), ("--config", true)] {
                    let value = flag_value(args, index, name, joined);
                    last_valid(&mut effort, value.and_then(codex_effort));
                }
            }
        }
    }
    (model, effort)
}

/// Parse launch metadata from the session's exact shell and effective args.
/// No settings, environment, defaults, aliases or live model are consulted.
pub(crate) fn parse_launch_metadata(shell: &str, args: Option<&[String]>) -> LaunchMetadata {
    let (Some(provider), Some(args)) = (Provider::of(shell), args) else {
        return LaunchMetadata::UNAVAILABLE;
    };
    let (model, effort) = parse_flags(provider, args);
    LaunchMetadata {
        coding_agent_id: Some(provider.id().to_string()),
        model,
        effort,
        source: MetadataSource::LaunchCommand,
    }
}

// ---- Transitions -------------------------------------------------------------------------

/// One scrape outcome, as offered to the history ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObservedValue {
    Reading(u8),
    Unavailable,
    SessionOver,
}

/// The immutable segment context of one observed runtime instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstanceContext {
    pub(crate) app_run_id: Uuid,
    pub(crate) runtime_session_id: Uuid,
    pub(crate) scope: HistoryScope,
    pub(crate) launch: LaunchMetadata,
}

/// One stamped observation: UTC time and ingress sequence are fixed at ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub(crate) observed_at_utc_ms: i64,
    pub(crate) sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveSession {
    logical_session_id: Uuid,
    ordinal: u64,
    started_at_utc_ms: Option<i64>,
    initial_record_id: Option<Uuid>,
    initial_kind: InitialKind,
    continuity_known: bool,
}

/// Transition state of one `(appRunId, runtimeSessionId, enableEpoch)` segment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InstanceState {
    /// Last observed number, kept across unavailability and gaps for dedup.
    last_percent: Option<u8>,
    /// The previous observation was a zero with proven consecutiveness.
    zero_consecutive: bool,
    /// A gap lost observations while no logical session was open.
    start_lost: bool,
    active: Option<ActiveSession>,
    sessions_opened: u64,
    last_sequence: Option<u64>,
    retired: bool,
}

struct Emit<'a> {
    context: &'a InstanceContext,
    stamp: Stamp,
    record_id: Uuid,
}

fn record(
    emit: &Emit<'_>,
    session: &ActiveSession,
    kind: RecordKind,
    percent: Option<u8>,
    boundary_reason: Option<BoundaryReason>,
) -> HistoryRecord {
    let scope = &emit.context.scope;
    let launch = &emit.context.launch;
    HistoryRecord {
        schema_version: SCHEMA_VERSION,
        record_id: emit.record_id,
        app_run_id: emit.context.app_run_id,
        runtime_session_id: emit.context.runtime_session_id,
        logical_session_id: session.logical_session_id,
        sequence: emit.stamp.sequence,
        ordinal: session.ordinal,
        observed_at_utc_ms: emit.stamp.observed_at_utc_ms,
        logical_started_at_utc_ms: session.started_at_utc_ms,
        partial_start: session.started_at_utc_ms.is_none(),
        partial_end: false,
        initial_record_id: session.initial_record_id,
        initial_kind: session.initial_kind,
        continuity_known: session.continuity_known,
        project_path: scope.layout.project_path.clone(),
        project_name: scope.layout.project_name.clone(),
        agent_key: scope.layout.agent_key.clone(),
        instance_fqn: scope.instance_fqn.clone(),
        room: scope.layout.room.clone(),
        matrix_type: scope.layout.matrix_type,
        team_attribution: scope.team_attribution,
        team_ids: scope.team_ids.clone(),
        coding_agent_id: launch.coding_agent_id.clone(),
        model: launch.model.clone(),
        effort: launch.effort.clone(),
        metadata_source: launch.source,
        context_window: None,
        kind,
        percent,
        boundary_reason,
    }
}

impl InstanceState {
    /// The instance ended; later observations write nothing.
    #[cfg(test)]
    pub(crate) fn is_retired(&self) -> bool {
        self.retired
    }

    /// Apply one observation and return the records it produces (zero or one).
    /// A sequence not above the last accepted one is rejected; the timestamp
    /// is never used to reject or reorder.
    pub(crate) fn observe(
        &mut self,
        context: &InstanceContext,
        value: ObservedValue,
        stamp: Stamp,
        new_id: &mut dyn FnMut() -> Uuid,
    ) -> Option<HistoryRecord> {
        if self.retired
            || self
                .last_sequence
                .is_some_and(|last| stamp.sequence <= last)
        {
            return None;
        }
        self.last_sequence = Some(stamp.sequence);
        match value {
            ObservedValue::Reading(0) => self.observe_zero(context, stamp, new_id),
            ObservedValue::Reading(percent) => {
                self.observe_positive(context, percent, stamp, new_id)
            }
            ObservedValue::Unavailable => {
                self.zero_consecutive = false;
                None
            }
            ObservedValue::SessionOver => {
                self.retired = true;
                let session = self.active.take()?;
                let emit = Emit {
                    context,
                    stamp,
                    record_id: new_id(),
                };
                let reason = Some(BoundaryReason::Lifecycle);
                Some(record(&emit, &session, RecordKind::Boundary, None, reason))
            }
        }
    }

    fn observe_zero(
        &mut self,
        context: &InstanceContext,
        stamp: Stamp,
        new_id: &mut dyn FnMut() -> Uuid,
    ) -> Option<HistoryRecord> {
        self.zero_consecutive = true;
        let changed = self.last_percent != Some(0);
        self.last_percent = Some(0);
        if !changed {
            return None;
        }
        let session = self.active.take()?;
        let emit = Emit {
            context,
            stamp,
            record_id: new_id(),
        };
        let reason = Some(BoundaryReason::ObservedZero);
        Some(record(
            &emit,
            &session,
            RecordKind::Boundary,
            Some(0),
            reason,
        ))
    }

    fn observe_positive(
        &mut self,
        context: &InstanceContext,
        percent: u8,
        stamp: Stamp,
        new_id: &mut dyn FnMut() -> Uuid,
    ) -> Option<HistoryRecord> {
        let percent = percent.min(100);
        if self.active.is_some() && self.last_percent == Some(percent) {
            return None;
        }
        self.last_percent = Some(percent);
        let record_id = new_id();
        if self.active.is_none() {
            self.open_session(stamp, record_id, new_id);
        }
        let session = self.active.as_ref()?;
        let emit = Emit {
            context,
            stamp,
            record_id,
        };
        Some(record(
            &emit,
            session,
            RecordKind::Change,
            Some(percent),
            None,
        ))
    }

    fn open_session(&mut self, stamp: Stamp, record_id: Uuid, new_id: &mut dyn FnMut() -> Uuid) {
        let (initial_kind, started, initial) = if self.start_lost {
            (InitialKind::Unknown, None, None)
        } else if self.zero_consecutive {
            let kind = InitialKind::ConfirmedZeroTransition;
            (kind, Some(stamp.observed_at_utc_ms), Some(record_id))
        } else {
            let kind = InitialKind::FirstObserved;
            (kind, Some(stamp.observed_at_utc_ms), Some(record_id))
        };
        self.sessions_opened += 1;
        self.active = Some(ActiveSession {
            logical_session_id: new_id(),
            ordinal: self.sessions_opened,
            started_at_utc_ms: started,
            initial_record_id: initial,
            initial_kind,
            continuity_known: !self.start_lost,
        });
        self.zero_consecutive = false;
        self.start_lost = false;
    }

    /// Observations were lost. An open session gets one `observation_gap`
    /// boundary and keeps its identity and start; with none open, the next
    /// session's start is unknown. The last number is kept for dedup.
    pub(crate) fn observe_gap(
        &mut self,
        context: &InstanceContext,
        stamp: Stamp,
        new_id: &mut dyn FnMut() -> Uuid,
    ) -> Option<HistoryRecord> {
        if self.retired {
            return None;
        }
        self.zero_consecutive = false;
        let Some(session) = self.active.as_mut() else {
            self.start_lost = true;
            return None;
        };
        let already_partial = !session.continuity_known;
        session.continuity_known = false;
        if already_partial {
            return None;
        }
        let session = session.clone();
        let emit = Emit {
            context,
            stamp,
            record_id: new_id(),
        };
        let reason = Some(BoundaryReason::ObservationGap);
        Some(record(&emit, &session, RecordKind::Boundary, None, reason))
    }
}

#[cfg(test)]
mod tests;
