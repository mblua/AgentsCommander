use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_bench_fixture_invariants() {
    for (r, m) in [(3usize, 2usize), (8, 1)] {
        let bench = bench_fixture(r, m);
        let on_disk = bench.configs.iter().filter(|path| path.is_file()).count();
        assert_eq!(on_disk, r * m, "config.json on disk for R={r} M={m}");
        let mut walked = super::CandidateDirs::new();
        for ac_root in &bench.ac_roots {
            super::collect_kind_replica_dirs(ac_root, &mut walked);
        }
        assert_eq!(walked.dirs.len(), r * m, "replicas walked by kind");
        let settings = state_for(bench.settings.clone());
        for (scope, expected) in [
            (super::ProfileAssignmentScope::Kind, r),
            (super::ProfileAssignmentScope::Workgroup, 1),
            (super::ProfileAssignmentScope::Replica, 1),
        ] {
            let removal = api_removal_preview(&settings, &bench.anchor, scope.clone()).await;
            assert_eq!(removal.targets.len(), expected, "{scope:?} R={r} M={m}");
            assert!(removal.counts_complete, "{scope:?} counts complete");
            let assignment = api_preview(
                &settings,
                &bench.anchor,
                scope.clone(),
                super::AssignmentMode::Ordinary,
            )
            .await;
            assert_eq!(assignment.targets.len(), expected, "{scope:?} R={r} M={m}");
            assert!(assignment.counts_complete, "{scope:?} counts complete");
        }
    }
}

/// Strict-reader calls in kind after p4's single read. p4 updated exactly
/// these two expected values: 16 -> 8 and 160 -> 120.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_strict_reads_before_the_single_read_change() {
    for (r, m, expected) in [(8usize, 1usize, 8usize), (40, 3, 120)] {
        let bench = bench_fixture(r, m);
        let settings = state_for(bench.settings.clone());
        strict_read_probe::register(bench.root());
        let removal = api_removal_preview(
            &settings,
            &bench.anchor,
            super::ProfileAssignmentScope::Kind,
        )
        .await;
        let calls = strict_read_probe::unregister(bench.root());
        assert_eq!(removal.targets.len(), r);
        assert_eq!(calls, expected, "strict-reader calls in kind, R={r} M={m}");
    }
}

#[test]
fn issue_2475_bench_counter_ignores_reads_outside_the_fixture() {
    let bench = bench_fixture(1, 1);
    let outside = selection_api_fixture();
    let outside_replica = selection_api_replica(
        &outside,
        "room-1-outside",
        "outside-agent",
        locked_tooling("B", "agent-0"),
    );
    strict_read_probe::register(bench.root());
    crate::config::replica_identity::read_wg_replica_config_read_only(&outside_replica)
        .expect("outside strict read");
    assert_eq!(
        strict_read_probe::count(bench.root()),
        0,
        "a strict read outside the fixture is not counted"
    );
    crate::config::replica_identity::read_wg_replica_config_read_only(&bench.anchor)
        .expect("inside strict read");
    assert_eq!(strict_read_probe::unregister(bench.root()), 1);
}

/// Strict-reader calls of one kind enumeration over `bench_fixture(r, m)`.
async fn kind_strict_reader_calls(r: usize, m: usize) -> usize {
    let bench = bench_fixture(r, m);
    let settings = state_for(bench.settings.clone());
    strict_read_probe::register(bench.root());
    let removal = api_removal_preview(
        &settings,
        &bench.anchor,
        super::ProfileAssignmentScope::Kind,
    )
    .await;
    let calls = strict_read_probe::unregister(bench.root());
    assert_eq!(removal.targets.len(), r, "kind targets, R={r} M={m}");
    calls
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_enumeration_reads_each_included_config_once() {
    assert_eq!(
        kind_strict_reader_calls(8, 1).await,
        8,
        "strict-reader calls in kind, R=8 M=1"
    );
}

/// The 8 Matrix-excluded replicas already cost one strict-reader call:
/// the Kind filter continues before the selection-state read. 16 -> 12,
/// never 24 -> 12.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_2475_excluded_replicas_already_cost_one_read() {
    assert_eq!(
        kind_strict_reader_calls(4, 3).await,
        12,
        "strict-reader calls in kind, R=4 M=3"
    );
}

#[test]
fn issue_2475_enumeration_result_is_byte_identical_after_the_single_read() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "qa",
        unlocked_tooling("A", "agent-1"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "bad-tooling",
        json!({ "selectionLocked": "yes" }),
    );
    let broken = selection_api_replica(&fixture, "room-1-team", "broken", json!({}));
    std::fs::write(broken.join("config.json"), b"{not json").expect("break config");
    let unreadable = selection_api_replica(&fixture, "room-1-team", "unreadable", json!({}));
    std::fs::remove_file(unreadable.join("config.json")).expect("remove config");
    std::fs::create_dir(unreadable.join("config.json")).expect("config.json as a dir");

    let settings = selection_api_settings(&fixture);
    let enumeration = super::enumerate_profile_assignment_targets(
        &settings,
        &anchor,
        &super::ProfileAssignmentScope::Workgroup,
        &[],
    )
    .expect("enumerate");

    let canonical = |path: &Path| super::canonical_real_dir(path, "test").expect("canonical");
    let room = canonical(&fixture.ac_root.join("room-1-team"));
    let wire =
        |path: &Path| crate::path_utils::path_to_string_without_windows_verbatim_prefix(path);
    let target = |name: &str, pair: Value, state: &str, error: Value| {
        json!({
            "workgroupName": "room-1-team",
            "workgroupPath": wire(&room),
            "replicaName": name,
            "replicaPath": wire(&room.join(format!("__agent_{name}"))),
            "identityPath": format!("../../_agent_{name}"),
            "originProject": "project",
            "liveSessionIds": [],
            "savedPair": pair,
            "selectionState": state,
            "selectionError": error,
        })
    };
    // read_dir order is platform-defined, so the warnings compare sorted;
    // the candidates are sorted by the enumeration itself.
    let mut warnings = enumeration.warnings.clone();
    warnings.sort();
    let actual = json!({
        "candidates": enumeration
            .candidates
            .iter()
            .map(|candidate| serde_json::to_value(&candidate.target).expect("serialize"))
            .collect::<Vec<_>>(),
        "warnings": warnings,
        "countsComplete": enumeration.counts_complete,
    });
    let expected = json!({
        "candidates": [
            target(
                "bad-tooling",
                Value::Null,
                "invalid",
                json!("tooling.selectionLocked must be a boolean"),
            ),
            target(
                "dev-rust",
                json!({ "codingAgentId": "agent-0", "requestedProfile": "B" }),
                "locked",
                Value::Null,
            ),
            target(
                "qa",
                json!({ "codingAgentId": "agent-1", "requestedProfile": "A" }),
                "unlocked",
                Value::Null,
            ),
        ],
        "warnings": [
            format!("Skipping invalid replica '{}'", room.join("__agent_broken").display()),
            format!("Skipping invalid replica '{}'", room.join("__agent_unreadable").display()),
        ],
        "countsComplete": false,
    });
    assert_eq!(actual, expected);
}

/// #2557 - the #2475 byte-identity fixture: two invalid replicas
/// (`broken` is not JSON, `unreadable` has config.json as a directory).
fn issue_2557_broken_room() -> (
    SelectionApiFixture,
    super::ProfileTargetEnumeration,
    PathBuf,
) {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "qa",
        unlocked_tooling("A", "agent-1"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "bad-tooling",
        json!({ "selectionLocked": "yes" }),
    );
    let broken = selection_api_replica(&fixture, "room-1-team", "broken", json!({}));
    std::fs::write(broken.join("config.json"), b"{not json").expect("break config");
    let unreadable = selection_api_replica(&fixture, "room-1-team", "unreadable", json!({}));
    std::fs::remove_file(unreadable.join("config.json")).expect("remove config");
    std::fs::create_dir(unreadable.join("config.json")).expect("config.json as a dir");

    let settings = selection_api_settings(&fixture);
    let enumeration = super::enumerate_profile_assignment_targets(
        &settings,
        &anchor,
        &super::ProfileAssignmentScope::Workgroup,
        &[],
    )
    .expect("enumerate");
    let room =
        super::canonical_real_dir(&fixture.ac_root.join("room-1-team"), "test").expect("canonical");
    (fixture, enumeration, room)
}

#[test]
fn issue_2557_scope_faults_name_the_replica_and_the_reason() {
    let (_fixture, enumeration, room) = issue_2557_broken_room();
    let wire =
        |path: &Path| crate::path_utils::path_to_string_without_windows_verbatim_prefix(path);
    let faults = &enumeration.scope_faults;

    assert_eq!(faults.len(), 2, "{faults:?}");
    assert_eq!(
        faults
            .iter()
            .map(|fault| (fault.replica_name.as_str(), fault.code))
            .collect::<Vec<_>>(),
        vec![
            ("broken", super::ScopeFaultCode::ConfigNotJson),
            ("unreadable", super::ScopeFaultCode::ConfigUnreadable),
        ]
    );
    assert_eq!(
        serde_json::to_value(&faults[0]).expect("serialize"),
        json!({
            "code": "configNotJson",
            "replicaName": "broken",
            "replicaPath": wire(&room.join("__agent_broken")),
        })
    );
}

#[test]
fn issue_2557_warnings_stay_byte_identical() {
    let (_fixture, enumeration, _room) = issue_2557_broken_room();
    assert_eq!(enumeration.warnings.len(), 2);
    assert_eq!(
        enumeration.warnings.len(),
        enumeration.scope_faults.len(),
        "every skipped replica yields one warning and one fault"
    );
}

#[test]
fn issue_2557_unreadable_folder_is_named_too() {
    let fixture = selection_api_fixture();
    let missing = fixture.ac_root.join("no-existe");
    type Collector = fn(&Path, &mut super::CandidateDirs);
    let collectors: [(&str, Collector); 2] = [
        ("kind", super::collect_kind_replica_dirs),
        ("workgroup", super::collect_replica_dirs_in_workgroup),
    ];
    for (label, collect) in collectors {
        let mut dirs = super::CandidateDirs::new();
        collect(&missing, &mut dirs);
        assert!(!dirs.complete, "{label}");
        assert_eq!(dirs.faults.len(), 1, "{label}: {:?}", dirs.faults);
        assert_eq!(
            dirs.faults[0].code,
            super::ScopeFaultCode::FolderUnreadable,
            "{label}"
        );
        assert_eq!(dirs.faults[0].replica_name, "no-existe", "{label}");
    }
}

#[test]
fn issue_2557_clean_scope_has_zero_faults() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    selection_api_replica(
        &fixture,
        "room-1-team",
        "qa",
        unlocked_tooling("A", "agent-1"),
    );
    let enumeration = super::enumerate_profile_assignment_targets(
        &selection_api_settings(&fixture),
        &anchor,
        &super::ProfileAssignmentScope::Workgroup,
        &[],
    )
    .expect("enumerate");
    assert!(enumeration.counts_complete);
    assert!(
        enumeration.scope_faults.is_empty(),
        "{:?}",
        enumeration.scope_faults
    );
}

#[test]
fn issue_2475_anchor_read_failure_keeps_its_error_text() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    // The lenient anchor validation accepts a duplicate key; the strict
    // reader rejects it, so the failure is the strict read's own.
    std::fs::write(
        anchor.join("config.json"),
        br#"{"identity":"../../_agent_dev-rust","tooling":{},"tooling":{}}"#,
    )
    .expect("break anchor");
    let settings = selection_api_settings(&fixture);
    let error = match super::enumerate_profile_assignment_targets(
        &settings,
        &anchor,
        &super::ProfileAssignmentScope::Replica,
        &[],
    ) {
        Ok(_) => panic!("a strictly unparseable anchor must fail the enumeration"),
        Err(error) => error,
    };
    assert!(error.contains("Target replica '"), "{error}");
}

#[test]
fn issue_2475_invalid_candidate_still_clears_counts_complete() {
    let fixture = selection_api_fixture();
    let anchor = selection_api_replica(
        &fixture,
        "room-1-team",
        "dev-rust",
        locked_tooling("B", "agent-0"),
    );
    let broken = selection_api_replica(&fixture, "room-1-team", "broken", json!({}));
    std::fs::write(broken.join("config.json"), b"{not json").expect("break config");
    let settings = selection_api_settings(&fixture);
    let enumeration = super::enumerate_profile_assignment_targets(
        &settings,
        &anchor,
        &super::ProfileAssignmentScope::Workgroup,
        &[],
    )
    .expect("enumerate");
    assert_eq!(enumeration.candidates.len(), 1);
    assert!(
        !enumeration.counts_complete,
        "an invalid replica clears counts_complete"
    );
}
