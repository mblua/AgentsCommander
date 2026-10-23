use super::activity_stamp::T1;
use super::pair_fixture::{
    append_line, bytes_of, get_json, lines_of, marked, no_change, pair, put_json,
    sample_state_values, set_tooling, spawn_pair_child, split_marker, state_keys_in, text, tree_of,
    Pair, PairChild, CALLER_DECISIONS, CALLER_STATE, CLEANUP_STATE, CLEANUP_TRACKED, HOOK_BOUND,
    MUTANT_GAP,
};
use super::*;

#[test]
fn every_state_key_moves_without_loss() {
    // (a) decisions-only, per key.
    for (key, value) in sample_state_values() {
        let p = pair();
        p.seed_decisions(json!({ "tooling": { key: value.clone(), "profile": "keep" } }));
        p.write(no_change).expect("migrate");
        let state = p.stored_state();
        assert_eq!(
            text(&state["tooling"][key]),
            text(&value),
            "(a) {key} must land in the state file byte for byte"
        );
        assert_eq!(
            p.tracked(),
            json!({ "tooling": { "profile": "keep" } }),
            "(a) {key} must leave the decisions file and nothing else may"
        );
        assert_eq!(state["split"], split_marker(), "(a) {key}: marker present");
    }

    // (b) conflict on a pair that was never split: the state value survives.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "tracked" } }));
    p.seed_state(json!({ "tooling": { "lastCodingAgent": "state" } }));
    p.write(no_change).expect("migrate a conflict");
    assert_eq!(
        p.stored_state()["tooling"]["lastCodingAgent"],
        json!("state"),
        "(b) with no marker the state file's value must survive"
    );
    assert!(
        state_keys_in(&p.tracked()).is_empty(),
        "(b) the decisions file must lose its copy"
    );

    // (c) a second run on the migrated tree.
    let p = pair();
    let all: serde_json::Map<String, Value> = sample_state_values()
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    p.seed_decisions(json!({ "tooling": all.clone() }));
    p.write(no_change).expect("first run");
    let (tracked, state) = (bytes_of(&p.decisions()), bytes_of(&p.state()));
    p.write(no_change).expect("second run");
    assert_eq!(
        p.stored_state()["tooling"],
        Value::Object(all),
        "(c) a second run must keep every state value"
    );
    assert_eq!(
        (bytes_of(&p.decisions()), bytes_of(&p.state())),
        (tracked, state),
        "(c) a second run must write neither file"
    );

    // (d) no state file, and the decisions write is forced to fail.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
    let tracked = bytes_of(&p.decisions());
    std::fs::create_dir(p.decisions_temp()).expect("block the decisions publish");
    p.write_with_publish(no_change)
        .expect_err("(d) the decisions write must fail");
    assert_eq!(
        bytes_of(&p.state()).map(|_| p.stored_state()["tooling"]["lastCodingAgent"].clone()),
        Some(json!("claude")),
        "(d) the state file must already hold the value when the decisions write fails"
    );
    assert_eq!(
        bytes_of(&p.decisions()),
        tracked,
        "(d) decisions file unchanged"
    );
}

// -- E1b -----------------------------------------------------------------

#[test]
fn a_value_written_back_by_an_older_binary_wins() {
    /// The foreign `v2` is in the tracked file: a read returns it and
    /// writes nothing, and the next write leaves it in the state file.
    fn check(shape: &str, p: &Pair, journal: &Path, failures: &mut Vec<String>) {
        let before = tree_of(&p.dir);
        let read = p.read("lastCodingAgent");
        if read != json!("v2") {
            failures.push(format!("({shape}) a read must return v2, got {read}"));
        }
        if tree_of(&p.dir) != before {
            failures.push(format!("({shape}) a read must write nothing"));
        }
        if let Err(e) = update_agent_config_in(&p.decisions(), Some(journal), no_change) {
            failures.push(format!("({shape}) the next write failed: {e}"));
            return;
        }
        let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
        if state != json!("v2") {
            failures.push(format!(
                "({shape}) the next write must leave v2 in the state file, got {state}"
            ));
        }
        let left = state_keys_in(&p.tracked());
        if !left.is_empty() {
            failures.push(format!(
                "({shape}) the tracked file must hold no state key, holds {left:?}"
            ));
        }
    }
    let migrated = || {
        let p = pair();
        p.seed_decisions(json!({ "tooling": {} }));
        p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
        p
    };
    let mut failures = Vec::new();

    // (a) after a completed migration.
    let p = migrated();
    p.foreign_write("lastCodingAgent", json!("v2"));
    check("a", &p, &p.journal, &mut failures);

    // (c) a pair created after phase C, built by the entry point itself.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "profile": "keep" } }));
    p.write(|_, state| {
        set_tooling(state, "lastCodingAgent", json!("v1"));
        Ok(())
    })
    .expect("create the pair through the entry point");
    p.foreign_write("lastCodingAgent", json!("v2"));
    check("c", &p, &p.journal, &mut failures);

    // (d) two journal directories, neither consulted.
    let p = migrated();
    let other_journal = p.tmp.path().join("journal-b");
    std::fs::create_dir_all(&other_journal).expect("second journal dir");
    p.foreign_write("lastCodingAgent", json!("v2"));
    check("d, first journal", &p, &p.journal, &mut failures);
    p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
    p.foreign_write("lastCodingAgent", json!("v2"));
    check("d, second journal", &p, &other_journal, &mut failures);

    // (e) the journal write forced to fail: its directory is a file.
    let p = migrated();
    let broken_journal = p.tmp.path().join("journal-is-a-file");
    std::fs::write(&broken_journal, b"not a directory").expect("block the journal");
    p.foreign_write("lastCodingAgent", json!("v2"));
    check("e", &p, &broken_journal, &mut failures);

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// -- E1c -----------------------------------------------------------------

#[test]
fn a_first_write_that_also_mutates_keeps_its_new_value() {
    fn leg(name: &str, p: &Pair, new: &str, failures: &mut Vec<String>) {
        let mut child = spawn_pair_child(
            p,
            &[CALLER_STATE],
            &json!({ "state": { "lastCodingAgent": new } }),
            None,
        );
        child.kill_at(CALLER_STATE);
        let read = p.read("lastCodingAgent");
        if read != json!(new) {
            failures.push(format!(
                "({name}) after the kill a read must return {new}, got {read}"
            ));
        }
        p.write(no_change).expect("the parent's write");
        let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
        if state != json!(new) {
            failures.push(format!(
                "({name}) the surviving value must be {new}, the state file holds {state}"
            ));
        }
    }
    let mut failures = Vec::new();

    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v_old" } }));
    leg("migrate and mutate", &p, "v_new", &mut failures);

    // (c') the tracked file is still dirty from an earlier crash.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v0" } }));
    p.seed_state(json!({ "tooling": { "lastCodingAgent": "v0" } }));
    leg("dirty tracked file", &p, "v1", &mut failures);

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// -- E1d -----------------------------------------------------------------

#[test]
fn a_foreign_value_and_a_new_value_both_survive_their_own_sequence() {
    let foreign = || {
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v2" } }));
        p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
        p
    };
    let v3 = json!({ "state": { "lastCodingAgent": "v3" } });

    // Killed after our own write: the survivor is v3.
    let p = foreign();
    let stages = spawn_pair_child(&p, &[CALLER_STATE], &v3, None).kill_at(CALLER_STATE);
    assert_eq!(stages, [CLEANUP_STATE, CLEANUP_TRACKED, CALLER_STATE]);
    assert_eq!(p.read("lastCodingAgent"), json!("v3"));
    p.write(no_change).expect("re-run");
    assert_eq!(p.stored_state()["tooling"]["lastCodingAgent"], json!("v3"));

    // Killed inside the cleanup: the survivor is v2, not yet absorbed. This
    // is the preservation window: marker present beside a tracked state
    // key, and both files agree.
    let p = foreign();
    let stages = spawn_pair_child(&p, &[CLEANUP_STATE], &v3, None).kill_at(CLEANUP_STATE);
    assert_eq!(stages, [CLEANUP_STATE]);
    let state = p.stored_state();
    assert_eq!(state["tooling"]["lastCodingAgent"], json!("v2"));
    assert_eq!(
        state["split"],
        split_marker(),
        "the marker is never removed"
    );
    assert_eq!(p.tracked()["tooling"]["lastCodingAgent"], json!("v2"));
    assert_eq!(p.read("lastCodingAgent"), json!("v2"));
    p.write(no_change).expect("re-run");
    let state = p.stored_state();
    assert_eq!(state["tooling"]["lastCodingAgent"], json!("v2"));
    assert_eq!(state["split"], split_marker());
    assert!(state_keys_in(&p.tracked()).is_empty());

    // A state-only mutation: the decisions map does not change, so its
    // stage never fires and the child ends by itself.
    let p = pair();
    p.seed_decisions(json!({ "tooling": {} }));
    p.seed_state(marked(json!({ "lastCodingAgent": "v1" })));
    let mut child = spawn_pair_child(&p, &[CALLER_STATE, CALLER_DECISIONS], &v3, None);
    assert_eq!(
        child.first_ready(&[CALLER_STATE, CALLER_DECISIONS], HOOK_BOUND),
        Some(CALLER_STATE)
    );
    child.release(CALLER_STATE);
    assert!(child.exit().success(), "the state-only write must finish");
    assert_eq!(child.stages(), [CALLER_STATE]);
    assert!(!child.ready(CALLER_DECISIONS));
    assert_eq!(p.read("lastCodingAgent"), json!("v3"));
}

// -- E1f -----------------------------------------------------------------

#[test]
fn nothing_lands_between_the_cleanup_and_the_caller_sequence() {
    const LOCK_TIMEOUT: Duration = Duration::from_secs(60);
    const DISPATCH_BOUND: Duration = Duration::from_secs(2);
    const JOIN_BOUND: Duration = Duration::from_secs(5);

    fn join_within<T>(handle: std::thread::JoinHandle<T>, bound: Duration) -> Result<T, String> {
        let started = Instant::now();
        while !handle.is_finished() {
            if started.elapsed() >= bound {
                return Err(format!("the thread did not finish within {bound:?}"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        handle.join().map_err(|_| "the thread panicked".to_string())
    }

    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v1" } }));
    let sequence = p.tmp.path().join("sequence.log");
    let mut child = spawn_pair_child(
        &p,
        &[CLEANUP_TRACKED, MUTANT_GAP, CALLER_STATE],
        &json!({ "state": { "lastCodingAgent": "v3" } }),
        Some(&sequence),
    );

    // (Xt) abort: no more evidence is wanted, so take the one order that
    // cannot block. Both releases, kill and reap the child, then `go`, then
    // a bounded join.
    type Contender = std::thread::JoinHandle<Result<(), String>>;
    let abort = |child: &mut PairChild,
                 contender: Option<(std::sync::mpsc::Sender<()>, Contender)>,
                 why: String|
     -> ! {
        for stage in [CLEANUP_TRACKED, MUTANT_GAP, CALLER_STATE] {
            child.release(stage);
        }
        let last_stage = child.stages().last().cloned();
        let child_status = child.kill();
        let thread_status = contender.map(|(go, handle)| {
            let _ = go.send(());
            join_within(handle, JOIN_BOUND)
        });
        panic!(
            "{why}\nsequence: {:?}\nlast stage: {last_stage:?}\n\
             child: {child_status}\nthread: {thread_status:?}",
            lines_of(&sequence)
        );
    };

    // (1) the child pauses inside the cleanup with the guard held.
    if child.first_ready(&[CLEANUP_TRACKED], HOOK_BOUND) != Some(CLEANUP_TRACKED) {
        abort(
            &mut child,
            None,
            "the child never paused in the cleanup".into(),
        );
    }

    // (2) the competing writer, a pre-C binary: it takes the decisions
    // sidecar, then waits at `go` still holding it.
    let (go, go_gate) = std::sync::mpsc::channel::<()>();
    let lock_path = std::fs::canonicalize(&p.dir)
        .expect("canonical agent dir")
        .join(".config.json.lock");
    let (decisions_path, thread_sequence) = (p.decisions(), sequence.clone());
    let handle: Contender = std::thread::spawn(move || {
        append_line(&thread_sequence, "parent:acquiring");
        let _lock = crate::config::local_config_io::acquire_sidecar_write_lock(
            &lock_path,
            LOCK_TIMEOUT,
            "contenderLockTimeout",
            "contending config write lock",
        )?;
        append_line(&thread_sequence, "parent:acquired");
        go_gate
            .recv_timeout(HOOK_BOUND)
            .map_err(|e| format!("go was never released: {e}"))?;
        let mut tracked = get_json(&decisions_path);
        set_tooling(
            tracked.as_object_mut().expect("object"),
            "lastCodingAgent",
            json!("v2"),
        );
        put_json(&decisions_path, &tracked);
        append_line(&thread_sequence, "parent:published v2");
        Ok(())
    });
    let has = |line: &str| lines_of(&sequence).iter().any(|l| l == line);
    let started = Instant::now();
    while !has("parent:acquiring") {
        if started.elapsed() >= JOIN_BOUND {
            abort(
                &mut child,
                Some((go, handle)),
                "the thread never started".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // (3) secondary check: the lock is not taken while the cleanup pause
    // holds. It passes on both builds.
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(500) {
        if has("parent:acquired") {
            abort(
                &mut child,
                Some((go, handle)),
                "the lock was taken while the child held it in the cleanup".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // (4) the whole branch dispatch: which stage the child reports next.
    child.release(CLEANUP_TRACKED);
    match child.first_ready(&[CALLER_STATE, MUTANT_GAP], DISPATCH_BOUND) {
        // (5t) the child owns the lock: child first, thread second.
        Some(CALLER_STATE) => {
            child.kill();
            let _ = go.send(());
            match join_within(handle, HOOK_BOUND) {
                Ok(Ok(())) => {}
                other => panic!("the contender failed: {other:?}"),
            }
        }
        // (6t) the thread owns the lock: thread first, child second.
        Some(MUTANT_GAP) => {
            let _ = go.send(());
            match join_within(handle, HOOK_BOUND) {
                Ok(Ok(())) => {}
                other => abort(&mut child, None, format!("the contender failed: {other:?}")),
            }
            child.release(MUTANT_GAP);
            if child.first_ready(&[CALLER_STATE], DISPATCH_BOUND) != Some(CALLER_STATE) {
                abort(
                    &mut child,
                    None,
                    "the child never published after the gap".into(),
                );
            }
            child.kill();
        }
        other => abort(
            &mut child,
            Some((go, handle)),
            format!("no stage within 2 s of the cleanup release: {other:?}"),
        ),
    }

    // The blocking assertion, on the final marker file.
    let lines = lines_of(&sequence);
    let at = |line: &str| {
        lines
            .iter()
            .position(|l| l == line)
            .unwrap_or_else(|| panic!("{line} is missing from {lines:?}"))
    };
    assert!(
        at("parent:acquired") > at("child:published v3"),
        "parent:acquired must not appear before child:published v3: {lines:?}"
    );
    assert_eq!(
        lines,
        [
            "child:paused",
            "parent:acquiring",
            "child:published v3",
            "parent:acquired",
            "parent:published v2"
        ]
    );

    // End state: the tracked file holds v2, the newer write, and it wins.
    assert_eq!(p.tracked()["tooling"]["lastCodingAgent"], json!("v2"));
    p.write(no_change).expect("reconcile");
    assert_eq!(p.stored_state()["tooling"]["lastCodingAgent"], json!("v2"));
    assert!(state_keys_in(&p.tracked()).is_empty());
}

// -- E1e -----------------------------------------------------------------

#[test]
fn a_post_migration_write_survives_a_kill() {
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "profile": "old" } }));
    p.seed_state(marked(json!({ "lastCodingAgent": "v_old" })));
    let mut child = spawn_pair_child(
        &p,
        &[CALLER_DECISIONS],
        &json!({
            "state": { "lastCodingAgent": "v_new" },
            "decisions": { "profile": "new" }
        }),
        None,
    );
    child.kill_at(CALLER_DECISIONS);

    let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
    let profile = p.tracked()["tooling"]["profile"].clone();
    assert_eq!(
        state,
        json!("v_new"),
        "the state value was lost: the state file holds {state} while the decisions file \
         already holds profile {profile}"
    );
    assert_eq!(profile, json!("new"));
    assert!(state_keys_in(&p.tracked()).is_empty());
    assert_eq!(p.read("lastCodingAgent"), json!("v_new"));
}

// -- E2 ------------------------------------------------------------------

#[test]
fn the_state_file_wins_when_there_is_no_marker() {
    // First leg, in this process. The marker is stamped by the caller's
    // sequence, so it is not on disk when the cleanup has finished.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "A" } }));
    p.seed_state(json!({ "tooling": { "lastCodingAgent": "B" } }));
    let marker_after_cleanup = std::cell::RefCell::new(None);
    let on_stage = |stage: &str| {
        if stage == CLEANUP_TRACKED {
            *marker_after_cleanup.borrow_mut() = Some(p.stored_state().get("split").cloned());
        }
    };
    update_agent_config_with_stage(&p.decisions(), Some(&p.journal), &on_stage, no_change)
        .expect("migrate a conflict");
    let state = p.stored_state();
    assert_eq!(
        state["tooling"]["lastCodingAgent"],
        json!("B"),
        "with no marker the state file's value must survive"
    );
    assert!(state_keys_in(&p.tracked()).is_empty());
    assert_eq!(
        state["split"],
        split_marker(),
        "the caller's sequence stamps it"
    );

    // Second leg, E2 crossed with E4: three kills, three runs. The second
    // state key, held only by the tracked file, makes the state side
    // change, so every named stage is reachable. Both results of a kill
    // are collected before the row fails, so a failing build still shows
    // the read and the re-run.
    let mut failures = Vec::new();
    for kill in [CLEANUP_STATE, CLEANUP_TRACKED, CALLER_STATE] {
        // The third kill is control-only: a build that has already lost
        // the value may never fire that stage, so it is not awaited.
        if kill == CALLER_STATE && !failures.is_empty() {
            break;
        }
        let p = pair();
        p.seed_decisions(json!({ "tooling": {
            "lastCodingAgent": "A",
            "codingAgents": { "claude": { "app": "Claude Code", "lastUsed": T1 } }
        } }));
        p.seed_state(json!({ "tooling": { "lastCodingAgent": "B" } }));
        spawn_pair_child(&p, &[kill], &json!({}), None).kill_at(kill);
        let read = p.read("lastCodingAgent");
        if read != json!("B") {
            failures.push(format!(
                "killed at {kill}: a read must return B, got {read}"
            ));
        }
        p.write(no_change).expect("re-run");
        let state = p.stored_state()["tooling"]["lastCodingAgent"].clone();
        if state != json!("B") {
            failures.push(format!(
                "killed at {kill}: the re-run must leave B in the state file, it holds {state}"
            ));
        }
        println!("E2 killed at {kill}: read {read}, state after the re-run {state}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    assert_eq!(
        marker_after_cleanup.into_inner(),
        Some(None),
        "the cleanup's publish must not carry the marker"
    );
}

// -- E3 ------------------------------------------------------------------

#[test]
fn a_second_migration_writes_nothing() {
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude", "profile": "keep" } }));
    p.write(no_change).expect("migrate");
    let journal = p.journal_bytes();
    assert!(
        String::from_utf8_lossy(journal.as_deref().expect("the audit note"))
            .contains("lastCodingAgent"),
        "the first run records the keys it moved"
    );
    let before = (bytes_of(&p.decisions()), bytes_of(&p.state()));
    p.write(no_change).expect("second run");
    assert_eq!(
        (bytes_of(&p.decisions()), bytes_of(&p.state())),
        before,
        "a second run must write neither config file"
    );
    assert_eq!(p.journal_bytes(), journal, "the journal must not grow");

    // Rule 6: a decision-only write creates no state file.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "profile": "old" } }));
    p.write(|decisions, _state| {
        set_tooling(decisions, "profile", json!("new"));
        Ok(())
    })
    .expect("decision-only write");
    assert_eq!(p.tracked()["tooling"]["profile"], json!("new"));
    assert!(
        !p.state().exists(),
        "a decision-only write must create no state file"
    );
    assert_eq!(p.journal_bytes(), None, "nothing moved, so no note");
}

// -- E4 ------------------------------------------------------------------

#[test]
fn an_interrupted_move_leaves_the_value_in_both_files() {
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
    // Killed at whichever cleanup stage fires first, between the two writes.
    let mut child = spawn_pair_child(&p, &[CLEANUP_STATE, CLEANUP_TRACKED], &json!({}), None);
    let stage = child.first_ready(&[CLEANUP_STATE, CLEANUP_TRACKED], HOOK_BOUND);
    let status = child.kill();
    assert!(
        stage.is_some(),
        "the child never reached the cleanup: {status}"
    );

    let tracked = p.tracked()["tooling"]["lastCodingAgent"].clone();
    let state = bytes_of(&p.state())
        .map(|_| p.stored_state()["tooling"]["lastCodingAgent"].clone())
        .unwrap_or(Value::Null);
    assert_eq!(
        (&tracked, &state),
        (&json!("claude"), &json!("claude")),
        "after the kill at {stage:?} the value must be in both files: \
         tracked {tracked}, state {state}"
    );

    p.write(no_change).expect("re-run");
    assert_eq!(
        p.stored_state()["tooling"]["lastCodingAgent"],
        json!("claude")
    );
    assert!(
        state_keys_in(&p.tracked()).is_empty(),
        "after the re-run the value is only in the state file"
    );
}

// -- E5 ------------------------------------------------------------------

#[test]
fn a_refusal_keeps_every_value() {
    let unmigrated = || {
        let p = pair();
        p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "v_old", "profile": "old" } }));
        p
    };
    let both = |p: &Pair| (bytes_of(&p.decisions()), bytes_of(&p.state()));
    let mutate_both = |decisions: &mut JsonMap, state: &mut JsonMap| {
        set_tooling(state, "lastCodingAgent", json!("v_new"));
        set_tooling(decisions, "profile", json!("new"));
        Ok(())
    };

    // Leg 1, lock refusal: the one five-second wait of the battery.
    let p = unmigrated();
    let before = both(&p);
    let held = crate::config::local_config_io::acquire_sidecar_write_lock(
        &std::fs::canonicalize(&p.dir)
            .expect("canonical agent dir")
            .join(".config.json.lock"),
        Duration::from_secs(1),
        "testLockTimeout",
        "test lock",
    )
    .expect("hold the decisions sidecar");
    let err = p.write(no_change).expect_err("the lock refusal");
    drop(held);
    assert!(err.contains("configLockTimeout"), "{err}");
    assert_eq!(both(&p), before, "lock refusal: both files byte-identical");
    assert_eq!(p.read("lastCodingAgent"), json!("v_old"));

    // Leg 2, Io refusal inside the cleanup: the decisions publish fails
    // after the state write succeeded.
    let p = unmigrated();
    let tracked = bytes_of(&p.decisions());
    std::fs::create_dir(p.decisions_temp()).expect("block the decisions publish");
    p.write_with_publish(no_change).expect_err("the Io refusal");
    assert_eq!(bytes_of(&p.decisions()), tracked, "Io: decisions unchanged");
    assert_eq!(
        p.stored_state()["tooling"]["lastCodingAgent"],
        json!("v_old"),
        "Io: the state file carries the copied key"
    );
    assert_eq!(p.read("lastCodingAgent"), json!("v_old"));
    std::fs::remove_dir(p.decisions_temp()).expect("unblock");
    p.write(no_change).expect("the next run converges");
    assert_eq!(
        p.stored_state()["tooling"]["lastCodingAgent"],
        json!("v_old")
    );
    assert_eq!(p.stored_state()["split"], split_marker());
    assert!(state_keys_in(&p.tracked()).is_empty());

    // Leg 3: a tracked `tooling` that is not an object is never repaired.
    let p = pair();
    p.seed_decisions(json!({ "tooling": 7 }));
    p.seed_state(json!({ "tooling": { "lastCodingAgent": "v_old" } }));
    let before = both(&p);
    let err = p.write(no_change).expect_err("a malformed tooling");
    assert!(err.contains("'tooling' must be a JSON object"), "{err}");
    assert_eq!(both(&p), before, "tooling: both files byte-identical");

    // Leg 4, the caller's partial publish: its state publish succeeds and
    // its decisions publish fails. The marker is present on the `Err`.
    let p = unmigrated();
    let block = |stage: &str| {
        if stage == CLEANUP_TRACKED {
            std::fs::create_dir(p.decisions_temp()).expect("block the decisions publish");
        }
    };
    update_agent_config_with_activity_and_publish(
        &p.decisions(),
        Some(&p.journal),
        &block,
        mutate_both,
        None,
        Some(&|tmp, target| p.publish(tmp, target)),
    )
    .expect_err("the caller's decisions publish");
    let state = p.stored_state();
    assert_eq!(state["tooling"]["lastCodingAgent"], json!("v_new"));
    assert_eq!(
        state["split"],
        split_marker(),
        "partial: the marker is present"
    );
    assert_eq!(
        p.tracked(),
        json!({ "tooling": { "profile": "old" } }),
        "partial: no state key in the tracked file, and the decision is at its old value"
    );
    assert_eq!(p.read("lastCodingAgent"), json!("v_new"));
    std::fs::remove_dir(p.decisions_temp()).expect("unblock");

    // Leg 5, the caller's state publish fails: nothing of its sequence
    // reaches disk.
    let p = unmigrated();
    let block = |stage: &str| {
        if stage == CLEANUP_TRACKED {
            std::fs::create_dir(p.state_temp()).expect("block the state publish");
        }
    };
    update_agent_config_with_activity_and_publish(
        &p.decisions(),
        Some(&p.journal),
        &block,
        mutate_both,
        None,
        Some(&|tmp, target| p.publish(tmp, target)),
    )
    .expect_err("the caller's state publish");
    assert_eq!(
        p.stored_state(),
        json!({ "tooling": { "lastCodingAgent": "v_old" } }),
        "state publish: the cleanup's value and no marker"
    );
    assert_eq!(p.tracked(), json!({ "tooling": { "profile": "old" } }));
    assert_eq!(p.read("lastCodingAgent"), json!("v_old"));
    std::fs::remove_dir(p.state_temp()).expect("unblock");
    p.write(no_change).expect("the next call");
    assert_eq!(
        p.stored_state()["split"],
        split_marker(),
        "the next call stamps it"
    );

    // Variant: `mutate` fails on a pair that already carries the marker.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "profile": "old" } }));
    p.seed_state(marked(json!({ "lastCodingAgent": "v_old" })));
    let before = both(&p);
    let err = p
        .write(|_, _| Err("the caller refused".to_string()))
        .expect_err("a failing mutate");
    assert_eq!(err, "the caller refused");
    assert_eq!(both(&p), before, "mutate: both files byte-identical");
    assert_eq!(p.stored_state()["split"], split_marker());
    assert_eq!(p.read("lastCodingAgent"), json!("v_old"));
}

// -- E5b -----------------------------------------------------------------

#[test]
fn a_read_only_tree_is_not_modified() {
    let p = pair();
    let all: serde_json::Map<String, Value> = sample_state_values()
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    p.seed_decisions(json!({ "tooling": all }));
    let before = tree_of(p.tmp.path());

    for (key, value) in sample_state_values() {
        assert_eq!(p.read(key), value, "{key} must come back");
    }
    let typed = read_agent_local_config(&p.dir).expect("typed read");
    assert_eq!(typed.tooling.last_coding_agent.as_deref(), Some("claude"));

    let after = tree_of(p.tmp.path());
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>(),
        "a read must create or remove no file"
    );
    assert!(after == before, "a read must modify no file");
}

// -- E5c -----------------------------------------------------------------

#[test]
fn a_journal_failure_does_not_change_the_outcome() {
    let migrated = |p: &Pair| {
        let state = p.stored_state();
        assert_eq!(state["tooling"]["lastCodingAgent"], json!("claude"));
        assert_eq!(state["split"], split_marker());
        assert!(state_keys_in(&p.tracked()).is_empty());
    };

    // The journal directory is a file, so the note cannot be written.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
    let broken_journal = p.tmp.path().join("journal-is-a-file");
    std::fs::write(&broken_journal, b"not a directory").expect("block the journal");
    let result = update_agent_config_in(&p.decisions(), Some(&broken_journal), no_change);
    assert_eq!(
        result,
        Ok(()),
        "a lost audit note must not fail a completed write"
    );
    migrated(&p);
    assert_eq!(
        std::fs::read(&broken_journal).expect("the blocking file"),
        b"not a directory"
    );

    // No journal directory: same outcome, no note attempted.
    let p = pair();
    p.seed_decisions(json!({ "tooling": { "lastCodingAgent": "claude" } }));
    assert_eq!(
        update_agent_config_in(&p.decisions(), None, no_change),
        Ok(())
    );
    migrated(&p);
    assert_eq!(p.journal_bytes(), None);
}
