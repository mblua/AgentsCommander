use super::*;
use crate::context_history::{InitialKind, MatrixType, MetadataSource};

const NOW: i64 = 1_800_000_000_000;

fn record(session: u128, started: i64, teams: &[&str], percent: u8) -> HistoryRecord {
    let attribution = match teams.len() {
        0 => TeamAttribution::Unknown,
        1 => TeamAttribution::Known,
        _ => TeamAttribution::Ambiguous,
    };
    HistoryRecord {
        schema_version: 2,
        record_id: Uuid::from_u128(session * 1000),
        app_run_id: Uuid::from_u128(7),
        runtime_session_id: Uuid::from_u128(8),
        logical_session_id: Uuid::from_u128(session),
        sequence: 0,
        ordinal: 1,
        observed_at_utc_ms: started,
        logical_started_at_utc_ms: Some(started),
        partial_start: false,
        partial_end: false,
        initial_record_id: Some(Uuid::from_u128(session * 1000)),
        initial_kind: InitialKind::FirstObserved,
        continuity_known: true,
        project_path: "C:/projects/demo".to_string(),
        project_name: "demo".to_string(),
        agent_key: "dev".to_string(),
        instance_fqn: "demo:room-1/dev".to_string(),
        room: None,
        matrix_type: MatrixType::Replica,
        team_attribution: attribution,
        team_ids: teams.iter().map(|team| (*team).to_string()).collect(),
        coding_agent_id: None,
        model: None,
        effort: None,
        metadata_source: MetadataSource::Unavailable,
        context_window: None,
        kind: RecordKind::Change,
        percent: Some(percent),
        boundary_reason: None,
    }
}

fn view(record: &HistoryRecord) -> SessionView<'_> {
    SessionView {
        id: record.logical_session_id,
        started_at_utc_ms: record.logical_started_at_utc_ms.expect("known start"),
        records: vec![record],
    }
}

fn ids(sessions: &[SessionView]) -> Vec<u128> {
    sessions
        .iter()
        .map(|session| session.id.as_u128())
        .collect()
}

#[test]
fn newest_keeps_the_tail_and_never_more_than_available() {
    let records: Vec<HistoryRecord> = (1..=4)
        .map(|id| record(id, NOW + i64::try_from(id).expect("id"), &["a"], 5))
        .collect();
    let views: Vec<SessionView> = records.iter().map(view).collect();
    assert_eq!(ids(newest(&views, 2)), [3, 4]);
    assert_eq!(ids(newest(&views, 10_000)), [1, 2, 3, 4]);
    assert!(newest(&views, 0).is_empty());
}

#[test]
fn recent_per_scope_ranks_each_scope_with_uuid_tie_break() {
    let records = [
        record(1, NOW - 10, &["a"], 5),
        record(3, NOW, &["a"], 5),
        record(2, NOW, &["a"], 5),
        record(4, NOW - 99, &["b"], 5),
        record(5, NOW - 50, &["b", "c"], 5),
        record(6, NOW - 40, &["c", "b"], 5),
    ];
    let (selected, windows) = recent_per_scope(records.iter().map(view).collect(), 2);
    assert_eq!(ids(&selected), [4, 5, 6, 2, 3], "ascending (start, id)");
    let shape: Vec<(Vec<String>, u32, u32)> = windows
        .iter()
        .map(|w| {
            (
                w.team_ids.clone(),
                w.requested_sessions,
                w.available_sessions,
            )
        })
        .collect();
    let teams = |list: &[&str]| list.iter().map(|team| (*team).to_string()).collect();
    assert_eq!(
        shape,
        [
            (teams(&["b", "c"]), 2, 2),
            (teams(&["a"]), 2, 2),
            (teams(&["b"]), 2, 1),
        ],
        "one canonical scope per sorted team ids, ordered by attribution"
    );
    assert_eq!(windows[0].team_attribution, TeamAttribution::Ambiguous);
    assert!(windows.iter().all(|window| window.agent_key == "dev"));
}

#[test]
fn boundary_only_sessions_have_no_records_but_still_exist() {
    let mut boundary = record(1, NOW, &["a"], 0);
    boundary.kind = RecordKind::Boundary;
    boundary.boundary_reason = Some(BoundaryReason::ObservedZero);
    let stats = view(&boundary).stats();
    assert_eq!((stats.records_count, stats.mean), (0, None));
    assert_eq!(mean_of(0, 1), Some(0.0));
    assert_eq!(mean_of(21, 2), Some(10.5));
    assert_eq!(mean_of(5, 0), None);
}
