use super::*;
// Each fixture owns its temporary directory. Production never scans debris.
fn p22_stages(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut stages: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "tmp") && path.is_file()
        })
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    stages.sort_by(|left, right| left.0.cmp(&right.0));
    stages
}

// A fresh positive-control fixture per call; no state is shared between tests.
fn calibrate_source_native_b(after: &[u8]) -> (usize, usize) {
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let parent = crate::path_identity::retain_directory(root.path()).unwrap();
    let target = parent.identity().canonical_path.join("native-target.json");
    let calls = Cell::new(0usize);
    let pre = Cell::new(0usize);
    let post = Cell::new(0usize);
    let stage = std::cell::RefCell::new(None);
    let observe = || {
        let ordinal = calls.get() + 1;
        calls.set(ordinal);
        let stages = p22_stages(root.path());
        if !target.exists() {
            if stages.len() == 1 && stages[0].1 == after {
                pre.set(ordinal);
                *stage.borrow_mut() = Some((
                    stages[0].0.clone(),
                    parent.verify_regular_file(&stages[0].0).unwrap(),
                ));
            }
        } else if post.get() == 0 {
            assert_eq!(std::fs::read(&target).unwrap(), after);
            assert!(stages.is_empty());
            post.set(ordinal);
            let staged = stage.borrow();
            let (path, identity) = staged.as_ref().unwrap();
            assert!(!path.exists());
            assert!(crate::path_identity::same_object(
                identity,
                &parent.verify_regular_file(&target).unwrap()
            ));
        }
        Ok(())
    };
    let published = publish_source_coordination_bytes_with_validation(
        &target,
        &PhysicalState::Absent,
        after,
        &parent,
        &observe,
    );
    assert_eq!(published.result, Ok(()));
    assert!(!published.guard_rejected);
    let landed = parent.verify_regular_file(&target).unwrap();
    assert!(crate::path_identity::same_object(
        published.published_identity.as_ref().unwrap(),
        &landed
    ));
    assert_eq!(std::fs::read(&target).unwrap(), after);
    assert!(p22_stages(root.path()).is_empty());
    assert!(pre.get() > 0);
    assert_eq!(post.get(), pre.get() + 1);
    (pre.get(), post.get())
}

#[test]
fn source_native_b_collision_and_foreign_identical_preserve_target_and_cleanup_owned_stage() {
    let after: &[u8] = br#"{"native":true}"#;
    let (pre_move, post_move) = calibrate_source_native_b(after);
    for foreign in [br#"{"foreign":true}"#.as_slice(), after] {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let sentinel = root.path().join("foreign-sentinel.tmp");
        let parent = crate::path_identity::retain_directory(root.path()).unwrap();
        let target = parent.identity().canonical_path.join("native-target.json");
        let calls = Cell::new(0usize);
        let phases = std::cell::RefCell::new(Vec::new());
        let owned = std::cell::RefCell::new(None);
        let foreign_identity = std::cell::RefCell::new(None);
        let sentinel_identity = std::cell::RefCell::new(None);
        let interfere = || {
            let ordinal = calls.get() + 1;
            calls.set(ordinal);
            if ordinal == pre_move {
                phases.borrow_mut().push("preMove");
                assert!(!target.exists());
                let stages = p22_stages(root.path());
                assert_eq!(stages.len(), 1);
                assert_eq!(stages[0].1, after);
                let identity = parent.verify_regular_file(&stages[0].0).unwrap();
                *owned.borrow_mut() = Some((stages[0].0.clone(), identity.clone()));
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .unwrap();
                file.write_all(foreign).unwrap();
                file.sync_all().unwrap();
                drop(file);
                let foreign_object = parent.verify_regular_file(&target).unwrap();
                assert!(!crate::path_identity::same_object(
                    &identity,
                    &foreign_object
                ));
                *foreign_identity.borrow_mut() = Some(foreign_object);
            } else if ordinal == post_move {
                phases.borrow_mut().push("postMove");
                assert_eq!(std::fs::read(&target).unwrap(), foreign);
                assert_eq!(
                    parent.verify_regular_file(&target).unwrap(),
                    *foreign_identity.borrow().as_ref().unwrap()
                );
                let stage = owned.borrow();
                let (path, identity) = stage.as_ref().unwrap();
                assert_eq!(std::fs::read(path).unwrap(), after);
                assert_eq!(parent.verify_regular_file(path).unwrap(), *identity);
            } else if ordinal == post_move + 1 {
                phases.borrow_mut().push("cleanupAuthority");
                assert!(owned.borrow().as_ref().unwrap().0.exists());
            } else if ordinal == post_move + 2 {
                phases.borrow_mut().push("cleanupOwnedIdentity");
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&sentinel)
                    .unwrap();
                file.write_all(b"foreign sentinel").unwrap();
                file.sync_all().unwrap();
                drop(file);
                *sentinel_identity.borrow_mut() =
                    Some(parent.verify_regular_file(&sentinel).unwrap());
            } else {
                assert!(ordinal < pre_move, "unexpected callback after cleanup");
                assert!(!target.exists());
            }
            Ok(())
        };
        let published = publish_source_coordination_bytes_with_validation(
            &target,
            &PhysicalState::Absent,
            after,
            &parent,
            &interfere,
        );
        assert_eq!(published.result, Err(PreparedPairError::Conflict));
        assert!(!published.guard_rejected);
        assert!(published.published_identity.is_none());
        assert_eq!(
            *phases.borrow(),
            [
                "preMove",
                "postMove",
                "cleanupAuthority",
                "cleanupOwnedIdentity"
            ]
        );
        assert_eq!(calls.get(), post_move + 2);
        assert_eq!(std::fs::read(&target).unwrap(), foreign);
        assert_eq!(
            parent.verify_regular_file(&target).unwrap(),
            *foreign_identity.borrow().as_ref().unwrap()
        );
        assert!(!owned.borrow().as_ref().unwrap().0.exists());
        assert_eq!(
            parent.verify_regular_file(&sentinel).unwrap(),
            *sentinel_identity.borrow().as_ref().unwrap()
        );
        assert_eq!(
            p22_stages(root.path()),
            vec![(sentinel, b"foreign sentinel".to_vec())]
        );
    }
}

#[test]
fn source_native_b_postmove_guard_refusal_preserves_after_without_ownership_or_cleanup() {
    let after: &[u8] = br#"{"native":true}"#;
    let (pre_move, post_move) = calibrate_source_native_b(after);
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let sentinel = root.path().join("foreign-sentinel.tmp");
    let parent = crate::path_identity::retain_directory(root.path()).unwrap();
    let target = parent.identity().canonical_path.join("native-target.json");
    let calls = Cell::new(0usize);
    let phases = std::cell::RefCell::new(Vec::new());
    let owned = std::cell::RefCell::new(None);
    let snapshot = std::cell::RefCell::new(None);
    let refuse = || {
        let ordinal = calls.get() + 1;
        calls.set(ordinal);
        if ordinal == pre_move {
            phases.borrow_mut().push("preMove");
            assert!(!target.exists());
            let stages = p22_stages(root.path());
            assert_eq!(stages.len(), 1);
            assert_eq!(stages[0].1, after);
            *owned.borrow_mut() = Some((
                stages[0].0.clone(),
                parent.verify_regular_file(&stages[0].0).unwrap(),
            ));
        } else if ordinal == post_move {
            phases.borrow_mut().push("postMoveRefusal");
            let landed = parent.verify_regular_file(&target).unwrap();
            let stage = owned.borrow();
            let (path, identity) = stage.as_ref().unwrap();
            assert!(!path.exists());
            assert!(crate::path_identity::same_object(identity, &landed));
            assert_eq!(std::fs::read(&target).unwrap(), after);
            assert!(p22_stages(root.path()).is_empty());
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&sentinel)
                .unwrap();
            file.write_all(b"foreign sentinel").unwrap();
            file.sync_all().unwrap();
            drop(file);
            *snapshot.borrow_mut() = Some((
                landed,
                std::fs::read(&target).unwrap(),
                p22_stages(root.path()),
                parent.verify_regular_file(&sentinel).unwrap(),
            ));
            return Err(PreparedPairError::Conflict);
        } else {
            assert!(
                ordinal < pre_move,
                "unexpected callback after authority refusal"
            );
            assert!(!target.exists());
        }
        Ok(())
    };
    let published = publish_source_coordination_bytes_with_validation(
        &target,
        &PhysicalState::Absent,
        after,
        &parent,
        &refuse,
    );
    assert_eq!(published.result, Err(PreparedPairError::Conflict));
    assert!(published.guard_rejected);
    assert!(published.published_identity.is_none());
    assert_eq!(*phases.borrow(), ["preMove", "postMoveRefusal"]);
    assert_eq!(calls.get(), post_move);
    assert_eq!(
        (
            parent.verify_regular_file(&target).unwrap(),
            std::fs::read(&target).unwrap(),
            p22_stages(root.path()),
            parent.verify_regular_file(&sentinel).unwrap()
        ),
        snapshot.into_inner().unwrap()
    );
    assert!(!owned.borrow().as_ref().unwrap().0.exists());
}

#[test]
fn source_native_b_premove_guard_refusal_retains_owned_stage_without_effect() {
    let after: &[u8] = br#"{"native":true}"#;
    let (pre_move, post_move) = calibrate_source_native_b(after);
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let sentinel = root.path().join("foreign-sentinel.tmp");
    let parent = crate::path_identity::retain_directory(root.path()).unwrap();
    let target = parent.identity().canonical_path.join("native-target.json");
    let calls = Cell::new(0usize);
    let phases = std::cell::RefCell::new(Vec::new());
    let snapshot = std::cell::RefCell::new(None);
    let refuse = || {
        let ordinal = calls.get() + 1;
        calls.set(ordinal);
        if ordinal == pre_move {
            phases.borrow_mut().push("preMoveRefusal");
            assert!(!target.exists());
            let stages = p22_stages(root.path());
            assert_eq!(stages.len(), 1);
            assert_eq!(stages[0].1, after);
            let identity = parent.verify_regular_file(&stages[0].0).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&sentinel)
                .unwrap();
            file.write_all(b"foreign sentinel").unwrap();
            file.sync_all().unwrap();
            drop(file);
            *snapshot.borrow_mut() = Some((
                stages[0].0.clone(),
                identity,
                p22_stages(root.path()),
                parent.verify_regular_file(&sentinel).unwrap(),
            ));
            return Err(PreparedPairError::Conflict);
        }
        assert!(
            ordinal < pre_move,
            "unexpected callback after authority refusal"
        );
        assert!(!target.exists());
        Ok(())
    };
    let published = publish_source_coordination_bytes_with_validation(
        &target,
        &PhysicalState::Absent,
        after,
        &parent,
        &refuse,
    );
    assert_eq!(published.result, Err(PreparedPairError::Conflict));
    assert!(published.guard_rejected);
    assert!(published.published_identity.is_none());
    assert_eq!(*phases.borrow(), ["preMoveRefusal"]);
    assert_eq!(calls.get(), pre_move);
    assert!(pre_move < post_move);
    assert!(!target.exists());
    let (stage, identity, stages, sentinel_object) = snapshot.into_inner().unwrap();
    assert_eq!(parent.verify_regular_file(&stage).unwrap(), identity);
    assert_eq!(std::fs::read(&stage).unwrap(), after);
    assert_eq!(p22_stages(root.path()), stages);
    assert_eq!(
        parent.verify_regular_file(&sentinel).unwrap(),
        sentinel_object
    );
    // Retained diagnostic stage is NOT cleanup or product GC proof.
    // Production never scans this debris; fixture TempDir Drop is fixture-only.
}

fn p22_filled_stage(target: &Path, bytes: &[u8]) -> PathBuf {
    let guard = PairTargetGuard::new(&|| Ok(()));
    let (stage, mut file) = acquire_fresh_config_stage(target, &guard).unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
    file.sync_all().unwrap();
    drop(file);
    stage
}

fn p22_next_writer(mode: usize, target: &Path) {
    match mode {
        0 => {
            update_config_json_object(target, true, |map| {
                map.insert("value".into(), json!(2));
                Ok(())
            })
            .unwrap();
        }
        1 => write_file_atomic(target, b"{\"value\":2}").unwrap(),
        2 => publish_pair_side(target, json!({"value":2}).as_object().unwrap()).unwrap(),
        3 => publish_prepared_pair_image(
            target,
            &read_config_pair_physical(target).unwrap(),
            &PhysicalState::from_bytes(b"{\"value\":2}".to_vec()),
        )
        .unwrap(),
        _ => unreachable!(),
    }
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(target).unwrap()).unwrap(),
        json!({"value":2})
    );
}

#[test]
fn p22_all_four_next_writers_leave_detached_legacy_debris_unchanged() {
    for mode in 0..4 {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.json");
        let debris = temp_config_path(&target);
        std::fs::write(&debris, b"detached stale bytes").unwrap();
        std::fs::write(&target, b"{\"value\":1}").unwrap();
        p22_next_writer(mode, &target);
        assert_eq!(std::fs::read(&debris).unwrap(), b"detached stale bytes");
        assert_eq!(
            p22_stages(root.path()),
            vec![(debris, b"detached stale bytes".to_vec())]
        );
    }
}

#[test]
fn p22_all_four_next_writers_preserve_refused_absent_publish_hardlink_alias() {
    for mode in 0..4 {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.json");
        let validate = || {
            if target.exists() {
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        };
        let guard = PairTargetGuard::new(&validate);
        let first = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
        assert_eq!(
            publish_prepared_pair_image_guarded(&target, &PhysicalState::Absent, &first, &guard),
            Err(PreparedPairError::Conflict)
        );
        let stages = p22_stages(root.path());
        assert_eq!(stages.len(), 1);
        let alias = stages[0].0.clone();
        assert_eq!(std::fs::read(&target).unwrap(), stages[0].1);
        p22_next_writer(mode, &target);
        assert_eq!(std::fs::read(&alias).unwrap(), b"{\"value\":1}");
        assert_eq!(p22_stages(root.path()), stages);
    }
}

#[test]
fn p22_atomic_candidate_collisions_regular_directory_hardlink_and_race() {
    for kind in 0..4 {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.json");
        let occupied = fresh_config_stage_path(&target, 0);
        let linked = root.path().join("linked-original");
        match kind {
            0 => std::fs::write(&occupied, b"occupied").unwrap(),
            1 => std::fs::create_dir(&occupied).unwrap(),
            2 => {
                std::fs::write(&linked, b"occupied").unwrap();
                std::fs::hard_link(&linked, &occupied).unwrap();
            }
            3 => {}
            _ => unreachable!(),
        }
        let mut calls = 0;
        let mut next = || {
            let value = calls;
            calls += 1;
            if kind == 3 && value == 0 {
                // Competitor creates the candidate before our atomic open.
                std::fs::write(&occupied, b"occupied").unwrap();
            }
            Ok(value)
        };
        let guard = PairTargetGuard::new(&|| Ok(()));
        match acquire_fresh_config_stage_with_sequence(&target, &guard, &mut next) {
            Ok((stage, mut file)) => {
                assert_eq!(stage, fresh_config_stage_path(&target, 1));
                assert_eq!(calls, 2);
                file.write_all(b"owned").unwrap();
                drop(file);
            }
            Err(PreparedPairError::Io(reason)) if kind == 1 => {
                // Some hosts report a directory as a non-AlreadyExists
                // open error; that precise error must surface immediately.
                assert!(reason.contains(&occupied.display().to_string()));
                assert_eq!(calls, 1);
            }
            other => panic!("unexpected collision result: {other:?}"),
        }
        if kind == 1 {
            assert!(occupied.is_dir());
        } else {
            assert_eq!(std::fs::read(&occupied).unwrap(), b"occupied");
        }
        if kind == 2 {
            assert_eq!(std::fs::read(linked).unwrap(), b"occupied");
        }
    }
}

#[test]
fn p22_exact_64_attempt_exhaustion_never_cleans_unacquired_candidates() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("config.json");
    for sequence in 0..MAX_STAGING_ATTEMPTS as u64 {
        std::fs::write(fresh_config_stage_path(&target, sequence), b"occupied").unwrap();
    }
    let original = p22_stages(root.path());
    let mut calls = 0;
    let mut next = || {
        let value = calls;
        calls += 1;
        Ok(value)
    };
    let guard = PairTargetGuard::new(&|| Ok(()));
    let result = publish_prepared_pair_image_with_io(
        &target,
        &PhysicalState::Absent,
        &PhysicalState::from_bytes(b"{}".to_vec()),
        &guard,
        &|_, _| panic!("no cleanup diagnostic without acquisition"),
        &mut next,
        &|_, _| panic!("no write without acquisition"),
    );
    assert!(matches!(result, Err(PreparedPairError::Io(reason)) if reason.contains("64 attempts")));
    assert_eq!(calls, 64);
    assert_eq!(p22_stages(root.path()), original);
    assert!(!target.exists());
}

#[test]
fn p22_sequence_overflow_never_wraps_or_creates_a_candidate() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let sequence = AtomicU64::new(u64::MAX - 1);
    assert_eq!(next_staging_sequence(&sequence), Ok(u64::MAX - 1));
    assert_eq!(sequence.load(Ordering::Relaxed), u64::MAX);
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("config.json");
    let guard = PairTargetGuard::new(&|| Ok(()));
    assert!(matches!(
        acquire_fresh_config_stage_with_sequence(&target, &guard, &mut || {
            next_staging_sequence(&sequence)
        }),
        Err(PreparedPairError::Io(_))
    ));
    assert_eq!(sequence.load(Ordering::Relaxed), u64::MAX);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn p22_allocator_refusal_before_each_candidate_is_sticky() {
    for cut in 1..=4 {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.json");
        let occupied = fresh_config_stage_path(&target, 0);
        std::fs::write(&occupied, b"occupied").unwrap();
        let calls = Cell::new(0);
        let validate = || {
            calls.set(calls.get() + 1);
            if calls.get() == cut {
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        };
        let guard = PairTargetGuard::new(&validate);
        let mut sequence = 0;
        let mut next = || {
            let value = sequence;
            sequence += 1;
            Ok(value)
        };
        assert!(matches!(
            acquire_fresh_config_stage_with_sequence(&target, &guard, &mut next),
            Err(PreparedPairError::Conflict)
        ));
        assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
        assert_eq!(calls.get(), cut);
        assert_eq!(
            p22_stages(root.path()),
            vec![(occupied, b"occupied".to_vec())]
        );
    }
}

#[test]
fn p22_fresh_staging_long_paths_all_four_writers() {
    let root = tempfile::tempdir().unwrap();
    let mut directory = root.path().to_path_buf();
    while directory.as_os_str().to_string_lossy().len() <= 300 {
        directory.push("long-path-component-0123456789");
    }
    std::fs::create_dir_all(&directory).unwrap();
    for mode in 0..4 {
        let target = directory.join(format!("config-{mode}.json"));
        std::fs::write(&target, b"{\"value\":1}").unwrap();
        p22_next_writer(mode, &target);
    }
    assert!(p22_stages(&directory).is_empty());
}

#[test]
fn p22_noncollision_acquisition_error_is_immediate_and_precise() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("missing-parent").join("config.json");
    let guard = PairTargetGuard::new(&|| Ok(()));
    let mut calls = 0;
    let mut next = || {
        calls += 1;
        Ok(0)
    };
    let result = publish_prepared_pair_image_with_io(
        &target,
        &PhysicalState::Absent,
        &PhysicalState::from_bytes(b"{}".to_vec()),
        &guard,
        &|_, _| panic!("no cleanup without acquisition"),
        &mut next,
        &|_, _| panic!("no write without acquisition"),
    );
    assert!(matches!(result, Err(PreparedPairError::Io(reason))
        if reason.contains(&fresh_config_stage_path(&target, 0).display().to_string())));
    assert_eq!(calls, 1);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn p22_observed_directory_witness_change_after_acquisition_preserves_stage() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("parent");
    std::fs::create_dir(&directory).unwrap();
    let witness = directory.join("fixture-directory-witness");
    std::fs::write(&witness, b"original witness").unwrap();
    let target = directory.join("config.json");
    std::fs::write(&target, b"{\"value\":0}").unwrap();
    let before = read_config_pair_physical(&target).unwrap();
    let validate = || {
        if std::fs::read(&witness).unwrap() == b"original witness" {
            Ok(())
        } else {
            Err(PreparedPairError::Conflict)
        }
    };
    let guard = PairTargetGuard::new(&validate);
    let mut next = next_shared_staging_sequence;
    let write = |file: &mut std::fs::File, bytes: &[u8]| {
        file.write_all(bytes).unwrap();
        // A fixture marker models an observed directory witness. It is
        // neither production identity proof nor a hostile-host claim.
        std::fs::write(&witness, b"changed witness").unwrap();
        Ok(())
    };
    assert_eq!(
        publish_prepared_pair_image_with_io(
            &target,
            &before,
            &PhysicalState::from_bytes(b"{\"value\":1}".to_vec()),
            &guard,
            &|_, _| panic!("sticky refusal skips cleanup"),
            &mut next,
            &write
        ),
        Err(PreparedPairError::Conflict)
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"{\"value\":0}");
    let stages = p22_stages(&directory);
    assert_eq!(stages.len(), 1);
    assert_eq!(stages[0].1, b"{\"value\":1}");
    assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
    assert_eq!(p22_stages(&directory), stages);
}

use serde_json::json;

const T1: &str = "2026-10-08T01:00:00Z";
const T2: &str = "2026-10-08T02:00:00Z";
const T3: &str = "2026-10-08T03:00:00Z";

#[test]
fn shared_staging_sequence_is_unique_across_ordinary_prepared_and_source_apis() {
    let fixture = Fixture::new(false);
    let target = fixture.d();
    let stages = std::cell::RefCell::new(HashSet::new());
    write_file_atomic_with_publish(&target, b"{}", |stage, path| {
        assert!(stages.borrow_mut().insert(stage.to_path_buf()));
        publish_temp_config(stage, path)
    })
    .unwrap();
    let observe = || {
        for (stage, _) in p22_stages(fixture.root.path()) {
            stages.borrow_mut().insert(stage);
        }
        Ok(())
    };
    let before = read_config_pair_physical(&target).unwrap();
    publish_prepared_pair_image_guarded(
        &target,
        &before,
        &PhysicalState::from_bytes(b"{\"value\":1}".to_vec()),
        &PairTargetGuard::new(&observe),
    )
    .unwrap();
    assert_eq!(stages.borrow().len(), 2);
    let parent = crate::path_identity::retain_directory(fixture.root.path()).unwrap();
    let before = read_config_pair_physical(&target).unwrap();
    let published = publish_source_coordination_bytes_with_validation(
        &target,
        &before,
        b"{\"value\":2}",
        &parent,
        &observe,
    );
    assert_eq!(published.result, Ok(()));
    assert_eq!(stages.borrow().len(), 3);
    assert!(p22_stages(fixture.root.path()).is_empty());
}

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new(absent_state: bool) -> Self {
        let fixture = Self {
            root: tempfile::tempdir().unwrap(),
        };
        fixture.write(
            &fixture.d(),
            json!({"decision":0,"cleanup":0,
            "tooling":{"lastAgentMessageAt":T1},"opaque":null}),
        );
        if !absent_state {
            fixture.write(
                &fixture.s(),
                json!({"cleanup":0,
                "tooling":{"lastAgentMessageAt":T1,
                    "codingAgents":{"legacy":{"opaque":true}}},"opaque":[1,2]}),
            );
        }
        fixture
    }
    fn d(&self) -> PathBuf {
        self.root.path().join("config.json")
    }
    fn s(&self) -> PathBuf {
        self.root.path().join("config.state.no-git.json")
    }
    fn r(&self) -> PathBuf {
        self.root
            .path()
            .join("config-identity-reservation.state.no-git.json")
    }
    fn write(&self, path: &Path, value: Value) {
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    fn tuple(&self) -> [PhysicalState; 2] {
        read_pair_tuple(&self.d(), &self.s()).unwrap()
    }
    fn packet(&self) -> PairReservation {
        read_pair_reservation(&self.r()).unwrap().unwrap().0
    }
    fn request(&self) -> PairReservationRequest {
        PairReservationRequest {
            operation_id: "operation-1".into(),
            source_physical_key: "source-key".into(),
            mappings_digest: "a".repeat(64),
            before_protected: protected_config_pair_revision(&self.tuple()).unwrap(),
        }
    }
    fn reserve(
        &self,
        request: &PairReservationRequest,
    ) -> Result<PairReservation, PreparedPairError> {
        let cleanup = |d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
            d.remove("cleanup");
            s.insert("cleanup".into(), json!(1));
            let old = d
                .get_mut("tooling")
                .and_then(Value::as_object_mut)
                .unwrap()
                .remove("lastAgentMessageAt");
            let tooling = s
                .entry("tooling")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            if let Some(value) = old {
                tooling.entry("lastAgentMessageAt").or_insert(value);
            }
            Ok(())
        };
        prepare_and_reserve_config_pair(
            &self.d(),
            &self.s(),
            &self.r(),
            request,
            Some(&cleanup),
            |d, s| {
                d.insert("decision".into(), json!(1));
                s.insert("caller".into(), json!(1));
                Ok(())
            },
        )
    }
    fn activity(&self, input: &str) -> Result<bool, String> {
        let landed = Cell::new(false);
        update_config_pair_guarded(
            &self.d(),
            &self.s(),
            &self.r(),
            &[],
            None,
            Some(&|_, _| panic!("pending activity cannot cleanup")),
            &|_| panic!("no ordinary stages"),
            |_, _| panic!("pending activity uses the exact leaf primitive"),
            &|_| panic!("pending activity cannot stamp"),
            Some((input, &landed)),
        )?;
        Ok(landed.get())
    }
    fn interrupted_activity(&self, input: &str, stop: &str) -> Result<bool, PreparedPairError> {
        let _writer = ConfigWriterActive::enter("pair_activity_fixture").unwrap();
        let _guard = lock_local_config_writes();
        let _d = acquire_config_file_write_lock(&self.d(), CONFIG_LOCK_TIMEOUT).unwrap();
        let _s = acquire_config_file_write_lock(&self.s(), CONFIG_LOCK_TIMEOUT).unwrap();
        let _r = acquire_config_file_write_lock(&self.r(), CONFIG_LOCK_TIMEOUT).unwrap();
        let (mut packet, mut physical) = read_pair_reservation(&self.r())?.unwrap();
        land_pair_activity(
            &self.d(),
            &self.s(),
            &self.r(),
            &mut packet,
            &mut physical,
            input,
            &|stage| {
                if stage == stop {
                    Err(PreparedPairError::Io("lost activity response".into()))
                } else {
                    Ok(())
                }
            },
        )
    }
    fn complete_projection(&self) -> Value {
        let packet = self.packet();
        let hashes = packet.protected_revision_digests().unwrap();
        let target = json!({"targetId":packet.physical_target_identity,
            "beforeProtectedRevision":{"kind":"bytes","sha256":hashes[0]},
            "afterProtectedRevision":{"kind":"bytes","sha256":hashes[1]},
            "planDigest":packet.plan_digest});
        let revision = json!({"kind":"bytes","sha256":"b".repeat(64)});
        json!({"schemaVersion":2,"physicalSourceKey":"source-key","operationId":"operation-1",
            "phase":"complete","afterSourceRevision":revision,"packetsPrepared":true,
            "sourcePublished":true,"refsVerified":true,"reloaded":true,"packets":[target],
            "receipt":{"operationId":"operation-1","sourceRevision":revision}})
    }
}

#[test]
fn pair_activity_atomic_packet_floor_and_locks_before_presence() {
    let f = Fixture::new(false);
    let request = f.request();
    let before = f.tuple();
    let packet = prepare_and_reserve_config_pair(&f.d(), &f.s(), &f.r(), &request, None, |d, _| {
        assert!(!f.r().exists(), "no reservation before complete plan");
        for path in [f.d(), f.s(), f.r()] {
            assert!(acquire_config_file_write_lock(&path, Duration::ZERO).is_err());
        }
        assert!(write_file_atomic(&f.d(), b"{}")
            .unwrap_err()
            .contains("Nested config write"));
        d.insert("decision".into(), json!(1));
        Ok(())
    })
    .unwrap();
    assert_eq!(f.tuple(), before);
    assert_eq!(f.packet(), packet);
    assert_eq!(packet.timestamp_floor.as_deref(), Some(T1));
    assert_eq!(packet.plan_images.stages.len(), 5);
    assert!(packet.activity_intent.is_none());
}

#[test]
fn pair_activity_occupied_legacy_stage_allows_packet_without_target_write() {
    let f = Fixture::new(false);
    let before = f.tuple();
    let debris = temp_config_path(&f.r());
    std::fs::create_dir(&debris).unwrap();
    let _packet = f.reserve(&f.request()).unwrap();
    assert!(f.r().is_file());
    assert!(debris.is_dir());
    assert_eq!(f.tuple(), before);
    for path in [f.d(), f.s(), f.r()] {
        drop(acquire_config_file_write_lock(&path, Duration::ZERO).unwrap());
    }
}

#[test]
fn single_editor_pair_activity_same_operation_original_and_incompatible_refusal() {
    let f = Fixture::new(false);
    let request = f.request();
    let original = f.reserve(&request).unwrap();
    let bytes = std::fs::read(f.r()).unwrap();
    assert_eq!(f.reserve(&request).unwrap(), original);
    assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
    assert_eq!(
        verify_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", &original.plan_digest),
        Err(PreparedPairError::Pending)
    );
    let mut foreign = request.clone();
    foreign.operation_id = "operation-2".into();
    assert_eq!(f.reserve(&foreign), Err(PreparedPairError::Conflict));
    let divergent =
        prepare_and_reserve_config_pair(&f.d(), &f.s(), &f.r(), &request, None, |d, _| {
            d.insert("decision".into(), json!(9));
            Ok(())
        });
    assert_eq!(divergent, Err(PreparedPairError::Conflict));
    assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
    execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
    assert_eq!(
        verify_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", &original.plan_digest)
            .unwrap(),
        f.tuple()
    );
}

#[test]
fn pair_activity_generic_noop_zero_write_and_history_selection_unknown_rejected() {
    let f = Fixture::new(false);
    f.reserve(&f.request()).unwrap();
    let before = f.tuple();
    let packet = std::fs::read(f.r()).unwrap();
    update_config_pair_guarded(
        &f.d(),
        &f.s(),
        &f.r(),
        &[],
        None,
        Some(&|_, _| panic!("no cleanup")),
        &|_| panic!("no publish"),
        |_, _| Ok(()),
        &|_| panic!("no stamp"),
        None,
    )
    .unwrap();
    for key in ["codingAgents", "configurationRef", "unknown"] {
        let error = update_config_pair_guarded(
            &f.d(),
            &f.s(),
            &f.r(),
            &[],
            None,
            None,
            &|_| {},
            |d, s| {
                if key == "configurationRef" {
                    d.insert(key.into(), json!({"changed":true}));
                } else {
                    s.insert(key.into(), json!({"changed":true}));
                }
                Ok(())
            },
            &|_| panic!("no stamp"),
            None,
        )
        .unwrap_err();
        assert!(error.contains("targetTransitionPending"), "{error}");
    }
    assert_eq!(f.tuple(), before);
    assert_eq!(std::fs::read(f.r()).unwrap(), packet);
    assert_eq!(
        execute_unreserved_config_pair(&f.d(), &f.s(), &f.r(), &f.packet().plan_images),
        Err(PreparedPairError::Pending)
    );
}

mod p22;
mod recovery;
