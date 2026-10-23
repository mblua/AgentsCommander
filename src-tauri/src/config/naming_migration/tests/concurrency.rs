use super::*;

// -- E11, E12: two scopes, one journal -----------------------------------

fn two_scope_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("cfg");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    for dir in [&cfg, &a, &b] {
        std::fs::create_dir_all(dir).expect("dir");
    }
    seed_many(&a, "a");
    seed_many(&b, "b");
    (tmp, cfg, a, b)
}

fn spawn_scope_child(
    cfg: &Path,
    data: &Path,
    scope: &str,
    rendezvous: &Path,
    stage: &str,
) -> Child {
    spawn_child(
        "scope",
        &[
            (CHILD_DATA_ENV, data.as_os_str().to_owned()),
            (CHILD_JOURNAL_ENV, cfg.as_os_str().to_owned()),
            (CHILD_SCOPE_ENV, scope.into()),
            (pause::PAUSE_DIR_ENV, rendezvous.as_os_str().to_owned()),
            (pause::PAUSE_STAGE_ENV, stage.into()),
        ],
    )
}

#[test]
fn two_scopes_cannot_lose_each_others_records() {
    // Two threads, two data locks, one journal.
    let (_tmp, cfg, a, b) = two_scope_fixture();
    let threads: Vec<_> = [(a.clone(), "scope-a"), (b.clone(), "scope-b")]
        .into_iter()
        .map(|(data, scope)| {
            let cfg = cfg.clone();
            std::thread::spawn(move || run_scope(&data, &MANY, scope, Some(&cfg)))
        })
        .collect();
    for thread in threads {
        thread.join().expect("scope thread");
    }
    assert_many_complete(&cfg, &a, "scope-a", "a");
    assert_many_complete(&cfg, &b, "scope-b", "b");

    // Two real processes: the child stops after its first `Renaming`
    // commit while the parent commits a whole scope, then it finishes.
    let (tmp, cfg, a, b) = two_scope_fixture();
    let rendezvous = tmp.path().join("rendezvous");
    std::fs::create_dir_all(&rendezvous).expect("rendezvous");
    let child = spawn_scope_child(&cfg, &b, "scope-b", &rendezvous, "before_rename");
    wait_for_file(&rendezvous.join(pause::READY_FILE));
    run_scope(&a, &MANY, "scope-a", Some(&cfg));
    std::fs::write(rendezvous.join(pause::RELEASE_FILE), b"go").expect("release");
    wait_child(child);
    assert_many_complete(&cfg, &a, "scope-a", "a");
    assert_many_complete(&cfg, &b, "scope-b", "b");
}

#[test]
fn a_commit_survives_a_crash_in_the_other_scope() {
    let (tmp, cfg, a, b) = two_scope_fixture();
    let rendezvous = tmp.path().join("rendezvous");
    std::fs::create_dir_all(&rendezvous).expect("rendezvous");
    let mut child = spawn_scope_child(&cfg, &b, "scope-b", &rendezvous, "before_rename");
    wait_for_file(&rendezvous.join(pause::READY_FILE));
    // Killed between its `Renaming` commit and its rename.
    child.kill().expect("kill child");
    child.wait().expect("reap child");

    run_scope(&a, &MANY, "scope-a", Some(&cfg));
    assert_many_complete(&cfg, &a, "scope-a", "a");
    let b_steps = steps_of(&cfg, "scope-b");
    assert_eq!(b_steps.len(), 1, "{b_steps:?}");
    assert_eq!(b_steps[0].state, StepState::Renaming);
    assert_eq!(get(&b, MANY[0].from), Some(b"b:legacy-0.json".to_vec()));
}

// -- E13, E14: the scope lock --------------------------------------------

fn spawn_holder(data: &Path, rendezvous: &Path) -> Child {
    std::fs::create_dir_all(rendezvous).expect("rendezvous");
    let child = spawn_child(
        "hold",
        &[
            (CHILD_DATA_ENV, data.as_os_str().to_owned()),
            (CHILD_RENDEZVOUS_ENV, rendezvous.as_os_str().to_owned()),
        ],
    );
    wait_for_file(&rendezvous.join(pause::READY_FILE));
    child
}

#[test]
fn the_scope_lock_excludes_a_second_process_and_then_expires() {
    let f = fixture();
    put(&f.data, FROM, b"old");

    let rendezvous = f.cfg.join("hold-1");
    let child = spawn_holder(&f.data, &rendezvous);
    let started = Instant::now();
    let refused = run_scope_with(
        &f.data,
        &[STEP],
        None,
        SCOPE,
        Some(&f.cfg),
        MIGRATION_LOCK_BUDGET,
    );
    let waited = started.elapsed();
    assert_eq!(
        refused,
        Err(Refusal::LockUnavailable),
        "the lock did not exclude"
    );
    assert!(waited >= MIGRATION_LOCK_BUDGET, "gave up after {waited:?}");
    assert_eq!(get(&f.data, FROM), Some(b"old".to_vec()));
    assert_eq!(get(&f.data, TO), None);
    std::fs::write(rendezvous.join(pause::RELEASE_FILE), b"go").expect("release");
    wait_child(child);

    let rendezvous = f.cfg.join("hold-2");
    let child = spawn_holder(&f.data, &rendezvous);
    let release = rendezvous.join(pause::RELEASE_FILE);
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        std::fs::write(release, b"go").expect("release");
    });
    let started = Instant::now();
    let outcomes = run_scope_with(
        &f.data,
        &[STEP],
        None,
        SCOPE,
        Some(&f.cfg),
        MIGRATION_LOCK_BUDGET,
    )
    .expect("proceeds once released");
    assert!(started.elapsed() < MIGRATION_LOCK_BUDGET);
    assert_eq!(outcomes, vec![Outcome::Renamed]);
    releaser.join().expect("releaser");
    wait_child(child);
}

#[test]
fn lock_scope_under_held_does_not_re_acquire_the_callers_handle() {
    let f = fixture();
    let own = open_sidecar(&f.data.join(NEW_LOCK)).expect("open own lock");
    own.lock().expect("caller's own lock");
    let started = Instant::now();
    let held = lock_scope_under_held(&f.data, Some(OLD_LOCK), MIGRATION_LOCK_BUDGET)
        .expect("under-held lock");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "it waited on the caller's handle"
    );
    assert_eq!(held._files.len(), 1, "it took more than the old sidecar");
    let probe = open_sidecar(&f.data.join(OLD_LOCK)).expect("probe old");
    assert!(
        matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        "the old sidecar is not held"
    );
    drop(held);
    probe.try_lock().expect("released with the guard");
}

// -- E15 -----------------------------------------------------------------

#[test]
fn retire_ignore_pairs_is_byte_exact() {
    let content = "keep\r\n\r\n# AC: old rule\r\n/legacy.json\r\n/legacy.json\r\nuser\r\n\n\
                   # AC: other\n/other.tmp\ntail";
    let retired = [
        ("# AC: old rule", "/legacy.json"),
        ("# AC: other", "/other.tmp"),
    ];
    let (body, changed) = retire_ignore_pairs(content, &retired);
    assert!(changed);
    assert_eq!(body, "keep\r\n/legacy.json\r\nuser\r\ntail");

    let bare = "/legacy.json\n# AC: old rule\n";
    assert_eq!(
        retire_ignore_pairs(bare, &retired),
        (bare.to_string(), false)
    );
}

// -- E19, E20, E21 -------------------------------------------------------

#[test]
fn a_complete_scope_is_re_run_when_the_old_name_is_back() {
    for both in [false, true] {
        let f = fixture();
        put(&f.data, FROM, b"old");
        if both {
            put(&f.data, TO, b"new");
        }
        update_journal(Some(&f.cfg), |j| {
            j.record_step(
                SCOPE,
                StepRecord::new(&f.data, FROM, TO, StepState::Renamed),
            );
            j.set_status(SCOPE, ScopeStatus::Complete);
        })
        .expect("seed a stale Complete");
        assert!(is_complete(&journal(&f.cfg), SCOPE));

        let outcomes = run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg));
        if both {
            assert_eq!(
                outcomes,
                vec![Outcome::SetAside(aside(1))],
                "a stale Complete was skipped"
            );
            assert_eq!(get(&f.data, TO), Some(b"new".to_vec()));
            assert_eq!(get(&f.data, &aside(1)), Some(b"old".to_vec()));
        } else {
            assert_eq!(
                outcomes,
                vec![Outcome::Renamed],
                "a stale Complete was skipped"
            );
            assert_eq!(get(&f.data, TO), Some(b"old".to_vec()));
        }
        assert_eq!(get(&f.data, FROM), None);
        assert_complete(&f.cfg, SCOPE);
    }
}

fn files_under(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root).expect("list") {
        let entry = entry.expect("entry");
        if entry.file_type().expect("type").is_dir() {
            found.extend(files_under(&entry.path()));
        } else {
            found.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    found
}

#[test]
fn a_journal_less_run_decides_every_step_from_the_disk() {
    for disk in [
        Disk::FromOnly,
        Disk::ToOnly,
        Disk::Neither,
        Disk::Both,
        Disk::BothWithAside,
    ] {
        let with = fixture();
        let without = tempfile::tempdir().expect("tempdir");
        seed(&with.data, disk);
        seed(without.path(), disk);
        let journalled = run_scope(&with.data, &[STEP], SCOPE, Some(&with.cfg));
        let journal_less = run_scope(without.path(), &[STEP], SCOPE, None);
        assert_eq!(journalled, journal_less, "{disk:?}");
        assert_eq!(snapshot(&with.data), snapshot(without.path()), "{disk:?}");
        let stray: Vec<String> = files_under(without.path())
            .into_iter()
            .filter(|name| name.contains("naming-migration"))
            .collect();
        assert!(stray.is_empty(), "the journal-less run wrote {stray:?}");
    }
}

#[test]
fn a_repeated_step_record_is_replaced_not_appended() {
    let f = fixture();
    put(&f.data, FROM, b"old-0");
    put(&f.data, TO, b"new");
    let journal_len = || {
        std::fs::metadata(f.cfg.join(NAMING_MIGRATION_STATE_NAME))
            .expect("journal")
            .len()
    };
    let mut first_at = String::new();
    let mut last_at = String::new();
    let mut len_after_two = 0;
    for cycle in 1..=50u32 {
        assert_eq!(
            run_scope(&f.data, &[STEP], SCOPE, Some(&f.cfg)),
            vec![Outcome::SetAside(aside(cycle))]
        );
        let steps = steps_of(&f.cfg, SCOPE);
        assert_eq!(
            steps.len(),
            1,
            "cycle {cycle}: the record was appended, not replaced"
        );
        assert!(steps[0].at >= last_at, "cycle {cycle}: at went backwards");
        last_at = steps[0].at.clone();
        if cycle == 1 {
            first_at = last_at.clone();
        }
        if cycle == 2 {
            len_after_two = journal_len();
        }
        put(&f.data, FROM, format!("old-{cycle}").as_bytes());
    }
    assert!(last_at > first_at, "at never advanced");
    // The only growth allowed is the chosen name's ordinal, `-2` to `-50`.
    let ordinal_growth = ("50".len() - "2".len()) as u64;
    assert_eq!(
        journal_len(),
        len_after_two + ordinal_growth,
        "the journal grew"
    );
    for n in 1..=50 {
        assert_eq!(
            get(&f.data, &aside(n)),
            Some(format!("old-{}", n - 1).into_bytes())
        );
    }
}
