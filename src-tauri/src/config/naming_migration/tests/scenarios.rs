use super::*;

// -- E4 ------------------------------------------------------------------

#[test]
fn happy_path_renames_byte_for_byte_and_is_idempotent() {
    let f = fixture();
    put(&f.data, FROM, b"legacy bytes\r\n");
    put(&f.data, "legacy.backup.1.json", b"backup one");
    put(&f.data, "legacy.backup.2.json", b"backup two\n");
    let family = Some(("legacy.backup.", "target.backup.", ".json"));

    let outcomes = run_scope_with(
        &f.data,
        &[STEP],
        family,
        SCOPE,
        Some(&f.cfg),
        MIGRATION_LOCK_BUDGET,
    )
    .expect("first run");
    assert_eq!(outcomes, vec![Outcome::Renamed; 3]);
    assert_eq!(get(&f.data, TO), Some(b"legacy bytes\r\n".to_vec()));
    assert_eq!(
        get(&f.data, "target.backup.1.json"),
        Some(b"backup one".to_vec())
    );
    assert_eq!(
        get(&f.data, "target.backup.2.json"),
        Some(b"backup two\n".to_vec())
    );
    for gone in [FROM, "legacy.backup.1.json", "legacy.backup.2.json"] {
        assert_eq!(get(&f.data, gone), None, "{gone} survived");
    }
    assert_complete(&f.cfg, SCOPE);
    assert!(scope_is_settled(Some(&journal(&f.cfg)), SCOPE));

    let before = snapshot(&f.data);
    let again = run_scope_with(
        &f.data,
        &[STEP],
        family,
        SCOPE,
        Some(&f.cfg),
        MIGRATION_LOCK_BUDGET,
    )
    .expect("second run");
    assert!(again.is_empty(), "a settled scope ran again: {again:?}");
    let held = lock(&f.data);
    assert_eq!(
        rename_step(&f.data, &STEP, SCOPE, Some(&f.cfg), &held),
        Outcome::AlreadyDone
    );
    drop(held);
    assert_eq!(snapshot(&f.data), before, "the second run changed bytes");
}

// -- E5, E5b, E5c, E5d ---------------------------------------------------

#[test]
fn both_present_sets_the_old_file_aside_and_keeps_the_new_one() {
    let f = fixture();
    put(&f.data, FROM, b"old bytes");
    put(&f.data, TO, b"new bytes");
    prime_locks(&f.data);
    let before = snapshot(&f.data);

    let outcomes = run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg));
    assert_eq!(outcomes, vec![Outcome::SetAside(aside(1))]);
    let after = snapshot(&f.data);
    // A move: `from` leaves and the set-aside name arrives, so the count
    // holds. One fewer would be a deletion.
    assert_eq!(after.len(), before.len(), "an entry was lost: {after:?}");
    assert_eq!(after.get(TO), before.get(TO), "the new file was touched");
    assert_eq!(get(&f.data, FROM), None);
    assert_eq!(get(&f.data, &aside(1)), Some(b"old bytes".to_vec()));
    let steps = steps_of(&f.cfg, SCOPE);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].state, StepState::SetAside);
    assert_eq!(steps[0].set_aside.as_deref(), Some(aside(1).as_str()));
    assert_complete(&f.cfg, SCOPE);
}

#[test]
fn set_aside_picks_the_first_absent_ordinal() {
    let f = fixture();
    put(&f.data, FROM, b"old");
    put(&f.data, TO, b"new");
    put(&f.data, &aside(1), b"first");
    put(&f.data, &aside(3), b"third");
    assert_eq!(
        run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
        vec![Outcome::SetAside(aside(2))]
    );
    assert_eq!(get(&f.data, &aside(1)), Some(b"first".to_vec()));
    assert_eq!(get(&f.data, &aside(2)), Some(b"old".to_vec()));
    assert_eq!(get(&f.data, &aside(3)), Some(b"third".to_vec()));

    let g = fixture();
    put(&g.data, FROM, b"old");
    put(&g.data, TO, b"new");
    for n in 1..=999 {
        put(&g.data, &aside(n), n.to_string().as_bytes());
    }
    prime_locks(&g.data);
    let before = snapshot(&g.data);
    let outcomes = run_scope(&g.data, &[STEP], SCOPE, Some(&g.cfg));
    match outcomes.as_slice() {
        [Outcome::Refused(Refusal::Io(message))] => {
            assert!(message.contains("ceiling of 999"), "{message}")
        }
        other => panic!("the 999 ceiling must be an Io refusal: {other:?}"),
    }
    assert_eq!(
        snapshot(&g.data),
        before,
        "a file was touched at the ceiling"
    );
}

#[test]
fn an_outsider_created_ordinal_is_never_replaced() {
    let f = fixture();
    put(&f.data, FROM, b"old");
    put(&f.data, TO, b"new");
    prime_locks(&f.data);
    let base = snapshot(&f.data).len();

    let first = pause::arm("before_set_aside_move", &f.data);
    let (data, cfg) = (f.data.clone(), f.cfg.clone());
    let worker = std::thread::spawn(move || run_scope(&data, &[STEP], SCOPE, Some(&cfg)));
    first
        .reached
        .recv_timeout(WAIT_BOUND)
        .expect("first pause reached");
    assert_eq!(snapshot(&f.data).len(), base, "a placeholder was created");
    assert_eq!(get(&f.data, &aside(1)), None);
    let second = pause::arm("before_set_aside_move", &f.data);
    // A sync tool creates the candidate in the one gap before the move.
    put(&f.data, &aside(1), b"outsider bytes");
    first.release.send(()).expect("release first");
    second
        .reached
        .recv_timeout(WAIT_BOUND)
        .expect("second pause reached");
    assert_eq!(
        snapshot(&f.data).len(),
        base + 1,
        "a placeholder was created"
    );
    assert_eq!(get(&f.data, &aside(2)), None);
    second.release.send(()).expect("release second");
    let outcomes = worker.join().expect("worker");

    assert_eq!(
        get(&f.data, &aside(1)),
        Some(b"outsider bytes".to_vec()),
        "the outsider's file was replaced"
    );
    assert_eq!(outcomes, vec![Outcome::SetAside(aside(2))]);
    assert_eq!(get(&f.data, &aside(2)), Some(b"old".to_vec()));
    assert_eq!(get(&f.data, TO), Some(b"new".to_vec()));
    assert_eq!(
        steps_of(&f.cfg, SCOPE)[0].set_aside.as_deref(),
        Some(aside(2).as_str())
    );
    assert_complete(&f.cfg, SCOPE);
}

#[test]
fn a_failed_set_aside_move_never_loses_bytes_and_never_leaves_a_stub() {
    let f = fixture();
    put(&f.data, FROM, b"old");
    put(&f.data, TO, b"new");
    prime_locks(&f.data);
    let before = snapshot(&f.data);

    #[cfg(windows)]
    {
        let candidate = f.data.join(aside(1));
        arm_migration_path_fault(
            "move_no_replace",
            &candidate,
            io::ErrorKind::PermissionDenied,
        );
        let outcomes = run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg));
        match outcomes.as_slice() {
            [Outcome::Refused(Refusal::Io(message))] => {
                assert!(message.contains(&aside(1)), "{message}")
            }
            other => panic!("expected an Io refusal: {other:?}"),
        }
        assert_eq!(snapshot(&f.data), before, "entries or bytes changed");
    }

    #[cfg(unix)]
    {
        arm_migration_path_fault(
            "remove_file",
            &f.data.join(FROM),
            io::ErrorKind::PermissionDenied,
        );
        let outcomes = run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg));
        assert!(
            matches!(outcomes.as_slice(), [Outcome::Refused(Refusal::Io(_))]),
            "{outcomes:?}"
        );
        assert_eq!(get(&f.data, FROM), Some(b"old".to_vec()));
        assert_eq!(get(&f.data, &aside(1)), Some(b"old".to_vec()));
        assert_eq!(get(&f.data, TO), Some(b"new".to_vec()));
        assert_eq!(snapshot(&f.data).len(), before.len() + 1);
        let notes = journal(&f.cfg).scope(SCOPE).expect("scope").notes.clone();
        assert!(notes.iter().any(|n| n.contains("leftover")), "{notes:?}");
        assert_eq!(
            run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
            vec![Outcome::SetAside(aside(2))]
        );
        assert_complete(&f.cfg, SCOPE);
    }
}

#[test]
fn a_family_enumeration_error_refuses_and_the_scope_cannot_complete() {
    let f = fixture();
    put(&f.data, "legacy.backup.1.json", b"backup one");
    let family = Some(("legacy.backup.", "target.backup.", ".json"));
    arm_migration_path_fault("read_dir_entry", &f.data, io::ErrorKind::PermissionDenied);
    let outcomes = run_scope_with(
        &f.data,
        &[],
        family,
        SCOPE,
        Some(&f.cfg),
        MIGRATION_LOCK_BUDGET,
    )
    .expect("the scope ran");
    assert!(
        matches!(outcomes.as_slice(), [Outcome::Refused(Refusal::Io(_))]),
        "an unreadable entry was skipped: {outcomes:?}"
    );
    assert_ne!(status(&journal(&f.cfg), SCOPE), Some(ScopeStatus::Complete));
    assert!(!scope_is_settled(Some(&journal(&f.cfg)), SCOPE));
    assert_eq!(
        get(&f.data, "legacy.backup.1.json"),
        Some(b"backup one".to_vec())
    );

    let outcomes = run_scope_with(
        &f.data,
        &[],
        family,
        SCOPE,
        Some(&f.cfg),
        MIGRATION_LOCK_BUDGET,
    )
    .expect("the scope ran again");
    assert_eq!(outcomes, vec![Outcome::Renamed]);
    assert_eq!(
        get(&f.data, "target.backup.1.json"),
        Some(b"backup one".to_vec())
    );
    assert_complete(&f.cfg, SCOPE);
}

// -- E6, E7 --------------------------------------------------------------

#[test]
fn the_disk_decides_when_there_is_no_record() {
    const OTHER: Rename = Rename {
        from: "other.json",
        to: "other-target.json",
    };
    let cases = [
        (Disk::ToOnly, Outcome::AlreadyDone, StepState::Renamed),
        (
            Disk::Neither,
            Outcome::SourceAbsent,
            StepState::SourceAbsent,
        ),
        (Disk::FromOnly, Outcome::Renamed, StepState::Renamed),
    ];
    for (disk, outcome, state) in cases.clone() {
        let f = fixture();
        seed(&f.data, disk);
        update_journal(Some(&f.cfg), |_| ()).expect("empty journal");
        assert_eq!(
            run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
            vec![outcome],
            "{disk:?}"
        );
        assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, state, "{disk:?}");
        assert_complete(&f.cfg, SCOPE);
    }
    // The same three with the journal file deleted mid-scope.
    for (disk, outcome, state) in cases {
        let f = fixture();
        seed(&f.data, disk);
        let held = lock(&f.data);
        assert_eq!(
            rename_step(&f.data, &OTHER, SCOPE, Some(&f.cfg), &held),
            Outcome::SourceAbsent
        );
        std::fs::remove_file(f.cfg.join(NAMING_MIGRATION_STATE_NAME)).expect("delete journal");
        assert_eq!(
            rename_step(&f.data, &STEP, SCOPE, Some(&f.cfg), &held),
            outcome,
            "{disk:?}"
        );
        assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, state, "{disk:?}");
    }
}

#[test]
fn a_refused_step_re_evaluates_after_the_state_changes() {
    let f = fixture();
    put(&f.data, FROM, b"old");
    let holder = lock(&f.data);
    let refused = run_scope_with(
        &f.data,
        &[STEP],
        None,
        SCOPE,
        Some(&f.cfg),
        Duration::from_millis(200),
    );
    assert_eq!(refused, Err(Refusal::LockUnavailable));
    // What the caller records for a lock expiry.
    update_journal(Some(&f.cfg), |j| {
        let record = StepRecord {
            reason: Some("lock unavailable".into()),
            ..StepRecord::new(&f.data, FROM, TO, StepState::Refused)
        };
        j.record_step(SCOPE, record)
    })
    .expect("record refusal");
    assert_eq!(get(&f.data, TO), None);
    assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, StepState::Refused);
    drop(holder);

    assert_eq!(
        run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
        vec![Outcome::Renamed]
    );
    assert_eq!(get(&f.data, TO), Some(b"old".to_vec()));
    assert_complete(&f.cfg, SCOPE);
}

// -- E8, E9, E10: a crash at each of the three gaps ----------------------

/// Runs the scope on a thread armed at `stage` and abandons it there: the
/// release sender is dropped, the paused thread panics and its locks drop,
/// which is what a crash leaves behind.
fn crash_at(stage: &str, f: &Fixture, steps: &'static [Rename]) {
    let armed = pause::arm(stage, &f.data);
    let (data, cfg) = (f.data.clone(), f.cfg.clone());
    let worker = std::thread::spawn(move || run_scope(&data, steps, SCOPE, Some(&cfg)));
    armed
        .reached
        .recv_timeout(WAIT_BOUND)
        .expect("pause reached");
    drop(armed.release);
    assert!(worker.join().is_err(), "the abandoned leg must not finish");
}

#[test]
fn crash_before_the_rename_resumes_and_completes() {
    let f = fixture();
    put(&f.data, FROM, b"old");
    crash_at("before_rename", &f, &ONE_STEP);
    assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, StepState::Renaming);
    assert_eq!(get(&f.data, FROM), Some(b"old".to_vec()));
    assert_eq!(
        run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
        vec![Outcome::Renamed]
    );
    assert_eq!(get(&f.data, TO), Some(b"old".to_vec()));
    assert_complete(&f.cfg, SCOPE);
}

#[test]
fn crash_after_the_rename_is_not_read_as_a_pre_existing_destination() {
    let f = fixture();
    put(&f.data, FROM, b"old");
    crash_at("after_rename", &f, &ONE_STEP);
    assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, StepState::Renaming);
    assert_eq!(get(&f.data, FROM), None);
    let outcomes = run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg));
    assert_eq!(
        outcomes,
        vec![Outcome::AlreadyDone],
        "the crash was misclassified"
    );
    assert_eq!(steps_of(&f.cfg, SCOPE)[0].state, StepState::Renamed);
    assert_eq!(get(&f.data, TO), Some(b"old".to_vec()));
    assert_eq!(get(&f.data, &aside(1)), None);
    assert_complete(&f.cfg, SCOPE);
}

#[test]
fn crash_before_the_scope_completes_replays_nothing() {
    let f = fixture();
    seed_many(&f.data, "t");
    crash_at("before_scope_complete", &f, &MANY);
    assert_eq!(
        status(&journal(&f.cfg), SCOPE),
        Some(ScopeStatus::InProgress)
    );
    assert!(steps_of(&f.cfg, SCOPE)
        .iter()
        .all(|s| s.state == StepState::Renamed));
    let before = snapshot(&f.data);
    let outcomes = run_scope(&f.data, &MANY, SCOPE, Some(&f.cfg));
    assert!(
        outcomes.iter().all(|o| *o == Outcome::AlreadyDone),
        "{outcomes:?}"
    );
    assert_eq!(snapshot(&f.data), before);
    assert_many_complete(&f.cfg, &f.data, SCOPE, "t");
}
