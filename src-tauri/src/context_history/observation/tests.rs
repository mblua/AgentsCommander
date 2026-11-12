use super::*;

fn scope() -> HistoryScope {
    let teams = [TeamMembers {
        team_id: "team-a".to_string(),
        member_paths: vec!["/p/proj/.ac/_agent_dev".to_string()],
    }];
    match resolve_history_scope(&ScopeInput {
        working_directory: "/p/proj/.ac/room-1-team-a/__agent_dev",
        is_root_agent: false,
        authoritative_ac_root: true,
        teams: &teams,
    }) {
        ScopeResolution::Scoped(scope) => scope,
        other @ ScopeResolution::Excluded(_) => panic!("expected scope, got {other:?}"),
    }
}

fn context() -> InstanceContext {
    InstanceContext {
        app_run_id: Uuid::new_v4(),
        runtime_session_id: Uuid::new_v4(),
        scope: scope(),
        launch: LaunchMetadata::UNAVAILABLE,
    }
}

struct Feed {
    context: InstanceContext,
    state: InstanceState,
    sequence: u64,
    records: Vec<HistoryRecord>,
}

impl Feed {
    fn new() -> Self {
        Self {
            context: context(),
            state: InstanceState::default(),
            sequence: 0,
            records: Vec::new(),
        }
    }

    fn stamp(&mut self) -> Stamp {
        self.sequence += 1;
        Stamp {
            observed_at_utc_ms: 1_000 * i64::try_from(self.sequence).unwrap(),
            sequence: self.sequence,
        }
    }

    fn push(&mut self, value: ObservedValue) {
        let stamp = self.stamp();
        let produced = self
            .state
            .observe(&self.context, value, stamp, &mut Uuid::new_v4);
        self.records.extend(produced);
    }

    fn readings(&mut self, values: &[u8]) {
        for value in values {
            self.push(ObservedValue::Reading(*value));
        }
    }

    fn gap(&mut self) {
        let stamp = self.stamp();
        let produced = self
            .state
            .observe_gap(&self.context, stamp, &mut Uuid::new_v4);
        self.records.extend(produced);
    }

    fn changes(&self) -> Vec<&HistoryRecord> {
        self.records
            .iter()
            .filter(|record| record.kind == RecordKind::Change)
            .collect()
    }
}

#[test]
fn changes_only_including_drops() {
    let mut feed = Feed::new();
    feed.readings(&[20, 20, 30, 15]);
    let percents: Vec<_> = feed.changes().iter().map(|r| r.percent).collect();
    assert_eq!(percents, [Some(20), Some(30), Some(15)]);
    let first = &feed.records[0];
    assert_eq!(first.initial_kind, InitialKind::FirstObserved);
    assert_eq!(first.initial_record_id, Some(first.record_id));
    assert!(feed.records.iter().all(|r| r.validate().is_ok()));
}

#[test]
fn zero_splits_sessions_and_confirms_the_next_start() {
    let mut feed = Feed::new();
    feed.readings(&[0, 0, 10, 10, 20, 0, 5]);
    let changes = feed.changes();
    assert_eq!(changes.len(), 3);
    assert_eq!(
        changes[0].initial_kind,
        InitialKind::ConfirmedZeroTransition
    );
    assert_eq!(changes[0].logical_session_id, changes[1].logical_session_id);
    assert_ne!(changes[1].logical_session_id, changes[2].logical_session_id);
    assert_eq!((changes[0].ordinal, changes[2].ordinal), (1, 2));
    let boundaries: Vec<_> = feed
        .records
        .iter()
        .filter(|r| r.kind == RecordKind::Boundary)
        .collect();
    assert_eq!(
        boundaries.len(),
        1,
        "a zero with no open session writes nothing"
    );
    assert_eq!(
        boundaries[0].boundary_reason,
        Some(BoundaryReason::ObservedZero)
    );
}

#[test]
fn unavailable_is_never_zero_and_does_not_duplicate() {
    let mut feed = Feed::new();
    feed.readings(&[40]);
    feed.push(ObservedValue::Unavailable);
    feed.readings(&[40, 41]);
    assert_eq!(feed.records.len(), 2);
    assert!(feed.records.iter().all(|r| r.percent != Some(0)));
}

#[test]
fn unavailable_breaks_zero_consecutiveness() {
    let mut feed = Feed::new();
    feed.readings(&[0]);
    feed.push(ObservedValue::Unavailable);
    feed.readings(&[7]);
    assert_eq!(feed.records[0].initial_kind, InitialKind::FirstObserved);
}

#[test]
fn session_over_closes_once_and_retires() {
    let mut feed = Feed::new();
    feed.readings(&[30]);
    feed.push(ObservedValue::SessionOver);
    feed.readings(&[50]);
    assert_eq!(feed.records.len(), 2);
    assert_eq!(
        feed.records[1].boundary_reason,
        Some(BoundaryReason::Lifecycle)
    );
    assert!(feed.state.is_retired());

    let mut closed = Feed::new();
    closed.readings(&[30, 0]);
    closed.push(ObservedValue::SessionOver);
    assert_eq!(
        closed.records.len(),
        2,
        "zero already closed; no second close"
    );
}

#[test]
fn gap_keeps_session_and_marks_partial_coverage() {
    let mut feed = Feed::new();
    feed.readings(&[30]);
    feed.gap();
    feed.gap();
    feed.readings(&[30, 31]);
    assert_eq!(feed.records.len(), 3, "one gap boundary, no duplicate 30");
    let gap = &feed.records[1];
    assert_eq!(gap.boundary_reason, Some(BoundaryReason::ObservationGap));
    assert_eq!(gap.logical_session_id, feed.records[0].logical_session_id);
    assert!(!feed.records[2].continuity_known);
    assert_eq!(
        feed.records[2].logical_started_at_utc_ms,
        feed.records[0].logical_started_at_utc_ms
    );
}

#[test]
fn gap_without_open_session_loses_the_start() {
    let mut feed = Feed::new();
    feed.readings(&[30, 0]);
    feed.gap();
    feed.readings(&[12]);
    let last = feed.records.last().unwrap();
    assert_eq!(last.initial_kind, InitialKind::Unknown);
    assert_eq!(last.logical_started_at_utc_ms, None);
    assert!(last.partial_start);
    assert!(last.validate().is_ok());
}

#[test]
fn out_of_order_sequence_is_rejected() {
    let mut feed = Feed::new();
    let late = Stamp {
        observed_at_utc_ms: 5,
        sequence: 9,
    };
    let early = Stamp {
        observed_at_utc_ms: 9_999,
        sequence: 3,
    };
    let ctx = feed.context.clone();
    assert!(feed
        .state
        .observe(&ctx, ObservedValue::Reading(10), late, &mut Uuid::new_v4)
        .is_some());
    assert!(feed
        .state
        .observe(&ctx, ObservedValue::Reading(20), early, &mut Uuid::new_v4)
        .is_none());
}

#[test]
fn layouts_and_exclusions() {
    let replica = parse_history_layout("/p/proj/.ac/room-1-x/__agent_dev").unwrap();
    assert_eq!(replica.agent_key, "dev");
    assert_eq!(replica.room.as_deref(), Some("room-1-x"));
    assert_eq!(replica.project_name, "proj");
    let origin = parse_history_layout("/p/proj/.ac/_agent_dev").unwrap();
    assert_eq!(origin.matrix_type, MatrixType::Origin);
    assert!(parse_history_layout("/p/proj/src").is_err());
    assert!(parse_history_layout("/p/proj/notac/room-1-x/__agent_dev").is_err());
    let root = ScopeInput {
        working_directory: "/p/proj/.ac/_agent_dev",
        is_root_agent: true,
        authoritative_ac_root: true,
        teams: &[],
    };
    assert_eq!(
        resolve_history_scope(&root),
        ScopeResolution::Excluded("root_agent")
    );
    let foreign = ScopeInput {
        is_root_agent: false,
        authoritative_ac_root: false,
        ..root
    };
    assert!(matches!(
        resolve_history_scope(&foreign),
        ScopeResolution::Excluded(_)
    ));
}

#[test]
fn team_attribution_counts_exact_members() {
    let member = |team: &str, path: &str| TeamMembers {
        team_id: team.to_string(),
        member_paths: vec![path.to_string()],
    };
    let resolve = |teams: &[TeamMembers]| match resolve_history_scope(&ScopeInput {
        working_directory: "/p/proj/.ac/room-1-x/__agent_dev",
        is_root_agent: false,
        authoritative_ac_root: true,
        teams,
    }) {
        ScopeResolution::Scoped(scope) => (scope.team_attribution, scope.team_ids),
        other @ ScopeResolution::Excluded(_) => panic!("{other:?}"),
    };
    assert_eq!(resolve(&[]).0, TeamAttribution::Unknown);
    let one = [member("a", "/p/proj/.ac/_agent_dev/")];
    assert_eq!(
        resolve(&one),
        (TeamAttribution::Known, vec!["a".to_string()])
    );
    let two = [
        member("b", "/p/proj/.ac/_agent_dev"),
        member("a", "/p/proj/.ac/_agent_dev"),
    ];
    assert_eq!(resolve(&two).0, TeamAttribution::Ambiguous);
    let suffix = [member("a", "/p/proj/.ac/_agent_dev-2")];
    assert_eq!(
        resolve(&suffix).0,
        TeamAttribution::Unknown,
        "no suffix matching"
    );
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn launch_metadata_per_provider() {
    let claude = parse_launch_metadata(
        "claude",
        Some(&args(&[
            "--model",
            "opus",
            "--effort=high",
            "--model=sonnet",
        ])),
    );
    assert_eq!(claude.coding_agent_id.as_deref(), Some("claude"));
    assert_eq!(claude.model.as_deref(), Some("sonnet"));
    assert_eq!(claude.effort.as_deref(), Some("high"));
    assert_eq!(claude.source, MetadataSource::LaunchCommand);

    let codex = parse_launch_metadata(
        "C:/bin/codex.exe",
        Some(&args(&[
            "-m",
            "gpt-x",
            "-c",
            "model_reasoning_effort=\"low\"",
        ])),
    );
    assert_eq!(codex.model.as_deref(), Some("gpt-x"));
    assert_eq!(codex.effort.as_deref(), Some("low"));

    let pi = parse_launch_metadata("pi", Some(&args(&["--thinking", "medium", "--model"])));
    assert_eq!(pi.effort.as_deref(), Some("medium"));
    assert_eq!(pi.model, None, "missing value is null");

    assert_eq!(
        parse_launch_metadata("cmd", Some(&args(&["/c", "claude", "--model", "x"]))),
        LaunchMetadata::UNAVAILABLE
    );
    assert_eq!(
        parse_launch_metadata("claude", None),
        LaunchMetadata::UNAVAILABLE
    );
}
