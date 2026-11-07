//! Pure context-history report (#3532 P04), shared by CLI, IPC and sidebar.
//!
//! Input is the retained `HistoryRecord` snapshot plus values the caller has
//! already resolved (project, scopes, retention, `now`). Sessions are selected
//! by `logical_started_at_utc_ms` inside `[from, to)`; every retained positive
//! `change` record of a selected session counts, even one observed after `to`.
//! Means are always `sum / count` over records, never a mean of means.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::context_history::{
    BoundaryReason, HistoryRecord, InitialKind, MatrixType, MetadataSource, RecordKind,
    TeamAttribution, MAX_IDENTITY_BYTES, MAX_METADATA_BYTES,
};

/// Response schema written by this build.
pub(crate) const REPORT_SCHEMA_VERSION: u32 = 2;
pub(crate) const DAY_MS: i64 = 24 * 60 * 60 * 1000;
pub(crate) const MAX_PERIOD_MS: i64 = 31 * DAY_MS;
/// Maximum rows in one page; also the maximum sidebar session ids.
pub(crate) const PAGE_LIMIT: usize = 100;
pub(crate) const MAX_SIDEBAR_SESSIONS: usize = 100;
pub(crate) const SELECTION_SESSION_STARTED: &str = "session_started";
/// Label of a session whose retained data is known to be incomplete.
pub(crate) const INCOMPLETE_SESSION_LABEL: &str = "Datos conservados de sesión incompleta";
/// Sidebar windows in days: day, week and month.
pub(crate) const SIDEBAR_WINDOW_DAYS: [u32; 3] = [1, 7, 30];

const CURSOR_VERSION: u32 = 1;
const CURSOR_PREFIX: &str = "ctxh1.";
const MAX_CURSOR_BYTES: usize = 32 * 1024;
const VERBATIM_PREFIX: &str = "//?/";

/// A rejected query; never reported as an empty result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReportError {
    /// The query itself is invalid; the text is a stable reason.
    InvalidQuery(&'static str),
    /// The cursor is malformed or belongs to another query or page kind.
    InvalidCursor(&'static str),
    /// The requested agent or session is not part of this selection.
    NotInSelection(&'static str),
}

impl fmt::Display for ReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidQuery(reason) => write!(formatter, "invalid query: {reason}"),
            Self::InvalidCursor(reason) => write!(formatter, "invalid cursor: {reason}"),
            Self::NotInSelection(reason) => write!(formatter, "not in selection: {reason}"),
        }
    }
}

/// A launch metadata filter. `Unknown` matches only a null value and is not `Any`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub(crate) enum LaunchFilter {
    Any,
    Unknown,
    Exact(String),
}

impl LaunchFilter {
    fn matches(&self, value: Option<&str>) -> bool {
        match self {
            Self::Any => true,
            Self::Unknown => value.is_none(),
            Self::Exact(expected) => value == Some(expected.as_str()),
        }
    }

    fn validate(&self) -> Result<(), ReportError> {
        match self {
            Self::Exact(value) if value.is_empty() || value.len() > MAX_METADATA_BYTES => {
                Err(ReportError::InvalidQuery("exact_filter_empty_or_too_long"))
            }
            _ => Ok(()),
        }
    }
}

/// The report query; `project_path` is the authoritative project resolved by the edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReportQuery {
    pub(crate) project_path: String,
    pub(crate) from_utc_ms: i64,
    pub(crate) to_utc_ms: i64,
    pub(crate) agent_key: Option<String>,
    pub(crate) team_id: Option<String>,
    pub(crate) coding_agent_id: LaunchFilter,
    pub(crate) model: LaunchFilter,
    pub(crate) effort: LaunchFilter,
    pub(crate) cursor: Option<String>,
}

fn check_identity(value: &str, empty: &'static str, long: &'static str) -> Result<(), ReportError> {
    if value.is_empty() {
        return Err(ReportError::InvalidQuery(empty));
    }
    if value.len() > MAX_IDENTITY_BYTES {
        return Err(ReportError::InvalidQuery(long));
    }
    Ok(())
}

impl ReportQuery {
    /// Reject an invalid query before any record is examined.
    pub(crate) fn validate(&self) -> Result<(), ReportError> {
        check_identity(
            &self.project_path,
            "project_path_empty",
            "project_path_too_long",
        )?;
        if self.from_utc_ms >= self.to_utc_ms {
            return Err(ReportError::InvalidQuery("from_not_before_to"));
        }
        if self
            .to_utc_ms
            .checked_sub(self.from_utc_ms)
            .is_none_or(|ms| ms > MAX_PERIOD_MS)
        {
            return Err(ReportError::InvalidQuery("period_longer_than_31_days"));
        }
        if let Some(agent_key) = &self.agent_key {
            check_identity(agent_key, "agent_key_empty", "agent_key_too_long")?;
        }
        if let Some(team_id) = &self.team_id {
            check_identity(team_id, "team_id_empty", "team_id_too_long")?;
        }
        self.coding_agent_id.validate()?;
        self.model.validate()?;
        self.effort.validate()
    }

    /// The query a cursor is bound to: normalized project and no cursor.
    fn normalized(&self) -> Self {
        Self {
            project_path: normalized_path(&self.project_path),
            cursor: None,
            ..self.clone()
        }
    }

    fn matches_session(&self, first: &HistoryRecord) -> bool {
        let agent = self
            .agent_key
            .as_ref()
            .is_none_or(|key| *key == first.agent_key);
        let team = self.team_id.as_ref().is_none_or(|team| {
            first.team_attribution == TeamAttribution::Known && first.team_ids == [team.as_str()]
        });
        agent
            && team
            && self
                .coding_agent_id
                .matches(first.coding_agent_id.as_deref())
            && self.model.matches(first.model.as_deref())
            && self.effort.matches(first.effort.as_deref())
    }
}

/// Values the edge resolved for one response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReportContext {
    pub(crate) retention_days: u32,
    pub(crate) generated_at_utc_ms: i64,
}

/// Separator-normalized path for ordinal comparison; never lowercased.
fn normalized_path(path: &str) -> String {
    let forward = path.replace('\\', "/");
    let bare = forward.strip_prefix(VERBATIM_PREFIX).unwrap_or(&forward);
    bare.trim_end_matches('/').to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestedPeriod {
    pub(crate) from_utc_ms: i64,
    pub(crate) to_utc_ms: i64,
    pub(crate) selection: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Coverage {
    pub(crate) retention_days: u32,
    pub(crate) retained_earliest_utc_ms: Option<i64>,
    pub(crate) retained_latest_utc_ms: Option<i64>,
    pub(crate) incomplete: bool,
    pub(crate) unknown_start_sessions_count: u64,
}

/// Fields shared by every report response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResponseHeader {
    pub(crate) schema_version: u32,
    pub(crate) requested_period: RequestedPeriod,
    pub(crate) generated_at_utc_ms: i64,
    pub(crate) coverage: Coverage,
}

/// Launch metadata and team attribution, immutable per logical session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionMetadata {
    pub(crate) coding_agent_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) metadata_source: MetadataSource,
    pub(crate) team_attribution: TeamAttribution,
    pub(crate) team_ids: Vec<String>,
}

/// One heterogeneous metadata combination seen among an agent's sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MetadataBucket {
    #[serde(flatten)]
    pub(crate) metadata: SessionMetadata,
    pub(crate) sessions_count: u64,
}

/// Totals over whole selected sessions: `mean = positiveSum / recordsCount`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Aggregate {
    pub(crate) records_count: u64,
    pub(crate) positive_sum: u64,
    pub(crate) mean: Option<f64>,
    pub(crate) sessions_count: u64,
    pub(crate) incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentSummary {
    pub(crate) agent_key: String,
    pub(crate) agent_name: String,
    pub(crate) metadata_buckets: Vec<MetadataBucket>,
    #[serde(flatten)]
    pub(crate) totals: Aggregate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReportResponse {
    #[serde(flatten)]
    pub(crate) header: ResponseHeader,
    pub(crate) agents_page: Vec<AgentSummary>,
    pub(crate) next_agent_cursor: Option<String>,
    pub(crate) total_agents: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionSummary {
    pub(crate) logical_session_id: Uuid,
    pub(crate) app_run_id: Uuid,
    pub(crate) runtime_session_id: Uuid,
    pub(crate) ordinal: u64,
    pub(crate) instance_fqn: String,
    pub(crate) room: Option<String>,
    pub(crate) matrix_type: MatrixType,
    pub(crate) logical_started_at_utc_ms: i64,
    pub(crate) first_observed_at_utc_ms: i64,
    pub(crate) last_observed_at_utc_ms: i64,
    pub(crate) initial_kind: InitialKind,
    #[serde(flatten)]
    pub(crate) stats: SessionStats,
    #[serde(flatten)]
    pub(crate) metadata: SessionMetadata,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentDetailResponse {
    #[serde(flatten)]
    pub(crate) header: ResponseHeader,
    pub(crate) agent_key: String,
    #[serde(flatten)]
    pub(crate) totals: Aggregate,
    pub(crate) sessions_page: Vec<SessionSummary>,
    pub(crate) next_session_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChangesPageResponse {
    #[serde(flatten)]
    pub(crate) header: ResponseHeader,
    pub(crate) agent_key: String,
    pub(crate) logical_session_id: Uuid,
    /// Whole retained records, with timestamp, value, kind, reason and provenance.
    pub(crate) records_page: Vec<HistoryRecord>,
    pub(crate) next_record_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum PageKind {
    Agents,
    Sessions,
    Records,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorPayload {
    version: u32,
    page: PageKind,
    query: ReportQuery,
    agent_key: Option<String>,
    logical_session_id: Option<Uuid>,
    /// The previous page's last key: agent key, (start, id) or (time, sequence, id).
    last: serde_json::Value,
}

/// Where a page sits: its kind and, for detail pages, the agent and session.
#[derive(Debug, Clone, Copy)]
struct PageTarget<'q> {
    page: PageKind,
    agent_key: Option<&'q str>,
    logical_session_id: Option<Uuid>,
}

impl<'q> PageTarget<'q> {
    fn new(page: PageKind, agent_key: Option<&'q str>, logical_session_id: Option<Uuid>) -> Self {
        Self {
            page,
            agent_key,
            logical_session_id,
        }
    }
}

fn next_cursor<K: Serialize>(query: &ReportQuery, target: PageTarget, last: &K) -> String {
    encode_cursor(&CursorPayload {
        version: CURSOR_VERSION,
        page: target.page,
        query: query.normalized(),
        agent_key: target.agent_key.map(str::to_string),
        logical_session_id: target.logical_session_id,
        last: serde_json::to_value(last).unwrap_or_default(),
    })
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn encode_cursor(payload: &CursorPayload) -> String {
    let json = serde_json::to_vec(payload).unwrap_or_default();
    let mut text = String::with_capacity(CURSOR_PREFIX.len() + json.len() * 2);
    text.push_str(CURSOR_PREFIX);
    for byte in json {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    text
}

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

fn decode_cursor(cursor: &str) -> Result<CursorPayload, ReportError> {
    if cursor.len() > MAX_CURSOR_BYTES {
        return Err(ReportError::InvalidCursor("cursor_too_long"));
    }
    let body = cursor
        .strip_prefix(CURSOR_PREFIX)
        .ok_or(ReportError::InvalidCursor("cursor_version_unsupported"))?;
    if body.len() % 2 != 0 {
        return Err(ReportError::InvalidCursor("cursor_malformed"));
    }
    let bytes = body
        .as_bytes()
        .chunks(2)
        .map(|pair| Some((hex_value(pair[0])? << 4) | hex_value(pair[1])?))
        .collect::<Option<Vec<u8>>>()
        .ok_or(ReportError::InvalidCursor("cursor_malformed"))?;
    let payload: CursorPayload =
        serde_json::from_slice(&bytes).or(Err(ReportError::InvalidCursor("cursor_malformed")))?;
    if payload.version != CURSOR_VERSION {
        return Err(ReportError::InvalidCursor("cursor_version_unsupported"));
    }
    Ok(payload)
}

/// Decode the query's cursor and check it belongs to this exact page.
fn cursor_after<K: serde::de::DeserializeOwned>(
    query: &ReportQuery,
    target: PageTarget,
) -> Result<Option<K>, ReportError> {
    let Some(cursor) = query.cursor.as_deref() else {
        return Ok(None);
    };
    let payload = decode_cursor(cursor)?;
    if payload.page != target.page {
        return Err(ReportError::InvalidCursor("cursor_page_kind_mismatch"));
    }
    if payload.query != query.normalized() {
        return Err(ReportError::InvalidCursor("cursor_query_mismatch"));
    }
    if payload.agent_key.as_deref() != target.agent_key
        || payload.logical_session_id != target.logical_session_id
    {
        return Err(ReportError::InvalidCursor("cursor_target_mismatch"));
    }
    serde_json::from_value(payload.last)
        .map(Some)
        .or(Err(ReportError::InvalidCursor("cursor_malformed")))
}

/// The positive value of a `change` record; zero, null and boundaries never count.
fn positive(record: &HistoryRecord) -> Option<u8> {
    (record.kind == RecordKind::Change)
        .then_some(record.percent)
        .flatten()
        .filter(|percent| *percent > 0)
}

fn record_order(record: &HistoryRecord) -> (i64, u64, Uuid) {
    (record.observed_at_utc_ms, record.sequence, record.record_id)
}

/// One selected logical session: its retained records in stable order.
#[derive(Debug, Clone)]
struct SessionView<'a> {
    id: Uuid,
    started_at_utc_ms: i64,
    records: Vec<&'a HistoryRecord>,
}

/// Values of one whole retained session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionStats {
    pub(crate) boundary_reason: Option<BoundaryReason>,
    pub(crate) records_count: u64,
    pub(crate) positive_sum: u64,
    pub(crate) mean: Option<f64>,
    pub(crate) max: Option<u8>,
    pub(crate) max_partial: bool,
    pub(crate) initial_value: Option<u8>,
    pub(crate) partial_start: bool,
    pub(crate) partial_end: bool,
    pub(crate) retention_clipped: bool,
    pub(crate) coverage_label: Option<&'static str>,
}

impl SessionStats {
    fn incomplete(self) -> bool {
        self.partial_start || self.partial_end || self.retention_clipped
    }
}

#[allow(clippy::cast_precision_loss)]
fn mean_of(sum: u64, count: u64) -> Option<f64> {
    (count > 0).then(|| sum as f64 / count as f64)
}

impl<'a> SessionView<'a> {
    fn first(&self) -> &'a HistoryRecord {
        self.records[0]
    }

    fn stats(&self) -> SessionStats {
        let first = self.first();
        let mut stats = SessionStats::default();
        let mut initial_retained = false;
        for record in &self.records {
            if let Some(percent) = positive(record) {
                stats.records_count += 1;
                stats.positive_sum += u64::from(percent);
                stats.max = stats.max.max(Some(percent));
            }
            if Some(record.record_id) == first.initial_record_id {
                initial_retained = true;
                stats.initial_value = positive(record);
            }
            stats.partial_start |= record.partial_start;
            stats.partial_end |= record.partial_end;
            if record.boundary_reason.is_some() {
                stats.boundary_reason = record.boundary_reason;
            }
        }
        let initial_lost = first.initial_record_id.is_some() && !initial_retained;
        stats.retention_clipped = initial_lost || first.observed_at_utc_ms > self.started_at_utc_ms;
        stats.mean = mean_of(stats.positive_sum, stats.records_count);
        stats.max_partial = stats.incomplete();
        stats.coverage_label = stats.incomplete().then_some(INCOMPLETE_SESSION_LABEL);
        stats
    }
}

fn session_order(session: &SessionView) -> (i64, Uuid) {
    (session.started_at_utc_ms, session.id)
}

#[derive(Debug, Clone, Default)]
struct Selection<'a> {
    sessions: Vec<SessionView<'a>>,
    unknown_start: u64,
    earliest: Option<i64>,
    latest: Option<i64>,
}

/// Every retained record of the project, once per record id, grouped by session.
fn project_sessions<'a>(
    records: &'a [HistoryRecord],
    project_path: &str,
) -> (
    BTreeMap<Uuid, Vec<&'a HistoryRecord>>,
    Option<i64>,
    Option<i64>,
) {
    let project = normalized_path(project_path);
    let mut seen = BTreeSet::new();
    let mut groups: BTreeMap<Uuid, Vec<&HistoryRecord>> = BTreeMap::new();
    let (mut earliest, mut latest) = (None::<i64>, None::<i64>);
    for record in records {
        if normalized_path(&record.project_path) != project || !seen.insert(record.record_id) {
            continue;
        }
        let observed = record.observed_at_utc_ms;
        earliest = Some(earliest.map_or(observed, |value| value.min(observed)));
        latest = Some(latest.map_or(observed, |value| value.max(observed)));
        groups
            .entry(record.logical_session_id)
            .or_default()
            .push(record);
    }
    (groups, earliest, latest)
}

/// Sessions started in `[from, to)` that pass every filter, ordered by start and id.
fn select<'a>(records: &'a [HistoryRecord], query: &ReportQuery) -> Selection<'a> {
    let (groups, earliest, latest) = project_sessions(records, &query.project_path);
    let mut selection = Selection {
        earliest,
        latest,
        ..Selection::default()
    };
    for (id, mut session_records) in groups {
        session_records.sort_by_key(|record| record_order(record));
        let first = session_records[0];
        if !query.matches_session(first) {
            continue;
        }
        match first.logical_started_at_utc_ms {
            None => selection.unknown_start += 1,
            Some(started) if (query.from_utc_ms..query.to_utc_ms).contains(&started) => {
                selection.sessions.push(SessionView {
                    id,
                    started_at_utc_ms: started,
                    records: session_records,
                });
            }
            Some(_) => {}
        }
    }
    selection.sessions.sort_by_key(session_order);
    selection
}

fn header(query: &ReportQuery, coverage: Coverage, context: ReportContext) -> ResponseHeader {
    ResponseHeader {
        schema_version: REPORT_SCHEMA_VERSION,
        requested_period: RequestedPeriod {
            from_utc_ms: query.from_utc_ms,
            to_utc_ms: query.to_utc_ms,
            selection: SELECTION_SESSION_STARTED,
        },
        generated_at_utc_ms: context.generated_at_utc_ms,
        coverage,
    }
}

fn coverage(selection: &Selection, sessions: &[&SessionView], context: ReportContext) -> Coverage {
    Coverage {
        retention_days: context.retention_days,
        retained_earliest_utc_ms: selection.earliest,
        retained_latest_utc_ms: selection.latest,
        incomplete: aggregate(sessions).incomplete || selection.unknown_start > 0,
        unknown_start_sessions_count: selection.unknown_start,
    }
}

fn aggregate(sessions: &[&SessionView]) -> Aggregate {
    let (mut records_count, mut positive_sum, mut incomplete) = (0, 0, false);
    for session in sessions {
        let stats = session.stats();
        records_count += stats.records_count;
        positive_sum += stats.positive_sum;
        incomplete |= stats.incomplete();
    }
    Aggregate {
        records_count,
        positive_sum,
        mean: mean_of(positive_sum, records_count),
        sessions_count: sessions.len() as u64,
        incomplete,
    }
}

/// Items after `after` (sorted by `key`), at most one page, and whether more remain.
fn page_after<T, K: PartialOrd>(
    items: &[T],
    key: impl Fn(&T) -> K,
    after: Option<K>,
) -> (&[T], bool) {
    let start = after.map_or(0, |after| items.partition_point(|item| key(item) <= after));
    let end = (start + PAGE_LIMIT).min(items.len());
    (&items[start..end], end < items.len())
}

fn sessions_by_agent<'s, 'a>(
    selection: &'s Selection<'a>,
) -> BTreeMap<&'a str, Vec<&'s SessionView<'a>>> {
    let mut agents: BTreeMap<&str, Vec<&SessionView>> = BTreeMap::new();
    for session in &selection.sessions {
        agents
            .entry(session.first().agent_key.as_str())
            .or_default()
            .push(session);
    }
    agents
}

fn metadata_of(record: &HistoryRecord) -> SessionMetadata {
    SessionMetadata {
        coding_agent_id: record.coding_agent_id.clone(),
        model: record.model.clone(),
        effort: record.effort.clone(),
        metadata_source: record.metadata_source,
        team_attribution: record.team_attribution,
        team_ids: record.team_ids.clone(),
    }
}

/// Distinct metadata combinations, ordered by their serialized form.
fn metadata_buckets(sessions: &[&SessionView]) -> Vec<MetadataBucket> {
    let mut buckets: BTreeMap<String, MetadataBucket> = BTreeMap::new();
    for session in sessions {
        let metadata = metadata_of(session.first());
        let key = serde_json::to_string(&metadata).unwrap_or_default();
        buckets
            .entry(key)
            .or_insert(MetadataBucket {
                metadata,
                sessions_count: 0,
            })
            .sessions_count += 1;
    }
    buckets.into_values().collect()
}

fn agent_summary(agent_key: &str, sessions: &[&SessionView]) -> AgentSummary {
    AgentSummary {
        agent_key: agent_key.to_string(),
        agent_name: agent_key.to_string(),
        metadata_buckets: metadata_buckets(sessions),
        totals: aggregate(sessions),
    }
}

/// One page of agents, ordered by `agentKey`.
pub(crate) fn build_report(
    records: &[HistoryRecord],
    query: &ReportQuery,
    context: ReportContext,
) -> Result<ReportResponse, ReportError> {
    query.validate()?;
    let target = PageTarget::new(PageKind::Agents, None, None);
    let after: Option<String> = cursor_after(query, target)?;
    let selection = select(records, query);
    let agents: Vec<(&str, Vec<&SessionView>)> =
        sessions_by_agent(&selection).into_iter().collect();
    let all: Vec<&SessionView> = selection.sessions.iter().collect();
    let (page, more) = page_after(&agents, |(key, _)| (*key).to_string(), after);
    let next_agent_cursor = more
        .then(|| page.last().map(|(key, _)| next_cursor(query, target, key)))
        .flatten();
    Ok(ReportResponse {
        header: header(query, coverage(&selection, &all, context), context),
        agents_page: page
            .iter()
            .map(|(key, sessions)| agent_summary(key, sessions))
            .collect(),
        next_agent_cursor,
        total_agents: agents.len() as u64,
    })
}

fn session_summary(session: &SessionView) -> SessionSummary {
    let first = session.first();
    let last = session.records[session.records.len() - 1];
    SessionSummary {
        logical_session_id: session.id,
        app_run_id: first.app_run_id,
        runtime_session_id: first.runtime_session_id,
        ordinal: first.ordinal,
        instance_fqn: first.instance_fqn.clone(),
        room: first.room.clone(),
        matrix_type: first.matrix_type,
        logical_started_at_utc_ms: session.started_at_utc_ms,
        first_observed_at_utc_ms: first.observed_at_utc_ms,
        last_observed_at_utc_ms: last.observed_at_utc_ms,
        initial_kind: first.initial_kind,
        stats: session.stats(),
        metadata: metadata_of(first),
    }
}

/// The agent's selected sessions; an agent key the query excludes is an error.
fn agent_sessions<'s, 'a>(
    selection: &'s Selection<'a>,
    query: &ReportQuery,
    agent_key: &str,
) -> Result<Vec<&'s SessionView<'a>>, ReportError> {
    check_identity(agent_key, "agent_key_empty", "agent_key_too_long")?;
    if query
        .agent_key
        .as_deref()
        .is_some_and(|key| key != agent_key)
    {
        return Err(ReportError::NotInSelection("agent_outside_query"));
    }
    Ok(selection
        .sessions
        .iter()
        .filter(|session| session.first().agent_key == agent_key)
        .collect())
}

/// One agent's totals and one page of its sessions, ordered by start and id.
pub(crate) fn build_agent_detail(
    records: &[HistoryRecord],
    query: &ReportQuery,
    agent_key: &str,
    context: ReportContext,
) -> Result<AgentDetailResponse, ReportError> {
    query.validate()?;
    let target = PageTarget::new(PageKind::Sessions, Some(agent_key), None);
    let after: Option<(i64, Uuid)> = cursor_after(query, target)?;
    let selection = select(records, query);
    let sessions = agent_sessions(&selection, query, agent_key)?;
    let (page, more) = page_after(&sessions, |session| session_order(session), after);
    let next_session_cursor = more
        .then(|| {
            page.last()
                .map(|last| next_cursor(query, target, &session_order(last)))
        })
        .flatten();
    Ok(AgentDetailResponse {
        header: header(query, coverage(&selection, &sessions, context), context),
        agent_key: agent_key.to_string(),
        totals: aggregate(&sessions),
        sessions_page: page
            .iter()
            .map(|session| session_summary(session))
            .collect(),
        next_session_cursor,
    })
}

/// One page of the whole retained session, which must belong to this selection.
pub(crate) fn build_changes_page(
    records: &[HistoryRecord],
    query: &ReportQuery,
    agent_key: &str,
    logical_session_id: Uuid,
    context: ReportContext,
) -> Result<ChangesPageResponse, ReportError> {
    query.validate()?;
    let session_id = Some(logical_session_id);
    let target = PageTarget::new(PageKind::Records, Some(agent_key), session_id);
    let after: Option<(i64, u64, Uuid)> = cursor_after(query, target)?;
    let selection = select(records, query);
    let sessions = agent_sessions(&selection, query, agent_key)?;
    let session = sessions
        .iter()
        .find(|session| session.id == logical_session_id)
        .ok_or(ReportError::NotInSelection("session_outside_selection"))?;
    let (page, more) = page_after(&session.records, |record| record_order(record), after);
    let next_record_cursor = more
        .then(|| {
            page.last()
                .map(|last| next_cursor(query, target, &record_order(last)))
        })
        .flatten();
    Ok(ChangesPageResponse {
        header: header(query, coverage(&selection, &[*session], context), context),
        agent_key: agent_key.to_string(),
        logical_session_id,
        records_page: page.iter().map(|record| (*record).clone()).collect(),
        next_record_cursor,
    })
}

/// How a live session's history scope was resolved by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SidebarScopeKind {
    Known,
    Ambiguous,
    Unknown,
}

/// A resolved scope: known has exactly one team, ambiguous two or more sorted
/// unique candidates, unknown none or an unverifiable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidebarScope {
    pub(crate) kind: SidebarScopeKind,
    pub(crate) agent_key: Option<String>,
    pub(crate) team_ids: Vec<String>,
}

impl SidebarScope {
    /// `(agentKey, teamId)` only for a consistent known scope.
    fn known_key(&self) -> Option<(&str, &str)> {
        match (
            self.kind,
            self.agent_key.as_deref(),
            self.team_ids.as_slice(),
        ) {
            (SidebarScopeKind::Known, Some(agent_key), [team_id]) => Some((agent_key, team_id)),
            _ => None,
        }
    }
}

/// One live runtime session and the scope the runtime resolved for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarTarget {
    pub(crate) runtime_session_id: Uuid,
    pub(crate) scope: SidebarScope,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidebarBucket {
    pub(crate) days: u32,
    pub(crate) from_utc_ms: i64,
    pub(crate) to_utc_ms: i64,
    #[serde(flatten)]
    pub(crate) totals: Aggregate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidebarWindows {
    pub(crate) day: SidebarBucket,
    pub(crate) week: SidebarBucket,
    pub(crate) month: SidebarBucket,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidebarEntry {
    pub(crate) runtime_session_id: Uuid,
    pub(crate) scope: SidebarScope,
    pub(crate) windows: Option<SidebarWindows>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidebarMeansResponse {
    pub(crate) schema_version: u32,
    pub(crate) generated_at_utc_ms: i64,
    pub(crate) entries: Vec<SidebarEntry>,
}

/// 1..=100 unique runtime session ids, checked before any history IO.
pub(crate) fn validate_sidebar_ids(ids: &[Uuid]) -> Result<(), ReportError> {
    if ids.is_empty() {
        return Err(ReportError::InvalidQuery("sidebar_ids_empty"));
    }
    if ids.len() > MAX_SIDEBAR_SESSIONS {
        return Err(ReportError::InvalidQuery("sidebar_ids_too_many"));
    }
    let unique: BTreeSet<&Uuid> = ids.iter().collect();
    if unique.len() != ids.len() {
        return Err(ReportError::InvalidQuery("sidebar_ids_duplicate"));
    }
    Ok(())
}

fn sidebar_bucket(selection: &Selection, days: u32, now: i64) -> SidebarBucket {
    let from = now - i64::from(days) * DAY_MS;
    let sessions: Vec<&SessionView> = selection
        .sessions
        .iter()
        .filter(|session| session.started_at_utc_ms >= from)
        .collect();
    let mut totals = aggregate(&sessions);
    totals.incomplete |= selection.unknown_start > 0;
    SidebarBucket {
        days,
        from_utc_ms: from,
        to_utc_ms: now,
        totals,
    }
}

/// Day, week and month buckets of one known scope, from one scan and one `now`.
fn known_windows(
    records: &[HistoryRecord],
    project_path: &str,
    agent_key: &str,
    team_id: &str,
    now: i64,
) -> SidebarWindows {
    let [day, week, month] = SIDEBAR_WINDOW_DAYS;
    let widest = now - i64::from(month) * DAY_MS;
    let query = ReportQuery {
        project_path: project_path.to_string(),
        from_utc_ms: widest,
        to_utc_ms: now,
        agent_key: Some(agent_key.to_string()),
        team_id: Some(team_id.to_string()),
        coding_agent_id: LaunchFilter::Any,
        model: LaunchFilter::Any,
        effort: LaunchFilter::Any,
        cursor: None,
    };
    let selection = select(records, &query);
    SidebarWindows {
        day: sidebar_bucket(&selection, day, now),
        week: sidebar_bucket(&selection, week, now),
        month: sidebar_bucket(&selection, month, now),
    }
}

/// Sidebar means per runtime session. Only a known scope computes windows;
/// ambiguous and unknown scopes return `windows: null`, never an empty count.
pub(crate) fn sidebar_means(
    records: &[HistoryRecord],
    project_path: &str,
    targets: &[SidebarTarget],
    now: i64,
) -> Result<SidebarMeansResponse, ReportError> {
    let ids: Vec<Uuid> = targets
        .iter()
        .map(|target| target.runtime_session_id)
        .collect();
    validate_sidebar_ids(&ids)?;
    check_identity(project_path, "project_path_empty", "project_path_too_long")?;
    let mut cache: BTreeMap<(&str, &str), SidebarWindows> = BTreeMap::new();
    for target in targets {
        if let Some(key) = target.scope.known_key() {
            cache
                .entry(key)
                .or_insert_with(|| known_windows(records, project_path, key.0, key.1, now));
        }
    }
    let entries = targets
        .iter()
        .map(|target| SidebarEntry {
            runtime_session_id: target.runtime_session_id,
            scope: target.scope.clone(),
            windows: target
                .scope
                .known_key()
                .and_then(|key| cache.get(&key).cloned()),
        })
        .collect();
    Ok(SidebarMeansResponse {
        schema_version: REPORT_SCHEMA_VERSION,
        generated_at_utc_ms: now,
        entries,
    })
}

#[cfg(test)]
mod tests;
