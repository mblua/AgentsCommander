use super::*;

// -----------------------------------------------------------------------
// #2786 (C1) - the config pair primitive. The state path is a literal and
// the stage hook is test-local: this module must name no other module.
// -----------------------------------------------------------------------

const PAIR_STATE_NAME: &str = "config.state.no-git.json";
const PAIR_STATE_KEYS: [&str; 4] = [
    "lastCodingAgent",
    "codingAgents",
    "lastAgentMessageAt",
    "profileContentHash",
];

fn pair_fixture(
    decisions: Option<&str>,
    state: Option<&str>,
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let decisions_path = temp.path().join("config.json");
    let state_path = temp.path().join(PAIR_STATE_NAME);
    if let Some(body) = decisions {
        std::fs::write(&decisions_path, body).expect("seed decisions");
    }
    if let Some(body) = state {
        std::fs::write(&state_path, body).expect("seed state");
    }
    (temp, decisions_path, state_path)
}

fn no_stage(_stage: &str) {}

fn read_value(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read json")).expect("parse json")
}

fn split_marker() -> Value {
    json!({"v": 1, "keys": PAIR_STATE_KEYS})
}

#[test]
fn the_state_file_is_written_before_the_decisions_file() {
    let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), None);
    let debris = temp_config_path(&decisions);
    std::fs::create_dir(&debris).unwrap();
    let stages = std::cell::RefCell::new(Vec::new());
    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &|stage| stages.borrow_mut().push(stage.to_string()),
        |d, s| {
            d.insert("a".to_string(), json!(2));
            s.insert("tooling".to_string(), json!({"lastCodingAgent": "claude"}));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(read_value(&state)["tooling"]["lastCodingAgent"], "claude");
    assert_eq!(read_value(&decisions)["a"], 2);
    assert_eq!(
        stages.into_inner(),
        vec!["after_caller_state_publish", "after_caller_decisions_write"]
    );
    assert!(debris.is_dir());
}

#[test]
fn an_absent_state_file_is_an_empty_tooling() {
    let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), None);
    let saw_empty = std::cell::Cell::new(false);

    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |_d, s| {
            saw_empty.set(s.is_empty());
            Ok(())
        },
    )
    .expect("an absent state file is a valid state");

    assert!(saw_empty.get(), "the closure must see an empty state map");
    assert!(
        !state.exists(),
        "an untouched absent state file stays absent"
    );
}

#[test]
fn an_untouched_side_is_not_written() {
    // Decisions only: no state file appears.
    let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), None);
    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |d, _s| {
            d.insert("a".to_string(), json!(2));
            Ok(())
        },
    )
    .expect("decisions-only pair call");
    assert_eq!(read_value(&decisions)["a"], 2);
    assert!(
        !state.exists(),
        "a decisions-only call must not create a state file"
    );

    // State only: the decisions file stays byte-identical.
    let original = "{\"a\":1,  \"b\" : [1,2]}";
    let (_temp, decisions, state) = pair_fixture(Some(original), None);
    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |_d, s| {
            s.insert(
                "tooling".to_string(),
                json!({"lastAgentMessageAt": "2026-09-29T00:00:00Z"}),
            );
            Ok(())
        },
    )
    .expect("state-only pair call");
    assert_eq!(std::fs::read_to_string(&decisions).unwrap(), original);
    assert_eq!(
        read_value(&state)["tooling"]["lastAgentMessageAt"],
        "2026-09-29T00:00:00Z"
    );
}

#[test]
fn an_unparseable_state_file_blocks_the_pair() {
    let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), Some("{ not json"));
    let ran = std::cell::Cell::new(false);

    let err = update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |d, s| {
            ran.set(true);
            d.insert("a".to_string(), json!(2));
            s.insert("tooling".to_string(), json!({}));
            Ok(())
        },
    )
    .expect_err("an unparseable state file must block the pair");

    assert!(err.contains(&decisions.display().to_string()), "{err}");
    assert!(err.contains(&state.display().to_string()), "{err}");
    assert!(!ran.get(), "mutate must not run");
    assert_eq!(std::fs::read_to_string(&decisions).unwrap(), r#"{"a":1}"#);
    assert_eq!(std::fs::read_to_string(&state).unwrap(), "{ not json");
}

#[test]
fn the_pair_preserves_unknown_keys_on_both_sides() {
    let decisions_seed = json!({"unknownDecision": {"x": [1, 2]}, "tooling": {"telegramBot": "b"}});
    let state_seed = json!({"unknownState": "keep", "split": split_marker(), "tooling": {}});
    let (_temp, decisions, state) = pair_fixture(
        Some(&decisions_seed.to_string()),
        Some(&state_seed.to_string()),
    );

    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |d, s| {
            d.insert("context".to_string(), json!(["Role.md"]));
            s.insert("tooling".to_string(), json!({"lastCodingAgent": "claude"}));
            Ok(())
        },
    )
    .expect("pair call");

    let saved_decisions = read_value(&decisions);
    let saved_state = read_value(&state);
    assert_eq!(
        saved_decisions["unknownDecision"],
        decisions_seed["unknownDecision"]
    );
    assert_eq!(saved_decisions["tooling"], decisions_seed["tooling"]);
    assert_eq!(saved_state["unknownState"], state_seed["unknownState"]);
    assert_eq!(saved_state["split"], split_marker());
    assert_eq!(saved_decisions["context"][0], "Role.md");
    assert_eq!(saved_state["tooling"]["lastCodingAgent"], "claude");
}

#[test]
fn the_guard_is_not_released_between_the_two_sequences() {
    let (_temp, decisions, state) = pair_fixture(
        Some(r#"{"tooling":{"lastCodingAgent":"claude","telegramBot":"b"}}"#),
        None,
    );
    let (paused_tx, paused_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();

    let worker = {
        let decisions = decisions.clone();
        let state = state.clone();
        std::thread::spawn(move || {
            let on_stage = |stage: &str| {
                if stage == "after_cleanup_tracked_write" {
                    paused_tx.send(()).expect("announce pause");
                    release_rx
                        .recv_timeout(Duration::from_secs(60))
                        .expect("release the pause");
                }
            };
            let cleanup = |d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
                let moved = d
                    .get_mut("tooling")
                    .and_then(Value::as_object_mut)
                    .and_then(|tooling| tooling.remove("lastCodingAgent"))
                    .ok_or_else(|| "no key to move".to_string())?;
                s.insert("tooling".to_string(), json!({"lastCodingAgent": moved}));
                s.insert("split".to_string(), split_marker());
                Ok(())
            };
            update_config_pair(
                &decisions,
                &state,
                &PAIR_STATE_KEYS,
                Some(&cleanup),
                &on_stage,
                |_d, _s| Ok(()),
            )
        })
    };

    paused_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("the call must reach after_cleanup_tracked_write");
    let blocked = acquire_config_file_write_lock(&decisions, Duration::from_millis(300));
    let blocked_err = blocked
        .as_ref()
        .err()
        .cloned()
        .unwrap_or_else(|| "acquired".to_string());
    drop(blocked);
    release_tx.send(()).expect("release");
    worker.join().expect("worker").expect("pair call");

    assert!(
        blocked_err.contains("configLockTimeout"),
        "a second acquirer must fail while the pause holds, got: {blocked_err}"
    );
    drop(
        acquire_config_file_write_lock(&decisions, Duration::from_millis(500))
            .expect("the sidecar must be free once the call returns"),
    );
    let saved_state = read_value(&state);
    assert_eq!(saved_state["tooling"]["lastCodingAgent"], "claude");
    assert_eq!(saved_state["split"], split_marker());
    let saved_decisions = read_value(&decisions);
    assert!(saved_decisions["tooling"].get("lastCodingAgent").is_none());
    assert_eq!(saved_decisions["tooling"]["telegramBot"], "b");
}

#[test]
fn the_cleanup_sequence_is_ordered_and_aborts_whole() {
    let decisions_seed = r#"{"tooling":{"lastCodingAgent":"claude"}}"#;
    let state_seed = r#"{"tooling":{"lastAgentMessageAt":"x"}}"#;

    // Leg 1: a failing cleanup writes nothing and mutate never runs.
    let (_temp, decisions, state) = pair_fixture(Some(decisions_seed), Some(state_seed));
    let ran = std::cell::Cell::new(false);
    let failing = |_d: &mut Map<String, Value>, _s: &mut Map<String, Value>| {
        Err("policy refused".to_string())
    };
    let err = update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        Some(&failing),
        &no_stage,
        |d, s| {
            ran.set(true);
            d.insert("a".to_string(), json!(1));
            s.insert("a".to_string(), json!(1));
            Ok(())
        },
    )
    .expect_err("a failing cleanup aborts the call");
    assert!(!ran.get(), "mutate must never run after a failed cleanup");
    assert!(err.contains("policy refused"), "{err}");
    assert!(err.contains(&decisions.display().to_string()), "{err}");
    assert!(err.contains(&state.display().to_string()), "{err}");
    assert_eq!(std::fs::read_to_string(&decisions).unwrap(), decisions_seed);
    assert_eq!(std::fs::read_to_string(&state).unwrap(), state_seed);

    // Leg 2: a state-only cleanup, writing the marker, fires only its
    // state stage and leaves the decisions file byte-identical.
    let (_temp, decisions, state) = pair_fixture(Some(decisions_seed), None);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let on_stage = |stage: &str| tx.send(stage.to_string()).expect("record stage");
    let state_only = |_d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
        s.insert("split".to_string(), split_marker());
        Ok(())
    };
    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        Some(&state_only),
        &on_stage,
        |_d, _s| Ok(()),
    )
    .expect("state-only cleanup");
    assert_eq!(
        rx.recv_timeout(Duration::from_millis(200)).as_deref(),
        Ok("after_cleanup_state_publish")
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "after_cleanup_tracked_write must not fire for a state-only cleanup"
    );
    assert_eq!(std::fs::read_to_string(&decisions).unwrap(), decisions_seed);
    assert_eq!(read_value(&state)["split"], split_marker());

    // Leg 3: no cleanup behaves as a single-sequence call.
    let (_temp, decisions, state) = pair_fixture(Some(decisions_seed), None);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let on_stage = |stage: &str| tx.send(stage.to_string()).expect("record stage");
    update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &on_stage,
        |d, _s| {
            d.insert("context".to_string(), json!([]));
            Ok(())
        },
    )
    .expect("single-sequence call");
    drop(tx);
    let stages: Vec<String> = rx.try_iter().collect();
    assert_eq!(stages, vec!["after_caller_decisions_write".to_string()]);
    assert!(
        !state.exists(),
        "a decisions-only mutate must not create a state file"
    );
}

fn prepared_fixture_plan() -> PreparedConfigPairPlan {
    let before = [
        PhysicalState::from_bytes(br#"{"d":0,"unknown":[1,2]}"#.to_vec()),
        PhysicalState::from_bytes(br#"{"s":0,"unknown":null}"#.to_vec()),
    ];
    let cleanup = |d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
        d.insert("d".into(), json!(1));
        s.insert("s".into(), json!(1));
        Ok(())
    };
    prepare_config_pair_plan(before, Some(&cleanup), |d, s| {
        d.insert("d".into(), json!(2));
        s.insert("s".into(), json!(2));
        Ok(())
    })
    .unwrap()
}

fn prepared_install(path: &Path, image: &PhysicalState) {
    match image {
        PhysicalState::Absent => {
            if path.exists() {
                std::fs::remove_file(path).unwrap();
            }
        }
        PhysicalState::Bytes { bytes, .. } => std::fs::write(path, bytes).unwrap(),
    }
}

#[test]
fn prepared_all_25_side_combinations_reachable_or_conflict() {
    let plan = prepared_fixture_plan();
    let d_steps = [0, 0, 1, 1, 2];
    let s_steps = [0, 1, 1, 2, 2];
    let mut reached = 0;
    let mut conflicts = 0;
    for (i, &d_step) in d_steps.iter().enumerate() {
        for (j, &s_step) in s_steps.iter().enumerate() {
            let (_temp, d, s) = pair_fixture(None, None);
            prepared_install(&d, &plan.stages[i][0]);
            prepared_install(&s, &plan.stages[j][1]);
            let before = [std::fs::read(&d).unwrap(), std::fs::read(&s).unwrap()];
            let reachable = s_step == d_step || s_step == d_step + 1;
            let result = execute_prepared_config_pair(&d, &s, &plan);
            if reachable {
                result.unwrap();
                reached += 1;
                assert_eq!(
                    [
                        read_config_pair_physical(&d).unwrap(),
                        read_config_pair_physical(&s).unwrap()
                    ],
                    plan.stages[4]
                );
                assert_eq!(read_value(&d)["unknown"], json!([1, 2]));
                assert_eq!(read_value(&s)["unknown"], Value::Null);
            } else {
                assert_eq!(result, Err(PreparedPairError::Conflict));
                conflicts += 1;
                assert_eq!(
                    [std::fs::read(&d).unwrap(), std::fs::read(&s).unwrap()],
                    before
                );
            }
        }
    }
    assert_eq!((reached, conflicts), (16, 9));
}

#[test]
fn prepared_each_publish_cut_original_plan_resume_idempotent() {
    for cut in 1..=4 {
        let plan = prepared_fixture_plan();
        let (_temp, d, s) = pair_fixture(None, None);
        prepared_install(&d, &plan.stages[0][0]);
        prepared_install(&s, &plan.stages[0][1]);
        let stages = std::cell::RefCell::new(Vec::new());
        let result = execute_prepared_config_pair_with_stage(&d, &s, &plan, &|stage| {
            stages.borrow_mut().push(stage);
            assert!(write_file_atomic(&d, b"{}")
                .unwrap_err()
                .contains("Nested config write"));
            // Both sidecars remain held across cleanup and caller images.
            for path in [&d, &s] {
                assert!(acquire_config_file_write_lock(path, Duration::ZERO)
                    .unwrap_err()
                    .contains("configLockTimeout"));
            }
            if stage == cut {
                Err(PreparedPairError::Io("lost progress".into()))
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err(PreparedPairError::Io("lost progress".into())));
        assert_eq!(*stages.borrow(), (1..=cut).collect::<Vec<_>>());
        assert_eq!(
            [
                read_config_pair_physical(&d).unwrap(),
                read_config_pair_physical(&s).unwrap()
            ],
            plan.stages[cut]
        );
        let restored: PreparedConfigPairPlan =
            serde_json::from_slice(&serde_json::to_vec(&plan).unwrap()).unwrap();
        execute_prepared_config_pair(&d, &s, &restored).unwrap();
        execute_prepared_config_pair_with_stage(&d, &s, &restored, &|_| {
            panic!("complete replay publishes nothing")
        })
        .unwrap();
        assert_eq!(
            [
                read_config_pair_physical(&d).unwrap(),
                read_config_pair_physical(&s).unwrap()
            ],
            plan.stages[4]
        );
    }
}

#[test]
fn prepared_initial_absence_collapsed_stages_and_empty_object_distinct() {
    let (_temp, d, s) = pair_fixture(None, None);
    let plan = prepare_config_pair_plan(
        [PhysicalState::Absent, PhysicalState::Absent],
        None,
        |_, _| Ok(()),
    )
    .unwrap();
    execute_prepared_config_pair_with_stage(&d, &s, &plan, &|_| panic!("no target writes"))
        .unwrap();
    assert!(!d.exists() && !s.exists());
    std::fs::write(&s, b"{}").unwrap();
    assert_eq!(
        execute_prepared_config_pair(&d, &s, &plan),
        Err(PreparedPairError::Conflict)
    );
    let create = prepare_config_pair_plan(
        [
            PhysicalState::Absent,
            PhysicalState::from_bytes(b"{}".to_vec()),
        ],
        None,
        |d, _| {
            d.insert("owned".into(), json!(true));
            Ok(())
        },
    )
    .unwrap();
    execute_prepared_config_pair(&d, &s, &create).unwrap();
    assert_eq!(std::fs::read(&s).unwrap(), b"{}");
    assert_eq!(read_value(&d)["owned"], true);
}

#[test]
fn prepared_external_bytes_disappearance_and_mid_stage_drift_no_clobber() {
    for external in [Some(b"{broken".as_slice()), Some(b"{}".as_slice()), None] {
        let plan = prepared_fixture_plan();
        let (_temp, d, s) = pair_fixture(None, None);
        prepared_install(&d, &plan.stages[0][0]);
        prepared_install(&s, &plan.stages[0][1]);
        if let Some(bytes) = external {
            std::fs::write(&s, bytes).unwrap();
        } else {
            std::fs::remove_file(&s).unwrap();
        }
        assert_eq!(
            execute_prepared_config_pair(&d, &s, &plan),
            Err(PreparedPairError::Conflict)
        );
        assert_eq!(read_config_pair_physical(&d).unwrap(), plan.stages[0][0]);
        assert_eq!(
            read_config_pair_physical(&s).unwrap(),
            external
                .map(|b| PhysicalState::from_bytes(b.to_vec()))
                .unwrap_or(PhysicalState::Absent)
        );
    }
    let plan = prepared_fixture_plan();
    let (_temp, d, s) = pair_fixture(None, None);
    prepared_install(&d, &plan.stages[0][0]);
    prepared_install(&s, &plan.stages[0][1]);
    let result = execute_prepared_config_pair_with_stage(&d, &s, &plan, &|stage| {
        assert_eq!(stage, 1);
        std::fs::write(&d, b"{\"external\":true}").unwrap();
        Ok(())
    });
    assert_eq!(result, Err(PreparedPairError::Conflict));
    assert_eq!(std::fs::read(&d).unwrap(), b"{\"external\":true}");
    assert_eq!(read_config_pair_physical(&s).unwrap(), plan.stages[1][1]);
}

#[test]
fn prepared_rejects_invalid_plan_and_nested_writer_before_target_effects() {
    let (_temp, d, s) = pair_fixture(None, None);
    let nested = prepare_config_pair_plan(
        [PhysicalState::Absent, PhysicalState::Absent],
        None,
        |_, _| write_file_atomic(&d, b"{}"),
    );
    assert!(nested
        .unwrap_err()
        .to_string()
        .contains("Nested config write"));
    assert!(!d.exists() && !s.exists());
    let rejected = prepare_config_pair_plan(
        [PhysicalState::Absent, PhysicalState::Absent],
        None,
        |d, _| {
            d.insert("owned".into(), json!(1));
            Err("rejected".into())
        },
    );
    assert!(matches!(rejected, Err(PreparedPairError::Preparation(_))));
    assert!(!d.exists());
    for malformed in [b"null".as_slice(), b"[]".as_slice(), b"{broken".as_slice()] {
        assert!(prepare_config_pair_plan(
            [
                PhysicalState::from_bytes(malformed.to_vec()),
                PhysicalState::Absent
            ],
            None,
            |_, _| Ok(())
        )
        .is_err());
    }
    let mut plan = prepared_fixture_plan();
    if let PhysicalState::Bytes { sha256, .. } = &mut plan.stages[0][0] {
        *sha256 = "forged".into();
    }
    assert!(matches!(
        execute_prepared_config_pair(&d, &s, &plan),
        Err(PreparedPairError::InvalidPlan(_))
    ));
    let mut plan = prepared_fixture_plan();
    plan.stages[1][0] = plan.stages[4][0].clone();
    assert!(matches!(
        execute_prepared_config_pair(&d, &s, &plan),
        Err(PreparedPairError::InvalidPlan(_))
    ));
    assert!(!d.exists() && !s.exists());
    assert!(!lock_sidecar_path(&d).exists());
}

#[test]
fn prepared_occupied_legacy_stage_does_not_block_and_releases_locks() {
    // Occupied legacy staging is debris, no longer an injected IO failure.
    let plan = prepared_fixture_plan();
    let (_temp, d, s) = pair_fixture(None, None);
    prepared_install(&d, &plan.stages[0][0]);
    prepared_install(&s, &plan.stages[0][1]);
    let debris = temp_config_path(&d);
    std::fs::create_dir(&debris).unwrap();
    execute_prepared_config_pair(&d, &s, &plan).unwrap();
    assert_eq!(
        [
            read_config_pair_physical(&d).unwrap(),
            read_config_pair_physical(&s).unwrap()
        ],
        plan.stages[4]
    );
    assert!(debris.is_dir());
    for path in [&d, &s] {
        drop(acquire_config_file_write_lock(path, Duration::ZERO).unwrap());
    }
}

#[test]
fn prepared_expected_absent_publication_preserves_other_creator() {
    let (_temp, d, _s) = pair_fixture(Some("{\"other\":1}"), None);
    let after = PhysicalState::from_bytes(b"{\"owned\":1}\n".to_vec());
    assert_eq!(
        publish_prepared_pair_image(&d, &PhysicalState::Absent, &after),
        Err(PreparedPairError::Conflict)
    );
    assert_eq!(std::fs::read(&d).unwrap(), b"{\"other\":1}");
    assert!(std::fs::read_dir(d.parent().unwrap())
        .unwrap()
        .all(|entry| entry
            .unwrap()
            .path()
            .extension()
            .is_none_or(|extension| extension != "tmp")));
}

#[test]
fn prepared_sidecar_timeout_and_process_death_release() {
    // Reuse the exact lock's bounded existing child-process coverage.
    issue_1937_config_lock_process_death_release();
    issue_1937_config_lock_same_process_handles();
}

const NESTED_CHILD_ACTION_ENV: &str = "AC_2786_NESTED_WRITER_CHILD_ACTION";
const NESTED_CHILD_DIR_ENV: &str = "AC_2786_NESTED_WRITER_CHILD_DIR";
const NESTED_CHILD_TEST_FQN: &str =
    "config::local_config_io::tests::pair::a_nested_config_writer_child";

/// #2786 (C1) - the child half of E2c. A no-op without the child-only
/// environment, so a normal suite run passes it untouched.
#[test]
fn a_nested_config_writer_child() {
    let Some(action) = std::env::var_os(NESTED_CHILD_ACTION_ENV) else {
        return;
    };
    let action = action.to_string_lossy().into_owned();
    let dir = PathBuf::from(std::env::var_os(NESTED_CHILD_DIR_ENV).expect("child dir env"));
    let decisions = dir.join("config.json");
    let state = dir.join(PAIR_STATE_NAME);
    let other = dir.join("other.json");

    if action == "panic" {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            update_config_pair(
                &decisions,
                &state,
                &PAIR_STATE_KEYS,
                None,
                &no_stage,
                |_d, _s| panic!("forced panic inside mutate"),
            )
        }));
        assert!(
            outcome.is_err(),
            "the mutate panic must reach the test boundary"
        );
        update_config_json_object(&decisions, true, |obj| {
            obj.insert("sameThread".to_string(), json!(true));
            Ok(())
        })
        .expect("a normal write on the same thread must succeed after a panic");
        let other_thread = other.clone();
        std::thread::spawn(move || write_file_atomic(&other_thread, b"{}\n"))
            .join()
            .expect("writer thread")
            .expect("a normal write on another thread must succeed after a panic");
        println!("AC_2786_NESTED_WRITER_CHILD_OK action={action}");
        return;
    }

    let entry = match action.as_str() {
        "nest_pair" => "update_config_pair",
        "nest_json" => "update_config_json_object",
        "nest_atomic" => "write_file_atomic",
        other => panic!("unknown child action {other}"),
    };
    let err = update_config_pair(
        &decisions,
        &state,
        &PAIR_STATE_KEYS,
        None,
        &no_stage,
        |_d, _s| match entry {
            "update_config_pair" => update_config_pair(
                &other,
                &state,
                &PAIR_STATE_KEYS,
                None,
                &no_stage,
                |_d, _s| Ok(()),
            ),
            "update_config_json_object" => {
                update_config_json_object(&other, true, |_obj| Ok(())).map(|_| ())
            }
            _ => write_file_atomic(&other, b"{}\n"),
        },
    )
    .expect_err("a nested config writer must return an error");
    assert!(
        err.contains(&format!("Nested config write: {entry}")),
        "the error must name the nesting: {err}"
    );
    update_config_json_object(&decisions, true, |obj| {
        obj.insert("afterNesting".to_string(), json!(true));
        Ok(())
    })
    .expect("the thread must stay writable after the nested error");
    println!("AC_2786_NESTED_WRITER_CHILD_OK action={action}");
}

/// #2786 (C1) E2c - every leg runs in a child process with a lifetime bound,
/// because a hang cannot be asserted in-process and a poisoned `Mutex`
/// must not leak into the rest of this suite.
#[test]
fn a_nested_config_writer_is_an_error_not_a_hang() {
    // One child at a time: `wait_bounded` kills and reaps the child it
    // waits on, so a timeout never leaves another child running.
    for action in ["nest_pair", "nest_json", "nest_atomic", "panic"] {
        let temp = tempfile::tempdir().expect("tempdir");
        let child = Command::new(std::env::current_exe().expect("current test exe"))
            .args([
                "--exact",
                NESTED_CHILD_TEST_FQN,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(NESTED_CHILD_ACTION_ENV, action)
            .env(NESTED_CHILD_DIR_ENV, temp.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn nested writer child");
        let label = format!("nested writer child {action}");
        let (status, stdout, stderr) = wait_bounded(child, Duration::from_secs(30), &label);
        let report = format!(
            "status={status:?}
stdout:
{stdout}
stderr:
{stderr}"
        );
        assert!(status.success(), "{label} failed: {report}");
        assert!(
            stdout.contains("test result: ok. 1 passed; 0 failed"),
            "{label} did not run exactly one passing test: {report}"
        );
        assert!(
            stdout.contains(&format!("AC_2786_NESTED_WRITER_CHILD_OK action={action}")),
            "{label} is missing its marker: {report}"
        );
    }
}
