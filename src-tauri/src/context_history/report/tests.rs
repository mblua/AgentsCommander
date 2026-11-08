use super::session_windows::INCOMPLETE_SESSION_LABEL;
use super::*;
use crate::context_history::{BoundaryReason, RecordKind};

const PROJECT: &str = "C:/projects/demo";
const NOW: i64 = 1_800_000_000_000;
const HOUR_MS: i64 = 60 * 60 * 1000;
const COUNTS: [u32; 3] = [25, 100, 300];
const CONTEXT: ReportContext = ReportContext {
    retention_days: 30,
    generated_at_utc_ms: NOW,
};

fn record(session: u128, index: u128, observed: i64, percent: u8) -> HistoryRecord {
    let (kind, percent, boundary_reason) = if percent == 0 {
        (
            RecordKind::Boundary,
            Some(0),
            Some(BoundaryReason::ObservedZero),
        )
    } else {
        (RecordKind::Change, Some(percent), None)
    };
    HistoryRecord {
        schema_version: 2,
        record_id: Uuid::from_u128(session * 1000 + index),
        app_run_id: Uuid::from_u128(7),
        runtime_session_id: Uuid::from_u128(8),
        logical_session_id: Uuid::from_u128(session),
        sequence: u64::try_from(index).expect("index"),
        ordinal: 1,
        observed_at_utc_ms: observed,
        logical_started_at_utc_ms: Some(observed),
        partial_start: false,
        partial_end: false,
        initial_record_id: Some(Uuid::from_u128(session * 1000)),
        initial_kind: InitialKind::FirstObserved,
        continuity_known: true,
        project_path: PROJECT.to_string(),
        project_name: "demo".to_string(),
        agent_key: "dev".to_string(),
        instance_fqn: "demo:room-1/dev".to_string(),
        room: Some("room-1".to_string()),
        matrix_type: MatrixType::Replica,
        team_attribution: TeamAttribution::Known,
        team_ids: vec!["team-a".to_string()],
        coding_agent_id: Some("claude".to_string()),
        model: Some("opus".to_string()),
        effort: None,
        metadata_source: MetadataSource::LaunchCommand,
        context_window: None,
        kind,
        percent,
        boundary_reason,
    }
}

/// One logical session started at `started`, with `(observedAt, percent)` values.
fn session(id: u128, started: i64, values: &[(i64, u8)]) -> Vec<HistoryRecord> {
    values
        .iter()
        .zip(0_u128..)
        .map(|(&(observed, percent), index)| {
            let mut record = record(id, index, observed, percent);
            record.logical_started_at_utc_ms = Some(started);
            record
        })
        .collect()
}

fn with(mut records: Vec<HistoryRecord>, edit: impl Fn(&mut HistoryRecord)) -> Vec<HistoryRecord> {
    records.iter_mut().for_each(edit);
    records
}

fn query(from: i64, to: i64) -> ReportQuery {
    ReportQuery {
        project_path: PROJECT.to_string(),
        from_utc_ms: from,
        to_utc_ms: to,
        agent_key: None,
        team_id: None,
        coding_agent_id: LaunchFilter::Any,
        model: LaunchFilter::Any,
        effort: LaunchFilter::Any,
        cursor: None,
        session_count: None,
    }
}

fn last_days(days: i64) -> ReportQuery {
    query(NOW - days * DAY_MS, NOW)
}

fn report(records: &[HistoryRecord], query: &ReportQuery) -> ReportResponse {
    build_report(records, query, CONTEXT).expect("report")
}

fn only_agent(response: &ReportResponse) -> Aggregate {
    assert_eq!(response.agents_page.len(), 1, "one agent");
    response.agents_page[0].totals
}

#[test]
fn approved_example_selects_only_the_session_started_in_the_window() {
    let mut records = session(1, NOW - 40 * DAY_MS, &[(NOW - DAY_MS, 90)]);
    records.extend(session(
        2,
        NOW - DAY_MS,
        &[(NOW - DAY_MS, 10), (NOW - HOUR_MS, 20)],
    ));
    let totals = only_agent(&report(&records, &last_days(7)));
    assert_eq!((totals.records_count, totals.positive_sum), (2, 30));
    assert_eq!(totals.mean, Some(15.0));
    assert_eq!(totals.sessions_count, 1);
}

#[test]
fn agent_mean_is_over_records_not_a_mean_of_means() {
    let mut records = session(
        1,
        NOW - 3 * DAY_MS,
        &[(NOW - 3 * DAY_MS, 1), (NOW - DAY_MS, 90)],
    );
    records.extend(session(2, NOW - 2 * DAY_MS, &[(NOW - 2 * DAY_MS, 20)]));
    let totals = only_agent(&report(&records, &last_days(7)));
    assert_eq!((totals.records_count, totals.positive_sum), (3, 111));
    assert_eq!(totals.mean, Some(37.0));
}

#[test]
fn start_at_from_is_included_and_start_at_to_is_excluded() {
    let from = NOW - 7 * DAY_MS;
    let mut records = session(1, from, &[(from, 10)]);
    records.extend(session(2, NOW, &[(NOW, 50)]));
    let response = report(&records, &query(from, NOW));
    assert_eq!(only_agent(&response).positive_sum, 10);
    assert_eq!(
        response.header.requested_period.selection,
        "session_started"
    );
}

#[test]
fn old_session_with_a_change_inside_the_window_is_excluded() {
    let records = session(
        1,
        NOW - 10 * DAY_MS,
        &[(NOW - 10 * DAY_MS, 5), (NOW - HOUR_MS, 60)],
    );
    let response = report(&records, &last_days(7));
    assert!(response.agents_page.is_empty());
    assert_eq!(response.total_agents, 0);
}

#[test]
fn record_after_to_of_an_included_session_counts() {
    let to = NOW - 5 * DAY_MS;
    let records = session(1, to - DAY_MS, &[(to - DAY_MS, 10), (to + DAY_MS, 30)]);
    let totals = only_agent(&report(&records, &query(to - 7 * DAY_MS, to)));
    assert_eq!((totals.records_count, totals.positive_sum), (2, 40));
}

#[test]
fn zero_and_boundaries_never_count_and_empty_mean_is_null() {
    let mut records = session(1, NOW - DAY_MS, &[(NOW - DAY_MS, 0)]);
    let mut lifecycle = record(1, 1, NOW - HOUR_MS, 0);
    lifecycle.logical_started_at_utc_ms = Some(NOW - DAY_MS);
    lifecycle.percent = None;
    lifecycle.boundary_reason = Some(BoundaryReason::Lifecycle);
    records.push(lifecycle);
    let totals = only_agent(&report(&records, &last_days(7)));
    assert_eq!((totals.records_count, totals.positive_sum), (0, 0));
    assert_eq!(totals.mean, None);
    assert_eq!(totals.sessions_count, 1);
}

#[test]
fn two_teams_are_bucketed_without_duplicating_records() {
    let mut records = session(1, NOW - DAY_MS, &[(NOW - DAY_MS, 10)]);
    records.extend(with(
        session(2, NOW - DAY_MS, &[(NOW - HOUR_MS, 30)]),
        |record| {
            record.team_ids = vec!["team-b".to_string()];
        },
    ));
    let duplicated: Vec<HistoryRecord> = records.iter().chain(records.iter()).cloned().collect();
    let response = report(&duplicated, &last_days(7));
    assert_eq!(only_agent(&response).positive_sum, 40);
    assert_eq!(response.agents_page[0].metadata_buckets.len(), 2);
    let mut team_a = last_days(7);
    team_a.team_id = Some("team-a".to_string());
    assert_eq!(only_agent(&report(&duplicated, &team_a)).positive_sum, 10);
}

#[test]
fn team_filter_excludes_unknown_and_ambiguous_attribution() {
    let mut records = with(session(1, NOW - DAY_MS, &[(NOW - DAY_MS, 10)]), |record| {
        record.team_attribution = TeamAttribution::Ambiguous;
        record.team_ids = vec!["team-a".to_string(), "team-b".to_string()];
    });
    records.extend(with(
        session(2, NOW - DAY_MS, &[(NOW - DAY_MS, 20)]),
        |record| {
            record.team_attribution = TeamAttribution::Unknown;
            record.team_ids.clear();
        },
    ));
    let mut team_a = last_days(7);
    team_a.team_id = Some("team-a".to_string());
    assert!(report(&records, &team_a).agents_page.is_empty());
    assert_eq!(
        only_agent(&report(&records, &last_days(7))).positive_sum,
        30
    );
}

#[test]
fn launch_filters_distinguish_any_unknown_and_exact() {
    let mut records = session(1, NOW - DAY_MS, &[(NOW - DAY_MS, 10)]);
    records.extend(with(
        session(2, NOW - DAY_MS, &[(NOW - DAY_MS, 20)]),
        |record| {
            record.model = None;
        },
    ));
    let sum_for = |model: LaunchFilter| {
        let mut filtered = last_days(7);
        filtered.model = model;
        only_agent(&report(&records, &filtered)).positive_sum
    };
    assert_eq!(sum_for(LaunchFilter::Any), 30);
    assert_eq!(sum_for(LaunchFilter::Unknown), 20);
    assert_eq!(sum_for(LaunchFilter::Exact("opus".to_string())), 10);
    let mut effort = last_days(7);
    effort.effort = LaunchFilter::Exact("high".to_string());
    assert!(report(&records, &effort).agents_page.is_empty());
}

#[test]
fn unknown_start_is_excluded_and_counted() {
    let mut records = with(session(1, 0, &[(NOW - DAY_MS, 10)]), |record| {
        record.logical_started_at_utc_ms = None;
        record.partial_start = true;
        record.initial_kind = InitialKind::Unknown;
        record.initial_record_id = None;
    });
    records.extend(session(2, NOW - DAY_MS, &[(NOW - DAY_MS, 20)]));
    let response = report(&records, &last_days(7));
    assert_eq!(only_agent(&response).positive_sum, 20);
    assert_eq!(response.header.coverage.unknown_start_sessions_count, 1);
    assert!(response.header.coverage.incomplete);
}

#[test]
fn lost_initial_record_keeps_original_start_and_marks_retention_clipped() {
    let started = NOW - 2 * DAY_MS;
    let records = session(1, started, &[(started, 40), (NOW - DAY_MS, 60)]);
    let retained: Vec<HistoryRecord> = records[1..].to_vec();
    let detail = build_agent_detail(&retained, &last_days(7), "dev", CONTEXT).expect("detail");
    let summary = &detail.sessions_page[0];
    assert_eq!(summary.logical_started_at_utc_ms, started);
    assert_eq!(summary.stats.initial_value, None);
    assert!(summary.stats.retention_clipped && summary.stats.max_partial);
    assert_eq!(summary.stats.coverage_label, Some(INCOMPLETE_SESSION_LABEL));
    assert_eq!(
        (summary.stats.records_count, summary.stats.max),
        (1, Some(60))
    );
    let full = build_agent_detail(&records, &last_days(7), "dev", CONTEXT).expect("detail");
    assert_eq!(full.sessions_page[0].stats.initial_value, Some(40));
    assert!(!full.sessions_page[0].stats.retention_clipped);
}

#[test]
fn partial_end_is_explicit_in_session_and_coverage() {
    let records = with(session(1, NOW - DAY_MS, &[(NOW - DAY_MS, 10)]), |record| {
        record.partial_end = true;
    });
    let detail = build_agent_detail(&records, &last_days(7), "dev", CONTEXT).expect("detail");
    assert!(detail.sessions_page[0].stats.partial_end);
    assert!(detail.header.coverage.incomplete && detail.totals.incomplete);
}

#[test]
fn other_projects_never_mix_and_separators_are_normalized() {
    let mut records = with(session(1, NOW - DAY_MS, &[(NOW - DAY_MS, 10)]), |record| {
        record.project_path = "C:\\projects\\demo\\".to_string();
    });
    records.extend(with(
        session(2, NOW - DAY_MS, &[(NOW - DAY_MS, 20)]),
        |record| {
            record.project_path = "C:/projects/other".to_string();
        },
    ));
    assert_eq!(
        only_agent(&report(&records, &last_days(7))).positive_sum,
        10
    );
}

fn many_agents(count: u128) -> Vec<HistoryRecord> {
    (1..=count)
        .flat_map(|id| {
            with(session(id, NOW - DAY_MS, &[(NOW - DAY_MS, 10)]), |record| {
                record.agent_key = format!("agent-{id:03}");
            })
        })
        .collect()
}

#[test]
fn agent_cursor_pages_in_stable_order_and_rejects_foreign_queries() {
    let records = many_agents(150);
    let first = report(&records, &last_days(7));
    assert_eq!((first.agents_page.len(), first.total_agents), (100, 150));
    assert_eq!(first.agents_page[0].agent_key, "agent-001");
    let mut next = last_days(7);
    next.cursor.clone_from(&first.next_agent_cursor);
    let second = report(&records, &next);
    assert_eq!(second.agents_page.len(), 50);
    assert_eq!(second.agents_page[0].agent_key, "agent-101");
    assert_eq!(second.next_agent_cursor, None);
    let mut foreign = next.clone();
    foreign.project_path = "C:/projects/other".to_string();
    let error = build_report(&records, &foreign, CONTEXT).expect_err("foreign");
    assert_eq!(error, ReportError::InvalidCursor("cursor_query_mismatch"));
    let mut filtered = next.clone();
    filtered.model = LaunchFilter::Unknown;
    assert!(build_report(&records, &filtered, CONTEXT).is_err());
    let kind = build_agent_detail(&records, &next, "agent-001", CONTEXT).expect_err("kind");
    assert_eq!(
        kind,
        ReportError::InvalidCursor("cursor_page_kind_mismatch")
    );
    let mut garbage = last_days(7);
    garbage.cursor = Some("ctxh1.zz".to_string());
    assert!(build_report(&records, &garbage, CONTEXT).is_err());
}

#[test]
fn session_and_record_cursors_walk_whole_sessions() {
    let values: Vec<(i64, u8)> = (0..150).map(|index| (NOW - DAY_MS + index, 5)).collect();
    let mut records = session(1, NOW - DAY_MS, &values);
    for id in 2..=120 {
        records.extend(session(
            id,
            NOW - DAY_MS + i64::try_from(id).expect("id"),
            &[(NOW, 1)],
        ));
    }
    let detail = build_agent_detail(&records, &last_days(7), "dev", CONTEXT).expect("detail");
    assert_eq!(
        (detail.sessions_page.len(), detail.totals.sessions_count),
        (100, 120)
    );
    let mut next = last_days(7);
    next.cursor.clone_from(&detail.next_session_cursor);
    let rest = build_agent_detail(&records, &next, "dev", CONTEXT).expect("rest");
    assert_eq!(rest.sessions_page.len(), 20);
    let id = Uuid::from_u128(1);
    let page = build_changes_page(&records, &last_days(7), "dev", id, CONTEXT).expect("changes");
    assert_eq!(page.records_page.len(), 100);
    let mut more = last_days(7);
    more.cursor.clone_from(&page.next_record_cursor);
    let tail = build_changes_page(&records, &more, "dev", id, CONTEXT).expect("tail");
    assert_eq!(
        (tail.records_page.len(), tail.next_record_cursor),
        (50, None)
    );
    let other = build_changes_page(&records, &more, "dev", Uuid::from_u128(2), CONTEXT);
    assert_eq!(
        other,
        Err(ReportError::InvalidCursor("cursor_target_mismatch"))
    );
}

#[test]
fn changes_page_rejects_sessions_outside_the_selection() {
    let mut records = session(1, NOW - DAY_MS, &[(NOW - DAY_MS, 10)]);
    records.extend(session(2, NOW - 20 * DAY_MS, &[(NOW - DAY_MS, 10)]));
    let outside = build_changes_page(&records, &last_days(7), "dev", Uuid::from_u128(2), CONTEXT);
    assert_eq!(
        outside,
        Err(ReportError::NotInSelection("session_outside_selection"))
    );
    let mut scoped = last_days(7);
    scoped.agent_key = Some("other".to_string());
    let agent = build_changes_page(&records, &scoped, "dev", Uuid::from_u128(1), CONTEXT);
    assert_eq!(
        agent,
        Err(ReportError::NotInSelection("agent_outside_query"))
    );
}

#[test]
fn invalid_queries_are_rejected() {
    let reject = |query: ReportQuery| build_report(&[], &query, CONTEXT).is_err();
    assert!(reject(query(NOW, NOW)));
    assert!(reject(query(NOW, NOW - 1)));
    assert!(reject(last_days(32)));
    assert!(!reject(query(NOW - MAX_PERIOD_MS, NOW)));
    let mut long = last_days(1);
    long.model = LaunchFilter::Exact("x".repeat(MAX_METADATA_BYTES + 1));
    assert!(reject(long));
    let mut empty = last_days(1);
    empty.agent_key = Some(String::new());
    assert!(reject(empty));
}

#[test]
fn empty_history_is_an_explicit_empty_report() {
    let response = report(&[], &last_days(7));
    assert_eq!(response.total_agents, 0);
    assert_eq!(response.header.coverage.retained_earliest_utc_ms, None);
    let json = serde_json::to_value(&response).expect("json");
    assert_eq!(json["schemaVersion"], 3);
    assert!(json["nextAgentCursor"].is_null());
    assert_eq!(json["requestedPeriod"]["selection"], "session_started");
}

fn known(id: u128) -> SidebarTarget {
    SidebarTarget {
        runtime_session_id: Uuid::from_u128(id),
        scope: SidebarScope {
            kind: SidebarScopeKind::Known,
            agent_key: Some("dev".to_string()),
            team_ids: vec!["team-a".to_string()],
        },
    }
}

#[test]
fn sidebar_windows_use_one_now_and_session_start() {
    let mut records = session(1, NOW - 2 * HOUR_MS, &[(NOW - HOUR_MS, 10)]);
    records.extend(session(2, NOW - 3 * DAY_MS, &[(NOW - HOUR_MS, 20)]));
    records.extend(session(3, NOW - 20 * DAY_MS, &[(NOW - HOUR_MS, 30)]));
    records.extend(session(4, NOW - 40 * DAY_MS, &[(NOW - HOUR_MS, 50)]));
    let response =
        sidebar_means(&records, PROJECT, &[known(1), known(2)], &COUNTS, NOW).expect("means");
    let windows = response.entries[0].windows.clone().expect("known windows");
    assert_eq!(windows.day.totals.mean, Some(10.0));
    assert_eq!(windows.week.totals.mean, Some(15.0));
    assert_eq!(windows.month.totals.mean, Some(20.0));
    assert_eq!((windows.week.days, windows.week.to_utc_ms), (7, NOW));
    assert_eq!(response.entries[1].windows, response.entries[0].windows);
    let month = report(&records, &{
        let mut parity = last_days(30);
        parity.agent_key = Some("dev".to_string());
        parity.team_id = Some("team-a".to_string());
        parity
    });
    assert_eq!(only_agent(&month), windows.month.totals);
}

#[test]
fn sidebar_ambiguous_and_unknown_scopes_have_null_windows() {
    let records = session(1, NOW - HOUR_MS, &[(NOW - HOUR_MS, 10)]);
    let mut ambiguous = known(1);
    ambiguous.scope.kind = SidebarScopeKind::Ambiguous;
    ambiguous.scope.team_ids = vec!["team-a".to_string(), "team-b".to_string()];
    let mut unknown = known(2);
    unknown.scope.kind = SidebarScopeKind::Unknown;
    unknown.scope.team_ids.clear();
    let response =
        sidebar_means(&records, PROJECT, &[ambiguous, unknown], &COUNTS, NOW).expect("means");
    assert!(response.entries.iter().all(|entry| entry.windows.is_none()));
    let json = serde_json::to_value(&response.entries[0]).expect("json");
    assert!(json["windows"].is_null());
    assert_eq!(json["scope"]["kind"], "ambiguous");
}

#[test]
fn sidebar_known_scope_without_records_is_an_empty_count_not_null() {
    let response = sidebar_means(&[], PROJECT, &[known(1)], &COUNTS, NOW).expect("means");
    let windows = response.entries[0].windows.clone().expect("known windows");
    assert_eq!(
        (windows.day.totals.records_count, windows.day.totals.mean),
        (0, None)
    );
}

#[test]
fn sidebar_ids_are_validated_before_any_scan() {
    assert!(sidebar_means(&[], PROJECT, &[], &COUNTS, NOW).is_err());
    assert!(sidebar_means(&[], PROJECT, &[known(1), known(1)], &COUNTS, NOW).is_err());
    let many: Vec<SidebarTarget> = (0..=100).map(known).collect();
    assert_eq!(
        sidebar_means(&[], PROJECT, &many, &COUNTS, NOW).map(|_| ()),
        Err(ReportError::InvalidQuery("sidebar_ids_too_many"))
    );
}

#[test]
fn root_sessions_never_reach_the_report() {
    use crate::context_history::observation::{resolve_history_scope, ScopeInput, ScopeResolution};
    let root = ScopeInput {
        working_directory: "C:/projects/demo/.ac/_agent_dev",
        is_root_agent: true,
        authoritative_ac_root: true,
        teams: &[],
    };
    assert_eq!(
        resolve_history_scope(&root),
        ScopeResolution::Excluded("root_agent")
    );
    let response = report(&[], &last_days(7));
    assert_eq!(response.total_agents, 0);
    assert!(response.agents_page.is_empty());
}

fn recent(count: u32) -> ReportQuery {
    let mut query = query(0, NOW);
    query.session_count = Some(count);
    query
}

fn on_team(records: Vec<HistoryRecord>, team: &str) -> Vec<HistoryRecord> {
    with(records, |record| record.team_ids = vec![team.to_string()])
}

fn detail_ids(records: &[HistoryRecord], query: &ReportQuery) -> Vec<u128> {
    let detail = build_agent_detail(records, query, "dev", CONTEXT).expect("detail");
    let ids = detail.sessions_page.iter();
    ids.map(|session| session.logical_session_id.as_u128())
        .collect()
}

#[test]
fn count_mode_takes_the_newest_sessions_despite_age_with_id_tie_break() {
    let mut records = session(1, NOW - 100 * DAY_MS, &[(NOW - 100 * DAY_MS, 10)]);
    records.extend(session(3, NOW - 50 * DAY_MS, &[(NOW - 50 * DAY_MS, 20)]));
    records.extend(session(2, NOW - 50 * DAY_MS, &[(NOW - 50 * DAY_MS, 30)]));
    records.extend(session(4, NOW - DAY_MS, &[(NOW - DAY_MS, 40)]));
    records.extend(session(5, NOW, &[(NOW, 50)]));
    assert_eq!(detail_ids(&records, &recent(2)), [3, 4]);
    assert_eq!(detail_ids(&records, &recent(3)), [2, 3, 4]);
    assert_eq!(detail_ids(&records, &recent(25)), [1, 2, 3, 4]);
    let response = report(&records, &recent(2));
    let json = serde_json::to_value(&response).expect("json");
    assert_eq!(json["requestedPeriod"]["selection"], "recent_sessions");
    assert_eq!(json["requestedPeriod"]["sessionCount"], 2);
    assert_eq!(json["agentsPage"][0]["recordsPerSession"], 1.0);
    let windows = &json["agentsPage"][0]["scopeWindows"];
    assert_eq!(windows[0]["requestedSessions"], 2);
    assert_eq!(windows[0]["availableSessions"], 2);
    assert_eq!(windows[0]["teamIds"], serde_json::json!(["team-a"]));
    assert_eq!(windows[0]["teamAttribution"], "known");
    assert!(windows[0].get("agentKey").is_none());
}

#[test]
fn date_mode_serializes_null_session_count_and_scope_windows() {
    let records = session(1, NOW - DAY_MS, &[(NOW - DAY_MS, 10)]);
    let json = serde_json::to_value(report(&records, &last_days(7))).expect("json");
    assert!(json["requestedPeriod"]["sessionCount"].is_null());
    assert!(json["agentsPage"][0]["scopeWindows"].is_null());
    let detail = build_agent_detail(&records, &last_days(7), "dev", CONTEXT).expect("detail");
    let json = serde_json::to_value(detail).expect("json");
    assert!(json["scopeWindows"].is_null());
    assert_eq!(json["recordsPerSession"], 1.0);
}

#[test]
fn all_teams_select_n_per_scope_and_weight_records_not_ratios() {
    let mut records = on_team(session(1, NOW - 9 * DAY_MS, &[(NOW, 90)]), "team-a");
    records.extend(on_team(
        session(2, NOW - 8 * DAY_MS, &[(NOW, 10), (NOW, 20), (NOW, 30)]),
        "team-a",
    ));
    records.extend(on_team(
        session(3, NOW - 7 * DAY_MS, &[(NOW, 40)]),
        "team-a",
    ));
    records.extend(on_team(
        session(4, NOW - 99 * DAY_MS, &[(NOW, 60)]),
        "team-b",
    ));
    let totals = only_agent(&report(&records, &recent(2)));
    assert_eq!((totals.sessions_count, totals.records_count), (3, 5));
    assert_eq!(totals.positive_sum, 160);
    assert_eq!(totals.mean, Some(32.0));
    let per_session = totals.records_per_session.expect("ratio");
    assert!(
        (per_session - 5.0 / 3.0).abs() < 1e-12,
        "not a mean of 2.0 and 1.0"
    );
    let detail = build_agent_detail(&records, &recent(2), "dev", CONTEXT).expect("detail");
    let windows = detail.scope_windows.expect("count mode");
    let availability: Vec<(&str, u32, u32)> = windows
        .iter()
        .map(|w| {
            (
                w.team_ids[0].as_str(),
                w.requested_sessions,
                w.available_sessions,
            )
        })
        .collect();
    assert_eq!(availability, [("team-a", 2, 2), ("team-b", 2, 1)]);
    let mut team_a = recent(2);
    team_a.team_id = Some("team-a".to_string());
    assert_eq!(detail_ids(&records, &team_a), [2, 3]);
}

#[test]
fn count_mode_filters_metadata_before_ranking_and_keeps_unknown_scopes_apart() {
    let mut records = session(1, NOW - 3 * DAY_MS, &[(NOW, 10)]);
    records.extend(with(session(2, NOW - DAY_MS, &[(NOW, 20)]), |record| {
        record.model = Some("haiku".to_string());
    }));
    records.extend(with(session(3, NOW - 2 * DAY_MS, &[(NOW, 30)]), |record| {
        record.team_attribution = TeamAttribution::Unknown;
        record.team_ids.clear();
    }));
    let mut opus = recent(1);
    opus.model = LaunchFilter::Exact("opus".to_string());
    assert_eq!(detail_ids(&records, &opus), [1, 3], "haiku is never ranked");
    let all = build_agent_detail(&records, &recent(1), "dev", CONTEXT).expect("detail");
    let scopes = all.scope_windows.expect("count mode");
    assert_eq!(scopes.len(), 2);
    assert_eq!(scopes[1].team_attribution, TeamAttribution::Unknown);
    assert_eq!(all.totals.sessions_count, 2);
}

#[test]
fn count_mode_excludes_unknown_starts_and_counts_late_records() {
    let mut records = with(session(1, 0, &[(NOW - DAY_MS, 10)]), |record| {
        record.logical_started_at_utc_ms = None;
        record.initial_record_id = None;
    });
    records.extend(session(
        2,
        NOW - DAY_MS,
        &[(NOW - DAY_MS, 20), (NOW + DAY_MS, 40)],
    ));
    let response = report(&records, &recent(25));
    assert_eq!(response.header.coverage.unknown_start_sessions_count, 1);
    assert!(response.header.coverage.incomplete);
    let totals = only_agent(&response);
    assert_eq!((totals.records_count, totals.positive_sum), (2, 60));
}

#[test]
fn records_per_session_divides_by_actual_selected_sessions() {
    let mut records = Vec::new();
    for id in 1..=100_u128 {
        let count = if id <= 60 { 16 } else { 15 };
        let start = NOW - i64::try_from(id).expect("id") * HOUR_MS;
        let values: Vec<(i64, u8)> = (0..count).map(|index| (start + index, 5)).collect();
        records.extend(session(id, start, &values));
    }
    let totals = only_agent(&report(&records, &recent(100)));
    assert_eq!((totals.records_count, totals.sessions_count), (1560, 100));
    assert_eq!(totals.records_per_session, Some(15.6));
    let few: Vec<(i64, u8)> = (0..20).map(|index| (NOW - DAY_MS + index, 5)).collect();
    let mut two = session(1, NOW - DAY_MS, &few);
    two.extend(session(2, NOW - HOUR_MS, &[(NOW - HOUR_MS, 7)]));
    let totals = only_agent(&report(&two, &recent(100)));
    assert_eq!(totals.records_per_session, Some(10.5));
    let empty = build_agent_detail(&[], &recent(100), "dev", CONTEXT).expect("empty");
    let empty = (
        empty.totals.sessions_count,
        empty.totals.records_per_session,
    );
    assert_eq!(empty, (0, None));
    let boundary = session(1, NOW - DAY_MS, &[(NOW - DAY_MS, 0)]);
    let totals = only_agent(&report(&boundary, &recent(25)));
    assert_eq!(
        (totals.sessions_count, totals.records_per_session),
        (1, Some(0.0))
    );
    assert_eq!(totals.mean, None);
}

#[test]
fn count_mode_validation_has_no_date_bound() {
    let reject = |query: ReportQuery| build_report(&[], &query, CONTEXT).err();
    assert_eq!(reject(recent(1)), None);
    assert_eq!(reject(recent(MAX_SESSION_COUNT)), None);
    let range = Some(ReportError::InvalidQuery("session_count_out_of_range"));
    assert_eq!(reject(recent(0)), range);
    assert_eq!(reject(recent(MAX_SESSION_COUNT + 1)), range);
    let mut dated = recent(25);
    dated.from_utc_ms = NOW - DAY_MS;
    let from = Some(ReportError::InvalidQuery("count_mode_from_not_zero"));
    assert_eq!(reject(dated), from);
    let mut cutoff = recent(25);
    cutoff.to_utc_ms = 0;
    let cutoff_error = Some(ReportError::InvalidQuery("count_mode_cutoff_not_positive"));
    assert_eq!(reject(cutoff), cutoff_error);
}

#[test]
fn count_cursors_page_full_selection_and_bind_count_cutoff_and_filters() {
    let records: Vec<HistoryRecord> = (1..=150_u128)
        .flat_map(|id| {
            let start = NOW - i64::try_from(id).expect("id") * DAY_MS;
            session(id, start, &[(NOW - DAY_MS, 3)])
        })
        .collect();
    let first = build_agent_detail(&records, &recent(300), "dev", CONTEXT).expect("first");
    assert_eq!(
        (first.sessions_page.len(), first.totals.sessions_count),
        (100, 150)
    );
    let mut next = recent(300);
    next.cursor.clone_from(&first.next_session_cursor);
    assert!(next
        .cursor
        .as_deref()
        .is_some_and(|text| text.starts_with("ctxh2.")));
    let rest = build_agent_detail(&records, &next, "dev", CONTEXT).expect("rest");
    assert_eq!(
        (rest.sessions_page.len(), rest.next_session_cursor),
        (50, None)
    );
    assert_eq!(rest.totals, first.totals, "pages never change totals");
    let mismatch = Err(ReportError::InvalidCursor("cursor_query_mismatch"));
    let mut count = next.clone();
    count.session_count = Some(100);
    assert_eq!(
        build_agent_detail(&records, &count, "dev", CONTEXT),
        mismatch
    );
    let mut cutoff = next.clone();
    cutoff.to_utc_ms -= 1;
    assert_eq!(
        build_agent_detail(&records, &cutoff, "dev", CONTEXT),
        mismatch
    );
    let mut filter = next.clone();
    filter.effort = LaunchFilter::Unknown;
    assert_eq!(
        build_agent_detail(&records, &filter, "dev", CONTEXT),
        mismatch
    );
    let mut dated = last_days(7);
    dated.cursor.clone_from(&next.cursor);
    assert_eq!(
        build_agent_detail(&records, &dated, "dev", CONTEXT),
        mismatch
    );
}

#[test]
fn version_one_cursors_are_refused() {
    let mut old = recent(25);
    old.cursor = Some("ctxh1.7b7d".to_string());
    assert_eq!(
        build_report(&[], &old, CONTEXT).err(),
        Some(ReportError::InvalidCursor("cursor_version_unsupported"))
    );
}

#[test]
fn sidebar_session_windows_follow_configured_order_and_actual_counts() {
    let mut records = session(1, NOW - 90 * DAY_MS, &[(NOW - HOUR_MS, 10), (NOW, 30)]);
    records.extend(session(2, NOW - 60 * DAY_MS, &[(NOW - HOUR_MS, 20)]));
    records.extend(session(3, NOW - 3 * DAY_MS, &[(NOW - HOUR_MS, 60)]));
    records.extend(session(4, NOW, &[(NOW, 99)]));
    let counts = [2, 1, 25];
    let response = sidebar_means(&records, PROJECT, &[known(1)], &counts, NOW).expect("means");
    let buckets = response.entries[0].session_windows.clone().expect("known");
    let shape: Vec<(u32, u64, u64)> = buckets
        .iter()
        .map(|b| {
            (
                b.requested_sessions,
                b.totals.sessions_count,
                b.totals.positive_sum,
            )
        })
        .collect();
    assert_eq!(shape, [(2, 2, 80), (1, 1, 60), (25, 3, 120)]);
    assert_eq!(buckets[1].totals.mean, Some(60.0));
    assert_eq!(buckets[2].totals.records_per_session, Some(4.0 / 3.0));
    assert!(
        response.entries[0].windows.is_some(),
        "transitional buckets stay"
    );
    let json = serde_json::to_value(&response.entries[0]).expect("json");
    assert_eq!(json["sessionWindows"][0]["requestedSessions"], 2);
    assert_eq!(json["sessionWindows"][0]["recordsPerSession"], 1.0);
    assert!(json["sessionWindows"][0].get("fromUtcMs").is_none());
    assert_eq!(json["windows"]["week"]["recordsPerSession"], 1.0);
    let parity = report(&records, &{
        let mut parity = recent(2);
        parity.team_id = Some("team-a".to_string());
        parity
    });
    assert_eq!(only_agent(&parity), buckets[0].totals);
}

#[test]
fn sidebar_session_windows_are_zero_for_known_and_null_otherwise() {
    let empty = sidebar_means(&[], PROJECT, &[known(1)], &COUNTS, NOW).expect("means");
    let buckets = empty.entries[0].session_windows.clone().expect("known");
    assert_eq!(buckets.len(), 3);
    assert!(buckets
        .iter()
        .all(|bucket| bucket.totals.sessions_count == 0
            && bucket.totals.mean.is_none()
            && bucket.totals.records_per_session.is_none()));
    let mut ambiguous = known(2);
    ambiguous.scope.kind = SidebarScopeKind::Ambiguous;
    ambiguous.scope.team_ids = vec!["team-a".to_string(), "team-b".to_string()];
    let response = sidebar_means(&[], PROJECT, &[ambiguous], &COUNTS, NOW).expect("means");
    let json = serde_json::to_value(&response.entries[0]).expect("json");
    assert!(json["sessionWindows"].is_null());
}
