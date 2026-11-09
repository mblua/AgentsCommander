//! Whole retained sessions and their count windows (#4091 P02).
//!
//! A session view holds one logical session's retained records in stable
//! order. Count selection keeps the newest `N` known starts of every scope
//! `(agentKey, teamAttribution, sorted teamIds)`; ties break on the session id.
//! The parent filters records first; this module never sees a query.

use std::collections::BTreeMap;

use serde::Serialize;
use uuid::Uuid;

use crate::context_history::{BoundaryReason, HistoryRecord, RecordKind, TeamAttribution};

/// Label of a session whose retained data is known to be incomplete.
pub(crate) const INCOMPLETE_SESSION_LABEL: &str = "Datos conservados de sesión incompleta";

/// The positive value of a `change` record; zero, null and boundaries never count.
pub(crate) fn positive(record: &HistoryRecord) -> Option<u8> {
    (record.kind == RecordKind::Change)
        .then_some(record.percent)
        .flatten()
        .filter(|percent| *percent > 0)
}

pub(crate) fn record_order(record: &HistoryRecord) -> (i64, u64, Uuid) {
    (record.observed_at_utc_ms, record.sequence, record.record_id)
}

/// One selected logical session: its retained records in stable order.
#[derive(Debug, Clone)]
pub(crate) struct SessionView<'a> {
    pub(crate) id: Uuid,
    pub(crate) started_at_utc_ms: i64,
    pub(crate) records: Vec<&'a HistoryRecord>,
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
    pub(crate) fn incomplete(self) -> bool {
        self.partial_start || self.partial_end || self.retention_clipped
    }
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn mean_of(sum: u64, count: u64) -> Option<f64> {
    (count > 0).then(|| sum as f64 / count as f64)
}

impl<'a> SessionView<'a> {
    pub(crate) fn first(&self) -> &'a HistoryRecord {
        self.records[0]
    }

    pub(crate) fn stats(&self) -> SessionStats {
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

pub(crate) fn session_order(session: &SessionView) -> (i64, Uuid) {
    (session.started_at_utc_ms, session.id)
}

/// How many sessions one scope asked for and how many retained ones matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopeWindow {
    /// Owner of the scope; agent pages already carry it.
    #[serde(skip)]
    pub(crate) agent_key: String,
    pub(crate) team_attribution: TeamAttribution,
    pub(crate) team_ids: Vec<String>,
    pub(crate) requested_sessions: u32,
    pub(crate) available_sessions: u32,
}

/// The scopes of one agent; `None` (date mode) stays `None`.
pub(crate) fn agent_windows(
    windows: Option<&[ScopeWindow]>,
    agent_key: &str,
) -> Option<Vec<ScopeWindow>> {
    let owned = windows?
        .iter()
        .filter(|window| window.agent_key == agent_key);
    Some(owned.cloned().collect())
}

type ScopeKey = (String, &'static str, Vec<String>);

fn scope_key(record: &HistoryRecord) -> ScopeKey {
    let mut team_ids = record.team_ids.clone();
    team_ids.sort();
    let attribution = record.team_attribution.as_str();
    (record.agent_key.clone(), attribution, team_ids)
}

/// The newest `count` sessions of a slice ordered by [`session_order`].
pub(crate) fn newest<'s, 'a>(sessions: &'s [SessionView<'a>], count: u32) -> &'s [SessionView<'a>] {
    let keep = usize::try_from(count).unwrap_or(usize::MAX);
    &sessions[sessions.len().saturating_sub(keep)..]
}

/// The newest `count` sessions of every scope, in ascending session order,
/// and each scope's availability. Every session must have a known start.
pub(crate) fn recent_per_scope(
    sessions: Vec<SessionView<'_>>,
    count: u32,
) -> (Vec<SessionView<'_>>, Vec<ScopeWindow>) {
    let mut scopes: BTreeMap<ScopeKey, Vec<SessionView>> = BTreeMap::new();
    for session in sessions {
        scopes
            .entry(scope_key(session.first()))
            .or_default()
            .push(session);
    }
    let mut selected = Vec::new();
    let mut windows = Vec::with_capacity(scopes.len());
    for (key, mut group) in scopes {
        group.sort_by_key(session_order);
        let chosen = newest(&group, count);
        windows.push(ScopeWindow {
            agent_key: key.0,
            team_attribution: group[0].first().team_attribution,
            team_ids: key.2,
            requested_sessions: count,
            available_sessions: u32::try_from(chosen.len()).unwrap_or(u32::MAX),
        });
        selected.extend_from_slice(chosen);
    }
    selected.sort_by_key(session_order);
    (selected, windows)
}

#[cfg(test)]
mod tests;
