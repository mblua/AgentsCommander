use super::*;

// P22: deterministic failure at each reached validation boundary.
fn p22_prepare(
    fixture: &Fixture,
    request: &PairReservationRequest,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<PairReservation, PreparedPairError> {
    let cleanup = |decisions: &mut Map<String, Value>, state: &mut Map<String, Value>| {
        decisions.insert("cleanup".into(), json!(1));
        state.insert("cleanup".into(), json!(1));
        Ok(())
    };
    prepare_and_reserve_config_pair_guarded(
        &fixture.d(),
        &fixture.s(),
        &fixture.r(),
        request,
        Some(&cleanup),
        |decisions, state| {
            decisions.insert("decision".into(), json!(2));
            state.insert("caller".into(), json!(2));
            Ok(())
        },
        validate,
    )
}

type P22Artifacts = (Vec<u8>, Vec<u8>, Option<Vec<u8>>, Vec<(String, Vec<u8>)>);

fn p22_artifacts(fixture: &Fixture) -> P22Artifacts {
    let mut stages = std::fs::read_dir(fixture.root.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "tmp"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(path).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    stages.sort();
    (
        std::fs::read(fixture.d()).unwrap(),
        std::fs::read(fixture.s()).unwrap(),
        std::fs::read(fixture.r()).ok(),
        stages,
    )
}

fn p22_case(mode: usize, stop: Option<usize>) -> (usize, Result<(), PreparedPairError>) {
    let fixture = Fixture::new(false);
    let original_request = fixture.request();
    let mut proof = None;
    if mode != 0 {
        p22_prepare(&fixture, &original_request, &|| Ok(())).unwrap();
    }
    if [2, 4, 9, 10].contains(&mode) {
        assert!(fixture
            .interrupted_activity(T2, "activity_intent_durable")
            .is_err());
    }
    if mode == 5 {
        let mut state: Value =
            serde_json::from_slice(&std::fs::read(fixture.s()).unwrap()).unwrap();
        state["tooling"]["lastAgentMessageAt"] = json!(T3);
        fixture.write(&fixture.s(), state);
    }
    if [6, 7, 8].contains(&mode) {
        execute_reserved_config_pair(&fixture.d(), &fixture.s(), &fixture.r(), "operation-1")
            .unwrap();
        if mode == 7 {
            assert!(fixture
                .interrupted_activity(T3, "activity_intent_durable")
                .is_err());
        }
        proof = Some(
            PairCompleteProof::from_single_editor_complete(
                &fixture.complete_projection(),
                "operation-1",
                &fixture.packet().physical_target_identity,
                &json!({"kind":"bytes", "sha256":"b".repeat(64)}),
            )
            .unwrap(),
        );
        if mode == 8 {
            release_reserved_config_pair(
                &fixture.d(),
                &fixture.s(),
                &fixture.r(),
                proof.as_ref().unwrap(),
            )
            .unwrap();
        }
    }
    let calls = Cell::new(0);
    let at_failure = std::cell::RefCell::new(None);
    let validate = || {
        calls.set(calls.get() + 1);
        if stop == Some(calls.get()) {
            *at_failure.borrow_mut() = Some(p22_artifacts(&fixture));
            Err(PreparedPairError::Conflict)
        } else {
            Ok(())
        }
    };
    let result = match mode {
        0..=3 | 10 => p22_prepare(&fixture, &original_request, &validate).map(|_| ()),
        4..=5 => execute_reserved_config_pair_guarded(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            "operation-1",
            &validate,
        ),
        6 | 9 => verify_reserved_config_pair_guarded(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            "operation-1",
            &fixture.packet().plan_digest,
            &validate,
        )
        .map(|_| ()),
        7..=8 => release_reserved_config_pair_guarded(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            proof.as_ref().unwrap(),
            &validate,
        )
        .map(|_| ()),
        _ => unreachable!(),
    };
    if let Some(expected) = at_failure.borrow().as_ref() {
        assert_eq!(
            &p22_artifacts(&fixture),
            expected,
            "no target, packet or staging mutation after failed validation"
        );
        assert_eq!(result, Err(PreparedPairError::Conflict));
        assert_eq!(Some(calls.get()), stop, "failed guard remains failed");
    }
    (calls.get(), result)
}

#[test]
fn single_editor_pair_activity_every_reached_reservation_execution_verify_release_cut() {
    let mut total_cuts = 0;
    for mode in 0..11 {
        let (count, result) = p22_case(mode, None);
        if mode == 9 {
            assert_eq!(result, Err(PreparedPairError::Pending));
        } else {
            result.unwrap();
        }
        assert!(count > 0);
        for cut in 1..=count {
            assert_eq!(
                p22_case(mode, Some(cut)).1,
                Err(PreparedPairError::Conflict)
            );
            total_cuts += 1;
        }
    }
    assert!(
        total_cuts >= 11,
        "all eleven independently prepared paths ran"
    );
}

#[test]
fn p22_validation_after_mutation_before_first_packet_preserves_original_tuple() {
    let fixture = Fixture::new(false);
    let request = fixture.request();
    let before = fixture.tuple();
    let mutated = Cell::new(false);
    let result = prepare_and_reserve_config_pair_guarded(
        &fixture.d(),
        &fixture.s(),
        &fixture.r(),
        &request,
        None,
        |decisions, _| {
            decisions.insert("decision".into(), json!(2));
            mutated.set(true);
            Ok(())
        },
        &|| {
            if mutated.get() {
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        },
    );
    assert_eq!(result, Err(PreparedPairError::Conflict));
    assert_eq!(fixture.tuple(), before);
    assert!(!fixture.r().exists());
}

// Insert inside pair_activity_tests; existing source_guard cut fixture is retained.
#[test]
fn source_guard_postpublish_failure_preserves_exact_after_without_false_rollback() {
    for absent in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("source-guard-postpublish.json");
        if !absent {
            std::fs::write(&target, b"{\"value\":0}").unwrap();
        }
        let before = read_config_pair_physical(&target).unwrap();
        let after = b"{\"value\":1}";
        let rejected = Cell::new(false);
        let evidence = std::cell::RefCell::new(None);
        let result = publish_coordination_bytes_with_validation(&target, &before, after, &|| {
            if std::fs::read(&target).ok().as_deref() == Some(after.as_slice()) {
                rejected.set(true);
                *evidence.borrow_mut() =
                    Some((std::fs::read(&target).unwrap(), p22_stages(root.path())));
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err(PreparedPairError::Conflict));
        assert!(
            rejected.get(),
            "must reach a validation boundary after publication"
        );
        assert_eq!(
            (std::fs::read(&target).unwrap(), p22_stages(root.path())),
            evidence.borrow_mut().take().unwrap()
        );
        assert_eq!(std::fs::read(&target).unwrap(), after);
        // This fixture proves after-evidence retention at the adapter.
        // The driver must separately classify pending/conflict and forbid
        // complete until durable refs/reload/acks/receipt all succeed.
    }
}

#[test]
fn source_guard_callback_adapter_noop_absent_replace_and_every_failure_cut() {
    for mode in 0..3 {
        let setup = |root: &Path| {
            let target = root.join("source-guard.json");
            if mode != 1 {
                std::fs::write(&target, b"{\"value\":0}").unwrap();
            }
            let before = read_config_pair_physical(&target).unwrap();
            let after = if mode == 0 {
                b"{\"value\":0}".as_slice()
            } else {
                b"{\"value\":1}".as_slice()
            };
            (target, before, after)
        };
        let root = tempfile::tempdir().unwrap();
        let (target, before, after) = setup(root.path());
        let calls = Cell::new(0);
        publish_coordination_bytes_with_validation(&target, &before, after, &|| {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap();
        assert!(calls.get() > 0);
        assert_eq!(std::fs::read(&target).unwrap(), after);
        assert!(p22_stages(root.path()).is_empty());
        for cut in 1..=calls.get() {
            let root = tempfile::tempdir().unwrap();
            let (target, before, after) = setup(root.path());
            let reached = Cell::new(0);
            let observed = std::cell::RefCell::new(None);
            let result =
                publish_coordination_bytes_with_validation(&target, &before, after, &|| {
                    reached.set(reached.get() + 1);
                    if reached.get() == cut {
                        *observed.borrow_mut() =
                            Some((std::fs::read(&target).ok(), p22_stages(root.path())));
                        Err(PreparedPairError::Conflict)
                    } else {
                        Ok(())
                    }
                });
            assert_eq!(result, Err(PreparedPairError::Conflict));
            assert_eq!(reached.get(), cut);
            assert_eq!(
                (std::fs::read(&target).ok(), p22_stages(root.path())),
                observed.borrow_mut().take().unwrap(),
                "a rejected callback must forbid later publication and cleanup",
            );
        }
    }
}

#[test]
fn p22_publisher_failure_cuts_preserve_staging_and_no_false_rollback() {
    for absent in [false, true] {
        let setup = |root: &Path| {
            let target = root.join("pair-side.json");
            if !absent {
                std::fs::write(&target, b"{\"value\":0}").unwrap();
            }
            let before = read_config_pair_physical(&target).unwrap();
            (target, before)
        };
        let root = tempfile::tempdir().unwrap();
        let (target, before) = setup(root.path());
        let after = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
        let count = Cell::new(0);
        let allow = || {
            count.set(count.get() + 1);
            Ok(())
        };
        publish_prepared_pair_image_guarded(
            &target,
            &before,
            &after,
            &PairTargetGuard::new(&allow),
        )
        .unwrap();
        for cut in 1..=count.get() {
            let root = tempfile::tempdir().unwrap();
            let (target, before) = setup(root.path());
            let calls = Cell::new(0);
            let observed = std::cell::RefCell::new(None);
            let reject = || {
                calls.set(calls.get() + 1);
                if calls.get() == cut {
                    *observed.borrow_mut() =
                        Some((std::fs::read(&target).ok(), p22_stages(root.path())));
                    Err(PreparedPairError::Conflict)
                } else {
                    Ok(())
                }
            };
            let guard = PairTargetGuard::new(&reject);
            assert_eq!(
                publish_prepared_pair_image_guarded(&target, &before, &after, &guard),
                Err(PreparedPairError::Conflict)
            );
            assert_eq!(
                (std::fs::read(&target).ok(), p22_stages(root.path())),
                observed.borrow_mut().take().unwrap()
            );
            assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
            assert_eq!(calls.get(), cut);
        }
    }
}

#[test]
fn p22_cleanup_rejection_retains_diagnostic_stage_and_observed_drift() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("pair-side.json");
    std::fs::write(&target, b"{\"value\":0}").unwrap();
    let before = read_config_pair_physical(&target).unwrap();
    let after = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
    let drifted = Cell::new(false);
    let validate = || {
        if drifted.get() {
            Err(PreparedPairError::Conflict)
        } else {
            Ok(())
        }
    };
    let guard = PairTargetGuard::new(&validate);
    let mut next = next_shared_staging_sequence;
    let write = |file: &mut std::fs::File, bytes: &[u8]| {
        file.write_all(bytes).unwrap();
        std::fs::write(&target, b"{\"value\":2}").unwrap();
        drifted.set(true);
        Ok(())
    };
    assert_eq!(
        publish_prepared_pair_image_with_io(
            &target,
            &before,
            &after,
            &guard,
            &|_, _| panic!("validation failure is already sticky"),
            &mut next,
            &write
        ),
        Err(PreparedPairError::Conflict)
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"{\"value\":2}");
    assert_eq!(p22_stages(root.path()).len(), 1);
    assert_eq!(p22_stages(root.path())[0].1, b"{\"value\":1}");
}

#[test]
fn p22_allowing_guard_ordinary_adapter_packet_and_floor_parity() {
    // Same physical target avoids comparing path-dependent packet identities.
    let fixture = Fixture::new(false);
    let request = fixture.request();
    let first = p22_prepare(&fixture, &request, &|| Ok(())).unwrap();
    let observed = p22_prepare(&fixture, &request, &|| Ok(())).unwrap();
    assert_eq!(first, observed);
    execute_reserved_config_pair_guarded(
        &fixture.d(),
        &fixture.s(),
        &fixture.r(),
        "operation-1",
        &|| Ok(()),
    )
    .unwrap();
    let guarded_after = fixture.tuple();
    execute_reserved_config_pair(&fixture.d(), &fixture.s(), &fixture.r(), "operation-1").unwrap();
    assert_eq!(fixture.tuple(), guarded_after);
    assert_eq!(fixture.packet().timestamp_floor.as_deref(), Some(T1));
}

#[test]
fn p22_ordinary_and_guarded_reservation_same_packet_and_tuples() {
    let fixture = Fixture::new(false);
    let request = fixture.request();
    let ordinary = prepare_and_reserve_config_pair(
        &fixture.d(),
        &fixture.s(),
        &fixture.r(),
        &request,
        None,
        |decisions, _| {
            decisions.insert("decision".into(), json!(1));
            Ok(())
        },
    )
    .unwrap();
    let packet_bytes = std::fs::read(fixture.r()).unwrap();
    let before = fixture.tuple();
    let guarded = prepare_and_reserve_config_pair_guarded(
        &fixture.d(),
        &fixture.s(),
        &fixture.r(),
        &request,
        None,
        |decisions, _| {
            decisions.insert("decision".into(), json!(1));
            Ok(())
        },
        &|| Ok(()),
    )
    .unwrap();
    assert_eq!(ordinary, guarded);
    assert_eq!(std::fs::read(fixture.r()).unwrap(), packet_bytes);
    assert_eq!(fixture.tuple(), before);
}

#[test]
fn p22_primary_io_diagnostic_and_cleanup_refusal_both_retained() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("pair-side.json");
    std::fs::write(&target, b"{\"value\":0}").unwrap();
    let before = read_config_pair_physical(&target).unwrap();
    let after = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
    let io_failed = Cell::new(false);
    let validations = Cell::new(0);
    let validate = || {
        validations.set(validations.get() + 1);
        if io_failed.get() {
            Err(PreparedPairError::Conflict)
        } else {
            Ok(())
        }
    };
    let guard = PairTargetGuard::new(&validate);
    let diagnostics = std::cell::RefCell::new(Vec::new());
    let owned = std::cell::RefCell::new(None);
    let mut next = next_shared_staging_sequence;
    let write = |_: &mut std::fs::File, _: &[u8]| {
        let stages = p22_stages(root.path());
        assert_eq!(stages.len(), 1);
        *owned.borrow_mut() = Some(stages[0].0.clone());
        io_failed.set(true);
        Err(PreparedPairError::Io(
            "injected write failure after exclusive acquisition".into(),
        ))
    };
    let report = |primary: &PreparedPairError, refusal: &PreparedPairError| {
        assert!(matches!(primary, PreparedPairError::Io(_)));
        assert_eq!(refusal, &PreparedPairError::Conflict);
        diagnostics
            .borrow_mut()
            .push((primary.to_string(), refusal.to_string()));
    };
    assert_eq!(
        publish_prepared_pair_image_with_io(
            &target, &before, &after, &guard, &report, &mut next, &write
        ),
        Err(PreparedPairError::Conflict)
    );
    let calls = validations.get();
    assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
    assert_eq!(validations.get(), calls);
    let observed = diagnostics.into_inner();
    assert_eq!(observed.len(), 1);
    assert!(observed[0].0.starts_with("preparedPairIo:"));
    assert!(observed[0].1.starts_with("preparedPairConflict:"));
    assert_eq!(std::fs::read(&target).unwrap(), b"{\"value\":0}");
    assert_eq!(std::fs::read(owned.into_inner().unwrap()).unwrap(), b"");
}

#[cfg(not(windows))]
fn p22_os_publish_attempt(stage: &Path, target: &Path) -> Result<(), std::io::Error> {
    std::fs::rename(stage, target)
}

#[cfg(windows)]
fn p22_os_publish_attempt(stage: &Path, target: &Path) -> Result<(), std::io::Error> {
    replace_temp_config_once(&publish_path_wide(target), &publish_path_wide(stage))
}

#[test]
fn p22_transient_error_then_guard_refusal_has_no_second_os_attempt() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("pair-side.json");
    std::fs::write(&target, b"before").unwrap();
    let stage = p22_filled_stage(&target, b"after");
    let attempts = Cell::new(0);
    let validations = Cell::new(0);
    let delays = std::cell::RefCell::new(Vec::new());
    let validate = || {
        validations.set(validations.get() + 1);
        if attempts.get() > 0 {
            Err(PreparedPairError::Conflict)
        } else {
            Ok(())
        }
    };
    let guard = PairTargetGuard::new(&validate);
    let mut attempt = || {
        attempts.set(attempts.get() + 1);
        if attempts.get() == 1 {
            Err(std::io::Error::from_raw_os_error(
                TRANSIENT_PUBLISH_OS_ERRORS[1],
            ))
        } else {
            p22_os_publish_attempt(&stage, &target)
        }
    };
    let backoff = |duration| delays.borrow_mut().push(duration);
    assert_eq!(
        publish_temp_config_retry_with_attempt(&stage, &target, &guard, &mut attempt, &backoff,),
        Err(PreparedPairError::Conflict)
    );
    assert_eq!(attempts.get(), 1, "second OS primitive forbidden");
    assert_eq!(validations.get(), 3);
    assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
    assert_eq!(validations.get(), 3);
    assert_eq!(
        delays.into_inner(),
        vec![std::time::Duration::from_millis(PUBLISH_BACKOFFS_MS[0])]
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"before");
    assert_eq!(std::fs::read(&stage).unwrap(), b"after");
}

#[test]
fn p22_accepting_retry_and_ordinary_adapter_have_exact_success_parity() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("pair-side.json");
    for ordinary in [false, true] {
        std::fs::write(&target, b"before").unwrap();
        let stage = p22_filled_stage(&target, b"after");
        let attempts = Cell::new(0);
        let delays = std::cell::RefCell::new(Vec::new());
        let mut attempt = || {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                Err(std::io::Error::from_raw_os_error(
                    TRANSIENT_PUBLISH_OS_ERRORS[1],
                ))
            } else {
                p22_os_publish_attempt(&stage, &target)
            }
        };
        let backoff = |duration| delays.borrow_mut().push(duration);
        if ordinary {
            assert_eq!(
                publish_temp_config_with_attempt(&stage, &target, &mut attempt, &backoff,),
                Ok(())
            );
        } else {
            assert_eq!(
                publish_temp_config_retry_with_attempt(
                    &stage,
                    &target,
                    &PairTargetGuard::new(&|| Ok(())),
                    &mut attempt,
                    &backoff,
                ),
                Ok(())
            );
        }
        assert_eq!(attempts.get(), 2);
        assert_eq!(
            delays.into_inner(),
            vec![std::time::Duration::from_millis(PUBLISH_BACKOFFS_MS[0])]
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"after");
        assert!(!stage.exists());
    }
    // Ordinary production adapter still chooses the same host OS primitive.
    std::fs::write(&target, b"before").unwrap();
    let stage = p22_filled_stage(&target, b"after");
    assert_eq!(publish_temp_config(&stage, &target), Ok(()));
    assert_eq!(std::fs::read(&target).unwrap(), b"after");
    assert!(!stage.exists());
}
